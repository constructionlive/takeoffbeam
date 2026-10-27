//! The document map tools: `document_map` reads it; `map_sheets`,
//! `map_add` and `map_remove` are how the AI builds it, sheet by sheet, the
//! way it does a takeoff (look, zoom, record); `contact_sheet` skims many
//! pages in one picture for the first survey. Saved next to the PDF after
//! every write, so the graph fills in live and a stopped build resumes.

use std::collections::HashMap;
use std::io::Cursor;

use serde::Deserialize;
use serde_json::Value;
use takeoff_core::docmap::{self, DocMap, Link, LinkKind, NodeRef, Part, PartKind, Region, Unresolved};
use takeoff_pdf::burn::{burn_boxes, burn_label, LabelledBox};
use takeoff_pdf::FracRect;

use super::{view_box_to_page, ToolCtx, ToolOutput, ViewImage};

pub const NO_MAP: &str = "This drawing has no document map yet. The estimator starts one from Graph view (the Page | Graph switch above the drawing), or asks you to map the set. Until then work from get_drawing, search_text and view.";

// ── Reading ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct MapArgs {
	pub page: Option<u32>,
	/// A node id from an earlier answer ("s7" or "s7.2").
	pub node: Option<String>,
	/// Words to look for in sheet numbers, titles and notes.
	pub query: Option<String>,
}

pub fn document_map(ctx: &mut ToolCtx, a: MapArgs) -> Result<ToolOutput, String> {
	let Some(map) = docmap::load(ctx.active_path()).filter(|m| m.page_count == ctx.page_count()) else { return Ok(ToolOutput::text(NO_MAP)) };
	if let Some(id) = a.node.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
		let id = if id.starts_with('s') { id.to_string() } else { format!("s{id}") };
		return match map.node(&id) {
			Some(n) => Ok(ToolOutput::text(node_text(&map, &id, n))),
			None => Err(format!("no node '{id}' in the map. Node ids look like s7 (page 7) or s7.2 (an area on it).")),
		};
	}
	if let Some(page) = a.page {
		let id = docmap::sheet_id(page);
		return match map.node(&id) {
			Some(n) => Ok(ToolOutput::text(node_text(&map, &id, n))),
			None if page >= 1 && page <= map.page_count => Ok(ToolOutput::text(format!("Page {page} is not in the map yet."))),
			None => Err(format!("page {page} does not exist (the drawing has {} pages)", map.page_count)),
		};
	}
	if let Some(q) = a.query.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
		return Ok(ToolOutput::text(search(&map, q)));
	}
	Ok(ToolOutput::text(index(&map)))
}

fn region(r: Option<Region>) -> String {
	match r {
		Some(r) => format!(" region x={:.3} y={:.3} w={:.3} h={:.3}", r.x, r.y, r.w, r.h),
		None => String::new(),
	}
}

fn short_title(map: &DocMap, id: &str) -> String {
	map.node(id).map(|n| n.title()).unwrap_or_else(|| id.to_string())
}

fn link_line(map: &DocMap, l: &Link, outgoing: bool) -> String {
	let other = if outgoing { &l.to } else { &l.from };
	let page = docmap::page_of(other).unwrap_or(0);
	let other_region = map.node(other).and_then(|n| n.region());
	let dir = if outgoing { format!("{} → {other}", l.from) } else { format!("{other} → {}", l.to) };
	let text = if l.text.is_empty() { String::new() } else { format!(" \"{}\"", l.text) };
	let note = if l.note.is_empty() { String::new() } else { format!(" — {}", l.note) };
	format!("{dir} [{}]{text}: {}, page {page}{}{note}\n", l.kind.key(), short_title(map, other), region(other_region))
}

