//! The command registry. Ids are AutoCAD command names in lower case; menu paths follow
//! AutoCAD's menu bar so the catalog doubles as the parity metric.

pub mod adcenter;
pub(crate) mod annoscale;
pub(crate) mod annotate;
pub mod array;
pub(crate) mod ask;
mod attipedit;
mod audit;
pub mod autospec;
pub mod bimprops;
pub mod blockdetect;
pub(crate) mod blocks;
pub mod bom;
pub mod bspec;
pub mod calibrate;
mod cam;
pub mod ceilings;
pub mod classic;
pub(crate) mod clip_partners;
pub mod clipboard;
pub mod colorbook;
pub mod constraints;
pub mod coordination;
pub mod copricing;
mod dash_styles;
mod draw;
mod draw2;
mod draworder;
mod edit;
mod edit3d;
pub mod estimate;
mod exact_ends;
mod exchange;
mod express;
mod express4;
pub mod exterior;
mod field;
pub mod file;
pub mod find;
mod frames;
pub mod grids;
mod gripcmds;
mod griphot;
pub mod groups;
pub(crate) mod hatch;
pub mod help_texts;
pub mod history;
pub mod hyperlink;
mod imageattach;
mod imageclip;
pub mod import_check;
pub(crate) mod inquiry;
mod layer;
pub mod levels;
pub mod markup;
pub mod paramconv;
pub mod pointcloud;
pub mod procure;
pub mod progress;
pub mod project;
mod rasterout;
pub mod rfi;
pub mod schedules;
mod setbylayer;
pub mod spec_import;
pub mod tags;
mod vpsync;
pub mod wholejob;
mod xplode;
/// The layers a layer filter shows (the Layer Properties Manager's filter tree).
pub use layer::filters::layers_of as layer_filter_layers;
pub use layer::notify::unreconciled as unreconciled_layers;
pub use qselect::wildcard;
pub use render::apply_environment;
pub use underlay::forget_pages as forget_underlay_pages;
pub mod agentready;
pub mod bcf;
pub mod clashreport;
pub mod cobie;
pub mod concrete;
pub mod dgn;
pub mod dimmodes;
pub mod drywall;
pub mod dwgprops;
pub mod fieldview;
pub mod fourd;
pub mod furnish;
pub mod gear;
pub mod geo;
pub mod grounding;
pub mod ifcmodel;
pub mod ifcref;
pub mod imagetrace;
pub mod labels;
pub(crate) mod layout;
pub mod lightbake;
pub mod lights;
mod lisp;
pub mod materials;
mod measuregeom;
pub mod mep;
pub mod mepqty;
mod mleader_content;
mod mleader_edit;
mod models3d;
mod modify;
mod modify2;
mod mtext_opts;
mod overkill;
pub mod overrides;
pub mod p2meval;
pub mod part;
pub mod parts;
pub mod pdfplans;
mod perceive;
mod pline_opts;
pub(crate) mod plotstamp;
pub mod plotters;
pub mod plugins_cmd;
pub mod printrev;
pub mod printset;
pub mod projrules;
mod props;
pub mod readers;
pub mod redline;
pub mod revise3d;
pub(crate) mod rhai_cmd;
pub mod rooms3d;
pub mod sitetakeoff;
pub mod spell;
pub mod stairs;
pub mod symbol;
mod takeoff;
pub mod tours;
pub mod ugsheets;
pub(crate) mod underlay;
mod unittypes;
pub mod validate;
pub mod verify;
pub mod walkpub;
pub mod wallgen;
pub use props::MatchProps;
pub mod actions;
pub mod rulemodel;
pub(crate) mod qselect;
mod render;
mod sections;
mod selecttools;
mod settings;
mod solids;
mod solview;
mod spline_opts;
pub mod standards;
mod surfaces;
pub(crate) mod table;
/// Excel workbooks for data links and table export.
pub use table::xlsx;
pub mod tolerance;
mod trimextend;
mod ucs;
mod utility;
pub(crate) mod view;
/// The steering wheel's wedges and the quick views' thumbnails, for the app.
pub use view::{navwheel, quickview};

