# Roadmap

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

### Still to do from slice 0
- **Vision accuracy probe.** Measure how well Qwen3.8-27B and Gemma 4 place boxes on real
  sheets, compared with Gemini, for each coordinate convention. Qwen3-VL answers on a 0–1000
  grid in `[x,y]` order, which needs a third dialect. This decides whether local models run
  full takeoff or assist, with the cloud doing detection. Needs the `view` tool (slice 4).

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
