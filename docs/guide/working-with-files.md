# Working with Files

CADaddy reads and writes several file formats. This guide covers everyday file
operations, the batch converter and exchange notes.

## Supported formats

### Open (input)

| Extension | Description |
|:----------|:------------|
| `.cdy` | CADaddy native format |
| `.dwg` | AutoCAD drawing (R2000 through R2018) |
| `.dxf` | Drawing Exchange Format (ASCII and binary) |
| `.dwt` | Drawing template |
| `.pdf` | Pages are converted to CAD geometry |
| `.svg`, `.svgz` | Scalable Vector Graphics |
| `.ifc` | Industry Foundation Classes (building models) |
| `.stl`, `.obj` | 3D mesh formats |
| `.glb`, `.gltf` | GL Transmission Format |
| `.cdywalk` | Walk-through package |

### Save (output)

| Extension | Description |
|:----------|:------------|
| `.cdy` | CADaddy native (fastest, lossless) |
| `.dwg` | AutoCAD drawing (version selectable: R2000-R2018) |
| `.dxf` | Drawing Exchange Format |
| `.dwt` | Template |
| `.pdf` | Plotted output |
| `.svg` | Vector export |
| `.png` | Raster export |
| `.ifc` | IFC4 building model |
| `.glb`, `.gltf`, `.obj`, `.stl`, `.3mf` | 3D mesh exports |

## Everyday operations

### New drawing

```
NEW
```

Opens a blank drawing. To start from a template:

```
NEW template.dwt
```

### Open

```
OPEN
```

Or drag files onto the window. Multiple drawings can be open at once; switch with
the tabs at the top or `Ctrl+Tab`.

### Save

| Command | Behaviour |
|:--------|:----------|
| `QSAVE` (Ctrl+S) | Save in the current format and location. |
| `SAVEAS` | Choose a new name, location or format. |
| `SAVE` | Same as SAVEAS. |

### Close

```
CLOSE
```

Closes the active drawing (prompts to save if modified). `CLOSEALL` closes every
open drawing.

## The converter

The `CONVERT` command (also `DWGCONVERT` or `CDYCONVERT`) batch-converts between
`.cdy`, `.dwg` and `.dxf`.

### Convert a single file

```
CONVERT input.dwg output.cdy
```

### Convert a folder

```
CONVERT --input "Project Files" --output "CADaddy Files" --format cdy
```

Subfolders are replicated in the output. Parameters:

| Parameter | Default | Description |
|:----------|:--------|:------------|
| `input` | required | File or folder path |
| `output` | beside input | Destination folder or file |
| `format` | `cdy` | Target: `cdy`, `dwg` or `dxf` |
| `version` | 2018 | DWG version (2000, 2004, 2007, 2010, 2013, 2018) |
| `exists` | `skip` | What to do if the output exists: `skip`, `overwrite`, `rename` |
| `recursive` | `true` | Process subfolders |

### From the CLI

```sh
cadaddy-cli convert --input "Project Files" --output "CADaddy Files" --format cdy
```

The CLI processes files without opening a window.

### Handling failures

If a file in the middle of a batch fails, the converter logs the error, skips it
and continues with the rest. The return value lists every file and its status.

## Exchange notes

When converting between formats, some features do not have an exact equivalent.
CADaddy produces **exchange notes** that document every difference:

- What was approximated (e.g. a spline fit tolerance changed)
- What was dropped (e.g. an unknown extension dictionary)
- What was substituted (e.g. a font that was not found)

Notes appear in the converter output. They are also available programmatically
in the JSON return value so scripts and agents can inspect them.

A round-trip test (DWG -> .cdy -> DWG -> .cdy) should be stable after the first
conversion: the exchange notes should list only genuine differences, with no
false positives and no missing entries.

## Recovery and backups

CADaddy saves automatic backups:

- **Autosave** writes a recovery file at a configurable interval (default 10
  minutes). If the app crashes, the next launch offers to recover.
- **`.bak` files** are created beside the drawing on every manual save (when
  enabled in Options).
- The `RECOVER` command attempts to open a damaged `.cdy`, `.dwg` or `.dxf` file
  and repair what it can, reporting what was fixed.

## eTransmit

```
ETRANSMIT
```

Packages the current drawing with all its dependencies (xrefs, images, fonts,
plot styles) into a `.zip` file for sending to someone else. The recipient can
open the main drawing and everything resolves.