pub mod curves;
pub mod helpers;
pub mod machines;
pub mod options;

use cadaddy_doc::Handle;
use cadaddy_geom::Vec2;
use serde::Serialize;
use serde_json::Value;

use crate::{EngineError, Interactive, Result, Session};

pub type Run = fn(&mut Session, &Value) -> Result<Value>;
pub type Enabled = fn(&Session) -> std::result::Result<(), String>;
pub type Factory = fn(&Session) -> Result<Box<dyn Interactive>>;

pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Menu placement, e.g. `["Draw", "Circle", "Center, Radius"]`. Empty = not in menus.
    pub menu: &'static [&'static str],
    pub shortcut: Option<&'static str>,
    pub aliases: &'static [&'static str],
    pub params: &'static str,
    pub enabled: Enabled,
    pub run: Run,
    pub undoable: bool,
    /// The command restores a drawing from the undo/redo history: the follow-ups (associative
    /// dimensions, groups, tables, fields…) must not run on the restored drawing.
    pub restores_history: bool,
    pub interactive: Option<Factory>,
    /// Can run inside another command with a leading apostrophe.
    pub transparent: bool,
    /// Hand-written help text for the command (empty = auto-generated only).
    pub help: &'static str,
    /// What it reaches beyond the drawing, when its parameters don't say ([`CommandSpec::reach`]).
    pub access: Option<Access>,
}

/// What a command reaches beyond the drawing: plug-ins need a grant for files or the network
/// (PLUGINS grant).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// The open drawing (and the session) only.
    Drawing,
    /// Reads or writes files.
    Files,
    /// Uses the network.
    Network,
}

impl Access {
    /// The grant a plug-in needs for it (`None`: none).
    pub fn grant(self) -> Option<&'static str> {
        match self {
            Access::Drawing => None,
            Access::Files => Some("files"),
            Access::Network => Some("network"),
        }
    }
}

/// Parameter names that are a file or a folder (`{path}`, `{output}`, `{pdf: path}`…).
const FILE_PARAMS: &[&str] = &[
    "path",
    "paths",
    "file",
    "files",
    "filename",
    "folder",
    "dir",
    "directory",
    "output",
    "save",
    "pdf",
    "dwg",
    "dxf",
    "png",
    "svg",
    "csv",
    "xlsx",
    "lin",
    "pat",
    "font",
    "zip",
    "destination",
];
/// Parameter names that are a place on the network.
const NETWORK_PARAMS: &[&str] = &["url", "urls", "uri", "endpoint", "server", "feed"];

