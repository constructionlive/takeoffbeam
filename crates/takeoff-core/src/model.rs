use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
}

impl Category {
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
	Length,
	Area,
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
}

/// One PDF optional-content layer, with what it means for AI runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerInfo {
	pub name: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
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
}
