//! Linetypes from the user's own `.lin` files: LINETYPE `{load, file | text}` and the
//! command-line -LINETYPE (`?`, `Load` from the built-in library or a file, `Set`).

use cadaddy_doc::library::files::parse_lin;
use cadaddy_doc::{Drawing, Linetype};
use serde_json::{Value, json};

use super::super::helpers::support_text;
use super::super::qselect::wildcard;
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Step};

/// Names no file may define: they are not linetype definitions.
const RESERVED: &[&str] = &["bylayer", "byblock", "continuous"];

/// Whether a LINETYPE call names a file (`file`) or gives its contents (`text`).
pub(super) fn has_file(p: &Value) -> bool {
    p.get("file").is_some_and(|v| !v.is_null()) || p.get("text").is_some_and(|v| !v.is_null())
}

/// What loading from a file did.
#[derive(Default)]
struct Loaded {
    added: Vec<String>,
    reloaded: Vec<String>,
    /// Already in the drawing and not reloaded.
    kept: Vec<String>,
    warnings: Vec<String>,
}

/// Whether `name` is one of the comma-separated names or wildcards in `names`.
fn wanted(names: &str, name: &str) -> bool {
    names.split(',').map(str::trim).filter(|n| !n.is_empty()).any(|n| wildcard(n, name))
}

/// The linetypes of `text` that `names` asks for, without the reserved names.
fn read(text: &str, names: &str, out: &mut Loaded) -> Vec<Linetype> {
    let parsed = parse_lin(text);
    out.warnings.extend(parsed.warnings);
    let mut keep = Vec::new();
    for lt in parsed.items.into_iter().filter(|l| wanted(names, &l.name)) {
        if RESERVED.contains(&lt.name.to_ascii_lowercase().as_str()) {
            out.warnings.push(format!("{} is a reserved name; not loaded", lt.name));
        } else {
            keep.push(lt);
        }
    }
    keep
}

/// Put `lt` into `d`: added, or replacing the definition of the same name when `reload`.
fn put(d: &mut Drawing, mut lt: Linetype, reload: bool, out: &mut Loaded) {
    for e in lt.pattern.iter().filter(|e| e.text.is_some()) {
        if let Some(st) = e.style.as_deref()
            && d.text_style(st).is_none()
        {
            out.warnings.push(format!("{}: text style {st} is not in this drawing; its text uses the default style", lt.name));
            break;
        }
    }
    // A shape by name gets its number from its shape file: CADaddy's own (cadaddy.shx, built in)
    // or a file on the font search path. Without the file the element is kept and draws nothing
    // until it is there.
    let resolved: Vec<Option<(u16, String)>> =
        lt.pattern.iter().map(|e| if e.shape_name.is_some() { cadaddy_render::linetype_text::resolve_shape(d, e) } else { None }).collect();
    for (e, r) in lt.pattern.iter_mut().zip(resolved) {
        match (r, &e.shape_name) {
            (Some((n, _)), _) => e.shape = Some(n),
            (None, Some(name)) => out.warnings.push(format!(
                "{}: shape {name} isn't in {} (or the file isn't on the font path): it draws nothing until it is",
                lt.name,
                e.style.as_deref().unwrap_or("its shape file")
            )),
            (None, None) => {}
        }
    }
    match d.linetypes.iter_mut().find(|l| l.name.eq_ignore_ascii_case(&lt.name)) {
        Some(old) if reload => {
            out.reloaded.push(old.name.clone());
            // The drawing keeps the name as it had it (objects refer to it).
            *old = Linetype { name: old.name.clone(), ..lt };
        }
        Some(old) => out.kept.push(old.name.clone()),
        None => {
            out.added.push(lt.name.clone());
            d.linetypes.push(lt);
        }
    }
}

