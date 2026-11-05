//! `cadaddy-cli`: headless CADaddy.
//!
//! ```text
//! cadaddy-cli help [COMMAND]                     the main workflows, or one command's summary, parameters and example
//! cadaddy-cli guide                              every workflow step by step (the MCP `workflows` text)
//! cadaddy-cli info FILE [--full]                   short summary (--full for detailed JSON with manifest and chunk sizes)
//! cadaddy-cli convert IN.(dwg|dxf|cdy) OUT.(dwg|dxf|cdy|pdf|svg|png) [--window x0,y0,x1,y1 | --view extents|current]
//! cadaddy-cli convert IN_FOLDER OUT_FOLDER --to (dwg|dxf|cdy|pdf|svg|png)   every drawing in a folder
//! cadaddy-cli verify FILE.cdy                     check every chunk's checksum, the index and the drawing (exit 1 if damaged)
//! cadaddy-cli check-import FILE.(cdy|dwg|dxf)     import accuracy check (exit 1 on fail)
//! cadaddy-cli run [FILE|--sample|--metric] [--script TEXT|--script-file F.scr] [--cmd 'id {json}']... [--cmd-file F.jsonl] [--save OUT] [--export OUT.png] [--window ... | --view ...]
//! cadaddy-cli plot IN_FOLDER OUT_FOLDER [--layouts NAME,...] [--stamp | --no-stamp]   batch plot: one PDF per drawing
//! cadaddy-cli commands [FILTER]                   command catalog (JSON)
//! cadaddy-cli mcp [--headless | --connect HOST:PORT [--lazy] [--launch]]  MCP server on stdio (the open app on the agent port, else headless; or bridged to a given app)
//! cadaddy-cli perf [N]                            timing table on a synthetic N-entity drawing
//! cadaddy-cli perf FILE                           canvas display-list build of a drawing, zoomed in step by step
//! cadaddy-cli sample (bracket|floorplan) OUT       write a built-in sample drawing
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// `println!` that doesn't panic when stdout is closed (`cadaddy-cli commands | head`) with
/// "failed printing to stdout: Broken pipe (os error 32)". Once stdout has failed, later output is
/// dropped and the work goes on: see `stdout_failed`.
macro_rules! outln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        if !$crate::stdout_gone() {
            if let Err(e) = writeln!(std::io::stdout(), $($arg)*) {
                $crate::stdout_failed(e);
            }
        }
    }};
}

use std::process::ExitCode;

use cadaddy_engine::doc::{Handle, Space};
use cadaddy_engine::geom::{Bounds2, Vec2};
use cadaddy_engine::{Session, View};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

/// Set once a write to stdout has failed; later output is dropped.
static STDOUT_GONE: AtomicBool = AtomicBool::new(false);
/// Set when stdout failed for a reason other than a closed reader: the exit status becomes 1.
static STDOUT_ERROR: AtomicBool = AtomicBool::new(false);

fn stdout_gone() -> bool {
    STDOUT_GONE.load(Ordering::Relaxed)
}

/// stdout went away. The program doesn't stop there: the rest of `run` (later `--cmd` results,
/// `--save`, `--export`) still happens, so `cadaddy-cli run … --save out.dxf | head` keeps the
/// drawing. A reader that stopped early (a closed pipe) is not an error, as in ripgrep; any other
/// write error is reported and makes the exit status 1.
fn stdout_failed(e: std::io::Error) {
    if STDOUT_GONE.swap(true, Ordering::Relaxed) || e.kind() == std::io::ErrorKind::BrokenPipe {
        return;
    }
    eprintln!("cadaddy-cli: can't write to stdout: {e}");
    STDOUT_ERROR.store(true, Ordering::Relaxed);
}

const USAGE: &str = "usage:
  cadaddy-cli help [COMMAND]    workflows, or one command's summary, parameters and example
  cadaddy-cli guide             every workflow with its MCP calls and shell commands
  cadaddy-cli info FILE.(dwg|dxf|cdy) [--full]
  cadaddy-cli convert IN.(dwg|dxf|cdy) OUT.(dwg|dxf|cdy|svg|png|pdf) [FRAMING]
  cadaddy-cli convert IN_FOLDER OUT_FOLDER --to (dwg|dxf|cdy|svg|png|pdf) [--recurse] [FRAMING]
  cadaddy-cli verify FILE.cdy
  cadaddy-cli bim FILE.pdf|FOLDER [--out X.cdy] [--force]
  cadaddy-cli check-import FILE.(cdy|dwg|dxf) [--json]
  cadaddy-cli run [FILE | --sample | --metric] [--script TEXT] [--script-file F.scr] [--cmd 'id {json}']... [--cmd-file F (one `id {json}` per line)] [--save OUT.dxf] [--export OUT.(png|svg|pdf)] [FRAMING]
  cadaddy-cli plot IN_FOLDER OUT_FOLDER [--layouts NAME,...] [--stamp | --no-stamp]
  cadaddy-cli accuracy MODEL.cdy [--config C.json] [--commit SHA] [--job-s S] [--peak-mb MB] [--trend T.jsonl ...] (exit 2: a regression)
  cadaddy-cli commands [FILTER]
  cadaddy-cli mcp [--headless | --connect HOST:PORT [--lazy] [--no-launch]]
  cadaddy-cli perf [N | FILE]
  cadaddy-cli sample (bracket|floorplan) OUT.(dxf|dwg|svg|png)
  cadaddy-cli --version

image framing (png 2400x1600, svg, pdf; always model space):
  default                  convert: fit the drawing extents.
                           run --export: the current view if the script/commands changed it
                           (zoom, pan, view.set...), otherwise fit the drawing extents.
  --window x0,y0,x1,y1     show this rectangle (drawing units, x0<x1, y0<y1), centred and
                           scaled to fit the image without distortion.
  --view extents           always fit the drawing extents.
  --view current           the current view (as last set by zoom/pan/view.set; a freshly
                           opened drawing's view is its extents plus a 10% margin).

--cmd chaining (run):
  A --cmd parameter value of exactly \"{last}\" is replaced with the last object handle an
  earlier --cmd in the same run returned, so a command can act on what the previous one made:
    --cmd 'mview {\"p1\":[20,20],\"p2\":[200,150]}'        prints {\"viewports\":[\"100\"]}
    --cmd 'viewport.set {\"handle\":\"{last}\",\"scale\":1}' changes viewport 100
  It works in any parameter, at any depth (e.g. \"handles\":[\"{last}\"]); every other string is
  passed unchanged. With no earlier returned handle the run fails and says so.";

/// One `cadaddy-cli` subcommand for `cadaddy-cli help`: its usage, what it does, an example.
struct CliCommand {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    example: &'static str,
}

/// Every subcommand `cli` dispatches (a test keeps the two in step).
const CLI_COMMANDS: &[CliCommand] = &[
    CliCommand {
        name: "help",
        usage: "cadaddy-cli help [COMMAND]",
        summary: "This help: the main workflows, or one command's summary, parameters and an example. COMMAND is a \
                  cadaddy-cli subcommand (bim, run, convert…) or any drawing command (line, bim.takeoff, job.start…). \
                  Also --help and -h, and `cadaddy-cli COMMAND --help`.",
        example: "cadaddy-cli help bim.takeoff",
    },
    CliCommand {
        name: "guide",
        usage: "cadaddy-cli guide",
        summary: "Every workflow step by step, each step with its MCP call and its cadaddy-cli command (the same text as the MCP `workflows` tool).",
        example: "cadaddy-cli guide",
    },
    CliCommand {
        name: "info",
        usage: "cadaddy-cli info FILE.(dwg|dxf|cdy) [--full]",
        summary: "A short JSON summary of a drawing: format, entities, layers, blocks, units, extents. --full gives the detailed JSON \
                  (with a .cdy file's manifest and chunk sizes).",
        example: "cadaddy-cli info plan.dwg",
    },
    CliCommand {
        name: "convert",
        usage: "cadaddy-cli convert IN.(dwg|dxf|cdy) OUT.(dwg|dxf|cdy|svg|png|pdf) [FRAMING]\n  \
                cadaddy-cli convert IN_FOLDER OUT_FOLDER --to (dwg|dxf|cdy|svg|png|pdf) [--recurse] [FRAMING]",
        summary: "Converts one drawing, or every drawing in a folder, by the output's extension. FRAMING (--window, --view) picks what an image shows.",
        example: "cadaddy-cli convert drawings converted --to cdy",
    },
    CliCommand {
        name: "verify",
        usage: "cadaddy-cli verify FILE.cdy",
        summary: "Checks every chunk's checksum, the index and the drawing of a .cdy file; exit status 1 if it is damaged.",
        example: "cadaddy-cli verify model.cdy",
    },
    CliCommand {
        name: "bim",
        usage: "cadaddy-cli bim FILE.pdf|FOLDER [--out X.cdy] [--force]",
        summary: "PDF prints to a BIM model in one call: imports the prints, checks the import, drafts a building spec, builds the \
                  model and saves it (--out names the .cdy). A failed import check stops the build unless --force. Prints JSON \
                  with the saved paths, check results, built counts and the next steps.",
        example: "cadaddy-cli bim prints --out model.cdy",
    },
    CliCommand {
        name: "check-import",
        usage: "cadaddy-cli check-import FILE.(cdy|dwg|dxf) [--json]",
        summary: "The import accuracy check on an imported drawing; exit status 1 when it fails.",
        example: "cadaddy-cli check-import model.cdy --json",
    },
    CliCommand {
        name: "run",
        usage: "cadaddy-cli run [FILE | --sample | --metric] [--script TEXT] [--script-file F.scr] [--cmd 'id {json}']... \
                [--cmd-file F] [--save OUT] [--export OUT.(png|svg|pdf)] [FRAMING]",
        summary: "Opens a drawing (or a new one), runs commands in order and prints each result as JSON, then saves or exports. \
                  --cmd takes any drawing command with JSON parameters (`cadaddy-cli help COMMAND` shows them); --cmd-file \
                  reads one `id {json}` per line; --script runs command-line text.",
        example: "cadaddy-cli run model.cdy --cmd 'bim.takeoff {\"group_by\":\"level\"}'",
    },
    CliCommand {
        name: "accuracy",
        usage: "cadaddy-cli accuracy MODEL.cdy [--config C.json] [--commit SHA] [--label TEXT] [--job-s S] [--peak-mb MB] [--row OUT.json]                 [--trend T.jsonl [--merge-log LOG] [--commits FILE] [--last N] [--table OUT.md] [--svg OUT.svg]] [--force]",
        summary: "Measures one built model with no agent (validate.measure: Gate A, G-COVERAGE, every prescreen rule over the building                   and the fixed rooms, the takeoff by trade and system, elements by kind, job time and peak memory) and prints the                   row keyed by --commit; with --trend appends it to the trend file and compares it with the build before                   (validate.trend): exit status 2 when a metric regressed beyond its tolerance. --config holds the fixed rooms and                   defaults: {rooms, skip_rules, label, trend, merge_log, last, table, svg}; --commits is `git log --oneline` of the                   commits between the two builds.",
        example: "cadaddy-cli accuracy built.cdy --config trend-config.json --commit 1a2b3c4 --job-s 1800 --peak-mb 3100 --trend trend.jsonl",
    },
    CliCommand {
        name: "plot",
        usage: "cadaddy-cli plot IN_FOLDER OUT_FOLDER [--layouts NAME,...] [--stamp | --no-stamp]",
        summary: "Batch plot: one PDF per drawing in a folder.",
        example: "cadaddy-cli plot drawings pdfs",
    },
    CliCommand {
        name: "commands",
        usage: "cadaddy-cli commands [FILTER]",
        summary: "The command catalog as JSON (id, label, menu, params), filtered by id or label.",
        example: "cadaddy-cli commands takeoff",
    },
    CliCommand {
        name: "mcp",
        usage: "cadaddy-cli mcp [--headless | --connect HOST:PORT [--lazy] [--no-launch]]",
        summary: "An MCP server on stdio for AI agents: the open app on the agent port, else headless; or bridged to a given app.",
        example: "cadaddy-cli mcp --headless",
    },
    CliCommand {
        name: "perf",
        usage: "cadaddy-cli perf [N | FILE]",
        summary: "A timing table on a synthetic N-entity drawing, or the display-list build of a drawing zoomed in step by step.",
        example: "cadaddy-cli perf 100000",
    },
    CliCommand {
        name: "sample",
        usage: "cadaddy-cli sample (bracket|floorplan) OUT.(dxf|dwg|svg|png)",
        summary: "Writes a built-in sample drawing.",
        example: "cadaddy-cli sample floorplan floorplan.dxf",
    },
    CliCommand { name: "--version", usage: "cadaddy-cli --version", summary: "The version.", example: "cadaddy-cli --version" },
];

/// `cadaddy-cli help`: the workflows with copy-paste commands, then where to look next.
fn help_overview() -> String {
    let more = [
        "More:",
        "  cadaddy-cli guide              every workflow step by step, with the MCP calls",
        "  cadaddy-cli help COMMAND       a subcommand (bim, run, convert…) or a drawing command (bim.takeoff, job.start,",
        "                                 line…): its summary, parameters and an example",
    ]
    .join("\n");
    format!(
        "cadaddy-cli {} — CADaddy without a window.\n\n\
         Main workflows (copy-paste; `prints` is a PDF or a folder of PDFs, `costs.csv` a cost table):\n{}\n{more}\n  \
         cadaddy-cli commands [FILTER]  find drawing commands ({} in all)\n\n{USAGE}",
        env!("CARGO_PKG_VERSION"),
        cadaddy_engine::workflows::summary_text(),
        cadaddy_engine::command_specs().len(),
    )
}

/// `cadaddy-cli help NAME`: a subcommand's usage, summary and example, and/or a drawing
/// command's help page (summary, parameters, example).
fn help_for(name: &str) -> Result<String, String> {
    let mut out = Vec::new();
    let bare = name.trim_start_matches("--");
    if let Some(c) = CLI_COMMANDS.iter().find(|c| c.name == name || c.name.trim_start_matches("--") == bare) {
        out.push(format!("cadaddy-cli {}\n\n{}\n\nUsage:\n  {}\n\nExample:\n  {}\n", c.name, c.summary, c.usage, c.example));
    }
    if let Some(spec) = cadaddy_engine::cmd::find_by_id_or_alias(name) {
        let page = spec.help_page();
        let mut t = format!("{} — {}\n\n{}\n", page.id, page.label, page.help_text);
        let params = if page.params.trim().is_empty() { "{} (none)" } else { page.params };
        t.push_str(&format!("\nParameters (JSON):\n  {params}\n"));
        let mut facts = Vec::new();
        if !page.aliases.is_empty() {
            facts.push(format!("aliases {}", page.aliases.join(", ")));
        }
        if !page.menu_path.is_empty() {
            facts.push(format!("menu {}", page.menu_path));
        }
        if !page.shortcut.is_empty() {
            facts.push(format!("shortcut {}", page.shortcut));
        }
        if !page.prompts.is_empty() {
            facts.push(format!("asks first: {}", page.prompts.join(" ")));
        }
        if !facts.is_empty() {
            t.push_str(&format!("\n{}\n", facts.join("; ")));
        }
        t.push_str(&format!("\nExample:\n  {}\n  MCP: {}\n", page.example, page.mcp_example));
        if page.generated {
            t.push_str("  (example values come from the parameter list: replace them with yours)\n");
        }
        if !out.is_empty() {
            t = format!("Drawing command (run it with `cadaddy-cli run … --cmd`):\n\n{t}");
        }
        out.push(t);
    }
    if out.is_empty() {
        let l = name.to_ascii_lowercase();
        // The closest first (the shortest names holding what was typed), not the registry's order.
        let mut near: Vec<String> = cadaddy_engine::command_specs().iter().filter(|c| c.id.contains(l.as_str())).map(|c| c.id.to_string()).collect();
        near.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        near.truncate(10);
        let hint = if near.is_empty() { String::new() } else { format!(" Did you mean: {}?", near.join(", ")) };
        return Err(format!(
            "help: no subcommand or drawing command `{name}`.{hint} `cadaddy-cli help` lists the subcommands; `cadaddy-cli commands FILTER` finds drawing commands."
        ));
    }
    Ok(out.join("\n"))
}

fn help(args: &[String]) -> Result<(), String> {
    match args {
        [] => outln!("{}", help_overview()),
        [name] => outln!("{}", help_for(name)?),
        _ => return Err("usage: cadaddy-cli help [COMMAND]".into()),
    }
    Ok(())
}

/// What part of model space an image export shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Framing {
    /// `run`: the current view if the commands changed it, else the extents. `convert`: extents.
    Auto,
    Extents,
    Current,
    Window(Bounds2),
}

/// Parse `--window x0,y0,x1,y1` (min corner first). Hostile values are errors, never panics.
fn parse_window(text: &str) -> Result<Bounds2, String> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    let [x0, y0, x1, y1] = parts.as_slice() else {
        return Err(format!("--window needs four numbers x0,y0,x1,y1, got `{text}`"));
    };
    let num = |name: &str, v: &str| -> Result<f64, String> {
        match v.parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(n),
            _ => Err(format!("--window: {name} must be a finite number, got `{v}`")),
        }
    };
    let (x0, y0, x1, y1) = (num("x0", x0)?, num("y0", y0)?, num("x1", x1)?, num("y1", y1)?);
    if !(x0 < x1 && y0 < y1) {
        return Err(format!("--window: need x0 < x1 and y0 < y1 (lower-left corner first), got {x0},{y0},{x1},{y1}"));
    }
    let w = Bounds2 { min: Vec2::new(x0, y0), max: Vec2::new(x1, y1) };
    cadaddy_io::check_window(&w).map_err(|e| format!("--window: {e}"))?;
    Ok(w)
}

