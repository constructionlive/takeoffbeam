# Takeoff Desk

**Bluebeam-style quantity takeoff with a built-in AI agent.** Open a drawing set, count symbols,
measure lengths and areas, and let an agent do the same on the sheet in front of you: look at it,
mark it, read it, check its own work. It runs on a local model, or on cloud models if you sign
in to [construction.live](https://construction.live).

> Status: early development (slice 0). Not usable yet. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Why
- **Native and fast.** Written in Rust, with pdfium for rendering. On an 859-page civil set, a
  full 17×11 sheet renders at 200 DPI in about 50 ms.
- **Private by default.** Drawings never leave your machine when you use a local model.
- **The agent only touches the loaded drawing.** It has no file system, shell or internet
  access. Its tools are the same ones you use: view, count, mark, measure, calibrate, read.
- **Your takeoff is a plain file.** It lives next to the PDF as `plan.pdf.takeoff.json`, with
  positions stored as fractions of the page. The same format imports into construction.live.

## Crates
| Crate | What it does |
|---|---|
| `takeoff-core` | Data model, measurement maths, badge numbering. No I/O. |
| `takeoff-pdf` | Open PDFs, render a page or a region at any DPI, read text (pdfium). |
| `takeoff-models` | Read GGUF model headers and check whether a model fits this machine before loading it. |

Planned: `takeoff-agent` (agent loop + drawing tools), `takeoff-mcp` (the same tools as an MCP
server), `takeoff-cli`, and the desktop app (native UI).

## Develop
```sh
scripts/fetch-pdfium.sh          # downloads libpdfium into vendor/pdfium
cargo test --workspace
cargo run --release -p takeoff-pdf --example spike -- plans.pdf 3 /tmp
cargo run --release -p takeoff-models --example fit -- model.gguf mmproj.gguf 32768
```

## License
Apache License 2.0, see [LICENSE](LICENSE). If you redistribute Takeoff Desk or something built
from it, keep the [NOTICE](NOTICE) file with it (License §4(d)) so construction.live is credited.
