# The assembled BIM pipeline: end-to-end performance

`cargo xtask perf` runs `cadaddy-mepfix`'s `bimbench` example. It takes the synthetic hotel's print set (a made-up four-storey building, generated vector PDF and DXF sheets) through every stage a person or an agent takes it through, as engine commands and print readers. Each stage's time, the run's peak memory, the `.cdy` and IFC sizes, and the largest command answer an agent reads are checked against `perf/budget.json` (budget = today's number plus 25 %; more than twice the budget fails).

- `cargo xtask perf --bim` runs only the pipeline, on the full hotel (10 rooms a side per floor).
- `--full` also runs the double hotel (20 rooms a side).
- `--bim --update-budget` re-baselines only the `bim` keys on this platform.
- `cargo run -p cadaddy-mepfix --example bimbench -- --sizes full,double --detail FILE` writes every command's time and answer size.

Numbers below: macOS (Apple silicon), dev profile as CI builds it, milliseconds. Synthetic project only.

## Stages

| Stage | What it does | Full before | Full after | Double after |
|---|---|---:|---:|---:|
| import | print-set project from the folder, sheets classified | 12 | 13 | 4 |
| pdf_to_cdy | every plan PDF imported and saved as `.cdy` (incl. classify) | 1,682 | 582 | 772 |
| classify | `plan.classify` on the architectural plans | 1,347 | 220 | 342 |
| open_dxf | all 46 DXF sheets opened | 70 | 74 | 100 |
| read_sheets | DXFs parsed, sheets recognised and aligned | 127 | 142 | 195 |
| read_ceilings / read_walls / read_arch / read_assemblies | architectural readers | 94 | 105 | 138 |
| read_struct | structural reader | 0.1 | 0.1 | 0.2 |
| read_ducts / read_pipes / read_sprinklers | MEP readers | 65 | 73 | 99 |
| read_devices | electrical reader | 14 | 15 | 23 |
| read_callouts | detail callouts resolved across the set | 62 | 70 | 91 |
| build_arch | plan → spec → `model.build` | 134 | 92 | 103 |
| validate | `bspec.validate` + `model.issues` | 28 | 30 | 36 |
| edit_validate | a spec edit, then validate again | 26 | 29 | 37 |
| clashes / review / takeoff | `model.clashes`, `rfi.list`, `takeoff` | 1 | 1 | 1 |
| export_ifc / export_bcf | IFC4 and BCF 2.1 | 2 | 2 | 2 |
| render | `model.render` 800×600 | 60 | 61 | 46 |
| save_cdy / open_cdy | the built model | 13 | 14 | 12 |
| revision | one sheet changed, set re-imported: only it is redone | 3 | 3 | 4 |
| **all stages** | | **≈ 2,600** | **≈ 1,300** | **≈ 1,660** |

Other measures:
- **Peak memory:** 136 MB (full), 181 MB (double).
- **Built model:** `.cdy` 2.5 MB of plan sheets; IFC 196 KB.
- **CPU:** about 1.2 cores busy (the pipeline is sequential).
- **Revision:** re-importing after one changed sheet reports exactly one sheet changed.
- **Largest command answer:** 6–7 KB (about 1,700 tokens), now that `model.issues` pages.

The double hotel takes 1.3× the full hotel's time for twice the building: no stage grows faster than the building now.

## What was fixed

1. **`plan.classify` rooms: 1,421 → 439 ms on the full hotel (−69 %).** Each room name searched for its closed outline by outlining every object in the drawing again, for every doubling of the search window. A plan with 60 names did about 700 full outline passes.
   - Fix: `hatch::Outlines` outlines the drawing once per command, and each window is a filter over it.
   - It also remembers the boundaries found: a room's number lies in the same area as its name, so it reuses the result.
   - Same answers: `outlines_made_once_find_the_same_loops`. Every `plan.classify` and `model.build` answer on both hotels was compared with and without the cache: identical.
   - Used by `plan.classify`, `model.build` (room tracing), `bspec.draft`, `rcp.read` and `plan.rooms`.
2. **`plan.classify` walls: quadratic → linear (439 → 266 ms full, 2,146 → 390 ms double).** Each wall face was tested against every wall pair (faces × pairs: 12,600 × 3,200 on a double guest floor).
   - Fix: a grid of the pairs' reach boxes gives each face only the pairs that can pass the same test.
   - Same answers: `the_pair_grid_misses_nothing`, plus the full-answer comparison above.
3. **`model.issues` pages its answer** (`limit`, default 100, at most 1,000; `cursor`/`next`). `total` and `byKind` still count everything. A big model's issue list no longer floods an agent's context (40 KB → 7 KB on the double hotel). The Plans to Model palette asks for 1,000.
4. **The double hotel's plans fit their sheets** (1/16″ or 1/32″ when no usual scale fits). Before, the plan ran off the sheet into the title block, and the readers saw half the building.

## Findings not fixed here

- **Readers built for the office fixture:** the wall, door and wall-type readers find nothing on the hotel prints (0 walls, 0 doors, 0 wall types), and 80 of 92 callouts don't resolve. They read the office fixture's conventions; the hotel's need adding.
- **Single-threaded:** about 1.2 cores busy. The per-sheet stages (PDF import, classify, the readers) are independent per sheet and could run in parallel, for about 3–4× on a typical 8-core machine.
- **Room search:** what remains of `plan.classify` is mostly the region search itself on the largest windows (a corridor spans the whole floor). Further gains there are in `cadaddy_geom::region`.
- **`plan.classify` room names** include title-block and legend text that happens to sit in a closed box (the project name, LEGEND, GYP notes).
- **Windows has no budget yet:** `cargo xtask perf --bim --full --update-budget` needs to run once on a Windows machine.

Re-run this check after each later BIM round: `cargo xtask perf --full`.