/// Parse a framing option (`--window`, `--view`) at `a`, taking its value from `it`. `Ok(false)`
/// means `a` is not a framing option.
fn framing_option<'a>(a: &str, it: &mut impl Iterator<Item = &'a String>, framing: &mut Framing) -> Result<bool, String> {
    let f = match a {
        "--window" => Framing::Window(parse_window(it.next().ok_or("--window needs x0,y0,x1,y1")?)?),
        "--view" => match it.next().map(|v| v.to_ascii_lowercase()).as_deref() {
            Some("extents") => Framing::Extents,
            Some("current") => Framing::Current,
            _ => return Err("--view needs `extents` or `current`".into()),
        },
        _ => return Ok(false),
    };
    if *framing != Framing::Auto && std::mem::discriminant(framing) != std::mem::discriminant(&f) {
        return Err("use only one of --window and --view".into());
    }
    *framing = f;
    Ok(true)
}

/// The model-space view of every open drawing, by drawing id (to tell whether commands changed it).
fn model_views(s: &Session) -> Vec<(u64, Option<View>)> {
    s.docs.iter().map(|st| (st.uid, st.views.iter().find(|(sp, _)| *sp == Space::Model).map(|(_, v)| *v))).collect()
}

/// The active drawing's current model-space view as a rectangle with the viewport's aspect ratio
/// (the CLI viewport is 3:2 like the PNG export, so the image shows exactly this view).
fn current_window(s: &Session) -> Option<Bounds2> {
    let st = s.state().ok()?;
    let v = st.views.iter().find(|(sp, _)| *sp == Space::Model).map(|(_, v)| *v)?;
    let (w, h) = s.viewport_px;
    let half = Vec2::new(v.height * w / h.max(1.0), v.height) * 0.5;
    Some(Bounds2::new(v.center - half, v.center + half))
}

/// The window an image export shows, or `None` to fit the extents. `before` is [`model_views`]
/// taken before the commands ran (`None` for `convert`, where nothing ran).
fn export_window(s: &Session, framing: Framing, before: Option<&[(u64, Option<View>)]>) -> Result<Option<Bounds2>, String> {
    match framing {
        Framing::Window(w) => Ok(Some(w)),
        Framing::Extents => Ok(None),
        Framing::Current => current_window(s).map(Some).ok_or_else(|| "--view current: the drawing has no model-space view".into()),
        Framing::Auto => {
            let Some(before) = before else { return Ok(None) };
            let Ok(st) = s.state() else { return Ok(None) };
            let now = model_views(s);
            let was = before.iter().find(|(uid, _)| *uid == st.uid);
            let is = now.iter().find(|(uid, _)| *uid == st.uid);
            // Only a view the commands changed counts; a drawing opened by the commands keeps
            // the extents framing.
            match (was, is) {
                (Some(a), Some(b)) if a.1 != b.1 => Ok(current_window(s)),
                _ => Ok(None),
            }
        }
    }
}

