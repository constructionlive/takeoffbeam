//! Live smoke test: one agent turn on a real drawing against a real model.
//! `cargo run --release -p takeoff-agent --example live -- <pdf> <page> <base_url> <model> <out_dir> "<prompt>"`
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use takeoff_agent::{start_turn, AgentContext, AgentEvent, ChatSession, ProviderConfig};
use takeoff_core::{Document, TakeoffData};
use takeoff_pdf::burn::{burn, BurnOptions};
use takeoff_pdf::{Drawing, FracRect};

fn main() {
	let a: Vec<String> = std::env::args().skip(1).collect();
	let (pdf, page, base, model, out, prompt) = (&a[0], a[1].parse::<u32>().unwrap(), &a[2], &a[3], &a[4], &a[5]);
	let d = Drawing::open(pdf).unwrap();
	let sizes: Vec<(f64, f64)> = (1..=d.page_count()).map(|p| d.page_size_pt(p).map(|(w, h)| (w as f64, h as f64)).unwrap()).collect();
	let doc = Arc::new(Mutex::new(Document::new(TakeoffData::default(), sizes, "tester")));
	let session = Arc::new(Mutex::new(ChatSession::new()));
	let ctx = AgentContext {
		doc: doc.clone(),
		pdf_path: pdf.into(),
		current_page: page,
		visible_region: None,
		selected_ids: vec![],
		provider: ProviderConfig { base_url: base.clone(), api_key: std::env::var("TAKEOFF_API_KEY").ok(), model: model.clone(), context_tokens: 32768, vision: true },
		workspace: None,
		others: vec![],
		attachments: vec![],
	};
	let t0 = Instant::now();
	let turn = start_turn(session.clone(), ctx, prompt.clone());
	let deadline = Duration::from_secs(std::env::var("LIVE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(1500));
	let mut text = String::new();
	let mut reasoning = 0usize;
	if std::env::var("LIVE_THINK").is_ok() {
		takeoff_agent::set_local_thinking(true);
	}
	loop {
		if t0.elapsed() > deadline && !turn.is_cancelled() {
			println!("\n[deadline] cancelling");
			turn.cancel();
		}
		match turn.events.recv_timeout(Duration::from_secs(5)) {
			Ok(AgentEvent::TextDelta(t)) => {
				text.push_str(&t);
				print!("{t}");
				std::io::stdout().flush().ok();
			}
			Ok(AgentEvent::ReasoningDelta(t)) => reasoning += t.len(),
			Ok(AgentEvent::ToolStarted { name, args, .. }) => println!("\n[{:>6.1}s] → {name} {}", t0.elapsed().as_secs_f32(), args.to_string().chars().take(300).collect::<String>()),
			Ok(AgentEvent::ToolFinished { name, ok, summary, image, .. }) => println!("[{:>6.1}s] ← {name} ok={ok} img={} {}", t0.elapsed().as_secs_f32(), image.is_some(), summary.chars().take(240).collect::<String>()),
			Ok(AgentEvent::Usage { prompt_tokens, completion_tokens }) => println!("[{:>6.1}s] usage {prompt_tokens}/{completion_tokens}", t0.elapsed().as_secs_f32()),
			Ok(AgentEvent::Error(e)) => println!("\nERROR: {e}"),
			Ok(AgentEvent::Done { steps }) => {
				println!("\n[{:>6.1}s] DONE in {steps} steps ({reasoning} chars of reasoning)", t0.elapsed().as_secs_f32());
				break;
			}
			Ok(_) => {}
			Err(_) => {}
		}
	}
	let dd = doc.lock();
	println!("\nscales: {:?}", dd.data.page_scales);
	for c in &dd.data.categories {
		let n = dd.data.marks.iter().filter(|m| m.category_id.as_deref() == Some(&c.id)).count();
		println!("category {} ({:?}): {n} marks", c.name, c.kind());
	}
	println!("measurements: {}", dd.data.measurements.len());
	let mut r = Drawing::open(pdf).unwrap().render(page, 200.0, FracRect { x: 0.05, y: 0.0, w: 0.5, h: 0.45 }, Some(2400)).unwrap();
	burn(&mut r.image, r.region, dd.page_size_pt(page), &dd.data, page, &BurnOptions::default());
	r.image.save(format!("{out}/live_result.png")).unwrap();
	std::fs::write(format!("{out}/live_session.json"), serde_json::to_string_pretty(&*session.lock()).unwrap()).unwrap();
}
