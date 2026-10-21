# Getting Started

This guide walks you through installing CADaddy, drawing your first lines, saving
your work and plotting to PDF.

## Install

### macOS

Download `CADaddy-Mac.dmg` from the latest
[GitHub release](https://github.com/AICodeForgedev/cadaddy/releases). Open the
disk image and drag **CADaddy** into Applications. On first launch Gatekeeper
may ask you to confirm; the app is notarised and Developer-ID signed.

### Windows

Download the `.msi` for your architecture (x64, x86 or arm64). Run it; the
installer is per-user and does not need administrator rights. `.dwg` and `.dxf`
files are registered automatically.

A portable `.zip` is also available if you prefer not to install.

### Linux

Pick the package that suits your distribution:

| Format | Notes |
|:-------|:------|
| `.AppImage` | Runs on most distributions. `chmod +x` and double-click. |
| `.flatpak` | Install with `flatpak install CADaddy.flatpak`. |
| `.deb` | Debian, Ubuntu, Mint: `sudo dpkg -i cadaddy_*.deb`. |
| `.rpm` | Fedora, openSUSE: `sudo rpm -i cadaddy-*.rpm`. |
| `.tar.gz` | Extract anywhere and run `./cadaddy`. |

Aarch64 and RISC-V builds are provided as `.AppImage` / `.tar.gz`.

### FreeBSD

Download the x86_64 `.tar.gz`, extract it and run `./cadaddy`.

### Web

Unzip the web build and open `index.html` in a modern browser. No install needed.

## First drawing

Launch CADaddy. You see the model-space canvas, a command line at the bottom, the
ribbon across the top and a few docked palettes.

### Draw a line

Type at the command line:

```
LINE 0,0 100,0 100,60 0,60 C
```

This draws a rectangle from four corner points (`C` closes it back to the start).

### Draw a circle

```
CIRCLE 50,30 15
```

Centre at (50, 30), radius 15.

### Zoom to see everything

```
ZOOM E
```

(`E` for Extents: the view fits every object.)

### Undo a mistake

Press **Ctrl+Z** or type `U` to undo the last command. `REDO` brings it back.

## Save your drawing

```
SAVEAS
```

Choose a location and file name. CADaddy supports three formats:

| Format | Extension | Notes |
|:-------|:----------|:------|
| CADaddy native | `.cdy` | Fastest; preserves everything. |
| AutoCAD DWG | `.dwg` | Industry standard; R2000 through R2018. |
| AutoCAD DXF | `.dxf` | Text-based exchange format. |

After the first save, **Ctrl+S** (or `QSAVE`) saves in place.

## Open an existing drawing

```
OPEN
```

Or drag a `.dwg`, `.dxf` or `.cdy` file onto the window.

CADaddy also opens `.svg`, `.pdf`, `.ifc`, `.stl`, `.obj`, `.glb` and `.gltf`
files.

## Plot to PDF

```
PLOT
```

The Plot dialog lets you pick a page size, scale and area. For a quick PDF of
everything visible:

```
-PLOT
```

The command-line form writes a PDF to the path you choose with no dialog.

You can also use the JSON form (handy for scripts and agents):

```json
{"command": "plot", "params": {"output": "MyDrawing.pdf"}}
```

## Keyboard shortcuts

A few you will use constantly:

| Key | Command |
|:----|:--------|
| **Ctrl+S** | Save |
| **Ctrl+Z** | Undo |
| **Ctrl+Y** | Redo |
| **Ctrl+C** | Copy to clipboard |
| **Ctrl+V** | Paste |
| **Escape** | Cancel the active command |
| **F3** | Toggle object snaps |
| **F8** | Toggle ortho mode |
| **Scroll wheel** | Zoom in/out |
| **Middle-click drag** | Pan |

## Next steps

- [Coming from AutoCAD](coming-from-autocad.md) if you already know AutoCAD.
- [Working with Files](working-with-files.md) for the converter and exchange notes.
- [Command Reference](command-reference.md) for every command and its parameters.
