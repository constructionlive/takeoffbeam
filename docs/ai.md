# The AI agent

The **AI** tab (⌘J) is a drawing agent that works on the sheet you have open. It can:
- read the title block and set the scale, checking it against a second dimension;
- count symbols, by sweeping the sheet tile by tile or matching a legend symbol;
- measure runs and areas, snapping its points onto the drawing's lines;
- read notes, schedules and tags;
- add callouts and clouds;
- review your takeoff for gaps.

It sees the sheet the way you do: same layers, same markups, numbered badges included.

![AI panel](images/dark-ai.png)

## You stay in charge

- **Its marks wait for you.** Everything it adds is marked **to review**: dashed outline, "AI"
  author. Confirm items one at a time (Properties or right-click), per item (*Confirm all* in the
  Takeoff tab), or with *Edit → Confirm all AI marks*.
- **Its edits are undoable.** Every edit is an ordinary step in the undo history.
- **You can watch it.** While it works, an orange box on the canvas shows where it is looking.
  The chat shows each step (view, add marks, set scale…), with thumbnails of what it saw.
- **It only touches this drawing.** It has no file system, shell or web access. Its only tools are
  the drawing tools below.
- **Stop** ends the run at the next step.

The prompt line knows your context: the current page, the visible region when you are zoomed in,
and your selection. That makes "count these", "measure the selected wall" and "what's this note"
work.

## Choosing a model

Click the model button at the top of the AI tab (or *Choose a model…*) to open **Models**.

### On this computer (private, free)

Local models run with llama.cpp on your machine, and your drawings never leave it. Use *Download*
for vision models that can read drawings (Qwen3.8 27B, Gemma 4 26B/12B/E4B/E2B), or *Import a
.gguf* for a model you already have. Models installed by construction.live Desktop are found
automatically.

**Before anything loads, Takeoff Desk checks whether the model fits.** It reads the model file's
header and adds up:
- the weights;
- the context cache (only the layers that have one; hybrid models like Qwen3.5+ cache 1 layer in
  4);
- the fixed recurrent state;
- the working buffers.

It compares that with what llama.cpp says your GPU may use. On Apple silicon that is the Metal
limit, e.g. 28.1 GB on a 36 GB Mac. The result is one of:
- **Fits**;
- **Tight**: close other apps first;
- **Not enough memory**: with the largest context that would fit.

*Compact context (Q8 cache)* halves the context memory with negligible quality loss.

### construction.live (cloud)

*Sign in with construction.live* shows a code and opens your browser. Approve the device there,
and Gemini, Claude, GPT and other cloud models appear, billed to your construction.live credits.
Your drawing images are sent to the model provider. The sign-in token is kept in the OS keychain.
*Sign out* forgets it.

### Custom endpoint

Any OpenAI-compatible server works: LM Studio, Ollama (`http://localhost:11434/v1`), vLLM,
OpenRouter, or a llama-server you run yourself. *Test & list models* checks the connection.

## How it sees coordinates

Models answer positions in different conventions:
- **Gemini:** a 0–1000 grid, `[y, x]`.
- **Qwen3-VL:** a 0–1000 grid, `[x, y]`.
- **Claude, GPT and most others:** pixels.

Takeoff Desk picks the convention per model, sizes each render so the model sees it unresized, and
converts every answer back to page positions in code. The model never does coordinate maths: it
reads positions off a numbered **view** (`v3`) and the tools convert them.

## Agent tools

| Tool | What it does |
|---|---|
| `get_drawing` | Pages, scales, items with counts, layers |
| `view` | Render a page or region with markups and badges; registers a view id |
| `read_items` / `find_items_at` | List items; find which item is at a spot in a view |
| `add_marks` / `add_measurements` | Count marks; lengths, areas, angles (optionally snapped to lines) |
| `update_items` / `delete_items` | Edit or remove items |
| `create_category` / `update_category` | Takeoff items |
| `summary` | Counts and totals per page and item |
| `add_annotation` | Clouds, callouts, text, arrows… |
| `set_scale` / `set_scale_preset` / `check_scale` / `calibrate` | Scale from a dimension, a printed scale, or automatically; measure-back check |
| `detect` | Tiled symbol sweep, optionally against a legend crop |
| `trace` | Trace polylines/polygons with vision |
| `read_text` / `search_text` | The PDF's own text with positions |
| `set_hidden_layers` | Layer visibility (shared with you) |
| `follow_line` | Measure a whole connected run from one point |
| `set_viewport` | Show where it's looking |

The same tools are available to other AI apps through the [MCP server](cli.md#mcp).

## What to expect from local models (measured)

These are measurements from Qwen3.8 27B (Q4_K_M) on an M3 Max with 36 GB.

- **Coordinates.** The model's boxes were exactly right: 23 of 23 symbols on target with the
  Qwen 0–1000 `[x,y]` convention. The other conventions scored 10 of 23 and 0 of 23.
- **Scale.** It set the scale from a grid dimension and checked it back against a second
  dimension within 2.4%. On another run it rejected a bad automatic calibration (40% cross-check
  disagreement) and set the scale by hand instead.
- **Speed.** About 30–140 s per step, so a 15-step job takes 10–30 minutes. Use a cloud model for
  interactive work, or a smaller local model. Thinking is off by default for local models; turn
  it on in *Models* for hard sheets.
- **Not yet verified with a live local model:** full symbol sweeps (`detect`), `trace` and
  `follow_line`. They are covered by tests with a scripted model.

## Your own Claude Code or Codex

If you already pay for Claude or ChatGPT, *Models → Subscriptions* runs the agent on the Claude
Code or Codex CLI installed on your computer, under the sign-in you already have. Takeoff Desk
never sees or stores your credentials.

- The CLI runs headless in an empty folder of ours, with only the drawing tools allowed (no shell,
  no file edits). The tools come from the app over a private local MCP bridge, so marks appear
  live and are undoable like any other.
- Usage counts against your plan's limits, not construction.live credits.
- Codex is found even when it only ships inside the ChatGPT desktop app. Sign in once with
  `codex login`.
- Symbol sweeps (`detect`, `calibrate`, `trace`) make their own vision calls. They use your
  construction.live model or a loaded local model.

> **Note on Claude plans:** Anthropic's terms ask third-party apps to use API keys and not to
> route requests through Pro/Max plan credentials without Anthropic's approval. See
> code.claude.com/docs/en/legal-and-compliance. Takeoff Desk only launches the unmodified Claude
> Code that you signed in to yourself. Distributors should confirm this use with Anthropic.
