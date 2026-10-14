// Generates golden outputs from construction.live's TypeScript so takeoff-core
// can assert identical results. Maintainers only (needs the platform source):
//   PLATFORM=/path/to/construction.live scripts/parity/gen.sh
import { buildMeasurementSeqMap, buildSeqMap, computeMeasurementValue, unitKey } from "@platform/src/components/EstimationBench/takeoffTypes";
import { boxToPageFractions, claudeResizedSize, coordDialect, imageFit, parseBoxes, safeMaxEdge } from "@platform/convex/agentTools/visionCoords";

// Deterministic pseudo-random marks: jittered rows across 3 pages, 3 categories.
let seed = 42;
const rnd = (): number => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
const marks = Array.from({ length: 400 }, (_, i) => ({
	id: `m${i}`, categoryId: ["a", "b", undefined][i % 3], page: 1 + (i % 3),
	x: rnd(), y: Math.floor(rnd() * 20) / 20 + rnd() * 0.03, w: 0.01, h: 0.01, source: "ai", status: "pending",
}));
const measurements = Array.from({ length: 60 }, (_, i) => ({
	id: `ms${i}`, page: 1 + (i % 2), type: i % 2 ? "area" : "length", categoryId: i % 4 ? "a" : undefined,
	points: Array.from({ length: 3 + (i % 4) }, () => ({ x: rnd(), y: rnd() })),
}));
const scale = { page: 1, p1: { x: 0.1, y: 0.2 }, p2: { x: 0.35, y: 0.27 }, distance: 23.5, unit: "ft" };
const values = measurements.map((m) => computeMeasurementValue(m as never, scale, 2592, 1728));
const models = ["google/gemini-3.6-flash", "anthropic/claude-opus-5.5", "openai/gpt-5.4", "x-ai/grok-4.5", ""];
const sizes = [[3400, 2200], [1092, 1092], [800, 4000], [7200, 4800], [1568, 1568], [1569, 100]];
const out = {
	marks, measurements, scale,
	markSeq: Object.fromEntries(buildSeqMap(marks as never)),
	measurementSeq: Object.fromEntries(buildMeasurementSeqMap(measurements as never)),
	values,
	unitKeys: Object.fromEntries(["sq ft", "SF", "ft²", "SM", "m2", "EA", "each", "Sq.Yd", "LF"].map((u) => [u, unitKey(u)])),
	claude: sizes.map(([w, h]) => claudeResizedSize(w, h)),
	fits: models.flatMap((m) => sizes.map(([w, h]) => ({ model: m, w, h, fit: imageFit(m, w, h), safe: safeMaxEdge(m, 2000), dialect: coordDialect(m) }))),
	boxes: (["grid1000", "pixel"] as const).map((d) => ({
		dialect: d,
		parsed: parseBoxes(d, "Sure:\n```json\n[{\"box_2d\":[100,200,300,450],\"label\":\"A\"},{\"box\":[\"5\",6,7,8]},{\"box\":[1,2,3]}]\n```"),
		page: boxToPageFractions(d, { x1: 100, y1: 200, x2: 300, y2: 450 }, { w: 1000, h: 800 }, { x: 0.25, y: 0.5, w: 0.5, h: 0.25 }),
	})),
};
process.stdout.write(JSON.stringify(out, null, 1));
