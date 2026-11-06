# IFC model viewer and BCF issues

CADaddy opens an IFC file (IFC4 or IFC2x3) **as a model**: every building element with a body,
each carrying what the file says about it (class, GlobalId, name, site, building, storey, space,
type, systems, property and quantity sets). Coordination issues on the model travel as **BCF 2.1**
files between CADaddy and any other BCF-capable program.

Open it with **File ▸ Open IFC Model...**, then **Window ▸ IFC Model** shows the palette.

## The palette

- **Model tree**: site → building → storeys (bottom up) → spaces → classes → objects. A click
  selects an object; a double-click on a storey, space or class isolates it.
- **Properties**: the selected object's identity (class, name, GlobalId, type, storey, space,
  building, systems, size) and its property and quantity sets (`Pset_…`, `Qto_…`).
- **Filter**: by class (`Wall` also finds `WallStandardCase`; the `Ifc` prefix is optional),
  storey, system and property (`Set.Name` or `Set.Name=value`). Isolate, hide or select what
  matches; Show All brings everything back.
- **Section box and measure**: box the selection (everything wholly outside the box hides; renders
  and BCF snapshots cut exactly at the box), and measure between two selected objects (centre to
  centre, and the gap between them).
- **Walk** walks the model: its storeys are Walk's levels, walls and columns are what you bump into.
- **BCF issues**: new from the view (the camera, the selected objects, a comment and a snapshot),
  status (Open, In Progress, Resolved, Closed), assignee, comments; a click goes back to an issue's
  view with its objects selected. Import and export `.bcf` files.

## Commands

| Command | What it does |
|---|---|
| `ifc.open {path}` | Open an IFC file as a model (3D view, shaded) |
| `ifc.tree {limit?}` | The spatial tree |
| `ifc.properties {handle? \| guid?}` | One object's properties and quantities |
| `ifc.filter {class?, level?, space?, system?, type?, property?, name?, guids?, action?}` | List, select, isolate, hide; `action: show` shows all |
| `ifc.sectionbox {on?, min?, max?, fit?, pad?}` | The section box |
| `ifc.measure {from, to} \| {a, b}` | A 3D distance between points or objects |
| `ifc.camera {eye, target} \| {fit, dir?}` | Look from a place |
| `ifc.render {path?, width?, height?, style?, eye?, target?, dir?, highlight?}` | The model as it shows, as a PNG |
| `bcf.create {title, comment?, assignee?, …}` | A topic from the view and selection |
| `bcf.list`, `bcf.get {topic}` | The topics; one in full |
| `bcf.update {topic, status?, assignee?, comment?, …}` | Change a topic, add a comment |
| `bcf.show {topic}` | Back to its viewpoint, its objects selected |
| `bcf.delete {topic}` | Remove a topic |
| `bcf.export {path, topics?}`, `bcf.import {path}` | BCF 2.1 files |

Topics are kept in the drawing (saved in `.cdy` and DXF). Viewpoints are kept as BCF keeps them:
camera position, direction and up vector in metres, in the model's coordinates; the selection by
IFC GlobalId (objects another model doesn't have are reported as missing, not refused).

Importing a topic that is already here merges its comments and viewpoints; its fields (status,
assignee, title…) come from whichever side modified it last. BCF 2.0 files read too (a topic
folder's `viewpoint.bcfv` without a `Viewpoints` list); snapshots are PNG or JPEG up to 4 MB.

## Limits

- The section box hides whole objects in the live view (an object reaching into the box shows
  whole); renders and snapshots cut at the box.
- Renders and snapshots are parallel projections; a BCF perspective camera's field of view is kept
  and written back but not drawn.
- Measuring between objects uses their boxes (exact for upright walls, columns and straight runs).
- IFC models attached as references (`ifc.attach`) are coordinated with `ifc.elements` and
  `model.clashes`; the viewer works on a model opened as the drawing.

Tests: `crates/engine/tests/suite/files/ifc_viewer.rs` (a made-up IFC4 model written in the test; BCF round
trips between two sessions, a BCF file as another program might write it, junk files),
`crates/ui-egui/tests/suite/panels/ifc_palette.rs` (`CADADDY_IFC_SHOTS=<dir>` saves the palette's renders).
