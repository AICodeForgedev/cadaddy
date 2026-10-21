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
