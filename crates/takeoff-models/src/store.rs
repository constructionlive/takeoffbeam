//! Where local models live and which ones are installed.
//!
//! Three sources, de-duplicated by id:
//! - `models_dir()/<id>/<file>`: downloaded from the catalog;
//! - `models_dir()/imported.json`: files the user picked elsewhere on disk;
//! - existing construction.live desktop installs (macOS
//!   `~/Library/Application Support/construction.live/models`), so people
//!   who already downloaded a model there don't download it twice.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::catalog;

pub fn models_dir() -> PathBuf {
	let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
	let dir = base.join("Takeoff Desk").join("models");
	let _ = std::fs::create_dir_all(&dir);
	dir
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstalledModel {
	pub id: String,
	pub label: String,
	pub model_path: PathBuf,
	pub mmproj_path: Option<PathBuf>,
	/// Model + projector file sizes.
	pub size_bytes: u64,
	/// Has a vision projector (the agent can see drawings).
	pub vision: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Registry {
	imported: Vec<ImportedEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ImportedEntry {
	id: String,
	label: String,
	model: PathBuf,
	mmproj: Option<PathBuf>,
}

fn registry_path() -> PathBuf {
	models_dir().join("imported.json")
}

fn load_registry() -> Registry {
	std::fs::read(registry_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_registry(r: &Registry) -> Result<(), String> {
	let json = serde_json::to_vec_pretty(r).map_err(|e| e.to_string())?;
	let path = registry_path();
	let tmp = path.with_extension("json.tmp");
	std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
	std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn size_of(p: &Path) -> u64 {
	std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn make(id: String, label: String, model: PathBuf, mmproj: Option<PathBuf>) -> InstalledModel {
	let size_bytes = size_of(&model) + mmproj.as_deref().map(size_of).unwrap_or(0);
	InstalledModel { id, label, vision: mmproj.is_some(), model_path: model, mmproj_path: mmproj, size_bytes }
}

fn is_projector(name: &str) -> bool {
	name.to_lowercase().contains("mmproj")
}

/// Scan one directory in the catalog layout: `<dir>/<id>/<model>.gguf [+ mmproj*.gguf]`,
/// plus loose `<dir>/<model>.gguf` files.
fn scan_dir(dir: &Path, out: &mut Vec<InstalledModel>) {
	let Ok(entries) = std::fs::read_dir(dir) else { return };
	let label_for = |id: &str| catalog::find(id).map(|c| c.label).unwrap_or_else(|| id.to_string());
	for e in entries.flatten() {
		let path = e.path();
		let name = e.file_name().to_string_lossy().into_owned();
		if path.is_dir() {
			let Ok(files) = std::fs::read_dir(&path) else { continue };
			let mut ggufs: Vec<PathBuf> = files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|x| x == "gguf")).collect();
			ggufs.sort();
			let mmproj = ggufs.iter().find(|p| is_projector(&p.file_name().unwrap_or_default().to_string_lossy())).cloned();
			// Prefer the catalog's file name; else the biggest non-projector GGUF.
			let want = catalog::find(&name).map(|c| c.file);
			let model = ggufs
				.iter()
				.filter(|p| !is_projector(&p.file_name().unwrap_or_default().to_string_lossy()))
				.find(|p| want.as_deref().is_some_and(|w| p.file_name().is_some_and(|f| f == w)))
				.or_else(|| {
					ggufs.iter().filter(|p| !is_projector(&p.file_name().unwrap_or_default().to_string_lossy())).max_by_key(|p| size_of(p))
				})
				.cloned();
			if let Some(model) = model {
				out.push(make(name.clone(), label_for(&name), model, mmproj));
			}
		} else if path.extension().is_some_and(|x| x == "gguf") && !is_projector(&name) {
			let id = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
			out.push(make(id.clone(), label_for(&id), path, None));
		}
	}
}

/// Existing construction.live desktop model folder, if any.
fn construction_live_dir() -> Option<PathBuf> {
	let d = dirs::data_dir()?.join("construction.live").join("models");
	d.is_dir().then_some(d)
}

pub fn installed() -> Vec<InstalledModel> {
	let mut out = Vec::new();
	scan_dir(&models_dir(), &mut out);
	for e in load_registry().imported {
		if e.model.is_file() {
			out.push(make(e.id, e.label, e.model, e.mmproj.filter(|p| p.is_file())));
		}
	}
	if let Some(dir) = construction_live_dir() {
		scan_dir(&dir, &mut out);
	}
	// Downloads in progress leave `.part` files only, so they never appear here.
	let mut seen = std::collections::HashSet::new();
	out.retain(|m| seen.insert(m.id.clone()));
	out.sort_by(|a, b| b.vision.cmp(&a.vision).then(b.size_bytes.cmp(&a.size_bytes)));
	out
}

/// Register a GGUF the user already has (no copy).
pub fn import(model: &Path, mmproj: Option<&Path>) -> Result<InstalledModel, String> {
	if !model.is_file() {
		return Err(format!("{} is not a file", model.display()));
	}
	crate::gguf::read_meta(model).map_err(|e| format!("not a usable GGUF model: {e}"))?;
	if let Some(p) = mmproj {
		let meta = crate::gguf::read_meta(p).map_err(|e| format!("projector is not a GGUF file: {e}"))?;
		if meta.arch() != Some("clip") {
			return Err("the second file is not a vision projector (mmproj)".into());
		}
	}
	let meta = crate::gguf::read_meta(model).map_err(|e| e.to_string())?;
	let id = model.file_stem().unwrap_or_default().to_string_lossy().into_owned();
	let label = meta.get("general.name").and_then(crate::gguf::Value::as_str).map(str::to_string).unwrap_or_else(|| id.clone());
	let mut reg = load_registry();
	reg.imported.retain(|e| e.id != id);
	reg.imported.push(ImportedEntry { id: id.clone(), label: label.clone(), model: model.to_path_buf(), mmproj: mmproj.map(Path::to_path_buf) });
	save_registry(&reg)?;
	Ok(make(id, label, model.to_path_buf(), mmproj.map(Path::to_path_buf)))
}

/// Delete a downloaded model, or forget an imported one (its files stay).
/// Models found in the construction.live folder are left alone.
pub fn remove(id: &str) -> Result<(), String> {
	let mut reg = load_registry();
	let before = reg.imported.len();
	reg.imported.retain(|e| e.id != id);
	if reg.imported.len() != before {
		return save_registry(&reg);
	}
	let dir = models_dir().join(id);
	if dir.is_dir() {
		return std::fs::remove_dir_all(&dir).map_err(|e| e.to_string());
	}
	let loose = models_dir().join(format!("{id}.gguf"));
	if loose.is_file() {
		return std::fs::remove_file(&loose).map_err(|e| e.to_string());
	}
	Err(format!("{id} is not a model Takeoff Desk manages"))
}
