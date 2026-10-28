//! `takeoff`: the Takeoff Desk toolbox for scripts and other AI tools.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use takeoff_core::pages::parse_page_ranges;
use takeoff_core::sidecar;
use takeoff_pdf::tools::SplitMode;
use takeoff_pdf::{Drawing, FracRect};

mod evals;
mod mcp;
mod serve;

#[derive(Parser)]
#[command(name = "takeoff", version, about = "Takeoff Desk: PDF takeoff tools and a drawing agent", propagate_version = true)]
struct Cli {
	#[command(subcommand)]
	cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
	/// Pages, sizes, labels, layers, bookmarks and the takeoff summary.
	Info { pdf: PathBuf },
	/// Render a page (or region) to PNG, optionally with the takeoff drawn on it.
	Render {
		pdf: PathBuf,
		#[arg(long, default_value_t = 1)]
		page: u32,
		#[arg(long, default_value_t = 150.0)]
		dpi: f32,
		/// x,y,w,h as fractions of the page
		#[arg(long)]
		region: Option<String>,
		/// Draw the takeoff (marks, measurements, markups)
		#[arg(long)]
		markups: bool,
		/// Hide PDF layers by name (repeatable)
		#[arg(long = "hide-layer")]
		hide_layer: Vec<String>,
		#[arg(short, long)]
		out: PathBuf,
	},
	/// Text on a page (optionally a region), with positions as JSON.
	Text {
		pdf: PathBuf,
		#[arg(long, default_value_t = 1)]
		page: u32,
		#[arg(long)]
		region: Option<String>,
		#[arg(long)]
		json: bool,
	},
	/// Find text across the drawing set.
	Search {
		pdf: PathBuf,
		query: String,
		#[arg(long)]
		match_case: bool,
		#[arg(long)]
		whole_word: bool,
	},
	/// List the PDF's layers.
	Layers { pdf: PathBuf },
	/// Split into several files (takeoffs follow their pages).
	Split {
		pdf: PathBuf,
		#[arg(long, conflicts_with_all = ["ranges", "bookmarks"])]
		every: Option<u32>,
		/// e.g. "1-12,13-40,41-"
		#[arg(long)]
		ranges: Option<String>,
		#[arg(long)]
		bookmarks: bool,
		#[arg(short, long)]
		out_dir: PathBuf,
	},
	/// Merge PDFs in order (takeoffs merge too).
	Merge {
		inputs: Vec<PathBuf>,
		#[arg(short, long)]
		out: PathBuf,
	},
	/// Copy pages into a new PDF, e.g. --pages "3,1-2".
	Extract {
		pdf: PathBuf,
		#[arg(long)]
		pages: String,
		#[arg(short, long)]
		out: PathBuf,
	},
	/// Delete pages in place (a backup of the original is kept once).
	Delete {
		pdf: PathBuf,
		#[arg(long)]
		pages: String,
	},
	/// Rotate pages clockwise in place by 90° × turns.
	Rotate {
		pdf: PathBuf,
		#[arg(long)]
		pages: String,
		#[arg(long, default_value_t = 1)]
		turns: u32,
	},
	/// Replace sheets with a newer revision, matched by sheet number (the takeoff stays on them).
	SlipSheet {
		pdf: PathBuf,
		/// The new revision: one PDF or several
		#[arg(required = true)]
		new: Vec<PathBuf>,
		/// Add pages whose sheet isn't in the drawing at the end
		#[arg(long)]
		add_new: bool,
		/// Only show the matches
		#[arg(long)]
		dry_run: bool,
	},
		/// Export the takeoff: flattened PDF, markups CSV, quantity summary CSV, Excel workbook.
	Export {
		pdf: PathBuf,
		#[arg(long)]
		pdf_out: Option<PathBuf>,
		#[arg(long)]
		csv: Option<PathBuf>,
		#[arg(long)]
		summary: Option<PathBuf>,
		/// Excel workbook: the markups list and the quantity summary, one sheet each.
		#[arg(long)]
		xlsx: Option<PathBuf>,
	},
	/// Local models: list installed, check memory fit.
	Models {
		#[command(subcommand)]
		cmd: ModelsCmd,
	},
	/// Ask the drawing agent something about a PDF (headless).
	Agent {
		pdf: PathBuf,
		prompt: String,
		#[arg(long, default_value_t = 1)]
		page: u32,
		/// OpenAI-compatible base URL (llama-server, LM Studio, a cloud proxy…)
		#[arg(long, env = "TAKEOFF_BASE_URL")]
		base_url: Option<String>,
		#[arg(long, env = "TAKEOFF_API_KEY")]
		api_key: Option<String>,
		/// Model id at that endpoint, or "local:<installed id>" to start llama-server
		#[arg(long, env = "TAKEOFF_MODEL")]
		model: String,
		#[arg(long, default_value_t = 32768)]
		context: u32,
	},
	/// Score the agent on drawings with known answers, or probe how well a model places boxes.
	Eval {
		#[command(subcommand)]
		cmd: evals::EvalCmd,
	},
	/// Serve the drawing tools over MCP (stdio) for Claude Desktop, Cursor, etc.
	Mcp {
		/// Drawing to open at start (tools can also open one).
		pdf: Option<PathBuf>,
	},
	/// Keep one drawing open and render its pages and tiles on request (JSON lines over stdio), for apps that show it.
	Serve { pdf: PathBuf },
}

