# Whole-job estimate

`wholejob.*` puts every trade's quantities into one estimate organised by CSI division. It covers bid packages for subs, general conditions, bid leveling and a bid history. No prices ship with CADaddy: the company's own packs, a public pack, or assumed values marked as such.

## Quantities and scope
- `wholejob.quantities {trade, division?, lines: [{item, unit, qty, spec_section?, source?}]}` sets a trade's takeoff. The envelope, finishes and sitework readers produce these lines. `{from_bom: true}` takes the MEP trades from `bim.bom`.
- `wholejob.scope {trade, inclusions?, exclusions?, alternates?}` sets the scope of work from the specs.
- The division comes from the trade's name (roofing 07, painting 09, plumbing 22, electrical 26, earthwork 31, …) unless you give one.

## Prices and contract value
- `wholejob.prices {trade, quality, source, date?, rows: [{item, unit, unit_cost}]}`. The quality is `private` (the company's history), `public` (the public pack) or `assumed`. **Electrical work (divisions 26–28) takes private prices only.** The public pack carries no electrical labor.
- `wholejob.settings {markups?: {trade: %}, gc?: {...}, electrical_from_estimate?}`. Typical trade markups are data and can be edited. With `electrical_from_estimate`, the electrical trade's value is the electrical estimate's total (`estimate.status`).
- `wholejob.estimate` lists, per trade, the priced and unpriced lines, direct cost, markup, **contract value** and its **data quality**. It also gives totals by division, **general conditions** (supervision, temporary facilities, equipment and cleanup per week × the schedule's weeks, then insurance and bond percentages) and the total.

## Bid packages and leveling
- `wholejob.package {trade, path?: .xlsx|.csv}` builds the quantities, spec sections, inclusions, exclusions and alternates, ready to send.
- `wholejob.bid {trade, bidder, total, lines?, exclusions?, alternates?}` records a sub's bid. The agent extracts the lines from the bid's PDF or XLSX.
- `wholejob.level {trade}` compares each bid with the model's quantities and scope:
  - **missing scope** (not in the bid, or excluded);
  - **quantity gaps** (more than 10 %);
  - **outlier unit prices** (more than 35 % from the bidders' median);
  - a **leveled total** that prices the missing scope at our price or the other bidders' median.

  It **recommends the low COMPLETE bid**, and says so when no bid is complete.

## Bid history
- `wholejob.history {pack: .jsonl, private?, trade, building_type, region, date, sf, amount, source}` adds one public bid tabulation row ($/SF). **Electrical history from any source goes only to a private pack** (`private: true`).
- `wholejob.benchmarks {packs, building_type?, region?}` gives the median, minimum and maximum $/SF per trade.
- `wholejob.outcome {pack, private: true, trade, award?, final_cost?, change_orders?}` records outcomes. They feed only the company's private history.
