# Field progress and actual production rates

The foreman, the crew or an agent (through MCP) marks installed work against the model, and
timecard hours come in from CSV; CADaddy works out percent complete and earned value per phase and
floor, installed against planned quantities, flags phases running over their estimate early, and
computes the **actual production rate per driver**, which goes to the company's **private** field
observations — never into the drawing.

| Command | What it does |
|---|---|
| `progress.plan {drivers?, from_bom?, rates?}` | What is to be installed per floor and driver (from the bill of materials, or given) and the book rates |
| `progress.mark {phase, level?, fraction?, count?, driver?, elements?, date?, by?, note?}` | Installed work: a floor's phase complete ("level 3 rough complete"), a share of it, a count of one driver on one floor, or elements of the model |
| `progress.hours {path \| rows}` | Timecards (date, name or crew, hours, phase or cost code, driver, floor) |
| `progress.status {date?, crew_rate?}` | Per phase and floor: where the budget came from (`budget_source`), budget, earned and actual hours, percent complete, CPI, projected hours, earned value and actual cost at the crew rate (the estimate's when set), and the flags |
| `progress.rates {date?}` | The actual rate per driver, its ratio to the book rate, and whether it was `observed` or `apportioned` |
| `progress.observe {path, context?, date?}` | Appends the actual rates to the private `field_observations.csv` (building type, wiring method, working height as context) |

## Drivers and phases

| Driver | Phase | Unit | Book rate (h/unit, invented default) |
|---|---|---|---|
| boxes | rough | each | 0.25 |
| conduit_ft | rough | ft | 0.04 |
| terminations | makeup | each | 0.08 |
| devices | trim | each | 0.2 |
| luminaires | trim | each | 0.5 |
| panels | panel | each | 8 |
| devices_tested | test | each | 0.05 |

From the BOM: boxes (and three terminations a box, `custom/progress.conductors_per_box`),
conduit feet, devices, luminaires, panels and switchboards, per floor. The book rates are an
assumption until the company's production model gives its own (`progress.plan {rates}`).

## The budget: the estimate first

When the drawing has an estimate (`estimate.*`) whose in-contract lines carry labor hours, each
driver's budget on each floor is the estimate's hours for the BOM lines of that driver on that
floor: labor hours per unit × the line's net quantity there. A box line is the boxes driver's,
conduit the conduit feet's, a device the devices', a luminaire the luminaires', a panel or
switchboard the panels'. Terminations and devices tested have no estimate line of their own, and a
driver the estimate doesn't price falls back to planned × book rate. `progress.status` says which,
per driver, floor and phase: `budget_source` is `estimate`, `book_rate_assumption`, or `mixed`
(some drivers each way). The estimate's labor on lines no driver carries (wire, fittings…) is
reported as `estimate_hours_outside_drivers`, so the gap between the budgets and the bid is visible.
Earned hours are installed ÷ planned × the driver's budget, so percent complete and the overrun flag
measure against the bid.

## Marks

- A **fraction** with no floor applies to every floor; with a floor, to that floor.
- A **count** needs its driver (of the mark's phase) and its floor; without a floor it is refused,
  never dropped.
- **Elements** are resolved against the model (the BOM lines' elements: device and luminaire tags,
  `run:<n>` conduit runs, equipment tags). An unknown id is refused by name. The element's floor
  comes from the model (a `level` that disagrees is refused) and its driver from the phase (a
  receptacle is a box in rough, three terminations in makeup, a device in trim; a run is its
  feet); a `driver` that the element isn't is refused, and one is required when the element feeds
  two drivers in that phase. An element counts once however often it is marked
  (`new_elements` says how many were new).

## Timecards and the rates' basis

A timecard row may name its driver: a `driver` column (which must be one of the row's phase's
drivers, else the row is skipped with the reason), or a cost code that names one
(`EL-ROUGH-CONDUIT`, `Rough boxes`; a cost code naming a driver of another phase only gives the
phase). A row with no phase but a driver takes the driver's phase.

- A driver with hours coded to it: its rate is those hours over its installed units, `observed`.
- The phase's uncoded hours are shared across its drivers with no coded hours, by their earned
  hours at the book rates: `apportioned` (the drivers in one phase then share one ratio to the book
  rate, so these rates say nothing per driver). In a phase with a single driver (makeup, panel,
  test) they are that driver's own: `observed`.
- The basis goes into `progress.rates` and the `basis` column of `field_observations.csv`.

## The stored record

The progress record lives in the building spec. If it can't be read (damaged, or an older or newer
shape), every progress command fails with the reason and the record is left as it is; it is never
replaced by an empty one.

## The numbers

- Installed = the larger of a floor's marked share of the phase and the counted units (counts plus
  each marked element once), up to the plan.
- Budget hours = the estimate's hours, else planned × book rate; earned hours = installed ÷
  planned × budget; percent complete =
  earned ÷ budget; CPI = earned ÷ actual; projected = actual ÷ percent complete.
- A phase is **running over** when, at least `custom/progress.early_pct` (5%) along, its projected
  hours pass its budget by more than `custom/progress.over_pct` (10%).
- The actual rate is a driver's hours over its units installed (see the basis above).
- Timecards without a floor count toward their phase (and its rates) but not toward a floor.

Tests: `crates/mepfix/tests/progress_hotel.rs` (the synthetic hotel, every number against a hand
calculation), `crates/engine/tests/progress.rs`.
