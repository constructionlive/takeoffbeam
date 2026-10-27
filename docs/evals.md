# Evals and the vision probe

Two ways to measure the AI agent, so a prompt or tool change can't quietly make it worse:
- **`takeoff eval run`** runs the agent headless on drawings with known answers and scores the
  takeoff it leaves: counts, lengths, areas, the scale, title-block fields.
- **`takeoff eval probe`** measures how accurately a model places boxes on a sheet, for each
  [coordinate convention](ai.md#how-it-sees-coordinates). It answers "can this model do full
  takeoff, or only assist?".

Both read the same case files and write a JSON report, so runs can be compared.

## Running

```sh
# A local llama-server, LM Studio or Ollama
takeoff eval run --base-url http://127.0.0.1:8080/v1 --model qwen3.8-27b
takeoff eval probe --base-url http://127.0.0.1:11434/v1 --model gemma3:4b

# An installed local model: Takeoff Desk starts llama-server for it
takeoff eval run --model local:Qwen3.8-27B-Q4_K_M --only title-block

# Several models in one report, each in every dialect
takeoff eval probe --base-url https://… --api-key … --model google/gemini-3.6-flash,anthropic/claude-sonnet-5

# Compare with an earlier run
takeoff eval run --model … --baseline evals/runs/run-1793120392.json
```

Run them from the repo root: the cases default to `evals/cases`, and reports go to
`evals/runs/<run|probe>-<time>.json` (not committed). Other options:
- `--only <text>`: just the cases whose name contains it.
- `--timeout <s>`: cancel a case after this long (default 1800). A case's own `timeout_secs` wins.
- `--repeat <n>`: run every case n times, to see how much a model varies.
- `--workspace <dir>`: give the agent its shell.
- `--dialect all|native|gemini,qwen3,pixel` (probe): which conventions to ask in. `native` is the
  one Takeoff Desk picks for that model today.

`takeoff eval run` prints one row per case (score, wall time, steps, tokens), then each expected
result with what the agent got and the error. This is Qwen3.8 27B on an M3 Max, with a 900 s
timeout:

```
case                         model                       score     time  steps     tokens  notes
rooms-measure                Qwen3.8-27B-Q4_K_M            5/6     910s     14     230222  timed out after 900s
    ✓ scale                              want 1/4" = 1'-0"     got 4.018 ft/in      +0.5%
    ✓ area office                        want 300 sf           got 297.3 sf         -0.9%
    ✓ area storage                       want 180 sf           got 178.0 sf         -1.1%
    ✗ length exterior                    want 94 ft            got 0.0 ft           -100.0%
    ✓ answer sheet number                want A-101            got A-101
    ✓ answer sheet title                 want FIRST FLOOR PLAN got FIRST FLOOR PLAN
symbols-count                Qwen3.8-27B-Q4_K_M            2/3     910s      8     126152  timed out after 900s
    ✓ count receptacle                   want 9                got 9                +0.0%
    ✗ count light                        want 6                got 0                -100.0%
    ✓ answer sheet number                want E-101            got E-101
title-block                  Qwen3.8-27B-Q4_K_M            4/4     184s      2      23733
    ✓ answer project                     want EVAL PROJECT     got EVAL PROJECT
    ✓ answer sheet number                want A-101            got A-101
    ✓ answer sheet title                 want FIRST FLOOR PLAN got FIRST FLOOR PLAN
    ✓ answer scale                       want 1/4"             got 1/4"
total 11/13 expectations passed
```

Both cases with a miss hit the 900 s timeout while the agent was still working: what it had
finished was right, but the walls and the lights were not done. Give a 27B local model 30 minutes
a case. (A length with nothing matching now reads "nothing matching" rather than 0.0 ft.)

Each case's resulting takeoff is saved beside the report (`run-<time>.takeoffs/<case>.takeoff.json`)
so you can see what the agent marked. The drawing's own takeoff file is never touched: every run
starts from an empty takeoff.

The same runs are available as ignored tests, without the CLI:

```sh
TAKEOFF_BASE_URL=http://127.0.0.1:11434/v1 TAKEOFF_MODEL=gemma3:4b \
  cargo test -p takeoff-agent --test eval_harness -- --ignored --nocapture
```

`TAKEOFF_ONLY`, `TAKEOFF_DIALECTS`, `TAKEOFF_TIMEOUT` and `TAKEOFF_REPORT=out.json` do what the
CLI options do.

## Cases

One JSON file per case in `evals/cases`:

```json
{
	"name": "symbols-count",
	"pdf": "../drawings/symbols.pdf",
	"page": 1,
	"task": {
		"prompt": "Count the duplex receptacles … as count items named \"Duplex receptacles\" …",
		"setup": { "scale": "1/8\" = 1'-0\"" },
		"expect": [
			{ "kind": "count", "item": "receptacle", "value": 9 },
			{ "kind": "area", "item": "office", "value": 300, "unit": "sf", "tol_pct": 5 },
			{ "kind": "length", "item": "exterior", "value": 94, "unit": "ft" },
			{ "kind": "scale", "preset": "1/4\" = 1'-0\"" },
			{ "kind": "answer", "field": "sheet number", "value": "E-101" }
		]
	},
	"probe": {
		"region": [0.04, 0.06, 0.74, 0.86],
		"targets": [
			{ "ask": "every duplex receptacle symbol", "boxes": [[0.141, 0.107, 0.011, 0.028]] },
			{ "ask": "every shore post", "item": "Shore posts" },
			{ "ask": "the sheet number in the title block", "text": "A-101" }
		]
	}
}
```

- **`pdf`** is relative to the case file. For drawings that can't be committed, start it with an
  environment variable: `"$TAKEOFF_EVAL_DRAWINGS/E-201.pdf"`. Without the variable or the file, the
  case is skipped, not failed.
- **`setup.scale`** gives the page a scale before the agent starts, for cases that only test
  counting.
- **`expect`**: `item` matches the names of the agent's takeoff items (and measurement labels) by
  substring, ignoring case, so say in the prompt what to name them. Without `item`, everything of
  that kind on the page counts.
  - `count`: marks; `tol` is absolute (default 0).
  - `length`, `area`: the sum of matching measurements, converted to `unit` (ft, in, m, sf, m2…);
    `tol_pct` defaults to 5.
  - `scale`: the page's scale, as a `preset` label or `ft_per_in` (feet per paper inch);
    `tol_pct` defaults to 2.
  - `answer`: the agent's replies mention `value`, ignoring case and spaces (title-block fields,
    a note, a schedule entry).