/// LINETYPE `{load?: names | "*", file?: path, text?: contents, reload?, current?}`: load the
/// named linetypes (comma-separated, wildcards allowed; all by default) from a `.lin` file.
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let (text, source) = support_text("linetype", p, "file", "text")?.ok_or_else(|| bad("linetype", "`file` or `text` is required"))?;
    let names = str_param(p, "load").unwrap_or("*");
    let mut out = Loaded::default();
    let found = read(&text, names, &mut out);
    if found.is_empty() {
        return Err(bad("linetype", format!("no linetype matching `{names}` in {source}")));
    }
    let reload = bool_or(p, "reload", false);
    let d = s.doc_mut()?;
    for lt in found {
        put(d, lt, reload, &mut out);
    }
    // Setting the current linetype and the listing are LINETYPE's own.
    let mut rest = p.clone();
    if let Some(o) = rest.as_object_mut() {
        for k in ["file", "text", "load", "reload"] {
            o.remove(k);
        }
    }
    let mut r = super::run_linetype(s, &rest)?;
    if let Some(o) = r.as_object_mut() {
        o.insert("added".into(), json!(out.added));
        o.insert("reloaded".into(), json!(out.reloaded));
        o.insert("alreadyLoaded".into(), json!(out.kept));
        o.insert("warnings".into(), json!(out.warnings));
    }
    Ok(r)
}

// ---------- -LINETYPE ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    Option,
    LoadNames,
    LoadFile,
    Reload,
    Set,
}

/// -LINETYPE at the command line.
pub(super) struct DashLinetype {
    ask: Ask,
    names: String,
    /// Linetypes from the file that are already loaded, asked about one at a time.
    reloads: Vec<Linetype>,
}

impl DashLinetype {
    pub(super) fn new() -> Self {
        DashLinetype { ask: Ask::Option, names: String::new(), reloads: Vec::new() }
    }

    fn list(s: &mut Session) {
        let names: Vec<String> =
            s.doc().map(|d| d.linetypes.iter().map(|l| format!("{:<20} {}", l.name, l.description)).collect()).unwrap_or_default();
        s.echo("Loaded linetypes:");
        for n in names {
            s.echo(n);
        }
    }

    /// Load the asked-for names from the built-in library (`path` empty) or a file.
    fn load(&mut self, s: &mut Session, path: &str) -> Result<()> {
        if path.is_empty() {
            for name in self.names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                let before = s.doc()?.linetypes.len();
                match super::run_linetype(s, &json!({ "load": name })) {
                    Ok(_) if s.doc()?.linetypes.len() > before => s.echo(format!("Linetype(s) {name} loaded.")),
                    Ok(_) => s.echo(format!("Linetype {name} is already loaded.")),
                    Err(e) => s.echo(e.to_string()),
                }
            }
            self.ask = Ask::Option;
            return Ok(());
        }
        let text = match support_text("linetype", &json!({ "file": path }), "file", "text") {
            Ok(Some((text, _))) => text,
            Ok(None) => String::new(),
            Err(e) => {
                s.echo(e.to_string());
                self.ask = Ask::LoadFile;
                return Ok(());
            }
        };
        let mut out = Loaded::default();
        let found = read(&text, &self.names, &mut out);
        for w in out.warnings.drain(..) {
            s.echo(w);
        }
        if found.is_empty() {
            s.echo(format!("No linetype matching {} in {path}.", self.names));
        }
        let d = s.doc_mut()?;
        for lt in found {
            if d.linetype(&lt.name).is_some() {
                self.reloads.push(lt);
            } else {
                put(d, lt, false, &mut out);
            }
        }
        for w in out.warnings.drain(..) {
            s.echo(w);
        }
        for n in &out.added {
            s.echo(format!("Linetype {n} loaded."));
        }
        // `reloads` is asked from its end: reversed, the questions follow the file's order.
        self.reloads.reverse();
        self.ask = if self.reloads.is_empty() { Ask::Option } else { Ask::Reload };
        Ok(())
    }
}

