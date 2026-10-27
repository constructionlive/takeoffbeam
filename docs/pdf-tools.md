# PDF tools

## Pages panel

The thumbnails show a badge with the number of takeoff items on each page.
- Click a thumbnail to go to that page.
- Click its corner (or ⌘/Shift-click) to pick several pages for these actions:
  - **Rotate** clockwise or counter-clockwise.
  - **Insert** a blank page, or the pages of another PDF.
  - **Extract** the picked pages to a new PDF.
  - **Delete** the picked pages.
- **Drag** a thumbnail to move the page.
- **Split** and **Merge** sit in the same toolbar and in the *Document* menu.

**Your takeoff follows the pages.**
- Deleting a page drops its markups.
- Rotating a page turns its markups with it.
- Moving or inserting pages renumbers everything.
- Extracted, split and merged files get matching `.takeoff.json` files, so their markups come
  along.

Tools that rewrite the open PDF (rotate, delete, insert, move) ask once per session, and they keep
a one-time backup, `<name> (original).pdf`, next to the file. Undo puts the PDF back as it was
before the edit, along with the takeoff.

- **Split:** every N pages, by page ranges (`1-12, 13-40, 41-`), or one file per top-level
  bookmark.
- **Merge:** reorder the list, then *Merge and open*.

## Compare revisions

*Document → Compare revisions…* overlays the same page of another revision:
- **red**: only on this sheet;
- **blue**: only on the other one;
- **black**: unchanged.

The bar at the bottom shows the share of the sheet that changed and how many takeoff items sit on
the changes (click to select them), toggles the overlay, and follows you as you change pages.

## Slip-sheets and sets

*Document → Slip-sheet new revision…* replaces sheets with a newer revision, matched by sheet
number. The takeoff moves onto the new pages, the old pages are kept for *Compare with the
previous revision*, and one undo puts it all back. Sets group several drawings into one project.
See [Slip-sheets and sets](sets.md).

## Layers

The **Layers** tab lists the PDF's optional-content layers (the CAD layers in most exported
sets). Toggle them with the eye icons, or use the filter, *Show all* and *Hide all*.
- **The AI sees the same layers.** Hiding *E-LIGHTING* hides it for the agent too.
- **The PDF itself is never changed.** Takeoff Desk renders through a small in-memory update to
  the PDF's layer settings.

## Search and bookmarks

- **Search (⌘F)** finds text on every sheet and highlights the hits on the drawing. Click a result
  to zoom to it.
- **Bookmarks** shows the PDF outline.

## Export

*File → Export*:
- **Marked-up PDF (flattened):** your takeoff and markups drawn into the pages as real vector
  content, so any viewer or printer shows them. Badges, values and labels are included.
- **Page image (PNG):** the current page at 200 DPI with markups.
- **Markups list / Quantity summary (CSV).**
- **Markups list + summary / Quantity summary (Excel):** the same columns as the CSVs, as an
  `.xlsx` with numbers as numbers.
- **Takeoff file (JSON):** the sidecar, for construction.live or your own tools.
