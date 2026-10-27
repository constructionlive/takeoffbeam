//! The tool palette: what each tool is called, its icon, shortcut and group.

use takeoff_core::{AnnotationKind, CategoryKind};

use crate::icons as i;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
	Select,
	Pan,
	Count,
	Length,
	/// A length along a smooth curve through the points clicked.
	Curve,
	/// A length along the drawn line between the points clicked.
	Trace,
	Perimeter,
	Area,
	/// The space around the point clicked, outlined along the drawn lines.
	Fill,
	RectArea,
	Volume,
	Cutout,
	Angle,
	Calibrate,
	Viewport,
	FollowLine,
	Annot(AnnotationKind),
	/// A curved line markup (stored as a polyline along the curve).
	CurveMarkup,
	/// A named region quantities are attributed to, clicked corner by corner.
	Space,
	/// A space outlined along the drawn lines around the point clicked.
	SpaceFill,
	/// A markup legend box.
	Legend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
	Navigate,
	Measure,
	Markup,
}

impl Tool {
	pub const MEASURE: [Tool; 14] = [Tool::Count, Tool::Length, Tool::Curve, Tool::Trace, Tool::Perimeter, Tool::Area, Tool::Fill, Tool::RectArea, Tool::Volume, Tool::Cutout, Tool::Angle, Tool::FollowLine, Tool::Calibrate, Tool::Viewport];
	/// Not on the toolbar (it's full): started from the Takeoff tab.
	pub const ORGANIZE: [Tool; 3] = [Tool::Space, Tool::SpaceFill, Tool::Legend];
	pub const MARKUP: [Tool; 13] = [
		Tool::Annot(AnnotationKind::Text),
		Tool::Annot(AnnotationKind::Callout),
		Tool::Annot(AnnotationKind::Cloud),
		Tool::Annot(AnnotationKind::Rectangle),
		Tool::Annot(AnnotationKind::Ellipse),
		Tool::Annot(AnnotationKind::Line),
		Tool::Annot(AnnotationKind::Arrow),
		Tool::Annot(AnnotationKind::Polyline),
		Tool::CurveMarkup,
		Tool::Annot(AnnotationKind::Polygon),
		Tool::Annot(AnnotationKind::Highlight),
		Tool::Annot(AnnotationKind::Ink),
		Tool::Annot(AnnotationKind::Stamp),
	];

