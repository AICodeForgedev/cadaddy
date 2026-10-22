# CADaddy's open estimating baseline format

`estimate.import {folder}` reads a company's estimating baseline from a folder of plain files. The format is open: any tool can write it. CADaddy ships **no** prices or cost data, and its tests use invented values only.

| File | Columns / keys | Meaning |
|---|---|---|
| `item_labor_units.csv` | `item, unit, material, labor_normal, labor_difficult, labor_very_difficult` | Per item: material cost per unit, labor hours per unit at three difficulty tiers. |
| `assemblies.csv` | `assembly, unit, description, labor_hours` | Installed assemblies ("duplex receptacle, metal stud, box and ring"): labor hours per unit. |
| `assembly_parts.csv` | `assembly, item, qty` | The parts of each assembly, quantity per assembly unit (priced from `prices.csv`). |
| `prices.csv` | `item, size?, unit, material, labor_hours? \| labor_dollars?, date, source` | Dated prices. Labor may be **dollars** (as some cost databases store it): hours = dollars ÷ `crew_rate`. |
| `rates.json` | `{"crew_rate": 85.0, …}` | Crew rate (dollars per labor hour) and other rates. |
| `benchmarks.json` | `{"ratios": [{"match", "per_level_low", "per_level_high", "source"}]}` | Typical quantities from past jobs, for the estimate validator's ratio checks. |

## How the estimate uses it
- **Quantities** come from the model's bill of materials (`bim.bom`).
- **Pricing:** each line is priced through its mapped assembly (parts × part prices + assembly labor) or through its own price row (by item, then item and size). Material is taken on the quantity including waste; labor on the net quantity, since waste isn't installed.
- **Units:** a price in another unit than the counted line (`C` per hundred vs `each`, `C-ft` vs `ft`) is refused, not converted silently.
- **Stale prices:** prices older than `stale_days` (365 by default) before the estimate's `today` are flagged.
- **Scope:** every line has one (`in_contract`, `by_others`, `nic`, `ofci`, `alternate:<n>`), set by `estimate.set_scope` with a reason. Undecided lines reach no total. Alternates are priced apart.
- **Markups:** tax (on material), overhead, profit, contingency and bond come from `estimate.settings` and compound in that order.
- **Traceability:** `estimate.explain` traces every number to its math, its BOM line and the model elements behind it.

## Not yet read
`indexes.csv` (escalation by price index), `field_observations.csv` and `production_model.json` (driver-based labor) are part of the planned format; this version doesn't read them.
