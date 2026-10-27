# Roadmap

**Status 2026-10-14:** slices 0–8 built. See *Done* at the bottom; open items are under *Next*.

Decided 2026-10-14:
- Fully native UI (egui).
- Apache-2.0 with a NOTICE that requires credit.
- Separate public repo.
- pdfium for PDFs, not MuPDF (AGPL).
- Local models through llama.cpp's `llama-server`, with a memory-fit check before loading.

## Slice 0: spikes (done)

### Rendering: pdfium via pdfium-render 0.9.4, pinned to pdfium chromium/7881

| Set | Open | All page sizes | Full sheet @200 DPI | Region @200 | Text |
|---|---|---|---|---|---|
| Franconia, 859 pp, 708 MB, 17×11 | 21 ms | 23 ms | 47–58 ms (3400×2200) | 13–16 ms | title block OK |
| Plans.pdf, 60 pp, 36×24 | 4 ms | 9 ms | 223 ms (7200×4800) | 131 ms | 11k chars |
| Drawing A, 52 pp, 36×24 | 4 ms | 4 ms | 114 ms | 14 ms | title block + legend OK |

- **Region renders are pixel-exact crops.** They use `set_origin` with a region-sized bitmap,
  so the agent's zoomed views never rasterize the whole sheet.
- **Read page sizes with `FPDF_GetPageSizeByIndexF`.** Loading every page took 7.8 s and 1 GB
  on 859 pages. The size lookup takes 23 ms.
- **Thumbnails are slow.** They cost 28–115 ms per page, because pdfium parses the whole
  content stream. They need a disk cache and a background worker.
- **Text is a bonus, not a given.** Many CAD sheets draw text as vector strokes (SHX fonts), so
  `read_text` sometimes returns little and vision stays the main way to read.
- **Layers (OCG) need work.** pdfium's public API has no optional-content control. Plan: read
  the OCG names with `lopdf`, and hide layers by appending an incremental update that sets
  `/OCProperties /D /OFF` (only a few KB, even on a 700 MB file), then load that into pdfium.
  This mirrors the platform's pikepdf approach. Still to verify (slice 2).

### Memory fit: estimate vs a real llama-server load (Qwen3.8-27B Q4_K_M + Q8 projector, 32K context)

| | Our estimate | llama.cpp actual |
|---|---|---|
| Weights | 18.3 GB + 0.6 GB vision | 18,084 MiB GPU + 682 MiB CPU |
| Context cache | 2,048 MiB | 2,048 MiB |
| Recurrent state | 150 MiB | 149.6 MiB |
| Buffers | ~1.5 GB | ~1.2 GB |

- **Hybrid models are handled.** Qwen3.5+ has only 16 attention layers out of 64, so a naive
  estimate overstates the context cost by 4×.
- **Sliding-window layers are handled** (Gemma).
- **The machine's limit comes from `llama-server --list-devices`.** On this M3 Max with 36 GB,
  Metal allows 28.1 GB. The 2/3–3/4 rule of thumb said 24 GB, which was wrong. The same call
  reports VRAM on CUDA and Vulkan.
- **Verdicts:** Fits, Tight (close other apps), or Not enough memory, plus the largest context
  that fits.

### Vision accuracy probe (built 2026-10-27)

`takeoff eval probe` renders a view the way the agent's `view` tool does, asks the model to box
known targets in each coordinate convention, and scores the boxes. See [Evals](evals.md).
Targets come from the synthetic sheets in `evals/drawings` (exact), or from your own drawings
with the targets counted by hand in Takeoff Desk.

First results, on the two synthetic sheets (18 targets: 9 receptacles, 6 light fixtures, 3 text
labels), M3 Max 36 GB:

| Model | Gemini `[y,x]` 0–1000 | Qwen `[x,y]` 0–1000 | Pixels | Verdict |
|---|---|---|---|---|
| Qwen3.8 27B Q4_K_M (llama.cpp) | 14/18, 0.38% | **18/18, 0.32%** | 0/18 | **full takeoff** (Qwen dialect, as mapped) |
| Gemma 3 4B Q4_K_M (Ollama) | 0/18 | 1/18 | 0/18 | not usable |

Hits / targets, then the median centre error as % of the view's long edge. About 20–60 s per
question for the 27B model, 2–18 s for the 4B one.

- **Qwen3.8 27B can place marks itself** in the dialect it is mapped to, so local full takeoff is
  viable on a machine that runs it. It confirms the earlier 23/23 on a real sheet.
- **Small local models assist at best.** Gemma 3 4B placed nothing usable in any dialect: use it
  for reading, not marking.
- **Whole jobs** (`takeoff eval run`, 900 s a case): Qwen3.8 27B passed 11 of 13 expectations.
  It got the scale within 0.5%, both room areas within 1.1%, all 9 receptacles and every
  title-block field. The two misses (outside wall length, light fixtures) were work it had not
  finished when the timeout hit.
