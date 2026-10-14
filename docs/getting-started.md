# Getting started

## Install

**macOS (Apple silicon):** download `Takeoff-Desk-<version>-arm64.dmg`, drag *Takeoff Desk* to
Applications. Everything it needs is inside the app: the PDF engine (pdfium), llama.cpp for local
models, and the `takeoff` command-line tool (`Takeoff Desk.app/Contents/Resources/bin/takeoff`).

**From source** (any platform with Rust 1.85+):

```sh
git clone https://github.com/construction-live/takeoff-desk && cd takeoff-desk
scripts/fetch-pdfium.sh
cargo run --release -p takeoff-app            # the app
cargo install --path crates/takeoff-cli       # the `takeoff` CLI
```

For local AI on a source build, install llama.cpp (`brew install llama.cpp`) or point
Preferences → llama-server at a `llama-server` binary.

## Open a drawing

*File → Open* (⌘O), drop PDFs on the window, or pick a recent file on the start screen. Each
drawing opens in a tab. Your takeoff is saved automatically, about a second after each change, to
`<drawing>.pdf.takeoff.json` next to the PDF. The status bar shows **Saved**.

![Start screen](images/dark-start.png)

## The window

```
┌ File Edit Document View Help │ tabs ───────────────────────────────────────────┐
│ ↶ ↷ │ Select Pan │ Measure: Count Length Perimeter Area … │ Markup: Text … │ ⌁ # │ AI │
├──────────┬──────────────────────────────────────────────────┬──────────────────┤
│ Pages    │                                                  │ Takeoff          │
│ Bookmarks│                drawing canvas                    │ Properties       │
│ Layers   │                                                  │ AI               │
│ Search   ├──────────────────────────────────────────────────┤                  │
│          │ Markups list (sortable, filterable, CSV)         │                  │
├──────────┴──────────────────────────────────────────────────┴──────────────────┤
│ ‹ 3 / 52 › sheet │ − 100% + ⤢ │ scale 1/4" = 1'-0" · x 12'-6" y 30'-1" │ Saved │
└──────────────────────────────────────────────────────────────────────────────┘
```

- **Navigate:** scroll or pinch to zoom (*Preferences → Mouse wheel* chooses whether the wheel
  zooms or scrolls), hold **Space** to pan with any tool, **PgUp/PgDn** for pages, **⌘0** to
  fit the page.
- **Tools:** each has a one-letter shortcut. Press **?** for the full list.
- **Light or dark:** *View → Theme* (it follows the system by default).

Next: [Taking off](takeoff.md).
