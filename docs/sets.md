# Slip-sheets and sets

## Slip-sheet a new revision

*Document → Slip-sheet new revision…* (or the ↻ button in the Pages panel) replaces sheets with
their newer revision, the way Bluebeam's "replace with new revision" does. Pick the new PDF, or
several.

**Review.** Each new page is matched to the sheet it replaces:
- **by sheet number** first. The number comes from the document map, a page label, the title
  block's text (the token next to *Sheet No.*, in big type, towards the bottom-right corner) or a
  bookmark. `A-201`, `A201` and `a 201` are the same sheet. Dates, scales, job numbers and
  references to other sheets in the notes are not taken for it.
- **by page order** for pages without a number.
- a page whose number isn't in the drawing is left out, or added at the end if you choose
  *Add as a new sheet*.

Change any match in its menu, or set it to *Skip*. A ⚠ marks a new page on paper of another shape.

**Apply.** The new pages take the old ones' places in the PDF. Your takeoff and markups stay on
them:
- **Same paper shape:** every item keeps its exact position (positions are fractions of the page,
  so a half-size reissue lines up too).
- **Another shape:** the old sheet is taken as drawn to fit the new one (scaled evenly, centred).
  Items, scales and viewports move with it, so lengths stay true. You get a warning to check them.

**One undo puts everything back**: the old PDF and the takeoff as it was. Redo swaps the new
pages in again. The first edit also keeps `<name> (original).pdf`, like the page tools.

**History.** The replaced pages are kept in `plan.pdf.revisions/plan 2026-10-27 1403.pdf`, with
their takeoff as it was. Revised pages show a ⟲ badge in the Pages panel.

## What changed, and what to check

*Document → Compare with the previous revision* (or a revised thumbnail's right-click menu)
overlays the page with the revision it replaced: red is only on the new sheet, blue only on the
old one. It follows you to the next revised page.

The compare bar counts the **items on changes**: takeoff items and markups that touch a part of
the sheet that changed. Click it to select them. The same list is saved when the slip-sheet is
applied (`review` in [the file](file-format.md#sheet-revisions)).

## Sets

A set groups several drawings, such as Architectural, Structural and MEP, into one project.
- *File → New set…*: pick the PDFs, then save the set file (`Tower.takeoffset.json`, best next to
  them). *File → Open set…* opens one; you can also drop it on the window.
- The **Set** tab in the left panel lists every sheet of every drawing. Click a sheet to open its
  drawing in a tab, at that sheet.
- **Project takeoff** (Σ in the Set tab, or *File → Project takeoff…*) adds up the items of every
  drawing: one column per drawing, then the total and cost, with CSV export. Items with the same
  name, type and unit are one line. The *Markups* tab lists every drawing's markups; click one to
  go there.
- **Slip-sheet into the set** (↻ in the Set tab): new pages are matched against the sheets of every
  drawing, and each goes to its own drawing. Each drawing's swap is one undo step in its tab.

Each drawing keeps its own `.takeoff.json`, which stays the truth: the set file only lists the
PDFs. Open drawings are read live; the others from their files.

From the command line, `takeoff slip-sheet plan.pdf "plan rev C.pdf"` does the same swap (add
`--dry-run` to see the matches, `--add-new` to append new sheets).
