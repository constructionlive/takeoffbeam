# Graph view: the document map

A drawing set is a web of references. A callout on a plan names a detail on another sheet, a
section cut names the sheet that draws it, a general note governs work drawn everywhere, the same
wall shows up in plan, elevation and detail. **Graph view** shows that web: every sheet, the areas
on it that matter, and how they relate, as a graph of pictures cut from the sheets.

Switch with **Page | Graph** at the right of the toolbar, *View → Graph view*, or **⌘G**.

## Mapping a set

The AI builds the map the way it does a takeoff: by looking. It runs in the AI chat, on whatever
model the chat uses: a construction.live cloud model, a model on this computer, your Claude Code,
Codex or OpenCode account, or a custom endpoint.

1. In Graph view, type any **instructions** you have ("only the architectural and structural
   sheets", "sheet numbers are bottom-left on this set", "pay attention to the wall types").
2. Press **Start mapping**. A new conversation opens in the AI tab with the request.
3. Watch it work there, add instructions as it goes, or press Stop. Sheets and areas appear in
   the graph as it records them.

What the AI does:
- **Survey.** It skims the whole set in contact sheets (24 pages to a picture) with each page's
  title-block text, and writes the sheet index: number, title, discipline and a line on each.
- **Divide.** On a big set it splits the sheets into batches that belong together and hands each
  to a **subagent**. Subagents run in parallel, each with a fresh context and the same drawing
  tools. Claude Code uses its own subagents; the app's own agent (cloud, local, custom models)
  uses `delegate`, up to 6 at once.
- **Map each sheet closely.** It views the sheet, zooms where the content is, and records:
  - each titled drawing (plan, detail, section, elevation, schedule, legend, key plan, diagram)
    and the notes blocks, with boxes;
  - the callouts that point elsewhere (detail bubbles, section cuts, elevation markers,
    enlarged-plan boxes, matchlines, "SEE …" notes), inside the drawing they sit on;
  - notes and elements that govern something drawn elsewhere.

  Each link goes from the precise source to the precise target, with a sentence on how they
  relate: "the north stair's handrail is drawn in detail 3", "note 4 applies to every
  partition on this floor".
- **Join up.** It resolves links that still point at a whole sheet, writes the overview, and tells
  you what the map holds and what it is unsure of.

Every step is saved at once, so a stopped run loses nothing. *Map → Continue mapping…* starts a
new chat that picks up the sheets not mapped yet (and anything else you ask). *Map → Map again
from scratch…* starts over.

Mapping a large set takes a while and uses your model's credits or plan limits like any chat:
the AI looks at every sheet up close. Sheets only indexed in the survey show with a faded colour
band until they are mapped.

## Moving around the set

The map works in both views, through the **Map** tab in the right panel (next to Takeoff,
Properties and AI).

- **The Map tab** shows where you are: in Page view, the sheet on screen, or the area you
  clicked on it; in Graph view, the selected card. It has a picture of it, the AI's note, what is
  on it or inside it, what it **points to** and what it is **referenced from**, each with the AI's
  note on how. Click any entry to go there.
- **Links on the sheet.** In Page view the callouts the AI mapped are lightly highlighted. Hover
  one to see where it leads; click it to go there: the sheet opens zoomed to the detail, which
  flashes so you see what you arrived at. Click any other mapped area (a plan, a notes block, a
  schedule) to show it in the Map tab. The link button in the toolbar turns this off.
- **Back and Forward** (the arrows in the Map tab, **⌥←** / **⌥→**, or **⌘[** / **⌘]**) step
  along where you have been and put the sheet back exactly as you left it. The breadcrumb next to
  them shows the trail; click a step to return to it.
- **Disciplines.** The Pages list is grouped by discipline, each under a header you can fold,
  and shows each page's sheet number and title. The **disciplines** filter at the top of the
  Pages panel (the same one is in the graph's bar) shows some disciplines only: disciplines the
  AI filed as "Civil/Track" and "Civil/Yard" sit under **Civil**, so you can tick the whole
  group or one part, or hover a row and press **Only**. The button says what is showing; × shows
  everything again.

## Reading the graph

- **Board or Web.** *Board* (the default) arranges the set: a lane per discipline in the order
  the set uses them, its sheets in sheet order, each with its main areas stacked beneath it
  (drawings, then details and sections, schedules, notes, callouts; "+N more" when a sheet has
  more, all of them in the Map tab). Links show for the card you hover or select. *Web* lays
  everything out by how it links, with all the links showing.
- **Cards.** Sheets carry their thumbnail, sheet number and title, with a colour band for the
  discipline. The areas recorded on a sheet sit around it
  as pictures cut from it; callouts hang from the drawing they sit on. **Show** chooses which
  kinds of area show: drawings, details and sections, schedules and legends, notes and elements,
  callouts.
- **Arrows** follow the links, coloured by kind: detail, section, elevation, enlarged plan,
  matchline, schedule, note / spec, same element, reference. The legend is at the bottom left.
- **Hover** a card to bring forward what it connects to. **Click** it to show it in the Map tab.
  **Double-click** it, or press *Open on sheet* there, to open it in Page view, zoomed to it.
- **Focus** shows only what is linked to the selected card: press once for one step away, again
  for two, again for everything.
- **Search** (⌘F in Graph view) finds sheets, areas and links by number, title or note. Enter
  selects the first match.
- Drag the background to pan, pinch or scroll to zoom, drag a card to move it. The fit button
  shows the whole map again.

## The AI uses it

When a map exists, the AI is told so with every message. `document_map` with no arguments gives
the overview, the sheet index and which pages are not mapped yet. Given a page or node id, it
gives that sheet's areas with their regions and the links both ways with their notes. Given a
query, it searches. So "check the stair detail referenced on the first floor plan" goes straight
to the right detail, instead of paging through the set. The same tools are served to Claude Code,
Codex and OpenCode, and over the [MCP server](cli.md#mcp).

## File

The map lives next to the drawing: `plan.pdf` → `plan.pdf.map.json`. It holds regions, not
pictures, and is kept apart from the takeoff file, which construction.live shares. See
[File format](file-format.md#document-map). When the PDF's page count changes (page tools), Graph
view says the map is out of date; map it again.
