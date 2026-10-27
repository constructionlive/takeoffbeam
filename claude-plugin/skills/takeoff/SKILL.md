---
name: takeoff
description: Quantity takeoff and drawing review on a PDF drawing set with Takeoff Beam's MCP tools. Use when the user asks to count symbols, measure lengths or areas, set a scale, read a title block, schedule or note, find text across sheets, mark up a drawing, or report quantities from a PDF plan. Triggers on "takeoff", "count the receptacles", "measure the wall", "how many fixtures", "set the scale", "what does this note say", "mark up", "cloud this", "quantities", and any PDF drawing, plan, sheet or blueprint.
argument-hint: <path/to/plans.pdf> <what to count, measure or read>
---

# Takeoff with Takeoff Beam

You have Takeoff Beam's drawing tools (the `takeoff` MCP server). They work on ONE open PDF, and
every change is saved next to it as `<file>.takeoff.json`, which Takeoff Beam and construction.live
both open. The user reviews your marks in the app: everything you add is tagged "AI" and
"to review" there, so be accurate rather than exhaustive.

## The loop

1. **Open.** `open_drawing {path}` with an absolute path. Then `get_drawing` for pages, labels,
   paper sizes, whether each page has a scale, existing items, and PDF layers.
2. **Scale first, for anything measured.** Counts do not need a scale; lengths, areas and
   volumes do. In order of preference:
   - `calibrate` on the page: it finds printed dimensions, pins their endpoints, sets the scale
     and cross-checks. Read its report.
   - `set_scale_preset` when the title block states the printed scale (e.g. 1/4" = 1'-0") and
     the paper size is real, not a reduced print.
   - `set_scale` from two points you read off a view and a known distance.
   - Always `check_scale` against a second printed dimension before measuring. Within about 1%
     is fine; otherwise fix the scale, do not measure.
3. **Look before you mark.** `view` renders part of a page with the current takeoff drawn on it.
   A whole sheet is unreadable at full-page scale: zoom with `region` (page fractions) and then
   `zoom_view` + `zoom_box` to get close. Every `view` returns a view id and a caption stating
   its coordinate convention. **Every location you hand to any tool is read off a view and sent
   with that view's id.** Never convert coordinates yourself, never reuse a box from a different
   view.
4. **Count.** For symbols: `create_category` (or reuse one from `get_drawing`), then either
   - `detect` with a description or a legend symbol location, which sweeps a page or region and
     adds one mark per instance, or
   - `add_marks` with boxes you read off a view, tile by tile, for symbols a sweep gets wrong.
   Sweep the sheet in overlapping tiles; a single wide view misses small symbols.
5. **Measure.** `add_measurements` with points read off a view for lengths, perimeters and
   areas (with cutouts and an optional depth for volume). For runs that follow drawn lines, use
   `follow_line` from a point on the line, or `trace` to find all runs or areas matching a
   description across a page.
6. **Read.** `read_text` for the PDF's own text (title blocks, schedules, notes, tags) with
   positions. `search_text` to find sheet numbers, tags and spec references across pages.
   Prefer text over vision when the PDF has a text layer: it is exact.
7. **Correct by location, not by number.** Before `update_items` or `delete_items`, call
   `find_items_at` with the box or point so you change the item that is actually there.
8. **Report from `summary`.** It gives totals per item and per page, pending versus confirmed,
   and pages with no scale. Quote those numbers, not your own tally.

## Habits that keep the takeoff right

- Hide clutter with `hidden_layers` on a single `view` (that call only) rather than
  `set_hidden_layers`, which changes what the user sees too.
- `set_viewport` as you move around, so the user watching in Takeoff Beam sees where you are.
- Mark what you are unsure of with `add_annotation` (a cloud or callout with a short note)
  instead of guessing a quantity.
- One drawing per server. To work a second PDF, call `open_drawing` again.
- Do not claim a count you did not sweep for. Say which pages and regions you covered.

## Reporting

End with the `summary` numbers, the scale used per page and how it was checked, which pages
and regions were swept, and anything left as a review annotation. Tell the user to open the PDF
in Takeoff Beam to confirm the AI marks (Edit → Confirm all AI marks, or one at a time).

## When invoked as a command

`/takeoff-beam:takeoff <plans.pdf> <ask>`: the first argument is the PDF path (make it absolute),
the rest is the ask. If the path is missing, ask for it before calling any tool. If the server
is not running, Takeoff Beam is probably not installed: point the user to
https://www.construction.live/open.

Ask: $ARGUMENTS
