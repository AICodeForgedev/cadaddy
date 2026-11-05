# Export for AR (`bim.export.ar`)

Status: phase 1 (export only), Q6207. A part of the built model goes to a phone or tablet as files
its own augmented-reality viewer opens: AR Quick Look takes a `.usdz` from Files, Mail or AirDrop
(no app); any glTF viewer takes the `.glb`. The user places the model on the real floor by matching
one corner, then walks it at true size.

## Calling it

```
bim.export.ar {room: "101"}                          one room (the default: the first one built)
bim.export.ar {rooms: ["101", "102"], trades: ["electrical"]}
bim.export.ar {level: "L2", kinds: ["receptacle", "switch"]}
bim.export.ar {level: "L2", box: {min: [0, 0], max: [40, 30], z: [0, 12]}}
bim.export.ar {room: "101", anchor: {grid: "C-4"}}   or {corner: 2}, or {name: "control point", at: [12.5, 3]}
bim.export.ar {room: "101", context: "off", small: true, formats: ["usdz"], path: "out/room101"}
```

- **Scope.** `room`/`rooms` (number, name or handle), `level` (all of it), `box` (building feet on
  the level, `z` in feet above its floor). `trades` matches the trade (`electrical`, `plumbing`,
  `mechanical`, `fire_protection`, `architecture`…), the system (`devices`, `lights`, `conduit`,
  `pipe_sanitary`…) or the category; `kinds` matches the element's kind (`receptacle`, `switch`,
  `box`, `duct`…), its build part or its type. An element belongs to a room when its plan box
  comes within `scope.room_margin_ft` of the room's outline.
- **Files.** `path` names `NAME`, `NAME.glb`, `NAME.usdz` or `NAME.json`; all three are written
  beside each other (`formats` picks glb and/or usdz; the sidecar is always written). Without
  `path` they go beside the drawing as `DRAWING-ar-SCOPE.*`, never over an earlier export. An
  explicit file that exists is replaced only with `force: true`.
- **Answer.** Files and sizes, nodes, meshes (shared), triangles drawn, counts by kind and trade,
  the anchor, the scope, what was dropped and why, what was simplified, the time taken.

## The frame

| | glTF (`.glb`) | USD (`.usdz`) |
|---|---|---|
| units | metres (glTF 2.0 §3.4) | `metersPerUnit = 1` |
| up | +Y | `upAxis = "Y"` |
| plan x | +X | +X |
| plan y | −Z | −Z |
| origin | the anchor, on its level's finished floor | same |

Each element stands at its model elevation above the anchor level's finished floor (y = 0 is the
floor), so a user places the model by matching the anchor to one real floor point and turning it
to match a wall.

**The anchor** (default: the room corner nearest the room's entry). The entry is the door on the
room's outline whose far side is the biggest other room (a corridor or lobby beats a closet or a
bathroom), then one to the outside, then the widest; the corner is the outline's turning vertex
nearest that door, the doorway's own jambs left out. With no door found: the room's south-west
corner. Other anchors: `{corner: n}` (the sidecar's `points` list the corners, 1-based),
`{grid: "A-1"}` (an intersection `grids.detect` finds on the level), `{name, at: [x, y]}` (a named
point in building feet), or `{name}` alone (a plan text with exactly that wording). A level or
box scope without a room anchors at the box's corner or the level's origin.

## Per element

One node (glTF) / one `instanceable` Xform (USD) per element, named `kind_tag`
(letters, digits, `_`). The same metadata is in glTF node `extras`, USD `customData` (dictionary
`cadaddy`) and the sidecar's `elements` (RealityKit doesn't expose `customData`, so a viewer
matches its entities to the sidecar by `node`):

