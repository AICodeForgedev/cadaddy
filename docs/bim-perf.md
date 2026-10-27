# The assembled BIM pipeline: end-to-end performance

`cargo xtask perf` runs `cadaddy-mepfix`'s `bimbench` example. It takes the synthetic hotel's print set (a made-up four-storey building, generated vector PDF and DXF sheets) through every stage a person or an agent takes it through, as engine commands, print readers and MCP tools. It builds the whole job as one model (every level, every discipline) and runs the downstream stages on that model. Each stage's time, the run's peak memory, the model's element counts, the `.cdy` and IFC sizes, the agent tools' answer sizes and the largest command answer are checked against `perf/budget.json` (budget = today's number plus 25 %; more than twice the budget fails).

Every stage is also checked for working, not only timed. A stage fails the run (bimbench exits non-zero, so `cargo xtask perf` fails) when:
- one of its commands errors;
- its key result is empty where the hotel has items (no rooms read, no columns, no takeoff rows, no walls in the IFC, a reopened model with fewer objects than were saved…);
- the revision doesn't report exactly one changed sheet;
- an agent tool's answer is over its Q319 cap (6,000 characters for the briefing, 16,000 for the rest);
- the double hotel's model has under 1.8× the full hotel's elements.

The run prints a `FAILED` line for each.

- `cargo xtask perf --bim` runs only the pipeline, on the full hotel (10 rooms a side per floor).
- `--full` also runs the double hotel (20 rooms a side).
- `--bim --update-budget` re-baselines only the `bim` keys on this platform (keys the pipeline no longer emits are dropped).
- `cargo run -p cadaddy-mepfix --example bimbench -- --sizes full,double --detail FILE` writes every command's time and answer size.

Numbers below: Linux (x86-64 laptop, 12 threads), dev profile as CI builds it, milliseconds, from one `cargo xtask perf --bim --full` run. Synthetic project only.

## Stages

| Stage | What it does | Full | Double |
|---|---|---:|---:|
| import | print-set project from the folder, sheets classified | 43 | 20 |
| pdf_to_cdy | every plan PDF imported and saved as `.cdy` (incl. classify) | 1,981 | 5,615 |
| classify | `plan.classify` on the architectural plans | 945 | 4,417 |
| open_dxf | all 49 DXF sheets opened | 146 | 200 |
| read_sheets | DXFs parsed, sheets recognised and aligned | 248 | 322 |
| read_ceilings / read_walls / read_arch / read_assemblies | architectural readers | 165 | 299 |
| read_struct | structural reader | 0.3 | 0.4 |
| read_ducts / read_pipes / read_sprinklers | MEP readers | 112 | 182 |
| read_devices | electrical reader | 22 | 42 |
| read_callouts | detail callouts resolved across the set | 107 | 162 |
| compose | each level's A, S, M, P, FP and E plans inserted side by side into one drawing | 380 | 558 |
| spec | `bspec.draft`, then the levels and their plan regions, the trades' systems, the columns, electrical devices and sprinkler heads the readers found, and the room schedule as prints | 85 | 186 |
| mep_read | `mep.read {apply: true}`: duct and pipe runs off every level's plans | 132 | 441 |
| build | `bim.build {discipline: all}`: every level | 46 | 86 |
| rooms_build | `rooms.build`: a volume per room | 19 | 31 |
| validate | `bspec.validate` + `model.issues` | 31 | 59 |
| validate_full | `bim.validate`: the whole model against the prints | 4 | 7 |
| edit_validate | an agent's spec edit, then the incremental check (`bim.validate` on the edited objects, `model.issues`) | 39 | 69 |
| clashes / review / briefing | `model.clashes`, `review.list` + `rfi.list`, `project.briefing` | 14 | 23 |
| takeoff | `takeoff` | 5 | 6 |
| export_ifc / export_bcf | IFC4 and BCF 2.1 | 15 | 22 |
| render | `model.render` 800×600 | 176 | 187 |
| save_cdy / open_cdy | the whole model | 158 | 217 |
| spec_intake | the hotel's spec book into types (`bim.spec_import`, the MCP `bim_spec_import` path), accepted, a submittal per section (`procure.submittal`, `procure.submittals`) | 80 | 116 |
| viewer_open | the 3D scene Walk mode derives when it opens the model | 2.5 | 4.5 |
| viewer_orbit | 8 frames orbiting the model (software renderer, 640×480) | 138 | 166 |
| revision | one sheet changed, set re-imported: exactly one sheet reported changed | 13 | 17 |
| **all stages** (without classify, which is inside pdf_to_cdy, and the soak) | | **≈ 4,160** | **≈ 9,040** |
| soak | 200 agent cycles: edit → `bspec.validate` → `model.issues` | 6,929 | — |

### The model

| Elements | Full | Double |
|---|---:|---:|
| architecture (walls, doors, ceilings, floors) | 282 | 537 |
| structure (columns, floor to floor) | 228 | 388 |
| MEP (duct and pipe runs) | 70 | 114 |
| electrical (devices in the model) | 514 | 958 |
| rooms (volumes) | 69 | 129 |
| **total** | **1,163** | **2,126 (1.83×)** |

### The agent tools through MCP

One call of each tool through the MCP server (`cadaddy_mcp::Server` on a headless session holding the built model), as `mcp_<tool>_ms` and `mcp_<tool>_kb`:

| Tool (call) | Full ms | Double ms | Answer KB | Cap (characters) |
|---|---:|---:|---:|---:|
| `execute` (`model.issues`, 100 a page) | 25 | 52 | 0.2 | 16,000 |
| `bim_job` (`briefing`) | 5 | 9 | 1.4 | 6,000 |
| `project` (`info`) | 0.5 | 0.7 | 0.3 | 16,000 |
| `mep` (`list`) | 1.1 | 1.6 | 5.7 | 16,000 |
| `rooms` (`list`) | 14 | 24 | 14.5 | 16,000 |

The `rooms` tool's list was 26 KB on the full hotel (every room's outline), over the cap. It now leaves the outlines out unless asked (`outline: true`) and comes a page at a time (`limit`, default 50; `offset`; `next`). `count` is still every room.

