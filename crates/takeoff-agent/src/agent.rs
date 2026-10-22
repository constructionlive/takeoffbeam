//! The agent loop: stream a completion, run the tool calls it asks for, feed
//! the results (and their renders) back, until the model answers without
//! tools, the step cap is hit, or the user cancels.

use std::collections::HashSet;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossbeam_channel::{unbounded, Sender};
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::llm::{self, Delta};
use crate::prompt::{system_prompt, SUMMARY_PROMPT};
use crate::session::{estimate_tokens, ChatSession, Msg, ToolImage};
use crate::tools::{self, openai_tools, ToolCtx};
use crate::{AgentContext, AgentEvent, TurnHandle};

pub const MAX_STEPS: u32 = 200;
const TOOL_TEXT_LIMIT: usize = 16_000;

pub fn start_turn(session: Arc<Mutex<ChatSession>>, ctx: AgentContext, user_text: String) -> TurnHandle {
	let (tx, rx) = unbounded();
	let cancel = Arc::new(AtomicBool::new(false));
	let c2 = cancel.clone();
	std::thread::Builder::new()
		.name("takeoff-agent".into())
		.spawn(move || {
			let steps = run(&session, ctx, user_text, &tx, &c2);
			let _ = tx.send(AgentEvent::Done { steps });
		})
		.expect("spawn agent thread");
	TurnHandle { events: rx, cancel }
}

fn context_line(ctx: &AgentContext, drawing: &takeoff_pdf::Drawing) -> String {
	let others: Vec<String> = ctx.others.iter().filter_map(|o| o.path.file_name()).map(|f| f.to_string_lossy().into_owned()).collect();
	viewer_line(&ctx.doc, drawing, ctx.current_page, ctx.visible_region, &ctx.selected_ids, &others)
}

/// What the estimator is looking at, sent (not shown) with each message.
pub(crate) fn viewer_line(doc: &Mutex<takeoff_core::Document>, drawing: &takeoff_pdf::Drawing, current_page: u32, visible_region: Option<(f64, f64, f64, f64)>, selected_ids: &[String], others: &[String]) -> String {
	let n = drawing.page_count();
	let label = drawing.page_label(current_page).ok().flatten().filter(|l| *l != current_page.to_string());
	let mut s = format!("[Viewer: page {current_page} of {n}{}", label.map(|l| format!(" ({l})")).unwrap_or_default());
	if let Some(r) = visible_region {
		if r.2 * r.3 < 0.95 {
			s.push_str(&format!(" · zoomed to x={:.2} y={:.2} w={:.2} h={:.2}", r.0, r.1, r.2, r.3));
		}
	}
	if !selected_ids.is_empty() {
		let ids: Vec<&str> = selected_ids.iter().take(20).map(String::as_str).collect();
		s.push_str(&format!(" · selected: {}", ids.join(", ")));
	}
	let has_scale = doc.lock().data.scale_for(current_page).is_some();
	s.push_str(if has_scale { " · page is scaled" } else { " · page has no scale" });
	if let Some(m) = crate::tools::map::presence(drawing.path()) {
		s.push_str(&format!(" · {m}"));
	}
	if !others.is_empty() {
		let file = drawing.path().file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
		s.push_str(&format!(" · this tab: \"{file}\" · also open: {}", others.iter().map(|o| format!("\"{o}\"")).collect::<Vec<_>>().join(", ")));
	}
	s.push(']');
	s
}

/// Cap a tool's text so one result can't flood the context.
pub(crate) fn clip_tool_text(text: &mut String) {
	if text.len() > TOOL_TEXT_LIMIT {
		let cut = text.char_indices().nth(TOOL_TEXT_LIMIT).map(|(i, _)| i).unwrap_or(text.len());
		text.truncate(cut);
		text.push_str("\n… (truncated — narrow the request)");
	}
}

pub(crate) fn thumb(png: &[u8]) -> Option<(image::RgbaImage, Vec<u8>)> {
	let img = image::load_from_memory(png).ok()?.to_rgba8();
	let (w, h) = img.dimensions();
	let s = (480.0 / w.max(h) as f32).min(1.0);
	let small = image::imageops::resize(&img, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1), image::imageops::FilterType::Triangle);
	let mut out = Vec::new();
	small.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
	Some((small, out))
}

