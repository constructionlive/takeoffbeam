//! The document map: what each sheet of a drawing set is, the areas on it
//! that matter (plans, details, sections, schedules, notes, callouts,
//! elements) and how they relate ("callout 3 on the stair plan → detail 3 on
//! A-501", "general note 4 → every slab on S-201"). The AI builds it the way
//! it does a takeoff: it looks, zooms, and records what it finds through its
//! map tools, one sheet at a time. Lives next to the drawing: `plan.pdf` →
//! `plan.pdf.map.json` (kept out of the takeoff file, which the platform
//! shares). Only regions are stored; pictures are cut from the PDF.
//!
//! Node ids: a sheet is `s{page}`, an area on it `s{page}.{n}` (n from 1).

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAP_VERSION: u32 = 2;

/// A part of a page, in page fractions (x, y = top-left).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
}

impl Region {
	pub fn contains(&self, x: f64, y: f64) -> bool {
		x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
	}
	pub fn center(&self) -> (f64, f64) {
		(self.x + self.w / 2.0, self.y + self.h / 2.0)
	}
	pub fn area(&self) -> f64 {
		self.w * self.h
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
	Plan,
	Detail,
	Section,
	Elevation,
	Schedule,
	Legend,
	Notes,
	/// A callout bubble, section cut, tag or "SEE …" pointing elsewhere.
	Callout,
	/// A thing on a drawing: a room, a wall run, a piece of equipment, an area.
	Element,
	TitleBlock,
	KeyPlan,
	Diagram,
	#[default]
	#[serde(other)]
	Other,
}

impl PartKind {
	pub const ALL: [PartKind; 13] = [
		PartKind::Plan,
		PartKind::Detail,
		PartKind::Section,
		PartKind::Elevation,
		PartKind::Schedule,
		PartKind::Legend,
		PartKind::Notes,
		PartKind::Callout,
		PartKind::Element,
		PartKind::TitleBlock,
		PartKind::KeyPlan,
		PartKind::Diagram,
		PartKind::Other,
	];

	pub fn label(self) -> &'static str {
		match self {
			PartKind::Plan => "Plan",
			PartKind::Detail => "Detail",
			PartKind::Section => "Section",
			PartKind::Elevation => "Elevation",
			PartKind::Schedule => "Schedule",
			PartKind::Legend => "Legend",
			PartKind::Notes => "Notes",
			PartKind::Callout => "Callout",
			PartKind::Element => "Element",
			PartKind::TitleBlock => "Title block",
			PartKind::KeyPlan => "Key plan",
			PartKind::Diagram => "Diagram",
			PartKind::Other => "Other",
		}
	}