#[derive(Subcommand)]
enum ModelsCmd {
	/// Installed local models with a fit verdict.
	List {
		#[arg(long, default_value_t = 32768)]
		context: u32,
	},
	/// The downloadable catalog with a fit verdict.
	Catalog {
		#[arg(long, default_value_t = 32768)]
		context: u32,
	},
}

fn region(s: &Option<String>) -> Result<FracRect, String> {
	match s {
		None => Ok(FracRect::FULL),
		Some(s) => {
			let v: Vec<f64> = s.split(',').map(|x| x.trim().parse::<f64>().map_err(|_| format!("bad region '{s}' (want x,y,w,h)"))).collect::<Result<_, _>>()?;
			let [x, y, w, h] = v[..] else { return Err(format!("bad region '{s}' (want x,y,w,h)")) };
			Ok(FracRect { x, y, w, h })
		}
	}
}

fn pages(spec: &str, pdf: &std::path::Path) -> Result<Vec<u32>, String> {
	let n = Drawing::open(pdf).map_err(|e| e.to_string())?.page_count();
	parse_page_ranges(spec, n)
}

fn run(cli: Cli) -> Result<(), String> {
	match cli.cmd {
		Cmd::Info { pdf } => {
			let d = Drawing::open(&pdf).map_err(|e| e.to_string())?;
			let n = d.page_count();
			println!("{}: {n} pages", pdf.display());
			for p in 1..=n.min(500) {
				let (w, h) = d.page_size_pt(p).map_err(|e| e.to_string())?;
				let label = d.page_label(p).ok().flatten().unwrap_or_default();
				println!("  p{p:<4} {:>5.1} × {:<5.1} in  {label}", w / 72.0, h / 72.0);
			}
			let layers = d.layer_names();
			println!("layers: {}", if layers.is_empty() { "none".into() } else { layers.len().to_string() });
			println!("bookmarks: {}", d.outline().len());
			let data = sidecar::load(&pdf).map_err(|e| e.to_string())?;
			let doc = takeoff_core::Document::new(data, (1..=n).map(|p| d.page_size_pt(p).map(|(w, h)| (w as f64, h as f64)).unwrap_or((612.0, 792.0))).collect(), "cli");
			let lines = takeoff_core::list::summary(&doc);
			if lines.is_empty() {
				println!("takeoff: empty");
			} else {
				println!("takeoff:");
				for l in lines {
					println!("  {:<28} {:>14}  ({} items{})", l.name, l.total_text, l.items, l.warning.map(|w| format!(", {w}")).unwrap_or_default());
				}
			}
		}
		Cmd::Render { pdf, page, dpi, region: r, markups, hide_layer, out } => {
			let mut d = Drawing::open(&pdf).map_err(|e| e.to_string())?;
			let hidden: HashSet<String> = hide_layer.into_iter().collect();
			d.set_hidden_layers(&hidden).map_err(|e| e.to_string())?;
			let mut img = d.render(page, dpi, region(&r)?, None).map_err(|e| e.to_string())?;
			if markups {
				let data = sidecar::load(&pdf).map_err(|e| e.to_string())?;
				let (w, h) = d.page_size_pt(page).map_err(|e| e.to_string())?;
				takeoff_pdf::burn::burn(&mut img.image, img.region, (w as f64, h as f64), &data, page, &Default::default());
			}
			img.image.save(&out).map_err(|e| e.to_string())?;
			println!("{} ({}×{})", out.display(), img.image.width(), img.image.height());
		}
		Cmd::Text { pdf, page, region: r, json } => {
			let d = Drawing::open(&pdf).map_err(|e| e.to_string())?;
			let rr = r.as_ref().map(|_| region(&r)).transpose()?;
			let runs = d.text_runs(page, rr, 5000).map_err(|e| e.to_string())?;
			if json {
				let v: Vec<serde_json::Value> = runs.iter().map(|t| serde_json::json!({ "text": t.text, "x": t.rect.0, "y": t.rect.1, "w": t.rect.2, "h": t.rect.3 })).collect();
				println!("{}", serde_json::to_string_pretty(&v).unwrap());
			} else {
				for t in runs {
					println!("{}", t.text);
				}
			}
		}
		Cmd::Search { pdf, query, match_case, whole_word } => {
			let d = Drawing::open(&pdf).map_err(|e| e.to_string())?;
			let mut total = 0;
			for p in 1..=d.page_count() {
				for h in d.search_page(p, &query, match_case, whole_word, 500).map_err(|e| e.to_string())? {
					total += 1;
					println!("p{:<4} ({:.3},{:.3})  {}", h.page, h.rect.0, h.rect.1, h.context);
				}
			}
			eprintln!("{total} hits");
		}
		Cmd::Layers { pdf } => {
			let d = Drawing::open(&pdf).map_err(|e| e.to_string())?;
			let off: HashSet<String> = d.default_off_layers().into_iter().collect();
			for l in d.layer_names() {
				println!("{}{l}", if off.contains(&l) { "(off) " } else { "" });
			}
		}
		Cmd::Split { pdf, every, ranges, bookmarks, out_dir } => {
			let mode = if bookmarks {
				SplitMode::Bookmarks
			} else if let Some(r) = ranges {
				let n = Drawing::open(&pdf).map_err(|e| e.to_string())?.page_count();
				let mut out = vec![];
				for part in r.split(',').map(str::trim).filter(|s| !s.is_empty()) {
					let p = parse_page_ranges(part, n)?;
					out.push((*p.iter().min().unwrap(), *p.iter().max().unwrap()));
				}
				SplitMode::Ranges(out)
			} else {
				SplitMode::EveryN(every.unwrap_or(1))
			};
			let data = sidecar::load(&pdf).map_err(|e| e.to_string())?;
			for (f, a, b) in takeoff_pdf::ops::split(&pdf, &data, &mode, &out_dir).map_err(|e| e.to_string())? {
				println!("{}  (pages {a}-{b})", f.display());
			}
		}
		Cmd::Merge { inputs, out } => {
			if inputs.len() < 2 {
				return Err("merge needs at least two PDFs".into());
			}
			takeoff_pdf::ops::merge(&inputs, &out, |_| None).map_err(|e| e.to_string())?;
			println!("{}", out.display());
		}
		Cmd::Extract { pdf, pages: spec, out } => {
			let pg = pages(&spec, &pdf)?;
			let data = sidecar::load(&pdf).map_err(|e| e.to_string())?;
			takeoff_pdf::ops::extract(&pdf, &data, &pg, &out).map_err(|e| e.to_string())?;
			println!("{} ({} pages)", out.display(), pg.len());
		}
		Cmd::Delete { pdf, pages: spec } => {
			let pg = pages(&spec, &pdf)?;
			takeoff_pdf::ops::backup_once(&pdf).map_err(|e| e.to_string())?;
			let mut data = sidecar::load(&pdf).map_err(|e| e.to_string())?;
			let msg = takeoff_pdf::ops::edit_in_place(&pdf, &mut data, takeoff_pdf::ops::Edit::Delete(&pg)).map_err(|e| e.to_string())?;
			sidecar::save(&pdf, &data).map_err(|e| e.to_string())?;
			println!("{msg}");
		}
		Cmd::Rotate { pdf, pages: spec, turns } => {
			let pg = pages(&spec, &pdf)?;
			takeoff_pdf::ops::backup_once(&pdf).map_err(|e| e.to_string())?;
			let mut data = sidecar::load(&pdf).map_err(|e| e.to_string())?;
			let msg = takeoff_pdf::ops::edit_in_place(&pdf, &mut data, takeoff_pdf::ops::Edit::Rotate(&pg, turns)).map_err(|e| e.to_string())?;
			sidecar::save(&pdf, &data).map_err(|e| e.to_string())?;
			println!("{msg}");
		}
		Cmd::SlipSheet { pdf, new, add_new, dry_run } => {
			let current = takeoff_pdf::sheets::index(&pdf).map_err(|e| e.to_string())?;
			let prop = takeoff_pdf::slip::propose(&current, &new).map_err(|e| e.to_string())?;
			for m in &prop.matches {
				let (file, key) = &prop.incoming[m.incoming];
				let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
				let to = match (m.target, m.by) {
					(Some(t), Some(by)) => format!("→ p{} {} (by {})", t + 1, current[t].name(), by.label()),
					_ if add_new => "→ added at the end".into(),
					_ => "→ skipped (not in the drawing)".into(),
				};
				println!("{name} p{} {}  {to}", key.page, key.name());
			}
			if dry_run {
				return Ok(());
			}
			takeoff_pdf::ops::backup_once(&pdf).map_err(|e| e.to_string())?;
			let mut data = sidecar::load(&pdf).map_err(|e| e.to_string())?;
			let out = takeoff_pdf::slip::slip_sheet(&pdf, &mut data, &prop.pages(add_new)).map_err(|e| e.to_string())?;
			sidecar::save(&pdf, &data).map_err(|e| e.to_string())?;
			for w in &out.warnings {
				eprintln!("warning: {w}");
			}
			println!("{}", out.message());
			if let Some(h) = &out.history {
				println!("old pages kept in {}", h.display());
			}
		}
				Cmd::Export { pdf, pdf_out, csv, summary, xlsx } => {
			let data = sidecar::load(&pdf).map_err(|e| e.to_string())?;
			if pdf_out.is_none() && csv.is_none() && summary.is_none() && xlsx.is_none() {
				return Err("say what to export: --pdf-out, --csv, --summary and/or --xlsx".into());
			}
			if let Some(o) = pdf_out {
				let n = takeoff_pdf::export::export_flattened(&pdf, &data, &o).map_err(|e| e.to_string())?;
				println!("{} ({n} marked-up pages)", o.display());
			}
			let d = Drawing::open(&pdf).map_err(|e| e.to_string())?;
			let sizes = (1..=d.page_count()).map(|p| d.page_size_pt(p).map(|(w, h)| (w as f64, h as f64)).unwrap_or((612.0, 792.0))).collect();
			let doc = takeoff_core::Document::new(data, sizes, "cli");
			if let Some(o) = csv {
				std::fs::write(&o, takeoff_core::list::rows_csv(&doc, &takeoff_core::list::rows(&doc))).map_err(|e| e.to_string())?;
				println!("{}", o.display());
			}
			if let Some(o) = summary {
				std::fs::write(&o, takeoff_core::list::summary_csv(&takeoff_core::list::summary(&doc))).map_err(|e| e.to_string())?;
				println!("{}", o.display());
			}
			if let Some(o) = xlsx {
				std::fs::write(&o, takeoff_core::list::rows_xlsx(&doc, &takeoff_core::list::rows(&doc))?).map_err(|e| e.to_string())?;
				println!("{}", o.display());
			}
		}
		Cmd::Models { cmd } => match cmd {
			ModelsCmd::List { context } => {
				for m in takeoff_models::store::installed() {
					let fit = takeoff_models::fit_for(&m, context, takeoff_models::fit::CacheType::Q8_0);
					println!("{:<34} {:>6.1} GB  vision={:<5}  {}", m.id, m.size_bytes as f64 / (1u64 << 30) as f64, m.vision, fit.summary());
				}
			}
			ModelsCmd::Catalog { context } => {
				for m in takeoff_models::catalog::builtin() {
					let fit = takeoff_models::fit_for_catalog(&m, context);
					println!("{:<34} {:>6.1} GB  {}", m.id, m.size_bytes as f64 / 1e9, fit.summary());
				}
			}
		},
		Cmd::Agent { pdf, prompt, page, base_url, api_key, model, context } => agent(pdf, prompt, page, base_url, api_key, model, context)?,
		Cmd::Eval { cmd } => evals::run(cmd)?,
		Cmd::Mcp { pdf } => mcp::serve(pdf)?,
		Cmd::Serve { pdf } => serve::serve(pdf)?,
	}
	Ok(())
}

