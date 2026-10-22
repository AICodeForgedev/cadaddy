# Parametric parts

A parametric part is history-based. Constrained 2D sketches feed features (Pad, Pocket, Revolve, Fillet, Chamfer), which are kept in a tree with named parameters. Change a parameter or a sketch and the part rebuilds from the top.

The evaluated body is an ordinary 3D solid in the drawing, so it saves, exports (DXF, DWG, STL, …), plots and measures like any other. The tree itself is kept in the drawing: in `.cdy` (`PRTS`) and in DXF (`CADADDY_PARTS`).

**Window ▸ Model Tree** shows the part's parameters and its features:
- edit an expression and the part rebuilds;
- suppress a feature;
- double-click to rename;
- move up or down;
- delete;
- rebuild.

A feature that fails shows why, and the features after it build on the body as it was.

## Parameters
- `part.new {name?, parameters?: {name: expr}}` starts a part.
- `part.parameter {part?, name, expr}` adds or changes a parameter; `expr: null` removes it.

Expressions use the parameters, e.g. `thick = 5`, `width = 8*thick`, `hole = thick/2`. A cycle or an unknown name is refused, and nothing changes.

## Sketches
`part.sketch {name?, plane?, offset?, face?, geometry, constraints?}` adds a sketch.

- **Plane:** `xy`, `xz` or `yz` (moved by `offset`, an expression), or `{origin, normal, x?}`. `face: {point}` takes the plane of the part's face at that point; in this phase that is a one-time snapshot, not a reference that follows the face.
- **Geometry** is in the sketch's own 2D coordinates:
  - `{type: line, a, b}`;
  - `{type: arc, center, radius, start, end}` (degrees, counter-clockwise);
  - `{type: circle, center, radius}`;
  - any of them with `construction: true` to keep it out of profiles.
- **Constraints** use the drawing's constraint solver, e.g. `{kind: "horizontal_distance", refs: [{geom: 0, sub: "start"}, {geom: 0, sub: "end"}], value: "width"}`.
  - Kinds: coincident, collinear, concentric, fix, parallel, perpendicular, horizontal, vertical, tangent, symmetric, equal, distance, horizontal_distance, vertical_distance, angle, radius, diameter.
  - `sub` is start, end, mid, center, or the whole curve.
  - Dimension values are numbers or expressions in the part's parameters.

The solved geometry is written back to the sketch. Its closed loops (lines and arcs chained end to end, and circles) are the profiles; loops inside others are holes.

## Features
| Command | What it does |
|---|---|
| `part.pad {sketch, length, reversed?, symmetric?}` | extrudes the profile along the sketch's normal (or against it, or both ways) and adds it |
| `part.pocket {sketch, depth \| through_all, reversed?}` | cuts the profile into the body, against the normal |
| `part.revolve {sketch, angle?: 360, axis?: v\|h, cut?}` | turns the profile about the sketch's Y (`v`) or X (`h`) axis; the profile stays on one side of the axis |
| `part.fillet {edges: [[x, y, z]], radius}` | rounds straight edges between flat faces |
| `part.chamfer {edges: [[x, y, z]], distance}` | bevels them |

Edges are picked by points on them. They are kept as the two faces either side, so they stay on the same edges when the part is resized.

## The tree
- `part.edit {feature, name?, suppressed?, length?|depth?|angle?|radius?|distance?, reversed?, symmetric?, through_all?, axis?, cut?, geometry?, constraints?, plane?, offset?}` changes a feature.
- `part.reorder {feature, to}` moves a feature. A feature can't move above its sketch.
- `part.delete {feature}` removes a feature. A sketch in use can't be deleted.
- `part.rebuild` rebuilds the part.
- `part.tree` returns the parameters with their values, and every feature with its state (`ok`, `suppressed`, `error` with the reason).
- `part.list` lists the parts.

## Phase 2
Not in this phase:
- patterns (linear, polar) and mirror;
- sketch-on-face references that follow the face through a rebuild;
- external geometry in sketches;
- fillets and chamfers on curved edges.
