use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Fresh id with a prefix (`m`, `ms`, `an`, `c`, `vp`), unique within a process run
/// and across runs (time-based), like the platform's `newTakeoffId`.
pub fn new_id(prefix: &str) -> String {
	use std::sync::atomic::{AtomicU64, Ordering};
	static COUNTER: AtomicU64 = AtomicU64::new(0);
	let n = COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
	format!("{prefix}_{}_{}", radix36(now_ms() as u64), radix36(n))
}

pub fn now_ms() -> i64 {
	std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn radix36(mut n: u64) -> String {
	const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
	if n == 0 {
		return "0".into();
	}
	let mut out = Vec::new();
	while n > 0 {
		out.push(DIGITS[(n % 36) as usize]);
		n /= 36;
	}
	out.reverse();
	String::from_utf8(out).unwrap()
}

/// Sidecar format version. Keep in step with construction.live `takeoffTypes.ts`.
pub const TAKEOFF_VERSION: u32 = 1;

pub const MEASURE_UNITS: [&str; 5] = ["ft", "in", "m", "cm", "mm"];

pub const CATEGORY_COLORS: [&str; 8] = [
	"#4CC2FF", // blue
	"#34D399", // emerald
	"#FBBF24", // amber
	"#A78BFA", // violet
	"#F87171", // red
	"#F472B6", // pink
	"#22D3EE", // cyan
	"#A3E635", // lime
];

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
	pub x: f64,
	pub y: f64,
}

/// A rectangle on a page, as fractions of the full page.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
	pub page: u32,
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
	pub page: u32,
	pub width_px: f64,
	pub height_px: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CategoryKind {
	#[default]
	Count,
	Length,
	Area,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineStyle {
	Solid,
	Dashed,
	Dotted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hatch {
	Diagonal,
	Cross,
	Grid,
	Dots,
}

/// A takeoff item (group): every mark or measurement belongs to at most one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
	pub id: String,
	pub name: String,
	pub color: String,
	/// What the item looks like on the drawing, in the user's words. AI runs read it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
	/// Unset means "count" (back-compat with early files).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub kind: Option<CategoryKind>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub icon: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub line_style: Option<LineStyle>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub hatch: Option<Hatch>,
	/// Platform pricing link; carried through untouched.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub chest_item_id: Option<String>,
	/// Mark fill/icon opacity, 0.15–1. Unset = opaque.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub opacity: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub legend_crop: Option<String>,
	/// Where the reference symbol sits (usually the legend).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub legend_region: Option<Region>,
	/// Price per unit (each / ft / ft² …) for the cost column.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub unit_cost: Option<f64>,
	/// Hidden in the viewer (Bluebeam "hide markups of this subject").
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub hidden: bool,
}

impl Category {
	pub fn new(id: impl Into<String>, name: impl Into<String>, color: impl Into<String>, kind: CategoryKind) -> Self {
		Self {
			id: id.into(),
			name: name.into(),
			color: color.into(),
			description: None,
			kind: Some(kind),
			icon: None,
			line_style: None,
			hatch: None,
			chest_item_id: None,
			opacity: None,
			legend_crop: None,
			legend_region: None,
			unit_cost: None,
			hidden: false,
		}
	}
	pub fn kind(&self) -> CategoryKind {
		self.kind.unwrap_or_default()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
	Ai,
	#[default]
	User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkStatus {
	#[default]
	Pending,
	Confirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileRef {
	pub row: u32,
	pub col: u32,
}

/// Who made an item and when, plus Bluebeam-style list metadata. Flattened
/// into marks, measurements and annotations; every field is optional so
/// platform files (which have none of these) load unchanged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemMeta {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub author: Option<String>,
	/// Unix milliseconds.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub created_at: Option<i64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub modified_at: Option<i64>,
	/// Markup layer (Bluebeam "Layer" column), not a PDF layer.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub layer: Option<String>,
	/// Review status label ("Accepted", "Rejected", "Completed", …).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub review: Option<String>,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub locked: bool,
}

/// A counted symbol: top-left + size as full-page fractions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
	pub id: String,
	/// Unset = ungrouped quick mark.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub category_id: Option<String>,
	pub page: u32,
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
	#[serde(default)]
	pub source: Source,
	#[serde(default)]
	pub status: MarkStatus,
	/// Provenance only, never used for positioning.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tile: Option<TileRef>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub note: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub fields: Option<BTreeMap<String, String>>,
	#[serde(flatten)]
	pub meta: ItemMeta,
}

/// A user-defined markups-list column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDef {
	pub id: String,
	pub name: String,
}

/// Two reference points + the real distance between them. Units-per-pixel is
/// derived at use time, so the scale is independent of render DPI and zoom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageScale {
	pub page: u32,
	pub p1: Point,
	pub p2: Point,
	pub distance: f64,
	pub unit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeasurementType {
	/// Polyline; `closed` makes it a perimeter.
	Length,
	/// Polygon (implicitly closed), optional cutouts and depth (→ volume).
	Area,
	/// Three points: arm end, vertex, arm end. Not scaled.
	Angle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
	pub id: String,
	pub page: u32,
	#[serde(rename = "type")]
	pub kind: MeasurementType,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub category_id: Option<String>,
	/// Full-page fractions; area polygons are implicitly closed.
	pub points: Vec<Point>,
	/// Unset = user-drawn.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub source: Option<Source>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub label: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub note: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub color: Option<String>,
	/// Derived from points + page scale on save; a convenience for consumers.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub value: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub unit: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub fields: Option<BTreeMap<String, String>>,
	/// Length only: closing segment counted (perimeter).
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub closed: bool,
	/// Area only: holes subtracted from the area (Bluebeam "cutout").
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub cutouts: Vec<Vec<Point>>,
	/// Area only: depth in the scale's unit; turns the area into a volume.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub depth: Option<f64>,
	#[serde(flatten)]
	pub meta: ItemMeta,
}

impl Measurement {
	pub fn new(id: impl Into<String>, page: u32, kind: MeasurementType, points: Vec<Point>) -> Self {
		Self {
			id: id.into(),
			page,
			kind,
			category_id: None,
			points,
			source: None,
			label: None,
			note: None,
			color: None,
			value: None,
			unit: None,
			fields: None,
			closed: false,
			cutouts: vec![],
			depth: None,
			meta: ItemMeta::default(),
		}
	}
}

/// What an annotation markup draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationKind {
	/// points: [top-left, bottom-right]
	Text,
	/// points: [box top-left, box bottom-right, leader tip]
	Callout,
	/// points: polygon vertices, drawn with scalloped edges
	Cloud,
	/// points: [corner, opposite corner]
	Rectangle,
	/// points: [corner, opposite corner] of the bounding box
	Ellipse,
	/// points: [start, end]
	Line,
	/// points: [tail, head]
	Arrow,
	Polyline,
	Polygon,
	/// points: [corner, opposite corner], drawn translucent, multiply-like
	Highlight,
	/// points: freehand stroke
	Ink,
	/// points: [top-left, bottom-right]; text = stamp label ("APPROVED")
	Stamp,
}

