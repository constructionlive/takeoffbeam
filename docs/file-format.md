# File format: `.takeoff.json`

The takeoff lives next to the drawing: `plan.pdf` → `plan.pdf.takeoff.json`. It is plain JSON,
diff-friendly, and compatible with construction.live's takeoff model.

**Coordinate rule:** every position is a fraction (0–1) of the full page as displayed. It is never
pixels, never a model's 0–1000 grid, and never relative to a zoomed view. Pixels only exist at
render time.

```jsonc
{
  "version": 1,
  "reviewType": "takeoff",
  "lastUpdated": "2026-10-14T18:03:12Z",
  "pages": [{ "page": 1, "widthPx": 7200, "heightPx": 4800 }],        // aspect only (200 DPI)
  "categories": [{
    "id": "c_…", "name": "Duplex receptacle", "color": "#4CC2FF",
    "kind": "count",                         // count | length | area
    "description": "circle with two lines",  // for the AI
    "icon": null, "lineStyle": "dashed", "hatch": "diagonal", "opacity": 0.8,
    "unitCost": 42.0, "hidden": false        // Takeoff Desk extensions
  }],
  "marks": [{                                // counts: top-left + size
    "id": "m_…", "categoryId": "c_…", "page": 1, "x": 0.41, "y": 0.22, "w": 0.006, "h": 0.009,
    "source": "ai", "status": "pending", "note": "GFCI",
    "fields": { "f_…": "Phase 2" },
    "author": "AI", "createdAt": 1791993600000, "modifiedAt": 1791993600000,
    "layer": null, "review": null, "locked": false
  }],
  "measurements": [{
    "id": "ms_…", "page": 1, "type": "area",  // length | area | angle
    "categoryId": "c_…", "points": [{ "x": 0.1, "y": 0.1 }, …],
    "closed": false,                          // length: perimeter
    "cutouts": [[{ "x": …, "y": … }, …]],      // area: holes
    "depth": 0.33,                            // area: → volume
    "value": 267.0, "unit": "ft²", "label": "Pour 2", "source": "user"
  }],
  "pageScales": [{ "page": 1, "p1": { "x": 0, "y": 0 }, "p2": { "x": 0.0069, "y": 0 }, "distance": 1, "unit": "ft" }],
  "viewports": [{ "id": "vp_…", "page": 1, "name": "Detail 3", "x": 0.6, "y": 0.1, "w": 0.3, "h": 0.3, "scale": { … } }],
  "annotations": [{
    "id": "an_…", "page": 1, "kind": "callout",  // text callout cloud rectangle ellipse line arrow polyline polygon highlight ink stamp
    "points": [{ … }, { … }, { … }], "text": "Verify in field",
    "style": { "stroke": "#E8531A", "fill": null, "width": 1.5, "opacity": 1, "fontSize": 10, "lineStyle": null },
    "subject": "Callout", "review": "Accepted"
  }],
  "customFields": [{ "id": "f_…", "name": "Phase" }],
  "hiddenLayers": ["E-LIGHTING"],
  "layers": [{ "name": "E-LIGHTING", "description": "lighting fixtures and circuits" }],
  "display": { "lengthFormat": "feetInches", "precision": 2, "fractionDenominator": 16 }
}
```

- **Scale:** the real distance between `p1` and `p2`. Units per pixel are derived at use time,
  so the scale doesn't depend on DPI. Presets store one paper inch along the page width.
- **Values:** `value`/`unit` are recomputed on every change. Consumers may read them, but the
  points are the truth.
- **construction.live compatibility:** platform files load unchanged. Takeoff Desk files import
  into the platform, which keeps the fields it knows: categories, marks, measurements, page
  scales, layers and custom fields.
- **Badge numbers** are reading order per item: page → rows top to bottom → left to right. They
  are recomputed, never stored.