fn run(session: &Arc<Mutex<ChatSession>>, ctx: AgentContext, user_text: String, tx: &Sender<AgentEvent>, cancel: &Arc<AtomicBool>) -> u32 {
	let drawing = match takeoff_pdf::Drawing::open(&ctx.pdf_path) {
		Ok(d) => d,
		Err(e) => {
			let _ = tx.send(AgentEvent::Error(format!("could not open the drawing: {e}")));
			return 0;
		}
	};
	let ctx_line = context_line(&ctx, &drawing);
	let file = ctx.pdf_path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
	let system = system_prompt(&file, drawing.page_count(), ctx.provider.vision, ctx.workspace.as_deref());
	let provider = ctx.provider.clone();
	let mut tctx = ToolCtx::new(ctx.doc.clone(), drawing, &provider.model, Some(provider.clone()));
	tctx.events = Some(tx.clone());
	tctx.current_page = ctx.current_page;
	tctx.cancel = Some(cancel.clone());
	tctx.workspace = ctx.workspace.clone();
	if let Err(e) = tctx.set_others(ctx.others.clone()) {
		let _ = tx.send(AgentEvent::Error(e));
	}
	let hidden: HashSet<String> = ctx.doc.lock().data.hidden_layers.iter().cloned().collect();
	let _ = tctx.drawing.set_hidden_layers(&hidden);
	let ctx_line = ctx_line + &crate::session::attachments_note(&ctx.attachments, &ctx.pdf_path) + &crate::session::images_note(&ctx.images, provider.vision, false);
	session.lock().messages.push(Msg::User { text: user_text, context: Some(ctx_line), attachments: ctx.attachments.clone(), images: ctx.images.clone() });
	let mut tool_defs = openai_tools(tctx.workspace.is_some());
	tool_defs.push(delegate_def());
	let env = DelegateEnv { doc: ctx.doc.clone(), pdf: ctx.pdf_path.clone(), workspace: ctx.workspace.clone(), provider: provider.clone(), current_page: ctx.current_page };
	let steps = run_loop(session, &mut tctx, &system, &provider, &tool_defs, tx, cancel, MAX_STEPS, Some(&env));
	if steps >= MAX_STEPS {
		let _ = tx.send(AgentEvent::Error(format!("Stopped after {MAX_STEPS} steps. Say \"continue\" to carry on.")));
	}
	steps
}

/// What a `delegate` call needs to start subagents on the same drawing.
struct DelegateEnv {
	doc: Arc<Mutex<takeoff_core::Document>>,
	pdf: std::path::PathBuf,
	workspace: Option<std::path::PathBuf>,
	provider: crate::ProviderConfig,
	current_page: u32,
}

/// Subagents running at once, and the most one call may hand out.
const DELEGATE_PARALLEL: usize = 6;
const DELEGATE_MAX_TASKS: usize = 12;
const SUB_STEPS: u32 = 120;
const SUB_REPORT_CHARS: usize = 4000;

fn delegate_def() -> Value {
	json!({"type": "function", "function": {
		"name": "delegate",
		"description": "Hand pieces of a long job to subagents that work IN PARALLEL (up to 6 at once), each with a fresh context and the same drawing tools. They see nothing of this conversation: give each a self-contained brief (what to do, which pages, what to record with which tools, what to report back). Returns each one's final report. For jobs too big for one context: mapping a whole set, taking off many sheets.",
		"parameters": {"type": "object", "properties": {"tasks": {"type": "array", "items": {"type": "object", "properties": {
			"title": {"type": "string", "description": "Short name, e.g. \"Architectural sheets 1-12\"."},
			"brief": {"type": "string"}}, "required": ["title", "brief"]}}}, "required": ["tasks"], "additionalProperties": false}
	}})
}

const SUBAGENT_NOTE: &str = "\n\n# You are a subagent\n\nAnother agent handed you the brief below as part of a larger job, and other subagents are working on other parts at the same time. Nobody reads your messages until you finish, so do not ask questions or wait for answers: do the work with your tools, saving as you go, then end with a short report of what you did, what you recorded and what you could not resolve. Stay inside your brief.";