impl AnnotationKind {
	pub const ALL: [AnnotationKind; 12] = [
		AnnotationKind::Text,
		AnnotationKind::Callout,
		AnnotationKind::Cloud,
		AnnotationKind::Rectangle,
		AnnotationKind::Ellipse,
		AnnotationKind::Line,
		AnnotationKind::Arrow,
		AnnotationKind::Polyline,
		AnnotationKind::Polygon,
		AnnotationKind::Highlight,
		AnnotationKind::Ink,
		AnnotationKind::Stamp,
	];
	pub fn label(self) -> &'static str {
		match self {
			AnnotationKind::Text => "Text Box",
			AnnotationKind::Callout => "Callout",
			AnnotationKind::Cloud => "Cloud",
			AnnotationKind::Rectangle => "Rectangle",
			AnnotationKind::Ellipse => "Ellipse",
			AnnotationKind::Line => "Line",
			AnnotationKind::Arrow => "Arrow",
			AnnotationKind::Polyline => "Polyline",
			AnnotationKind::Polygon => "Polygon",
			AnnotationKind::Highlight => "Highlight",
			AnnotationKind::Ink => "Pen",
			AnnotationKind::Stamp => "Stamp",
		}
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationStyle {
	pub stroke: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub fill: Option<String>,
	/// Stroke width in PDF points (scales with zoom like the drawing does).
	pub width: f64,
	pub opacity: f64,
	/// Font size in PDF points.
	#[serde(default = "default_font_size")]
	pub font_size: f64,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub line_style: Option<LineStyle>,
}

fn default_font_size() -> f64 {
	10.0
}

impl Default for AnnotationStyle {
	fn default() -> Self {
		Self { stroke: "#E8531A".into(), fill: None, width: 1.5, opacity: 1.0, font_size: 10.0, line_style: None }
	}
}

/// A Bluebeam-style markup that is not a quantity: text, clouds, arrows, pen…
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
	pub id: String,
	pub page: u32,
	pub kind: AnnotationKind,
	pub points: Vec<Point>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub text: Option<String>,
	#[serde(default)]
	pub style: AnnotationStyle,
	/// Markups-list "Subject"; defaults to the kind label.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub subject: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub note: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub source: Option<Source>,
	#[serde(flatten)]
	pub meta: ItemMeta,
}

