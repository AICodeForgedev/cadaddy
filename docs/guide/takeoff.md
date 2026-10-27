# Takeoff (Quantity Measurement)

CADaddy includes measurement and quantity-takeoff tools for counting objects,
measuring lengths and areas, and exporting the results.

## TAKEOFF command

The `TAKEOFF` command (alias `QTO`) measures lengths or areas from selected
objects, grouped by layer.

### Length takeoff

```
TAKEOFF
```

Measures the total length of lines, arcs, circles, polylines, ellipses and
splines on each layer. Results include real-world units based on the drawing's
INSUNITS setting.

### Area takeoff

At the first prompt, choose the **Area** keyword:

```
Command: TAKEOFF
Mode [Length/Area] <Length>: Area
```

Measures the enclosed area of closed polylines, circles, ellipses and hatches.

### Filter by layer

At the layers prompt, enter a wildcard pattern:

```
Layers <*>: E-*,P-*
```

### Add waste

```
Waste % <0>: 10
```

Adds 10% waste to every measurement. The waste percentage is also stored in the
`CADADDY_TAKEOFF_WASTE` system variable so it persists across commands.

### Export to CSV

```
File <>: takeoff.csv
```

Writes a CSV with columns: layer, object count, drawing units, real units,
with-waste value.

### Insert as a table

```
Table insertion point <>: 0,0
```

Places a TABLE entity in the drawing at the given point with the takeoff results.

### Nested objects

By default, takeoff counts shapes inside block references, arrays and external
references, applying the correct scale. The interactive prompts also ask about
nested and clipped options.

### JSON parameters (for scripts and agents)

```json
{
  "command": "takeoff",
  "params": {
    "mode": "length",
    "layers": "E-*",
    "waste": 10,
    "file": "takeoff.csv"
  }
}
```

Returns:

```json
{
  "mode": "length",
  "unit": "ft",
  "rows": [
    {"layer": "E-POWER", "objects": 42, "drawing": 1250.5, "real": 1250.5, "withWaste": 1375.55}
  ],
  "total": {"objects": 42, "drawing": 1250.5, "real": 1250.5, "withWaste": 1375.55}
}
```

## Count palette

The Count palette (**Tools > Palettes > Count**) shows a live count of objects
grouped by type, layer, block name or property. It updates as you edit the
drawing.

Use it for:

- Counting block references (fixtures, devices, furniture)
- Tallying objects per layer
- Highlighting and zooming to counted items

## MEASUREGEOM

The `MEASUREGEOM` command (menu: **Tools > Inquiry > Measurement**) is an
interactive measurement tool with several modes:

| Mode | What it measures |
|:-----|:-----------------|
| **Distance** | Pick two points; shows distance, angle and X/Y/Z deltas |
| **Radius** | Select a circle or arc; shows radius and diameter |
| **Angle** | Pick a vertex and two points; shows the angle in degrees |
| **Area** | Pick multiple points; shows area and perimeter. Press Enter to total. |
| **Quick** | Real-time measurement at the cursor |

## Other measurement commands

| Command | Alias | What it does |
|:--------|:------|:-------------|
| `DIST` | `DI` | Distance between two points |
| `AREA` | `AA` | Area of a polygon defined by points or objects |
| `LIST` | `LI` | Full property listing of selected objects |
| `MASSPROP` | | Mass properties of 3D solids and regions |
| `ID` | | Coordinates of a picked point |

## Typical workflow

1. Open a floor plan.
2. Run TAKEOFF and choose **Length** mode, filter to `E-*` layers, set 10% waste
   to get conduit lengths.
3. Run TAKEOFF again, choose **Area** mode, filter to `A-FLOOR-*` to get floor
   areas.
4. Export both to CSV or insert as tables for your estimate.

For scripting and agents, use `execute` with JSON parameters instead of
interactive prompts:

```json
execute {"command": "takeoff", "params": {"mode": "length", "layers": "E-*", "waste": 10, "file": "conduit.csv"}}
execute {"command": "takeoff", "params": {"mode": "area", "layers": "A-FLOOR-*", "at": [0,0]}}
```