fn delegate(env: &DelegateEnv, args: &Value, tx: &Sender<AgentEvent>, cancel: &Arc<AtomicBool>) -> Result<String, String> {
	let tasks: Vec<(String, String)> = args
		.get("tasks")
		.and_then(Value::as_array)
		.ok_or("delegate needs tasks: [{title, brief}]")?
		.iter()
		.filter_map(|t| Some((t.get("title")?.as_str()?.trim().to_string(), t.get("brief")?.as_str()?.trim().to_string())))
		.filter(|(_, b)| !b.is_empty())
		.collect();
	if tasks.is_empty() {
		return Err("delegate needs tasks: [{title, brief}], each with a brief".into());
	}
	if tasks.len() > DELEGATE_MAX_TASKS {
		return Err(format!("at most {DELEGATE_MAX_TASKS} tasks per call; group the work or call again"));
	}
	let file = env.pdf.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
	let next = std::sync::atomic::AtomicUsize::new(0);
	let reports: Mutex<Vec<Option<String>>> = Mutex::new(vec![None; tasks.len()]);
	// Only what the estimator should see live passes through: the map and the
	// takeoff changing, and where the agents are looking.
	let (sub_tx, sub_rx) = unbounded::<AgentEvent>();
	let fwd = tx.clone();
	let forward = std::thread::spawn(move || {
		for e in sub_rx {
			if matches!(e, AgentEvent::DocChanged | AgentEvent::Viewport { .. } | AgentEvent::Usage { .. }) {
				let _ = fwd.send(e);
			}
		}
	});
	std::thread::scope(|s| {
		for _ in 0..DELEGATE_PARALLEL.min(tasks.len()) {
			s.spawn(|| loop {
				let i = next.fetch_add(1, Ordering::Relaxed);
				if i >= tasks.len() || cancel.load(Ordering::Relaxed) {
					break;
				}
				let report = run_subagent(env, &file, &tasks[i].1, &sub_tx, cancel);
				reports.lock()[i] = Some(match report {
					Ok(r) => r,
					Err(e) => format!("(failed: {e})"),
				});
			});
		}
	});
	drop(sub_tx);
	let _ = forward.join();
	let reports = reports.into_inner();
	let mut out = String::new();
	for (i, ((title, _), r)) in tasks.iter().zip(reports).enumerate() {
		out.push_str(&format!("## Subagent {} · {title}\n{}\n\n", i + 1, r.unwrap_or_else(|| "(not started: cancelled)".into())));
	}
	Ok(out.trim_end().to_string())
}

fn run_subagent(env: &DelegateEnv, file: &str, brief: &str, tx: &Sender<AgentEvent>, cancel: &Arc<AtomicBool>) -> Result<String, String> {
	let drawing = takeoff_pdf::Drawing::open(&env.pdf).map_err(|e| format!("could not open the drawing: {e}"))?;
	let pages = drawing.page_count();
	let system = system_prompt(file, pages, env.provider.vision, env.workspace.as_deref()) + SUBAGENT_NOTE;
	let mut tctx = ToolCtx::new(env.doc.clone(), drawing, &env.provider.model, Some(env.provider.clone()));
	tctx.events = Some(tx.clone());
	tctx.current_page = env.current_page;
	tctx.cancel = Some(cancel.clone());
	tctx.workspace = env.workspace.clone();
	let hidden: HashSet<String> = env.doc.lock().data.hidden_layers.iter().cloned().collect();
	let _ = tctx.drawing.set_hidden_layers(&hidden);
	let session = Arc::new(Mutex::new(ChatSession::new()));
	let line = viewer_line(&env.doc, &tctx.drawing, env.current_page, None, &[], &[]);
	session.lock().messages.push(Msg::User { text: brief.to_string(), context: Some(line), attachments: vec![], images: vec![] });
	let tool_defs = openai_tools(tctx.workspace.is_some());
	let steps = run_loop(&session, &mut tctx, &system, &env.provider, &tool_defs, tx, cancel, SUB_STEPS, None);
	let s = session.lock();
	let last = s.messages.iter().rev().find_map(|m| match m {
		Msg::Assistant { text, .. } if !text.trim().is_empty() => Some(text.trim().to_string()),
		_ => None,
	});
	let mut report = last.unwrap_or_else(|| "(no report)".into());
	if report.len() > SUB_REPORT_CHARS {
		let cut = report.char_indices().nth(SUB_REPORT_CHARS).map(|(i, _)| i).unwrap_or(report.len());
		report.truncate(cut);
		report.push('…');
	}
	if steps >= SUB_STEPS {
		report.push_str(&format!("\n(stopped after {SUB_STEPS} steps: the brief may not be finished)"));
	}
	Ok(report)
}