	pub fn label(self) -> &'static str {
		match self {
			Tool::Select => "Select",
			Tool::Pan => "Pan",
			Tool::Count => "Count",
			Tool::Length => "Length",
			Tool::Curve => "Curve",
			Tool::Trace => "Trace Line",
			Tool::CurveMarkup => "Curve",
			Tool::Perimeter => "Perimeter",
			Tool::Area => "Area",
			Tool::Fill => "Fill Area",
			Tool::RectArea => "Rectangle Area",
			Tool::Volume => "Volume",
			Tool::Cutout => "Cutout",
			Tool::Angle => "Angle",
			Tool::Calibrate => "Calibrate",
			Tool::Viewport => "Viewport",
			Tool::FollowLine => "Follow Line",
			Tool::Space => "Space",
			Tool::SpaceFill => "Fill Space",
			Tool::Legend => "Legend",
			Tool::Annot(k) => k.label(),
		}
	}

	pub fn icon(self) -> &'static str {
		match self {
			Tool::Select => i::MOUSE_POINTER_2,
			Tool::Pan => i::HAND,
			Tool::Count => i::HASH,
			Tool::Length => i::RULER,
			Tool::Curve | Tool::CurveMarkup => i::SPLINE,
			Tool::Trace => i::ROUTE,
			Tool::Perimeter => i::WAYPOINTS,
			Tool::Area => i::PENTAGON,
			Tool::Fill => i::PAINT_BUCKET,
			Tool::RectArea => i::SQUARE_DASHED,
			Tool::Volume => i::BOX,
			Tool::Cutout => i::SCISSORS,
			Tool::Angle => i::TRIANGLE_RIGHT,
			Tool::Calibrate => i::SCALING,
			Tool::Viewport => i::FRAME,
			Tool::FollowLine => i::SCAN,
			Tool::Space => i::LASSO,
			Tool::SpaceFill => i::SQUARE_DOT,
			Tool::Legend => i::TABLE,
			Tool::Annot(k) => match k {
				AnnotationKind::Text => i::TYPE,
				AnnotationKind::Callout => i::MESSAGE_SQUARE_TEXT,
				AnnotationKind::Cloud => i::CLOUD,
				AnnotationKind::Rectangle => i::SQUARE,
				AnnotationKind::Ellipse => i::CIRCLE,
				AnnotationKind::Line => i::MINUS,
				AnnotationKind::Arrow => i::MOVE_UP_RIGHT,
				AnnotationKind::Polyline => i::WAYPOINTS,
				AnnotationKind::Polygon => i::HEXAGON,
				AnnotationKind::Highlight => i::HIGHLIGHTER,
				AnnotationKind::Ink => i::PEN_LINE,
				AnnotationKind::Stamp => i::STAMP,
			},
		}
	}

	pub fn shortcut(self) -> Option<char> {
		Some(match self {
			// Every letter is taken.
			Tool::Curve | Tool::Trace | Tool::CurveMarkup | Tool::Fill | Tool::Space | Tool::SpaceFill | Tool::Legend => return None,
			Tool::Select => 'V',
			Tool::Pan => 'H',
			Tool::Count => 'C',
			Tool::Length => 'L',
			Tool::Perimeter => 'P',
			Tool::Area => 'A',
			Tool::RectArea => 'B',
			Tool::Volume => 'U',
			Tool::Cutout => 'X',
			Tool::Angle => 'G',
			Tool::Calibrate => 'K',
			Tool::Viewport => 'Z',
			Tool::FollowLine => 'F',
			Tool::Annot(k) => match k {
				AnnotationKind::Text => 'T',
				AnnotationKind::Callout => 'Q',
				AnnotationKind::Cloud => 'O',
				AnnotationKind::Rectangle => 'R',
				AnnotationKind::Ellipse => 'E',
				AnnotationKind::Line => 'N',
				AnnotationKind::Arrow => 'W',
				AnnotationKind::Polyline => 'Y',
				AnnotationKind::Polygon => 'J',
				AnnotationKind::Highlight => 'I',
				AnnotationKind::Ink => 'D',
				AnnotationKind::Stamp => 'M',
			},
		})
	}

	pub fn hint(self) -> &'static str {
		match self {
			Tool::Select => "Click to select, drag to move, drag on empty space to box-select. Pan: two fingers, or hold Space",
			Tool::Pan => "Drag to pan. Tip: hold Space with any tool",
			Tool::Count => "Click each symbol to count it",
			Tool::Length | Tool::Perimeter => "Click points · double-click or Enter to finish · Backspace removes a point · Shift = 45°",
			Tool::Curve => "Click points along the curve (more on a tight bend) · double-click or Enter to finish",
			Tool::Trace => "Click points ON a drawn line (start, end, where it forks) · it follows the line between them · double-click or Enter to finish",
			Tool::CurveMarkup => "Click points along the curve · double-click to finish",
			Tool::Area | Tool::Volume => "Click the corners · double-click or Enter to close",
			Tool::Fill => "Click inside a room or space to measure it (doorways close by themselves) · Shift-click another piece to add it",
			Tool::RectArea => "Drag a rectangle",
			Tool::Cutout => "Select an area first, then click the hole's corners",
			Tool::Angle => "Click arm, vertex, arm",
			Tool::Calibrate => "Click both ends of a known dimension",
			Tool::Viewport => "Drag around a detail drawn at a different scale",
			Tool::FollowLine => "Click a line on the drawing to measure the whole run",
			Tool::Space => "Click the corners of a level, unit or room · double-click or Enter to close · quantities inside belong to it",
			Tool::SpaceFill => "Click inside a room to make it a space, outlined along the drawn lines",
			Tool::Legend => "Drag a box for a legend of the items on this page",
			Tool::Annot(AnnotationKind::Polyline | AnnotationKind::Polygon | AnnotationKind::Cloud) => "Click points · double-click to finish",
			Tool::Annot(AnnotationKind::Callout) => "Drag the text box, then click where the leader points",
			Tool::Annot(AnnotationKind::Ink) => "Draw freehand",
			Tool::Annot(_) => "Drag to place",
		}
	}

	/// Which kind of takeoff item this tool records into (None = annotation/utility).
	pub fn category_kind(self) -> Option<CategoryKind> {
		match self {
			Tool::Count => Some(CategoryKind::Count),
			Tool::Length | Tool::Curve | Tool::Trace | Tool::Perimeter | Tool::FollowLine => Some(CategoryKind::Length),
			Tool::Area | Tool::Fill | Tool::RectArea | Tool::Volume => Some(CategoryKind::Area),
			_ => None,
		}
	}

	pub fn group(self) -> Group {
		match self {
			Tool::Select | Tool::Pan => Group::Navigate,
			Tool::Annot(_) | Tool::CurveMarkup | Tool::Legend => Group::Markup,
			_ => Group::Measure,
		}
	}

	/// Click-by-click polyline tools.
	pub fn is_multi_point(self) -> bool {
		matches!(
			self,
			Tool::Length | Tool::Curve | Tool::Trace | Tool::Perimeter | Tool::Area | Tool::Volume | Tool::Cutout | Tool::Angle | Tool::Calibrate | Tool::CurveMarkup | Tool::Space
				| Tool::Annot(AnnotationKind::Polyline | AnnotationKind::Polygon | AnnotationKind::Cloud)
		)
	}

	pub fn snaps(self) -> bool {
		matches!(
			self,
			Tool::Length | Tool::Curve | Tool::Trace | Tool::CurveMarkup | Tool::Perimeter | Tool::Area | Tool::Volume | Tool::Cutout | Tool::Angle | Tool::Calibrate | Tool::RectArea | Tool::Viewport | Tool::Count | Tool::Space
				| Tool::Annot(AnnotationKind::Line | AnnotationKind::Arrow | AnnotationKind::Polyline | AnnotationKind::Polygon)
		)
	}

	pub fn all() -> impl Iterator<Item = Tool> {
		[Tool::Select, Tool::Pan].into_iter().chain(Tool::MEASURE).chain(Tool::MARKUP).chain(Tool::ORGANIZE)
	}
}
