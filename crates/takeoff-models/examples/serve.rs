//! Live check: list installed models, fit, start llama-server, one completion, stop.
//! `cargo run --release -p takeoff-models --example serve -- [model-id] [ctx]`
use std::time::{Duration, Instant};

use takeoff_models::fit::CacheType;
use takeoff_models::server::{LlamaServer, ServerState};
use takeoff_models::{catalog, fit_for, fit_for_catalog, store};

fn main() {
	let args: Vec<String> = std::env::args().skip(1).collect();
	println!("llama-server: {:?}", takeoff_models::server::find_llama_server());
	println!("models dir: {}", store::models_dir().display());
	for c in catalog::builtin() {
		println!("catalog {:<28} {}", c.id, fit_for_catalog(&c, c.context_tokens).summary());
	}
	let installed = store::installed();
	for m in &installed {
		println!("installed {:<28} vision={} {:.1} GB {}", m.id, m.vision, m.size_bytes as f64 / 1e9, m.model_path.display());
	}
	let id = args.first().cloned().unwrap_or_else(|| "Qwen3.8-27B-Q4_K_M".into());
	let ctx: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(32768);
	let Some(model) = installed.iter().find(|m| m.id == id) else { println!("{id} not installed"); return };
	println!("fit: {}", fit_for(model, ctx, CacheType::F16).summary());
	let t = Instant::now();
	let mut srv = LlamaServer::start(model, ctx, CacheType::F16).expect("start");
	let state = srv.wait_ready(Duration::from_secs(180));
	println!("state {state:?} after {:?} at {}", t.elapsed(), srv.base_url());
	if state != ServerState::Ready {
		println!("log:\n{}", srv.log_tail(30).join("\n"));
		return;
	}
	let t = Instant::now();
	let body = serde_json::json!({"model": srv.model_id(), "messages": [{"role": "user", "content": "Reply with the single word: ready"}], "max_tokens": 16});
	let r = reqwest::blocking::Client::new().post(format!("{}/chat/completions", srv.base_url())).json(&body).timeout(Duration::from_secs(120)).send().unwrap();
	let v: serde_json::Value = r.json().unwrap();
	println!("completion in {:?}: {:?}", t.elapsed(), v["choices"][0]["message"]["content"]);
	let pid = srv.port();
	srv.stop();
	println!("stopped (port {pid}) state {:?}", srv.state());
}