/// Stream, run the tools asked for, repeat. `env`: this agent may delegate.
#[allow(clippy::too_many_arguments)]
fn run_loop(session: &Arc<Mutex<ChatSession>>, tctx: &mut ToolCtx, system: &str, provider: &crate::ProviderConfig, tool_defs: &[Value], tx: &Sender<AgentEvent>, cancel: &Arc<AtomicBool>, max_steps: u32, env: Option<&DelegateEnv>) -> u32 {
	let mut steps = 0;
	let mut failures_in_a_row = 0;
	while steps < max_steps {
		if cancel.load(Ordering::Relaxed) {
			break;
		}
		compact_if_needed(session, system, provider, tx);
		let messages = session.lock().api_messages(system, &provider.model, provider.vision);
		let main = env.is_some();
		let result = llm::stream(provider, &messages, tool_defs, cancel, |d| {
			if main {
				let _ = tx.send(match d {
					Delta::Text(t) => AgentEvent::TextDelta(t),
					Delta::Reasoning(t) => AgentEvent::ReasoningDelta(t),
				});
			}
		});
		let completion = match result {
			Ok(c) => c,
			Err(e) if e == "cancelled" => break,
			Err(e) => {
				failures_in_a_row += 1;
				if failures_in_a_row >= 2 {
					if main {
						let _ = tx.send(AgentEvent::Error(e));
					} else {
						session.lock().messages.push(Msg::Assistant { text: format!("(stopped: the model failed twice: {e})"), reasoning: String::new(), tool_calls: vec![] });
					}
					break;
				}
				std::thread::sleep(std::time::Duration::from_millis(1200));
				continue;
			}
		};
		failures_in_a_row = 0;
		steps += 1;
		let _ = tx.send(AgentEvent::Usage { prompt_tokens: completion.prompt_tokens, completion_tokens: completion.completion_tokens });
		{
			let mut s = session.lock();
			s.prompt_tokens = completion.prompt_tokens.max(s.prompt_tokens);
			s.completion_tokens += completion.completion_tokens;
			s.messages.push(Msg::Assistant { text: completion.text.clone(), reasoning: completion.reasoning.clone(), tool_calls: completion.tool_calls.clone() });
		}
		if completion.tool_calls.is_empty() {
			break;
		}
		for call in &completion.tool_calls {
			if cancel.load(Ordering::Relaxed) {
				// Every tool call needs a result, or the next request is invalid.
				session.lock().messages.push(Msg::Tool { call_id: call.id.clone(), name: call.name.clone(), text: "Cancelled by the user.".into(), ok: false, images: vec![], image_captions: vec![], thumb: None });
				continue;
			}
			let args: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({"_raw": call.arguments}));
			if main {
				let _ = tx.send(AgentEvent::ToolStarted { call_id: call.id.clone(), name: call.name.clone(), args: args.clone() });
			}
			let (ok, mut text, images) = if args.get("_raw").is_some() {
				(false, format!("Your arguments for {} were not valid JSON: {}", call.name, call.arguments.chars().take(300).collect::<String>()), vec![])
			} else if call.name == "delegate" {
				match env.map(|e| delegate(e, &args, tx, cancel)) {
					Some(Ok(t)) => (true, t, vec![]),
					Some(Err(e)) => (false, format!("Error: {e}"), vec![]),
					None => (false, "Error: subagents can't delegate further; do the work yourself.".into(), vec![]),
				}
			} else {
				match tools::call(tctx, &call.name, args) {
					Ok(o) => (true, o.text, o.images),
					Err(e) => (false, format!("Error: {e}"), vec![]),
				}
			};
			if call.name != "delegate" {
				clip_tool_text(&mut text);
			}
			let mut tool_images = Vec::new();
			let mut captions = Vec::new();
			let mut first_thumb = None;
			for img in &images {
				if first_thumb.is_none() && main {
					first_thumb = thumb(&img.png);
				}
				captions.push(img.caption.clone());
				tool_images.push(ToolImage { png: Arc::new(img.png.clone()), width: img.width, height: img.height, caption: img.caption.clone() });
			}
			if main {
				let summary = text.lines().next().unwrap_or("").chars().take(300).collect::<String>();
				let _ = tx.send(AgentEvent::ToolFinished { call_id: call.id.clone(), name: call.name.clone(), ok, summary, image: first_thumb.as_ref().map(|t| t.0.clone()) });
			}
			session.lock().messages.push(Msg::Tool {
				call_id: call.id.clone(),
				name: call.name.clone(),
				text,
				ok,
				images: tool_images,
				image_captions: captions,
				thumb: first_thumb.map(|t| Arc::new(t.1)),
			});
		}
	}
	steps
}