/// A picture laid over part of a page, semi-transparent: a map, a diagram, a
/// colour-coded correlation the agent (or a person) drew to show something on
/// the sheet itself. Not a quantity and not burned into exports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
	pub id: String,
	pub page: u32,
	/// Where it sits, in page fractions (top-left + size).
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
	/// SVG markup, stored inline so the overlay travels with the takeoff.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub svg: Option<String>,
	/// Or a PNG/JPEG file on disk (absolute path), e.g. an HTML page rendered to an image.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub image: Option<String>,
	/// 0 = invisible, 1 = opaque.
	#[serde(default = "default_overlay_opacity")]
	pub opacity: f64,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub label: Option<String>,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub hidden: bool,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub source: Option<Source>,
	#[serde(flatten)]
	pub meta: ItemMeta,
}

fn default_overlay_opacity() -> f64 {
	0.6
}

/// A region of a page with its own scale (Bluebeam "viewport"): details drawn
/// at a different scale than the plan on the same sheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
	pub id: String,
	pub page: u32,
	pub name: String,
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
	pub scale: PageScale,
}

impl Viewport {
	pub fn contains(&self, p: Point) -> bool {
		p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
	}
}

/// A named region of a page (Bluebeam "Space"): "Level 2", "Unit 204",
/// "Kitchen". Quantities inside it are attributed to it; spaces nest by
/// geometry, and the smallest one around an item wins (see `spaces`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
	pub id: String,
	pub page: u32,
	pub name: String,
	pub color: String,
	/// Polygon in page fractions, implicitly closed.
	pub points: Vec<Point>,
	/// Hidden on the sheet only; still attributes quantities.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub hidden: bool,
	#[serde(flatten)]
	pub meta: ItemMeta,
}

/// Which items a legend lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LegendScope {
	/// Items on the legend's own page.
	#[default]
	Page,
	/// Items on every page.
	Document,
	/// A scope this version doesn't know: read as the page.
	#[serde(other)]
	Unknown,
}

/// Legend columns besides the swatch and the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegendColumns {
	#[serde(default = "yes")]
	pub count: bool,
	#[serde(default = "yes")]
	pub quantity: bool,
	#[serde(default)]
	pub cost: bool,
}

fn yes() -> bool {
	true
}

impl Default for LegendColumns {
	fn default() -> Self {
		Self { count: true, quantity: true, cost: false }
	}
}

/// A markup legend on the sheet (Bluebeam "Legend"): a box listing the items
/// in scope with swatch, name, count and total. Its rows are computed, never
/// stored (see `legend`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Legend {
	pub id: String,
	pub page: u32,
	/// Top-left + size, page fractions.
	pub x: f64,
	pub y: f64,
	pub w: f64,
	pub h: f64,
	/// Unset = "Legend".
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	#[serde(default)]
	pub scope: LegendScope,
	/// Only these items (category ids); empty = every item in scope.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub category_ids: Vec<String>,
	#[serde(default)]
	pub columns: LegendColumns,
	/// Also list markups (text, clouds…) by subject.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub markups: bool,
	#[serde(flatten)]
	pub meta: ItemMeta,
}

impl Legend {
	pub fn new(id: impl Into<String>, page: u32, x: f64, y: f64, w: f64, h: f64) -> Self {
		Self { id: id.into(), page, x, y, w, h, title: None, scope: LegendScope::Page, category_ids: vec![], columns: LegendColumns::default(), markups: false, meta: ItemMeta::default() }
	}
	pub fn title(&self) -> &str {
		self.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or("Legend")
	}
}

/// How lengths are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LengthFormat {
	/// 12.50 ft
	#[default]
	Decimal,
	/// 12'-6 1/2" (imperial scales only; fractions to `fraction_denominator`)
	FeetInches,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySettings {
	#[serde(default)]
	pub length_format: LengthFormat,
	/// Decimal places.
	#[serde(default = "default_precision")]
	pub precision: u8,
	/// 2, 4, 8, 16, 32 or 64.
	#[serde(default = "default_denominator")]
	pub fraction_denominator: u32,
}

