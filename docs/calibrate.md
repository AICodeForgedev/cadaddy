# Rates from public projects

Public projects publish both their bid documents and their results. CADaddy builds a project's quantities with the BIM pipeline, then fits unit rates to the published totals. Everything runs **locally**.

**What leaves the machine:** collected plans, specs and project names are never committed, redistributed or published. Only fitted numbers leave, and only non-electrical ones may become public.

## Projects
`calibrate.project {pack: .jsonl, id, building_type, region, bid_date, delivery, quantities | from_wholejob, totals?, total?, hours?}` adds one project to a local pack.
- `id` is anonymous, never the project's name. Adding an id that is already in the pack replaces that project's row, so a retry never counts one project twice.
- Quantities are `{trade: {driver: qty}}`. `from_wholejob: true` takes them from the drawing's whole-job estimate (`wholejob.quantities`), with one driver per item and unit.
- The result can be trade totals (construction-manager-at-risk trade bids), the project total, or field hours by trade from certified payrolls (`calibrate.payroll`).

## The fit
`calibrate.fit {packs, target?: cost|hours, building_type?, region?, priors?, markups?, regions?, index?, base_date?, min_projects?: 3, holdout_every?: 5, out?, private?}`

It fits **observed total ≈ Σ quantity × rate × region factor × index(bid date)/index(base date) × (1 + trade markup)**:
- per trade where the result names one, otherwise per project;
- as a Bayesian ridge regression on relative error;
- with priors `{"trade/driver": {rate, sd}}` from the public pack or the company's history, and a weak prior otherwise.

For each rate it reports:
- the rate and its standard deviation;
- the number of projects behind it;
- a quality: `fitted` (sd ≤ 25 % of the rate), `weak`, or **`insufficient data`** (fewer than `min_projects`; no number is given).

Each id counts once: if the packs hold the same id more than once, the last row is used (pack order, then line order) and `warnings` lists the id.

It also reports the noise level (the relative residual) and the held-out projects' prediction error (every n-th project is held out and predicted by a fit without it).

`out` appends the rates to a pack. **Electrical rates (divisions 26–28) are written only with `private: true`.** Otherwise they are withheld and listed. With `target: "hours"` the rates are field hours per unit.

## Certified payrolls
`calibrate.payroll {rows | path, map?}` sums certified-payroll hours by trade, using the classification ("Electrician", "Sheet Metal Worker", …) or your `map`. Unmapped classifications are listed. "Cement Mason" and "Concrete Finisher" are concrete, not masonry.

From a CSV, a row's hours are:
- the one hours column, or the "Total Hours" column when there is one;
- else straight time + overtime (`ST Hours` + `OT Hours`, double time included), or the per-day columns (Mon … Sun, Day 1 …) added up;
- anything else is refused with the columns named. Add a total column or send `rows`.

`hours_columns` in the answer names the columns used.

Pair a project's payroll hours with its BIM quantities (`hours` in `calibrate.project`). That gives actual production rates, the strongest public labor source. Electrical results go only to the private pack.

## Top-down estimate
`calibrate.topdown {values, census | ratios, wage_escalation?, materials_escalation?, bottom_up_hours?, gap_pct?: 25}` estimates field hours and wages per trade from the census construction figures. Give each trade's raw figures in `census` (`value_of_work`, `subcontracted_out`, `construction_worker_hours`, `construction_worker_wages`) and CADaddy works out the ratios:
- the self-performed share of the value of work: (value of work − subcontracted out) / value of work;
- field hours per dollar of self-performed work: worker hours / self-performed value;
- the wage per hour: worker wages / worker hours.

Ratios worked out elsewhere are still accepted in `ratios` (`self_performed_share`, `hours_per_dollar`, `wage_per_hour`). Each row's `ratios` shows the ones used. Zero or negative figures are refused.

It escalates from the census year to the bid date by wage growth and a materials factor, weighted by the wage share. Beside the bottom-up hours, it flags gaps over `gap_pct`. Census averages run high for lean commercial work, so treat this as a check, not the estimate.

## Collecting projects
Collection is an agent's job, with these rules:
- Start with sources that have APIs or structured listings: federal contract opportunities and award data, state and large local procurement portals, universities, transit agencies.
- Respect each portal's terms, robots rules and rate limits.
- Plan-room registrations, portal accounts and public-records requests are outward-facing. They happen only on the owner's go, one agency at a time, and are logged.
- Keep the per-project build deterministic (rule build only), so thousands of projects cost compute, not agent tokens.
