//! Sets: several drawings (Architectural, Structural, MEP…) as one project.
//! A set file only lists the PDFs, in order; each PDF keeps its own
//! `.takeoff.json`, which stays the source of truth. The project summary adds
//! up the documents' takeoffs item by item.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::doc::Document;
use crate::list::{self, SummaryLine};
use crate::model::*;

pub const SET_VERSION: u32 = 1;

/// What a set file's name ends with: `Project.takeoffset.json`.
pub const SET_SUFFIX: &str = ".takeoffset.json";

/// `Project.takeoffset.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetFile {
	pub version: u32,
	pub name: String,
	pub documents: Vec<SetDocument>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_updated: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDocument {
	/// The PDF, relative to the set file's folder (or absolute).
	pub path: String,
	/// What the set calls it ("Structural"); the file name when unset.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
}

pub fn is_set_file(p: &Path) -> bool {
	p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().ends_with(SET_SUFFIX))
}

impl SetFile {
	pub fn new(name: impl Into<String>) -> Self {
		Self { version: SET_VERSION, name: name.into(), documents: vec![], last_updated: None }
	}

	/// Add a PDF, stored relative to the set's folder when it sits under it.
	pub fn add(&mut self, set_path: &Path, pdf: &Path) {
		let dir = set_path.parent().unwrap_or(Path::new(""));
		let rel = pdf.strip_prefix(dir).map(Path::to_path_buf).unwrap_or_else(|_| pdf.to_path_buf());
		let path = rel.to_string_lossy().replace('\\', "/");
		if !self.documents.iter().any(|d| d.path == path) {
			self.documents.push(SetDocument { path, name: None });
		}
	}

	/// The documents' PDFs, resolved against the set's folder.
	pub fn paths(&self, set_path: &Path) -> Vec<PathBuf> {
		let dir = set_path.parent().unwrap_or(Path::new(""));
		self.documents.iter().map(|d| resolve(dir, &d.path)).collect()
	}

