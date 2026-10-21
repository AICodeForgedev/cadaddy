# MEP print fixtures (`crates/mepfix`)

Made-up print sets for scoring the "MEP from prints" readers (sheet recognition, ductwork, piping,
heights, 3D, clashes, equipment, revisions). Every set is a made-up two-storey office drawn the way
consulting engineers draw it, with the ground truth of everything on it. Nothing here comes from a
real project.

```
cargo xtask mep-fixtures [--seed N] [--count K] [--out DIR]   # default target/mep-fixtures/seed-N/
```

Each `seed-N/` holds one PDF and one DXF per sheet, `set.pdf` (every sheet, bookmarked) and
`truth.json`. The same seed always gives byte-identical files.

## Sheets

| Sheet | Shows |
|---|---|
| G-001 | cover: project, sheet index, general notes, level elevations |
| S-102, S-103 | level 2 and roof framing: grid, columns, girders and beams with sizes, T.O.S. and slab notes |
| A-121, A-122 | reflected ceiling plans: ACT grids, GYP, open to structure, ceiling tags, diffusers, grilles, heads |
| M-101, M-102 | mechanical: AHU, rectangular supply/return mains, round branches, VAV boxes, runouts, diffusers, return and exhaust grilles, OA louvre, duct risers, size tags and height notes |
| P-101, P-102 | plumbing: CW, HW, HWR, SAN (sloped, below floor shown hidden), V, gas; fixtures, water heater, gas meter, risers |
| P-401–P-403 | enlarged toilet rooms (both levels) and level 1 mechanical room at 1/4"–1/2" |
| FP-101, FP-102 | fire protection: riser, main (with reducers), branch lines, pendent heads |
| M-601, P-601 | schedules: AHU, VAV, exhaust fan, air devices; water heater, gas meter, fixtures |

Varied by seed: building size and bays, floor heights, structure, ceiling heights and types, room
names, airflows; abbreviations (`SA`/`S`/`SUP`…), size tag styles (`24x14`, `24"x14"`, `24/14`;
`12"Ø`, `12" DIA`, `12Ø`, `12" RD`), height note styles (`BOD @`, `B.O.D. … AFF`, `BOT EL`, `TOD @`,
`CL @`), leaders, centrelines, radius or mitred elbows, tag orientation, pens, paper (ARCH D, ANSI D,
ARCH E1), title block on the right or along the bottom, grid labelling, riser and head symbols, plan
scale, plans turned 90° on their sheet, drawings far from the origin, PDF layers on or off.

## Truth (`truth.json`, schema `cadaddy-mepfix/1`)

Coordinates are **building feet** (x east, y north, z above the level 1 floor); sizes are
**inches**. See `crates/mepfix/src/truth.rs` for every field.

- `building`: levels, grid lines, rooms (with ceilings), columns, beams (size, depth, T.O.S.).
- `systems`: our codes (`SA RA EA OA CW HW HWR SAN V G FP`) and the abbreviation the prints use.
- `nodes` + `segments`: each system as a graph of straight runs. Segments carry shape and size,
  their centreline path, `height` (bottom/top/centre above the floor and the printed `note` if one
  was given), the size `tag` as printed (text, position, leader) and drain `slope`. Nodes are
  fittings (`elbow tee cross transition reducer coupling`), connections (`terminal equipment
  fixture`), `service` entries, `cap`, `vtr`.
- `terminals` (diffusers, grilles, louvre, sprinkler heads), `equipment` (with schedule columns),
  `fixtures`, `risers` (with the symbol and note shown on each level).
- `sheets`: number, title, discipline, level, scale, and the exact maps building → drawing
  (`model_from_building`) and drawing → paper (`paper_from_model`); enlarged plans' `window_ft`;
  legend and abbreviations as printed.
- `clashes`: computed from the geometry. Every set plants at least one on purpose (a sprinkler
  branch through the supply main, a runout hanging below the ceiling, a hot water main raised into a
  girder); `planted` marks those and anything planting caused. There are no others.

## Scoring

`cadaddy_mepfix::score` matches a reader's runs to the truth by plan geometry (parallel, within a
tolerance, overlapping) and reports precision and recall by length, size and system accuracy and
height error. `score::building_from_model` / `building_from_paper` invert a sheet's placement.
`score::perfect` gives the truth's own runs (a baseline).

## Extending

A new kind of element (cable tray, structure, ceilings…) goes into `truth.rs` (its numbers),
`design.rs` (where the building gets it; keep the clash scan clean), `draw.rs` (how sheets show
it) and `score.rs` if readers will find it. `tests/fixtures.rs` checks the truth stays consistent
over many seeds.