impl Interactive for DashLinetype {
    fn name(&self) -> &'static str {
        "-LINETYPE"
    }

    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        let cur = s.doc()?.header.str("CELTYPE", "ByLayer");
        s.echo(format!("Current linetype: \"{cur}\""));
        Ok(Step::Continue)
    }

    fn prompt(&self, s: &Session) -> Prompt {
        match self.ask {
            Ask::Option => Prompt::new("Enter an option", Accept::TEXT).kw(&["?", "Load", "Set"]),
            Ask::LoadNames => Prompt::new("Enter linetype(s) to load", Accept::TEXT),
            Ask::LoadFile => Prompt::new("Enter name of linetype file", Accept::TEXT).default("built-in library"),
            Ask::Reload => {
                let name = self.reloads.last().map(|l| l.name.as_str()).unwrap_or("");
                Prompt::new(format!("Linetype {name} is already loaded. Reload it?"), Accept::TEXT).kw(&["Yes", "No"]).default("Yes")
            }
            Ask::Set => {
                let cur = s.doc().map(|d| d.header.str("CELTYPE", "ByLayer")).unwrap_or_default();
                Prompt::new("Specify linetype name", Accept::TEXT).kw(&["?"]).default(cur)
            }
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let t = match &i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            Input::Enter => String::new(),
            _ => return Ok(Step::Continue),
        };
        match self.ask {
            Ask::Option => match t.to_ascii_lowercase().as_str() {
                "" => return Ok(Step::Done),
                "?" => Self::list(s),
                "load" | "l" => self.ask = Ask::LoadNames,
                "set" | "s" => self.ask = Ask::Set,
                _ => s.echo("Invalid option keyword."),
            },
            Ask::LoadNames if t.is_empty() => self.ask = Ask::Option,
            Ask::LoadNames => {
                self.names = t;
                self.ask = Ask::LoadFile;
            }
            Ask::LoadFile => self.load(s, &t)?,
            Ask::Reload => {
                let yes = !t.to_ascii_lowercase().starts_with('n');
                if let Some(lt) = self.reloads.pop() {
                    let name = lt.name.clone();
                    if yes {
                        let mut out = Loaded::default();
                        put(s.doc_mut()?, lt, true, &mut out);
                        s.echo(format!("Linetype {name} reloaded."));
                    }
                }
                if self.reloads.is_empty() {
                    self.ask = Ask::Option;
                }
            }
            Ask::Set if t == "?" => Self::list(s),
            Ask::Set if t.is_empty() => self.ask = Ask::Option,
            Ask::Set => {
                if let Err(e) = super::run_linetype(s, &json!({ "current": t })) {
                    s.echo(e.to_string());
                }
                self.ask = Ask::Option;
            }
        }
        Ok(Step::Continue)
    }
}

/// The elements of a LINETYPE.DEFINE call: `{dash: length}` / `{gap: length}` / `{dot: true}`,
/// or a text or shape (`{text, style?, …}` / `{shape: name, file?, …}`, with `scale`, `rotation`
/// in degrees, `absolute`, `x`, `y`) that sits where the element before it ends (as in a `.lin`
/// file). Lengths are as `.lin` files give them: a dash > 0, a gap < 0 (either sign is taken for
/// `dash` and `gap`).
fn elements(p: &Value) -> Result<Vec<cadaddy_doc::DashElement>> {
    use cadaddy_doc::DashElement;
    let rows =
        p.get("elements").and_then(Value::as_array).ok_or_else(|| bad("linetype.define", "elements required: [{dash|gap|dot|text|shape…}]"))?;
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && x.abs() <= cadaddy_doc::library::files::MAX_VALUE);
    let mut out: Vec<DashElement> = Vec::new();
    for (i, r) in rows.iter().enumerate().take(cadaddy_doc::library::files::MAX_LINETYPE_ELEMENTS) {
        let attach = |e: DashElement, out: &mut Vec<DashElement>| {
            if out.last().is_none_or(|l| l.text.is_some() || l.shape_name.is_some()) {
                out.push(DashElement::dash(0.0));
            }
            if let Some(last) = out.last_mut() {
                *last = DashElement { length: last.length, ..e };
            }
        };
        let options = |mut e: DashElement| -> Result<DashElement> {
            if let Some(s) = r.get("scale") {
                e.scale = s
                    .as_f64()
                    .filter(|s| s.is_finite() && *s > 0.0 && *s <= 1e6)
                    .ok_or_else(|| bad("linetype.define", format!("element {i}: scale must be more than zero")))?;
            }
            e.rotation = num(r, "rotation").unwrap_or(0.0).to_radians();
            e.absolute = r.get("absolute").and_then(Value::as_bool).unwrap_or(false);
            e.offset = cadaddy_geom::Vec2::new(num(r, "x").unwrap_or(0.0), num(r, "y").unwrap_or(0.0));
            Ok(e)
        };
        if let Some(d) = num(r, "dash") {
            out.push(DashElement::dash(d.abs()));
        } else if let Some(g) = num(r, "gap") {
            out.push(DashElement::dash(-g.abs()));
        } else if r.get("dot").is_some() {
            out.push(DashElement::dash(0.0));
        } else if let Some(t) = r.get("text").and_then(Value::as_str) {
            if t.trim().is_empty() || t.contains(['"', '\n', '\r']) || t.chars().count() > 255 {
                return Err(bad("linetype.define", format!("element {i}: text must be one line without quotes")));
            }
            let style = r.get("style").and_then(Value::as_str).unwrap_or("Standard").trim().to_string();
            attach(options(DashElement { text: Some(t.to_string()), style: Some(style), scale: 0.1, ..DashElement::dash(0.0) })?, &mut out);
        } else if let Some(sh) = r.get("shape").and_then(Value::as_str) {
            let name = sh.trim().to_ascii_uppercase();
            if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_alphanumeric() || "_-$".contains(c)) {
                return Err(bad("linetype.define", format!("element {i}: bad shape name `{sh}`")));
            }
            let file = r.get("file").and_then(Value::as_str).unwrap_or(cadaddy_doc::library::SHAPE_FILE).trim().to_string();
            attach(options(DashElement { shape_name: Some(name), style: Some(file), scale: 0.1, ..DashElement::dash(0.0) })?, &mut out);
        } else {
            return Err(bad("linetype.define", format!("element {i}: give dash, gap, dot, text or shape")));
        }
    }
    if out.iter().all(|e| e.length == 0.0 && e.text.is_none() && e.shape_name.is_none()) {
        return Err(bad("linetype.define", "a linetype needs a dash, a gap, a text or a shape"));
    }
    Ok(out)
}

