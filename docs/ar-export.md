# Export for AR (`bim.export.ar`)

Status: phase 1 (export only), Q6207. A part of the built model goes to a phone or tablet as files
its own augmented-reality viewer opens: AR Quick Look takes a `.usdz` from Files, Mail or AirDrop
(no app); any glTF viewer takes the `.glb`. The user places the model on the real floor by matching
one corner, then walks it at true size.

## Calling it

```
bim.export.ar {room: "12"}                          one room (the default: the first one built)
bim.export.ar {rooms: ["12", "14"], trades: ["electrical"]}
bim.export.ar {level: "L2", kinds: ["receptacle", "switch"]}
bim.export.ar {level: "L2", box: {min: [0, 0], max: [40, 30], z: [0, 12]}}
bim.export.ar {room: "12", anchor: {grid: "C-4"}}   or {corner: 2}, or {name: "control point", at: [12.5, 3]}
bim.export.ar {room: "12", context: "off", small: true, formats: ["usdz"], path: "out/room12"}
```

- **Scope.** `room`/`rooms` (number, name or handle), `level` (all of it), `box` (building feet on
  the level, `z` in feet above its floor). `trades` matches the trade (`electrical`, `plumbing`,
  `mechanical`, `fire_protection`, `architecture`…), the system (`devices`, `lights`, `conduit`,
  `pipe_sanitary`…) or the category; `kinds` matches the element's kind (`receptacle`, `switch`,
  `box`, `duct`…), its build part or its type. An element belongs to a room when its plan box
  comes within `scope.room_margin_ft` of the room's outline. Geometry is cut to the scope's plan
  box (the rooms' outlines grown by that margin, within `box` when given), so a corridor wall or
  a conduit run that goes on past the room ends at its edge (`cut_to_scope` counts them; the
  element's metadata says `cut_to_scope: true`); `clip: false` keeps every element whole. A level
  scope is never cut.
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
| `cut_to_scope` | true when its geometry was cut at the scope's plan box |
| `simplified` | `box` when a cap made it its bounding box |
| `position` | (sidecar) the node's translation, metres |

Repeated parts share one mesh: a glTF mesh referenced by many nodes; a USD prototype under the
abstract `class` prim `/CADaddy_AR/Prototypes` that each element references.

## The sidecar (`NAME.json`)

```
{ "schema": "cadaddy.ar/1", "version": 1, "exported": "2026-11-06T03:58:10Z", "units": "m", "meters_per_unit": 1, "up_axis": "Y",
  "model": "NAME.usdz", "glb": "NAME.glb", "usdz": "NAME.usdz",
  "anchor": {"kind": "room_corner"|"grid_intersection"|"named_point", "name", "level", "room",
             "plan_x", "plan_y", "plan_units": "ft", "floor_elevation_ft", "how", "entry", "model": [0,0,0]},
  "scope": {...}, "rules": {...}, "counts": {"nodes", "meshes", "triangles"},
  "dropped": {"small_parts": {...}, "context_off", "filtered", "not_meshes", "over_element_cap", "over_caps": {...}},
  "cut_to_scope": n, "simplified": "nothing"|"context as boxes"|"context left out"|"context left out, the elements furthest from the anchor left out",
  "points": [{"name": "12 OFFICE corner 1", "kind": "room_corner", "level", "plan": [x, y], "position": [x, y, z]}],
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
- `kind_trades`: a kind's trade and system when the BIM view's layer-based classification
  doesn't say it (a plumbing fixture on a device layer is plumbing; fire alarm devices are their
  own trade).
- `scope`: the room margin and the entry probes (feet past a door's far face).

## Files are valid

- `.glb`: the Khronos glTF 2.0 binary container; POSITION accessors carry min/max, normals are
  unit, indices `UNSIGNED_INT`. Checked with the Khronos glTF Validator (0 errors, 0 warnings).
- `.usdz`: a zip with every file stored (no compression, no encryption), each file's data at a
  multiple of 64 bytes (padding in the local header's extra field), the root layer `model.usda`
  first (Pixar's USDZ specification). Opened and checked with Pixar's USD (`usd-core`) validators.
  Apple's `usdchecker --arkit` adds a rule that the root layer be binary `.usdc`; this export
  writes `.usda` (which AR Quick Look opens), so that one ARKit rule is expected to flag it.

## Serving exports to the field app (`bim.export.ar.manifest`, Q6218)

CADaddy Field's **Update** button pulls the latest packages from the office over the local network.
The office side is a folder of exports, a manifest, and any static web server:

```
bim.export.ar {room: "12", path: "exports/room-12"}      (as often as the model changes)
bim.export.ar.manifest {dir: "exports"}                  writes exports/manifest.json
python3 -m http.server 8000 --directory exports         or any static server on the office LAN
```

On the phone, the first tap of Update asks for the address: the folder's URL (for example
`http://192.168.1.20:8000/` or one with a private path in it, `http://192.168.1.20:8000/k3y/`), or
the manifest's own URL. The address is stored on the device and can be changed (Library ▸ + ▸ Office
address, or a long press on Update). Only local addresses are accepted: private, link-local, loopback
or carrier-grade-NAT IPs, `.local`/`.lan`/`.home.arpa` names and single-label host names. The app's
Info.plist allows plain HTTP to local hosts only (`NSAllowsLocalNetworking`), not a blanket ATS
exception. Nothing is fetched except when Update is tapped.

**The command.** `{dir, force?}`. Reads every `.json` in `dir` and two folder levels down (hidden
entries and links skipped, 10,000 entries at most) and lists each AR sidecar (`schema:
"cadaddy.ar/1"`, or `format: "cadaddy-ar"`) with the models it names beside it (`glb`, `usdz`, else
`model`). A sidecar that names a path, `..`, a link or a missing file is skipped and reported. It is
read-only apart from `manifest.json`. That file goes through the output guard: an earlier manifest is
replaced, and any other file of that name is replaced only with `force: true`. The answer is `{path,
packages, files, bytes, names, skipped: [{name, why}]}`.

**The manifest** (`manifest.json`, format version 1):

```
{ "format": "cadaddy-ar-manifest", "version": 1, "generator": "CADaddy bim.export.ar.manifest",
  "generated": "2026-11-06T04:00:00Z",
  "packages": [
    { "name": "room-12",                  unique: the sidecar's relative path without .json
      "label": "12 OFFICE",               the sidecar's title, else its rooms, else the anchor's room or level
      "level": "L2",                      the anchor's level, when known
      "exported": "2026-11-06T03:58:10Z", the sidecar's own `exported`, else its file time
      "files": [ { "name": "room-12.glb",  "role": "glb",     "bytes": 20708, "sha256": "…64 hex…" },
                 { "name": "room-12.usdz", "role": "usdz",    "bytes": 31232, "sha256": "…" },
                 { "name": "room-12.json", "role": "sidecar", "bytes": 9293,  "sha256": "…" } ] } ] }
```

- `name` (package and file) is relative to the manifest's folder, `/`-separated, never absolute,
  never `.`/`..`, with no `\`, `:`, `%` or control characters. The app refuses any other name.
- `role` is `glb`, `usdz` or `sidecar`. A missing role is taken from the extension. The app
  downloads the `.glb` (it carries per-object metadata), else the `.usdz`, plus the sidecar.
- `bytes` is the exact size; `sha256` is lowercase hex.

**What the app does with it.**
- **Match.** Each package matches a library package by its stored source name: the manifest name it
  was last updated from, else the file or folder name it was opened from by hand.
- **Diff.** A package is unchanged when the model's and the sidecar's SHA-256 both match what is
  stored. Unchanged packages are not downloaded.
- **Verify, then swap.** Each new or changed file is downloaded under a cap of its stated size and
  hashed as it arrives. The size and hash are then checked, and the model must open (the full GLB
  read, or a valid USDZ zip) before anything is replaced. The new version is built beside the old
  one and swapped in one step (APFS `RENAME_SWAP`). Any failure leaves the old package working and
  is reported for that package.
- **Field work carries over.**
  - Marks and photos carry over, matched by `element_id`.
  - A rebuild re-issues element ids. A mark whose id is gone follows its device when the old
    element's `devid` names exactly one new element, and keeps the old id as `element_id_was`.
  - A mark whose element is really gone stays, flagged `element_gone: true` ("element no longer in
    model"). It is never deleted.
  - Each mark keeps the `package_version` (`model_sha256`, `exported`) it was made against.
- **The saved room.**
  - An anchor that is the same point (kind, level, units, plan position within 1/8") keeps the
    saved room and its alignment.
  - When the anchor moved, the old anchor is kept (shown as a grey cross on the plan and in AR) and
    the app asks the user to align again.

## Not in phase 1

LiDAR fitting, writing field status back. The phone app is Q6208 `arapp`; its Update button is
Q6218 `arupdate` (above).