fn node_text(map: &DocMap, id: &str, n: NodeRef) -> String {
	let mut out = String::new();
	match n {
		NodeRef::Sheet(s) => {
			out.push_str(&format!("{id}: page {} · {}{}{}\n", s.page, s.name(), s.discipline.as_deref().map(|d| format!(" ({d})")).unwrap_or_default(), if s.mapped { "" } else { " · indexed, not mapped closely yet" }));
			if !s.summary.is_empty() {
				out.push_str(&format!("{}\n", s.summary));
			}
			if !s.parts.is_empty() {
				out.push_str("Areas (regions are page fractions: view with this page and region to look):\n");
				for p in &s.parts {
					let summary = if p.summary.is_empty() { String::new() } else { format!(": {}", p.summary) };
					let inside = p.parent.as_deref().map(|q| format!(" (in {q})")).unwrap_or_default();
					out.push_str(&format!("{} [{}] {}{inside}{}{summary}\n", p.id, p.kind.key(), docmap::part_name(s, p), region(p.region)));
				}
			}
		}
		NodeRef::Part(s, p) => {
			out.push_str(&format!("{id} [{}]: {} on page {}{}\n", p.kind.key(), docmap::part_name(s, p), s.page, region(p.region)));
			if let Some(q) = &p.parent {
				out.push_str(&format!("Inside {q}: {}\n", short_title(map, q)));
			}
			if !p.summary.is_empty() {
				out.push_str(&format!("{}\n", p.summary));
			}
			let inner: Vec<&Part> = s.parts.iter().filter(|c| c.parent.as_deref() == Some(id)).collect();
			if !inner.is_empty() {
				out.push_str("Contains:\n");
				for c in inner {
					out.push_str(&format!("{} {}{}\n", c.id, docmap::part_name(s, c), region(c.region)));
				}
			}
		}
	}
	let outgoing: Vec<&Link> = map.links_from(id).collect();
	if !outgoing.is_empty() {
		out.push_str("Points to:\n");
		for l in outgoing {
			out.push_str(&link_line(map, l, true));
		}
	}
	let incoming: Vec<&Link> = map.links_to(id).collect();
	if !incoming.is_empty() {
		out.push_str("Referenced from:\n");
		for l in incoming {
			out.push_str(&link_line(map, l, false));
		}
	}
	let page = docmap::page_of(id);
	let loose: Vec<&str> = map.unresolved.iter().filter(|u| u.from == id || (!id.contains('.') && docmap::page_of(&u.from) == page)).map(|u| u.text.as_str()).collect();
	if !loose.is_empty() {
		out.push_str(&format!("Points outside this PDF: {}\n", loose.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(", ")));
	}
	out
}

/// "1-4, 9, 12" for a sorted list of pages.
pub fn ranges(pages: &[u32]) -> String {
	let mut out: Vec<String> = Vec::new();
	let mut i = 0;
	while i < pages.len() {
		let mut j = i;
		while j + 1 < pages.len() && pages[j + 1] == pages[j] + 1 {
			j += 1;
		}
		out.push(if i == j { pages[i].to_string() } else { format!("{}-{}", pages[i], pages[j]) });
		i = j + 1;
	}
	out.join(", ")
}

fn index(map: &DocMap) -> String {
	let mapped = map.pages_mapped();
	let areas: usize = map.sheets.iter().map(|s| s.parts.len()).sum();
	let mut out = format!("Document map: {} of {} pages indexed, {} mapped closely, {areas} areas, {} links", map.sheets.len(), map.page_count, mapped.len(), map.links.len());
	if let Some(m) = &map.model {
		out.push_str(&format!(" (built by {m})"));
	}
	out.push_str(".\n");
	if !map.overview.is_empty() {
		out.push_str(&format!("{}\n", map.overview));
	}
	let missing: Vec<u32> = (1..=map.page_count).filter(|p| map.sheet(*p).is_none()).collect();
	if !missing.is_empty() {
		out.push_str(&format!("Not indexed yet: pages {}\n", ranges(&missing)));
	}
	let unmapped: Vec<u32> = map.sheets.iter().filter(|s| !s.mapped).map(|s| s.page).collect();
	if !unmapped.is_empty() {
		out.push_str(&format!("Indexed but not mapped closely: pages {}\n", ranges(&unmapped)));
	}
	out.push_str("Sheets (id · page · number and title · areas · links out/in):\n");
	for s in &map.sheets {
		let id = s.id();
		let d = s.discipline.as_deref().map(|d| format!(" [{d}]")).unwrap_or_default();
		out.push_str(&format!("{id} · p{} · {}{d} · {} areas · →{} ←{}{}\n", s.page, s.name(), s.parts.len(), map.links_from(&id).count(), map.links_to(&id).count(), if s.mapped { "" } else { " · not mapped" }));
	}
	out.push_str("Ask again with page (or node) for a sheet's areas and links, or query to search.");
	out
}