/// Write `lt` into the `.lin` file at `path` (its definition of the same name replaced, others
/// kept; a new file when there is none).
fn save_lin(path: &str, lt: &Linetype) -> Result<()> {
    let new = cadaddy_doc::library::files::write_lin(std::slice::from_ref(lt)).map_err(|e| bad("linetype.define", e))?;
    // Into a file that is there: only this linetype's lines change.
    let text = match existing_text(path, "linetype.define")? {
        Some(old) => cadaddy_doc::library::files::put_definition(&old, &lt.name, &new),
        None => new,
    };
    crate::writefile::write(path, text).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))
}

/// The text of the `.lin` / `.pat` file at `path` a definition is saved into (`None`: there is
/// none yet). One that can't be read, or isn't text, is refused rather than written over.
pub(crate) fn existing_text(path: &str, cmd: &str) -> Result<Option<String>> {
    match crate::readfile::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| {
            bad(cmd, format!("`{path}` isn't a text file (not UTF-8), so it isn't written over: save into a .lin or .pat text file, or a new one"))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(bad(cmd, format!("`{path}` can't be read ({e}), so it isn't written over: give a file you can read, or a new name"))),
    }
}

/// LINETYPE.DEFINE `{name, description?, elements, save?: .lin path, current?}`: make (or remake)
/// a linetype from dashes, gaps, dots, texts and shapes; with `save`, also into a user `.lin`
/// file. Shapes come from CADaddy's `cadaddy.shx` unless an element names another file.
pub(super) fn run_define(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").map(str::trim).ok_or_else(|| bad("linetype.define", "name required"))?;
    super::super::names::check("linetype.define", "linetype", name)?;
    if RESERVED.contains(&name.to_ascii_lowercase().as_str()) || name.contains([',', '*', ';', '\n']) {
        return Err(bad("linetype.define", format!("`{name}` can't be a linetype name")));
    }
    let lt = Linetype {
        name: name.to_string(),
        description: str_param(p, "description").unwrap_or("").trim().chars().take(255).collect(),
        pattern: elements(p)?,
    };
    // The file first (with the shapes' numbers looked up as the drawing will have them), so a
    // file that can't be written leaves the drawing as it was.
    if let Some(path) = str_param(p, "save").map(str::trim).filter(|x| !x.is_empty()) {
        let mut scratch = Drawing { linetypes: Vec::new(), ..(*s.doc()?).clone() };
        put(&mut scratch, lt.clone(), true, &mut Loaded::default());
        save_lin(path, scratch.linetypes.first().unwrap_or(&lt))?;
    }
    let mut out = Loaded::default();
    put(s.doc_mut()?, lt, true, &mut out);
    if bool_or(p, "current", false) {
        super::run_linetype(s, &json!({ "current": name }))?;
    }
    let saved = s
        .doc()?
        .linetypes
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(name))
        .cloned()
        .ok_or_else(|| bad("linetype.define", "the linetype wasn't added"))?;
    Ok(
        json!({ "name": saved.name, "added": !out.added.is_empty(), "elements": saved.pattern.len(), "patternLength": saved.pattern_length(), "warnings": out.warnings }),
    )
}
