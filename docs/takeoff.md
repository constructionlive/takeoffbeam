# Taking off

All quantities live in **takeoff items**: a named, colored group such as "Duplex receptacle",
"Edge form" or "Slab pour 2", of one kind (count, length or area). The **Takeoff** tab lists
them with totals and cost. Click an item to keep taking off into it.

![Takeoff panel](images/dark-takeoff.png)

## Set the scale first

Each page has its own scale. There are three ways to set it:

- **Preset:** *Takeoff → Preset scale*. Choose from architectural (1/32" = 1'-0" … 3" = 1'-0"),
  engineering (1" = 10' … 1" = 200') or metric (1:1 … 1:1000). A PDF page's size *is* its paper
  size, so a printed scale is exact without clicking anything.
- **Calibrate (K):** click both ends of a known dimension, then type its real length: `22'-6"`,
  `22' 6 1/2"`, `12.5`, `3.2 m`, `150mm`. Tick *Use for all pages of the same size* to apply it
  across the set.
- **Viewport (Z):** drag around a detail drawn at a different scale, then pick its scale.
  Measurements inside the viewport use it; everything else uses the page scale.

The status bar shows the active scale and the cursor's real-world position. *Show lengths* switches
between `12.50 ft` and `12'-6"`.

## Tools

| Tool | Key | How |
|---|---|---|
| Count | C | Click each symbol. Every click adds a numbered mark. |
| Length | L | Click points; double-click or Enter to finish; Backspace removes the last point. |
| Curve | | A length along a smooth curve through the points you click: click a few along the bend (more on a tight one); double-click or Enter to finish. |
| Trace line | | Click points **on** a drawn line (its start, its end, anywhere it forks): the length runs along that line between them, through dashes and past lines that cross it. |
| Perimeter | P | Like Length, but closes the loop. |
| Area | A | Click the corners; double-click or Enter closes it. Shows area and perimeter. |
| Rectangle area | B | Drag a rectangle. |
| Volume | U | An area, then you type its depth (e.g. 0.33 ft for a 4" slab). Shows yd³ too. |
| Cutout | X | Select an area, then click the corners of the hole to subtract. |
| Angle | G | Click arm, vertex, arm. |
| Follow line | F | Click a line on the drawing: the whole connected run becomes one length. |
| Calibrate | K | See above. |
| Viewport | Z | See above. |

**Snapping (S)** clips your clicks to the drawing's own vector lines:
- ■ endpoint
- ▲ midpoint
- ✕ intersection
- ◆ on the line

Hold **Alt** to place a point freely and **Shift** to lock to 0/45/90°. Scanned sheets have no
vectors, so snapping quietly does nothing there.

**Curves.** *Curve* and *Trace line* measure curved runs (track, roads, curbs, pipe), and the
*Curve* markup tool draws a curved line. *Trace line* uses the PDF's own line work, and on scans,
flattened sheets and lines drawn over aerial photos it follows the line's colour in the picture.
Both points of a span must be on one drawn line: a centreline that isn't drawn (midway between
two rails) can't be traced. Where no single line joins two points, that span stays straight.

Either way the result is an ordinary length (or markup): the curve is stored as many short
segments, so its value is right in the Takeoff list, the exports and construction.live.

## Editing

- **Select (V):**
  - Click an item, or drag on empty space to box-select. Shift/⌘ adds to the selection.
  - In the **Markups** list: click a row, ⌘-click to add or remove one, Shift-click for a range, or
    **Select all** (follows the filters). The bar above the list confirms or deletes the selection.
  - Drag a selection to move it, and nudge it with the arrow keys (Shift = ×10).
  - Drag a vertex to reshape, or a corner to resize.
  - **Delete** removes the selection.
- **Edit commands:** ⌘C / ⌘V / ⌘D copy, paste and duplicate. ⌘Z / ⇧⌘Z undo and redo, with 200
  steps.
- **Right-click menu:** confirm, move to another item, lock/unlock, duplicate, delete.
- **Properties tab:** every detail of the selection:
  - the item, label, note, depth, perimeter/closed and custom-column values;
  - status (*Pending* / *Confirmed* for AI marks; *Accepted*, *Rejected*, *Completed*… for
    markups);
  - author and date.

## Items, styles and the Tool Chest

Each item's **⋯** menu sets:
- **Color.**
- **Line style:** solid, dashed or dotted.
- **Hatch:** diagonal, cross or grid.
- **Opacity.**
- **Unit cost:** gives a cost column and an estimated total.
- **Description for the AI:** e.g. "circle with two parallel lines".

*Save to Tool Chest* keeps an item's style and cost for every future drawing. *New → From Tool
Chest* reuses it.

## Markups List

The bottom panel lists every count, measurement and markup with these columns: page, type, subject
(item), number, label, quantity, details (perimeter / volume), status, author, date, your custom
columns, and note.
- **Sort:** click a header.
- **Filter:** by text, *This page*, or *To review*.
- **Jump:** click a row to go to the item; double-click to zoom to it.
- **Custom columns:** add them under *Columns*. Edit the values in Properties.
- **Export:** *Export CSV* writes the full list; *Summary CSV* writes one line per item (count or
  total, unit, volume, unit cost, cost).
