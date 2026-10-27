# 3D, Walk and Plans-to-Model

CADaddy includes 3D solid modelling, first-person walkthrough navigation, and an
automated pipeline that turns 2D floor plans into 3D building models.

## 3D primitives

Create solids from the command line or the Draw > 3D menu:

| Command | Description | Example |
|:--------|:------------|:--------|
| `BOX` | Rectangular solid | `BOX 0,0,0 10,8,4` |
| `CYLINDER` | Round column | `CYLINDER 5,5,0 2 10` (centre, radius, height) |
| `CONE` | Cone or truncated cone | `CONE 5,5,0 3 8` |
| `SPHERE` | Sphere | `SPHERE 5,5,5 3` |
| `TORUS` | Donut/ring | `TORUS 5,5,0 4 1` (centre, ring radius, tube radius) |
| `WEDGE` | Triangular solid | `WEDGE 0,0,0 6,4,3` |
| `PYRAMID` | N-sided pyramid | `PYRAMID 5,5,0 3 6` (centre, radius, height) |

## Extrude, revolve, sweep, loft

Turn 2D profiles into 3D solids:

| Command | What it does |
|:--------|:-------------|
| `EXTRUDE` | Push a closed shape along a height or path |
| `REVOLVE` | Spin a profile around an axis |
| `SWEEP` | Move a profile along a path curve |
| `LOFT` | Skin between two or more cross-sections |
| `PRESSPULL` | Click inside a closed area and drag up or down |

Example: draw a closed polyline, then:

```
EXTRUDE
Select objects: (pick the polyline)
Height: 10
```

## 3D editing

| Command | Alias | Description |
|:--------|:------|:------------|
| `3DMOVE` | `3M` | Move objects in 3D space |
| `3DROTATE` | `3R` | Rotate around any axis |
| `3DSCALE` | | Scale in 3D |
| `3DALIGN` | `3AL` | Align by picking source and destination points |
| `3DMIRROR` | | Mirror across a 3D plane |
| `3DARRAY` | `3A` | Rectangular or polar array in 3D |

## Viewing in 3D

| Command | What it does |
|:--------|:-------------|
| `VPOINT` | Set the view direction (e.g. `VPOINT 1,-1,1` for an isometric view) |
| `3DORBIT` | Interactive orbit with the mouse |
| `PLAN` | Return to the top-down plan view |
| `VIEW` | Save and restore named views |
| `VISUALSTYLES` | Switch between wireframe, hidden, shaded and realistic |

## Walk and Fly

First-person navigation through a 3D model:

```
3DWALK
```

| Control | Action |
|:--------|:-------|
| **W / Up** | Move forward |
| **S / Down** | Move backward |
| **A / Left** | Strafe left |
| **D / Right** | Strafe right |
| **Mouse** | Look around |
| **Scroll wheel** | Adjust step size |
| **Escape** | Exit walk mode |

`3DFLY` is similar but allows vertical movement (the camera is not locked to a
walking height).

Speed is controlled by the system variables `STEPSIZE` and `STEPSPERSEC`.

## Publish a walkthrough

```
WALK.PUBLISH
```

Packages the current drawing into a `.cdywalk` file containing:

- The drawing file (or an auto-drafted building spec)
- A glTF 3D model
- A baked lightmap
- A manifest

The `.cdywalk` package can be opened directly in CADaddy (desktop or web build)
and enters walk mode automatically.

## Plans-to-Model

The plans-to-model pipeline reads a 2D floor plan and builds a 3D model
automatically. It works best on plans that follow NCS layer conventions (walls on
`A-WALL`, doors on `A-DOOR`, etc.) but can also process messy drawings and even
PDF imports.

### The pipeline

1. **`PLAN.LAYERS`** — Classify layers by purpose (walls, doors, windows, etc.)
2. **`PLAN.WALLS`** — Detect wall runs from the geometry
3. **`PLAN.ROOMS`** — Identify enclosed rooms and label them
4. **`PLAN.NOTES`** — Read annotations (room names, dimensions, ceiling heights)
5. **`BSPEC.DRAFT`** — Generate a building specification
6. **`STAIRS.DETECT`** — Find stair elements
7. **`MODEL.BUILD`** — Create the 3D model
8. **`MODEL.ISSUES`** — Check the model for problems
9. **`MODEL.COUNTS`** — Tally rooms, doors, windows