fn install_io() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: Some(cadaddy_io::read_dwf_page),
        write_dwfx: Some(cadaddy_io::write_dwfx_sheets),
    });
    cadaddy_engine::cmd::pointcloud::set_reader(cadaddy_io::pointcloud_io::read);
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    // .cdy saves append only what changed to the file a drawing was saved to or read from.
    cadaddy_engine::cmd::file::set_cdy_hooks(cadaddy_engine::cmd::file::CdyHooks {
        save: |d, prev| {
            use cadaddy_engine::cmd::file::CdyWrite;
            let prev = prev.and_then(|p| p.as_ref().downcast_ref::<cadaddy_io::cdy::SaveState>());
            let (out, state) = cadaddy_io::cdy::save(d, prev, &|| cadaddy_io::cdy::preview(d))?;
            let out = match out {
                cadaddy_io::cdy::Output::Full(bytes) => CdyWrite::Full(bytes),
                cadaddy_io::cdy::Output::Append { from, bytes, index_at } => CdyWrite::Append { from, bytes, index_at },
            };
            Ok((out, std::sync::Arc::new(state)))
        },
        seed: |d, bytes| cadaddy_io::cdy::seed(d, bytes).map(|s| std::sync::Arc::new(s) as cadaddy_engine::cmd::file::CdyState),
    });
    cadaddy_engine::cmd::file::set_pdf_merge(|parts| cadaddy_io::pdf_merge::merge(parts).map_err(|e| e.to_string()));
    cadaddy_engine::cmd::file::set_write_as(|d, name, o| {
        cadaddy_io::write_with(d, name, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
    // AutoLISP's entget, entmake and entmod see entities as DXF groups.
    cadaddy_engine::cmd::file::set_entity_codec(cadaddy_engine::cmd::file::EntityCodec {
        groups: cadaddy_io::entity_groups,
        entity: cadaddy_io::entity_from_groups,
        records: cadaddy_io::entity_records,
        attrib: cadaddy_io::attrib_from_groups,
    });
}

fn open(s: &mut Session, path: &str) -> Result<Value, String> {
    s.execute("open", &json!({ "path": path })).map_err(|e| e.to_string())
}

#[derive(Clone, Copy)]
struct OptionSpec {
    name: &'static str,
    takes_value: bool,
}

fn validate_options(command: &str, args: &[String], specs: &[OptionSpec]) -> Result<(), String> {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if let Some(name) = arg.strip_prefix("--") {
            let Some(spec) = specs.iter().find(|spec| spec.name == arg) else {
                return Err(format!("{command}: unknown option --{name}"));
            };
            if spec.takes_value && args.get(i + 1).is_some_and(|value| !value.starts_with("--")) {
                i += 2;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    Ok(())
}

const NO_OPTIONS: &[OptionSpec] = &[];

/// One JSON document: the drawing's summary, and for a .cdy file its container too (manifest,
/// what each kind of chunk takes, the bytes earlier saves left behind). `full` adds entity
/// details and container internals; the default is a compact summary.
fn info(path: &str, full: bool) -> Result<(), String> {
    let v = info_value(path, full)?;
    outln!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    Ok(())
}

/// The document [`info`] prints. What opening the file said about it (a damaged index, parts it
/// lost) is in `warnings`, as `open` reports it.
fn info_value(path: &str, full: bool) -> Result<Value, String> {
    let container = std::fs::read(path).ok().and_then(|bytes| {
        let m = cadaddy_io::cdy::manifest(&bytes)?;
        let check = cadaddy_io::cdy::verify(&bytes).unwrap_or_default();
        Some(json!({ "manifest": m, "chunks": check["sizes"], "waste": check["waste"], "bytes": bytes.len() }))
    });
    let mut s = Session::for_app();
    let opened = open(&mut s, path)?;
    let mut v = s.execute("drawing.inspect", &json!({ "entities": false })).map_err(|e| e.to_string())?;
    if let (Some(obj), Some(serde_json::Value::Object(c))) = (v.as_object_mut(), container) {
        obj.extend(c);
    }
    let warnings = opened.get("warnings").cloned().filter(|w| w.as_array().is_some_and(|a| !a.is_empty()));
    if let (Some(obj), Some(w)) = (v.as_object_mut(), &warnings) {
        obj.insert("warnings".into(), w.clone());
    }
    if full {
        return Ok(v);
    }
    // Short summary: file, format, entities, extents, units, layer/block counts.
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("?");
    let entities = v.get("entityCount").and_then(Value::as_u64).unwrap_or(0);
    let layers = v.get("layers").and_then(Value::as_array).map_or(0, Vec::len);
    let blocks = v.get("blocks").and_then(Value::as_array).map_or(0, Vec::len);
    let units = v.get("units").and_then(Value::as_str).unwrap_or("?");
    let extents = v.get("extents").cloned().unwrap_or(Value::Null);
    let mut short = json!({
        "file": path,
        "format": ext,
        "entities": entities,
        "layers": layers,
        "blocks": blocks,
        "units": units,
        "extents": extents,
        // Records a damaged file lost stay visible without --full.
        "skipped": v.get("skipped").cloned().unwrap_or(Value::Null),
    });
    if let (Some(obj), Some(w)) = (short.as_object_mut(), warnings) {
        obj.insert("warnings".into(), w);
    }
    Ok(short)
}

/// Write `bytes` to `out` through the engine's atomic writer (an exclusively created sibling file
/// renamed over `out`), so a failed or interrupted write leaves the previous file untouched.
fn write_atomic(out: &str, bytes: &[u8]) -> std::io::Result<()> {
    cadaddy_engine::atomic_write::write_atomic(std::path::Path::new(out), bytes)
}

fn export(s: &Session, out: &str) -> Result<(), String> {
    export_framed(s, out, None)
}

fn export_framed(s: &Session, out: &str, window: Option<Bounds2>) -> Result<(), String> {
    let d = s.doc().map_err(|e| e.to_string())?;
    let bytes = cadaddy_io::write_framed(d, out, window).map_err(|e| e.to_string())?;
    write_atomic(out, &bytes).map_err(|e| format!("{out}: {e}"))?;
    eprintln!("wrote {out} ({} bytes)", bytes.len());
    if cadaddy_engine::cmd::file::drops_unknown(out) {
        let lost = cadaddy_engine::cmd::file::not_saved(d);
        if let Some(m) = cadaddy_engine::cmd::file::not_saved_message(&lost) {
            eprintln!("{m}");
        }
    }
    Ok(())
}

fn sample(args: &[String]) -> Result<(), String> {
    let (Some(which), Some(out)) = (args.first(), args.get(1)) else { return Err(USAGE.into()) };
    let d = match which.as_str() {
        "bracket" => cadaddy_engine::sample::bracket(),
        "floorplan" | "floor" => cadaddy_engine::sample::floor_plan(),
        other => return Err(format!("unknown sample `{other}` (bracket, floorplan)")),
    };
    let bytes = cadaddy_io::write(&d, out).map_err(|e| e.to_string())?;
    write_atomic(out, &bytes).map_err(|e| format!("{out}: {e}"))?;
    eprintln!("wrote {out} ({} bytes)", bytes.len());
    Ok(())
}

/// Reject an explicit framing option without an image output to apply it to.
fn check_framed_output(framing: Framing, out: Option<&str>) -> Result<(), String> {
    match out {
        _ if framing == Framing::Auto => Ok(()),
        Some(o) if !cadaddy_io::is_image(o) => Err(format!("--window/--view only apply to image output (png, svg, pdf), not `{o}`")),
        Some(_) => Ok(()),
        None => Err("--window/--view need an image output (--export OUT.png|svg|pdf)".into()),
    }
}

/// Formats a conversion writes.
const OUTPUTS: [&str; 6] = ["dwg", "dxf", "cdy", "svg", "png", "pdf"];

fn convert(args: &[String]) -> Result<(), String> {
    let mut framing = Framing::Auto;
    let mut files = Vec::new();
    let mut to: Option<String> = None;
    let mut recurse = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--to" {
            to = Some(it.next().ok_or("--to needs a format (dwg, dxf, cdy, svg, png, pdf)")?.trim_start_matches('.').to_ascii_lowercase());
        } else if a == "--recurse" {
            recurse = true;
        } else if a.starts_with("--") && !framing_option(a, &mut it, &mut framing)? {
            return Err(format!("convert: unknown option {a}"));
        } else if !a.starts_with("--") {
            files.push(a);
        }
    }
    let [i, o] = files.as_slice() else { return Err(USAGE.into()) };
    if !std::path::Path::new(i).exists() {
        return Err(format!("{i}: no such file or folder"));
    }
    if std::path::Path::new(i).is_dir() {
        let to = to.ok_or("converting a folder needs --to FORMAT (dwg, dxf, cdy, svg, png, pdf)")?;
        return convert_folder(i, o, &to, recurse, framing);
    }
    if to.is_some() || recurse {
        return Err("--to and --recurse are for converting a folder; give the output file's name instead".into());
    }
    convert_file(i, o, framing)
}

fn convert_file(i: &str, o: &str, framing: Framing) -> Result<(), String> {
    check_framed_output(framing, Some(o))?;
    let mut s = Session::for_app();
    open(&mut s, i)?;
    if let Some(m) = skipped_message(s.doc().map_err(|e| e.to_string())?, i) {
        eprintln!("{m}");
    }
    if cadaddy_io::is_image(o)
        && let Some(m) = undrawn_message(s.doc().map_err(|e| e.to_string())?, i)
    {
        eprintln!("{m}");
    }
    let window = export_window(&s, framing, None)?;
    export_framed(&s, o, window)
}

/// Every drawing (.dwg, .dxf, .cdy) in folder `i` (and its subfolders with `recurse`, kept in the
/// output) converted to `to` in folder `o` (made when missing), each under its own name: the
/// engine's DWGCONVERT batch, with the image formats written by [`convert_file`]. A drawing that
/// fails is reported and the rest go on; the summary is printed as JSON and the exit status is 1
/// when any failed.
fn convert_folder(i: &str, o: &str, to: &str, recurse: bool, framing: Framing) -> Result<(), String> {
    use cadaddy_engine::cmd::file::convert;
    if !OUTPUTS.contains(&to) {
        return Err(format!("--to: unknown format `{to}` (dwg, dxf, cdy, svg, png, pdf)"));
    }
    check_framed_output(framing, Some(&format!("x.{to}")))?;
    let inputs = convert::folder_inputs(std::path::Path::new(i), recurse)?;
    // PDFs import through the same path as `convert IN.pdf OUT` (the batch job reads drawings only).
    let pdfs = convert::folder_inputs_of(std::path::Path::new(i), recurse, &["pdf"])?;
    if inputs.is_empty() && pdfs.is_empty() {
        return Err(format!("{i}: no drawings (.dwg, .dxf, .cdy, .pdf) in this folder"));
    }
    std::fs::create_dir_all(o).map_err(|e| format!("{o}: {e}"))?;
    let mut pdf_failed = 0usize;
    for p in &pdfs {
        let stem = p.path.file_stem().map(std::path::PathBuf::from).unwrap_or_default();
        let out = std::path::Path::new(o).join(&p.sub).join(stem).with_extension(to);
        let made = out.parent().map_or(Ok(()), std::fs::create_dir_all).map_err(|e| e.to_string());
        if let Err(e) = made.and_then(|()| convert_file(&p.path.to_string_lossy(), &out.to_string_lossy(), framing)) {
            eprintln!("{}: {e}", p.path.display());
            pdf_failed += 1;
        }
    }
    if inputs.is_empty() {
        return if pdf_failed == 0 { Ok(()) } else { Err(format!("{pdf_failed} of {} PDFs could not be converted", pdfs.len())) };
    }
    let job =
        convert::Job { inputs, to: to.to_string(), out: Some(o.into()), exists: convert::Exists::Overwrite, bak: false, ..convert::Job::default() };
    let image = |from: &std::path::Path, out: &std::path::Path, _: &convert::Job| {
        convert_file(&from.to_string_lossy(), &out.to_string_lossy(), framing).map(|()| convert::Report::default())
    };
    let converter: Option<convert::Converter> = if convert::TARGETS.contains(&to) { None } else { Some(&image) };
    let sum = convert::run(&job, converter, &std::sync::atomic::AtomicBool::new(false), &mut |_, o| {
        if o.status != convert::Status::Converted {
            eprintln!("{}: {}", o.from, o.reason.as_deref().unwrap_or_default());
        }
    });
    outln!("{}", serde_json::to_string_pretty(&sum.to_json()).unwrap_or_default());
    let failed = sum.count(convert::Status::Failed) + pdf_failed;
    if failed == 0 { Ok(()) } else { Err(format!("{failed} of {} drawings could not be converted", sum.total + pdfs.len())) }
}

/// `verify FILE.cdy`: the check as JSON; an error (exit status 1) when the file is damaged.
fn verify(path: &str) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let check = cadaddy_io::cdy::verify(&bytes).map_err(|e| format!("{path}: {e}"))?;
    outln!("{}", serde_json::to_string_pretty(&check).unwrap_or_default());
    match verify_failure(path, &check) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// Why `check` (from [`cadaddy_io::cdy::verify`]) is not a clean bill of health, in words; `None`
/// when it is clean. Bytes after the end of a sound file are named as that, not as damage.
fn verify_failure(path: &str, check: &Value) -> Option<String> {
    if check["ok"] == json!(true) {
        return None;
    }
    let damaged = |c: &Value| c.get("damage").is_some();
    let chunks = check["chunks"].as_array();
    if let Some(n) = check["trailing"].as_u64().filter(|n| *n > 0)
        && chunks.is_some_and(|c| !c.iter().any(damaged))
    {
        return Some(format!("{path}: {n} bytes of trailing data after the end of the file (the drawing before them is sound)"));
    }
    let bad = chunks.map_or(0, |c| c.iter().filter(|c| damaged(c) && c["live"] == json!(true)).count());
    let index = check["index"].as_str().unwrap_or("?");
    let notes = check["notes"].as_array().map_or(0, Vec::len);
    // Nothing in the drawing itself is damaged: say what is wrong instead of "0 damaged parts".
    if bad == 0 && index == "sound" && notes == 0 {
        if let Some(at) = check["appendedCopy"].as_u64() {
            return Some(format!("{path}: a second copy of the drawing is appended (from byte {at}); the first copy is sound"));
        }
        let lost = check["unreadable"].as_array().map_or(0, Vec::len);
        if lost > 0 {
            return Some(format!(
                "{path}: the drawing is sound, but {lost} stretch{} of the file can't be read as parts (see the report)",
                if lost == 1 { "" } else { "es" }
            ));
        }
    }
    // The parts are sound but reading left something out (the header's settings, say): say what.
    if bad == 0
        && index == "sound"
        && let Some(notes) = check["notes"].as_array().filter(|n| !n.is_empty())
    {
        let first = notes.first().and_then(Value::as_str).unwrap_or("something couldn't be read");
        let more = notes.len().saturating_sub(1);
        let tail = if more > 0 { format!(" (and {more} more in the report)") } else { String::new() };
        return Some(format!("{path}: {first}{tail}"));
    }
    Some(format!("{path}: damaged ({bad} damaged parts, index {index}; see the report)"))
}

/// `cadaddy-cli bim FILE.pdf|FOLDER [--out X.cdy] [--force]`: one-call PDF → BIM pipeline.
fn bim(args: &[String]) -> Result<(), String> {
    let mut path: Option<&str> = None;
    let mut out: Option<String> = None;
    let mut force = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(it.next().ok_or("--out needs a .cdy file path")?.clone()),
            "--force" => force = true,
            o if o.starts_with('-') => return Err(format!("bim: unknown option {o}")),
            f => {
                if path.is_some() {
                    return Err("bim takes one file or folder".into());
                }
                path = Some(f);
            }
        }
    }
    let path = path.ok_or("usage: cadaddy-cli bim FILE.pdf|FOLDER [--out X.cdy] [--force]")?;
    let mut s = Session::for_app();
    s.new_drawing_with_viewports(false);
    // The model is saved at --out (default: a temp folder); nothing goes next to the PDFs.
    let mut p = json!({ "path": path, "force": force });
    if let Some(o) = &out {
        p["out"] = json!(o);
    }
    let r = s.execute("bim.from_pdf", &p).map_err(|e| e.to_string())?;
    outln!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
    // A run that failed (the job didn't finish, a hard import-check fail, an empty storey) exits
    // non-zero even when --force saved its model.
    let failed: Vec<&str> = r.get("failed").and_then(Value::as_array).map(|f| f.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let cdy_paths = r.get("cdy_paths").and_then(Value::as_array).map_or(0, Vec::len);
    if !failed.is_empty() {
        Err(format!("bim: {}", failed.join("; ")))
    } else if cdy_paths == 0 {
        Err("no BIM models were built; see warnings in the output".into())
    } else {
        Ok(())
    }
}

/// `cadaddy-cli accuracy MODEL.cdy …`: the build measured (`validate.measure`), then with
/// `--trend` appended to the trend file and compared with the build before (`validate.trend`).
/// Prints the row, then the trend's answer (its table on stderr); true when a metric regressed.
fn accuracy(args: &[String]) -> Result<bool, String> {
    const OPTIONS: &[OptionSpec] = &[
        OptionSpec { name: "--config", takes_value: true },
        OptionSpec { name: "--commit", takes_value: true },
        OptionSpec { name: "--label", takes_value: true },
        OptionSpec { name: "--job-s", takes_value: true },
        OptionSpec { name: "--peak-mb", takes_value: true },
        OptionSpec { name: "--row", takes_value: true },
        OptionSpec { name: "--trend", takes_value: true },
        OptionSpec { name: "--merge-log", takes_value: true },
        OptionSpec { name: "--commits", takes_value: true },
        OptionSpec { name: "--last", takes_value: true },
        OptionSpec { name: "--table", takes_value: true },
        OptionSpec { name: "--svg", takes_value: true },
        OptionSpec { name: "--force", takes_value: false },
    ];
    validate_options("accuracy", args, OPTIONS)?;
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let force = args.iter().any(|a| a == "--force");
    let mut skip_next = false;
    let mut model = None;
    for a in args {
        if std::mem::take(&mut skip_next) {
            continue;
        }
        if let Some(spec) = OPTIONS.iter().find(|o| o.name == a) {
            skip_next = spec.takes_value;
        } else if model.is_none() {
            model = Some(a.clone());
        } else {
            return Err(format!("accuracy: one model only (also got {a})"));
        }
    }
    let model = model.ok_or("accuracy: name the built model (MODEL.cdy)")?;
    let config: Value = match value("--config") {
        Some(f) => {
            serde_json::from_str(&std::fs::read_to_string(&f).map_err(|e| format!("--config {f}: {e}"))?).map_err(|e| format!("--config {f}: {e}"))?
        }
        None => json!({}),
    };
    if !config.is_object() {
        return Err("--config holds a JSON object: {rooms, skip_rules, label, trend, merge_log, last, table, svg}".into());
    }
    let number = |name: &str| -> Result<Option<f64>, String> {
        value(name).map(|v| v.trim().parse::<f64>().map_err(|_| format!("{name} is a number, not {v}"))).transpose()
    };
    let mut m = json!({"force": force});
    for k in ["rooms", "skip_rules", "label"] {
        if !config[k].is_null() {
            m[k] = config[k].clone();
        }
    }
    for (flag, k) in [("--commit", "commit"), ("--label", "label"), ("--row", "path")] {
        if let Some(v) = value(flag) {
            m[k] = json!(v);
        }
    }
    for (flag, k) in [("--job-s", "job_s"), ("--peak-mb", "peak_mb")] {
        if let Some(v) = number(flag)? {
            m[k] = json!(v);
        }
    }
    let mut s = Session::for_app();
    open(&mut s, &model)?;
    let row = s.execute("validate.measure", &m).map_err(|e| e.to_string())?;
    outln!("{row}");
    let text_of = |flag: &str, key: &str| value(flag).or_else(|| config[key].as_str().map(str::to_string));
    let Some(trend) = text_of("--trend", "trend") else { return Ok(false) };
    let mut t = json!({"path": trend, "row": row, "force": force});
    for (flag, k) in [("--merge-log", "merge_log"), ("--table", "table"), ("--svg", "svg")] {
        if let Some(v) = text_of(flag, k) {
            t[k] = json!(v);
        }
    }
    if let Some(n) = number("--last")?.map(|n| json!(n as u64)).or_else(|| config.get("last").filter(|v| v.is_u64()).cloned()) {
        t["last"] = n;
    }
    if let Some(f) = value("--commits") {
        let text = std::fs::read_to_string(&f).map_err(|e| format!("--commits {f}: {e}"))?;
        t["commits"] = json!(text.lines().map(str::trim).filter(|l| !l.is_empty()).take(500).collect::<Vec<_>>());
    }
    let r = s.execute("validate.trend", &t).map_err(|e| e.to_string())?;
    if let Some(table) = r["table"].as_str() {
        eprintln!("{table}");
    }
    for f in r["findings"].as_array().into_iter().flatten().filter_map(|f| f["issue"].as_str()) {
        eprintln!("{f}");
    }
    outln!("{r}");
    Ok(r["regressions"].as_u64().is_some_and(|n| n > 0))
}

fn check_import(args: &[String]) -> Result<(), String> {
    let mut json_output = false;
    let mut file = None;
    for a in args {
        match a.as_str() {
            "--json" => json_output = true,
            o if o.starts_with('-') => return Err(format!("check-import: unknown option {o}")),
            f => {
                if file.is_some() {
                    return Err("check-import takes one file".into());
                }
                file = Some(f);
            }
        }
    }
    let path = file.ok_or("check-import FILE.cdy [--json]")?;
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let name = std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let d = cadaddy_io::read(&bytes, name).map_err(|e| format!("{path}: {e}"))?;
    let result = cadaddy_engine::cmd::import_check::check_drawing(&d);
    if json_output {
        outln!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    } else {
        outln!("{} (score {}/100)", if result.pass { "PASS" } else { "FAIL" }, result.score);
        for c in &result.checks {
            let mark = if c.pass { "ok" } else { "FAIL" };
            outln!("  [{mark}] {}: {}", c.id, c.found);
            if !c.pass && !c.hint.is_empty() {
                outln!("       hint: {}", c.hint);
            }
        }
    }
    if result.pass { Ok(()) } else { Err(format!("{path}: import check failed (score {}/100)", result.score)) }
}

/// `plot IN_FOLDER OUT_FOLDER`: every drawing in a folder plotted to one PDF each (PUBLISH's batch
/// plot). The summary is printed as JSON; the exit status is 1 when any drawing failed.
fn plot(args: &[String]) -> Result<(), String> {
    let mut dirs = Vec::new();
    let mut p = json!({});
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--layouts" => {
                let list = it.next().ok_or("--layouts needs names, separated by commas (Model for model space)")?;
                p["layouts"] = json!(list.split(',').map(str::trim).filter(|n| !n.is_empty()).collect::<Vec<_>>());
            }
            "--stamp" => p["stamp"] = json!(true),
            "--no-stamp" => p["stamp"] = json!(false),
            o if o.starts_with("--") => return Err(format!("plot: unknown option {o}")),
            d => dirs.push(d),
        }
    }
    let [input, output] = dirs.as_slice() else { return Err(USAGE.into()) };
    p["folder"] = json!(input);
    p["out"] = json!(output);
    // PUBLISH runs with a drawing open; the folder's drawings are read on their own.
    let mut s = Session::new();
    let r = s.execute("publish", &p).map_err(|e| e.to_string())?;
    outln!("{}", serde_json::to_string_pretty(&json!({ "plotted": r["drawings"], "failed": r["failed"] })).unwrap_or_default());
    let failed = r["failed"].as_array().map_or(0, Vec::len);
    if failed == 0 { Ok(()) } else { Err(format!("{failed} drawing(s) could not be plotted")) }
}

