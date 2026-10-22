# Procurement from the model

Material is released from the bill of materials (`bim.bom`): floor by floor, area by area or trade
by trade, each release the exact need of what the BOM still needs there plus spares by rule. Every
release takes from what is left, so **the releases always sum to the BOM** (`procure.list` says so,
and lists what no release has taken yet).

| Command | What it does |
|---|---|
| `procure.release {name, level?, trades?, categories?, need_by?, allow_unlinked?}` | A release: exact need + spares (`custom/spares.<category>` percent; defaults: devices 2%, lighting 1%) |
| `procure.list {today?}` | The releases, each line's lead time, need date, order-by date, submittal status, order status and flags; what's unreleased; whether the releases sum to the BOM |
| `procure.status {release, item, status, received?}` | Ordered, shipped, received, short |
| `procure.submittal {item, spec_section?, status?, submitted?, returned?, reviewer?, comment?}`, `procure.submittals` | The submittal log |
| `procure.leadtimes {set?}` | Lead times per item class (the company's data over the defaults) |
| `procure.export {release, path}` | A release as XLSX, CSV or PDF |

## Order-by dates

Order by = the day the material is needed − the item class's longest lead time − the submittal
review allowance while its submittal isn't approved (`custom/procure.review_days`, default 14).
The day it is needed is the release's `need_by`, else the 4D schedule's first task on its floor
(`fourd.import`). A line still planned past its order-by date is flagged **late**; gear, panels,
transformers, lighting, equipment and lug kits without an approved submittal are flagged too.

Default lead times (weeks; placeholders for the company's own data): gear 30–60, transformers
20–40, panelboards 8–16, disconnects 4–8, lug kits 4–8, equipment 12–24, lighting 6–12,
devices 1–2, wire 1–3, conduit 1–2, ductwork 2–4, piping 1–3, drywall 1–2, other 2–4.

## Items that arrive together

Lugs and bolt kits with the gear they land on (the BOM's lug kit lines from `gear.check`), covers
and trims with panels, breakers with load centers: a release that takes some of a group without
the rest is refused, naming what would ship without what; with `allow_unlinked` it is made and
flagged.

Tests: `crates/engine/tests/procure.rs`.
