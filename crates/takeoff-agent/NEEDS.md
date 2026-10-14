# takeoff-agent: notes for the parent / other crates

## Coordinate dialects (verified 2026-10-14)
- Qwen3.8-27B (llama.cpp, ggml-org Q4_K_M + Q8 mmproj) answers `bbox_2d [x1,y1,x2,y2]` on a
  0-1000 grid, as `takeoff_core::coords::dialect_for` assumes (`Grid1000Xy`). Probe on
  drawing.pdf p1: 23/23 boxes on the green shore posts as Grid1000Xy; 10/23 if read as [y,x],
  0/23 as pixels (`examples/dialect_probe.rs`). No change needed to the family table.
- Gemma 4 is mapped to `Pixel` (family Other). Not probed (model not on this machine).

## Wishes for takeoff-pdf / takeoff-core (worked around, not blocking)
- `Drawing` cannot be shared across threads, so the agent thread opens its own copy of the
  PDF. pdfium serialises calls anyway, so a long sweep render can delay the UI's renders slightly.
- `burn::BurnOptions` has no "extra items" overlay. set_scale/calibrate push a temporary
  `probe_*` measurement into `doc.data` (outside `edit`, so no undo step) for the caliper
  render and remove it right after. The UI may see it for a frame; it is easy to filter (`id`
  starts with `probe_`).
