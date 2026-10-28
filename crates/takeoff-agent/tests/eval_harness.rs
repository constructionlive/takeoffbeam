//! The eval harness against scripted models: a run that marks 9 of 9
//! receptacles but only 5 of 6 lights must score exactly that, and a probe
//! against a model that answers in Qwen's [x,y] grid must find it on target in
//! that dialect and off in the others. Also checks the committed synthetic
//! drawings and cases still match the generator (`TAKEOFF_BLESS=1` rewrites
//! the drawings).
//!
//! The `live_*` tests are ignored: they run the committed cases against a real
//! model. `TAKEOFF_BASE_URL=http://127.0.0.1:8080/v1 TAKEOFF_MODEL=… cargo
//! test -p takeoff-agent --test eval_harness -- --ignored --nocapture`
//! (`TAKEOFF_API_KEY`, `TAKEOFF_REPORT=out.json`, `TAKEOFF_DIALECTS=pixel,qwen3`
//! optional). `takeoff eval run|probe` does the same from the CLI.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use takeoff_agent::eval::{self, synth, Case, Report};
use takeoff_agent::ProviderConfig;
use takeoff_core::coords::Dialect;

fn evals_dir() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evals")
}

fn case(name: &str) -> Case {
	Case::load(&evals_dir().join("cases").join(name)).unwrap()
}

// ── A scripted OpenAI-compatible server ───────────────────────────────────

fn read_request(stream: &mut std::net::TcpStream) -> Value {
	let mut reader = BufReader::new(stream.try_clone().unwrap());
	let mut len = 0;
	loop {
		let mut line = String::new();
		reader.read_line(&mut line).unwrap();
		if line == "\r\n" || line.is_empty() {
			break;
		}
		if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
			len = v.trim().parse().unwrap();
		}
	}
	let mut body = vec![0u8; len];
	reader.read_exact(&mut body).unwrap();
	serde_json::from_slice(&body).unwrap()
}

/// Answer `n` requests with `reply(step, request) -> (content type, body)`.
/// Gives up after 30 s without one, so a test that asks less fails, not hangs.
fn serve(n: usize, reply: impl Fn(usize, &Value) -> (&'static str, String) + Send + 'static) -> (String, std::thread::JoinHandle<()>) {
	let listener = TcpListener::bind("127.0.0.1:0").unwrap();
	listener.set_nonblocking(true).unwrap();
	let base = format!("http://127.0.0.1:{}/v1", listener.local_addr().unwrap().port());
	let h = std::thread::spawn(move || {
		for step in 0..n {
			let t0 = std::time::Instant::now();
			let mut s = loop {
				match listener.accept() {
					Ok((s, _)) => break s,
					Err(_) if t0.elapsed() < Duration::from_secs(30) => std::thread::sleep(Duration::from_millis(10)),
					Err(e) => panic!("request {step} of {n} never came: {e}"),
				}
			};
			s.set_nonblocking(false).unwrap();
			let body = read_request(&mut s);
			let (ct, out) = reply(step, &body);
			let head = format!("HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len());
			s.write_all(head.as_bytes()).unwrap();
			s.write_all(out.as_bytes()).unwrap();
		}
	});
	(base, h)
}

fn sse(chunks: &[Value]) -> String {
	let mut s = String::new();
	for c in chunks {
		s.push_str(&format!("data: {c}\n\n"));
	}
	s.push_str("data: [DONE]\n\n");
	s
}

fn tool_call(id: &str, name: &str, args: Value) -> String {
	sse(&[
		json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}]}),
		json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 1000, "completion_tokens": 20}}),
	])
}

/// The newest view caption's pixel size, as a model reads it.
fn view_px(request: &Value, view: &str) -> (f64, f64) {
	let text = request.to_string();
	let at = text.rfind(&format!("View {view} ")).expect("view caption in the request");
	let px = text[at..].split(" · ").find_map(|p| p.split_once(" px").map(|x| x.0).filter(|x| x.contains('x') && x.chars().next().unwrap().is_ascii_digit())).unwrap();
	let (w, h) = px.split_once('x').unwrap();
	(w.parse().unwrap(), h.parse().unwrap())
}

/// The region a caption states (3 decimals, as the model sees it).
fn view_region(text: &str) -> (f64, f64, f64, f64) {
	let at = text.find("region x=").unwrap();
	let v: Vec<f64> = text[at + 7..].split_whitespace().take(4).map(|kv| kv.split_once('=').unwrap().1.parse().unwrap()).collect();
	(v[0], v[1], v[2], v[3])
}

