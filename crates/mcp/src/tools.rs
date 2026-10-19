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
        json!({"name": "find_text", "description": "Read the drawing's texts: TEXT, MTEXT (formatting removed), block attributes and multileaders, each with its text, insertion point `at`, height, rotation, style, layer and handle. Filter by `contains` (any case), `layer` and/or `window` [[x0,y0],[x1,y1]] around the insertion points; page with offset and limit (default 200). Use it to find room names, sheet titles and notes without reading all the geometry.",
            "inputSchema": {"type": "object", "properties": {"contains": {"type": "string"}, "layer": {"type": "string"},
                "window": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 2, "maxItems": 2},
                "limit": {"type": "integer", "minimum": 0}, "offset": {"type": "integer", "minimum": 0}}}}),
        json!({"name": "stairs", "description": "Stairs and ramps of the building spec. action \"detect\" finds them in the plan (tread rows between stringers, UP/DN notes, landings, ramp outlines) and with store: true puts them in the spec (confirmed ones stay); \"list\" gives the spec's stairs with risers, riser and tread sizes and code warnings (risers 4\"–7¾\", treads ≥ 10\", ramps ≤ 1:12); \"confirm\" marks names (or all) as checked. model.build then builds them and Walk climbs them.",
            "inputSchema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["detect", "list", "confirm"]}, "store": {"type": "boolean"},
                "names": {"type": "array", "items": {"type": "string"}}, "all": {"type": "boolean"}, "confirmed": {"type": "boolean"}}, "required": ["action"]}}),
        json!({"name": "new_drawing", "description": "Create a new drawing (imperial by default, or metric).",
            "inputSchema": {"type": "object", "properties": {"metric": {"type": "boolean"}}}}),
        json!({"name": "open", "description": "Open a drawing (CADaddy .cdy, DWG, DXF, DWT; an SVG or PDF is converted) as a new tab that becomes current. Returns its index and any warnings (what reading left out, styles reset). Opening a file that is already open switches to it.", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}),
        json!({"name": "save", "description": "Save the drawing, its format by extension: .cdy (CADaddy's own, keeps everything), .dwg or .dxf (the result's `notKept` lists what those wouldn't keep, the first time). A .svg, .png or .pdf path exports instead: the drawing keeps its name and unsaved changes.", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}}}),
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