fn search(map: &DocMap, q: &str) -> String {
	let words: Vec<String> = q.to_lowercase().split_whitespace().map(str::to_string).collect();
	let mut hits = Vec::new();
	for id in map.node_ids() {
		let Some(n) = map.node(&id) else { continue };
		let hay = format!("{} {} {}", n.title(), n.summary(), if let NodeRef::Sheet(s) = n { s.discipline.clone().unwrap_or_default() } else { String::new() }).to_lowercase();
		if words.iter().all(|w| hay.contains(w)) {
			hits.push(format!("{id} · page {} · {}{}\n", n.page(), n.title(), region(n.region())));
		}
	}
	for l in &map.links {
		let hay = format!("{} {}", l.text, l.note).to_lowercase();
		if !hay.trim().is_empty() && words.iter().all(|w| hay.contains(w)) {
			hits.push(format!("link {} → {} [{}] {} {}\n", l.from, l.to, l.kind.key(), l.text, l.note));
		}
	}
	if hits.is_empty() {
		return format!("Nothing in the map matches '{q}'. Try fewer words, or search_text for the sheet's own text.");
	}
	let n = hits.len();
	hits.truncate(80);
	format!("{n} match(es) for '{q}':\n{}", hits.concat())
}

/// For the [Viewer] line: that a map exists, and how far it got.
pub fn presence(pdf: &std::path::Path) -> Option<String> {
	let m = docmap::load(pdf)?;
	Some(format!("document map: {} of {} pages indexed, {} mapped, {} links (document_map)", m.sheets.len(), m.page_count, m.pages_mapped().len(), m.links.len()))
}

// ── Writing ─────────────────────────────────────────────────────────────

fn model_name(ctx: &ToolCtx) -> Option<String> {
	Some(ctx.model_id.clone()).filter(|m| !m.is_empty())
}

fn now() -> String {
	takeoff_core::time::iso_now()
}

fn stamp(ctx: &ToolCtx, m: &mut DocMap) {
	m.built_at = Some(now());
	if m.model.is_none() {
		m.model = model_name(ctx);
	}
}

#[derive(Deserialize)]
pub struct SheetIn {
	pub page: u32,
	pub number: Option<String>,
	pub title: Option<String>,
	pub discipline: Option<String>,
	pub summary: Option<String>,
	/// This sheet has been looked at closely and its areas and links recorded.
	pub mapped: Option<bool>,
}

#[derive(Deserialize)]
pub struct SheetsArgs {
	pub overview: Option<String>,
	pub sheets: Option<Vec<SheetIn>>,
}