/// The warning for the model-space entities of `path` an image can't show, by type (those kept
/// as unknown objects, `Proxy` in `info`), or `None` when there are none.
fn undrawn_message(d: &cadaddy_engine::doc::Drawing, path: &str) -> Option<String> {
    let mut types: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for e in d.model.iter() {
        if let cadaddy_engine::doc::EntityKind::Unknown(u) = &e.kind {
            let n = types.entry(u.dxf_type.as_str()).or_insert(0);
            *n = n.saturating_add(1);
        }
    }
    let n = types.values().fold(0usize, |a, n| a.saturating_add(*n));
    if n == 0 {
        return None;
    }
    let list: Vec<String> = types.iter().map(|(t, n)| format!("{t}: {n}")).collect();
    let entities = if n == 1 { "entity" } else { "entities" };
    Some(format!("{path}: {n} {entities} not drawn ({})", list.join(", ")))
}

/// The warning for the records of `path` the reader could not place, or `None` when there are none.
fn skipped_message(d: &cadaddy_engine::doc::Drawing, path: &str) -> Option<String> {
    let n = d.skipped_records.values().fold(0usize, |a, n| a.saturating_add(*n));
    if n == 0 {
        return None;
    }
    let list: Vec<String> = d.skipped_records.iter().map(|(section, n)| format!("{section}: {n}")).collect();
    let records = if n == 1 { "record" } else { "records" };
    Some(format!("{path}: {n} {records} could not be read and are left out ({})", list.join(", ")))
}

/// A `--cmd` parameter value of exactly `{last}` is filled in with the last object handle an
/// earlier `--cmd` in the same run returned: `mview` answers `{"viewports":["100"]}`, so
/// `viewport.set {"handle":"{last}","scale":1}` changes viewport 100. Any other string is passed
/// through unchanged, and `--script` text is never rewritten.
const LAST_HANDLE: &str = "{last}";

/// How deep [`result_handles`] walks a command result (results are the engine's own JSON).
const RESULT_DEPTH: usize = 16;

/// Result keys whose values are object references: the engine's result convention for handles.
/// A handle can be a string (`handle`, `viewport`, `insert`…) or a list of them (`handles`,
/// `viewports`, `members`…). Every other field may hold a user-defined name, a message or a
/// count, and is never read as a handle — a group named `100` is a name, not object `100`.
fn handle_field(key: &str) -> bool {
    matches!(
        key,
        "handle"
            | "handles"
            | "viewport"
            | "viewports"
            | "boundary"
            | "clip"
            | "insert"
            | "members"
            | "source"
            | "objects"
            | "entities"
            | "selection"
            | "parts"
            | "refs"
            | "matches"
            | "line"
            | "arc"
            | "blocks"
    )
}