/// The endpoint for `model`. "local:<id>" starts llama-server, which runs while the handle lives.
fn provider(base_url: Option<String>, api_key: Option<String>, model: String, context: u32) -> Result<(takeoff_agent::ProviderConfig, Option<takeoff_models::server::LlamaServer>), String> {
	let mut _server = None;
	let provider = if let Some(id) = model.strip_prefix("local:") {
		let m = takeoff_models::store::installed().into_iter().find(|m| m.id == id).ok_or(format!("no installed model '{id}' (see `takeoff models list`)"))?;
		let mut s = takeoff_models::server::LlamaServer::start(&m, context, takeoff_models::fit::CacheType::Q8_0)?;
		eprintln!("loading {}…", m.label);
		match s.wait_ready(std::time::Duration::from_secs(300)) {
			takeoff_models::server::ServerState::Ready => {}
			takeoff_models::server::ServerState::Failed(e) => return Err(format!("llama-server failed: {e}")),
			other => return Err(format!("llama-server not ready: {other:?}")),
		}
		let p = takeoff_agent::ProviderConfig { base_url: s.base_url(), api_key: None, model: s.model_id(), context_tokens: context, vision: m.vision };
		_server = Some(s);
		p
	} else {
		let base = base_url.ok_or("--base-url is required unless --model local:<id>")?;
		takeoff_agent::ProviderConfig { base_url: base, api_key, model, context_tokens: context, vision: true }
	};
	Ok((provider, _server))
}