pub fn map_sheets(ctx: &mut ToolCtx, a: SheetsArgs) -> Result<ToolOutput, String> {
	let n = ctx.page_count();
	let sheets = a.sheets.unwrap_or_default();
	if let Some(bad) = sheets.iter().find(|s| s.page < 1 || s.page > n) {
		return Err(format!("page {} does not exist (the drawing has {n} pages)", bad.page));
	}
	if sheets.is_empty() && a.overview.is_none() {
		return Err("give sheets (page + number, title, discipline, summary, mapped) and/or overview".into());
	}
	let pdf = ctx.active_path().to_path_buf();
	let (indexed, mapped) = docmap::edit(&pdf, n, |m| {
		for s in &sheets {
			let e = m.sheet_mut(s.page);
			let clean = |v: &Option<String>| v.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
			if let Some(v) = clean(&s.number) {
				e.number = Some(v);
			}
			if let Some(v) = clean(&s.title) {
				e.title = v;
			}
			if let Some(v) = clean(&s.discipline) {
				e.discipline = Some(v);
			}
			if let Some(v) = clean(&s.summary) {
				e.summary = v;
			}
			if let Some(v) = s.mapped {
				e.mapped = v;
			}
		}
		if let Some(o) = a.overview.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
			m.overview = o.to_string();
		}
		stamp(ctx, m);
		(m.sheets.len(), m.pages_mapped().len())
	})
	.map_err(|e| format!("could not save the map: {e}"))?;
	Ok(ToolOutput::text(format!("Saved {} sheet{}. The map now has {indexed} of {n} pages indexed, {mapped} mapped closely.", sheets.len(), if sheets.len() == 1 { "" } else { "s" })))
}

#[derive(Deserialize)]
pub struct AreaIn {
	/// A name to link this area by within the same call ("a1").
	pub key: Option<String>,
	/// An existing area's id, to change it instead of adding one.
	pub id: Option<String>,
	pub view: Option<String>,
	#[serde(rename = "box")]
	pub bx: Option<Value>,
	pub kind: Option<String>,
	pub label: Option<String>,
	pub title: Option<String>,
	pub note: Option<String>,
	/// The area it sits in (an id, or a key from this call).
	pub parent: Option<String>,
}

#[derive(Deserialize)]
pub struct LinkIn {
	pub from: String,
	pub to: Option<String>,
	/// The target is not in this PDF: what it names ("S-201 in the structural set").
	pub outside: Option<String>,
	pub kind: Option<String>,
	pub text: Option<String>,
	pub note: Option<String>,
}

#[derive(Deserialize)]
pub struct AddArgs {
	pub areas: Option<Vec<AreaIn>>,
	pub links: Option<Vec<LinkIn>>,
	/// Mark these pages mapped (looked at closely, areas and links recorded).
	pub mapped_pages: Option<Vec<u32>>,
}

/// A node reference as the model wrote it: a key from this call, a node id
/// ("s7", "s7.2", "7"), or a sheet number ("A-501").
fn resolve_ref(m: &DocMap, keys: &HashMap<String, String>, r: &str, pages: u32) -> Result<String, String> {
	let r = r.trim();
	if let Some(id) = keys.get(r) {
		return Ok(id.clone());
	}
	let id = if r.chars().all(|c| c.is_ascii_digit() || c == '.') && !r.is_empty() { format!("s{r}") } else { r.to_string() };
	if m.node(&id).is_some() {
		return Ok(id);
	}
	if let Some(p) = docmap::page_of(&id).filter(|p| !id.contains('.') && (1..=pages).contains(p)) {
		return Ok(docmap::sheet_id(p));
	}
	if let Some(s) = m.find_sheet(r) {
		return Ok(s.id());
	}
	Err(format!("'{r}' is not a key from this call, a node id (s7, s7.2) or a sheet number in the map"))
}

/// An area just recorded: its page, region and label, for the check render.
type Drawn = (u32, Region, String);
/// What `map_add` reports: areas added or changed, links made, boxes to show.
type Added = (Vec<String>, Vec<String>, Vec<Drawn>);

const COLORS: [(u8, u8, u8); 6] = [(0x4C, 0xC2, 0xFF), (0x34, 0xD3, 0x99), (0xFB, 0xBF, 0x24), (0xA7, 0x8B, 0xFA), (0xF8, 0x71, 0x71), (0xF4, 0x72, 0xB6)];

