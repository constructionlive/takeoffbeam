//! JSON schemas for every tool (OpenAI `tools` format / MCP `inputSchema`).

use serde_json::{json, Value};

use super::ToolDef;

const BOX: &str = "4 numbers exactly as you read them off that view's image, in the convention its caption states (never page fractions, never converted)";
const POINT: &str = "2 numbers exactly as read off that view's image, in its caption's convention";

fn region() -> Value {
	json!({"type": "object", "description": "Part of the page as fractions 0-1 of the full page (x, y = top-left).", "properties": {
		"x": {"type": "number"}, "y": {"type": "number"}, "w": {"type": "number"}, "h": {"type": "number"}}, "required": ["x", "y", "w", "h"]})
}
fn bx() -> Value {
	json!({"type": "array", "items": {"type": "number"}, "minItems": 4, "maxItems": 4, "description": BOX})
}
fn pt() -> Value {
	json!({"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": POINT})
}
fn view_id() -> Value {
	json!({"type": "string", "description": "View id (e.g. \"v3\") of the render the locations were read from."})
}
fn layers() -> Value {
	json!({"type": "array", "items": {"type": "string"}, "description": "PDF layer names to hide for this call only."})
}

fn def(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolDef {
	ToolDef {
		name: name.into(),
		description: description.into(),
		parameters: json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false}),
	}
}