fn pixel_boxes(boxes: &[[f64; 4]], (w, h): (f64, f64)) -> Vec<[f64; 4]> {
	boxes.iter().map(|b| [b[0] * w, b[1] * h, (b[0] + b[2]) * w, (b[1] + b[3]) * h]).collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[test]
fn synthetic_drawings_match_the_committed_ones() {
	let dir = evals_dir().join("drawings");
	if std::env::var_os("TAKEOFF_BLESS").is_some() {
		synth::write_all(&dir).unwrap();
	}
	for (name, bytes) in synth::drawings() {
		let committed = std::fs::read(dir.join(name)).unwrap_or_else(|_| panic!("evals/drawings/{name} missing: run with TAKEOFF_BLESS=1"));
		assert!(committed == bytes, "evals/drawings/{name} differs from synth.rs: run with TAKEOFF_BLESS=1 and update the cases");
		let d = takeoff_pdf::Drawing::open(dir.join(name)).unwrap();
		assert_eq!(d.page_size_pt(1).unwrap(), (synth::W as f32, synth::H as f32));
	}
	// The title blocks are real text, so read_text and the probe's text targets find them.
	let d = takeoff_pdf::Drawing::open(dir.join("rooms.pdf")).unwrap();
	for t in ["A-101", "FIRST FLOOR PLAN", "OFFICE", "12'-0\""] {
		assert_eq!(d.search_page(1, t, true, false, 10).unwrap().len(), 1, "{t}");
	}
}

#[test]
fn committed_cases_load_and_agree_with_the_generator() {
	let cases = eval::load_cases(&[evals_dir().join("cases")]).unwrap();
	assert!(cases.len() >= 3);
	for c in &cases {
		c.pdf_path().unwrap_or_else(|e| panic!("{}: {e}", c.name));
		assert!(c.task.is_some() || c.probe.is_some(), "{}", c.name);
	}
	let s = case("symbols.json");
	let probe = s.probe.unwrap();
	let close = |a: &[[f64; 4]], b: &[[f64; 4]]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.iter().zip(y).all(|(p, q)| (p - q).abs() < 2e-6));
	assert!(close(&probe.targets[0].boxes, &synth::receptacle_boxes()));
	assert!(close(&probe.targets[1].boxes, &synth::light_boxes()));
	// Private drawings come from an environment variable.
	let mut c = case("symbols.json");
	c.pdf = "$TAKEOFF_EVAL_NOT_SET_ANYWHERE/x.pdf".into();
	assert!(c.pdf_path().unwrap_err().contains("set TAKEOFF_EVAL_NOT_SET_ANYWHERE"));
}