| key | what |
|---|---|
| `id` | the element's handle in the drawing (to write status back by id) |
| `kind`, `tag` | what it is (a device's own kind; a run's piece) and its tag or type mark |
| `circuit`, `panel` | from its properties, else its build tag, else its plan block's attribute |
| `mounting_height_in`, `mounting_height_m`, `mounting_height_from`, `mount_datum` | the documented height when there is one (`documented`), else its bottom above the floor (`model bottom`) |
| `bottom_aff_in`, `top_aff_in` | its model bottom and top above the floor |
| `catalog`, `manufacturer`, `type_mark`, `product_source` | its product (the element's own, else the type catalog's by mark) |
| `trade`, `system`, `category`, `type` | the BIM view's classification |
| `level`, `room`, `devid`, `part`, `source` | where it is and what it was built from |
| `context` | true for the shell (walls, floors, ceilings, doors…) |
| `simplified` | `box` when a cap made it its bounding box |
| `position` | (sidecar) the node's translation, metres |

Repeated parts share one mesh: a glTF mesh referenced by many nodes; a USD prototype under the
abstract `class` prim `/CADaddy_AR/Prototypes` that each element references.

## The sidecar (`NAME.json`)

```
{ "schema": "cadaddy.ar/1", "version": 1, "units": "m", "meters_per_unit": 1, "up_axis": "Y",
  "model": "NAME.usdz", "glb": "NAME.glb", "usdz": "NAME.usdz",
  "anchor": {"kind": "room_corner"|"grid_intersection"|"named_point", "name", "level", "room",
             "plan_x", "plan_y", "plan_units": "ft", "floor_elevation_ft", "how", "entry", "model": [0,0,0]},
  "scope": {...}, "rules": {...}, "counts": {"nodes", "meshes", "triangles"},
  "dropped": {"small_parts": {...}, "context_off", "filtered", "not_meshes", "over_element_cap", "over_caps": {...}},
  "simplified": "nothing"|"context as boxes"|"context left out"|"context left out, the elements furthest from the anchor left out",
  "points": [{"name": "101 GUEST corner 1", "kind": "room_corner", "level", "plan": [x, y], "position": [x, y, z]}],
  "elements": [{...the metadata above..., "position": [x, y, z]}] }
```

The same header (without `points` and `elements`) is in the glTF's `asset.extras.cadaddy` and the
root node's `extras.cadaddy`, and in the USD layer's `customLayerData` dictionary `cadaddy`.

## Rules data

`crates/engine/src/cmd/arexport.json`; a job overrides any part with
`bim.export.ar.rules {rules: {...}}` (kept under `ar_export` in the building spec; `reset: true`
goes back):

- `caps`: `max_triangles` (300,000), `max_bytes` (50 MB), `max_elements`. A call can lower them
  (`max_triangles`, `max_bytes`). Over a cap: the context as boxes, then no context, then the
  elements furthest from the anchor, until it fits; the answer and sidecar report each step.
- `context`: the build parts shown as context, the default mode (`translucent`, `solid`, `off`)
  and the opacity.
- `skip_parts`: parts never exported (room elements: spaces, not things).
- `small_parts`: the trades' loose pieces left out unless `small: true`: build parts (`atom`),
  run pieces (`strap`, `staple`, `connector`, `wire`…) and anything under `max_size_mm` across.
- `colors`: RGB by system (the BIM view's trade keys), then by trade, then `default`.
- `scope`: the room margin and the entry probes (feet past a door's far face).

## Files are valid

- `.glb`: the Khronos glTF 2.0 binary container; POSITION accessors carry min/max, normals are
  unit, indices `UNSIGNED_INT`. Checked with the Khronos glTF Validator (0 errors, 0 warnings).
- `.usdz`: a zip with every file stored (no compression, no encryption), each file's data at a
  multiple of 64 bytes (padding in the local header's extra field), the root layer `model.usda`
  first (Pixar's USDZ specification). Opened and checked with Pixar's USD (`usd-core`) validators.
  Apple's `usdchecker --arkit` adds a rule that the root layer be binary `.usdc`; this export
  writes `.usda` (which AR Quick Look opens), so that one ARKit rule is expected to flag it.

## Not in phase 1

The phone app (Q6208 `arapp`, reading this format), LiDAR fitting, writing field status back.
