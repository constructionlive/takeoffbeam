# Takeoff Beam for Claude Code

Do a quantity takeoff from Claude Code. The plugin connects Claude to Takeoff Beam's MCP server,
which runs on your machine and works on one PDF at a time: count symbols, measure lengths and
areas, set and check the scale, read title blocks, schedules and notes, and leave review markups.
The takeoff is saved as `<file>.takeoff.json` next to the PDF, and you confirm the AI's marks in
Takeoff Beam.

## Install

1. Install Takeoff Beam (free, Mac and Windows): https://www.construction.live/open.
   The `takeoff` CLI ships inside the app. Or build it: `cargo install --path crates/takeoff-cli`.
2. In Claude Code:
   ```
   /plugin marketplace add constructionlive/takeoffbeam
   /plugin install takeoff-beam@takeoffbeam
   ```
3. Ask:
   ```
   /takeoff-beam:takeoff ~/Plans/E-201.pdf count the duplex receptacles on page 2
   ```
   or just say "open ~/Plans/A-101.pdf and measure the exterior wall". The `takeoff` skill
   loads on its own when a drawing is involved.

## What is in it

- `takeoff` MCP server: the 24 drawing tools Takeoff Beam's own agent uses (`view`, `detect`,
  `add_marks`, `add_measurements`, `calibrate`, `check_scale`, `read_text`, `search_text`,
  `summary`, …). Drawing tools only: no file system, no shell, no web.
- `takeoff` skill: how to run a takeoff that holds up. Scale first and checked against a second
  dimension, zoom before marking, sweep in tiles, correct by location, report from `summary`.

## Finding the binary

`scripts/takeoff-mcp.js` looks for `takeoff` in the app bundle (`/Applications/Takeoff Beam.app`
or `Takeoff Desk.app`, also under `~/Applications`), in the Windows install directory from the
registry or Program Files, in `~/.cargo/bin`, then on `PATH`. Set `TAKEOFF_BIN` to point at it
directly.

## Privacy

Everything runs locally. The MCP server reads the PDF on your disk and writes the takeoff next
to it. Which model reads the images is Claude Code's choice, so the views the agent looks at go
to the model you are using in Claude Code; the PDF itself does not.