/// Summarise older turns when the next request would pass 70% of the window.
fn compact_if_needed(session: &Arc<Mutex<ChatSession>>, system: &str, p: &crate::ProviderConfig, tx: &Sender<AgentEvent>) {
	let limit = (p.context_tokens.max(4096) as f64 * 0.7) as u64;
	let (est, msgs) = {
		let s = session.lock();
		let m = s.api_messages(system, &p.model, p.vision);
		(estimate_tokens(&m).max(s.prompt_tokens.min(estimate_tokens(&m) * 2)), s.messages.clone())
	};
	if est <= limit || msgs.len() < 6 {
		return;
	}
	// Keep the newest turns that fit in ~35% of the window, cut at a user message.
	let budget = (p.context_tokens as f64 * 0.35) as u64;
	let mut keep_from = msgs.len();
	let mut acc = 0;
	for i in (0..msgs.len()).rev() {
		acc += msg_tokens(&msgs[i]);
		if acc > budget {
			break;
		}
		if matches!(msgs[i], Msg::User { .. }) {
			keep_from = i;
		}
	}
	if keep_from == msgs.len() {
		// The latest user turn alone is too big: keep from its last user message anyway.
		keep_from = msgs.iter().rposition(|m| matches!(m, Msg::User { .. })).unwrap_or(0);
	}
	if keep_from == 0 {
		return;
	}
	let old = &msgs[..keep_from];
	let mut transcript = String::new();
	if let Some(s) = &session.lock().summary {
		transcript.push_str(&format!("Earlier summary:\n{s}\n\n"));
	}
	for m in old {
		match m {
			Msg::User { text, .. } => transcript.push_str(&format!("ESTIMATOR: {text}\n")),
			Msg::Assistant { text, tool_calls, .. } => {
				if !text.trim().is_empty() {
					transcript.push_str(&format!("BENCH: {text}\n"));
				}
				for c in tool_calls {
					transcript.push_str(&format!("  call {}({})\n", c.name, c.arguments.chars().take(200).collect::<String>()));
				}
			}
			Msg::Tool { name, text, .. } => transcript.push_str(&format!("  {name} → {}\n", text.chars().take(600).collect::<String>())),
		}
	}
	let req = vec![json!({"role": "system", "content": SUMMARY_PROMPT}), json!({"role": "user", "content": transcript})];
	let summary = match llm::complete_with(p, &req, Some(1200), true) {
		Ok((t, _, _)) if !t.trim().is_empty() => t,
		_ => format!("(Earlier turns were dropped to fit the model's context. {} messages removed.)", old.len()),
	};
	let mut s = session.lock();
	let n = keep_from.min(s.messages.len());
	s.messages.drain(..n);
	s.summary = Some(summary);
	s.prompt_tokens = 0;
	let _ = tx.send(AgentEvent::TextDelta(String::new()));
}

fn msg_tokens(m: &Msg) -> u64 {
	match m {
		Msg::User { text, context, .. } => (text.len() + context.as_ref().map_or(0, |c| c.len())) as u64 / 4 + 4,
		Msg::Assistant { text, tool_calls, .. } => (text.len() + tool_calls.iter().map(|c| c.arguments.len() + c.name.len()).sum::<usize>()) as u64 / 4 + 4,
		Msg::Tool { text, images, .. } => text.len() as u64 / 4 + images.len() as u64 * 1600 + 4,
	}
}