	pub fn key(self) -> &'static str {
		match self {
			PartKind::Plan => "plan",
			PartKind::Detail => "detail",
			PartKind::Section => "section",
			PartKind::Elevation => "elevation",
			PartKind::Schedule => "schedule",
			PartKind::Legend => "legend",
			PartKind::Notes => "notes",
			PartKind::Callout => "callout",
			PartKind::Element => "element",
			PartKind::TitleBlock => "title_block",
			PartKind::KeyPlan => "key_plan",
			PartKind::Diagram => "diagram",
			PartKind::Other => "other",
		}
	}

	/// Lenient: what a model writes ("Wall Section", "enlarged plan", "title-block").
	pub fn parse(s: &str) -> PartKind {
		let s = s.trim().to_lowercase().replace(['-', ' '], "_");
		match s.as_str() {
			_ if s.contains("title") => PartKind::TitleBlock,
			_ if s.contains("key") => PartKind::KeyPlan,
			_ if s.contains("callout") || s.contains("bubble") || s.contains("tag") || s.contains("marker") || s.contains("cut") || s.contains("matchline") => PartKind::Callout,
			_ if s.contains("schedule") || s.contains("table") => PartKind::Schedule,
			_ if s.contains("legend") || s.contains("symbol") || s.contains("abbreviation") => PartKind::Legend,
			_ if s.contains("note") || s.contains("spec") => PartKind::Notes,
			_ if s.contains("section") => PartKind::Section,
			_ if s.contains("elevation") => PartKind::Elevation,
			_ if s.contains("detail") => PartKind::Detail,
			_ if s.contains("plan") => PartKind::Plan,
			_ if s.contains("diagram") || s.contains("riser") || s.contains("one_line") => PartKind::Diagram,
			_ if s.contains("element") || s.contains("room") || s.contains("area") || s.contains("wall") || s.contains("equipment") || s.contains("zone") => PartKind::Element,
			_ => PartKind::Other,
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
	/// A detail callout ("3/A-501").
	Detail,
	/// A section cut.
	Section,
	/// An elevation marker.
	Elevation,
	/// An enlarged plan of an area.
	Enlarged,
	/// A matchline: the drawing continues there.
	Continues,
	/// A tag pointing at a schedule (door, window, panel…).
	Schedule,
	/// A note, spec or legend that governs the target.
	Governs,
	/// The same thing drawn in another place (plan ↔ elevation ↔ detail).
	Same,
	/// "SEE SHEET …", or any other relation.
	#[default]
	#[serde(other)]
	Reference,
}

impl LinkKind {
	pub const ALL: [LinkKind; 9] = [LinkKind::Detail, LinkKind::Section, LinkKind::Elevation, LinkKind::Enlarged, LinkKind::Continues, LinkKind::Schedule, LinkKind::Governs, LinkKind::Same, LinkKind::Reference];

	pub fn label(self) -> &'static str {
		match self {
			LinkKind::Detail => "Detail",
			LinkKind::Section => "Section",
			LinkKind::Elevation => "Elevation",
			LinkKind::Enlarged => "Enlarged plan",
			LinkKind::Continues => "Matchline",
			LinkKind::Schedule => "Schedule",
			LinkKind::Governs => "Note / spec",
			LinkKind::Same => "Same element",
			LinkKind::Reference => "Reference",
		}
	}

	pub fn key(self) -> &'static str {
		match self {
			LinkKind::Detail => "detail",
			LinkKind::Section => "section",
			LinkKind::Elevation => "elevation",
			LinkKind::Enlarged => "enlarged",
			LinkKind::Continues => "continues",
			LinkKind::Schedule => "schedule",
			LinkKind::Governs => "governs",
			LinkKind::Same => "same",
			LinkKind::Reference => "reference",
		}
	}

	pub fn parse(s: &str) -> LinkKind {
		let s = s.trim().to_lowercase();
		match s.as_str() {
			_ if s.contains("section") => LinkKind::Section,
			_ if s.contains("elevation") => LinkKind::Elevation,
			_ if s.contains("enlarg") => LinkKind::Enlarged,
			_ if s.contains("match") || s.contains("contin") => LinkKind::Continues,
			_ if s.contains("schedule") || s.contains("tag") => LinkKind::Schedule,
			_ if s.contains("govern") || s.contains("note") || s.contains("spec") || s.contains("applies") || s.contains("legend") => LinkKind::Governs,
			_ if s.contains("same") => LinkKind::Same,
			_ if s.contains("detail") => LinkKind::Detail,
			_ => LinkKind::Reference,
		}
	}
}

/// One area of a sheet worth knowing about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
	/// `s{page}.{n}`.
	pub id: String,
	pub kind: PartKind,
	/// The number it is called out by ("3", "A", "D4"), if it has one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub label: Option<String>,
	pub title: String,
	/// What it is and what matters about it, in the AI's words.
	#[serde(default, skip_serializing_if = "String::is_empty")]
	pub summary: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub region: Option<Region>,
	/// The area it sits in (a callout inside a plan), if not the sheet.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub parent: Option<String>,
}

/// One page of the set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
	pub page: u32,
	/// From the title block, e.g. "A-201".
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub number: Option<String>,
	#[serde(default)]
	pub title: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub discipline: Option<String>,
	#[serde(default)]
	pub summary: String,
	#[serde(default)]
	pub parts: Vec<Part>,
	/// Looked at closely and its areas and links recorded (not just indexed).
	#[serde(default)]
	pub mapped: bool,
}

impl Sheet {
	pub fn new(page: u32) -> Self {
		Sheet { page, number: None, title: String::new(), discipline: None, summary: String::new(), parts: vec![], mapped: false }
	}

	pub fn id(&self) -> String {
		sheet_id(self.page)
	}

