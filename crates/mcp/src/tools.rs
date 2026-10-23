//! MCP tool definitions and dispatch.

use serde_json::{Value, json};

use crate::backend::Backend;

pub fn tool_definitions(has_ui: bool) -> Value {
    let mut tools = vec![
        json!({"name": "command_line", "description": "Type one line at the CADaddy command line, exactly as a user would at an AutoCAD-style prompt. Starts commands by name or alias (LINE, L, CIRCLE, C, PLINE, RECTANG, MOVE, TRIM, OFFSET, ZOOM…) and answers the active prompt: points `x,y`, relative `@dx,dy`, polar `@dist<angle`, distances, keywords (e.g. `c` for Close), empty text = Enter. Spaces act as Enter, so `circle 0,0 5` works in one call. Returns the new prompt and command-line output, and `error` when the line was refused. In the running app some commands typed alone open a dialog, as in AutoCAD (LAYER, UNITS, DSETTINGS, HATCH, PURGE, FIND, COLOR…): type the dash form (-PURGE, -LAYER), add the input on the same line (`color 1`), or use execute; ui_elements shows an open dialog's controls.",
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}),
        json!({"name": "script", "description": "Run a multi-line script (like an AutoCAD .scr file): one or more inputs per line, blank line = Enter.",
            "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}),
        json!({"name": "execute", "description": "Run any command with JSON parameters (no prompts or dialogs). Use list_commands to find ids and parameter docs. Examples: line {points:[[0,0],[10,0]]}, circle {center:[0,0], radius:5}, layer.new {name:\"Walls\", color:\"red\", current:true}, move {handles:[\"10A\"], delta:[5,0]}, zoom {mode:\"extents\"}. In the running app, interface commands work too, e.g. ui.dock {operation:\"detach\"|\"dock\"|\"open\"|\"close\", panel:\"properties\"} floats a palette in its own window or docks it back.",
            "inputSchema": {"type": "object", "properties": {"command": {"type": "string"}, "params": {"type": "object"}}, "required": ["command"]}}),
        json!({"name": "execute_many", "description": "Run many commands in one call and one undo step (up to 10,000): placing hundreds of objects at once instead of a call each. Each result comes back in order; a failed command's result is {error}, or with stopOnError the batch stops there, everything it did is undone, and the reply names the failing command.", "inputSchema": {"type": "object", "properties": {"commands": {"type": "array", "description": "[{command, params}] as for execute"}, "stopOnError": {"type": "boolean"}}, "required": ["commands"]}}),
        json!({"name": "watch_events", "description": "Watch what the user (or anyone) does in CADaddy: after this, events arrive as notifications/cadaddy/event — drawing changed (with added/removed/changed handles, coalesced), selection changed, command started/ended/run, prompt changed, document opened/closed/switched, dialog opened. on:false stops.", "inputSchema": {"type": "object", "properties": {"on": {"type": "boolean"}}}}),
        json!({"name": "list_commands", "description": "List commands with ids, labels, menu paths, aliases and parameter docs. Optional filter: a substring of the command id, label or an alias.",
            "inputSchema": {"type": "object", "properties": {"filter": {"type": "string"}}}}),
        json!({"name": "inspect_drawing", "description": "Summary of the active drawing: entity counts, extents, layers, styles, blocks, layouts, selection, undo history and (optionally) entities with handles and geometry.",
            "inputSchema": {"type": "object", "properties": {"entities": {"type": "boolean"}, "limit": {"type": "integer"}}}}),
        json!({"name": "query_entities", "description": "Entities filtered by type (Line, Circle, Polyline…), layer and/or a window [[x0,y0],[x1,y1]] (objects whose geometry is inside or crosses it; crossing:false = entirely inside). Returns count (all matches), returned (this page) and entities; page with offset and limit (default 500).",
            "inputSchema": {"type": "object", "properties": {"type": {"type": "string"}, "layer": {"type": "string"},
                "window": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 2, "maxItems": 2},
                "crossing": {"type": "boolean"}, "limit": {"type": "integer", "minimum": 0}, "offset": {"type": "integer", "minimum": 0}}}}),
        json!({"name": "render", "description": "Render the active drawing to a PNG and return it as an image: fitted to the extents by default, or just the area `window` [[x0,y0],[x1,y1]] (drawing units), e.g. one sheet or one room of a large drawing. With a window and no size the image is 1600 px wide with the window's shape.",
            "inputSchema": {"type": "object", "properties": {"width": {"type": "integer", "minimum": 16}, "height": {"type": "integer", "minimum": 16}, "fit": {"type": "boolean"},
                "window": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 2, "maxItems": 2},
                "path": {"type": "string"}}}}),
        json!({"name": "drawing_summary", "description": "Start here on an unfamiliar drawing: what it holds without its geometry, capped and paged for huge drawings. Units, extents per space, layouts (paper, viewports), layers with object counts (placed and inside blocks), blocks with reference counts (nested included), every text with its position (paged: textOffset, textLimit ≤ 5000, filter textContains), title block attributes, xrefs, images and PDF underlays with found/missing, dimension styles in use. `sections` picks some of: units, extents, layouts, layers, blocks, texts, titleBlocks, references, dimStyles.",
            "inputSchema": {"type": "object", "properties": {"sections": {"type": "array", "items": {"type": "string"}}, "textOffset": {"type": "integer", "minimum": 0},
                "textLimit": {"type": "integer", "minimum": 0}, "textContains": {"type": "string"}}}}),
        json!({"name": "drawing_query", "description": "Find objects across model space and the layouts and get their handles: by type (Line, Circle, Polyline, Block Reference… or DXF name), layer (wildcards * ?), space (all, current, model or a layout name), window [[x0,y0],[x1,y1]] (crossing unless crossing:false), text (texts, attributes and multileaders containing it) and block (references of it). Returns count (all matches), handles and objects {handle, type, layer, space}; page with offset and limit (default 1000).",
            "inputSchema": {"type": "object", "properties": {"type": {"type": "string"}, "layer": {"type": "string"}, "space": {"type": "string"},
                "window": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 2, "maxItems": 2},
                "crossing": {"type": "boolean"}, "text": {"type": "string"}, "block": {"type": "string"}, "limit": {"type": "integer", "minimum": 0}, "offset": {"type": "integer", "minimum": 0}}}}),
        json!({"name": "find_text", "description": "Read the drawing's texts: TEXT, MTEXT (formatting removed), block attributes and multileaders, each with its text, insertion point `at`, height, rotation, style, layer and handle. Filter by `contains` (any case), `layer` and/or `window` [[x0,y0],[x1,y1]] around the insertion points; page with offset and limit (default 200). Use it to find room names, sheet titles and notes without reading all the geometry.",
            "inputSchema": {"type": "object", "properties": {"contains": {"type": "string"}, "layer": {"type": "string"},
                "window": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 2, "maxItems": 2},
                "limit": {"type": "integer", "minimum": 0}, "offset": {"type": "integer", "minimum": 0}}}}),
        json!({"name": "stairs", "description": "Stairs and ramps of the building spec. action \"detect\" finds them in the plan (tread rows between stringers, UP/DN notes, landings, ramp outlines) and with store: true puts them in the spec (confirmed ones stay); \"list\" gives the spec's stairs with risers, riser and tread sizes and code warnings (risers 4\"–7¾\", treads ≥ 10\", ramps ≤ 1:12); \"confirm\" marks names (or all) as checked. model.build then builds them and Walk climbs them.",
            "inputSchema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["detect", "list", "confirm"]}, "store": {"type": "boolean"},
                "names": {"type": "array", "items": {"type": "string"}}, "all": {"type": "boolean"}, "confirmed": {"type": "boolean"}}, "required": ["action"]}}),
        json!({"name": "rooms", "description": "The plan's rooms as 3D spaces (found from room name labels inside closed outlines, with their numbers): action \"list\" gives each room's name, number, area (drawing units, sq ft, m²), ceiling height (the building spec's, a ceiling note in the room, the level's, else 9'), volume, finishes (codes written in the room, spec materials) and outline; \"get\" one room by name or number; \"goto\" zooms the view to it; \"build\" puts a see-through volume per room on layer 3D-Rooms (clear: true removes them).",
            "inputSchema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["list", "get", "goto", "build"]}, "name": {"type": "string"}, "number": {"type": "string"}, "outline": {"type": "boolean"}, "clear": {"type": "boolean"}}, "required": ["action"]}}),
        json!({"name": "bim_types", "description": "BIM element types in the drawing: shared spec records (type mark, description, manufacturer, catalog number, spec section, material, ratings, mounting, raceway/cable make-up). action \"list\" returns all types (filter by mark or category); \"set\" creates or updates one (fields: {key: {value, source?, confidence?} or string}); \"remove\" deletes one; \"get_spec\" reads the spec of entities (resolved through their type mark); \"set_spec\" assigns a type mark to entities; \"unset_spec\" removes the assignment.",
            "inputSchema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["list", "set", "remove", "get_spec", "set_spec", "unset_spec"]},
                "mark": {"type": "string"}, "category": {"type": "string"},
                "fields": {"type": "object"}, "remove_fields": {"type": "array", "items": {"type": "string"}},
                "handles": {"type": "array", "items": {"type": "string"}},
                "create": {"type": "object"}}, "required": ["action"]}}),
        json!({"name": "project_rules", "description": "Project rules: mounting heights, wiring methods, clearances, tolerances, naming and stud/joist layout per device/room type, each with a source (spec section, RFI #) and a tier (field_verified > rfi > submittal > drawing_revision > spec > project_rule > built_in). The highest tier wins. action \"list\" returns all rules (filter by domain); \"get\" resolves one rule (which tier won, what it overrode); \"set\" adds/updates a value with source, tier and reason (shows what changes); \"remove\" deletes a rule or tier; \"preview\" shows what setting would change without modifying; \"label\" gets/sets the project name; \"import\" reads a TOML/JSON file; \"export\" writes one.",
            "inputSchema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["list", "get", "set", "remove", "preview", "label", "import", "export"]},
                "domain": {"type": "string", "enum": ["mounting_height", "wiring_method", "stud_layout", "joist_layout", "clearance", "tolerance", "naming", "custom"]},
                "key": {"type": "string"}, "value": {"type": "string"},
                "tier": {"type": "string", "enum": ["field_verified", "rfi", "submittal", "drawing_revision", "spec", "project_rule", "built_in"]},
                "source": {"type": "string"}, "set_by": {"type": "string"}, "reason": {"type": "string"},
                "label": {"type": "string"}, "path": {"type": "string"}, "merge": {"type": "boolean"}},
                "required": ["action"]}}),
        json!({"name": "bim_spec_import", "description": "Read a spec book, schedule or submittal into proposed BIM types for the drawing. action \"import\" reads a PDF or text file and returns proposals with citations; \"accept\" merges chosen proposals into the type catalog. Nothing is applied silently — the agent reviews first. Spec-vs-schedule disagreements produce RFI items. Approved submittals override the spec's \"or equal\"; rejected ones are recorded but not applied.",
            "inputSchema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["import", "accept"]},
                "file": {"type": "string", "description": "path to PDF or text file (for import)"},
                "text": {"type": "string", "description": "inline text instead of file (for import)"},
                "format": {"type": "string", "enum": ["spec", "schedule", "submittal", "auto"], "description": "auto-detected if omitted"},
                "proposals": {"type": "array", "description": "proposals to accept (for accept action)"},
                "overwrite": {"type": "boolean", "description": "replace existing fields instead of merging (for accept)"}
            }, "required": ["action"]}}),
        json!({"name": "new_drawing", "description": "Create a new drawing (imperial by default, or metric).",
            "inputSchema": {"type": "object", "properties": {"metric": {"type": "boolean"}}}}),
        json!({"name": "open", "description": "Open a drawing (CADaddy .cdy, DWG, DXF, DWT; an SVG or PDF is converted) as a new tab that becomes current. Returns its index and any warnings (what reading left out, styles reset). Opening a file that is already open switches to it.", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}),
        json!({"name": "save", "description": "Save the drawing, its format by extension: .cdy (CADaddy's own, keeps everything), .dwg or .dxf (the result's `notKept` and `notKeptDetail` — feature, dropped|changed, count, handles — list what those wouldn't keep, the first time; the checkexchange command lists it without saving). A .svg, .png or .pdf path exports instead: the drawing keeps its name and unsaved changes.", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}}}),
        json!({"name": "cancel", "description": "Press Escape: cancel the running command (or clear the selection).", "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "get_prompt", "description": "What CADaddy is asking for right now: the running command, the prompt text and bare message, its keyword options (type the capital letters to choose one), the default Enter takes, which inputs it accepts (point, number, text, selection), whether a drawing is still opening (`busy`), whether a \"Save changes?\" question is open (`closePrompt`), and the last command-line lines. Use it to step through a command with command_line.",
            "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "wait_until_idle", "description": "Wait until CADaddy is no longer busy (a drawing opening in the background), up to timeoutMs (default 30000). Returns the prompt state, with timedOut: true if it was still busy.",
            "inputSchema": {"type": "object", "properties": {"timeoutMs": {"type": "integer", "minimum": 0}}}}),
        json!({"name": "list_drawings", "description": "The open drawings: index, title, path, whether each has unsaved changes (`modified`), which is current, and their object counts.", "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "switch_drawing", "description": "Make another open drawing current, by index, path or title.",
            "inputSchema": {"type": "object", "properties": {"index": {"type": "integer", "minimum": 0}, "path": {"type": "string"}, "title": {"type": "string"}}}}),
        json!({"name": "close_drawing", "description": "Close a drawing (the current one by default, or by index, path or title). A drawing with unsaved changes is closed only with save: true (to its own file), saveAs: a path, or discard: true — otherwise the call is refused and nothing is lost.",
            "inputSchema": {"type": "object", "properties": {"index": {"type": "integer", "minimum": 0}, "path": {"type": "string"}, "title": {"type": "string"}, "save": {"type": "boolean"}, "saveAs": {"type": "string"}, "discard": {"type": "boolean"}}}}),
        json!({"name": "close_all", "description": "Close every drawing. Refused (closing nothing) while one has unsaved changes, unless save: true (each saved to its own file; refused if one has none yet) or discard: true.",
            "inputSchema": {"type": "object", "properties": {"save": {"type": "boolean"}, "discard": {"type": "boolean"}}}}),
        json!({"name": "undo", "description": "Undo the last `count` commands (default 1).", "inputSchema": {"type": "object", "properties": {"count": {"type": "integer", "minimum": 1}}}}),
        json!({"name": "redo", "description": "Redo `count` undone commands (default 1).", "inputSchema": {"type": "object", "properties": {"count": {"type": "integer", "minimum": 1}}}}),
        json!({"name": "select", "description": "Set the selection (commands like MOVE, ERASE and PROPERTIES then act on it): by handles, a window [[x0,y0],[x1,y1]] (crossing: true selects what it touches), all: true, or clear: true; add: true adds to the current selection. Returns the selected handles.",
            "inputSchema": {"type": "object", "properties": {"handles": {"type": "array", "items": {"type": "string"}},
                "window": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 2, "maxItems": 2},
                "crossing": {"type": "boolean"}, "all": {"type": "boolean"}, "clear": {"type": "boolean"}, "add": {"type": "boolean"}}}}),
        json!({"name": "get_variable", "description": "Read a system variable (CLAYER, LTSCALE, DIMSCALE, FILEDIA, INSUNITS…).", "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}}),
        json!({"name": "set_variable", "description": "Set a system variable, as SETVAR does (range-checked; read-only ones are refused).",
            "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}, "value": {}}, "required": ["name", "value"]}}),
        json!({"name": "run_lisp", "description": "Evaluate AutoLISP in the current drawing (entget/entmod/ssget/command… and loaded routines). `inputs` answers get… calls in order. Returns the value and printed output.",
            "inputSchema": {"type": "object", "properties": {"code": {"type": "string"}, "inputs": {"type": "array"}}, "required": ["code"]}}),
        json!({"name": "plot", "description": "Plot a layout (or Model) to a PDF file with its page setup, as PLOT does, without the dialog.",
            "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "layout": {"type": "string"}}, "required": ["path"]}}),
        json!({"name": "project", "description": "Manage projects (groups of related drawings sharing rules, types and standards). Actions: \"create\" {path, name?, metric?}, \"open\" {path}, \"close\", \"info\", \"list\" {discipline?, status?, kind?}, \"activate\" {name}, \"shelve\" {save?}, \"add\" {path, discipline?, kind?, title?}, \"remove\" {name}, \"disk\", \"shelve_project\", \"restore_project\" {path}, \"import_manifest\" {path}, \"validate_manifest\" {path}, \"standards.list\" {domain?}, \"standards.set\" {domain, key, value, source?}, \"standards.remove\" {domain, key}, \"standards.import\" {from, domain?, keys?}.",
            "inputSchema": {"type": "object", "properties": {
                "action": {"type": "string"}, "path": {"type": "string"}, "name": {"type": "string"},
                "metric": {"type": "boolean"}, "discipline": {"type": "string"}, "kind": {"type": "string"},
                "status": {"type": "string"}, "title": {"type": "string"}, "save": {"type": "boolean"},
                "domain": {"type": "string"}, "key": {"type": "string"}, "value": {},
                "source": {"type": "string"}, "from": {"type": "string"},
                "keys": {"type": "array", "items": {"type": "string"}}
            }, "required": ["action"]}}),
        json!({"name": "bim_job", "description": "Start here for BIM work on a project (you need no memory of earlier sessions: the project keeps it). One call per step; answers are short, paged (cursor) and capped; repeating a call is safe. Actions, each with an example of params:\n- briefing {} — one page: revision, what is built per level, open review items, recent changes, handoff notes, the next call.\n- note {text: \"Level 2 ducts reviewed to grid D; mech room next\"} — leave a handoff note in the project.\n- import {path: \"/jobs/hotel/prints\"} — open a folder of prints as the project and classify its sheets.\n- build {level: \"LEVEL 2\"} — build the model from the building spec (the same spec gives the same model).\n- review_next {kind: \"mep_guess\"} — the most important open item with candidates and the documents to check.\n- review_list {status: \"open\", cursor: \"c10\"} — the queue, 10 at a time.\n- resolve {id: \"RV-M1a2b3c4d\", answer: {size_in: [14, 10], bottom_ft: 10.5}, citation: {kind: \"drawing\", document: \"M-101\", ref: \"tag at C-4\"}} — answer an item; anything the documents speak to needs a citation (drawing, schedule, spec, submittal, rfi). A concern from code knowledge alone: {id, concern: \"...\"} raises it for review instead of changing the model.\n- defer {id, reason: \"field to verify the beam depth\"} — leave it for a field decision.\n- rfi {id, question: \"Confirm duct size at C-4\"} — put it on the RFI list.\n- batch {kind: \"mep_guess\", system: \"M-DUCT-SUPP\", answer: {...}, citation: {...}} — one answer for many identical items: returns a preview and a token; call again with confirm: token.\n- log {author: \"agent\", approved: false} — what agents decided, for a person to approve.\n- takeoff {} — quantities. rfis {} — the RFI list (20 most severe; rfis {path: \"/jobs/hotel/rfis.csv\"} exports all). export {path: \"/jobs/hotel/model.ifc\"} — the model as IFC.\nLow-level commands (execute) stay available but aren't needed for these.",
            "inputSchema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["briefing", "note", "import", "build", "review_next", "review_list", "resolve", "defer", "rfi", "batch", "log", "takeoff", "rfis", "export"]}, "params": {"type": "object"}}, "required": ["action"]}}),
        json!({"name": "site_takeoff", "description": "Sitework quantities read off a job's civil sheets (grading, site and utility plans, DXF/DWG; none: the open drawing): earthwork cut AND fill between the existing and proposed surfaces (contours and spot grades), paving, curbs, fences, utility structures with RIM/INV, and storm/sanitary/water pipe LF by size and material with trench excavation and backfill. Each line has its status (read, rule, assumed or insufficient data: no number when a surface is missing), source sheets and the objects whose shares add up to it. Paged with offset and limit (default 100); unknown symbols are listed, not guessed. Opens no dialog.",
        "inputSchema": {"type": "object", "properties": {
            "paths": {"type": "array", "description": "civil sheet files: a path, or {path, sheet} to name the sheet", "items": {}},
            "units_per_foot": {"type": "number"}, "offset": {"type": "integer", "minimum": 0}, "limit": {"type": "integer", "minimum": 1},
            "elements": {"type": "boolean", "description": "list each line's objects (default true)"}
        }}}),
        json!({"name": "mep", "description": "MEP review: list every run/fitting/item the reader made, grouped by system and level, with confidence and the source of its height and size. Edit a run's size, height or system, mark it reviewed, and rebuild the model keeping hand edits. Actions: \"list\" (default) returns items (paged, default 50), \"edit\" changes one item by its stable id (e.g. \"R1\").",
        "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string"},
            "level": {"type": "string"}, "system": {"type": "string"}, "guessed_only": {"type": "boolean"},
            "cursor": {"type": "integer", "minimum": 0, "description": "offset for paging (list action)"},
            "limit": {"type": "integer", "minimum": 1, "maximum": 200, "description": "items per page (default 50, max 200)"},
            "id": {"type": "string", "description": "stable run id (e.g. \"R1\")"},
            "size_in": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2},
            "bottom_ft": {"type": "number"}, "reviewed": {"type": "boolean"},
            "edited_by": {"type": "string", "description": "who made this edit (default \"agent\")"},
            "rebuild": {"type": "boolean"}
        }}}),
    ];
    if has_ui {
        tools.push(json!({"name": "screenshot", "description": "Screenshot the CADaddy window (PNG).", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}}}));
        tools.push(json!({"name": "ui_inspect", "description": "UI state: panels, canvas rect, view, cursor and snap, performance.", "inputSchema": {"type": "object", "properties": {}}}));
        tools.push(json!({"name": "answer_close_prompt", "description": "When the app shows \"Save changes to …?\" (a person closed a drawing or the app), answer it: save, discard or cancel. Fails when no such question is open.",
            "inputSchema": {"type": "object", "properties": {"choice": {"type": "string"}}, "required": ["choice"]}}));
        tools.push(json!({"name": "quit_app", "description": "Quit CADaddy. Refused while a drawing has unsaved changes unless save: true (saves each to its own file first) or discard: true.",
            "inputSchema": {"type": "object", "properties": {"save": {"type": "boolean"}, "discard": {"type": "boolean"}}}}));
        tools.push(json!({"name": "ui_elements", "description": "The controls on screen now, dialogs and menus included: label, role (button, checkbox, radio, tab, combobox, textedit, value, menuitem, link), screen rectangle [x,y,w,h], enabled, selected, focused. Filter by exact `label` or `role`. Click one with ui_click {x, y, space:\"screen\"} at its centre.",
            "inputSchema": {"type": "object", "properties": {"label": {"type": "string"}, "role": {"type": "string"}, "focused": {"type": "boolean"}}}}));
        tools.push(json!({"name": "ui_click", "description": "Click in the drawing area at world coordinates (or screen with space:\"screen\"). Right button = Enter.", "inputSchema": {"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "space": {"type": "string"}, "button": {"type": "string"}}, "required": ["x", "y"]}}));
    }
    json!({ "tools": tools })
}

/// `engine.execute` of `command` with `params`.
/// The `bim_job` tool: one job-level action per call (see its description).
fn bim_job(b: &mut dyn Backend, a: &Value) -> Result<Value, String> {
    let params = a.get("params").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
    let command = match a.get("action").and_then(Value::as_str) {
        Some("briefing") => "project.briefing",
        Some("note") => "project.note",
        Some("build") => "bim.build",
        Some("review_next") => "review.next",
        Some("review_list") => "review.list",
        Some("resolve") => "review.resolve",
        Some("defer") => "review.defer",
        Some("rfi") => "review.rfi_draft",
        Some("batch") => "review.batch",
        Some("log") => "review.log",
        Some("takeoff") => "takeoff",
        Some("export") => "export",
        Some("rfis") => {
            // The RFI list: exported in full with a path, else a compact page.
            if params.get("path").is_some() {
                return exec(b, "rfi.export", params);
            }
            let v = exec(b, "rfi.list", params)?;
            let items: Vec<Value> = v["items"]
                .as_array()
                .into_iter()
                .flatten()
                .take(20)
                .map(|i| json!({"id": i["id"], "title": i["title"], "severity": i["severity"], "status": i["status"], "level": i["level"]}))
                .collect();
            return Ok(
                json!({"total": v["total"], "bySeverity": v["bySeverity"], "items": items, "more": "bim_job rfis {path: \"rfis.csv\"} exports the whole list"}),
            );
        }
        Some("import") => {
            // Open the prints as the project, classify sheets, then run readers.
            let opened = exec(b, "printset.open", params)?;
            let classified = exec(b, "printset.classify", json!({}))?;
            // Run each reader in dependency order on the classified sheets.
            let mut reader_results = json!({});
            for (cmd, key) in [
                ("structure.read", "structure"),
                ("walls.read", "walls"),
                ("ceilings.read", "ceilings"),
                ("ducts.read", "ducts"),
                ("pipes.read", "pipes"),
                ("conduit.read", "conduit"),
                ("trays.read", "trays"),
                ("sprinklers.read", "sprinklers"),
                ("gear.read", "gear"),
                ("mep.equipment.read", "equipment"),
                ("mep.read_devices", "devices"),
                ("site.read", "site"),
                ("mep.heights", "heights"),
                ("rfi.read", "rfis"),
            ] {
                match exec(b, cmd, json!({})) {
                    Ok(v) => {
                        reader_results[key] = json!({"ok": true, "summary": v.get("message")});
                    }
                    Err(e) => {
                        reader_results[key] = json!({"ok": false, "error": e.chars().take(200).collect::<String>()});
                    }
                }
            }
            return Ok(
                json!({"opened": {"name": opened["name"], "sheets": opened["sheets"], "revision": opened["revision"]}, "classified": classified, "readers": reader_results, "next": "bim_job build {}"}),
            );
        }
        Some(other) => {
            return Err(format!(
                "bim_job has no action `{}` — use briefing, note, import, build, review_next, review_list, resolve, defer, rfi, batch, log, takeoff, rfis or export",
                other.chars().take(30).collect::<String>()
            ));
        }
        None => return Err("bim_job needs an action — start with {action: \"briefing\"}".into()),
    };
    exec(b, command, params)
}

fn exec(b: &mut dyn Backend, command: &str, params: Value) -> Result<Value, String> {
    b.call("engine.execute", json!({"command": command, "params": params}))
}

fn text(v: &Value) -> Value {
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()}]})
}