impl CommandSpec {
    pub const fn new(id: &'static str, label: &'static str, run: Run) -> Self {
        CommandSpec {
            id,
            label,
            menu: &[],
            shortcut: None,
            aliases: &[],
            params: "",
            enabled: has_doc,
            run,
            undoable: true,
            restores_history: false,
            interactive: None,
            transparent: false,
            help: "",
            access: None,
        }
    }
    /// It reads or writes files though no parameter names one (QSAVE: the drawing's own file).
    pub const fn files(mut self) -> Self {
        self.access = Some(Access::Files);
        self
    }
    /// It uses the network.
    pub const fn network(mut self) -> Self {
        self.access = Some(Access::Network);
        self
    }
    /// What it reaches: as marked, else as its parameters say (a file or folder parameter: files;
    /// an address: the network), else only the drawing. The one place plug-in host calls are
    /// classified ([`crate::plugins`]).
    pub fn reach(&self) -> Access {
        if let Some(a) = self.access {
            return a;
        }
        let names = crate::documented_params(self.params);
        let any = |list: &[&str]| names.iter().any(|n| list.iter().any(|k| n.eq_ignore_ascii_case(k)));
        if any(NETWORK_PARAMS) {
            Access::Network
        } else if any(FILE_PARAMS) {
            Access::Files
        } else {
            Access::Drawing
        }
    }
    pub const fn menu(mut self, m: &'static [&'static str]) -> Self {
        self.menu = m;
        self
    }
    pub const fn key(mut self, k: &'static str) -> Self {
        self.shortcut = Some(k);
        self
    }
    pub const fn alias(mut self, a: &'static [&'static str]) -> Self {
        self.aliases = a;
        self
    }
    pub const fn params(mut self, p: &'static str) -> Self {
        self.params = p;
        self
    }
    pub const fn enabled(mut self, e: Enabled) -> Self {
        self.enabled = e;
        self
    }
    pub const fn noundo(mut self) -> Self {
        self.undoable = false;
        self
    }
    /// It restores a drawing from the undo/redo history: the after-command follow-ups must not
    /// run on the restored drawing (they would take restored objects for moved ones).
    pub const fn restores_history(mut self) -> Self {
        self.restores_history = true;
        self
    }
    pub const fn interactive(mut self, f: Factory) -> Self {
        self.interactive = Some(f);
        self
    }
    pub const fn transparent(mut self) -> Self {
        self.transparent = true;
        self
    }
    pub const fn help(mut self, h: &'static str) -> Self {
        self.help = h;
        self
    }
    /// Whether the id, label or an alias contains `filter` (any case): what `list_commands` and
    /// `engine.commands {filter}` select by, before anything is serialised.
    pub fn matches(&self, filter: &str) -> bool {
        let f = filter.trim().to_ascii_lowercase();
        let has = |t: &str| t.to_ascii_lowercase().contains(&f);
        f.is_empty() || has(self.id) || has(self.label) || self.aliases.iter().any(|a| has(a))
    }

    pub fn info(&self, s: &Session) -> CommandInfo {
        let e = (self.enabled)(s);
        CommandInfo {
            id: self.id,
            label: self.label,
            menu: self.menu.to_vec(),
            shortcut: self.shortcut,
            aliases: self.aliases.to_vec(),
            params: self.params,
            interactive: self.interactive.is_some(),
            enabled: e.is_ok(),
            disabled_reason: e.err(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: Vec<&'static str>,
    pub shortcut: Option<&'static str>,
    pub aliases: Vec<&'static str>,
    pub params: &'static str,
    pub interactive: bool,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

/// A generated help page for a command.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelpPage {
    pub id: String,
    pub label: &'static str,
    pub aliases: Vec<String>,
    pub menu_path: String,
    pub shortcut: String,
    pub params: &'static str,
    pub interactive: bool,
    pub transparent: bool,
    pub help_text: &'static str,
    /// The keywords of the command's first prompt, as the command line offers them
    /// ("Ttr (tan tan radius)" is typed TTR or T); empty when it offers none.
    pub options: Vec<String>,
    /// What the command asks first, as the command line shows it
    /// ("Specify center point for circle or [3P/2P/Ttr (tan tan radius)]:").
    pub prompts: Vec<String>,
}

impl CommandSpec {
    /// Generate a help page for this command.
    pub fn help_page(&self) -> HelpPage {
        let first = first_prompt(self);
        HelpPage {
            id: self.id.to_ascii_uppercase(),
            label: self.label,
            aliases: self.aliases.iter().map(|a| a.to_ascii_uppercase()).collect(),
            menu_path: if self.menu.is_empty() { String::new() } else { self.menu.join(" > ") },
            shortcut: self.shortcut.unwrap_or("").to_string(),
            params: self.params,
            interactive: self.interactive.is_some(),
            transparent: self.transparent,
            help_text: self.help,
            options: first.as_ref().map(|p| p.keywords.clone()).unwrap_or_default(),
            prompts: first.iter().map(crate::Prompt::display).collect(),
        }
    }
}

/// The first prompt of an interactive command, read from the command's own machine on a fresh
/// drawing. Only the factory and `prompt` run, and both only read the session, so nothing is
/// drawn, saved or opened. `None` for a command with no prompts, or one that can't start on an
/// empty drawing (it needs a selection, say).
pub fn first_prompt(spec: &CommandSpec) -> Option<crate::Prompt> {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let factory = spec.interactive?;
    let s = Session::new();
    let machine = catch_unwind(AssertUnwindSafe(|| factory(&s))).ok()?.ok()?;
    catch_unwind(AssertUnwindSafe(|| machine.prompt(&s))).ok()
}

/// Find a command by id or alias.
pub fn find_by_id_or_alias(name: &str) -> Option<&'static CommandSpec> {
    let l = name.to_ascii_lowercase();
    command_specs().iter().find(|c| c.id == l || c.aliases.contains(&l.as_str()))
}

/// Return all command ids that have no hand-written help text.
pub fn commands_without_help() -> Vec<&'static str> {
    command_specs().iter().filter(|c| c.help.is_empty()).map(|c| c.id).collect()
}