pub fn definitions() -> Vec<ToolDef> {
	vec![
		def("get_drawing", "Overview of the active drawing: pages (labels, paper sizes, scale set or not, item counts), takeoff items (categories) with counts, PDF layers and which are hidden, overlays, and the other open drawings.", json!({}), &[]),
		def("document_map", "The drawing set's map: what each sheet is (number, title, discipline, summary), the areas on it that matter (plans, details, sections, schedules, notes, callouts, elements, with page-fraction regions) and how they relate (a callout to the detail it names, a note to what it governs, the same element in plan and elevation), each link with a note on how. No arguments: the overview, the sheet index and which pages are not mapped yet. page or node (an id like s7 or s7.2): that sheet or area with its links both ways. query: search titles, notes and link notes.", json!({
			"page": {"type": "integer", "description": "1-based page."},
			"node": {"type": "string", "description": "Node id from an earlier answer, e.g. \"s7\" or \"s7.2\"."},
			"query": {"type": "string", "description": "Words to find, e.g. \"stair detail\" or \"door schedule\"."}}), &[]),
		def("contact_sheet", "Skim up to 24 pages in ONE picture (small tiles, each labelled with its page) plus the text the PDF has in each title block. For surveying a set before mapping it: which pages are plans, details, schedules, notes. Too small for detail or locations; view a page to look closely.", json!({
			"pages": {"type": "string", "description": "e.g. \"1-24\" or \"25-48\" (default: the first 24)."}}), &[]),
		def("map_sheets", "Write the map's sheet index (and the set's overview): per page its sheet number, title, discipline and a one-line summary of what is on it. mapped: true once the sheet has been looked at closely and its areas and links recorded. Only the fields you give change.", json!({
			"overview": {"type": "string", "description": "3-5 sentences on the whole set: the project, the disciplines and sheet groups, where the plans, details and schedules are."},
			"sheets": {"type": "array", "items": {"type": "object", "properties": {
				"page": {"type": "integer"}, "number": {"type": "string", "description": "As printed, e.g. A-501."}, "title": {"type": "string"},
				"discipline": {"type": "string", "description": "Architectural, Structural, Mechanical, Electrical, Plumbing, Civil, Fire, Landscape, General…"},
				"summary": {"type": "string"}, "mapped": {"type": "boolean"}}, "required": ["page"]}}}), &[]),
		def(
			"map_add",
			"Record what you found into the map: areas (each a box read off a view, like every location) and links between them. An area is anything worth finding again: a titled drawing (plan, detail, section, elevation, schedule, legend, key plan, diagram), a notes block or one note, a callout bubble / section cut / tag / matchline, or an element (a room, a wall run, a piece of equipment). Links say how two things relate, from the precise source to the precise target: the callout to the detail it names, a note to what it governs, a tag to its schedule, the same element in two views. Refer to nodes by id (s7 = a sheet, s7.2 = an area), by a key given to an area in this same call, or by sheet number (\"A-501\": its sheet). To change an area give its id (and view + box to move it). mapped_pages marks sheets done. Saves at once and attaches a check render of the new boxes.",
			json!({
				"areas": {"type": "array", "items": {"type": "object", "properties": {
					"key": {"type": "string", "description": "Name to link it by in this call, e.g. \"a1\"."},
					"id": {"type": "string", "description": "Existing area to change instead of adding."},
					"view": view_id(), "box": bx(),
					"kind": {"type": "string", "enum": ["plan", "detail", "section", "elevation", "schedule", "legend", "notes", "callout", "element", "key_plan", "diagram", "title_block", "other"]},
					"label": {"type": "string", "description": "Its printed number or tag: \"3\", \"A\", \"3/A-501\", \"D-12\"."},
					"title": {"type": "string"},
					"note": {"type": "string", "description": "What it is and what matters about it for an estimator."},
					"parent": {"type": "string", "description": "The area it sits in (id or key): a callout inside a plan, a note inside a notes block."}}}},
				"links": {"type": "array", "items": {"type": "object", "properties": {
					"from": {"type": "string"}, "to": {"type": "string"},
					"outside": {"type": "string", "description": "Instead of to: the target is not in this PDF (\"S-201, structural set\")."},
					"kind": {"type": "string", "enum": ["detail", "section", "elevation", "enlarged", "continues", "schedule", "governs", "same", "reference"]},
					"text": {"type": "string", "description": "The callout or note text as printed."},
					"note": {"type": "string", "description": "How they relate, in a sentence."}}, "required": ["from"]}},
				"mapped_pages": {"type": "array", "items": {"type": "integer"}}
			}),
			&[],
		),
		def("map_remove", "Remove areas or sheets from the map by id (an area takes the areas inside it and every link touching them), or links by from + to.", json!({
			"ids": {"type": "array", "items": {"type": "string"}},
			"links": {"type": "array", "items": {"type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string"}}, "required": ["from", "to"]}}}), &[]),
		def("switch_drawing", "Point every tool at another open drawing (the estimator's other tabs): by file name or number from get_drawing. Each drawing has its own pages, scales and takeoff. Locations read off a view always go to the drawing that view came from, so you only need this before page-based calls (view by page, read_text, search_text, summary, detect…). Returns that drawing's overview.", json!({
			"drawing": {"type": "string", "description": "File name (or part of it) or number."}}), &["drawing"]),
		def(
			"view",
			"Render part of a page with the takeoff drawn on it (numbered badges per item) and look at it. Returns the image with a view id; every location you hand back to other tools is read off a view and passed with its id. Whole sheets are unreadable at full-page scale: zoom with region (page fractions, coarse) or zoom_view + zoom_box (a box read off an earlier view, precise).",
			json!({
				"page": {"type": "integer", "description": "1-based page (default: the page the user is on)."},
				"region": region(),
				"zoom_view": view_id(),
				"zoom_box": bx(),
				"categories": {"type": "array", "items": {"type": "string"}, "description": "Only draw these items (ids or names)."},
				"show_marks": {"type": "boolean"},
				"show_measurements": {"type": "boolean"},
				"show_annotations": {"type": "boolean"},
				"hidden_layers": layers(),
				"max_px": {"type": "integer", "description": "Long edge of the render (default 2000, capped for the model)."}
			}),
			&[],
		),
		def("read_items", "List takeoff items (200 per call) with ids, badges, categories, status, values, and the space each is in (its full path, e.g. \"Level 2 › Unit 204 › Kitchen\").", json!({
			"page": {"type": "integer"}, "category": {"type": "string"},
			"kind": {"type": "string", "enum": ["mark", "measurement", "annotation"]},
			"space": {"type": "string", "description": "Space name or id: only items in it, or in spaces inside it."}, "offset": {"type": "integer"}}), &[]),
		def("find_items_at", "Which items are at a location (box or point read off a view). Use it before correcting or deleting: correct by LOCATION, never by badge number.", json!({
			"view": view_id(), "box": bx(), "point": pt()}), &["view"]),
		def(
			"add_marks",
			"Mark counted symbols: one box per instance, read off a view. Saved as PENDING for the user to confirm. Duplicates of existing marks in the same category are skipped. Attaches a check render.",
			json!({
				"view": view_id(),
				"boxes": {"type": "array", "items": bx(), "description": "One box per symbol instance."},
				"category": {"type": "string", "description": "Takeoff item id or name (created if new), e.g. \"Duplex outlet\"."},
				"description": {"type": "string", "description": "What the symbol looks like (stored on a new item)."},
				"note": {"type": "string"}
			}),
			&["view", "boxes", "category"],
		),
		def(
			"add_measurements",
			"Measure lengths (polylines), perimeters, areas (polygons) or angles (3 points: arm, vertex, arm) with vertices read off a view. Values come from the page scale. snap: true fits vertices onto the drawing's own lines (vector PDFs). Curves (track, roads, curbs, pipe bends, arcs): path \"follow\" runs along the drawing's own line between each pair of points, through dashes and past crossing lines, so a few points on the line give its exact curve; path \"smooth\" draws a smooth curve through the points (scanned sheets: put a point every few degrees of bend). Default \"straight\".",
			json!({
				"view": view_id(),
				"items": {"type": "array", "items": {"type": "object", "properties": {
					"type": {"type": "string", "enum": ["length", "perimeter", "area", "angle"]},
					"points": {"type": "array", "items": pt()},
					"label": {"type": "string"}, "note": {"type": "string"},
					"depth": {"type": "number", "description": "Area only: depth in the scale unit (gives a volume)."},
					"path": {"type": "string", "enum": ["straight", "follow", "smooth"], "description": "How the points are joined: straight lines; follow the drawn line between them (vector PDFs); or a smooth curve through them."}
				}, "required": ["type", "points"]}},
				"category": {"type": "string"},
				"snap": {"type": "boolean"}
			}),
			&["view", "items"],
		),
		def(
			"fill_area",
			"Measure a space by a point inside it: the outline the drawn lines make around it (a room, a slab, a paved area, a shaft, an opening), exact to the line work, on vector and scanned sheets. One click instead of tracing every corner, so use it for any area bounded by lines. Openings such as doorways are closed for you: the narrowest closing that shuts the space (gap sets it). When lines inside the space split it (a stair drawn in an opening, an X across a shaft, a hatch boundary), give a point in each piece: they fill together as one. Text and symbols inside are part of the space. Saves measurements and attaches a check render: a fill that leaked through an opening shows far bigger than the space.",
			json!({
				"view": view_id(),
				"items": {"type": "array", "items": {"type": "object", "properties": {
					"points": {"type": "array", "items": pt(), "description": "A point inside the space, in open paper; several fill together as one item."},
					"type": {"type": "string", "enum": ["area", "perimeter"]},
					"gap": {"type": "number", "description": "Widest opening to close, in the page's scale unit (e.g. 6 for a 6' opening). Default: found."},
					"label": {"type": "string"}, "note": {"type": "string"},
					"depth": {"type": "number", "description": "Depth in the scale unit (gives a volume)."}
				}, "required": ["points"]}},
				"category": {"type": "string"}
			}),
			&["view", "items"],
		),
		def("update_items", "Change up to 50 items: move/resize (new box or points + the view they were read from; path re-shapes a measurement's points as in add_measurements), recategorise, set status (pending|confirmed — only when the user asks), note, label, text, depth.", json!({
			"items": {"type": "array", "items": {"type": "object", "properties": {
				"id": {"type": "string"}, "view": view_id(), "box": bx(), "points": {"type": "array", "items": pt()},
				"category": {"type": "string"}, "status": {"type": "string", "enum": ["pending", "confirmed"]},
				"note": {"type": "string"}, "label": {"type": "string"}, "text": {"type": "string"}, "depth": {"type": "number"},
				"path": {"type": "string", "enum": ["straight", "follow", "smooth"]}
			}, "required": ["id"]}}}), &["items"]),
		def("delete_items", "Delete items by id (get ids from find_items_at or a view's index). Items the user locked are kept.", json!({
			"ids": {"type": "array", "items": {"type": "string"}}}), &["ids"]),
		def("create_category", "Create a takeoff item (what is being counted or measured).", json!({
			"name": {"type": "string"}, "kind": {"type": "string", "enum": ["count", "length", "area"]},
			"color": {"type": "string", "description": "#RRGGBB"}, "icon": {"type": "string"}, "description": {"type": "string"},
			"legend_view": view_id(), "legend_box": bx()}), &["name"]),
		def("update_category", "Rename or restyle a takeoff item, or store where its legend symbol is.", json!({
			"category": {"type": "string"}, "name": {"type": "string"}, "color": {"type": "string"}, "icon": {"type": "string"},
			"description": {"type": "string"}, "legend_view": view_id(), "legend_box": bx()}), &["category"]),
		def("summary", "Totals per item, per space (when the drawing has spaces: named levels, units, rooms) and per page × item (counts, pending vs confirmed, summed lengths/areas), and pages with no scale. Report numbers from here, never by counting yourself.", json!({
			"page": {"type": "integer"}, "space": {"type": "string", "description": "Space name or id: totals for it and the spaces inside it only."}}), &[]),
		def("add_annotation", "Add a review markup (not a quantity): text box, callout (box + leader tip point), cloud, rectangle, ellipse, line, arrow, polyline, polygon, highlight. Use for questions and flags to the user.", json!({
			"view": view_id(), "kind": {"type": "string", "enum": ["text", "callout", "cloud", "rectangle", "ellipse", "line", "arrow", "polyline", "polygon", "highlight"]},
			"box": bx(), "points": {"type": "array", "items": pt()}, "text": {"type": "string"}, "color": {"type": "string"}}), &["view", "kind"]),
		def(
			"set_scale",
			"Set the page scale from a printed dimension: p1, p2 = the two arrowhead/tick points where the dimension line meets its extension lines (NOT the ends of the text), read off a zoomed view; distance + unit = the printed value. Points snap to line work by default. Then verify with check_scale on a DIFFERENT dimension.",
			json!({"view": view_id(), "p1": pt(), "p2": pt(), "distance": {"type": "number"},
				"unit": {"type": "string", "enum": ["ft", "in", "m", "cm", "mm"]}, "label": {"type": "string"}, "snap": {"type": "boolean"}}),
			&["view", "p1", "p2", "distance", "unit"],
		),
		def("set_scale_preset", "Set a printed scale exactly from the sheet's paper size (e.g. 1/4\" = 1'-0\", 1\" = 20', 1:100). Only valid if the PDF is at full print size — verify with check_scale.", json!({
			"page": {"type": "integer"}, "preset": {"type": "string"}, "all_pages": {"type": "boolean"}}), &["preset"]),
		def("check_scale", "Measure between two points under the current scale without writing anything (measure-back check). Give expected to get the error %; within ~3% the scale holds.", json!({
			"view": view_id(), "p1": pt(), "p2": pt(), "expected": {"type": "number"}, "snap": {"type": "boolean"}}), &["view", "p1", "p2"]),
		def("calibrate", "Automatic scale: finds printed dimensions on the page (text layer, else a vision sweep), pins their endpoints, sets the scale and cross-checks it against a second dimension.", json!({
			"page": {"type": "integer"}, "replace": {"type": "boolean"}, "hidden_layers": layers()}), &[]),
		def(
			"detect",
			"Sweep for one symbol (or up to 8 at once via targets) across a page or region: the area is tiled at working resolution, each tile is read by the vision model, finds are deduplicated and saved as PENDING marks. Re-sweeping is safe (existing marks are skipped). Best with a legend sample (legend_view + legend_box); otherwise describe the symbol. Scope with page + region, or view (+ box).",
			json!({
				"category": {"type": "string"}, "symbol_hint": {"type": "string"},
				"targets": {"type": "array", "items": {"type": "object", "properties": {"category": {"type": "string"}, "hint": {"type": "string"}}, "required": ["category", "hint"]}},
				"legend_view": view_id(), "legend_box": bx(),
				"page": {"type": "integer"}, "region": region(), "view": view_id(), "box": bx(), "hidden_layers": layers()
			}),
			&[],
		),
		def("trace", "Vision trace of linear runs (kind length → polylines) or areas (kind area → polygons) matching a description, across a page or region. Saved as measurements; vertices are approximate — check and adjust.", json!({
			"kind": {"type": "string", "enum": ["length", "area"]}, "target": {"type": "string"}, "category": {"type": "string"},
			"page": {"type": "integer"}, "region": region(), "view": view_id(), "box": bx(), "hidden_layers": layers()}), &["kind", "target"]),
		def("read_text", "Read the PDF's own text (exact, with positions) on a page, a region, or the area of a view (optionally a box in it). Title blocks, schedules, notes, dimension strings. CAD text drawn as strokes has no text layer — read it from a view then.", json!({
			"page": {"type": "integer"}, "region": region(), "view": view_id(), "box": bx(), "limit": {"type": "integer"}}), &[]),
		def("search_text", "Find text across pages (sheet numbers, tags, spec references).", json!({
			"query": {"type": "string"}, "pages": {"type": "string", "description": "e.g. \"1-5, 9\""},
			"match_case": {"type": "boolean"}, "whole_word": {"type": "boolean"}, "limit": {"type": "integer"}}), &["query"]),
		def("set_hidden_layers", "Hide PDF layers for everyone (SHARED: the user sees it too). Give the complete list to hide; [] shows all. For one sweep only, use hidden_layers on view/detect/trace instead.", json!({
			"hidden": {"type": "array", "items": {"type": "string"}}}), &["hidden"]),
		def("follow_line", "From a point on a drawn line, follow the connected line work until a fork or gap (vector PDFs) and report its length; add_to saves it as a length measurement.", json!({
			"view": view_id(), "point": pt(), "add_to": {"type": "string", "description": "Category to save the run in."}, "label": {"type": "string"}}), &["view", "point"]),
		def("set_viewport", "Show the user where you are working (their viewer highlights it).", json!({
			"view": view_id(), "page": {"type": "integer"}, "region": region(), "label": {"type": "string"}}), &[]),
		def(
			"add_overlay",
			"Lay a picture over part of a page, semi-transparent, to SHOW something on the sheet: a map, zones, a heat map, a diagram, a colour-coded correlation, a legend. Give exactly one source: svg (markup; it is stretched to fill the box, so set its viewBox to the box's shape), html (markup, rendered to a transparent image with headless Chrome/Edge), or file (a .svg/.png/.jpg/.html in the workspace). Place it with view + box (read off that view; view alone covers what it shows) or page + region (page fractions; default the whole page). Not a quantity: overlays never change counts or measurements. Attaches a check render.",
			json!({
				"view": view_id(), "box": bx(), "page": {"type": "integer"}, "region": region(),
				"svg": {"type": "string", "description": "SVG markup."},
				"html": {"type": "string", "description": "HTML markup (transparent background unless it sets one)."},
				"file": {"type": "string", "description": "Path in the workspace (relative) or absolute."},
				"opacity": {"type": "number", "description": "0.05-1, default 0.6."},
				"label": {"type": "string", "description": "Short name the estimator sees in the overlay list."},
				"width_px": {"type": "integer", "description": "HTML only: long edge to render at (default 1600)."}
			}),
			&[],
		),
		def("update_overlays", "Change or remove overlays by id (get_drawing lists them): opacity, hidden, label, delete.", json!({
			"items": {"type": "array", "items": {"type": "object", "properties": {
				"id": {"type": "string"}, "opacity": {"type": "number"}, "hidden": {"type": "boolean"}, "label": {"type": "string"}, "delete": {"type": "boolean"}
			}, "required": ["id"]}}}), &["items"]),
		def(
			"bash",
			"Run a shell command (bash; PowerShell on Windows) in your workspace folder, which persists between calls and turns. Use it to compute, convert, script (python, node…), fetch data, and write files you then overlay. Environment: TAKEOFF_PDF (the drawing's path, read-only use), TAKEOFF_WORKSPACE, TAKEOFF_PAGE. Default timeout 120s (max 600). Returns exit code, stdout and stderr (long output keeps its end).",
			json!({"command": {"type": "string"}, "timeout_secs": {"type": "integer"}}),
			&["command"],
		),
	]
}

/// Tools that need the workspace folder (left out when it is off).
pub const SHELL_TOOLS: &[&str] = &["bash"];

/// OpenAI `tools` array; `shell` = the workspace tools are available.
pub fn openai_tools(shell: bool) -> Vec<Value> {
	definitions()
		.into_iter()
		.filter(|d| shell || !SHELL_TOOLS.contains(&d.name.as_str()))
		.map(|d| json!({"type": "function", "function": {"name": d.name, "description": d.description, "parameters": d.parameters}}))
		.collect()
}