/// Collect the handles of `v`, a value under a handle field: a handle string, an array of them,
/// or objects that carry handle fields of their own (`entities`, `parts`).
fn collect_result_handles(s: &Session, v: &Value, depth: usize, out: &mut Vec<Handle>) {
    if depth > RESULT_DEPTH {
        return;
    }
    match v {
        Value::String(t) => {
            if let Some(h) = Handle::parse_hex(t)
                && s.doc().is_ok_and(|d| d.entity(h).is_some())
            {
                out.push(h);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_result_handles(s, x, depth + 1, out)),
        Value::Object(m) => {
            for (k, x) in m {
                if handle_field(k) {
                    collect_result_handles(s, x, depth + 1, out);
                } else {
                    find_result_handles(s, x, depth + 1, out);
                }
            }
        }
        _ => {}
    }
}

/// Find handle fields nested in containers that are not handles themselves (`groups` in
/// `groups.list`, whose members are handles): strings here are names, never references.
fn find_result_handles(s: &Session, v: &Value, depth: usize, out: &mut Vec<Handle>) {
    if depth > RESULT_DEPTH {
        return;
    }
    match v {
        Value::Array(a) => a.iter().for_each(|x| find_result_handles(s, x, depth + 1, out)),
        Value::Object(m) => {
            for (k, x) in m {
                if handle_field(k) {
                    collect_result_handles(s, x, depth + 1, out);
                } else {
                    find_result_handles(s, x, depth + 1, out);
                }
            }
        }
        _ => {}
    }
}

/// The object handles a command result mentions, in the order it lists them. A handle-only result
/// (a handle string or an array of them) counts as it is; otherwise only handle fields do.
fn result_handles(s: &Session, v: &Value, depth: usize, out: &mut Vec<Handle>) {
    match v {
        Value::String(_) => collect_result_handles(s, v, depth, out),
        Value::Array(a) if a.iter().all(Value::is_string) => collect_result_handles(s, v, depth, out),
        Value::Array(_) | Value::Object(_) => find_result_handles(s, v, depth, out),
        _ => {}
    }
}

/// The last handle `r` returned: what a later `{last}` stands for.
fn returned_handle(s: &Session, r: &Value) -> Option<Handle> {
    let mut hs = Vec::new();
    result_handles(s, r, 0, &mut hs);
    hs.last().copied()
}

/// The active drawing's identity ([`cadaddy_engine::DocState::uid`]): a handle is only meaningful
/// in the drawing it came from, and `new`, `open`, `document.switch` and `close` can make another
/// drawing current.
fn active_drawing(s: &Session) -> Option<u64> {
    s.docs.get(s.active).map(|st| st.uid)
}

/// Every `{last}` in `p`, at any depth, replaced by `h`. With no handle to fill in it is an
/// error, never a token reaching the command as a (bad) handle.
fn fill_last(p: &Value, h: Option<Handle>) -> Result<Value, String> {
    match p {
        Value::String(t) if t.as_str() == LAST_HANDLE => match h {
            Some(h) => Ok(Value::String(h.hex())),
            None => Err(format!("`{LAST_HANDLE}` needs an earlier --cmd in this run to have returned a handle")),
        },
        Value::Array(a) => a.iter().map(|x| fill_last(x, h)).collect::<Result<Vec<_>, _>>().map(Value::Array),
        Value::Object(m) => {
            m.iter().map(|(k, x)| Ok((k.clone(), fill_last(x, h)?))).collect::<Result<serde_json::Map<String, Value>, _>>().map(Value::Object)
        }
        other => Ok(other.clone()),
    }
}

/// The error a guided job's result reports (`job.start`, `job.next`, `job.answer`…), if any.
fn job_error(id: &str, r: &Value) -> Option<String> {
    if !id.starts_with("job.") || r.get("status").and_then(Value::as_str) != Some("error") {
        return None;
    }
    let stage = r.pointer("/error/stage").and_then(Value::as_str).unwrap_or("?");
    let message = r.pointer("/error/message").and_then(Value::as_str).unwrap_or("");
    Some(format!("the job stopped with an error in its {stage} stage: {message}"))
}

fn run(args: &[String]) -> Result<(), String> {
    let mut s = Session::for_app();
    let mut it = args.iter();
    let mut save = None;
    let mut exp: Option<String> = None;
    let mut framing = Framing::Auto;
    let mut steps: Vec<(String, String)> = Vec::new();
    while let Some(a) = it.next() {
        if framing_option(a, &mut it, &mut framing)? {
            continue;
        }
        match a.as_str() {
            "--sample" => {
                s.open_drawing(cadaddy_engine::sample::default_sample(), "Bracket", None);
            }
            "--metric" => {
                s.new_drawing(true);
            }
            "--script" => steps.push(("script".into(), it.next().ok_or("--script needs text")?.replace("\\n", "\n"))),
            "--script-file" => {
                let p = it.next().ok_or("--script-file needs a path")?;
                steps.push(("script".into(), std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?));
            }
            "--cmd" => steps.push(("cmd".into(), it.next().ok_or("--cmd needs `id {json}`")?.clone())),
            // One `id {json}` per line (PowerShell 5 strips the quotes inside a --cmd argument).
            "--cmd-file" => {
                let p = it.next().ok_or("--cmd-file needs a path")?;
                let bytes = std::fs::read(p).map_err(|e| format!("--cmd-file {p}: {e}"))?;
                let text = cmd_file_text(&bytes).map_err(|e| format!("--cmd-file {p}: {e}"))?;
                for (n, line) in text.lines().enumerate() {
                    let line = line.trim();
                    if !line.is_empty() && !line.starts_with('#') {
                        steps.push((format!("cmd {p}:{}", n + 1), line.to_string()));
                    }
                }
            }
            "--save" => save = Some(it.next().ok_or("--save needs a path")?.clone()),
            "--export" => exp = Some(it.next().ok_or("--export needs a path")?.clone()),
            f if !f.starts_with("--") => open(&mut s, f).map(|_| ())?,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if s.docs.is_empty() {
        s.new_drawing(false);
    }
    check_framed_output(framing, exp.as_deref())?;
    let before = model_views(&s);
    let mut last_handle: Option<Handle> = None;
    for (kind, body) in steps {
        // A handle belongs to the drawing that returned it: if this step makes another drawing
        // current (`new`, `open`, `document.switch`, `close`, or a script running one of them),
        // the remembered handle is stale. A handle the step itself returns is taken afterwards,
        // so it is a handle of the drawing it made current.
        let drawing = active_drawing(&s);
        match kind.as_str() {
            "script" => {
                s.script(&body).map_err(|e| e.to_string())?;
                if active_drawing(&s) != drawing {
                    last_handle = None;
                }
            }
            _ => {
                // `cmd FILE:LINE` for a --cmd-file line: errors say where it came from.
                let at = kind.strip_prefix("cmd ").map(|w| format!("{w}: ")).unwrap_or_default();
                let (id, json) = body.split_once(' ').unwrap_or((&body, "{}"));
                let params: Value = serde_json::from_str(json).map_err(|e| format!("{at}--cmd {id}: {e}"))?;
                let params = fill_last(&params, last_handle).map_err(|e| format!("{at}--cmd {id}: {e}"))?;
                let r = s.execute(id, &params).map_err(|e| format!("{at}{e}"))?;
                if active_drawing(&s) != drawing {
                    last_handle = None;
                }
                if let Some(h) = returned_handle(&s, &r) {
                    last_handle = Some(h);
                }
                if !r.is_null() {
                    outln!("{r}");
                }
                // A guided job that stopped in an error (no readable prints, a stage that
                // failed) is a failed run: a non-zero exit, not a quiet success.
                if let Some(err) = job_error(id, &r) {
                    return Err(format!("{at}{id}: {err}"));
                }
            }
        }
    }
    for l in &s.log {
        eprintln!("{l}");
    }
    if let Some(p) = save {
        // PDF images an import left in the session's temp folder go next to the saved drawing.
        for note in cadaddy_engine::cmd::file::adopt_session_images(&mut s, &p) {
            eprintln!("{note}");
        }
        export(&s, &p)?;
    }
    if let Some(p) = exp {
        let window = if cadaddy_io::is_image(&p) { export_window(&s, framing, Some(&before))? } else { None };
        export_framed(&s, &p, window)?;
    }
    Ok(())
}

/// The text of a --cmd-file: UTF-8 (a leading byte-order mark is dropped), or UTF-16 with a
/// byte-order mark, as Windows PowerShell 5's `>` and `Out-File` write it.
fn cmd_file_text(bytes: &[u8]) -> Result<String, String> {
    let utf16 = |big: bool, rest: &[u8]| {
        let (units, odd) = rest.as_chunks::<2>();
        if !odd.is_empty() {
            return Err("UTF-16 text with an odd number of bytes".to_string());
        }
        let units: Vec<u16> = units.iter().map(|b| if big { u16::from_be_bytes(*b) } else { u16::from_le_bytes(*b) }).collect();
        String::from_utf16(&units).map_err(|e| e.to_string())
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).map_err(|e| e.to_string()),
        [0xFF, 0xFE, rest @ ..] => utf16(false, rest),
        [0xFE, 0xFF, rest @ ..] => utf16(true, rest),
        _ => String::from_utf8(bytes.to_vec()).map_err(|_| "not UTF-8 or UTF-16 text (save it as UTF-8)".to_string()),
    }
}

/// Small deterministic generator (xorshift64*) so perf runs are repeatable without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
}

/// A synthetic drawing with `n` mixed entities (lines, circles, arcs, polylines, text) spread
/// over a square whose side grows with sqrt(n), so density stays constant.
fn synthetic(n: usize, side: f64) -> Result<cadaddy_engine::doc::Drawing, String> {
    use cadaddy_engine::doc::{self as d, Common, EntityKind, Space};
    use cadaddy_engine::geom::{PolyVertex, Vec2, Vec3};
    let mut dr = d::Drawing::new_imperial();
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let v3 = |x: f64, y: f64| Vec3::new(x, y, 0.0);
    for i in 0..n {
        let (x, y) = (r.range(0.0, side), r.range(0.0, side));
        let s = r.range(0.5, 8.0);
        let kind = match i % 5 {
            0 => EntityKind::Line(d::Line { a: v3(x, y), b: v3(x + r.range(-s, s), y + r.range(-s, s)) }),
            1 => EntityKind::Circle(d::Circle { center: v3(x, y), radius: s * 0.5 }),
            2 => EntityKind::Arc(d::Arc { center: v3(x, y), radius: s * 0.5, start: r.range(0.0, 3.0), end: r.range(3.2, 6.2) }),
            3 => EntityKind::LwPolyline(d::LwPolyline {
                vertices: (0..6)
                    .map(|k| PolyVertex {
                        p: Vec2::new(x + f64::from(k) * s * 0.3, y + r.range(-s, s) * 0.3),
                        bulge: if k == 2 { 0.4 } else { 0.0 },
                        start_width: 0.0,
                        end_width: 0.0,
                    })
                    .collect(),
                closed: i % 2 == 0,
                const_width: 0.0,
                elevation: 0.0,
                plinegen: false,
            }),
            _ => EntityKind::Text(d::Text {
                insert: v3(x, y),
                align_pt: None,
                height: s * 0.2,
                value: format!("T{i}"),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: d::HAlign::Left,
                valign: d::VAlign::Baseline,
            }),
        };
        dr.add(&Space::Model, Common::default(), kind).map_err(|e| e.to_string())?;
    }
    Ok(dr)
}

/// `perf FILE`: the display list built as the canvas builds it (a 1600 x 1000 px view at the
/// drawing's centre: tolerance, dash level of detail and view window from the zoom band), at
/// zoom extents and zoomed in 4x at a time, next to the full-detail build plots use.
fn perf_file(path: &str) -> Result<(), String> {
    use cadaddy_engine::geom::{Bounds2, Vec2};
    use cadaddy_engine::render::{Options, build};
    use std::time::Instant;
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let mut s = Session::for_app();
    let t = Instant::now();
    open(&mut s, path)?;
    let open_ms = ms(t);
    let st = s.state().map_err(|e| e.to_string())?;
    let (d, space) = (&st.doc, &st.space);
    let ext = d.extents(space);
    let size = ext.width().max(ext.height());
    if ext.is_empty() || !(size.is_finite() && size > 0.0) {
        return Err("perf: the drawing is empty".into());
    }
    outln!("open {open_ms:.0} ms, {} entities, extents {:.1} x {:.1}", d.entity_count(), ext.width(), ext.height());
    // The steps of a working session, each timed on its own.
    s.viewport_px = (1600.0, 1000.0);
    {
        let mut step = |name: &str, f: &mut dyn FnMut(&mut Session) -> Result<String, String>| -> Result<(), String> {
            let t = Instant::now();
            let note = f(&mut s)?;
            outln!("{name:<24} {:>9.1} ms  {note}", ms(t));
            Ok(())
        };
        step("first draw", &mut |s| {
            let st = s.state().map_err(|e| e.to_string())?;
            let v = st.view();
            let px = v.height / 1000.0;
            let o = Options {
                tolerance: px * 0.5,
                min_dash: px * 2.0,
                view: Some(Bounds2::new(v.center - Vec2::new(v.height * 0.8, v.height * 0.5), v.center + Vec2::new(v.height * 0.8, v.height * 0.5))),
                ..Default::default()
            };
            Ok(format!("{} prims", build(&st.doc, &st.space, &o).prims.len()))
        })?;
        step("zoom extents", &mut |s| s.zoom_extents().map(|_| String::new()).map_err(|e| e.to_string()))?;
        step("select all + move", &mut |s| {
            let hs: Vec<String> = s.doc().map_err(|e| e.to_string())?.model.iter().map(|e| e.handle.hex()).collect();
            let n = hs.len();
            s.execute("move", &serde_json::json!({ "handles": hs, "delta": [1, 1] })).map_err(|e| e.to_string())?;
            Ok(format!("{n} objects"))
        })?;
        step("regen", &mut |s| s.execute("regen", &serde_json::json!({})).map(|_| String::new()).map_err(|e| e.to_string()))?;
        step("save DWG", &mut |s| {
            let d = s.doc().map_err(|e| e.to_string())?;
            cadaddy_io::write(d, "perf.dwg").map(|b| format!("{:.1} MB", b.len() as f64 / 1e6)).map_err(|e| e.to_string())
        })?;
        step("plot (PDF)", &mut |s| {
            let d = s.doc().map_err(|e| e.to_string())?;
            cadaddy_io::write(d, "perf.pdf").map(|b| format!("{:.1} MB", b.len() as f64 / 1e6)).map_err(|e| e.to_string())
        })?;
    }
    let st = s.state().map_err(|e| e.to_string())?;
    let (d, space) = (&st.doc, &st.space);
    outln!("{:>8} {:>10} {:>10}", "zoom", "build ms", "prims");
    let mut finest = Options::default();
    for k in 0..5 {
        let zoom = 4f64.powi(k);
        let h = size / zoom;
        let pixel = 2f64.powi((h / 1000.0).max(1e-300).log2().floor() as i32);
        let half = Vec2::new(h * 0.8, h * 0.5);
        let visible = Bounds2::new(ext.center() - half, ext.center() + half);
        let screen = Options { tolerance: pixel * 0.5, min_dash: pixel * 2.0, view: Some(visible.expand(h * 0.8)), ..Default::default() };
        let t = Instant::now();
        let n = build(d, space, &screen).prims.len();
        outln!("{:>7}x {:>10.1} {n:>10}", zoom, ms(t));
        finest = screen;
    }
    // Plots: every pattern line, everywhere (last, so its large list doesn't weigh on the above).
    let t = Instant::now();
    let n = build(d, space, &Options { min_dash: 0.0, view: None, ..finest }).prims.len();
    outln!("{:>8} {:>10.1} {n:>10}", "full", ms(t));
    Ok(())
}

fn perf(args: &[String]) -> Result<(), String> {
    use cadaddy_engine::doc::Space;
    use cadaddy_engine::geom::{Bounds2, Vec2};
    use cadaddy_engine::{select, snap};
    use std::time::Instant;
    if let Some(path) = args.first().filter(|a| a.parse::<usize>().is_err()) {
        return perf_file(path);
    }
    let n: usize = match args.first() {
        Some(a) => a.parse().map_err(|_| format!("perf: bad entity count {a}"))?,
        None => 200_000,
    };
    let n = n.clamp(1, 20_000_000);
    let side = (n as f64).sqrt() * 10.0;
    let mut rows: Vec<(String, f64, String)> = Vec::new();
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let d = synthetic(n, side)?;
    rows.push(("build synthetic drawing".into(), ms(t), format!("{n} entities")));

    // Display list at a 1600 px wide "zoom extents" view (tolerance = half a pixel).
    let px = side / 1600.0;
    let opts = cadaddy_engine::render::Options { tolerance: px * 0.5, min_dash: px * 2.0, ..Default::default() };
    let t = Instant::now();
    let list = cadaddy_engine::render::build(&d, &Space::Model, &opts);
    rows.push(("display list build".into(), ms(t), format!("{} prims, {} segments", list.prims.len(), list.segment_count())));

    // Hit testing on a zoomed-in view (aperture = 5 px at 0.02 units/px).
    let ap = 0.1;
    let mut r = Rng(42);
    let pts: Vec<Vec2> = (0..1000).map(|_| Vec2::new(r.range(0.0, side), r.range(0.0, side))).collect();
    let t = Instant::now();
    let first = select::pick(&d, &Space::Model, pts.first().copied().unwrap_or(Vec2::ZERO), ap);
    rows.push(("first pick (incl. index build)".into(), ms(t), format!("{first:?}")));
    let t = Instant::now();
    let hits = pts.iter().filter(|p| select::pick(&d, &Space::Model, **p, ap).is_some()).count();
    let total = ms(t);
    rows.push(("pick x1000".into(), total, format!("{hits} hits, {:.3} ms/pick", total / 1000.0)));

    // One edit, then a pick: only the touched chunk is re-indexed.
    let mut edited = d.clone();
    if let Some(h) = edited.model.last().map(|e| e.handle) {
        edited.model.modify(h, |e| e.common.layer = "0".into());
    }
    let t = Instant::now();
    let _ = select::pick(&edited, &Space::Model, pts.first().copied().unwrap_or(Vec2::ZERO), ap);
    rows.push(("pick after one edit (incremental re-index)".into(), ms(t), String::new()));

    let mut sel = 0usize;
    let t = Instant::now();
    for k in 0..20 {
        let c = Vec2::new(r.range(0.0, side), r.range(0.0, side));
        let half = side * 0.05;
        sel += select::select_window(&d, &Space::Model, Bounds2::new(c - Vec2::new(half, half), c + Vec2::new(half, half)), k % 2 == 1).len();
    }
    rows.push(("window/crossing select x20 (10% side)".into(), ms(t), format!("{sel} selected")));

    let t = Instant::now();
    let fence: Vec<Vec2> = (0..4).map(|_| Vec2::new(r.range(0.0, side), r.range(0.0, side))).collect();
    let fsel = select::select_fence(&d, &Space::Model, &fence).len();
    rows.push(("fence select (3 legs)".into(), ms(t), format!("{fsel} selected")));

    let osmode = snap::mode::END | snap::mode::MID | snap::mode::CEN | snap::mode::QUA | snap::mode::INT;
    let t = Instant::now();
    let snaps = pts.iter().filter(|p| snap::osnap(&d, &Space::Model, **p, ap * 2.0, osmode, None, false).is_some()).count();
    let total = ms(t);
    rows.push(("osnap x1000".into(), total, format!("{snaps} hits, {:.3} ms/query", total / 1000.0)));

    let t = Instant::now();
    let bytes = cadaddy_io::write(&d, "perf.dxf").map_err(|e| e.to_string())?;
    rows.push(("DXF write".into(), ms(t), format!("{:.1} MB", bytes.len() as f64 / 1e6)));
    let t = Instant::now();
    let back = cadaddy_io::read(&bytes, "perf.dxf").map_err(|e| e.to_string())?;
    rows.push(("DXF read".into(), ms(t), format!("{} entities", back.entity_count())));

    outln!("{:<40} {:>12}  notes", "operation", "ms");
    outln!("{}", "-".repeat(80));
    for (name, t, note) in rows {
        outln!("{name:<40} {t:>12.2}  {note}");
    }
    Ok(())
}

/// The backend `cadaddy-cli mcp` serves from.
#[derive(Debug, PartialEq, Eq)]
enum McpTarget {
    /// The open CADaddy window on the agent port if it answers, else an in-process session (no
    /// arguments; what the Claude connector runs).
    Auto,
    /// An in-process drawing session (`--headless`).
    Headless,
    /// The running app's control channel at `HOST:PORT`; `lazy`: reached at the first call;
    /// `launch`: opened then if it isn't running.
    Connect { addr: String, lazy: bool, launch: bool },
}

/// Parses the arguments after `mcp`. `--connect` must be followed by a `HOST:PORT`; a missing,
/// empty or flag-like value is an error, never a silent fall back to a headless session (#103).
/// When no `--connect` is given, `CADADDY_CONTROL_PORT` and the legacy `CADADDY_CONTROL_PORT` are
/// checked: a port there becomes `--connect 127.0.0.1:PORT --launch`.
fn parse_mcp_args(args: &[String]) -> Result<McpTarget, String> {
    let mut addr: Option<String> = None;
    let (mut lazy, mut headless) = (false, false);
    let mut no_launch = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--connect" => {
                let a = it
                    .next()
                    .map(|v| v.trim())
                    .filter(|v| !v.is_empty() && !v.starts_with('-'))
                    .ok_or("--connect needs HOST:PORT (the app's --control port, e.g. 127.0.0.1:7979)")?;
                if addr.is_some() {
                    return Err("--connect given more than once".into());
                }
                addr = Some(a.to_string());
            }
            "--lazy" => lazy = true,
            "--headless" => headless = true,
            // Launch is the default for --connect; the flag stays accepted.
            "--launch" => {}
            "--no-launch" => no_launch = true,
            other => return Err(format!("mcp: unknown argument {other}\n{USAGE}")),
        }
    }
    // When no --connect is given, check CADADDY_CONTROL_PORT / CADADDY_CONTROL_PORT.
    if addr.is_none() && !headless {
        let port_str = std::env::var("CADADDY_CONTROL_PORT").or_else(|_| std::env::var("CADADDY_CONTROL_PORT")).ok().filter(|v| !v.trim().is_empty());
        if let Some(ps) = port_str {
            let p = ps.trim();
            if let Ok(port) = p.parse::<u16>() {
                addr = Some(format!("127.0.0.1:{port}"));
            }
        }
    }
    match addr {
        Some(_) if headless => Err("--headless and --connect exclude each other".into()),
        // `--launch` defaults on for --connect: the app opens at first call if not running.
        // Pass `--no-launch` to turn it off.
        Some(addr) => {
            let launch = !no_launch;
            Ok(McpTarget::Connect { addr, lazy: lazy || launch, launch })
        }
        None if lazy => Err("--lazy needs --connect HOST:PORT".into()),
        None if headless => Ok(McpTarget::Headless),
        None => Ok(McpTarget::Auto),
    }
}

