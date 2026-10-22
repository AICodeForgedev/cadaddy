# Rates from public projects

Public projects publish both their bid documents and their results. CADaddy builds a project's quantities with the BIM pipeline, then fits unit rates to the published totals. Everything runs **locally**.

**What leaves the machine:** collected plans, specs and project names are never committed, redistributed or published. Only fitted numbers leave, and only non-electrical ones may become public.

## Projects
`calibrate.project {pack: .jsonl, id, building_type, region, bid_date, delivery, quantities | from_wholejob, totals?, total?, hours?}` adds one project to a local pack.
- `id` is anonymous, never the project's name.
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

It also reports the noise level (the relative residual) and the held-out projects' prediction error (every n-th project is held out and predicted by a fit without it).

`out` appends the rates to a pack. **Electrical rates (divisions 26–28) are written only with `private: true`.** Otherwise they are withheld and listed. With `target: "hours"` the rates are field hours per unit.

## Certified payrolls
`calibrate.payroll {rows | path, map?}` sums certified-payroll hours by trade, using the classification ("Electrician", "Sheet Metal Worker", …) or your `map`. Unmapped classifications are listed.

Pair a project's payroll hours with its BIM quantities (`hours` in `calibrate.project`). That gives actual production rates, the strongest public labor source. Electrical results go only to the private pack.

## Top-down estimate
`calibrate.topdown {values, ratios, wage_escalation?, materials_escalation?, bottom_up_hours?, gap_pct?: 25}` estimates field hours and wages per trade from census construction ratios:
- the self-performed share of the value of work;
- field hours per dollar of self-performed work;
- the wage per hour.

It escalates from the census year to the bid date by wage growth and a materials factor, weighted by the wage share. Beside the bottom-up hours, it flags gaps over `gap_pct`. Census averages run high for lean commercial work, so treat this as a check, not the estimate.

## Collecting projects
Collection is an agent's job, with these rules:
- Start with sources that have APIs or structured listings: federal contract opportunities and award data, state and large local procurement portals, universities, transit agencies.
- Respect each portal's terms, robots rules and rate limits.
- Plan-room registrations, portal accounts and public-records requests are outward-facing. They happen only on the owner's go, one agency at a time, and are logged.
- Keep the per-project build deterministic (rule build only), so thousands of projects cost compute, not agent tokens.