	/// A document's name in the set.
	pub fn doc_name(&self, i: usize) -> String {
		let d = &self.documents[i];
		d.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| Path::new(&d.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
	}
}

/// A path from a file next to `dir` (relative or absolute).
pub fn resolve(dir: &Path, p: &str) -> PathBuf {
	let p = Path::new(p);
	if p.is_absolute() { p.to_path_buf() } else { dir.join(p) }
}

#[derive(Debug, thiserror::Error)]
pub enum SetError {
	#[error("io: {0}")]
	Io(#[from] std::io::Error),
	#[error("bad set file: {0}")]
	Json(#[from] serde_json::Error),
}

pub fn load(path: &Path) -> Result<SetFile, SetError> {
	Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

/// Atomic save, like the sidecar.
pub fn save(path: &Path, set: &SetFile) -> Result<(), SetError> {
	let mut set = set.clone();
	set.last_updated = Some(crate::time::iso_now());
	let json = serde_json::to_vec_pretty(&set)?;
	let tmp = path.with_extension("json.tmp");
	{
		let mut f = std::fs::File::create(&tmp)?;
		f.write_all(&json)?;
		f.sync_all()?;
	}
	std::fs::rename(&tmp, path)?;
	Ok(())
}

/// A document from its sidecar alone (no PDF): page sizes come from the
/// sidecar's `pages`, so quantities are the same as with the drawing open.
pub fn sidecar_document(pdf: &Path) -> Document {
	let data = crate::sidecar::load(pdf).unwrap_or_default();
	let sizes = data.pages.iter().map(|p| (p.width_px * 72.0 / 200.0, p.height_px * 72.0 / 200.0)).collect();
	Document::new(data, sizes, "")
}

/// One item of the project summary: everything of that name and kind in every document.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectLine {
	pub name: String,
	pub color: String,
	pub kind: CategoryKind,
	pub unit: String,
	/// Per document (same order as the set), None where it doesn't appear.
	pub per_doc: Vec<Option<SummaryLine>>,
	pub items: usize,
	pub total: f64,
	pub cost: Option<f64>,
	pub warning: Option<String>,
}

/// Items with the same name, kind and unit are added up across documents.
pub fn project_summary(docs: &[&Document]) -> Vec<ProjectLine> {
	let mut out: Vec<ProjectLine> = Vec::new();
	for (k, doc) in docs.iter().enumerate() {
		for l in list::summary(doc) {
			let unit_key = crate::geometry::unit_key(&l.unit);
			let at = out.iter().position(|p| p.name == l.name && p.kind == l.kind && (p.unit.is_empty() || l.unit.is_empty() || crate::geometry::unit_key(&p.unit) == unit_key));
			let i = at.unwrap_or_else(|| {
				out.push(ProjectLine { name: l.name.clone(), color: l.color.clone(), kind: l.kind, unit: String::new(), per_doc: vec![None; docs.len()], items: 0, total: 0.0, cost: None, warning: None });
				out.len() - 1
			});
			let p = &mut out[i];
			if p.unit.is_empty() {
				p.unit = l.unit.clone();
			}
			p.items += l.items;
			p.total += l.total;
			if let Some(c) = l.cost {
				*p.cost.get_or_insert(0.0) += c;
			}
			if l.warning.is_some() {
				p.warning = l.warning.clone();
			}
			p.per_doc[k] = Some(l);
		}
	}
	out
}

/// The project summary as CSV: one column per document, then the total.
pub fn project_csv(names: &[String], lines: &[ProjectLine]) -> String {
	let cell = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
	let mut head = vec!["Item".to_string(), "Type".into(), "Unit".into()];
	head.extend(names.iter().cloned());
	head.extend(["Total".to_string(), "Count".into(), "Cost".into(), "Note".into()]);
	let mut out = head.iter().map(|s| cell(s)).collect::<Vec<_>>().join(",");
	for l in lines {
		let mut row = vec![l.name.clone(), format!("{:?}", l.kind), l.unit.clone()];
		row.extend(l.per_doc.iter().map(|d| d.as_ref().map(|d| format!("{}", d.total)).unwrap_or_default()));
		row.extend([format!("{}", l.total), l.items.to_string(), l.cost.map(|c| format!("{c:.2}")).unwrap_or_default(), l.warning.clone().unwrap_or_default()]);
		out.push('\n');
		out.push_str(&row.iter().map(|s| cell(s)).collect::<Vec<_>>().join(","));
	}
	out.push('\n');
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn set_file_round_trip() {
		let dir = std::env::temp_dir().join(format!("takeoff-set-{}", std::process::id()));
		std::fs::create_dir_all(dir.join("Structural")).unwrap();
		let set_path = dir.join("Tower.takeoffset.json");
		assert!(is_set_file(&set_path) && !is_set_file(&dir.join("a.pdf")));
		let mut s = SetFile::new("Tower");
		s.add(&set_path, &dir.join("Arch.pdf"));
		s.add(&set_path, &dir.join("Structural/S.pdf"));
		s.add(&set_path, Path::new("/elsewhere/MEP.pdf"));
		s.add(&set_path, &dir.join("Arch.pdf"));
		s.documents[1].name = Some("Structural".into());
		save(&set_path, &s).unwrap();
		let text = std::fs::read_to_string(&set_path).unwrap();
		assert!(text.contains("\"path\": \"Arch.pdf\"") && text.contains("\"path\": \"Structural/S.pdf\""), "{text}");
		let back = load(&set_path).unwrap();
		assert!(back.last_updated.is_some());
		assert_eq!(SetFile { last_updated: None, ..back.clone() }, s);
		assert_eq!(back.paths(&set_path), vec![dir.join("Arch.pdf"), dir.join("Structural/S.pdf"), PathBuf::from("/elsewhere/MEP.pdf")]);
		assert_eq!((back.doc_name(0).as_str(), back.doc_name(1).as_str()), ("Arch", "Structural"));
		// Unknown fields from a newer writer are ignored.
		let newer: SetFile = serde_json::from_str(r#"{"version":2,"name":"X","documents":[{"path":"a.pdf","color":"red"}],"future":1}"#).unwrap();
		assert_eq!(newer.documents[0].path, "a.pdf");
		std::fs::remove_dir_all(dir).ok();
	}

	#[test]
	fn project_summary_adds_up_documents() {
		let doc = |n: usize, extra: &str| {
			let mut d = TakeoffData::default();
			d.categories.push(Category::new("c1", "Door", "#4CC2FF", CategoryKind::Count));
			d.categories.push(Category::new("c2", extra, "#34D399", CategoryKind::Count));
			for i in 0..n {
				d.marks.push(Mark { id: format!("m{i}"), category_id: Some(if i % 2 == 0 { "c1" } else { "c2" }.into()), page: 1, x: 0.1, y: 0.1, w: 0.01, h: 0.01, source: Source::User, status: MarkStatus::Confirmed, tile: None, note: None, fields: None, meta: Default::default() });
			}
			Document::new(d, vec![(612.0, 792.0)], "")
		};
		let (a, b) = (doc(4, "Window"), doc(3, "Diffuser"));
		let lines = project_summary(&[&a, &b]);
		let door = lines.iter().find(|l| l.name == "Door").unwrap();
		assert_eq!((door.total, door.items), (4.0, 4));
		assert_eq!(door.per_doc[0].as_ref().unwrap().total, 2.0);
		let diffuser = lines.iter().find(|l| l.name == "Diffuser").unwrap();
		assert!(diffuser.per_doc[0].is_none() && diffuser.total == 1.0);
		let csv = project_csv(&["Arch".into(), "MEP".into()], &lines);
		assert!(csv.starts_with("Item,Type,Unit,Arch,MEP,Total"), "{csv}");
	}
}