- **`probe.targets`**: what to ask for, and where the answers really are, as any of:
  - `boxes`: page fractions `[x, y, w, h]`;
  - `item`: the count marks of that item in the drawing's own takeoff. **This is how you mark targets
    by hand:** open the drawing in Takeoff Desk, count the symbols as an item, save, and name the
    item here;
  - `text`: a string printed on the sheet, found with the PDF's own text.

The committed cases use two synthetic sheets in `evals/drawings`, so the answers are exact:
- `symbols.pdf`: an E-sheet at 1/8" = 1'-0" with 9 duplex receptacles and 6 light fixtures in
  the plan, plus one of each in a legend that must not be counted.
- `rooms.pdf`: an A-sheet at 1/4" = 1'-0" with two dimensioned rooms (300 sf and 180 sf, 94 ft
  of outside wall) and a title block.

They are written by `crates/takeoff-agent/src/eval/synth.rs`. After changing it, rewrite them
with `takeoff eval drawings` (or `TAKEOFF_BLESS=1 cargo test -p takeoff-agent --test eval_harness`)
and update the boxes in the cases. A test fails while they disagree.

## The probe

For each case, model and dialect, the probe:
1. renders the view the way the agent's `view` tool does: same size for that model, same caption,
   with the reading rule for the dialect being tested;
2. asks for every target, one question per target, in that dialect;
3. matches the boxes to the truth one-to-one, closest first. A box is **on target** when its
   centre is within one target size of the target's centre.

It reports, per question: hits, boxes given, the median centre error in view pixels and in % of the
view's long edge, and the mean overlap (IoU). Each reply is also read in the other two dialects. A
model that answers in a convention it wasn't asked for shows up under **best reading**.

The summary gives one verdict per model and dialect:
- **full takeoff**: at least 90% of targets found, at least 80% of its boxes on target, and a median
  error of at most 1.5% of the view. The model can place marks itself.
- **assist**: at least half the targets found. Let it read, find and check, and run detection on a
  stronger model.
- **not usable**: it should not place marks.

The last lines answer the question per model, and say when another dialect would do better than
the one Takeoff Desk uses for it now. That is a change to `dialect_for` in
`takeoff-core/src/coords.rs`.

## In CI

`cargo test` runs the harness against scripted models: a run that marks 9 of 9 receptacles and 5 of
6 lights must score exactly that, and a probe against a model that answers in Qwen's `[x, y]` grid
must find it on target in that dialect and off in the others. No real model runs in CI.