	/// "A-201 · Floor plan", or "Page 3" before it has a number.
	pub fn name(&self) -> String {
		match (&self.number, self.title.trim()) {
			(Some(n), "") => n.clone(),
			(Some(n), t) => format!("{n} · {t}"),
			(None, "") => format!("Page {}", self.page),
			(None, t) => format!("p{} · {t}", self.page),
		}
	}
}

/// `from` relates to `to` (node ids), as the AI judged it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
	pub from: String,
	pub to: String,
	pub kind: LinkKind,
	/// The callout or note text it came from, if any.
	#[serde(default, skip_serializing_if = "String::is_empty")]
	pub text: String,
	/// How the two relate, for an estimator.
	#[serde(default, skip_serializing_if = "String::is_empty")]
	pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DocMap {
	pub version: u32,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub built_at: Option<String>,
	/// The model that built it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub model: Option<String>,
	/// A few sentences about the whole set.
	#[serde(default, skip_serializing_if = "String::is_empty")]
	pub overview: String,
	pub page_count: u32,
	/// One per page indexed, in page order.
	pub sheets: Vec<Sheet>,
	pub links: Vec<Link>,
	/// References to drawings that are not in this PDF (another set, a missing sheet).
	#[serde(default)]
	pub unresolved: Vec<Unresolved>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unresolved {
	pub from: String,
	pub text: String,
}

/// A node in the map: a whole sheet or an area of one.
#[derive(Debug, Clone, Copy)]
pub enum NodeRef<'a> {
	Sheet(&'a Sheet),
	Part(&'a Sheet, &'a Part),
}

impl NodeRef<'_> {
	pub fn page(&self) -> u32 {
		match self {
			NodeRef::Sheet(s) | NodeRef::Part(s, _) => s.page,
		}
	}
	/// Page fractions to show (the whole page for a sheet).
	pub fn region(&self) -> Option<Region> {
		match self {
			NodeRef::Sheet(_) => None,
			NodeRef::Part(_, p) => p.region,
		}
	}
	pub fn title(&self) -> String {
		match self {
			NodeRef::Sheet(s) => s.name(),
			NodeRef::Part(s, p) => part_name(s, p),
		}
	}
	pub fn summary(&self) -> &str {
		match self {
			NodeRef::Sheet(s) => &s.summary,
			NodeRef::Part(_, p) => &p.summary,
		}
	}
}

/// "Detail 3 · Stair handrail (A-501)".
pub fn part_name(sheet: &Sheet, p: &Part) -> String {
	let head = match &p.label {
		Some(l) => format!("{} {l}", p.kind.label()),
		None => p.kind.label().to_string(),
	};
	let on = sheet.number.clone().unwrap_or_else(|| format!("p{}", sheet.page));
	if p.title.trim().is_empty() {
		format!("{head} ({on})")
	} else {
		format!("{head} · {} ({on})", p.title.trim())
	}
}

pub fn sheet_id(page: u32) -> String {
	format!("s{page}")
}

pub fn part_id(page: u32, n: usize) -> String {
	format!("s{page}.{n}")
}

/// The page a node id is on ("s12" and "s12.3" → 12).
pub fn page_of(id: &str) -> Option<u32> {
	id.strip_prefix('s')?.split('.').next()?.parse().ok()
}

/// "A-501", "A501", "a 501", "A.501" all compare equal.
pub fn norm_sheet(s: &str) -> String {
	s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect::<String>().trim_start_matches("SHEET").to_string()
}

impl DocMap {
	pub fn new(page_count: u32) -> Self {
		DocMap { version: MAP_VERSION, page_count, ..Default::default() }
	}

	pub fn sheet(&self, page: u32) -> Option<&Sheet> {
		self.sheets.iter().find(|s| s.page == page)
	}

	/// That page's sheet, added (empty) if it isn't indexed yet.
	pub fn sheet_mut(&mut self, page: u32) -> &mut Sheet {
		let i = match self.sheets.binary_search_by_key(&page, |s| s.page) {
			Ok(i) => i,
			Err(i) => {
				self.sheets.insert(i, Sheet::new(page));
				i
			}
		};
		&mut self.sheets[i]
	}

	/// Pages looked at closely.
	pub fn pages_mapped(&self) -> Vec<u32> {
		self.sheets.iter().filter(|s| s.mapped).map(|s| s.page).collect()
	}

