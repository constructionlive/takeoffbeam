//! `takeoff eval`: run the eval cases against a model and score them, probe
//! box placement per coordinate dialect, or rewrite the synthetic drawings.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Subcommand;
use takeoff_agent::eval::{self, Case, Report};
use takeoff_agent::AgentEvent;

#[derive(clap::Args)]
pub struct ModelArgs {
	/// OpenAI-compatible base URL (llama-server, LM Studio, Ollama, a cloud proxy…)
	#[arg(long, env = "TAKEOFF_BASE_URL")]
	base_url: Option<String>,
	#[arg(long, env = "TAKEOFF_API_KEY")]
	api_key: Option<String>,
	/// Model id at that endpoint, or "local:<installed id>" to start llama-server; repeat or comma-separate for several
	#[arg(long = "model", env = "TAKEOFF_MODEL", value_delimiter = ',', required = true)]
	models: Vec<String>,
	#[arg(long, default_value_t = 32768)]
	context: u32,
}

#[derive(Subcommand)]
pub enum EvalCmd {
	/// Run each case's task headless and score the takeoff the agent leaves.
	Run {
		/// Case files, or folders of them
		#[arg(default_value = "evals/cases")]
		cases: Vec<PathBuf>,
		#[command(flatten)]
		model: ModelArgs,
		/// Seconds before a case is cancelled (a case's own timeout_secs wins)
		#[arg(long, default_value_t = 1800)]
		timeout: u64,
		/// Run every case this many times
		#[arg(long, default_value_t = 1)]
		repeat: u32,
		/// Give the agent a shell in this folder
		#[arg(long)]
		workspace: Option<PathBuf>,
		/// Only cases whose name contains this
		#[arg(long)]
		only: Option<String>,
		/// JSON report (default: evals/runs/run-<time>.json beside the cases folder)
		#[arg(long)]
		report: Option<PathBuf>,
		/// An earlier report to compare with
		#[arg(long)]
		baseline: Option<PathBuf>,
	},
	/// Ask a model to box known targets on a view, per coordinate dialect, and measure the error.
	Probe {
		/// Case files, or folders of them
		#[arg(default_value = "evals/cases")]
		cases: Vec<PathBuf>,
		#[command(flatten)]
		model: ModelArgs,
		/// all, native, or any of gemini, qwen3, pixel
		#[arg(long, default_value = "all", value_delimiter = ',')]
		dialect: Vec<String>,
		/// Only cases whose name contains this
		#[arg(long)]
		only: Option<String>,
		/// JSON report (default: evals/runs/probe-<time>.json beside the cases folder)
		#[arg(long)]
		report: Option<PathBuf>,
	},
	/// Write the synthetic drawings the committed cases use.
	Drawings {
		#[arg(default_value = "evals/drawings")]
		dir: PathBuf,
	},
}

pub fn run(cmd: EvalCmd) -> Result<(), String> {
	match cmd {
		EvalCmd::Run { cases, model, timeout, repeat, workspace, only, report, baseline } => {
			let list = pick(&cases, only.as_deref(), |c| c.task.is_some())?;
			let mut rep = Report::new();
			for m in &model.models {
				// `claude:<model>` / `codex:<model>`: the subscription CLI, as in the app.
				let channel = m.split_once(':').and_then(|(k, id)| takeoff_agent::channel::Channel::from_key(k).map(|c| (c, id.to_string())));
				let (provider, _server) = match &channel {
					Some(_) => (takeoff_agent::ProviderConfig { base_url: String::new(), api_key: None, model: m.clone(), context_tokens: model.context, vision: true }, None),
					None => crate::provider(model.base_url.clone(), model.api_key.clone(), m.clone(), model.context)?,
				};
				let keep = report_path(&report, &cases, "run", &rep).with_extension("takeoffs");
				let opts = eval::RunOptions { provider, timeout: Duration::from_secs(timeout), workspace: workspace.clone(), keep: Some(keep), channel };
				for case in &list {
					for n in 1..=repeat.max(1) {
						eprintln!("── {} · {}{}", case.name, m, if repeat > 1 { format!(" · run {n}") } else { String::new() });
						let mut r = eval::run_case(case, &opts, &mut progress);
						r.repeat = n;
						eprintln!("   {}", if let Some(s) = &r.skipped { format!("skipped: {s}") } else { format!("{}/{} in {:.0}s, {} steps", r.passed, r.total, r.wall_s, r.steps) });
						rep.runs.push(r);
					}
				}
			}
			println!("\n{}", eval::run_table(&rep.runs));
			if let Some(b) = baseline {
				let old: Report = serde_json::from_str(&std::fs::read_to_string(&b).map_err(|e| format!("{}: {e}", b.display()))?).map_err(|e| format!("{}: {e}", b.display()))?;
				println!("against {}:\n{}", b.display(), eval::compare(&old, &rep));
			}
			save(&rep, &report_path(&report, &cases, "run", &rep))
		}
		EvalCmd::Probe { cases, model, dialect, only, report } => {
			let list = pick(&cases, only.as_deref(), |c| c.probe.is_some())?;
			let mut rep = Report::new();
			for m in &model.models {
				let dialects = dialects(&dialect, m)?;
				let (provider, _server) = crate::provider(model.base_url.clone(), model.api_key.clone(), m.clone(), model.context)?;
				for case in &list {
					for d in &dialects {
						eprintln!("── {} · {} · {}", case.name, m, eval::dialect_name(*d));
						for r in eval::probe_case(case, &provider, *d) {
							eprintln!("   {}: {}", r.ask, r.error.clone().unwrap_or_else(|| format!("{}/{} on target, {} boxes, {:.0}s", r.hits, r.targets, r.found, r.wall_s)));
							rep.probes.push(r);
						}
					}
				}
			}
			println!("\n{}", eval::probe_table(&rep.probes));
			save(&rep, &report_path(&report, &cases, "probe", &rep))
		}
		EvalCmd::Drawings { dir } => {
			for p in eval::synth::write_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
				println!("{}", p.display());
			}
			Ok(())
		}
	}
}