pub fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}
pub fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.state().map(|_| ()).map_err(|_| "no drawing open".into())
}
pub fn has_selection(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    if s.selection().is_empty() { Err("nothing selected".into()) } else { Ok(()) }
}

pub fn command_specs() -> &'static [CommandSpec] {
    static SPECS: std::sync::OnceLock<Vec<CommandSpec>> = std::sync::OnceLock::new();
    SPECS.get_or_init(|| {
        let mut v = Vec::new();
        v.extend(file::specs());
        v.extend(file::alias_specs());
        v.extend(audit::specs());
        v.extend(edit::specs());
        v.extend(classic::specs());
        v.extend(qselect::specs());
        v.extend(selecttools::specs());
        v.extend(view::specs());
        v.extend(draw::specs());
        v.extend(draw2::specs());
        v.extend(annotate::specs());
        v.extend(mleader_edit::specs());
        v.extend(annoscale::specs());
        v.extend(hatch::specs());
        v.extend(overkill::specs());
        v.extend(blocks::specs());
        v.extend(blockdetect::specs());
        v.extend(modify::specs());
        v.extend(draworder::specs());
        v.extend(hyperlink::specs());
        v.extend(lights::specs());
        v.extend(geo::specs());
        v.extend(lightbake::specs());
        v.extend(materials::specs());
        v.extend(wallgen::specs());
        v.extend(revise3d::specs());
        v.extend(overrides::specs());
        v.extend(bspec::specs());
        v.extend(p2meval::specs());
        v.extend(schedules::specs());
        v.extend(pdfplans::specs());
        v.extend(grids::specs());
        v.extend(wholejob::specs());
        v.extend(calibrate::specs());
        v.extend(levels::specs());
        v.extend(estimate::specs());
        v.extend(progress::specs());
        v.extend(copricing::specs());
        v.extend(bom::specs());
        v.extend(procure::specs());
        v.extend(tags::specs());
        v.extend(mep::specs());
        v.extend(clashreport::specs());
        v.extend(concrete::specs());
    v.extend(ugsheets::specs());
        v.extend(gear::specs());
        v.extend(labels::specs());
        v.extend(sitetakeoff::specs());
        v.extend(grounding::specs());
        v.extend(mepqty::specs());
        v.extend(ifcref::specs());
        v.extend(ifcmodel::specs());
        v.extend(bcf::specs());
        v.extend(part::specs());
        v.extend(cobie::specs());
        v.extend(printset::specs());
        v.extend(readers::specs());
        v.extend(validate::specs());
        v.extend(printrev::specs());
        v.extend(ceilings::specs());
        v.extend(rfi::specs());
        v.extend(exterior::specs());
        v.extend(stairs::specs());
        v.extend(autospec::specs());
        v.extend(verify::specs());
        v.extend(imagetrace::specs());
        v.extend(crate::agent::specs());
        v.extend(dimmodes::specs());
        v.extend(redline::specs());
        v.extend(tours::specs());
        v.extend(fourd::specs());
        v.extend(walkpub::specs());
        v.extend(imageclip::specs());
        v.extend(array::specs());
        v.extend(symbol::specs());
        v.extend(parts::specs());
        v.extend(express::specs());
        v.extend(express4::specs());
        v.push(
            CommandSpec::new("spell", "Spelling", spell::run)
                .menu(&["Tools", "Spelling"])
                .alias(&["sp", "-spell"])
                .params("{action?: \"check\" (default) | \"suggest\" | \"change\" | \"changeAll\" | \"ignoreAll\" | \"add\", handles? (default every object in the current space), suggestions?: bool (default true), word?, to?, handle? and where? (\"text\" | \"dimension\" | \"multileader\" | {attribute: n} | {cell: [row, col]}) for one change, all?: bool} → check: {misspelled: [{handle, where, word, context, suggestions}], checked}; change: {changed}")
                .interactive(|_| Ok(Box::new(spell::SpellM::default()))),
        );
        v.extend(models3d::specs());
        v.extend(furnish::specs());
        v.extend(perceive::specs());
        v.extend(rooms3d::specs());
        v.extend(bimprops::specs());
        v.extend(projrules::specs());
        v.extend(spec_import::specs());
        v.extend(agentready::specs());
        v.extend(underlay::specs());
        v.extend(dgn::specs());
        v.extend(takeoff::specs());
        v.extend(drywall::specs());
        v.extend(unittypes::specs());
    v.extend(fieldview::specs());
        v.extend(imageattach::specs());
        v.extend(dash_styles::specs());
        v.extend(modify2::specs());
        v.extend(groups::specs());
        v.extend(gripcmds::specs());
        v.extend(file::compare::specs());
        v.extend(file::convert::specs());
        v.extend(layer::specs());
        v.extend(layout::specs());
        v.extend(plotstamp::specs());
        v.extend(dwgprops::specs());
        v.extend(crate::script_tools::specs());
        v.extend(cam::specs());
        v.extend(exchange::specs());
        v.extend(import_check::specs());
        v.extend(rasterout::specs());
        v.extend(props::specs());
        v.extend(inquiry::specs());
        v.extend(settings::specs());
        v.extend(options::specs());
        v.extend(utility::specs());
        v.extend(table::specs());
        v.extend(field::specs());
        v.extend(ucs::specs());
        v.extend(tolerance::specs());
        v.extend(pointcloud::specs());
        v.extend(markup::specs());
        v.extend(adcenter::specs());
        v.extend(history::specs());
        v.extend(standards::specs());
        v.extend(colorbook::specs());
        v.extend(constraints::specs());
        v.extend(paramconv::specs());
        v.extend(lisp::specs());
        v.extend(crate::secureload::specs());
        v.extend(solids::specs());
    v.extend(surfaces::specs());
        v.extend(sections::specs());
        v.extend(solview::specs());
        v.extend(render::specs());
        v.extend(actions::specs());
        v.extend(edit3d::specs());
        v.extend(setbylayer::specs());
        v.extend(xplode::specs());
        v.extend(vpsync::specs());
        v.extend(attipedit::specs());
        v.extend(frames::specs());
        v.extend(coordination::specs());
        v.extend(project::specs());
        v.extend(rhai_cmd::specs());
        v.extend(plugins_cmd::specs());
        v.extend(rulemodel::specs());
        // Apply hand-written help texts.
        for spec in &mut v {
            let text = help_texts::lookup(spec.id);
            if !text.is_empty() {
                spec.help = text;
            }
        }
        v
    })
}

