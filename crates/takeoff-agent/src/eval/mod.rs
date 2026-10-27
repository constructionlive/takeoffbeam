//! Evals: score the agent on drawings with known answers, and probe how well
//! a model places boxes on a view in each coordinate dialect.
//!
//! - A [`Case`] is one JSON file: a drawing, a page, and a `task` (a prompt
//!   plus expected quantities with tolerances) and/or a `probe` (targets with
//!   known boxes for the model to find).
//! - [`run_case`] runs the agent headless on the task and scores the takeoff
//!   it leaves; [`probe_case`] asks for boxes on one view per dialect.
//! - Results go in a [`Report`] (JSON, for comparing runs) and print as tables.
//! - [`synth`] writes the synthetic drawings the committed cases use.

mod probe;
mod run;
pub mod synth;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use takeoff_core::coords::Dialect;

pub use probe::{probe_case, probe_table, reply_for, verdicts, ProbeRow, Verdict};
pub use run::{run_case, score, CaseResult, RunOptions, Score};

/// One eval case (`evals/cases/*.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Case {
	/// Defaults to the file name.
	#[serde(default)]
	pub name: String,
	/// Relative to the case file, absolute, or starting with `$VAR` / `${VAR}`
	/// for drawings that can't be committed (e.g. `$TAKEOFF_EVAL_DRAWINGS/E-201.pdf`).
	pub pdf: String,
	#[serde(default = "first_page")]
	pub page: u32,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub notes: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub task: Option<Task>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub probe: Option<Probe>,
	/// Folder of the case file, for relative paths.
	#[serde(skip)]
	pub dir: PathBuf,
}

fn first_page() -> u32 {
	1
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Task {
	pub prompt: String,
	#[serde(default)]
	pub setup: Setup,
	#[serde(default)]
	pub expect: Vec<Expect>,
	/// Per-case cap; the runner's `--timeout` applies otherwise.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub timeout_secs: Option<u64>,
}

/// What the takeoff holds before the agent starts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Setup {
	/// A scale preset label for the page, e.g. `1/8" = 1'-0"`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub scale: Option<String>,
}

/// One expected result. `item` matches category names (and measurement
/// labels) case-insensitively by substring; omitted = everything of that kind
/// on the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expect {
	/// Count marks; `tol` is absolute (default 0).
	Count {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		item: Option<String>,
		value: f64,
		#[serde(default)]
		tol: f64,
	},
	/// Sum of length measurements, in `unit` (ft, m, in…).
	Length {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		item: Option<String>,
		value: f64,
		unit: String,
		#[serde(default = "default_tol_pct")]
		tol_pct: f64,
	},
	/// Sum of area measurements, in `unit` (sf, m2…).
	Area {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		item: Option<String>,
		value: f64,
		unit: String,
		#[serde(default = "default_tol_pct")]
		tol_pct: f64,
	},
	/// The page's scale, as feet per paper inch or a preset label.
	Scale {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		ft_per_in: Option<f64>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		preset: Option<String>,
		#[serde(default = "default_scale_tol")]
		tol_pct: f64,
	},
	/// The agent's replies mention `value` (title-block fields, notes…),
	/// ignoring case and spacing.
	Answer { field: String, value: String },
}

fn default_tol_pct() -> f64 {
	5.0
}

fn default_scale_tol() -> f64 {
	2.0
}

/// Things the model is asked to find on one view.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Probe {
	/// Page fractions [x, y, w, h] to show (omit = whole page).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub region: Option<[f64; 4]>,
	pub targets: Vec<Target>,
}

/// One question ("find every …") and where the answers really are: boxes in
/// page fractions, the marks of one item in the drawing's own takeoff (mark
/// them by hand in Takeoff Desk), or a text string the PDF contains.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Target {
	pub ask: String,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub boxes: Vec<[f64; 4]>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub item: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub text: Option<String>,
}

impl Case {
	pub fn load(path: &Path) -> Result<Case, String> {
		let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
		let mut c: Case = serde_json::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))?;
		if c.name.is_empty() {
			c.name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
		}
		c.dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
		Ok(c)
	}

	/// The drawing's path, or why it isn't there.
	pub fn pdf_path(&self) -> Result<PathBuf, String> {
		let p = expand(&self.pdf)?;
		let p = if p.is_absolute() { p } else { self.dir.join(p) };
		if !p.exists() {
			return Err(format!("drawing not found: {}", p.display()));
		}
		Ok(p)
	}
}

/// `$VAR/rest`, `${VAR}/rest` and `~/rest`.
fn expand(s: &str) -> Result<PathBuf, String> {
	if let Some(rest) = s.strip_prefix("~/") {
		return Ok(dirs::home_dir().unwrap_or_default().join(rest));
	}
	let Some(rest) = s.strip_prefix('$') else { return Ok(PathBuf::from(s)) };
	let (var, tail) = match rest.strip_prefix('{') {
		Some(r) => r.split_once('}').ok_or(format!("bad path '{s}'"))?,
		None => rest.split_at(rest.find(['/', '\\']).unwrap_or(rest.len())),
	};
	let base = std::env::var(var).map_err(|_| format!("set {var} to run this case (its drawing is not committed)"))?;
	Ok(PathBuf::from(format!("{base}{tail}")))
}

