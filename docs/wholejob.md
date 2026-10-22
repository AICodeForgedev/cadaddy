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

  A bid line covers **at most one** model line. Names match on their words, ignoring case and punctuation: the same words first; otherwise one name's words inside the other's, but only when that picks out a single model line (and the shorter name has 3 or more letters). A bid line such as "Ceiling paint" that names several model lines ("CEILING PAINT (GYP)", "CEILING PAINT (ACT)") covers none of them: they are reported as `"why": "ambiguous — review"` and the bid isn't complete. Exclusions match by the same rule, so a one-letter exclusion excludes nothing and an exclusion naming several lines leaves them for review.

  It **recommends the low COMPLETE bid**, and says so when no bid is complete.

## Bid history
- `wholejob.history {pack: .jsonl, private?, trade, building_type, region, date, sf, amount, source}` adds one public bid tabulation row ($/SF). **Electrical history from any source goes only to a private pack** (`private: true`).
- A pack is **all public or all private**: its first row (or a header row `{"kind": "pack", "private": true}`) sets it. `wholejob.history` and `wholejob.outcome` refuse a private row in a public pack and a public row in a private pack, and leave the file unchanged.
- Electrical means division 26, 27 or 28, or a trade named electrical, low voltage, lighting, data, communications, security, access control, CCTV, fire alarm, nurse call or AV. These take no public prices and no public history.
- `wholejob.benchmarks {packs, building_type?, region?, private?}` gives the median, minimum and maximum $/SF per trade. Rows of a private pack, or rows marked `private: true`, are left out unless `private: true` is passed; `private_rows_left_out` counts them.
- `wholejob.outcome {pack, private: true, trade, award?, final_cost?, change_orders?}` records outcomes. They go only into a private pack.