	/// Put a sheet in (replacing that page's), keeping page order.
	pub fn upsert(&mut self, sheet: Sheet) {
		match self.sheets.binary_search_by_key(&sheet.page, |s| s.page) {
			Ok(i) => self.sheets[i] = sheet,
			Err(i) => self.sheets.insert(i, sheet),
		}
	}

	/// Add an area to a page; returns its new id.
	pub fn add_part(&mut self, page: u32, mut part: Part) -> String {
		let s = self.sheet_mut(page);
		let n = s.parts.iter().filter_map(|p| p.id.rsplit('.').next()?.parse::<usize>().ok()).max().unwrap_or(0).saturating_add(1);
		part.id = part_id(page, n);
		let id = part.id.clone();
		s.parts.push(part);
		id
	}

	pub fn part_mut(&mut self, id: &str) -> Option<&mut Part> {
		let page = page_of(id)?;
		self.sheets.iter_mut().find(|s| s.page == page)?.parts.iter_mut().find(|p| p.id == id)
	}

	/// Remove a node: an area (with the areas inside it), or a sheet's areas
	/// and its index entry. Links touching what went are removed too.
	pub fn remove(&mut self, id: &str) -> bool {
		let Some(page) = page_of(id) else { return false };
		let Some(si) = self.sheets.iter().position(|s| s.page == page) else { return false };
		let mut gone: Vec<String> = Vec::new();
		if id.contains('.') {
			let parts = &mut self.sheets[si].parts;
			if !parts.iter().any(|p| p.id == id) {
				return false;
			}
			gone.push(id.to_string());
			// And whatever sits inside it, transitively.
			loop {
				let more: Vec<String> = parts.iter().filter(|p| p.parent.as_ref().is_some_and(|q| gone.contains(q)) && !gone.contains(&p.id)).map(|p| p.id.clone()).collect();
				if more.is_empty() {
					break;
				}
				gone.extend(more);
			}
			parts.retain(|p| !gone.contains(&p.id));
		} else {
			gone.push(id.to_string());
			gone.extend(self.sheets[si].parts.iter().map(|p| p.id.clone()));
			self.sheets.remove(si);
		}
		self.links.retain(|l| !gone.contains(&l.from) && !gone.contains(&l.to));
		self.unresolved.retain(|u| !gone.contains(&u.from));
		true
	}

	/// Add a link, or update the note of the same from → to. A link to an
	/// area replaces the same source's link to that area's whole sheet (it was
	/// a placeholder until the sheet was mapped); a link to a sheet is dropped
	/// when the source already points at an area on it.
	pub fn link(&mut self, link: Link) {
		let to_page = page_of(&link.to);
		if link.to.contains('.') {
			let sheet = to_page.map(sheet_id);
			self.links.retain(|l| !(l.from == link.from && Some(&l.to) == sheet.as_ref()));
		} else if self.links.iter().any(|l| l.from == link.from && l.to.contains('.') && page_of(&l.to) == to_page) {
			return;
		}
		if let Some(l) = self.links.iter_mut().find(|l| l.from == link.from && l.to == link.to) {
			*l = link;
		} else {
			self.links.push(link);
		}
	}

	pub fn unlink(&mut self, from: &str, to: &str) -> bool {
		let n = self.links.len();
		self.links.retain(|l| !(l.from == from && l.to == to));
		n != self.links.len()
	}