#[test]
fn a_scripted_run_scores_what_it_marked() {
	let (base, server) = serve(4, |step, req| {
		let body = match step {
			0 => tool_call("c1", "view", json!({"page": 1})),
			1 => tool_call("c2", "add_marks", json!({"view": "v1", "boxes": pixel_boxes(&synth::receptacle_boxes(), view_px(req, "v1")), "category": "Duplex receptacles"})),
			// One light missed.
			2 => tool_call("c3", "add_marks", json!({"view": "v1", "boxes": pixel_boxes(&synth::light_boxes()[..5], view_px(req, "v1")), "category": "Light fixtures"})),
			_ => sse(&[json!({"choices": [{"delta": {"content": "Marked 9 receptacles and 5 light fixtures on sheet E-101."}, "finish_reason": "stop"}]})]),
		};
		("text/event-stream", body)
	});
	let opts = eval::RunOptions {
		provider: ProviderConfig { base_url: base, api_key: None, model: "test-model".into(), context_tokens: 32_768, vision: true },
		timeout: Duration::from_secs(60),
		workspace: None,
		keep: Some(std::env::temp_dir().join(format!("takeoff-eval-keep-{}", std::process::id()))),
		channel: None,
	};
	let mut events = 0;
	let r = eval::run_case(&case("symbols.json"), &opts, &mut |_| events += 1);
	server.join().unwrap();
	assert!(r.skipped.is_none() && r.error.is_none(), "{r:?}");
	assert_eq!((r.steps, r.tool_calls, r.tool_errors), (4, 3, 0));
	assert_eq!((r.prompt_tokens, r.completion_tokens), (3000, 60));
	assert_eq!(r.tools.get("add_marks"), Some(&2));
	assert_eq!((r.passed, r.total), (2, 3), "{:#?}", r.scores);
	let pass: Vec<(String, bool)> = r.scores.iter().map(|s| (s.what.clone(), s.pass)).collect();
	assert_eq!(pass, [("count receptacle".to_string(), true), ("count light".into(), false), ("answer sheet number".into(), true)]);
	assert!((r.scores[1].err_pct.unwrap() + 16.67).abs() < 0.01);
	assert!(r.answer.contains("E-101"));
	assert!(events > 4);
	let kept = opts.keep.as_ref().unwrap().join("symbols-count.takeoff.json");
	assert!(std::fs::read_to_string(&kept).unwrap().contains("Light fixtures"));
	std::fs::remove_dir_all(opts.keep.unwrap()).ok();

	// The report round-trips, prints, and compares against an earlier one.
	let mut rep = Report::new();
	rep.runs.push(r.clone());
	let back: Report = serde_json::from_str(&serde_json::to_string(&rep).unwrap()).unwrap();
	assert_eq!(back.runs[0].scores, r.scores);
	let table = eval::run_table(&back.runs);
	assert!(table.contains("symbols-count") && table.contains("2/3") && table.contains("-16.7%"), "{table}");
	let mut better = back.clone();
	better.runs[0].passed = 3;
	better.runs[0].scores[1].pass = true;
	better.runs[0].scores[1].got = "6".into();
	let diff = eval::compare(&back, &better);
	assert!(diff.contains("2/3 → 3/3 (+1)") && diff.contains("fixed count light: 5 → 6"), "{diff}");
}

#[test]
fn missing_drawings_are_skipped_not_failed() {
	let mut c = case("symbols.json");
	c.pdf = "nowhere/missing.pdf".into();
	let opts = eval::RunOptions { provider: ProviderConfig { base_url: "http://127.0.0.1:9/v1".into(), api_key: None, model: "m".into(), context_tokens: 8192, vision: true }, timeout: Duration::from_secs(5), workspace: None, keep: None, channel: None };
	let r = eval::run_case(&c, &opts, &mut |_| {});
	assert!(r.skipped.as_deref().is_some_and(|s| s.contains("drawing not found")), "{r:?}");
	assert!(eval::probe_case(&c, &opts.provider, Dialect::Pixel)[0].error.is_some());
}

#[test]
fn the_probe_finds_a_qwen_style_model_in_its_own_dialect() {
	let c = case("symbols.json");
	// Always answers bbox_2d [x1,y1,x2,y2] on 0-1000, exactly on target.
	let (base, server) = serve(6, |_, req| {
		let prompt = req.pointer("/messages/0/content/0/text").and_then(Value::as_str).unwrap().to_string();
		let region = view_region(&prompt);
		let boxes = if prompt.contains("receptacle") { synth::receptacle_boxes() } else { synth::light_boxes() };
		let reply = eval::reply_for(region, (1000, 1000), Dialect::Grid1000Xy, &boxes);
		("application/json", json!({"choices": [{"message": {"content": format!("```json\n{reply}\n```")}}], "usage": {"prompt_tokens": 900, "completion_tokens": 80}}).to_string())
	});
	let provider = ProviderConfig { base_url: base, api_key: None, model: "qwen3-vl-test".into(), context_tokens: 32_768, vision: true };
	let mut rows = Vec::new();
	for d in eval::DIALECTS {
		rows.extend(eval::probe_case(&c, &provider, d));
	}
	server.join().unwrap();
	assert_eq!(rows.len(), 6);
	for r in &rows {
		assert!(r.error.is_none(), "{r:?}");
		assert_eq!(r.found, r.targets);
		assert_eq!(r.best_reading, "qwen3", "{r:?}");
		assert_eq!(r.best_hits, r.targets);
		assert_eq!(r.prompt_tokens, 900);
		if r.dialect == "qwen3" {
			assert!(r.native);
			assert_eq!(r.hits, r.targets);
			assert!(r.median_err_px.unwrap() < 2.0 && r.mean_iou.unwrap() > 0.8, "{r:?}");
		} else {
			assert!(r.hits < r.targets, "{r:?}");
		}
	}
	let v = eval::verdicts(&rows);
	let q = v.iter().find(|v| v.dialect == "qwen3").unwrap();
	assert_eq!((q.hits, q.targets, q.verdict.as_str()), (15, 15, "full takeoff"));
	assert!(v.iter().filter(|v| v.dialect != "qwen3").all(|v| v.verdict == "not usable"), "{v:?}");
	let table = eval::probe_table(&rows);
	assert!(table.contains("→ qwen3-vl-test: full takeoff (qwen3)"), "{table}");
}