pub fn map_add(ctx: &mut ToolCtx, a: AddArgs) -> Result<ToolOutput, String> {
	let n = ctx.page_count();
	let areas = a.areas.unwrap_or_default();
	let links = a.links.unwrap_or_default();
	let done = a.mapped_pages.unwrap_or_default();
	if areas.is_empty() && links.is_empty() && done.is_empty() {
		return Err("give areas, links and/or mapped_pages".into());
	}
	if let Some(p) = done.iter().find(|p| **p < 1 || **p > n) {
		return Err(format!("page {p} does not exist (the drawing has {n} pages)"));
	}
	// Every location first, so a bad box changes nothing.
	let mut placed: Vec<Option<(u32, Region)>> = Vec::new();
	for (i, ar) in areas.iter().enumerate() {
		let what = ar.key.clone().or(ar.id.clone()).unwrap_or_else(|| format!("area {}", i + 1));
		match (&ar.view, &ar.bx) {
			(Some(v), Some(b)) => {
				let view = ctx.view(v)?;
				if view.drawing.as_path() != ctx.active_path() {
					return Err(format!("{what}: view {} is of another drawing; switch_drawing to it first", view.id));
				}
				let (x, y, w, h) = view_box_to_page(&view, b).map_err(|e| format!("{what}: {e}"))?;
				placed.push(Some((view.page, Region { x, y, w, h })));
			}
			(None, None) if ar.id.is_some() => placed.push(None),
			_ => return Err(format!("{what}: a new area needs view + box (read off that view), like every location")),
		}
	}
	let pdf = ctx.active_path().to_path_buf();
	let model = model_name(ctx);
	let result = docmap::edit(&pdf, n, |m| -> Result<Added, String> {
		let mut keys: HashMap<String, String> = HashMap::new();
		let mut lines = Vec::new();
		let mut drawn = Vec::new();
		let clean = |v: &Option<String>| v.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
		for (ar, at) in areas.iter().zip(&placed) {
			let id = match &ar.id {
				Some(id) => {
					let id = id.trim().to_string();
					let moved_page = at.map(|(p, _)| p);
					if moved_page.is_some_and(|p| Some(p) != docmap::page_of(&id)) {
						return Err(format!("{id} is on page {}; a box on another page can't move it there (map_remove it and add a new one)", docmap::page_of(&id).unwrap_or(0)));
					}
					let parent = ar.parent.as_deref().map(|q| resolve_ref(m, &keys, q, n)).transpose()?;
					let p = m.part_mut(&id).ok_or_else(|| format!("no area '{id}' in the map"))?;
					if let Some((_, r)) = at {
						p.region = Some(*r);
					}
					if let Some(k) = clean(&ar.kind) {
						p.kind = PartKind::parse(&k);
					}
					if let Some(v) = clean(&ar.label) {
						p.label = Some(v);
					}
					if let Some(v) = clean(&ar.title) {
						p.title = v;
					}
					if let Some(v) = clean(&ar.note) {
						p.summary = v;
					}
					if parent.is_some() {
						p.parent = parent.filter(|q| q.contains('.') && *q != id);
					}
					lines.push(format!("Updated {id}"));
					id
				}
				None => {
					let (page, r) = at.expect("checked above");
					let parent = ar.parent.as_deref().map(|q| resolve_ref(m, &keys, q, n)).transpose()?.filter(|q| q.contains('.') && docmap::page_of(q) == Some(page));
					let part = Part {
						id: String::new(),
						kind: ar.kind.as_deref().map(PartKind::parse).unwrap_or_default(),
						label: clean(&ar.label),
						title: clean(&ar.title).unwrap_or_default(),
						summary: clean(&ar.note).unwrap_or_default(),
						region: Some(r),
						parent,
					};
					let id = m.add_part(page, part);
					lines.push(format!("Added {id}{}", ar.key.as_deref().map(|k| format!(" ({k})")).unwrap_or_default()));
					id
				}
			};
			if let Some(k) = &ar.key {
				keys.insert(k.trim().to_string(), id.clone());
			}
			if let (Some(NodeRef::Part(s, p)), Some(r)) = (m.node(&id), m.node(&id).and_then(|n| n.region())) {
				drawn.push((s.page, r, format!("{id} {}", p.kind.label())));
			}
		}
		let mut link_lines = Vec::new();
		for l in &links {
			let from = resolve_ref(m, &keys, &l.from, n)?;
			let kind = l.kind.as_deref().map(LinkKind::parse).unwrap_or_default();
			let text = clean(&l.text).unwrap_or_default();
			if let Some(out) = clean(&l.outside) {
				if !m.unresolved.iter().any(|u| u.from == from && u.text == out) {
					m.unresolved.push(Unresolved { from: from.clone(), text: out.clone() });
				}
				link_lines.push(format!("{from} → outside \"{out}\""));
				continue;
			}
			let to = resolve_ref(m, &keys, l.to.as_deref().ok_or_else(|| format!("link from {}: give to (or outside)", l.from))?, n)?;
			if to == from {
				return Err(format!("link {from} → {to} points at itself"));
			}
			m.link(Link { from: from.clone(), to: to.clone(), kind, text, note: clean(&l.note).unwrap_or_default() });
			link_lines.push(format!("{from} → {to} [{}]", kind.key()));
		}
		for p in &done {
			m.sheet_mut(*p).mapped = true;
		}
		m.built_at = Some(now());
		if m.model.is_none() {
			m.model = model.clone();
		}
		Ok((lines, link_lines, drawn))
	})
	.map_err(|e| format!("could not save the map: {e}"))??;
	let (lines, link_lines, drawn) = result;
	let mut text = String::new();
	if !lines.is_empty() {
		text.push_str(&format!("{}.\n", lines.join(", ")));
	}
	if !link_lines.is_empty() {
		text.push_str(&format!("Linked: {}.\n", link_lines.join("; ")));
	}
	if !done.is_empty() {
		text.push_str(&format!("Marked mapped: pages {}.\n", ranges(&{
			let mut d = done.clone();
			d.sort_unstable();
			d.dedup();
			d
		})));
	}
	let mut out = ToolOutput::text(text.trim_end().to_string());
	// Show the new boxes on the page with the most of them, so a wrong one is caught now.
	if let Some(page) = drawn.iter().map(|d| d.0).max_by_key(|p| drawn.iter().filter(|d| d.0 == *p).count()) {
		let boxes: Vec<&Drawn> = drawn.iter().filter(|d| d.0 == page).collect();
		if let Ok(img) = check_render(ctx, page, &boxes) {
			out.text.push_str(&format!("\nCheck render {} shows the areas outlined: if a box is off, correct it with map_add (id + view + box).", img.view_id));
			out.images.push(img);
		}
	}
	Ok(out)
}

