# Contributing

Thanks for helping. A few house rules:

- **Style.** Tabs, `cargo fmt` defaults otherwise. Keep comments for the *why*.
- **Coordinates are page fractions.** Anything that touches a position goes through
  `takeoff_core` types. Never store pixels.
- **One edit path.** Change a takeoff only through `Document::edit`, so undo, autosave and the
  agent all stay correct.
- **The agent only gets drawing tools.** New agent capabilities are new tools in
  `takeoff-agent/src/tools`, with typed JSON schemas.
- **Tests.** `cargo test --workspace`. UI changes: run the snapshot test and look at the images
  (see [building](docs/building.md)).
- **Licensing.** Contributions are Apache-2.0. Don't add AGPL/GPL dependencies; that's why we use
  pdfium, not MuPDF.