- **Still to measure:** Gemma 4 (26B/12B) and the cloud models (Gemini, Claude, GPT), and real
  sheets with hand-marked targets. Run `takeoff eval probe --model …` and add the rows here.

## Slices
1. **Core.** Port `normalize` and the 3-way merge, plus the platform's coordinate dialects
   (`visionCoords.ts`). Parity fixtures: a script dumps golden JSON from the TypeScript, and
   Rust must match it.
2. **PDF.**
   - Layers (OCG): list them and hide them.
   - Burn marks, measurements and badges into renders with tiny-skia; SVG overlays with resvg.
   - Minimap, a disk cache of pages and thumbnails, and region text.
3. **Viewer v0** (egui, no AI):
   - Pan/zoom with tiled rendering, pages and thumbnails, layers.
   - Count, length, area and calibrate tools.
   - Markups list, undo/redo, sidecar save, CSV export.
4. **Agent v0.**
   - Loop: continue while `finish_reason == tool_calls`, with a step cap and compaction at 70%
     of the context window.
   - Typed tools: view → act → verify render.
   - View ids, so the model never does coordinate maths.
   - Images injected after each tool run, at most 6 live.
   - Chat panel, plus the agent's live viewport.
   - Provider: any OpenAI-compatible endpoint.
5. **Local models.**
   - Run llama-server as a sidecar in router mode.
   - Model catalog, downloads, and the fit check shown before load.
   - A coordinate-dialect table for each model family.
6. **Sign in to construction.live.**
   - The device-code flow already exists on the platform (`/desktop/device/*`).
   - Cloud models through `/desktop/v1/chat/completions`.
   - Token kept in the OS keychain.
   - Platform side: a token scope so takeoff tokens can't call email or SMS.
7. **Vision pipelines.** Detect (tiles + legend crop), calibrate, trace, analyze layers.
8. **Release.**
   - Signed builds for macOS and Windows.
   - Publish the crates to crates.io and `cargo install takeoff-cli`.
   - mdBook docs and the MCP server.
9. **Later.** Snap to vector lines, revision compare, send to construction.live, Linux.

## Done (2026-10-14)

- **Core:**
  - Model with annotations, viewports, cutouts, volume, angle and item metadata.
  - Presets and ft-in formatting; vision dialects including Qwen3.
  - Undo/redo document store; markups list, summary and CSV; sidecar.
  - Parity fixtures against the platform TypeScript; page remap; snap engine.
- **PDF:**
  - Layers through an incremental `/OCProperties` update (own lazy COS reader).
  - Page tools plus takeoff-aware ops.
  - Bookmarks, search, positioned text, vectors.
  - Burn-in, flattened export, revision compare.
- **App (egui):**
  - Every measure and markup tool, with snapping and ortho.
  - Selection, move, reshape, resize, copy/paste, lock, right-click menu.
  - Takeoff, Properties and AI tabs; Pages, Bookmarks, Layers and Search tabs; Markups List.
  - Tabs, start screen, dialogs, shortcuts, light and dark themes.
  - Headless snapshot and simulated-input tests.
- **Models:** catalog, downloads with resume, store and import, llama-server supervisor, fit
  check from `--list-devices`.
- **Cloud:** device-code sign-in (live endpoint verified), keychain, models, chat base URL.
- **Agent:** see `crates/takeoff-agent`. Typed tools, streaming loop, image injection, vision
  pipelines, a live run on local Qwen3.8 27B.
- **CLI + MCP server.**
- **Slip-sheets and sets** (2026-10-27): sheet numbers from title blocks, labels, bookmarks and
  the map; review, carry-over (fitted on other paper), history PDFs, items on changes, undo that
  restores the PDF; set files, the Set tab, the project takeoff.
- **macOS packaging:** a 46 MB dmg with pdfium, llama-server and the CLI; ad-hoc or Developer ID
  signing; optional notarization.

## Next

- **Windows and Linux installers.** CI already builds and tests on Linux.
- **Slip-sheets, more:** match on the AI's title-block reading when a sheet has no text layer;
  carry the document map across; a revision list per sheet (not just the latest) in Compare;
  clean up `plan.pdf.revisions/` files an undone slip-sheet left behind.
- **Sets, more:** reorder and rename drawings in the Set tab; search, the map and the AI across a
  whole set.
- **Spaces, next:** split lengths at space boundaries as an option (today a length goes whole
  to the space holding most of it); spaces in the agent's renders; copying a floor's spaces to
  the other floors.
- **Legends, next:** a per-space scope, and a column for a custom field.
- **"Send to construction.live":** upload the takeoff into a project register.
- **More eval cases from real sheets** (kept out of the repo, through `$TAKEOFF_EVAL_DRAWINGS`),
  and a `takeoff eval run` before each release with the report kept as the next baseline.