/// The cases to run, with a clear error when there are none.
fn pick(paths: &[PathBuf], only: Option<&str>, has: impl Fn(&Case) -> bool) -> Result<Vec<Case>, String> {
	let all = eval::load_cases(paths)?;
	let list: Vec<Case> = all.into_iter().filter(|c| has(c) && only.is_none_or(|o| c.name.contains(o))).collect();
	if list.is_empty() {
		return Err(format!("no matching cases in {}", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")));
	}
	Ok(list)
}

fn dialects(spec: &[String], model: &str) -> Result<Vec<takeoff_core::coords::Dialect>, String> {
	let mut out = Vec::new();
	for s in spec {
		match s.trim() {
			"all" => out.extend(eval::DIALECTS),
			"native" => out.push(takeoff_core::coords::dialect_for(model)),
			other => out.push(eval::parse_dialect(other).ok_or(format!("unknown dialect '{other}' (all, native, gemini, qwen3, pixel)"))?),
		}
	}
	out.dedup();
	Ok(out)
}

fn progress(e: &AgentEvent) {
	match e {
		AgentEvent::ToolStarted { name, .. } => eprint!("   → {name}"),
		AgentEvent::ToolFinished { ok, .. } => eprintln!(" {}", if *ok { "✓" } else { "✗" }),
		AgentEvent::Error(e) => eprintln!("   error: {e}"),
		_ => {}
	}
}

/// `--report`, or `<cases folder>/../runs/<kind>-<time>.json`.
fn report_path(report: &Option<PathBuf>, cases: &[PathBuf], kind: &str, rep: &Report) -> PathBuf {
	if let Some(r) = report {
		return r.clone();
	}
	let first = cases.first().map(PathBuf::as_path).unwrap_or(Path::new("."));
	let dir = if first.is_dir() { first } else { first.parent().unwrap_or(Path::new(".")) };
	dir.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).join("runs").join(format!("{kind}-{}.json", rep.started))
}

fn save(rep: &Report, path: &Path) -> Result<(), String> {
	if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
		std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
	}
	std::fs::write(path, serde_json::to_string_pretty(rep).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", path.display()))?;
	eprintln!("report → {}", path.display());
	Ok(())
}

#[cfg(test)]
mod tests {
	use clap::Parser;

	use super::*;
	use crate::{Cli, Cmd};

	fn parse(args: &[&str]) -> EvalCmd {
		match Cli::try_parse_from(args).unwrap().cmd {
			Cmd::Eval { cmd } => cmd,
			_ => panic!("not eval"),
		}
	}

	#[test]
	fn parses_models_dialects_and_defaults() {
		let EvalCmd::Probe { cases, model, dialect, .. } = parse(&["takeoff", "eval", "probe", "--base-url", "http://127.0.0.1:8080/v1", "--model", "gemma3:4b,qwen3-vl", "--dialect", "native,pixel"]) else { panic!() };
		assert_eq!(cases, [PathBuf::from("evals/cases")]);
		assert_eq!(model.models, ["gemma3:4b", "qwen3-vl"]);
		assert_eq!(dialects(&dialect, "qwen3-vl").unwrap(), [takeoff_core::coords::Dialect::Grid1000Xy, takeoff_core::coords::Dialect::Pixel]);
		assert_eq!(dialects(&["all".into()], "x").unwrap().len(), 3);
		assert!(dialects(&["yx-ish".into()], "x").is_err());
		let EvalCmd::Run { timeout, repeat, report, .. } = parse(&["takeoff", "eval", "run", "evals/cases/rooms.json", "--model", "m", "--repeat", "2"]) else { panic!() };
		assert_eq!((timeout, repeat, report), (1800, 2, None));
		let rep = Report { started: 42, ..Report::new() };
		assert_eq!(report_path(&None, &[PathBuf::from("evals/cases/rooms.json")], "run", &rep), PathBuf::from("evals/runs/run-42.json"));
	}

	#[test]
	fn writes_the_synthetic_drawings() {
		let dir = std::env::temp_dir().join(format!("takeoff-cli-drawings-{}", std::process::id()));
		run(EvalCmd::Drawings { dir: dir.clone() }).unwrap();
		assert!(dir.join("symbols.pdf").exists() && dir.join("rooms.pdf").exists());
		std::fs::remove_dir_all(dir).ok();
	}
}