	pub fn node(&self, id: &str) -> Option<NodeRef<'_>> {
		let page = page_of(id)?;
		let sheet = self.sheet(page)?;
		if id.contains('.') {
			sheet.parts.iter().find(|p| p.id == id).map(|p| NodeRef::Part(sheet, p))
		} else {
			Some(NodeRef::Sheet(sheet))
		}
	}

	/// Every node id: each sheet, then its areas.
	pub fn node_ids(&self) -> Vec<String> {
		let mut out = Vec::new();
		for s in &self.sheets {
			out.push(s.id());
			out.extend(s.parts.iter().map(|p| p.id.clone()));
		}
		out
	}

	/// Links leaving `id`. For a sheet: leaving it or its areas for another page.
	pub fn links_from<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Link> + 'a {
		let sheet = !id.contains('.');
		self.links.iter().filter(move |l| l.from == id || (sheet && page_of(&l.from) == page_of(id) && page_of(&l.to) != page_of(id)))
	}

	/// Links arriving at `id`. For a sheet: at it or its areas, from another page.
	pub fn links_to<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Link> + 'a {
		let sheet = !id.contains('.');
		self.links.iter().filter(move |l| l.to == id || (sheet && page_of(&l.to) == page_of(id) && page_of(&l.from) != page_of(id)))
	}

	/// The sheet whose number is `number`, by normalized comparison.
	pub fn find_sheet(&self, number: &str) -> Option<&Sheet> {
		let n = norm_sheet(number);
		if n.is_empty() {
			return None;
		}
		self.sheets.iter().find(|s| s.number.as_deref().is_some_and(|m| norm_sheet(m) == n))
	}

	/// The area on this page whose region holds (x, y); the smallest wins.
	pub fn part_at(&self, page: u32, x: f64, y: f64) -> Option<&Part> {
		self.sheet(page)?
			.parts
			.iter()
			.filter(|p| p.kind != PartKind::TitleBlock && p.region.is_some_and(|r| r.contains(x, y)))
			.min_by(|a, b| a.region.unwrap().area().total_cmp(&b.region.unwrap().area()))
	}
}

// ── File ──────────────────────────────────────────────────────────────────

pub fn map_path(pdf: &Path) -> PathBuf {
	let mut name = pdf.file_name().map(|n| n.to_os_string()).unwrap_or_default();
	name.push(".map.json");
	pdf.with_file_name(name)
}

/// The map saved next to `pdf`, if there is one (a broken file reads as none).
pub fn load(pdf: &Path) -> Option<DocMap> {
	let bytes = std::fs::read(map_path(pdf)).ok()?;
	serde_json::from_slice(&bytes).ok()
}

/// Atomic save: write a temp file beside the target, then rename over it.
pub fn save(pdf: &Path, map: &DocMap) -> std::io::Result<()> {
	let path = map_path(pdf);
	let json = serde_json::to_vec_pretty(map).map_err(std::io::Error::other)?;
	let tmp = path.with_extension("json.tmp");
	{
		let mut f = std::fs::File::create(&tmp)?;
		f.write_all(&json)?;
		f.sync_all()?;
	}
	std::fs::rename(&tmp, &path)
}

/// Load (or start) the map, change it, save it. Serialised within the
/// process, so parallel agents writing the same map don't lose each other's work.
pub fn edit<R>(pdf: &Path, page_count: u32, f: impl FnOnce(&mut DocMap) -> R) -> std::io::Result<R> {
	static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
	let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
	let mut map = match load(pdf) {
		Some(m) if m.page_count == page_count => m,
		_ => DocMap::new(page_count),
	};
	let r = f(&mut map);
	map.version = MAP_VERSION;
	save(pdf, &map)?;
	Ok(r)
}