fn check_render(ctx: &mut ToolCtx, page: u32, boxes: &[&Drawn]) -> Result<ViewImage, String> {
	let (mut x0, mut y0, mut x1, mut y1) = (1.0f64, 1.0f64, 0.0f64, 0.0f64);
	for (_, r, _) in boxes {
		x0 = x0.min(r.x);
		y0 = y0.min(r.y);
		x1 = x1.max(r.x + r.w);
		y1 = y1.max(r.y + r.h);
	}
	let pad = ((x1 - x0).max(y1 - y0) * 0.15).max(0.03);
	let (x0, y0) = ((x0 - pad).max(0.0), (y0 - pad).max(0.0));
	let (x1, y1) = ((x1 + pad).min(1.0), (y1 + pad).min(1.0));
	let region = FracRect { x: x0, y: y0, w: (x1 - x0).max(0.02), h: (y1 - y0).max(0.02) };
	let max = takeoff_core::coords::safe_max_edge(&ctx.model_id, 1400);
	let r = ctx.drawing.render(page, 300.0, region, Some(max)).map_err(|e| e.to_string())?;
	let mut img = r.image;
	let (fw, fh) = takeoff_core::coords::image_fit(&ctx.model_id, img.width(), img.height());
	if (fw, fh) != img.dimensions() {
		img = image::imageops::resize(&img, fw, fh, image::imageops::FilterType::Triangle);
	}
	let list: Vec<LabelledBox> = boxes.iter().enumerate().map(|(i, (_, b, l))| ((b.x, b.y, b.w, b.h), l.clone(), COLORS[i % COLORS.len()])).collect();
	burn_boxes(&mut img, r.region, &list);
	let view = ctx.register_view(page, r.region, img.width(), img.height());
	let mut png = Vec::new();
	img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
	let caption = format!("View {} · page {page} · region x={:.3} y={:.3} w={:.3} h={:.3} · {}x{} px. {}. The map areas just recorded, outlined.", view.id, r.region.x, r.region.y, r.region.w, r.region.h, img.width(), img.height(), view.dialect.box_request(img.width(), img.height()));
	Ok(ViewImage { png, width: img.width(), height: img.height(), view_id: view.id, caption })
}