pub fn find_command(id: &str) -> Option<&'static CommandSpec> {
    let l = id.to_ascii_lowercase();
    command_specs().iter().find(|c| c.id == l)
}

/// Resolve a typed name or alias (AutoCAD-style short names such as `L`, `C`, `PL`) to an id.
/// Command names win; the user's aliases (ALIASEDIT, REINIT) come before the built-in ones.
pub fn resolve_alias(name: &str) -> String {
    let l = name.to_ascii_lowercase();
    if find_command(&l).is_some() {
        return l;
    }
    if let Some(id) = file::user_alias(&l) {
        return id;
    }
    command_specs().iter().find(|c| c.aliases.iter().any(|a| a.eq_ignore_ascii_case(&l))).map(|c| c.id.to_string()).unwrap_or(l)
}

// ---------- param helpers ----------

pub(crate) fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}
pub(crate) fn f64_or(p: &Value, key: &str, default: f64) -> f64 {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(default)
}
pub(crate) fn f64_req(cmd: &str, p: &Value, key: &str) -> Result<f64> {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(cmd, format!("`{key}` (number) is required")))
}
/// An optional size: `None` when absent or null, else a finite number above zero (or zero too
/// with `zero_ok`). Anything else is refused, not clamped or made absolute, so a bad value never
/// makes a degenerate object.
pub(crate) fn size_param(cmd: &str, p: &Value, key: &str, zero_ok: bool) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64().filter(|x| x.is_finite()) {
            Some(x) if x > 0.0 || (zero_ok && x == 0.0) => Ok(Some(x)),
            _ => Err(bad(cmd, format!("`{key}` must be {}", if zero_ok { "zero or positive" } else { "positive" }))),
        },
    }
}
/// `pts` without consecutive repeats (closer than `1e-12`), as the interactive LINE ignores a
/// point picked twice.
pub(crate) fn distinct_points(pts: &[Vec2]) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
    for p in pts {
        if !out.last().is_some_and(|l| l.near(*p, 1e-12)) {
            out.push(*p);
        }
    }
    out
}
pub(crate) fn bool_or(p: &Value, key: &str, default: bool) -> bool {
    p.get(key).and_then(Value::as_bool).unwrap_or(default)
}
pub(crate) fn str_param<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str)
}
pub fn point_value(v: &Value) -> Option<Vec2> {
    if let Some(a) = v.as_array() {
        let x = a.first()?.as_f64()?;
        let y = a.get(1)?.as_f64()?;
        let p = Vec2::new(x, y);
        return p.is_finite().then_some(p);
    }
    if let Some(s) = v.as_str() {
        return crate::prompt::parse_point(s, Vec2::ZERO);
    }
    let x = v.get("x")?.as_f64()?;
    let y = v.get("y")?.as_f64()?;
    let p = Vec2::new(x, y);
    p.is_finite().then_some(p)
}
pub(crate) fn point_param(p: &Value, key: &str) -> Option<Vec2> {
    p.get(key).and_then(point_value)
}
pub(crate) fn point_req(cmd: &str, p: &Value, key: &str) -> Result<Vec2> {
    point_param(p, key).ok_or_else(|| bad(cmd, format!("`{key}` ([x, y]) is required")))
}
pub(crate) fn points_param(p: &Value, key: &str) -> Option<Vec<Vec2>> {
    p.get(key)?.as_array()?.iter().map(point_value).collect()
}
/// Handles from `handles` (hex strings or numbers), `handle`, or else the current selection. A
/// `handle`/`handles` that can't be read is an error, never a silent fall back to the selection.
/// Evaluate a DIESEL expression (as RTEXT does).
pub fn express_diesel(s: &Session, expr: &str) -> String {
    express::rtext::diesel(s, expr)
}

pub(crate) fn targets(s: &Session, p: &Value) -> Result<Vec<Handle>> {
    let handle = |v: &Value| v.as_str().and_then(Handle::parse_hex).or_else(|| v.as_u64().map(Handle));
    let not_handle = |key: &str, v: &Value| EngineError::Other(format!("`{key}`: {v} is not an object handle (a hex string such as \"1A2\")"));
    match p.get("handles") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => return a.iter().map(|v| handle(v).ok_or_else(|| not_handle("handles", v))).collect(),
        Some(v) => return Err(EngineError::Other(format!("`handles` must be an array of object handles, got {v}"))),
    }
    match p.get("handle") {
        None | Some(Value::Null) => Ok(s.selection()),
        Some(v) => Ok(vec![handle(v).ok_or_else(|| not_handle("handle", v))?]),
    }
}
pub(crate) fn ok() -> Result<Value> {
    Ok(Value::Null)
}
