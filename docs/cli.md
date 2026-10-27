# Command line & MCP

`takeoff` ships inside the app (`Takeoff Desk.app/Contents/Resources/bin/takeoff`). Or install it
from source: `cargo install --path crates/takeoff-cli`.

```sh
takeoff info plans.pdf                                   # pages, sizes, labels, layers, takeoff summary
takeoff render plans.pdf --page 3 --dpi 150 --markups -o p3.png
takeoff render plans.pdf --page 3 --region 0.6,0.7,0.4,0.3 --hide-layer E-LIGHTING -o detail.png
takeoff text plans.pdf --page 3 --region 0.85,0.8,0.15,0.2 --json
takeoff search plans.pdf "DUPLEX"
takeoff layers plans.pdf

takeoff split plans.pdf --every 1 -o sheets/            # or --ranges "1-12,13-40" or --bookmarks
takeoff merge a.pdf b.pdf c.pdf -o set.pdf
takeoff extract plans.pdf --pages "3,1-2" -o subset.pdf
takeoff rotate plans.pdf --pages 4-6 --turns 1           # in place, keeps "(original).pdf"
takeoff delete plans.pdf --pages 7
takeoff slip-sheet plans.pdf "plans rev C.pdf" --dry-run   # match by sheet number; drop --dry-run to swap

takeoff export plans.pdf --pdf-out marked.pdf --csv markups.csv --summary quantities.csv
takeoff export plans.pdf --xlsx takeoff.xlsx             # markups + summary sheets

takeoff models list                                      # installed models + memory fit
takeoff models catalog --context 65536

takeoff agent plans.pdf --page 3 --model local:Qwen3.8-27B-Q4_K_M "count the duplex receptacles"
takeoff agent plans.pdf --base-url http://localhost:1234/v1 --model qwen3-vl-30b "set the scale"

takeoff eval run --base-url http://localhost:1234/v1 --model qwen3-vl-30b     # score the agent on evals/cases
takeoff eval probe --base-url http://localhost:1234/v1 --model qwen3-vl-30b   # box accuracy per coordinate convention
```

Page tools carry the takeoff along, exactly as in the app. `takeoff eval` is described in
[Evals](evals.md).

## MCP

`takeoff mcp [plan.pdf]` serves the drawing tools over the Model Context Protocol (stdio). The
client's own model drives them, and the takeoff is saved next to the PDF after every change.

Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "takeoff": {
      "command": "/Applications/Takeoff Desk.app/Contents/Resources/bin/takeoff",
      "args": ["mcp"]
    }
  }
}
```

Then ask it: *"Open ~/Plans/E-201.pdf and count the duplex receptacles on page 2."* The extra
tool `open_drawing {path}` picks the file. `view` returns images, sized and captioned for the
client model. Set `TAKEOFF_MCP_MODEL=gemini` (or `qwen3`) when the client model uses another
coordinate convention.

Open the same PDF in Takeoff Desk to see and confirm what the agent marked.

## Claude Code

The repo is a Claude Code plugin marketplace, so the same tools are two commands away in a
terminal:

```
/plugin marketplace add constructionlive/takeoffbeam
/plugin install takeoff-beam@takeoffbeam
```

Then `/takeoff-beam:takeoff ~/Plans/E-201.pdf count the duplex receptacles on page 2`, or just
mention a drawing: the plugin's `takeoff` skill knows to set and check the scale first, zoom
before marking, sweep in tiles and report from `summary`. The plugin starts `takeoff mcp`
through `claude-plugin/scripts/takeoff-mcp.js`, which finds the binary inside the installed app
(or on `PATH`, or at `TAKEOFF_BIN`). See [claude-plugin/README.md](../claude-plugin/README.md).

## Serving pages to an app

`takeoff serve plan.pdf` keeps one drawing open and renders pages, or tiles of them, on request:
JSON, one message per line over stdio. HaloLive's Takeoff Bench uses it so a sheet stays sharp as
you zoom without reopening the PDF for every tile.

```
→ {"id":1,"op":"pages"}
← {"id":1,"ok":true,"pages":[{"page":1,"label":"[1] T1.01 TITLE SHEET","width":36.0,"height":24.0,"unit":"in"}]}
→ {"id":2,"op":"render","page":1,"dpi":300,"region":[0.5,0.25,0.125,0.125],"out":"/tmp/tile.png"}
← {"id":2,"ok":true,"out":"/tmp/tile.png","width":1350,"height":900,"dpi":300.0}
```

The first line out is `{"ready":true,"pages":N,"version":"…"}`; it reopens the PDF if the file
changes, and exits when stdin closes.