/// Standard output shared by the MCP server and the event watcher: what one writes is held until
/// it flushes, then written whole under the lock, so their lines never interleave.
#[derive(Clone)]
struct SharedOut {
    out: std::sync::Arc<std::sync::Mutex<std::io::Stdout>>,
    buf: Vec<u8>,
}

impl std::io::Write for SharedOut {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let mut out = self.out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        out.write_all(&self.buf)?;
        self.buf.clear();
        out.flush()
    }
}

/// How often the event watcher asks the app for events.
const EVENT_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Send the app's events (once the client subscribed with `watch_events`) as they happen, read on
/// a connection of its own.
fn watch_events(addr: String, mut out: SharedOut) {
    let _ = std::thread::Builder::new().name("mcp-events".into()).spawn(move || {
        use std::io::Write;
        let mut since = 0u64;
        let mut conn: Option<cadaddy_mcp::Remote> = None;
        loop {
            std::thread::sleep(EVENT_POLL);
            if conn.is_none() {
                conn = cadaddy_mcp::Remote::connect(&addr).ok();
            }
            let Some(c) = conn.as_mut() else { continue };
            let (lines, next) = cadaddy_mcp::event_notifications(c, since);
            if next < since {
                // The app restarted or unsubscribed: start over.
                since = 0;
                continue;
            }
            since = next;
            for l in lines {
                let _ = out.write_all(l.as_bytes()).and_then(|()| out.write_all(b"\n"));
            }
            if out.flush().is_err() {
                return;
            }
        }
    });
}

