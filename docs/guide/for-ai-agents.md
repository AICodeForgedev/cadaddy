# For AI Agents

CADaddy is designed to be driven by AI agents. It exposes its full command set
through the Model Context Protocol (MCP) and a TCP JSON control channel.

## MCP setup in Claude Desktop

Add CADaddy to your Claude Desktop configuration
(`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS,
`%APPDATA%\Claude\claude_desktop_config.json` on Windows):

```json
{
  "mcpServers": {
    "cadaddy": {
      "command": "cadaddy-cli",
      "args": ["mcp"]
    }
  }
}
```

This starts `cadaddy-cli mcp`, which connects to the running CADaddy desktop app
on port 47820 (or runs headless if the app is not open).

### Connecting to a specific instance

```json
{
  "mcpServers": {
    "cadaddy": {
      "command": "cadaddy-cli",
      "args": ["mcp", "--connect", "127.0.0.1:47820"]
    }
  }
}
```

### Headless mode

```json
{
  "mcpServers": {
    "cadaddy": {
      "command": "cadaddy-cli",
      "args": ["mcp", "--headless"]
    }
  }
}
```

No desktop window; the agent works on drawings entirely through MCP tools.

## MCP setup in Claude Code

Add to your project's `.mcp.json` or global MCP settings:

```json
{
  "mcpServers": {
    "cadaddy": {
      "command": "cadaddy-cli",
      "args": ["mcp"]
    }
  }
}
```

## Available MCP tools

CADaddy exposes these tools over MCP. Start with the job-level tools
(`drawing_summary`, `bim_job`) when working on a project; fall back to
low-level `execute` only when needed.

### Start here (job-level)

| Tool | Description |
|:-----|:------------|
| `drawing_summary` | What a drawing holds without its geometry: units, extents, layouts, layers with counts, blocks, texts, title block attributes, xrefs, dim styles. Capped and paged for huge drawings. |
| `bim_job` | One call per BIM step: briefing, import prints, build model, review items, resolve with citations, takeoff, RFIs, export IFC. The project keeps state across sessions. |
| `guided_job` | Prints → model / takeoff / estimate as a guided pipeline: `start` a job on a folder of PDFs, then loop `next` / `answer`; the software runs import → classify → levels → rooms → devices → build → takeoff and pauses with questions it can't settle alone. `status`, `explain`, `worklist`, `undo`, `report`. |
| `import_check` | How good a PDF import is: scale, layers, text recovery, fills, symbols, grid — pass/fail, a 0-100 score and per-check evidence; check the open drawing or a file by path. |
| `bim_from_pdf` | One call from PDF prints to a 3D BIM: imports the PDF(s), checks the import, drafts the building spec and builds the model; returns the .cdy paths, check results, built counts, warnings and the exact next calls. Multi-plan sheets become stacked levels. |
| `workflows` | The main agent workflows with exact example calls: pdf-to-bim, view-3d, plot-set, convert-folder. Start here when unsure which tools to use. |
| `job_status` | What CADaddy is busy with (opening files, building models, rendering) and the progress of long commands like `bim_from_pdf` and `model.build`; pair with `wait_until_idle`. |
| `project` | Manage projects (groups of drawings sharing rules, types and standards): create, open, add/remove drawings, standards, shelve/restore. |
| `project_rules` | Project rules with source and tier (field-verified > RFI > submittal > spec > built-in): mounting heights, wiring methods, clearances, tolerances, naming, stud/joist layout. |

### BIM and building

| Tool | Description |
|:-----|:------------|
| `bim_types` | BIM element types: list, create, assign and query type marks (description, manufacturer, catalog, spec section, material, ratings). |
| `bim_props` | Every element's own properties (a device's type, mount height, circuit, room; a door's number, size, rating; a room's number, name, area, finishes…), each with its source (print, legend, schedule, spec, note, rule, default, user, computed): `get`, `set` (as the user's, kept through rebuilds), `find` by key/value/exists/missing, `summary` of coverage per kind. |
| `bim_spec_import` | Read a spec book, schedule or submittal into proposed BIM types. Import returns proposals with citations; accept merges them. Nothing applied silently. |
| `rooms` | Plan rooms as 3D spaces: list rooms with area/volume/finishes, zoom to a room, build see-through volumes. |
| `stairs` | Stairs and ramps: detect from plan, list with code checks (riser/tread sizes), confirm, then model.build builds them. |
| `mep` | MEP review: list runs/fittings by system and level with confidence, edit size/height/system, mark reviewed, rebuild. |
| `callouts` | Cross-reference callouts read from the imported print set (section marks, elevation tags, detail bubbles, enlarged-plan boundaries, match lines, key plans): each with its sheet, kind, detail, target and whether it resolves or is broken; filter by `kind`, `status`, `level`, paged. |
| `arch_read` | The architecture of the open job's prints: doors (mark, hand, swing rooms, card reader), windows, stair, elevator, rooms with finishes and plate colours, each with its source sheet and status (drawing, schedule, guess); `apply: true` merges it into the building spec. |
| `finishes_takeoff` | The finish takeoff from a print set's DXF sheets (or the open print set's): per-trade totals first, then 50 lines a page with room, quantity, unit, with-waste, source sheet and status (drawing, schedule, rule, guess), roll-ups by unit type and what was read only in part; filters `level`, `room`, `unit_type`, `trade`. With no rooms or finish schedule read it says to run `arch_read` first. |
| `site_takeoff` | Sitework quantities from civil sheets: earthwork cut/fill, paving, curbs, utilities with trench excavation and backfill. |
| `framing` | Studs from the prints: `build` frames every wall of a folder of DXF sheets (fixed members, both rule layouts, stacks); `list`, `wall` and `next_stud` give compact, paged answers per wall; `field_point` (tape, centre or photo pick) and `shop_drawing` (DXF) are kept in the job and re-applied on every rebuild; `view` draws the elevation, plan and Walk boxes on `FRAMING-*` layers. The BOM's framing lines and bits come from it. |

### Drawing commands

| Tool | Description |
|:-----|:------------|
| `command_line` | Type at the command line, exactly as a user at an AutoCAD-style prompt. |
| `script` | Run a multi-line script (like an AutoCAD `.scr` file). |
| `execute` | Run any command with JSON parameters (no prompts or dialogs). |
| `execute_many` | Run many commands in one call and one undo step (up to 10,000). |
| `list_commands` | List commands with ids, labels, menu paths, aliases and parameter docs. |

### Drawing inspection

| Tool | Description |
|:-----|:------------|
| `drawing_query` | Find objects across model space and layouts by type, layer, space, window, text or block. Paginated. |
| `inspect_drawing` | Summary of the active drawing: entity counts, extents, layers, styles, blocks, selection, undo history. |
| `query_entities` | Entities filtered by type, layer and/or window, with geometry. Paginated. |
| `find_text` | Texts (TEXT, MTEXT, attributes, multileaders) with content, position, layer and handle. Filter by contains, layer, window. |
| `watch_events` | Subscribe to live events: drawing changed, selection changed, command run, prompt changed, dialog opened. |
| `get_variable` | Read a system variable (CLAYER, LTSCALE, DIMSCALE, INSUNITS, etc.). |
| `set_variable` | Set a system variable. |
| `get_prompt` | Current command-line prompt state: running command, keywords, accepted inputs. |
| `wait_until_idle` | Block until no command is active, up to a timeout. |

### File operations

| Tool | Description |
|:-----|:------------|
| `new_drawing` | Create a blank drawing (imperial or metric). |
| `open` | Open a file (.cdy, .dwg, .dxf, .dwt, .svg, .pdf). |
| `save` | Save the active drawing (format by extension). |
| `close_drawing` | Close a drawing (with save/discard option). |
| `close_all` | Close every drawing. |
| `list_drawings` | List open drawings with index, title, path and modified state. |
| `switch_drawing` | Switch to another open drawing by index, path or title. |

### Visual feedback

| Tool | Description |
|:-----|:------------|
| `render` | Render the current view as PNG (returns the image). Optional window for a specific area. |
| `screenshot` | Screenshot of the application window (requires desktop mode). |
| `ui_inspect` | UI state: panels, canvas rect, view, cursor and snap, performance. |
| `ui_elements` | Visible controls: label, role, rectangle, enabled, selected, focused. |
| `ui_click` | Click at world or screen coordinates. |

### Editing

| Tool | Description |
|:-----|:------------|
| `select` | Select entities by handles, window, all, or clear. |
| `undo` | Undo the last N commands (default 1). |
| `redo` | Redo N undone commands (default 1). |
| `cancel` | Press Escape: cancel the running command. |
| `plot` | Plot a layout to PDF with its page setup. |

### AutoLISP

| Tool | Description |
|:-----|:------------|
| `run_lisp` | Evaluate AutoLISP in the current drawing. |

### Application control (desktop mode)

| Tool | Description |
|:-----|:------------|
| `answer_close_prompt` | Respond to a "save changes?" prompt (save, discard or cancel). |
| `quit_app` | Quit the application. |

## MCP resources

| URI | Description |
|:----|:------------|
| `cadaddy://drawing` | Active drawing summary (counts, layers, extents, selection, entities) |
| `cadaddy://layers` | The active drawing's layers and their states |
| `cadaddy://blocks` | The active drawing's block definitions |
| `cadaddy://layouts` | The active drawing's layouts and viewports |
| `cadaddy://commands` | Full command catalog with parameters |
| `cadaddy://drawing/{index}` | Summary of an open drawing by number |

## MCP prompts

| Prompt | Description |
|:-------|:------------|
| `floor_plan_from_sketch` | Turn a described or sketched room layout into a clean 2D floor plan. |
| `schedule_blocks` | Count block references and insert a schedule table. |
| `audit_report` | Check a drawing for problems and report them. |

## Example agent session

Here is a typical sequence an agent might use:

```
1. new_drawing {}
2. execute {"command": "rectang", "params": {"p1": [0,0], "p2": [100,60]}}
3. execute {"command": "circle", "params": {"center": [50,30], "radius": 10}}
4. execute {"command": "dimlinear", "params": {"p1": [0,0], "p2": [100,0], "at": [50,-8]}}
5. inspect_drawing {}
6. render {}
7. save {"path": "Sample Floor Plan.cdy"}
```

## TCP JSON control channel

For programmatic control without MCP, start CADaddy with a control port:

```sh
cadaddy --control 47820
```

Connect over TCP and send JSON-line requests. First authenticate with the token
written to `control/47820.token` in the settings directory:

```json
{"id": 0, "method": "auth", "params": {"token": "..."}}
```

Then send commands:

```json
{"id": 1, "method": "engine.execute", "params": {"command": "line", "params": {"points": [[0,0],[10,0]]}}}
{"id": 2, "method": "ui.screenshot", "params": {"path": "/tmp/shot.png"}}
```

The full protocol is documented in `docs/control-protocol.md`.

## Safety rails

CADaddy enforces safety limits on the control channel and MCP:

- **Authentication required.** Every connection must present a valid token.
- **Rate limiting.** 500 requests/second average, burst to 2,000.
- **Request size.** 256 MiB maximum per request.
- **Idle timeout.** Connections idle for 10 minutes are closed.
- **No file execution.** Hyperlinks, xrefs and packages pointing at executables,
  UNC paths or `file://` programs prompt the user before opening.
- **AutoLISP sandboxing.** Scripts outside trusted locations do not run
  automatically.

## CLI for batch scripting

`cadaddy-cli` can run scripts without a window:

```sh
cadaddy-cli run --script 'RECTANG 0,0 100,60\nCIRCLE 50,30 10\n' --export out.png
cadaddy-cli run --sample --export sample.pdf
cadaddy-cli info drawing.dxf
cadaddy-cli convert folder/ out/ --to cdy --recurse
```
