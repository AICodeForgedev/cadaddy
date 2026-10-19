//! MCP prompts: ready-made recipes an agent can follow with CADaddy's tools.

use serde_json::{Value, json};

struct Recipe {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// (name, description, required)
    arguments: &'static [(&'static str, &'static str, bool)],
}

const RECIPES: &[Recipe] = &[
    Recipe {
        name: "floor_plan_from_sketch",
        title: "Draw a floor plan from a sketch",
        description: "Turn a described or sketched room layout into a clean 2D floor plan: walls, openings, labels and dimensions on their own layers.",
        arguments: &[
            ("sketch", "The sketch or description: rooms, sizes, doors and windows", true),
            ("units", "Drawing units, e.g. \"feet\" or \"millimeters\" (default the drawing's)", false),
            ("wall_thickness", "Wall thickness in drawing units (default 0.5 ft / 150 mm)", false),
        ],
    },
    Recipe {
        name: "schedule_blocks",
        title: "Count and schedule blocks",
        description: "Count the block references in the drawing (with their attribute values) and put a schedule table on the sheet.",
        arguments: &[
            ("blocks", "Block names to include, wildcards allowed (default all)", false),
            ("at", "Where the table goes, \"x,y\" (default to the right of the drawing)", false),
        ],
    },
    Recipe {
        name: "audit_report",
        title: "Check a drawing for problems",
        description: "Run AUDIT, look for problems a reviewer would flag, and report them without changing the drawing unless asked.",
        arguments: &[("fix", "\"yes\" to let AUDIT fix what it finds (default: report only)", false)],
    },
];

pub fn list() -> Value {
    json!({"prompts": RECIPES.iter().map(|r| json!({
        "name": r.name,
        "title": r.title,
        "description": r.description,
        "arguments": r.arguments.iter().map(|(n, d, req)| json!({"name": n, "description": d, "required": req})).collect::<Vec<_>>(),
    })).collect::<Vec<_>>()})
}

fn arg<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// The recipe `name` filled with `args`, as MCP prompt messages.
pub fn get(name: &str, args: &Value) -> Result<Value, String> {
    let r = RECIPES.iter().find(|r| r.name == name).ok_or_else(|| format!("unknown prompt `{name}`"))?;
    for (n, _, required) in r.arguments {
        if *required && arg(args, n).is_none() {
            return Err(format!("prompt `{name}` needs the argument `{n}`"));
        }
    }
    if let Some(o) = args.as_object()
        && o.values().any(|v| v.as_str().is_some_and(|s| s.len() > 20_000))
    {
        return Err("a prompt argument is too long".into());
    }
    let text = match name {
        "floor_plan_from_sketch" => {
            let units = arg(args, "units").map_or_else(|| "the drawing's units".to_string(), str::to_string);
            let wall = arg(args, "wall_thickness").map_or_else(|| "0.5 ft (150 mm)".to_string(), str::to_string);
            format!(
                "Draw a 2D floor plan in CADaddy from this sketch:\n\n{}\n\nWork in {units}; walls are {wall} thick.\n\
                 1. Read the drawing first (resource cadaddy://drawing or inspect_drawing) and set up layers: \
                 `-layer m A-WALL c 7 A-WALL` style via command_line, or execute layer.new, for A-WALL, A-DOOR, A-GLAZ, A-ANNO, A-DIMS.\n\
                 2. Draw the outer walls on A-WALL with `pline` (closed) and OFFSET them inward by the wall thickness; draw interior walls the same way, then trim the crossings with TRIM.\n\
                 3. Cut door and window openings (BREAK the wall lines at the opening), draw door swings as an arc and a line on A-DOOR and windows as three parallel lines on A-GLAZ.\n\
                 4. Label each room with its name and area on A-ANNO (TEXT or MTEXT; AREA gives the area).\n\
                 5. Dimension the outside walls and each room on A-DIMS (DIMLINEAR / DIMCONTINUE).\n\
                 6. Check with render (zoom extents) and inspect_drawing; fix overlaps or gaps, then summarize what you drew.",
                arg(args, "sketch").unwrap_or_default()
            )
        }
        "schedule_blocks" => {
            let blocks = arg(args, "blocks").unwrap_or("*");
            let at = arg(args, "at").map_or_else(|| "to the right of the drawing's extents".to_string(), |a| format!("at {a}"));
            format!(
                "Count and schedule the blocks in the open CADaddy drawing.\n\
                 1. Read cadaddy://blocks and cadaddy://drawing to see which blocks exist and how many references there are.\n\
                 2. Run execute with command `dataextraction` and params {{\"blocks\": \"{blocks}\", \"combine\": true}} to get the counts and attribute values (rows come back as JSON).\n\
                 3. Check the numbers against `count` for one or two of the blocks.\n\
                 4. Insert the schedule as a table {at}: dataextraction with `at` set and a `title`.\n\
                 5. Render the area around the table to confirm it reads well, and report the totals per block."
            )
        }
        _ => {
            let fix = arg(args, "fix").is_some_and(|f| f.eq_ignore_ascii_case("yes") || f.eq_ignore_ascii_case("true"));
            format!(
                "Check the open CADaddy drawing for problems and write a short report.\n\
                 1. Run `audit` with execute, params {{\"fix\": {fix}}}{}; note every error it lists.\n\
                 2. Read cadaddy://drawing: look at `warnings` and `skipped` (what reading the file left out), objects on layer 0 or on layers that are off or frozen, and an extents box far larger than the drawing (stray objects).\n\
                 3. Read cadaddy://layers for layers with no objects and cadaddy://blocks for unused blocks (PURGE candidates; don't purge unless asked).\n\
                 4. Use query_entities for zero-length lines and duplicate objects (OVERKILL candidates).\n\
                 5. Report: what is wrong, where (handles), how serious, and the command that would fix each.",
                if fix { " (it fixes what it can)" } else { " (report only: nothing is changed)" }
            )
        }
    };
    Ok(json!({
        "description": r.description,
        "messages": [{"role": "user", "content": {"type": "text", "text": text}}],
    }))
}
