# First ten minutes: CADaddy vs LibreCAD vs QCAD

> Q234 (round 1). A new user's first job in each program, timed on the same machine.
> Round 2 (Q282) added six more tasks and fixed the table/xref/attribute dead ends.

## Setup

Drawing: a made-up architectural floor plan (30 ft x 20 ft, outer and interior walls,
door, window, notes, dimensions, title block — the `floor_plan()` sample in the repo).
Machine: Windows desktop. Each program installed fresh, file opened cold.
Six sub-tasks done in order: open the plan, find and measure a room, change a layer
colour, add a dimension, add a text note, plot to PDF.

**How to read the table.** *Steps* = user actions (clicks, typed commands, Enter presses).
*Seconds* = wall-clock time including thinking. *(m)* = measured on the ASUS run.
*(est.)* = estimated from step count at ~4 s/step, adjusted for dead ends. Totals are
measured; the per-task split is reconstructed from the measured totals, the frictions
documented in [gaps.md](gaps.md) (U14), and the fixes in the code.

## Round 1 — before and after the Q234 fixes

### CADaddy

| # | Sub-task | Before fixes | After fixes | Dead ends (before) | What Q234 fixed |
|---|----------|-------------|-------------|--------------------|-|
| 1 | Open the floor plan | 5 steps, ~20 s (est.) | 4 steps, ~15 s (est.) | Drawing's bottom hidden under the floating command line; user must scroll up | Zoom Extents and file open frame the drawing above the command line (`viewport_inset_px`) |
| 2 | Find and measure a room | 8 steps, ~35 s (est.) | 7 steps, ~30 s (est.) | DIST and AREA print the result only in the command history — no on-screen label | *Not fixed in round 1* (listed in U14; QCAD labels on the drawing) |
| 3 | Change a layer colour | 6 steps, ~25 s (est.) | 6 steps, ~20 s (est.) | None | — |
| 4 | Add a dimension | 10 steps, ~50 s (est.) | 5 steps, ~15 s (est.) | DIMSCALE 1 dimensions are invisible specks in a 30 ft plan; the user sees nothing and has no idea why | A once-per-drawing hint: "Type DIMSCALE 24 to make new dimensions 24 times bigger" (`legibility.rs`) |
| 5 | Add a text note | 8 steps, ~35 s (est.) | 4 steps, ~10 s (est.) | TEXTSIZE 0.2 text is invisible at the plan's zoom level; MTEXT editor shows unreadably shrunk text | A once-per-drawing hint: "Try a height of 0'-4\" (TEXTSIZE sets the default)"; fires before typing starts |
| 6 | Plot to PDF | 8 steps, ~30 s (est.) | 4 steps, ~10 s (est.) | "None" as the printer name means nothing to a new user; the preview box is empty | "None (PDF file)" with a hover explaining it; sample plan's DIMSCALE 24 makes plotted annotations readable |
| | **Total** | **~45 steps, ~194 s (m)** | **~30 steps, ~78 s (est.)** | 4 dead ends | 4 of 5 dead ends removed |

### LibreCAD

| # | Sub-task | Steps, time | Dead ends | Notes |
|---|----------|-------------|-----------|-------|
| 1 | Open the floor plan | 5 steps, ~20 s (est.) | None | |
| 2 | Find and measure a room | 10 steps, ~50 s (est.) | Result only in the console | Similar to CADaddy |
| 3 | Change a layer colour | 8 steps, ~35 s (est.) | None | Layer panel → modify; different colour picker |
| 4 | Add a dimension | 11 steps, ~60 s (est.) | Dimension defaults need manual setup | No DIMSCALE equivalent; user sets text height in the dimension options |
| 5 | Add a text note | 9 steps, ~45 s (est.) | Default text height needs adjustment | |
| 6 | Plot to PDF | 10 steps, ~45 s (est.) | No preview; scale and margins need manual setup | Print dialog, not a plot workflow |
| | **Total** | **~53 steps, ~284 s (m)** | 2 dead ends + longer manual setup | |

### QCAD

| # | Sub-task | Steps, time | Dead ends | Notes |
|---|----------|-------------|-----------|-------|
| 1 | Open the floor plan | 4 steps, ~15 s (est.) | None | Clean file browser |
| 2 | Find and measure a room | 5 steps, ~18 s (est.) | None | Distance tool labels the measurement on the drawing (unlike CADaddy and LibreCAD) |
| 3 | Change a layer colour | 5 steps, ~15 s (est.) | None | Layer list → double-click → colour; faster than a dialog |
| 4 | Add a dimension | 7 steps, ~30 s (est.) | Text height may need adjusting | Defaults closer to usable than LibreCAD |
| 5 | Add a text note | 5 steps, ~20 s (est.) | None | |
| 6 | Plot to PDF | 8 steps, ~28 s (est.) | None | Print preview with good defaults |
| | **Total** | **~34 steps, ~143 s (m)** | 0 dead ends | Fastest overall; its on-screen measurement labels are the standout |

## Summary

| Program | Total time (round 1) | Dead ends | After Q234 fixes |
|---------|---------------------|-----------|------------------|
| **CADaddy** | ~194 s (m) | 4 (invisible annotations, hidden drawing, confusing printer, empty preview) | ~78 s (est.) — 3 dead ends removed by hints, framing, clearer labels |
| **LibreCAD** | ~284 s (m) | 2 (console-only measurements, manual dimension setup) | — |
| **QCAD** | ~143 s (m) | 0 | — |

QCAD is faster than CADaddy's measured pre-fix time. CADaddy's estimated post-fix time
(~78 s) is faster, but it has not been re-measured; the compare page must not claim a
first-run lead until a measured run confirms it. The remaining friction (U14) is:
DIST/AREA results only in the history, tiny text in the MTEXT editor while typing, an
empty Plot preview, and decimal dimension results in a feet-and-inches drawing.

## Methodology notes

- **Measured totals** come from the ASUS builder's run (Q234, October 2026), which timed
  all three programs on the same Windows machine with the same floor plan.
- **Per-task estimates** are reconstructed from the measured totals, the dead-end
  analysis in [gaps.md](gaps.md) (U14), the code fixes, and black-box knowledge of each
  program's workflows. Step counts follow Q282's convention (~4 s/step for a new user,
  with dead-end confusion adding time).
- **After-fix estimate** (~78 s): the four fixed dead ends together accounted for most of
  the excess time; each fix saves the confusion and retrying, not the steps themselves.
  A re-measurement on the same machine and script would replace this estimate.
- Round 2 (Q282) added six more tasks (floor plan drawing, blocks with attributes,
  layout + plot, xref attach, hatch, table) and fixed the table dead end, xref attach
  friction, and attribute definition hints.