// ── Live (ignored) ────────────────────────────────────────────────────────

fn live_provider() -> ProviderConfig {
	let model = std::env::var("TAKEOFF_MODEL").expect("set TAKEOFF_MODEL (and TAKEOFF_BASE_URL)");
	let base_url = std::env::var("TAKEOFF_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8080/v1".into());
	ProviderConfig { base_url, api_key: std::env::var("TAKEOFF_API_KEY").ok(), model, context_tokens: std::env::var("TAKEOFF_CONTEXT").ok().and_then(|c| c.parse().ok()).unwrap_or(32_768), vision: true }
}

fn live_cases() -> Vec<Case> {
	let dir = std::env::var("TAKEOFF_CASES").map(PathBuf::from).unwrap_or_else(|_| evals_dir().join("cases"));
	let only = std::env::var("TAKEOFF_ONLY").ok();
	eval::load_cases(&[dir]).unwrap().into_iter().filter(|c| only.as_deref().is_none_or(|o| c.name.contains(o))).collect()
}

fn save(rep: &Report) {
	if let Ok(p) = std::env::var("TAKEOFF_REPORT") {
		std::fs::write(&p, serde_json::to_string_pretty(rep).unwrap()).unwrap();
		println!("report → {p}");
	}
}

#[test]
#[ignore = "needs a real model: TAKEOFF_BASE_URL, TAKEOFF_MODEL (or TAKEOFF_MODEL=claude:<model>)"]
fn live_run() {
	// `claude:haiku` and the like run on the subscription CLI; it serves the
	// tools through a bridge process, which is the CLI (TAKEOFF_BRIDGE_EXE).
	let channel = std::env::var("TAKEOFF_MODEL").ok().and_then(|m| m.split_once(':').and_then(|(k, id)| takeoff_agent::channel::Channel::from_key(k).map(|c| (c, id.to_string()))));
	let provider = if channel.is_some() { ProviderConfig { base_url: String::new(), api_key: None, model: String::new(), context_tokens: 0, vision: true } } else { live_provider() };
	let opts = eval::RunOptions { provider, timeout: Duration::from_secs(std::env::var("TAKEOFF_TIMEOUT").ok().and_then(|t| t.parse().ok()).unwrap_or(1800)), workspace: None, keep: None, channel };
	let mut rep = Report::new();
	for c in live_cases().iter().filter(|c| c.task.is_some()) {
		println!("── {}", c.name);
		let r = eval::run_case(c, &opts, &mut |e| {
			if let takeoff_agent::AgentEvent::ToolFinished { name, ok, .. } = e {
				println!("   {} {name}", if *ok { "✓" } else { "✗" });
			}
		});
		rep.runs.push(r);
	}
	println!("\n{}", eval::run_table(&rep.runs));
	save(&rep);
}

#[test]
#[ignore = "needs a real model: TAKEOFF_BASE_URL, TAKEOFF_MODEL"]
fn live_probe() {
	let p = live_provider();
	let dialects: Vec<Dialect> = match std::env::var("TAKEOFF_DIALECTS") {
		Ok(s) => s.split(',').map(|d| eval::parse_dialect(d).expect("gemini, qwen3 or pixel")).collect(),
		Err(_) => eval::DIALECTS.to_vec(),
	};
	let mut rep = Report::new();
	for c in live_cases().iter().filter(|c| c.probe.is_some()) {
		for d in &dialects {
			for r in eval::probe_case(c, &p, *d) {
				println!("{} · {} · {}: {}", c.name, eval::dialect_name(*d), r.ask, r.error.clone().unwrap_or_else(|| format!("{}/{} on target, {} boxes, {:.0}s", r.hits, r.targets, r.found, r.wall_s)));
				rep.probes.push(r);
			}
		}
	}
	println!("\n{}", eval::probe_table(&rep.probes));
	save(&rep);
}
