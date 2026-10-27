# Architecture

```
takeoff-core     model, geometry, numbering, page remap, snap engine, coordinate dialects,
                 document store (undo/redo), markups list + CSV/Excel, sidecar I/O, the document
                 map (sheets, areas, links), sheet numbers + slip-sheet matching and
                 carry-over, set files + project summary                             — no I/O deps
takeoff-pdf      pdfium: render page/region, text + search, bookmarks, vectors; own lazy PDF
                 object reader for layers; page tools + takeoff-aware ops; slip-sheets and
                 revision compare; burn-in; flattened export
takeoff-models   GGUF header reader, memory-fit estimate, model catalog/store/downloads,
                 llama-server supervisor
takeoff-cloud    construction.live device-code sign-in, keychain token, models, chat base URL
takeoff-agent    OpenAI-compatible streaming client, the agent loop (with parallel subagents),
                 drawing tools, the document-map tools, vision pipelines, evals + vision probe
takeoff-app      egui desktop app
takeoff-cli      `takeoff` CLI and the MCP server
```

**Opening files.** `main.rs` opens PDFs given as arguments (Windows "Open with", the command
line). On macOS Finder sends Apple Events instead, which winit drops, so the app builds its own
event loop and adds `application:openURLs:` to winit's app delegate before running it
(`open_files.rs`); the paths queue up and the next frame opens them like dropped files.

**Threads in the app.**
- The UI thread owns the egui state.
- A render worker thread owns the open `Drawing`, because pdfium is not re-entrant. The UI sends
  it requests (base page texture, sharp view region, thumbnails, vectors, search, layers) and
  drains results every frame. A newer view request supersedes a stale one.
- Each agent turn runs on its own thread with its own `Drawing`.
- The takeoff `Document` is shared behind a mutex. Every change goes through `Document::edit`,
  so the user and the agent get the same undo history, dirty tracking and recomputed quantities.

**Undo across PDF edits.** Page tools and slip-sheets rewrite the PDF. Each such undo step
carries a session-only `pdf_state`, and the app keeps the file of every state (a hard link in the
temp folder; the tools always rename a new file over the old one). When undo or redo lands on
another state, the matching file is put back and the drawing reloads with its undo history intact.

**Rendering.**
- Each page gets a base texture (about 2400 px on the long edge) for instant paging.
- When the view settles (140 ms), the worker renders the visible region at screen resolution:
  region renders are pixel-exact crops from pdfium (`set_origin` into a region-sized bitmap), so
  zooming a 36×24 sheet never rasterizes the whole sheet at 400%.
- Continuous view and spreads lay every page out from the page sizes alone (`layout.rs`). Base
  textures go only to pages on screen and a screen beyond, three at a time, nearest first, sized
  to the page on screen; far ones are dropped. Each visible page gets its sharp region, sent as
  one batch that supersedes the last.
- Overlays (marks, measurements, markups, snap markers, the agent's viewport) are drawn
  with egui in screen space from page fractions. Concave fills use earcut; hatches are
  scanline-clipped.

**Layers.**
- pdfium renders optional content from the document's default configuration but has no API to
  change it.
- Takeoff Desk parses the catalog lazily from a memory map (xref tables and streams, object
  streams, PNG predictors) and writes a small incremental update. The update replaces
  `/OCProperties /D` with the wanted `/OFF` list.
- pdfium loads "original bytes + update" from memory. The file is never modified, and switching
  layers on an 859-page set takes under a millisecond.

**The agent loop.**
- Streaming `chat/completions` with typed tools, continuing while `finish_reason == tool_calls`.
- Tool images are not put in tool messages. Each render is injected as a captioned user image
  after the tool run; only the last 6 stay live, and older ones become "run view again" notes.
- Compaction kicks in at 70% of the context window.
- Every coordinate a model reads comes from a registered view, and code converts it back to page
  fractions.

**Snapping.**
- The drawing's stroked paths are flattened into segments in page fractions (curves included).
  Fill-only paths such as hatches and outlined glyphs are skipped.
- The segments go into a uniform grid index. Endpoints beat midpoints, which beat intersections,
  which beat on-line points.
- The same engine serves the cursor, the agent's `snap: true` and Follow Line.