fn mcp(args: &[String]) -> Result<(), String> {
    let mut watch: Option<String> = None;
    let backend: Box<dyn cadaddy_mcp::Backend> = match parse_mcp_args(args)? {
        McpTarget::Connect { addr, lazy: true, launch } => {
            watch = Some(addr.clone());
            Box::new(cadaddy_mcp::Remote::lazy(&addr, launch))
        }
        McpTarget::Connect { addr, .. } => match cadaddy_mcp::Remote::connect(&addr) {
            Ok(r) => {
                watch = Some(addr.clone());
                Box::new(r)
            }
            Err(e) => return Err(format!("cadaddy-cli: cannot connect to {addr}: {e}")),
        },
        McpTarget::Headless => Box::new(cadaddy_mcp::Headless::default()),
        McpTarget::Auto => Box::new(cadaddy_mcp::Auto::default()),
    };
    let stdin = std::io::stdin();
    let out = SharedOut { out: std::sync::Arc::new(std::sync::Mutex::new(std::io::stdout())), buf: Vec::new() };
    let mut server = cadaddy_mcp::Server::new(backend);
    if let Some(addr) = watch {
        watch_events(addr, out.clone());
        server = server.with_event_watcher();
    }
    server.serve_cancellable(std::io::BufReader::new(stdin), out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod mcp_args_tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mcp_without_connect_finds_the_app_or_runs_headless() {
        assert_eq!(parse_mcp_args(&args(&[])), Ok(McpTarget::Auto));
        assert_eq!(parse_mcp_args(&args(&["--headless"])), Ok(McpTarget::Headless));
        assert!(parse_mcp_args(&args(&["--headless", "--connect", "127.0.0.1:1"])).is_err());
    }

    #[test]
    fn mcp_connect_with_address_selects_remote() {
        let to = |addr: &str, lazy, launch| Ok(McpTarget::Connect { addr: addr.into(), lazy, launch });
        // --launch defaults ON: --connect alone gets launch=true, lazy=true.
        assert_eq!(parse_mcp_args(&args(&["--connect", "127.0.0.1:7979"])), to("127.0.0.1:7979", true, true));
        assert_eq!(parse_mcp_args(&args(&["--connect", "localhost:7979"])), to("localhost:7979", true, true));
        assert_eq!(parse_mcp_args(&args(&["--lazy", "--connect", "localhost:7979"])), to("localhost:7979", true, true));
        assert_eq!(parse_mcp_args(&args(&["--connect", "localhost:7979", "--launch"])), to("localhost:7979", true, true));
        // --no-launch turns it off.
        assert_eq!(parse_mcp_args(&args(&["--connect", "localhost:7979", "--no-launch"])), to("localhost:7979", false, false));
        // --lazy without --connect is still an error.
        assert!(parse_mcp_args(&args(&["--lazy"])).is_err());
    }

    #[test]
    fn mcp_connect_without_address_is_an_error_not_headless() {
        for bad in [&["--connect"][..], &["--connect", ""], &["--connect", "  "], &["--connect", "--foo"], &["--connect", "-v"]] {
            let r = parse_mcp_args(&args(bad));
            let Err(e) = r else { panic!("{bad:?} should be rejected, got {r:?}") };
            assert!(e.contains("--connect needs HOST:PORT"), "{bad:?}: {e}");
        }
    }

    #[test]
    fn mcp_rejects_unknown_and_repeated_arguments() {
        assert!(parse_mcp_args(&args(&["--conect", "127.0.0.1:7979"])).is_err());
        assert!(parse_mcp_args(&args(&["127.0.0.1:7979"])).is_err());
        assert!(parse_mcp_args(&args(&["--connect", "127.0.0.1:1", "--connect", "127.0.0.1:2"])).is_err());
    }
}

#[cfg(test)]
mod help_tests {
    use super::*;

    /// The subcommand names `cli` dispatches, read from its `Some("…")` arms.
    fn dispatched() -> Vec<String> {
        let src = include_str!("main.rs");
        let body = src.split("\nfn cli() -> ExitCode {").nth(1).unwrap();
        let body = body.split("\n}\n").next().unwrap();
        let mut names = Vec::new();
        for arm in body.split("Some(\"").skip(1) {
            for name in arm.split(')').next().unwrap().split('|') {
                let name = name.trim().trim_matches('"');
                if !name.is_empty() && !matches!(name, "-V" | "-h" | "--help") {
                    names.push(name.to_string());
                }
            }
        }
        names
    }

    #[test]
    fn every_subcommand_has_help() {
        let names = dispatched();
        assert!(names.len() >= 12, "{names:?}");
        for name in &names {
            let c = CLI_COMMANDS.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no CLI_COMMANDS entry for `{name}`"));
            assert!(!c.summary.is_empty() && c.example.starts_with("cadaddy-cli"), "{name}");
            let text = help_for(name).unwrap();
            assert!(text.contains("Usage:") && text.contains("Example:") && text.contains(c.summary), "{name}: {text}");
        }
        for c in CLI_COMMANDS {
            assert!(names.iter().any(|n| n == c.name), "`{}` has help but cli() doesn't run it", c.name);
        }
    }

    #[test]
    fn every_drawing_command_has_help_with_parameters_and_an_example() {
        for spec in cadaddy_engine::command_specs() {
            let text = help_for(spec.id).unwrap_or_else(|e| panic!("{}: {e}", spec.id));
            let page = spec.help_page();
            assert!(!page.help_text.trim().is_empty(), "{}: empty help text", spec.id);
            assert!(text.contains(&page.help_text), "{}", spec.id);
            assert!(text.contains("Parameters (JSON):") && text.contains("Example:\n  cadaddy-cli run "), "{}: {text}", spec.id);
            // The example's JSON parses.
            let json = page.example.split_once(&format!("--cmd '{} ", spec.id)).unwrap().1.split('\'').next().unwrap();
            assert!(serde_json::from_str::<Value>(json).unwrap().is_object(), "{}: {json}", spec.id);
        }
    }

    #[test]
    fn overview_lists_prints_to_estimate_with_commands() {
        let text = help_overview();
        let bim = text.find("cadaddy-cli bim prints").unwrap();
        let takeoff = text.find("bim.takeoff").unwrap();
        let estimate = text.find("bim.estimate").unwrap();
        assert!(bim < takeoff && takeoff < estimate, "{text}");
        assert!(text.contains("cadaddy-cli guide") && text.contains("cadaddy-cli help COMMAND"));
    }

    #[test]
    fn help_finds_aliases_and_refuses_unknown_names() {
        assert!(help_for("l").unwrap().starts_with("LINE — "));
        assert!(help_for("--version").unwrap().contains("cadaddy-cli --version"));
        let e = help_for("takeof").unwrap_err();
        assert!(e.contains("bim.takeoff"), "{e}");
        assert!(help_for("zzzz-nothing").is_err());
        assert!(help(&args(&["a", "b"])).is_err());
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }
}

fn main() -> ExitCode {
    let code = cli();
    if STDOUT_ERROR.load(Ordering::Relaxed) { ExitCode::FAILURE } else { code }
}

fn cli() -> ExitCode {
    install_io();
    // A guided job (job.start) says what it is doing while it runs, one line per stage and sheet.
    cadaddy_engine::cmd::guided::set_progress_sink(Some(|line| eprintln!("{line}")));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    let r = match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => help(&rest),
        // `cadaddy-cli bim --help`: the subcommand's help instead of running it.
        Some(sub) if matches!(rest.first().map(String::as_str), Some("--help" | "-h")) => help(&[sub.to_string()]),
        Some("guide") => validate_options("guide", &rest, NO_OPTIONS).map(|_| outln!("{}", cadaddy_engine::workflows::guide_text())),
        Some("info") => {
            const INFO_OPTIONS: &[OptionSpec] = &[OptionSpec { name: "--full", takes_value: false }];
            validate_options("info", &rest, INFO_OPTIONS).and_then(|_| {
                let full = rest.iter().any(|a| a == "--full");
                let path = rest.iter().find(|a| !a.starts_with("--")).ok_or_else(|| USAGE.to_string())?;
                info(path, full)
            })
        }
        Some("convert") => convert(&rest),
        Some("verify") => {
            validate_options("verify", &rest, NO_OPTIONS).and_then(|_| rest.first().ok_or_else(|| USAGE.to_string())).and_then(|p| verify(p))
        }
        Some("plot") => plot(&rest),
        Some("accuracy") => match accuracy(&rest) {
            // Measured, and worse than the build before: its own exit status.
            Ok(true) => return ExitCode::from(2),
            Ok(false) => Ok(()),
            Err(e) => Err(e),
        },
        Some("run") => run(&rest),
        Some("perf") => validate_options("perf", &rest, NO_OPTIONS).and_then(|_| perf(&rest)),
        Some("sample") => validate_options("sample", &rest, NO_OPTIONS).and_then(|_| sample(&rest)),
        Some("commands") => validate_options("commands", &rest, NO_OPTIONS).map(|_| {
            let s = Session::new();
            let f = rest.first().map(|x| x.to_ascii_lowercase());
            let v: Vec<Value> = cadaddy_engine::command_specs()
                .iter()
                .filter(|c| f.as_ref().is_none_or(|f| c.id.contains(f.as_str()) || c.label.to_ascii_lowercase().contains(f.as_str())))
                .map(|c| serde_json::to_value(c.info(&s)).unwrap_or_default())
                .collect();
            outln!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        }),
        Some("bim") => bim(&rest),
        Some("check-import") => check_import(&rest),
        Some("mcp") => mcp(&rest),
        Some("--version" | "-V") => {
            outln!("cadaddy-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => Err(format!("cadaddy-cli: unknown command `{other}` (`cadaddy-cli help` lists the commands and workflows)\n{USAGE}")),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B4-015: a job that stopped in an error is a failed run (non-zero exit); a done or paused
    /// one, or another command's `status`, is not.
    #[test]
    fn a_job_that_ends_in_an_error_fails_the_run() {
        let err =
            json!({"job_id": "j", "status": "error", "error": {"stage": "classify", "message": "None of the print set's 3 file(s) could be read"}});
        let e = job_error("job.start", &err).expect("an error");
        assert!(e.contains("classify") && e.contains("could be read"), "{e}");
        assert!(job_error("job.start", &json!({"status": "done"})).is_none());
        assert!(job_error("job.next", &json!({"status": "in_progress"})).is_none());
        assert!(job_error("layer.list", &json!({"status": "error"})).is_none());
    }

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cadaddy-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn write_atomic_replaces_file_and_leaves_no_staging_files() {
        let dir = scratch_dir("replace");
        let out = dir.join("a.dxf");
        let out = out.to_str().expect("utf-8 path");
        write_atomic(out, b"old").expect("create");
        write_atomic(out, b"new").expect("overwrite");
        assert_eq!(std::fs::read(out).expect("read"), b"new");
        assert_eq!(std::fs::read_dir(&dir).expect("list").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_failure_keeps_destination_and_cleans_up() {
        let dir = scratch_dir("fail");
        let out = dir.join("target");
        std::fs::create_dir(&out).expect("dir as destination");
        std::fs::write(out.join("keep"), b"x").expect("seed");
        assert!(write_atomic(out.to_str().expect("utf-8 path"), b"new").is_err());
        assert_eq!(std::fs::read(out.join("keep")).expect("kept"), b"x");
        assert_eq!(std::fs::read_dir(&dir).expect("list").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn b(x0: f64, y0: f64, x1: f64, y1: f64) -> Bounds2 {
        Bounds2::new(Vec2::new(x0, y0), Vec2::new(x1, y1))
    }

    fn sample_session() -> Session {
        let mut s = Session::empty();
        s.open_drawing(cadaddy_engine::sample::default_sample(), "Bracket", None);
        s
    }

    fn view_box(svg: &str) -> String {
        let i = svg.find("viewBox=\"").map(|i| i + 9).unwrap_or(0);
        svg[i..].split('"').next().unwrap_or("").to_string()
    }

    fn assert_near(a: Bounds2, e: Bounds2) {
        let d = (a.min - e.min).len().max((a.max - e.max).len());
        assert!(d < 1e-9, "{a:?} != {e:?}");
    }

    #[test]
    fn window_parses() {
        assert_eq!(parse_window("0,0,30,20"), Ok(b(0.0, 0.0, 30.0, 20.0)));
        assert_eq!(parse_window(" -5.5, -1e3 ,2e3,4 "), Ok(b(-5.5, -1000.0, 2000.0, 4.0)));
    }

    #[test]
    fn hostile_window_is_an_error() {
        for w in [
            "",
            "1,2,3",
            "1,2,3,4,5",
            "a,0,1,1",
            "NaN,0,1,1",
            "0,0,inf,1",
            "0,0,1e400,1",
            "0,0,0,1",
            "0,0,1,0",
            "5,0,1,1",
            "0,5,1,1",
            "-1e300,-1e300,1e300,1e300",
            "0,0,1e-12,1e-12",
        ] {
            assert!(parse_window(w).is_err(), "`{w}` should be rejected");
        }
    }

    #[test]
    fn framing_options() {
        let parse = |args: &[&str]| -> Result<Framing, String> {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let mut f = Framing::Auto;
            let mut it = args.iter();
            while let Some(a) = it.next() {
                assert!(framing_option(a, &mut it, &mut f)?, "{a} is a framing option");
            }
            Ok(f)
        };
        assert_eq!(parse(&["--view", "extents"]), Ok(Framing::Extents));
        assert_eq!(parse(&["--view", "CURRENT"]), Ok(Framing::Current));
        assert_eq!(parse(&["--window", "0,0,2,1"]), Ok(Framing::Window(b(0.0, 0.0, 2.0, 1.0))));
        assert!(parse(&["--view"]).is_err());
        assert!(parse(&["--view", "selection"]).is_err());
        assert!(parse(&["--window"]).is_err());
        assert!(parse(&["--window", "0,0,2,1", "--view", "extents"]).is_err());
        assert!(check_framed_output(Framing::Extents, Some("a.dxf")).is_err());
        assert!(check_framed_output(Framing::Extents, None).is_err());
        assert!(check_framed_output(Framing::Extents, Some("a.png")).is_ok());
        assert!(check_framed_output(Framing::Auto, Some("a.dxf")).is_ok());
    }

    #[test]
    fn last_placeholder_takes_the_previous_handle() {
        let mut s = sample_session();
        let r = s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
        let h = returned_handle(&s, &r).expect("LINE returns its handle");
        assert_eq!(fill_last(&json!({"handle": "{last}"}), Some(h)), Ok(json!({"handle": h.hex()})));
        // Nested, as in a selection list.
        assert_eq!(fill_last(&json!({"handles": ["{last}", "1"]}), Some(h)), Ok(json!({"handles": [h.hex(), "1"]})));
        // Only the exact token: a name or a sentence that merely contains it stays as written.
        for p in [json!("last"), json!("{last} "), json!({"name": "last"})] {
            assert_eq!(fill_last(&p, Some(h)), Ok(p.clone()));
        }
        // Nothing returned yet: an error, not a token passed to the command as a bad handle.
        assert!(fill_last(&json!({"handle": "{last}"}), None).unwrap_err().contains("{last}"));
    }

    /// Handles are read from a result by looking the strings up in the drawing: counts and names
    /// are not mistaken for handles, and `mview`'s `viewports` works like `handle`/`handles`.
    #[test]
    fn returned_handle_reads_only_real_handles() {
        let mut s = Session::empty();
        s.new_drawing(false);
        s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
        let r = s.execute("mview", &json!({"p1": [20, 20], "p2": [200, 150]})).unwrap();
        let h = returned_handle(&s, &r).expect("mview returns its viewport handle");
        assert!(r["viewports"].as_array().is_some_and(|a| a.iter().any(|v| v.as_str() == Some(h.hex().as_str()))), "{r}");
        assert_eq!(returned_handle(&s, &json!({"changed": 3, "name": "A1"})), None);
        // The most recent of several.
        let a = s.execute("line", &json!({"points": [[0, 0], [1, 0]]})).unwrap();
        let b = s.execute("line", &json!({"points": [[0, 0], [2, 0]]})).unwrap();
        let (ha, hb) = (returned_handle(&s, &a).unwrap(), returned_handle(&s, &b).unwrap());
        assert_eq!(returned_handle(&s, &json!({"handles": [ha.hex(), hb.hex()]})), Some(hb));
    }

    #[test]
    fn default_framing_is_extents() {
        // convert: nothing ran, always the extents.
        let s = sample_session();
        assert_eq!(export_window(&s, Framing::Auto, None), Ok(None));
        // run: commands that leave the view alone keep the extents framing.
        let mut s = sample_session();
        let before = model_views(&s);
        s.script("LINE 0,0 10,10\n\n").unwrap();
        assert_eq!(export_window(&s, Framing::Auto, Some(&before)), Ok(None));
        // A fresh drawing too.
        let mut s = Session::empty();
        s.new_drawing(false);
        let before = model_views(&s);
        s.execute("circle", &json!({"center": [0, 0], "radius": 5})).unwrap();
        assert_eq!(export_window(&s, Framing::Auto, Some(&before)), Ok(None));
    }

    #[test]
    fn zoom_window_frames_export() {
        let mut s = sample_session();
        let before = model_views(&s);
        s.execute("zoom", &json!({"mode": "window", "p1": [0, 0], "p2": [30, 20]})).unwrap();
        let w = export_window(&s, Framing::Auto, Some(&before)).unwrap().unwrap();
        // The CLI viewport is 3:2, so a 3:2 window is shown exactly.
        assert_near(w, b(0.0, 0.0, 30.0, 20.0));
        assert_eq!(export_window(&s, Framing::Current, Some(&before)), Ok(Some(w)));
        // --view extents and --window override the zoom.
        assert_eq!(export_window(&s, Framing::Extents, Some(&before)), Ok(None));
        let explicit = b(1.0, 1.0, 2.0, 2.0);
        assert_eq!(export_window(&s, Framing::Window(explicit), Some(&before)), Ok(Some(explicit)));
        // A taller window keeps its height; the 3:2 image adds room left and right.
        s.execute("zoom", &json!({"mode": "center", "center": [5, 5], "height": 4})).unwrap();
        assert_near(export_window(&s, Framing::Auto, Some(&before)).unwrap().unwrap(), b(2.0, 3.0, 8.0, 7.0));
    }

    #[test]
    fn zoom_object_frames_the_selection() {
        let mut s = sample_session();
        let before = model_views(&s);
        // The smallest entity with an area, selected then framed with ZOOM Object.
        let d = s.doc().unwrap();
        let bounds = |e: &cadaddy_engine::doc::Entity| cadaddy_engine::doc::entity_bounds(d, e, 0);
        let e = d
            .model
            .iter()
            .filter(|e| bounds(e).width() > 0.0 && bounds(e).height() > 0.0)
            .min_by(|a, b| bounds(a).width().total_cmp(&bounds(b).width()))
            .unwrap();
        let (h, eb, ext) = (e.handle, bounds(e), d.extents(&Space::Model));
        s.set_selection(vec![h]);
        s.execute("zoom", &json!({"mode": "object"})).unwrap();
        let w = export_window(&s, Framing::Auto, Some(&before)).unwrap().unwrap();
        assert!(w.contains(eb.min) && w.contains(eb.max));
        assert!(w.width() < ext.width() && w.height() < ext.height(), "{w:?} vs extents {ext:?}");
    }

    #[test]
    fn run_reads_commands_from_a_file() {
        let dir = std::env::temp_dir().join(format!("cadaddy-cli-cmdfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cmds = dir.join("cmds.jsonl");
        // A quoted JSON argument PowerShell 5 would mangle, plus a comment and a blank line.
        std::fs::write(&cmds, "# draw a line\n\nline {\"points\":[[0,0],[10,0]]}\nline {\"points\":[[0,0],[0,5]]}\n").unwrap();
        let out = dir.join("out.dxf");
        run(&["--cmd-file".into(), cmds.to_string_lossy().to_string(), "--save".into(), out.to_string_lossy().to_string()]).unwrap();
        let dxf = std::fs::read_to_string(&out).unwrap();
        assert_eq!(dxf.matches("\nLINE\n").count() + dxf.matches("\r\nLINE\r\n").count(), 2, "two lines drawn");
        assert!(run(&["--cmd-file".into(), dir.join("missing.jsonl").to_string_lossy().to_string()]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_export_honours_zoom() {
        let dir = std::env::temp_dir().join(format!("cadaddy-cli-165-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = |n: &str| dir.join(n).to_string_lossy().to_string();
        let run_svg = |extra: &[&str], out: &str| -> String {
            let mut args: Vec<String> = ["--sample"].iter().chain(extra).map(|a| a.to_string()).collect();
            args.extend(["--export".to_string(), path(out)]);
            run(&args).unwrap();
            std::fs::read_to_string(path(out)).unwrap()
        };
        let base = run_svg(&[], "base.svg");
        let zoomed = run_svg(&["--cmd", r#"zoom {"mode":"window","p1":[0,0],"p2":[30,20]}"#], "zoom.svg");
        let window = run_svg(&["--window", "0,0,30,20"], "window.svg");
        let forced = run_svg(&["--cmd", r#"zoom {"mode":"window","p1":[0,0],"p2":[30,20]}"#, "--view", "extents"], "forced.svg");
        assert_ne!(view_box(&zoomed), view_box(&base));
        assert_eq!(view_box(&zoomed), "0 0 30.000000 20.000000");
        assert_eq!(zoomed, window);
        assert_eq!(forced, base);
        // Hostile --window: a clear error, nothing written.
        let bad: Vec<String> = ["--sample", "--window", "0,0,NaN,1", "--export", &path("bad.png")].iter().map(|a| a.to_string()).collect();
        assert!(run(&bad).unwrap_err().contains("--window"));
        assert!(!dir.join("bad.png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn validators_accept_only_the_documented_options() {
        assert!(validate_options("info", &args(&["file.dxf"]), NO_OPTIONS).is_ok());
        assert!(validate_options("perf", &args(&["100"]), NO_OPTIONS).is_ok());
        assert!(validate_options("sample", &args(&["bracket", "out.dxf"]), NO_OPTIONS).is_ok());
        assert!(validate_options("commands", &args(&["line"]), NO_OPTIONS).is_ok());
        for (command, specs) in [("info", NO_OPTIONS), ("perf", NO_OPTIONS), ("sample", NO_OPTIONS), ("commands", NO_OPTIONS)] {
            assert!(validate_options(command, &args(&["--typo"]), specs).is_err());
        }
    }

    #[test]
    fn unknown_options_are_rejected_before_side_effects() {
        assert_eq!(validate_options("info", &args(&["missing.dxf", "--typo"]), NO_OPTIONS), Err("info: unknown option --typo".into()));
        assert_eq!(validate_options("perf", &args(&["1", "--typo"]), NO_OPTIONS), Err("perf: unknown option --typo".into()));
        assert_eq!(validate_options("sample", &args(&["bracket", "out.dxf", "--typo"]), NO_OPTIONS), Err("sample: unknown option --typo".into()));
    }
}

#[cfg(test)]
mod b4polish_tests {
    use super::*;

    fn temp_file(tag: &str, bytes: &[u8]) -> String {
        let dir = std::env::temp_dir().join(format!("cadaddy-cli-b4-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.cdy");
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// B4-028: `info` reports what opening a damaged file said, in the short summary too.
    #[test]
    fn info_reports_the_warnings_open_does() {
        let good = cadaddy_io::write(&cadaddy_engine::sample::floor_plan(), "a.cdy").unwrap();
        install_io();
        let path = temp_file("cut", &good[..good.len() / 2]);
        let mut s = Session::for_app();
        let opened = open(&mut s, &path).unwrap();
        let said = opened["warnings"].as_array().cloned().unwrap_or_default();
        assert!(!said.is_empty(), "open said nothing about a cut-short file: {opened}");
        for full in [false, true] {
            let v = info_value(&path, full).unwrap();
            assert_eq!(v["warnings"].as_array(), Some(&said), "full={full}: {v}");
        }
        // A sound file has none.
        let ok = temp_file("ok", &good);
        assert!(info_value(&ok, false).unwrap().get("warnings").is_none());
    }

    /// B4-031: `verify` on a file with only bytes appended says exactly that.
    #[test]
    fn verify_names_trailing_data() {
        let good = cadaddy_io::write(&cadaddy_engine::sample::floor_plan(), "a.cdy").unwrap();
        let check = cadaddy_io::cdy::verify(&good).unwrap();
        assert_eq!(verify_failure("f.cdy", &check), None);
        let mut longer = good.clone();
        longer.extend_from_slice(&[0u8; 1234]);
        let why = verify_failure("f.cdy", &cadaddy_io::cdy::verify(&longer).unwrap()).unwrap();
        assert!(why.contains("1234 bytes of trailing data after the end") && !why.contains("damaged"), "{why}");
        // Real damage is still damage.
        let mut hurt = good.clone();
        if let Some(b) = hurt.get_mut(200) {
            *b ^= 0xff;
        }
        let why = verify_failure("f.cdy", &cadaddy_io::cdy::verify(&hurt).unwrap()).unwrap();
        assert!(why.contains("damaged"), "{why}");
    }

    /// B9-016: `verify` on a file whose header can't be read says that, not "0 damaged parts".
    #[test]
    fn verify_names_an_unreadable_header() {
        let check = json!({"ok": false, "index": "sound", "chunks": [{"tag": "HEAD", "live": true}], "notes": ["the drawing settings (header) couldn't be read for THICKNESS: those stay at their defaults, the other settings were kept"]});
        let why = verify_failure("f.cdy", &check).unwrap();
        assert!(why.contains("drawing settings (header) couldn't be read") && !why.contains("0 damaged parts"), "{why}");
    }

    /// B7-023: `verify` on a file with a whole second copy appended says that, and that the first copy is sound.
    #[test]
    fn verify_names_an_appended_copy() {
        let good = cadaddy_io::write(&cadaddy_engine::sample::floor_plan(), "a.cdy").unwrap();
        let mut twice = good.clone();
        twice.extend_from_slice(&good);
        let why = verify_failure("f.cdy", &cadaddy_io::cdy::verify(&twice).unwrap()).unwrap();
        assert!(why.contains("a second copy of the drawing is appended") && why.contains("first copy is sound"), "{why}");
        assert!(!why.contains("0 damaged parts"), "{why}");
    }

    /// B7-009: `import` of a damaged design file reports what opening it does.
    #[test]
    fn import_reports_what_reading_left_out() {
        let (dgn, _) = cadaddy_io::dgn::write(&cadaddy_engine::sample::floor_plan()).unwrap();
        install_io();
        let path = temp_file("cut.dgn", &dgn[..dgn.len() / 2]);
        let mut s = Session::for_app();
        s.new_drawing_with_viewports(false);
        let r = s.execute("import", &json!({ "path": path })).unwrap();
        let said = r["warnings"].as_array().cloned().unwrap_or_default();
        assert!(said.iter().any(|w| w.as_str().is_some_and(|w| w.contains("ended early"))), "{r}");
        // A sound file has none.
        let ok = temp_file("whole.dgn", &dgn);
        let r = s.execute("import", &json!({ "path": ok })).unwrap();
        assert!(r.get("warnings").is_none(), "{r}");
    }
}