#[derive(Deserialize)]
pub struct LinkRef {
	pub from: String,
	pub to: String,
}

#[derive(Deserialize)]
pub struct RemoveArgs {
	pub ids: Option<Vec<String>>,
	pub links: Option<Vec<LinkRef>>,
}

pub fn map_remove(ctx: &mut ToolCtx, a: RemoveArgs) -> Result<ToolOutput, String> {
	let n = ctx.page_count();
	let ids = a.ids.unwrap_or_default();
	let links = a.links.unwrap_or_default();
	if ids.is_empty() && links.is_empty() {
		return Err("give ids (areas or sheets) and/or links (from, to)".into());
	}
	let pdf = ctx.active_path().to_path_buf();
	let (gone, missing, unlinked) = docmap::edit(&pdf, n, |m| {
		let (mut gone, mut missing) = (Vec::new(), Vec::new());
		for id in &ids {
			if m.remove(id.trim()) {
				gone.push(id.trim().to_string());
			} else {
				missing.push(id.trim().to_string());
			}
		}
		let unlinked = links.iter().filter(|l| m.unlink(l.from.trim(), l.to.trim())).count();
		(gone, missing, unlinked)
	})
	.map_err(|e| format!("could not save the map: {e}"))?;
	let mut out = format!("Removed {} node{} and {unlinked} link{}.", gone.len(), if gone.len() == 1 { "" } else { "s" }, if unlinked == 1 { "" } else { "s" });
	if !missing.is_empty() {
		out.push_str(&format!(" Not in the map: {}.", missing.join(", ")));
	}
	Ok(ToolOutput::text(out))
}

// ── Skimming ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ContactArgs {
	/// e.g. "1-24" (at most 24 pages per call).
	pub pages: Option<String>,
}

const CONTACT_MAX: usize = 24;

pub fn parse_pages(spec: &str, n: u32) -> Result<Vec<u32>, String> {
	let mut out = Vec::new();
	for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
		let (a, b) = match part.split_once('-') {
			Some((a, b)) => (a.trim().parse::<u32>(), b.trim().parse::<u32>()),
			None => (part.parse::<u32>(), part.parse::<u32>()),
		};
		let (Ok(a), Ok(b)) = (a, b) else { return Err(format!("can't read pages '{spec}' (e.g. \"1-12, 15\")")) };
		// Only the drawing's own pages: "1-4000000000" from a model must not loop four billion times.
		for p in a.min(b).max(1)..=a.max(b).min(n) {
			if !out.contains(&p) {
				out.push(p);
			}
		}
	}
	if out.is_empty() {
		return Err(format!("no pages in '{spec}' (the drawing has {n})"));
	}
	Ok(out)
}

/// Text in the usual title-block places (right strip, bottom strip), squeezed.
fn title_text(ctx: &ToolCtx, page: u32) -> String {
	let mut t = String::new();
	for r in [FracRect { x: 0.8, y: 0.55, w: 0.2, h: 0.45 }, FracRect { x: 0.45, y: 0.86, w: 0.55, h: 0.14 }] {
		if let Ok(s) = ctx.drawing.text_in(page, r) {
			t.push(' ');
			t.push_str(&s);
		}
	}
	let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
	if t.chars().count() > 220 {
		format!("{}…", t.chars().take(220).collect::<String>())
	} else {
		t
	}
}