fn default_precision() -> u8 {
	2
}
fn default_denominator() -> u32 {
	16
}

impl Default for DisplaySettings {
	fn default() -> Self {
		Self { length_format: LengthFormat::Decimal, precision: 2, fraction_denominator: 16 }
	}
}

/// One PDF optional-content layer, with what it means for AI runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerInfo {
	pub name: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
}

/// An earlier revision of a sheet, kept when a newer one was slip-sheeted
/// over it: the old page lives on in a history PDF beside the drawing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetRevision {
	/// The page it is an earlier revision of (as the drawing is now).
	pub page: u32,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sheet: Option<String>,
	/// The history PDF, relative to the drawing's folder (or absolute).
	pub file: String,
	/// Its page in that file.
	pub file_page: u32,
	pub replaced_at: String,
	/// The new revision's file name and page, for the record.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub from: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub from_page: Option<u32>,
	/// One id per slip-sheet: everything replaced together.
	pub slip: String,
	/// Share of the sheet that changed (0–1).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub changed: Option<f64>,
	/// Items that sit on changed parts of the sheet: they may be wrong now.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub review: Vec<String>,
	/// E.g. the page size changed and the items were scaled to fit.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub warning: Option<String>,
}

/// The whole takeoff for one drawing file (the `.takeoff.json` sidecar).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoffData {
	pub version: u32,
	/// Always "takeoff".
	pub review_type: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_updated: Option<String>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub pages: Vec<PageInfo>,
	#[serde(default)]
	pub categories: Vec<Category>,
	#[serde(default)]
	pub marks: Vec<Mark>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub custom_fields: Vec<FieldDef>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub page_scales: Vec<PageScale>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub measurements: Vec<Measurement>,
	/// Layer NAMES turned off, in the viewer and in AI renders alike.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub hidden_layers: Vec<String>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub layers: Vec<LayerInfo>,
	// ── Takeoff Desk extensions (the platform ignores these) ──
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub annotations: Vec<Annotation>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub viewports: Vec<Viewport>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub overlays: Vec<Overlay>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub spaces: Vec<Space>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub legends: Vec<Legend>,
	#[serde(default)]
	pub display: DisplaySettings,
	/// Earlier revisions of slip-sheeted pages, oldest first (see `slip`).
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub sheet_revisions: Vec<SheetRevision>,
	/// Which version of the PDF file this takeoff goes with, within a session:
	/// page tools and slip-sheets rewrite the file, and undo puts the matching
	/// file back. Never saved.
	#[serde(skip)]
	pub pdf_state: u64,
}

impl Default for TakeoffData {
	fn default() -> Self {
		Self {
			version: TAKEOFF_VERSION,
			review_type: "takeoff".into(),
			last_updated: None,
			pages: vec![],
			categories: vec![],
			marks: vec![],
			custom_fields: vec![],
			page_scales: vec![],
			measurements: vec![],
			hidden_layers: vec![],
			layers: vec![],
			annotations: vec![],
			viewports: vec![],
			overlays: vec![],
			spaces: vec![],
			legends: vec![],
			display: DisplaySettings::default(),
			sheet_revisions: vec![],
			pdf_state: 0,
		}
	}
}

impl TakeoffData {
	pub fn next_category_color(&self) -> &'static str {
		CATEGORY_COLORS[self.categories.len() % CATEGORY_COLORS.len()]
	}
	pub fn scale_for(&self, page: u32) -> Option<&PageScale> {
		self.page_scales.iter().find(|s| s.page == page)
	}

	/// The scale that applies at `at` on `page`: the innermost viewport that
	/// contains the point, else the page calibration.
	pub fn scale_at(&self, page: u32, at: Point) -> Option<&PageScale> {
		self.viewports
			.iter()
			.filter(|v| v.page == page && v.contains(at))
			.min_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h)))
			.map(|v| &v.scale)
			.or_else(|| self.scale_for(page))
	}

	pub fn category(&self, id: &str) -> Option<&Category> {
		self.categories.iter().find(|c| c.id == id)
	}

	pub fn page_info(&self, page: u32) -> Option<&PageInfo> {
		self.pages.iter().find(|p| p.page == page)
	}

	/// The revision this page replaced most recently, if it was slip-sheeted.
	pub fn previous_revision(&self, page: u32) -> Option<&SheetRevision> {
		self.sheet_revisions.iter().rev().find(|r| r.page == page)
	}
}