fn err_text(e: &str) -> Value {
    json!({"content": [{"type": "text", "text": e}], "isError": true})
}

/// Whether `v` fits the subset of JSON Schema the tool definitions use: `type`, `items`,
/// `minItems`, `maxItems` and `minimum`.
fn fits(v: &Value, schema: &Value) -> bool {
    let typed = match schema["type"].as_str() {
        Some("string") => v.is_string(),
        // JSON Schema integers include numbers such as 2.0.
        Some("integer") => v.as_f64().is_some_and(|f| f.fract() == 0.0),
        Some("number") => v.is_number(),
        Some("boolean") => v.is_boolean(),
        Some("array") => v.is_array(),
        Some("object") => v.is_object(),
        _ => true,
    };
    let len = v.as_array().map(Vec::len);
    typed
        && schema["minimum"].as_f64().is_none_or(|m| v.as_f64().is_none_or(|f| f >= m))
        && schema["minItems"].as_u64().is_none_or(|m| len.is_none_or(|n| n as u64 >= m))
        && schema["maxItems"].as_u64().is_none_or(|m| len.is_none_or(|n| n as u64 <= m))
        && (schema["items"].is_null() || v.as_array().is_none_or(|a| a.iter().all(|i| fits(i, &schema["items"]))))
}

/// Arguments checked against the tool's input schema: an object, required properties present and
/// each given property fitting its schema (`null` counts as not given). `Err` is the
/// invalid-params message.
fn check_args(name: &str, args: &Value) -> Result<Value, String> {
    let defs = tool_definitions(true);
    let schema = defs["tools"].as_array().and_then(|t| t.iter().find(|t| t["name"] == name)).map(|t| t["inputSchema"].clone());
    let Some(schema) = schema else { return Err(format!("unknown tool `{name}`")) };
    let a = match args {
        Value::Null => json!({}),
        Value::Object(_) => args.clone(),
        _ => return Err(format!("`arguments` of `{name}` must be an object")),
    };
    for key in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if a.get(key).is_none_or(Value::is_null) {
            return Err(format!("`{name}` needs `{key}`"));
        }
    }
    for (key, prop) in schema["properties"].as_object().into_iter().flatten() {
        if let Some(v) = a.get(key).filter(|v| !v.is_null())
            && !fits(v, prop)
        {
            return Err(format!("`{key}` of `{name}` must match {prop}"));
        }
    }
    Ok(a)
}

