# Field progress and actual production rates

The foreman, the crew or an agent (through MCP) marks installed work against the model, and
timecard hours come in from CSV; CADaddy works out percent complete and earned value per phase and
floor, installed against planned quantities, flags phases running over their estimate early, and
computes the **actual production rate per driver**, which goes to the company's **private** field
observations — never into the drawing.

| Command | What it does |
|---|---|
| `progress.plan {drivers?, from_bom?, rates?}` | What is to be installed per floor and driver (from the bill of materials, or given) and the book rates |
| `progress.mark {phase, level?, fraction?, count?, driver?, elements?, date?, by?, note?}` | Installed work: a floor's phase complete ("level 3 rough complete"), a share of it, a count of one driver, or elements |
| `progress.hours {path \| rows}` | Timecards (date, name or crew, hours, phase or cost code, floor) |
| `progress.status {date?, crew_rate?}` | Per phase and floor: budget, earned and actual hours, percent complete, CPI, projected hours, earned value and actual cost at the crew rate (the estimate's when set), and the flags |
| `progress.rates {date?}` | The actual rate per driver and its ratio to the book rate |
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

## The numbers

- Installed = the larger of a floor's marked share of the phase and the counted units, up to the plan.
- Budget hours = planned × book rate; earned hours = installed × book rate; percent complete =
  earned ÷ budget; CPI = earned ÷ actual; projected = actual ÷ percent complete.
- A phase is **running over** when, at least `custom/progress.early_pct` (5%) along, its projected
  hours pass its budget by more than `custom/progress.over_pct` (10%).
- A phase's hours are shared across its drivers by their earned hours; the actual rate is that share
  over the units installed.
- Timecards without a floor count toward their phase (and its rates) but not toward a floor.

Tests: `crates/mepfix/tests/progress_hotel.rs` (the synthetic hotel, every number against a hand
calculation), `crates/engine/tests/progress.rs`.
