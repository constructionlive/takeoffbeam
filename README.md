# Takeoff Desk

**Bluebeam-style quantity takeoff with a built-in AI agent.** Open a drawing set, then count,
measure and mark it up. Ask an agent to do the same on the sheet in front of you: it looks, sets
the scale, counts symbols, measures runs, reads title blocks, and checks its own work. It runs on
a local model on your machine, or on cloud models if you sign in to
[construction.live](https://construction.live).

![Takeoff Desk](docs/images/dark-takeoff.png)

- **Takeoff:**
  - Counts, lengths, perimeters, areas with cutouts, volumes and angles.
  - Scale presets, calibration and viewports.
  - Snapping to the drawing's own lines, and Follow Line.
  - Items with colors, hatches, unit costs and a Tool Chest.
- **Markups:**
  - Text, callouts, clouds, shapes, arrows, highlight, pen and stamps.
  - A Bluebeam-style Markups List with custom columns and CSV export.
- **PDF tools:**
  - Split, merge, extract, insert, rotate, delete and reorder pages. The takeoff follows its
    pages.
  - Compare revisions, layers, text search and bookmarks.
  - Flattened marked-up PDF export.
- **AI agent:**
  - Drawing tools only: no file system, no shell. You review everything it adds.
  - Local models through llama.cpp, with a memory check before loading. Cloud models through
    construction.live. Or any OpenAI-compatible endpoint.
- **For other tools:**
  - The `takeoff` CLI.
  - An MCP server, so Claude Desktop or Cursor can work a drawing.
- **Native and small:**
  - Rust, egui and pdfium. A 46 MB macOS download.
  - An 859-page set opens in 21 ms, and a full sheet renders in about 50 ms.
- **Plain files:** the takeoff is `plan.pdf.takeoff.json` next to the PDF, compatible with
  construction.live.

## Get it

- **macOS (Apple silicon):** the `.dmg` from Releases, signed and notarized by Neuratwin Inc.
- **Windows 10/11 (x64):** `Takeoff-Desk-<version>-x64-setup.exe` from Releases.
- **From source:** `scripts/fetch-pdfium.sh && cargo run --release -p takeoff-app`.

See the [docs](docs/README.md):
- [Getting started](docs/getting-started.md)
- [Taking off](docs/takeoff.md)
- [PDF tools](docs/pdf-tools.md)
- [The AI agent](docs/ai.md)
- [CLI & MCP](docs/cli.md)
- [File format](docs/file-format.md)

## Crates

| Crate | |
|---|---|
| `takeoff-core` | Data model, geometry, numbering, snapping, coordinate dialects, undo, CSV, sidecar |
| `takeoff-pdf` | pdfium rendering, text, layers, vectors, page tools, burn-in, flattened export |
| `takeoff-models` | GGUF reader, memory-fit check, model catalog, downloads, llama-server |
| `takeoff-cloud` | construction.live sign-in and cloud models |
| `takeoff-agent` | The agent loop and drawing tools |
| `takeoff-app` | The desktop app |
| `takeoff-cli` | `takeoff` CLI + MCP server |

## License

Apache License 2.0; see [LICENSE](LICENSE). If you redistribute Takeoff Desk or something built
from it, keep the [NOTICE](NOTICE) file with it (License §4(d)) so construction.live is credited.

Third-party components:
- pdfium (BSD-3)
- Inter (SIL OFL 1.1), in `assets/fonts`
- Lucide icons (ISC), in `assets/fonts`
- llama.cpp (MIT), bundled in release builds
