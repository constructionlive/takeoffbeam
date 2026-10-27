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
  "overlays": [{
    "id": "ov_…", "page": 1, "x": 0.5, "y": 0, "w": 0.5, "h": 1,  // page fractions
    "svg": "<svg …>…</svg>",           // or "image": "/abs/path/zones.png"
    "opacity": 0.6, "label": "Pour zones", "hidden": false, "source": "ai"
  }],
  "spaces": [{                               // named regions quantities belong to
    "id": "sp_…", "page": 1, "name": "Unit 204", "color": "#34D399",
    "points": [{ "x": 0.1, "y": 0.1 }, …],    // polygon, implicitly closed
    "hidden": false, "author": "Ana", "createdAt": 1791993600000
  }],
  "legends": [{                              // a legend box on the sheet
    "id": "lg_…", "page": 1, "x": 0.68, "y": 0.72, "w": 0.28, "h": 0.2,
    "title": "Electrical",                   // unset = "Legend"
    "scope": "page",                         // page | document
    "categoryIds": ["c_…"],                  // only these items; empty = all in scope
    "columns": { "count": true, "quantity": true, "cost": false },
    "markups": false                         // also list markups by subject
  }],
  "customFields": [{ "id": "f_…", "name": "Phase" }],
  "hiddenLayers": ["E-LIGHTING"],
  "layers": [{ "name": "E-LIGHTING", "description": "lighting fixtures and circuits" }],
  "display": { "lengthFormat": "feetInches", "precision": 2, "fractionDenominator": 16 },
  "sheetRevisions": [{ "page": 2, "sheet": "A-102", "file": "plan.pdf.revisions/plan 2026-10-27 1403.pdf", … }]
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
- **Spaces** store only their outline. Which space an item is in is worked out from geometry on
  every change, never stored on the item (see [Spaces](takeoff.md#spaces)), so moving a mark
  or reshaping a space updates the Space column by itself. Spaces nest by geometry; there is no
  parent field.
- **Legends** store only their box and options. Their rows (items, counts, totals) are computed
  from the takeoff whenever they are drawn.
- **Adding fields without breaking anyone:** `spaces` and `legends` are left out of the file
  when empty, so a takeoff without them is byte-for-byte the shape it was before. Every reader
  ignores keys it doesn't know: older Takeoff Desk versions and construction.live open files
  with spaces and legends (and drop them if they save), and this version ignores keys added by
  newer ones. A legend `scope` this version doesn't know is read as `page`; missing `columns`
  entries take their defaults.

## Sheet revisions

A [slip-sheet](sets.md) keeps the pages it replaced in a history PDF next to the drawing:
`plan.pdf` → `plan.pdf.revisions/plan 2026-10-27 1403.pdf`, with its own `.takeoff.json` (the
takeoff of those pages as it was). The drawing's sidecar lists them, oldest first:

```jsonc
"sheetRevisions": [{
  "page": 2,                          // the page it is an earlier revision of, as the drawing is now
  "sheet": "A-102",                   // the new page's sheet number
  "file": "plan.pdf.revisions/plan 2026-10-27 1403.pdf",   // relative to the drawing's folder
  "filePage": 2,                      // its page in that file
  "replacedAt": "2026-10-27T14:03:12Z",
  "from": "plan rev C.pdf", "fromPage": 5,                 // where the new page came from
  "slip": "sl_…",                     // one id per slip-sheet
  "changed": 0.012,                   // share of the sheet that changed
  "review": ["m_…", "ms_…"],          // items on changed parts of the sheet: check them
  "warning": "Page size changed (36.0×24.0 in → 42.0×24.0 in): …"
}]
```

The field is optional: files without it load as before, and readers that don't know it (the
platform, older Takeoff Desk) ignore it. Page tools renumber it with the pages.

## Set files

A [set](sets.md) is a separate file, `Tower.takeoffset.json`, that lists drawings in order. Each
drawing keeps its own `.takeoff.json`; the set holds no takeoff.

```jsonc
{
  "version": 1,
  "name": "Tower",
  "documents": [
    { "path": "Tower - Architectural.pdf", "name": "Architectural" },   // relative to the set file
    { "path": "Structural/S-set.pdf" },                                 // name: the file name
    { "path": "/Volumes/Projects/Tower/MEP.pdf" }                       // or absolute
  ],
  "lastUpdated": "2026-10-27T14:03:12Z"
}
```

Unknown fields are ignored on load.

## Document map

[Graph view](graph-view.md)'s map is a separate file: `plan.pdf` → `plan.pdf.map.json`. The AI
writes it through its map tools, after every step. Regions use the same page-fraction rule as the
takeoff; pictures are cut from the PDF, never stored.

```jsonc
{
  "version": 2, "builtAt": "2026-10-21T14:02:11Z", "model": "claude-opus-5-5",
  "overview": "A four-storey office building…", "pageCount": 40,
  "sheets": [{
    "page": 3, "number": "A-201", "title": "Second floor plan", "discipline": "Architectural",
    "summary": "…", "mapped": true,             // looked at closely, not just indexed
    "parts": [
      { "id": "s3.1", "kind": "plan", "label": "1", "title": "Level 2", "summary": "…",
        "region": { "x": 0.04, "y": 0.06, "w": 0.7, "h": 0.86 } },
      { "id": "s3.2", "kind": "callout", "label": "3/A-501", "title": "", "parent": "s3.1",
        "region": { "x": 0.31, "y": 0.42, "w": 0.012, "h": 0.016 } }
    ]
  }],
  "links": [{ "from": "s3.2", "to": "s21.4", "kind": "detail", "text": "3/A-501",
              "note": "The north stair's handrail bracket." }],
  "unresolved": [{ "from": "s3.1", "text": "M-101, mechanical set" }]
}
```

- **Node ids:** a sheet is `s{page}`, an area on it `s{page}.{n}`. `parent` is the area it sits in.
- **Kinds:** areas are `plan detail section elevation schedule legend notes callout element
  title_block key_plan diagram other`. Links are `detail section elevation enlarged continues
  schedule governs same reference`.
- **`unresolved`** holds references to drawings that are not in this PDF.