fn agent(pdf: PathBuf, prompt: String, page: u32, base_url: Option<String>, api_key: Option<String>, model: String, context: u32) -> Result<(), String> {
	let (provider, _server) = provider(base_url, api_key, model, context)?;
	let d = Drawing::open(&pdf).map_err(|e| e.to_string())?;
	let sizes = (1..=d.page_count()).map(|p| d.page_size_pt(p).map(|(w, h)| (w as f64, h as f64)).unwrap_or((612.0, 792.0))).collect();
	drop(d);
	let data = sidecar::load(&pdf).map_err(|e| e.to_string())?;
	let doc = std::sync::Arc::new(parking_lot::Mutex::new(takeoff_core::Document::new(data, sizes, "AI")));
	let session = std::sync::Arc::new(parking_lot::Mutex::new(takeoff_agent::ChatSession::new()));
	let ctx = takeoff_agent::AgentContext { doc: doc.clone(), pdf_path: pdf.clone(), current_page: page, visible_region: None, selected_ids: vec![], provider, workspace: None, others: vec![], attachments: vec![], images: vec![] };
	let turn = takeoff_agent::start_turn(session, ctx, prompt);
	for ev in turn.events.iter() {
		use takeoff_agent::AgentEvent::*;
		match ev {
			TextDelta(t) => {
				print!("{t}");
				use std::io::Write;
				let _ = std::io::stdout().flush();
			}
			ToolStarted { name, args, .. } => eprintln!("\n→ {name} {}", short(&args.to_string(), 160)),
			ToolFinished { name, ok, summary, .. } => eprintln!("  {} {name}: {}", if ok { "✓" } else { "✗" }, short(&summary, 200)),
			Error(e) => eprintln!("\nerror: {e}"),
			Done { steps } => {
				eprintln!("\n({steps} steps)");
				break;
			}
			_ => {}
		}
	}
	let d = doc.lock();
	if d.is_dirty() {
		sidecar::save(&pdf, &d.data).map_err(|e| e.to_string())?;
		eprintln!("saved takeoff → {}", sidecar::sidecar_path(&pdf).display());
	}
	Ok(())
}

fn short(s: &str, n: usize) -> String {
	if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() }
}

fn main() -> ExitCode {
	// A subscription CLI (Claude Code, Codex) reaches the drawing tools through
	// this executable, as it does through the app's (see `takeoff eval run --model claude:…`).
	let args: Vec<String> = std::env::args().collect();
	if args.get(1).map(String::as_str) == Some(takeoff_agent::channel::BRIDGE_ARG) {
		std::process::exit(takeoff_agent::channel::run_bridge(args.get(2).map(String::as_str).unwrap_or("")));
	}
	// `takeoff … | head` should end quietly, not panic on a closed pipe.
	std::panic::set_hook(Box::new(|info| {
		let msg = info.to_string();
		if msg.contains("Broken pipe") {
			std::process::exit(0);
		}
		eprintln!("{msg}");
	}));
	match run(Cli::parse()) {
		Ok(()) => ExitCode::SUCCESS,
		Err(e) => {
			eprintln!("takeoff: {e}");
			ExitCode::FAILURE
		}
	}
}