/// Whether a command-catalog entry's id, label or one of its aliases contains `f` (lower case).
fn command_matches(c: &Value, f: &str) -> bool {
    let has = |s: &str| s.to_ascii_lowercase().contains(f);
    ["id", "label"].iter().any(|k| c[*k].as_str().is_some_and(has))
        || c["aliases"].as_array().is_some_and(|a| a.iter().filter_map(Value::as_str).any(has))
}

/// Run a tool. `Err` is a protocol error (unknown tool, or arguments that don't fit its input
/// schema), answered with JSON-RPC invalid params; a failing tool is an `isError` result.
pub fn call_tool(b: &mut dyn Backend, name: &str, args: &Value) -> Result<Value, String> {
    let a = check_args(name, args)?;
    let r = match name {
        "command_line" => b.call("cmdline.input", a),
        "script" => b.call("cmdline.script", a),
        "execute" => b.call(
            "engine.execute",
            json!({"command": a.get("command").cloned().unwrap_or(Value::Null), "params": a.get("params").cloned().unwrap_or(json!({}))}),
        ),
        "execute_many" => b.call("engine.executeMany", json!({"commands": a.get("commands").cloned().unwrap_or(json!([])), "stopOnError": a.get("stopOnError").cloned().unwrap_or(json!(false))})),
        "watch_events" => b.call("events.subscribe", json!({"on": a.get("on").and_then(Value::as_bool).unwrap_or(true)})),
        "list_commands" => b.call("engine.commands", json!({"filter": a.get("filter").cloned().unwrap_or(Value::Null)})).map(|v| {
            let f = a.get("filter").and_then(Value::as_str).map(str::to_ascii_lowercase);
            match (v, f) {
                (Value::Array(items), Some(f)) => Value::Array(items.into_iter().filter(|c| command_matches(c, &f)).collect()),
                (v, _) => v,
            }
        }),
        "inspect_drawing" => b.call("drawing.inspect", a),
        "query_entities" => b.call("engine.execute", json!({"command": "entities", "params": a})),
        "find_text" => b.call("engine.execute", json!({"command": "texts", "params": a})),
        "drawing_summary" => b.call("drawing.summary", a),
        "drawing_query" => b.call("drawing.query", a),
        "stairs" => {
            let command = match a.get("action").and_then(Value::as_str) {
                Some("detect") => "stairs.detect",
                Some("confirm") => "stairs.confirm",
                _ => "stairs.list",
            };
            let mut params = a.clone();
            if let Some(o) = params.as_object_mut() {
                o.remove("action");
            }
            b.call("engine.execute", json!({"command": command, "params": params}))
        }
        "rooms" => {
            let command = match a.get("action").and_then(Value::as_str) {
                Some("get") => "rooms.get",
                Some("goto") => "rooms.goto",
                Some("build") => "rooms.build",
                _ => "rooms.list",
            };
            let mut params = a.clone();
            if let Some(o) = params.as_object_mut() {
                o.remove("action");
            }
            b.call("engine.execute", json!({"command": command, "params": params}))
        }
        "bim_types" => {
            let command = match a.get("action").and_then(Value::as_str) {
                Some("set") => "bim.set_type",
                Some("remove") => "bim.remove_type",
                Some("get_spec") => "bim.get_spec",
                Some("set_spec") => "bim.set_spec",
                Some("unset_spec") => "bim.unset_spec",
                _ => "bim.types",
            };
            let mut params = a.clone();
            if let Some(o) = params.as_object_mut() {
                o.remove("action");
            }
            b.call("engine.execute", json!({"command": command, "params": params}))
        }
        "project_rules" => {
            let command = match a.get("action").and_then(Value::as_str) {
                Some("get") => "project.rules.get",
                Some("set") => "project.rules.set",
                Some("remove") => "project.rules.remove",
                Some("preview") => "project.rules.preview",
                Some("label") => "project.rules.label",
                Some("import") => "project.rules.import",
                Some("export") => "project.rules.export",
                _ => "project.rules.list",
            };
            let mut params = a.clone();
            if let Some(o) = params.as_object_mut() {
                o.remove("action");
            }
            b.call("engine.execute", json!({"command": command, "params": params}))
        }
        "bim_spec_import" => {
            let command = match a.get("action").and_then(Value::as_str) {
                Some("accept") => "bim.spec_accept",
                _ => "bim.spec_import",
            };
            let mut params = a.clone();
            if let Some(o) = params.as_object_mut() {
                o.remove("action");
            }
            b.call("engine.execute", json!({"command": command, "params": params}))
        }
        "new_drawing" => b.call("engine.execute", json!({"command": "new", "params": a})),
        "open" => b.call("app.open", a),
        "save" => b.call("app.save", a),
        "cancel" => b.call("cmdline.key", json!({"key": "escape"})),
        "get_prompt" => b.call("cmdline.state", json!({})),
        "wait_until_idle" => {
            let limit = a.get("timeoutMs").and_then(Value::as_u64).unwrap_or(30_000).min(600_000);
            let start = std::time::Instant::now();
            loop {
                let st = b.call("cmdline.state", json!({}));
                let busy = st.as_ref().is_ok_and(|v| v["busy"] == json!(true));
                if !busy {
                    break st;
                }
                if start.elapsed().as_millis() as u64 >= limit {
                    break st.map(|mut v| {
                        v["timedOut"] = json!(true);
                        v
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
        "list_drawings" => exec(b, "document.list", a),
        "switch_drawing" => exec(b, "document.switch", a),
        "close_drawing" => exec(b, "document.close", a),
        "close_all" => exec(b, "document.closeall", a),
        "undo" => exec(b, "undo", json!({"count": a.get("count").cloned().unwrap_or(json!(1))})),
        "redo" => exec(b, "redo", json!({"count": a.get("count").cloned().unwrap_or(json!(1))})),
        "select" => exec(b, "select", a).and_then(|mut r| {
            // The reply names what is selected, not only how many.
            let d = b.call("drawing.inspect", json!({"entities": false}))?;
            r["handles"] = d["selection"].clone();
            Ok(r)
        }),
        "get_variable" => exec(b, "getvar", a),
        "set_variable" => exec(b, "setvar", a),
        // An evaluation error is the tool's error, with what was printed before it.
        "run_lisp" => exec(b, "lisp", a).and_then(|r| match r.get("error").and_then(Value::as_str) {
            Some(e) => Err(format!(
                "LISP error: {e}{}",
                r["output"].as_array().filter(|o| !o.is_empty()).map(|o| format!(" (output: {})", Value::Array(o.clone()))).unwrap_or_default()
            )),
            None => Ok(r),
        }),
        "plot" => exec(b, "exportpdf", a),
        "bim_job" => bim_job(b, &a),
        "site_takeoff" => exec(b, "site.takeoff", a),
        "mep" => {
            let command = match a.get("action").and_then(Value::as_str) {
                Some("edit") => "mep.edit",
                _ => "mep.list",
            };
            let mut params = a.clone();
            if let Some(o) = params.as_object_mut() {
                o.remove("action");
            }
            exec(b, command, params)
        }
        "project" => {
            let command = match a.get("action").and_then(Value::as_str) {
                Some("create") => "project.create",
                Some("open") => "project.open",
                Some("close") => "project.close",
                Some("info") => "project.info",
                Some("list") => "project.list",
                Some("activate") => "project.activate",
                Some("shelve") => "project.shelve",
                Some("add") => "project.add",
                Some("remove") => "project.remove",
                Some("disk") => "project.disk",
                Some("shelve_project") => "project.shelve_project",
                Some("restore_project") => "project.restore_project",
                Some("import_manifest") => "project.import_manifest",
                Some("validate_manifest") => "project.validate_manifest",
                Some("standards.list") => "project.standards.list",
                Some("standards.set") => "project.standards.set",
                Some("standards.remove") => "project.standards.remove",
                Some("standards.import") => "project.standards.import",
                _ => "project.info",
            };
            let mut params = a.clone();
            if let Some(o) = params.as_object_mut() {
                o.remove("action");
            }
            exec(b, command, params)
        }
        "answer_close_prompt" => b.call("app.answerClose", a),
        "quit_app" => {
            // Refuse before quitting rather than lose work: the app's own quit discards.
            let save = a.get("save").and_then(Value::as_bool) == Some(true);
            let discard = a.get("discard").and_then(Value::as_bool) == Some(true);
            if !save && !discard {
                let open = b.call("engine.execute", json!({"command": "document.list", "params": {}}));
                let modified: Vec<String> = open
                    .ok()
                    .and_then(|v| v["drawings"].as_array().cloned())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|d| d["modified"] == json!(true))
                    .filter_map(|d| d["title"].as_str().map(str::to_string))
                    .collect();
                if !modified.is_empty() {
                    return Ok(err_text(&format!("unsaved changes in {}: pass save: true or discard: true", modified.join(", "))));
                }
            }
            b.call("app.quit", a)
        }
        "screenshot" => b.call("ui.screenshot", a),
        "ui_inspect" => b.call("ui.inspect", json!({})),
        "ui_click" => b.call("ui.pointer", a),
        "ui_elements" => b.call("ui.elements", a),
        "render" => {
            return Ok(match b.call("ui.render", a) {
                Ok(v) => match v.get("pngBase64").and_then(Value::as_str) {
                    Some(data) => json!({"content": [{"type": "image", "data": data, "mimeType": "image/png"}]}),
                    None => text(&v),
                },
                Err(e) => err_text(&e),
            });
        }
        other => return Err(format!("unknown tool `{other}`")),
    };
    Ok(match r {
        Ok(v) => text(&v),
        Err(e) => err_text(&e),
    })
}