/// Case files from files and folders (folders: every `*.json`, sorted).
pub fn load_cases(paths: &[PathBuf]) -> Result<Vec<Case>, String> {
	let mut files = Vec::new();
	for p in paths {
		if p.is_dir() {
			let mut found: Vec<PathBuf> = std::fs::read_dir(p).map_err(|e| format!("{}: {e}", p.display()))?.filter_map(|e| e.ok().map(|e| e.path())).filter(|f| f.extension().is_some_and(|x| x == "json")).collect();
			found.sort();
			files.extend(found);
		} else {
			files.push(p.clone());
		}
	}
	files.iter().map(|f| Case::load(f)).collect()
}

/// Short names for the dialects, as the CLI takes them.
pub fn dialect_name(d: Dialect) -> &'static str {
	match d {
		Dialect::Grid1000Yx => "gemini",
		Dialect::Grid1000Xy => "qwen3",
		Dialect::Pixel => "pixel",
	}
}

pub fn parse_dialect(s: &str) -> Option<Dialect> {
	match s.trim().to_lowercase().as_str() {
		"gemini" | "yx" | "grid1000yx" => Some(Dialect::Grid1000Yx),
		"qwen3" | "qwen" | "xy" | "grid1000xy" => Some(Dialect::Grid1000Xy),
		"pixel" | "px" => Some(Dialect::Pixel),
		_ => None,
	}
}

pub const DIALECTS: [Dialect; 3] = [Dialect::Grid1000Yx, Dialect::Grid1000Xy, Dialect::Pixel];

/// Everything one `takeoff eval` invocation measured.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
	/// Takeoff Desk version that ran it.
	pub version: String,
	/// Unix seconds.
	pub started: u64,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub runs: Vec<CaseResult>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub probes: Vec<ProbeRow>,
}

impl Report {
	pub fn new() -> Self {
		Report { version: env!("CARGO_PKG_VERSION").into(), started: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0), ..Default::default() }
	}
}

/// The task results as a table: one row per case, then each expectation.
pub fn run_table(runs: &[CaseResult]) -> String {
	let mut s = format!("{:<28} {:<26} {:>6} {:>8} {:>6} {:>10}  {}\n", "case", "model", "score", "time", "steps", "tokens", "notes");
	for r in runs {
		let note = r.skipped.clone().or(r.error.clone()).unwrap_or_default();
		s.push_str(&format!(
			"{:<28} {:<26} {:>6} {:>7.0}s {:>6} {:>10}  {}\n",
			clip(&r.case, 28),
			clip(&r.model, 26),
			if r.skipped.is_some() { "skip".into() } else { format!("{}/{}", r.passed, r.total) },
			r.wall_s,
			r.steps,
			r.prompt_tokens + r.completion_tokens,
			note
		));
		for sc in &r.scores {
			s.push_str(&format!("    {} {:<34} want {:<16} got {:<16} {}\n", if sc.pass { "✓" } else { "✗" }, clip(&sc.what, 34), clip(&sc.expected, 16), clip(&sc.got, 16), sc.err_pct.map(|e| format!("{e:+.1}%")).unwrap_or_default()));
		}
	}
	let (p, t) = runs.iter().fold((0, 0), |a, r| (a.0 + r.passed, a.1 + r.total));
	s.push_str(&format!("total {p}/{t} expectations passed\n"));
	s
}

/// What changed against an earlier report, case by case (matched by case
/// name and model; with one model on each side, by case name alone).
pub fn compare(old: &Report, new: &Report) -> String {
	let models = |r: &Report| r.runs.iter().map(|c| c.model.clone()).collect::<std::collections::BTreeSet<_>>();
	let by_name = models(old).len() <= 1 && models(new).len() <= 1;
	let mut s = String::new();
	for n in &new.runs {
		let Some(o) = old.runs.iter().find(|o| o.case == n.case && o.repeat == n.repeat && (by_name || o.model == n.model)) else { continue };
		let delta = n.passed as i64 - o.passed as i64;
		s.push_str(&format!("{:<28} {}/{} → {}/{}{}  ({:.0}s → {:.0}s, {} → {} steps)\n", clip(&n.case, 28), o.passed, o.total, n.passed, n.total, if delta != 0 { format!(" ({delta:+})") } else { String::new() }, o.wall_s, n.wall_s, o.steps, n.steps));
		for sc in &n.scores {
			if let Some(prev) = o.scores.iter().find(|x| x.what == sc.what).filter(|x| x.pass != sc.pass) {
				s.push_str(&format!("    {} {}: {} → {}\n", if sc.pass { "fixed" } else { "BROKE" }, sc.what, prev.got, sc.got));
			}
		}
	}
	if s.is_empty() {
		s.push_str("no cases in common with the baseline\n");
	}
	s
}

fn clip(s: &str, n: usize) -> String {
	if s.chars().count() > n { format!("{}…", s.chars().take(n - 1).collect::<String>()) } else { s.to_string() }
}