pub fn delete(pdf: &Path) -> std::io::Result<()> {
	match std::fs::remove_file(map_path(pdf)) {
		Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
		_ => Ok(()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn part(kind: PartKind, label: Option<&str>, title: &str, region: Region, parent: Option<&str>) -> Part {
		Part { id: String::new(), kind, label: label.map(str::to_string), title: title.into(), summary: String::new(), region: Some(region), parent: parent.map(str::to_string) }
	}

	fn r(x: f64, y: f64, w: f64, h: f64) -> Region {
		Region { x, y, w, h }
	}

	#[test]
	fn areas_links_and_removal() {
		let mut m = DocMap::new(3);
		m.sheet_mut(2).number = Some("A 501".into());
		m.sheet_mut(1).number = Some("A-101".into());
		assert_eq!(m.sheets.iter().map(|s| s.page).collect::<Vec<_>>(), vec![1, 2]);
		let plan = m.add_part(1, part(PartKind::Plan, None, "Level 1", r(0.0, 0.0, 0.8, 1.0), None));
		let bubble = m.add_part(1, part(PartKind::Callout, Some("3/A-501"), "", r(0.2, 0.2, 0.02, 0.02), Some(&plan)));
		let detail = m.add_part(2, part(PartKind::Detail, Some("3"), "Stair rail", r(0.0, 0.0, 0.5, 0.5), None));
		assert_eq!((plan.as_str(), bubble.as_str(), detail.as_str()), ("s1.1", "s1.2", "s2.1"));
		m.link(Link { from: bubble.clone(), to: detail.clone(), kind: LinkKind::Detail, text: "3/A-501".into(), note: String::new() });
		m.link(Link { from: bubble.clone(), to: detail.clone(), kind: LinkKind::Detail, text: "3/A-501".into(), note: "handrail at the north stair".into() });
		assert_eq!(m.links.len(), 1);
		assert_eq!(m.links[0].note, "handrail at the north stair");
		// A placeholder link to a sheet gives way to the precise one, and isn't re-added after it.
		m.link(Link { from: plan.clone(), to: "s2".into(), kind: LinkKind::Reference, text: "SEE A-501".into(), note: String::new() });
		assert_eq!(m.links.len(), 2);
		m.link(Link { from: plan.clone(), to: detail.clone(), kind: LinkKind::Reference, text: "SEE A-501".into(), note: "the stair detail".into() });
		assert_eq!(m.links.iter().filter(|l| l.from == plan).map(|l| l.to.as_str()).collect::<Vec<_>>(), vec!["s2.1"]);
		m.link(Link { from: plan.clone(), to: "s2".into(), kind: LinkKind::Reference, text: String::new(), note: String::new() });
		assert_eq!(m.links.len(), 2);
		m.unlink(&plan, &detail);
		assert_eq!(m.links_from("s1").count(), 1);
		assert_eq!(m.links_to("s2").count(), 1);
		assert_eq!(m.find_sheet("a501").map(|s| s.page), Some(2));
		assert_eq!(m.part_at(1, 0.21, 0.21).map(|p| p.id.as_str()), Some("s1.2"));
		assert_eq!(m.node("s2.1").map(|n| n.title()), Some("Detail 3 · Stair rail (A 501)".into()));
		// Removing the plan takes the callout inside it, and its link.
		assert!(m.remove(&plan));
		assert!(m.sheet(1).unwrap().parts.is_empty());
		assert!(m.links.is_empty());
		// New ids never reuse a number still on the page.
		assert_eq!(m.add_part(2, part(PartKind::Notes, None, "General notes", r(0.5, 0.0, 0.5, 0.5), None)), "s2.2");
	}

	#[test]
	fn kinds_parse_leniently() {
		assert_eq!(page_of("s12.3"), Some(12));
		assert_eq!(page_of("s7"), Some(7));
		assert_eq!(PartKind::parse("Wall Section"), PartKind::Section);
		assert_eq!(PartKind::parse("Enlarged Plan"), PartKind::Plan);
		assert_eq!(PartKind::parse("title-block"), PartKind::TitleBlock);
		assert_eq!(PartKind::parse("section cut"), PartKind::Callout);
		assert_eq!(PartKind::parse("room"), PartKind::Element);
		assert_eq!(LinkKind::parse("matchline"), LinkKind::Continues);
		assert_eq!(LinkKind::parse("note applies to"), LinkKind::Governs);
		let back: PartKind = serde_json::from_str("\"gizmo\"").unwrap();
		assert_eq!(back, PartKind::Other);
		assert_eq!(norm_sheet("Sheet A.501"), "A501");
	}

	#[test]
	fn edits_are_saved_and_survive_reload() {
		let dir = std::env::temp_dir().join(format!("takeoff-docmap-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let pdf = dir.join("set.pdf");
		assert_eq!(map_path(&pdf), dir.join("set.pdf.map.json"));
		assert!(load(&pdf).is_none());
		let id = edit(&pdf, 3, |m| m.add_part(3, part(PartKind::Schedule, None, "Door schedule", r(0.1, 0.1, 0.3, 0.3), None))).unwrap();
		assert_eq!(id, "s3.1");
		let m = load(&pdf).unwrap();
		assert_eq!(m.page_count, 3);
		assert_eq!(m.node("s3.1").map(|n| n.title()), Some("Schedule · Door schedule (p3)".into()));
		// A map of another page count is of an older PDF: edits start fresh.
		edit(&pdf, 4, |m| assert!(m.sheets.is_empty())).unwrap();
		delete(&pdf).unwrap();
		assert!(load(&pdf).is_none());
		let _ = std::fs::remove_dir_all(&dir);
	}
}