### Other measures

- **Peak memory:** 188 MB (full), 281 MB (double).
- **Leaks:** `leak_mb` = 0.0 MB over the 190 soak cycles after warm-up. The soak runs with undo off. With undo on, the same session grew 26 MB, about 140 KB per spec edit. That is the undo history keeping the spec before each edit, as designed, and it is bounded at 2,000 steps or 768 MB.
- **Sizes:** `.cdy` of the plan sheets 2.5 MB; the whole model's IFC 922 KB (full), 1,648 KB (double).
- **CPU:** about 1.2 cores busy (the pipeline is sequential).
- **Largest command answer:** 106 KB (full) and 182 KB (double), from `bspec.set`, which answers with the whole spec. The agent tools above stay within their caps.

### The viewer

The 3D viewer (Walk mode, the IFC viewer) draws with the GPU, and it has no headless path here. The bench times two things instead:
- what the viewer does on opening: deriving its scene from the model (`cadaddy_walk3d::derive`, 632 triangles on the full hotel);
- eight orbit frames from the software renderer (`model.render`), about 17 ms a frame on the full hotel.

A GPU frame is cheaper than a software frame, so this is an upper bound on the viewer's frame time, not a measurement of it.

## What was fixed (Q324)

1. **`plan.classify` rooms: 1,421 → 439 ms on the full hotel (−69 %).** Each room name searched for its closed outline by outlining every object in the drawing again, for every doubling of the search window. A plan with 60 names did about 700 full outline passes.
   - Fix: `hatch::Outlines` outlines the drawing once per command, and each window is a filter over it.
   - It also remembers the boundaries found: a room's number lies in the same area as its name, so it reuses the result.
   - Same answers: `outlines_made_once_find_the_same_loops`. Every `plan.classify` and `model.build` answer on both hotels was compared with and without the cache: identical.
   - Used by `plan.classify`, `model.build` (room tracing), `bspec.draft`, `rcp.read` and `plan.rooms`.
2. **`plan.classify` walls: quadratic → linear (439 → 266 ms full, 2,146 → 390 ms double, on macOS).** Each wall face was tested against every wall pair (faces × pairs: 12,600 × 3,200 on a double guest floor).
   - Fix: a grid of the pairs' reach boxes gives each face only the pairs that can pass the same test.
   - Same answers: `the_pair_grid_misses_nothing`, plus the full-answer comparison above.
3. **`model.issues` pages its answer** (`limit`, default 100, at most 1,000; `cursor`/`next`). `total` and `byKind` still count everything. A big model's issue list no longer floods an agent's context (40 KB → 7 KB on the double hotel). The Plans to Model palette asks for 1,000.
4. **The double hotel's plans fit their sheets** (1/16″ or 1/32″ when no usual scale fits). Before, the plan ran off the sheet into the title block, and the readers saw half the building.

## Findings not fixed here

- **`plan.classify` on the double hotel regressed: about 4.4 s, against 1.3 s when Q324 landed (full hotel unchanged at about 0.9 s).** It now grows about 5× for twice the building. `perf/budget.json` keeps the Q324 budget for `bim_classify@double`, so `cargo xtask perf --full` fails on it until it is fixed. Suspects on `next` since then: the rooms3 change to how autospec traces what bounds rooms, and the hotel generator's later additions.
- **Electrical in the model is data, not geometry:** the hotel's device symbols are drawn as lines and circles, not blocks, so the 514 devices enter the model as the spec's devices (`mep.devices`), not as placed 3D objects. Conduit and tray runs on the E sheets aren't built either (`mep.read` reads duct and pipe runs only there).
- **Readers built for the office fixture:** the door and wall-type readers find nothing on the hotel prints (0 doors, 0 wall types), and 80 of 92 callouts don't resolve. Their stages pass on what else they read (windows, stairs, rooms, ceiling assemblies), but those per-item gaps are accuracy work for the readers.
- **Single-threaded:** about 1.2 cores busy. The per-sheet stages (PDF import, classify, the readers) are independent per sheet and could run in parallel, for about 3–4× on a typical 8-core machine.
- **`bspec.set` answers with the whole spec** (106 KB on the full hotel): an agent editing one value reads all of it back.
- **macOS budgets for the new keys are estimates:** the Linux numbers scaled by the existing BIM keys' macOS/Linux ratio (0.28), with sizes and counts copied. Run `cargo xtask perf --bim --full --update-budget` once on a Mac to measure them.
- **Windows has no budget yet:** `cargo xtask perf --bim --full --update-budget` needs to run once on a Windows machine.

Re-run this check after each later BIM round: `cargo xtask perf --full`.