You can run the whole pipeline at once or step through it manually to inspect
and adjust at each stage.

### Element properties

Every element of the built model is an object with properties: an outlet holds
`type: duplex`, `mount_height: 18 in`, `circuit: G-12`, `room: 131`; a door its
number, size, frame, hardware set and fire rating. Each value says where it came
from — `print` (read from a sheet, with the sheet and where on it), `legend`,
`schedule`, `spec`, `note`, `rule` or `default` (assumed from a standard, always
marked), `user` (edited) or `computed` (an area, the room an element stands in).

- **`BIM.PROPS.GET`** — an element's properties with their sources, its kind's
  standard keys and which of them have no value yet.
- **`BIM.PROPS.SET`** — set values by hand (source `user`); on a generated element
  the edit is kept as an override (`OVERRIDE.LIST`, kind `props`), so rebuilding
  the model from the prints puts it back.
- **`BIM.PROPS.FIND`** — elements by kind and by `{key: value | "exists" | "missing"}`.
- **`BIM.PROPS.SUMMARY`** — per kind: how many elements, how many have each key
  filled, and from which sources.

The Properties palette shows them under **BIM Properties**, each value with its
source; editing one there is the same as `BIM.PROPS.SET`. They are saved in the
drawing (`.cdy`, DXF, DWG), written to IFC as `Pset_CADaddy` (and the matching
standard set: `Pset_WallCommon`, `Pset_DoorCommon`, `Pset_WindowCommon`,
`Pset_SpaceCommon`, `Pset_SpaceCoveringRequirements`,
`Pset_ManufacturerTypeInformation`), and read back from IFC into the same store
(an attached IFC model's property sets too).

Standard keys per kind (other keys are allowed):

| Kind | Keys |
|:-----|:-----|
| `device` | type, type_mark, mount_height, circuit, panel, voltage, room, level, host_wall, sheet |
| `door` | number, width, height, frame, hardware_set, fire_rating, swing, from_room, to_room, level |
| `window` | number, type_mark, width, height, sill_height, fire_rating, room, level |
| `room` | number, name, area, ceiling_height, floor_finish, wall_finish, ceiling_finish, level |
| `wall` | type, assembly, height, fire_rating, finish_a, finish_b, room_a, room_b, level |
| `floor` | type, assembly, thickness, finish, area, level |
| `ceiling` | type, height, finish, area, room, level |
| `roof` | type, assembly, slope, area, level |
| `duct` | size, system, shape, insulation, elevation, level |
| `pipe` | size, system, material, insulation, elevation, level |
| `conduit` | size, system, circuit, elevation, level |
| `fixture` | type_mark, model, circuit, room, level |
| `light_fixture` | type_mark, model, circuit, mount_height, room, level |
| `equipment` | type_mark, model, circuit, room, level |

#### Filled from the prints

`MODEL.BUILD` makes every device the readers found (`MEP.READ_DEVICES`, the
guided job's devices stage) an element of its own: a small box at its symbol, at
its mounting height, moved onto its level's architectural plan when it was read on
a trade sheet of the same size. It carries its type and type mark (the type is
what the sheets' legend or fixture schedule says the mark is: `A` beside `DUPLEX
RECEPTACLE` makes a `duplex receptacle`, source `legend`, with the row as its
`description`; a light fixture keeps its mark, `light fixture A`; a mark no
legend describes keeps its letter), the circuit
printed beside it, the panel the circuit names and `fed_from` (`panel LP1, circuit
LP1-3`), its sheet, `mount_height` and `offset` (from the device legend, else a
general note such as `MOUNT ALL HORN/STROBES AT 80" AFF`, else the usual height
for its kind, marked `default`) and its 3D `location` (feet, `x, y, z`).

**`BIM.PROPS.FILL`** then fills what the model and the prints give the other
elements (the guided job runs it, after `ROOMS.BUILD`, at the end of its build
stage):

| Kind | Filled |
|:-----|:-------|
| `device` | room (by containment; a device drawn on a room's wall, just outside its outline, takes the nearest room within 2 ft), host_wall (nearest wall), fed_from_handle (the panel's element), breaker, poles and circuit_description (the panel schedule's row) |
| `room` | level, ceiling_height and floor/wall/ceiling/base finishes, as `ROOMS.LIST` reads them |
| `door` | number (the nearest door-number tag within 5 ft of the opening), width and height (computed), from_room / to_room |
| `wall` | type (`print` when a wall tag names it, `rule` when it is the spec's type for the layer), thickness, height, room_a / room_b |
| `duct`, `pipe` | size, system, shape, elevation (a size read from a tag is `print`; an estimated route height is `default`) |
| air terminal (`fixture`) | type, cfm (`150 CFM` beside it, or the number under the type in its tag, `CD-1` over `150`; looked for on the mechanical plan the terminal was read on as well as the floor plan), room |
| `equipment` | type_mark, capacity (from its schedule row: cfm, tons, MBH…), room |
| plumbing `fixture` | placed: one element at each scheduled fixture's tag on a plumbing plan (else the floor plan; enlarged plans and legend entries skipped), with type_mark (`print`), type, cw_size / hw_size / waste_size / vent_size, fixture units and `connections` (`CW 1", SAN 3", V 2"`) from the plumbing fixture schedule (`schedule`), room, host_wall, location |

The schedule rows come in `BIM.PROPS.FILL {schedules: [{tag, sheet, title, row:
{column: text}}]}`. The guided job reads them off the set's mechanical and
plumbing schedule sheets (it imports only the plans into the job's drawing, so it
imports those sheets on their own to read their tables).

A value a person set (`user`) is never replaced, and a value read from the prints
is never replaced by an assumption.

### Tags are identities

A door number, a room number, a wall-type tag at a wall's centreline and a
device's type and circuit at its spot each name one thing in the building, and a
print set shows that thing on several sheets. `bim.identity` (run by the guided
job after the build) keeps one element per thing:

- Every plan sheet is put onto its level's reference plan (the architectural
  plan with the most door and room numbers) by the numbers both sheets print:
  scale, rotation and offset, with the fit's error (`residual`). A sheet with
  no shared numbers is placed by its frame only when it is the same size and the
  one or two numbers it does share sit where the frame puts them; otherwise it
  stays unregistered and is never merged with another sheet.
- Elements of one kind on one level with the same key merge when their places
  agree (doors within 6", devices within 12", plus the fit's error). The best
  source is kept (schedule or spec, then the architectural plan, then another
  discipline's background); it takes the values it lacks from the copies, your
  own edits always win, and `seen_on` lists every sheet it was found on.
- The same number far apart on one level is a finding, never a merge. So are
  door-schedule marks on no plan, door numbers the schedule lacks, a level whose
  door count is off from the schedule's, and finish-schedule rooms with no room
  on the plan.

The job's devices stage counts a device once even when several sheets of its
level show it. A door's swing closes its room: rooms are traced with each
swing's closed leaf across the opening. `rooms.graph` puts every door between
the rooms on its two sides (`from_room`, `to_room` on the door) and lists the
connections. An enlarged plan also registers on its overall plan by the door
and room numbers both print (`printset.links`, method `tags`) when no shared
grid line places it. When neither does, nor a boundary box, it registers where
its walls lie on the overall plan's (method `lines`, with `line_coverage`, the
share of its linework that lands on the plan's), at one to one or the printed
scales' ratio, turned by quarters. A view that fits several places equally (a
typical room along a corridor, a typical floor's plan on several levels) is
left unregistered rather than guessed.

### Quick start

Open a floor plan, then:

```
PLAN.WALLS
PLAN.ROOMS
BSPEC.DRAFT
MODEL.BUILD
3DWALK
```

The model builder creates walls with openings, room ceilings and floors. If the
drawing includes enough information (ceiling heights, door/window block
references), those are incorporated automatically.

### Scoring

The `P2M.SCORE` command compares a built model against ground-truth data and
returns accuracy scores (0-1) for walls, openings, rooms, heights and stairs.
This is used internally for regression testing but is also available for checking
how well the pipeline performed on your plan.

### Benchmark cases

`P2M.BENCH` runs the pipeline against built-in synthetic cases:

| Case | Description |
|:-----|:------------|
| `single` | One storey, NCS layers, 2 rooms, door and window |
| `multi` | Two storeys side-by-side with a stair |
| `messy` | Non-standard layers, furniture and dimensions in the way |
| `pdf` | A floor plan imported from PDF (no layers) |
