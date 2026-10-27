# Coming from AutoCAD

If you have used AutoCAD, most of what you know transfers directly. This page
covers what is the same, what is different, and what is not available yet.

## What is the same

- **Command line.** Type command names exactly as in AutoCAD: `LINE`, `CIRCLE`,
  `TRIM`, `OFFSET`, `COPY`, `MOVE`, `ARRAY`, `HATCH`, and so on. Aliases work
  too (`L` for LINE, `C` for CIRCLE, `CO` for COPY).
- **Coordinate entry.** Absolute (`10,20`), relative (`@5,0`), polar
  (`@10<45`), direct-distance entry (point the cursor and type a number).
- **Object snaps.** Endpoint, midpoint, center, intersection, perpendicular,
  tangent, nearest, node, insertion, quadrant. Toggle with **F3**.
- **Grips.** Click an object to show grips; drag a grip to stretch, move, rotate
  or scale.
- **Layers, linetypes, text styles, dimension styles.** The same concept and the
  same dialog names.
- **Layouts and viewports.** Model space and paper-space layouts with floating
  viewports, viewport scales, layer overrides.
- **Block references, attributes, dynamic blocks.** INSERT, BEDIT, BATTMAN,
  ATTEDIT all work.
- **Dimensions.** Linear, aligned, angular, radial, diameter, ordinate, arc
  length, QDIM, DIMBREAK, DIMSPACE.
- **Hatching.** HATCH and HATCHEDIT with pattern, solid, gradient and user-
  defined fills.
- **Plotting.** PLOT, PAGESETUP, named plot styles.
- **External references.** XREF, XCLIP, XBIND.
- **AutoLISP.** A growing subset of AutoLISP runs inside CADaddy.
- **System variables.** DIMSCALE, LTSCALE, INSUNITS, FILEDIA and many more work
  the same way.
- **File formats.** `.dwg` (R2000-R2018) and `.dxf` open and save.

## What is different

| Area | AutoCAD | CADaddy |
|:-----|:--------|:--------|
| **Native format** | `.dwg` | `.cdy` (our own; DWG is an exchange format) |
| **Sheet sets** | `.dst` (binary) | `.sheetset.json` (JSON text). AutoCAD `.dst` files cannot be opened; create a new set from the same drawings. |
| **Control channel** | COM/ActiveX/ObjectARX | TCP JSON on `--control PORT` or MCP over stdio. |
| **Scripting** | VBA, .NET, ObjectARX, AutoLISP | AutoLISP (partial), JSON commands, MCP tools. |
| **Rendering** | Mental Ray / Arnold | GPU rasteriser and path tracer; no external renderer. |
| **Licensing** | Subscription | Source-available (FSL-1.1-ALv2). Free to use. |
| **Platforms** | Windows (Mac via web) | macOS, Windows, Linux, FreeBSD, web. |

## Keyboard habits

Your muscle memory should transfer. CADaddy uses the same default shortcuts:

| AutoCAD | CADaddy | Notes |
|:--------|:--------|:------|
| Ctrl+S | Ctrl+S | Save |
| Ctrl+Z | Ctrl+Z | Undo |
| Ctrl+Y | Ctrl+Y | Redo |
| Ctrl+C / Ctrl+V | Ctrl+C / Ctrl+V | Clipboard |
| Escape | Escape | Cancel |
| F2 | F2 | Text window |
| F3 | F3 | Object snaps on/off |
| F7 | F7 | Grid on/off |
| F8 | F8 | Ortho on/off |
| F9 | F9 | Snap on/off |
| F10 | F10 | Polar tracking on/off |
| Space / Enter | Space / Enter | Repeat last command |

## Commands that are not available yet

CADaddy is pre-alpha. Some AutoCAD features are not yet implemented or are
partially implemented. The full gap list is maintained in the repository at
`docs/gaps.md`. Key areas still in progress:

- **Xref write-back** (REFEDIT works for blocks; editing an xref source in
  place and saving back is not yet supported)
- **Dynamic-block authoring** (existing dynamic blocks display and grip-edit,
  but creating or modifying parameters and actions in the Block Editor is not
  implemented)
- **Data extraction wizard**
- **Object tracking** (temporary alignment lines from snap points)
- **3D solid modelling** (Boolean operations, shell, slice and imprint are
  partially implemented)
- **Parametric constraints** (geometric and dimensional constraints exist but are
  incomplete)
- **Binary `.ctb`/`.stb` reading** (plot styles work, but importing the binary
  AutoCAD files is not yet supported)

For the latest status, run `cargo xtask parity` or see the
[Command Reference](command-reference.md) for every command that is live.