pub fn contact_sheet(ctx: &mut ToolCtx, a: ContactArgs) -> Result<ToolOutput, String> {
	let n = ctx.page_count();
	let spec = a.pages.unwrap_or_else(|| if n as usize <= CONTACT_MAX { format!("1-{n}") } else { format!("1-{CONTACT_MAX}") });
	let mut pages = parse_pages(&spec, n)?;
	let more = pages.len() > CONTACT_MAX;
	pages.truncate(CONTACT_MAX);
	let cols = ((pages.len() as f64 * 1.4).sqrt().ceil() as usize).clamp(1, 6);
	let rows = pages.len().div_ceil(cols);
	// The whole grid must survive the model's image limit with the tiles legible.
	let (gw, gh) = takeoff_core::coords::image_fit(&ctx.model_id, 2400, (2400.0 * rows as f64 / cols as f64 * 0.72) as u32);
	let (tw, th) = (gw / cols as u32, gh / rows as u32);
	// Grey behind the pages, so each sheet's edge shows.
	let mut grid = image::RgbaImage::from_pixel(gw, gh, image::Rgba([0xD4, 0xD8, 0xDE, 255]));
	let mut lines = Vec::new();
	for (i, page) in pages.iter().enumerate() {
		let (cx, cy) = ((i % cols) as u32 * tw, (i / cols) as u32 * th);
		if let Ok(r) = ctx.drawing.render(*page, 72.0, FracRect::FULL, Some(tw.max(th))) {
			let img = r.image;
			let s = ((tw - 8) as f64 / img.width() as f64).min((th - 8) as f64 / img.height() as f64);
			let small = image::imageops::resize(&img, ((img.width() as f64 * s) as u32).max(1), ((img.height() as f64 * s) as u32).max(1), image::imageops::FilterType::Triangle);
			image::imageops::overlay(&mut grid, &small, (cx + (tw - small.width()) / 2) as i64, (cy + (th - small.height()) / 2) as i64);
		}
		burn_label(&mut grid, cx as f32 + 6.0, cy as f32 + 6.0, (th as f32 / 9.0).clamp(12.0, 26.0), &format!("p{page}"), (255, 255, 255), (0x1F, 0x6F, 0xEB));
		let label = ctx.drawing.page_label(*page).ok().flatten().filter(|l| *l != page.to_string()).map(|l| format!(" (PDF label \"{l}\")")).unwrap_or_default();
		let tb = title_text(ctx, *page);
		lines.push(format!("p{page}{label}: {}", if tb.is_empty() { "no text layer in the title block".into() } else { tb }));
	}
	let mut png = Vec::new();
	grid.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
	let caption = format!("Contact sheet of pages {} ({cols} per row, each tile labelled with its page). Too small to read detail or to take locations from: view a page to look closely.", ranges(&{
		let mut p = pages.clone();
		p.sort_unstable();
		p
	}));
	let mut text = format!("{caption}\nTitle-block text per page (the PDF's own text, may be partial):\n{}", lines.join("\n"));
	if more {
		text.push_str(&format!("\nOnly the first {CONTACT_MAX} pages are shown: ask for the rest in another call."));
	}
	Ok(ToolOutput { text, images: vec![ViewImage { png, width: gw, height: gh, view_id: "contact".into(), caption }] })
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn page_ranges_stay_on_the_drawing() {
		assert_eq!(parse_pages("3-1, 2, 9", 5).unwrap(), vec![1, 2, 3]);
		assert_eq!(parse_pages("4-4000000000", 6).unwrap(), vec![4, 5, 6]);
		assert!(parse_pages("0", 6).is_err());
		assert!(parse_pages("7-9", 6).is_err());
	}
}
