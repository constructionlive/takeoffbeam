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
	viewer_line(&ctx.doc, drawing, ctx.current_page, ctx.visible_region, &ctx.selected_ids)
}

/// What the estimator is looking at, sent (not shown) with each message.
pub(crate) fn viewer_line(doc: &Mutex<takeoff_core::Document>, drawing: &takeoff_pdf::Drawing, current_page: u32, visible_region: Option<(f64, f64, f64, f64)>, selected_ids: &[String]) -> String {
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
	let system = system_prompt(&file, drawing.page_count(), ctx.provider.vision);
	let provider = ctx.provider.clone();
	let mut tctx = ToolCtx::new(ctx.doc.clone(), drawing, &provider.model, Some(provider.clone()));
	tctx.events = Some(tx.clone());
	tctx.current_page = ctx.current_page;
	tctx.cancel = Some(cancel.clone());
	let hidden: HashSet<String> = ctx.doc.lock().data.hidden_layers.iter().cloned().collect();
	let _ = tctx.drawing.set_hidden_layers(&hidden);
	session.lock().messages.push(Msg::User { text: user_text, context: Some(ctx_line) });
	let tool_defs = openai_tools();
	let mut steps = 0;
	let mut failures_in_a_row = 0;
	while steps < MAX_STEPS {
		if cancel.load(Ordering::Relaxed) {
			break;
		}
		compact_if_needed(session, &system, &provider, tx);
		let messages = session.lock().api_messages(&system, &provider.model, provider.vision);
		let result = llm::stream(&provider, &messages, &tool_defs, cancel, |d| {
			let _ = tx.send(match d {
				Delta::Text(t) => AgentEvent::TextDelta(t),
				Delta::Reasoning(t) => AgentEvent::ReasoningDelta(t),
			});
		});
		let completion = match result {
			Ok(c) => c,
			Err(e) if e == "cancelled" => break,
			Err(e) => {
				failures_in_a_row += 1;
				if failures_in_a_row >= 2 {
					let _ = tx.send(AgentEvent::Error(e));
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
			let _ = tx.send(AgentEvent::ToolStarted { call_id: call.id.clone(), name: call.name.clone(), args: args.clone() });
			let (ok, mut text, images) = if args.get("_raw").is_some() {
				(false, format!("Your arguments for {} were not valid JSON: {}", call.name, call.arguments.chars().take(300).collect::<String>()), vec![])
			} else {
				match tools::call(&mut tctx, &call.name, args) {
					Ok(o) => (true, o.text, o.images),
					Err(e) => (false, format!("Error: {e}"), vec![]),
				}
			};
			clip_tool_text(&mut text);
			let mut tool_images = Vec::new();
			let mut captions = Vec::new();
			let mut first_thumb = None;
			for img in &images {
				if first_thumb.is_none() {
					first_thumb = thumb(&img.png);
				}
				captions.push(img.caption.clone());
				tool_images.push(ToolImage { png: Arc::new(img.png.clone()), width: img.width, height: img.height, caption: img.caption.clone() });
			}
			let summary = text.lines().next().unwrap_or("").chars().take(300).collect::<String>();
			let _ = tx.send(AgentEvent::ToolFinished { call_id: call.id.clone(), name: call.name.clone(), ok, summary, image: first_thumb.as_ref().map(|t| t.0.clone()) });
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
	if steps >= MAX_STEPS {
		let _ = tx.send(AgentEvent::Error(format!("Stopped after {MAX_STEPS} steps. Say \"continue\" to carry on.")));
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
		Msg::User { text, context } => (text.len() + context.as_ref().map_or(0, |c| c.len())) as u64 / 4 + 4,
		Msg::Assistant { text, tool_calls, .. } => (text.len() + tool_calls.iter().map(|c| c.arguments.len() + c.name.len()).sum::<usize>()) as u64 / 4 + 4,
		Msg::Tool { text, images, .. } => text.len() as u64 / 4 + images.len() as u64 * 1600 + 4,
	}
}
