# CADaddy's open estimating baseline format

`estimate.import {folder}` reads a company's estimating baseline from a folder of plain files. The format is open: any tool can write it. CADaddy ships **no** prices or cost data, and its tests use invented values only.

| File | Columns / keys | Meaning |
|---|---|---|
| `item_labor_units.csv` | `item, unit, material, labor_normal, labor_difficult, labor_very_difficult` | Per item: material cost per unit, labor hours per unit at three difficulty tiers. |
| `assemblies.csv` | `assembly, unit, description, labor_hours` | Installed assemblies ("duplex receptacle, metal stud, box and ring"): labor hours per unit. |
| `assembly_parts.csv` | `assembly, item, qty` | The parts of each assembly, quantity per assembly unit (priced from `prices.csv`). |
| `prices.csv` | `item, size?, unit, material, labor_hours? \| labor_dollars?, date, source` | Dated prices. Labor may be **dollars** (as some cost databases store it): hours = dollars ÷ `crew_rate`. |
| `rates.json` | `{"crew_rate": 85.0, …}` | Crew rate (dollars per labor hour) and other rates. |
| `benchmarks.json` | `{"ratios": [{"match", "per_level_low", "per_level_high", "source"}], "per_unit_type": [{"unit_type", "match", "low", "high", "source"}], "hours_per_sf": {"low", "high", "source"}, "unit_prices": [{"item", "size?", "material", "source"}], "price_tolerance_pct": 25}` | Typical quantities and prices from past jobs, for the validator: quantities per level and per typical unit (the prints' rooms with a `unit_type`), labor hours per square foot (`estimate.settings {gross_sf}`), and unit prices a new price may not stray from by more than the tolerance. |
| `factors.csv` | `factor, kind, multiplier, basis, source, trade?, category?, item?` | Condition factors on labor. `kind` is height, ceiling, exposure, wall, floor, learning_curve or other; `basis` is **observed** or **assumption** (a row without one is refused). A row with trade/category/item applies to the lines it matches; one without is applied by `estimate.set_factor`. |
| `production_model.json` | `{"phases": [{"phase", "drivers": [{"driver", "unit", "rate", "prior_qty?", "context?", "match": {"trade?", "category?", "item?"}}]}]}` | Driver-based labor: hours per unit of a driver (boxes, devices, feet of feeder) by phase, in a context (`estimate.settings {context}`). `prior_qty` (10 by default) is how many units the book rate weighs against observations. |
| `field_observations.csv` | `driver, context?, qty, hours, date, source` | What the field took: blended with the book rate, weighted by quantity. `estimate.observe` adds more. |
| `indexes.csv` | `index, date, value` | Price indexes. A price with an `index` column escalates to the estimate's `today` by the index's latest value on or before `today` ÷ its latest value on or before the price's date. |
| `tool_rules.json` | `{"rules": [{"when", "min?", "items": [{"item", "supply": "purchase\|rental\|owned", "category?", "price?\|rate?", "per?": "each\|job\|day\|week\|month", "qty?\|qty_per?"}]}]}` | Job equipment, tools and setup the work implies. `when` is a fact of the job: `large_terminations` (#4 AWG and larger, and kcmil, phase conductors × 2 ends), `large_pull_ft`, `wire_ft`, `high_work_ft` (runs at `high_work_ft`, 14 ft by default, and up), `floors`, `peak_crew`, `duration_days`, `labor_hours` or `always`; the rule fires when the fact is above 0 (or at least `min`). Quantity is `qty`, or `qty_per` × the fact rounded up. Owned tools carry a tool charge and rentals a rate, each × the duration (`estimate.settings {duration_days}`; weeks and months rounded up); purchases a price × quantity. |
| `price_history.csv` | `item, size?, unit, material, date, source` | Earlier prices. A price that a new `estimate.set_price` replaces joins the history too. |

## How the estimate uses it
- **Quantities** come from the model's bill of materials (`bim.bom`).
- **Pricing:** each line is priced through its mapped assembly (parts × part prices + assembly labor) or through its own price row (by item, then item and size). Material is taken on the quantity including waste; labor on the net quantity, since waste isn't installed.
- **Units:** a price in another unit than the counted line (`C` per hundred vs `each`, `C-ft` vs `ft`) is refused, not converted silently.
- **Stale prices:** prices older than `stale_days` (365 by default) before the estimate's `today` are flagged.
- **Scope:** every line has one (`in_contract`, `by_others`, `nic`, `ofci`, `alternate:<n>`), set by `estimate.set_scope` with a reason. Undecided lines reach no total. Alternates are priced apart.
- **Markups:** tax (on material), overhead, profit, contingency and bond come from `estimate.settings` and compound in that order.
- **Traceability:** `estimate.explain` traces every number to its math, its BOM line and the model elements behind it.

## Labor
- **Tier:** book labor is at the job's tier (`estimate.settings {tier}`) or a line's (`estimate.set_factor {tier}`): the item's per-tier hours in `item_labor_units.csv`, else the book rate × `rates.json` `tier_multipliers` (difficult 1.4 and very difficult 1.8 by default).
- **Production model:** a line a driver matches takes its labor from the drivers instead: Σ of their blended rates, rate = (book × prior_qty + observed hours) ÷ (prior_qty + observed quantity). Its hours roll up by phase (`estimate.status` `labor_by_phase`; lines priced from book rates are `unphased`).
- **Factors** multiply either. `estimate.explain` lists each step: driver × rate, tier, factor.

## Job equipment
The BOM's required items (`bim.bom` `required`) and the tool rules become `job|…` lines, in contract unless their scope is set otherwise. A required item no tool rule or price prices, or work that implies a crimper, torque wrench and lugs (large terminations), a tugger (large pulls), a lift (high work), ladders and gang boxes (floors) with no priced line for it, is a `missing_equipment` finding.

## Validator
Besides the line checks: `price_off_history`, `no_index`, `missing_duration`, `missing_equipment`, `spec_unmatched` (the BOM's spec check), `model_validation_open` (the print validator's failures, when the drawing has prints), `off_ratio`, `off_ratio_unit` and `off_hours_per_sf`. Each finding has a fix.

## Outputs
- `estimate.status`: `by_level`, `by_trade` and `by_system` (the BOM category); each sums to the total to the cent (each line's share = its cost plus tax on its material, times the compounded markups; job equipment counts under level `job`).
- `estimate.export {path, format?, columns?}`: every line, its share of the total in `total`, and a last `TOTAL` row; CSV or XLSX.
- `estimate.bid_summary`: base bid by trade, alternates, and assumptions (rule quantities, assumed factors, tier, production model, escalation, duration), clarifications (owner-furnished items, open findings) and exclusions (not in contract, by others) drafted from the scope decisions, as data and Markdown.
- The project briefing (`project.briefing`, `bim_job briefing`) carries `estimate: {total, ok, open_findings}` once there is an estimate.
