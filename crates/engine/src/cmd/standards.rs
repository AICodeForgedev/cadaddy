//! Drawing standards: STANDARDS associates standards drawings (DWS files, or any DWG/DXF: a
//! standards file is just a drawing) with the drawing; CHECKSTANDARDS compares its layers,
//! linetypes, text styles and dimension styles with theirs, fixes what is asked and writes a
//! report; LAYTRANS translates layers through a mapping table (saved and loaded as JSON).
//!
//! A name the standards define with other properties is fixed by taking the standard's
//! definition. A name the standards don't define is fixed by replacing it with one they do: its
//! objects (and the styles and layers using it) move to the replacement, which is copied in from
//! the standards when the drawing lacks it, and the old definition goes.

use std::path::Path;
use std::sync::Arc;

use cadaddy_doc::{Drawing, EntityStore};
use serde_json::{Value, json};

use super::blocks::xref::load::{Sym, rename_refs};
use super::file::{io, store};
use super::{CommandSpec, bad, has_doc, str_param};
use crate::{Result, Session};

/// Header variable holding the associated standards files (a JSON list of paths).
const VAR: &str = "CADADDY_STANDARDS";
/// Most standards files associated, and the largest one read.
const MAX_FILES: usize = 32;
const MAX_BYTES: u64 = 256 << 20;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("standards", "Configure Standards", run_standards)
            .menu(&["Tools", "CAD Standards", "Configure..."])
            .params("{action?: list (default) | add {path} | remove {path} | clear} → {files: [path]}: standards drawings (.dws, .dwg, .dxf) the drawing is checked against")
            .enabled(has_doc),
        CommandSpec::new("checkstandards", "Check Standards", run_check)
            .menu(&["Tools", "CAD Standards", "Check..."])
            .params("{fix?: [{id, replaceWith?}] (an id from the list: properties are taken from the standards; a non-standard name needs replaceWith) | fixAll?: bool (every property difference), report?: path (.html or .txt)} → {problems: [{id, kind, name, issue, standard?, replacements}], fixed, checked}")
            .enabled(has_doc),
        CommandSpec::new("laytrans", "Layer Translator", run_laytrans)
            .menu(&["Tools", "CAD Standards", "Layer Translator..."])
            .params("{map?: [{from, to}] (objects on `from` move to `to`; `to` is copied from `standards` when missing), standards?: path (the drawing whose layers are the targets), save?: path (writes the map), translate?: bool (default true; false only saves the map), load?: path (reads one: {map})} → {translated, map, targets: the standards' layers}")
            .enabled(has_doc),
    ]
}

/// The standards files associated with drawing `d`.
pub fn associated(d: &Drawing) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&d.header.str(VAR, "[]")).unwrap_or_default()
}

fn set_associated(d: &mut Drawing, files: &[String]) {
    if files.is_empty() {
        d.header.vars.remove(VAR);
    } else {
        d.header.set_str(VAR, &serde_json::to_string(files).unwrap_or_default());
    }
}

/// The drawing at `path` (a stored path resolves beside the drawing at `host`).
fn read(cmd: &str, path: &str, host: Option<&str>) -> Result<Drawing> {
    let hooks = io().ok_or_else(|| bad(cmd, "file formats are not available in this build"))?;
    let file = crate::doc_path::resolve(path, host.map(Path::new)).unwrap_or_else(|_| Path::new(path).to_path_buf());
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Read;
        let f = crate::readfile::open(&file).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
        let mut bytes = Vec::new();
        f.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(bad(cmd, format!("{path}: too large")));
        }
        // A DWS is a DWG by another name; anything else that isn't a DWG is read as DXF.
        let name = if bytes.starts_with(b"AC10") { "standards.dwg" } else { "standards.dxf" };
        (hooks.read)(&bytes, name).map_err(|e| bad(cmd, format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (hooks, file);
        Err(bad(cmd, "standards files are not available on the web"))
    }
}

fn host(s: &Session) -> Option<String> {
    s.state().ok().and_then(|st| st.path.clone())
}

fn run_standards(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "standards";
    let mut files = associated(s.doc()?);
    match str_param(p, "action").unwrap_or("list").to_ascii_lowercase().as_str() {
        "list" => {}
        "add" => {
            let path = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(cmd, "`path` is required"))?;
            read(cmd, path, host(s).as_deref())?;
            if !files.iter().any(|f| f == path) {
                if files.len() >= MAX_FILES {
                    return Err(bad(cmd, format!("at most {MAX_FILES} standards files")));
                }
                files.push(path.to_string());
            }
            set_associated(s.doc_mut()?, &files);
        }
        "remove" => {
            let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;
            let n = files.len();
            files.retain(|f| f != path);
            if files.len() == n {
                return Err(bad(cmd, format!("{path} is not associated")));
            }
            set_associated(s.doc_mut()?, &files);
        }
        "clear" => {
            files.clear();
            set_associated(s.doc_mut()?, &files);
        }
        other => return Err(bad(cmd, format!("unknown action \"{other}\" (list, add, remove or clear)"))),
    }
    Ok(json!({ "files": files }))
}

/// What a standard covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Layer,
    Linetype,
    TextStyle,
    DimStyle,
}

impl Kind {
    fn id(self) -> &'static str {
        match self {
            Kind::Layer => "layer",
            Kind::Linetype => "linetype",
            Kind::TextStyle => "style",
            Kind::DimStyle => "dimstyle",
        }
    }
    fn sym(self) -> Sym {
        match self {
            Kind::Layer => Sym::Layer,
            Kind::Linetype => Sym::Linetype,
            Kind::TextStyle => Sym::Style,
            Kind::DimStyle => Sym::DimStyle,
        }
    }
    /// The header variable naming the current one.
    fn current_var(self) -> &'static str {
        match self {
            Kind::Layer => "CLAYER",
            Kind::Linetype => "CELTYPE",
            Kind::TextStyle => "TEXTSTYLE",
            Kind::DimStyle => "DIMSTYLE",
        }
    }
    /// Names every drawing has, never reported as non-standard.
    fn built_in(self, n: &str) -> bool {
        let n = n.to_ascii_lowercase();
        match self {
            Kind::Layer => n == "0" || n == "defpoints",
            Kind::Linetype => ["bylayer", "byblock", "continuous"].contains(&n.as_str()),
            Kind::TextStyle | Kind::DimStyle => n == "standard",
        }
    }
}

/// The definitions of `kind` in `d` as JSON keyed by name (properties compared, name left out).
fn definitions(d: &Drawing, kind: Kind) -> Vec<(String, Value)> {
    let strip = |mut v: Value| {
        if let Some(o) = v.as_object_mut() {
            o.remove("name");
            // Visibility and state aren't standards (AutoCAD's checker leaves them alone).
            for k in ["on", "frozen", "locked", "vpFreezeNew", "description"] {
                o.remove(k);
            }
        }
        v
    };
    let pairs: Vec<(String, Value)> = match kind {
        Kind::Layer => d.layers.iter().map(|l| (l.name.clone(), serde_json::to_value(l).unwrap_or(Value::Null))).collect(),
        Kind::Linetype => d.linetypes.iter().map(|l| (l.name.clone(), serde_json::to_value(l).unwrap_or(Value::Null))).collect(),
        Kind::TextStyle => d.text_styles.iter().map(|l| (l.name.clone(), serde_json::to_value(l).unwrap_or(Value::Null))).collect(),
        Kind::DimStyle => d.dim_styles.iter().map(|l| (l.name.clone(), serde_json::to_value(l).unwrap_or(Value::Null))).collect(),
    };
    pairs.into_iter().filter(|(n, _)| !n.contains('|')).map(|(n, v)| (n, strip(v))).collect()
}

/// The standards combined: the first file defining a name wins.
fn combined(standards: &[Drawing], kind: Kind) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();
    for d in standards {
        for (n, v) in definitions(d, kind) {
            if !out.iter().any(|(m, _)| m.eq_ignore_ascii_case(&n)) {
                out.push((n, v));
            }
        }
    }
    out
}

/// One standards violation.
struct Problem {
    kind: Kind,
    name: String,
    /// The standards define the name with other properties (else they don't define it).
    differs: bool,
    standard: Option<Value>,
}

impl Problem {
    fn id(&self) -> String {
        format!("{}:{}", self.kind.id(), self.name)
    }
}

fn problems(d: &Drawing, standards: &[Drawing]) -> Vec<Problem> {
    let mut out = Vec::new();
    for kind in [Kind::Layer, Kind::Linetype, Kind::TextStyle, Kind::DimStyle] {
        let std = combined(standards, kind);
        for (name, v) in definitions(d, kind) {
            match std.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                Some((_, sv)) if *sv != v => out.push(Problem { kind, name, differs: true, standard: Some(sv.clone()) }),
                Some(_) => {}
                None if kind.built_in(&name) => {}
                None => out.push(Problem { kind, name, differs: false, standard: None }),
            }
        }
    }
    out
}

/// Every object store of `d`: model space, layouts and blocks.
fn each_store(d: &mut Drawing, mut f: impl FnMut(&mut EntityStore)) {
    f(&mut d.model);
    for l in &mut d.layouts {
        f(&mut l.entities);
    }
    for b in d.blocks.values_mut() {
        if b.xref_path.is_none() && !b.name.contains('|') {
            f(&mut Arc::make_mut(b).entities);
        }
    }
}

/// Make sure `d` has `name` of `kind`, copying the definition from `standards`.
fn ensure(d: &mut Drawing, kind: Kind, name: &str, standards: &[Drawing]) -> std::result::Result<String, String> {
    let have = match kind {
        Kind::Layer => d.layer(name).map(|l| l.name.clone()),
        Kind::Linetype => d.linetype(name).map(|l| l.name.clone()),
        Kind::TextStyle => d.text_style(name).map(|l| l.name.clone()),
        Kind::DimStyle => d.dim_style(name).map(|l| l.name.clone()),
    };
    if let Some(n) = have {
        return Ok(n);
    }
    for s in standards {
        match kind {
            Kind::Layer => {
                if let Some(l) = s.layer(name) {
                    if d.linetype(&l.linetype).is_none()
                        && let Some(lt) = standards.iter().find_map(|x| x.linetype(&l.linetype))
                    {
                        d.linetypes.push(lt.clone());
                    }
                    d.layers.push(l.clone());
                    return Ok(l.name.clone());
                }
            }
            Kind::Linetype => {
                if let Some(l) = s.linetype(name) {
                    d.linetypes.push(l.clone());
                    return Ok(l.name.clone());
                }
            }
            Kind::TextStyle => {
                if let Some(l) = s.text_style(name) {
                    d.text_styles.push(l.clone());
                    return Ok(l.name.clone());
                }
            }
            Kind::DimStyle => {
                if let Some(l) = s.dim_style(name) {
                    if d.text_style(&l.text_style).is_none()
                        && let Some(ts) = standards.iter().find_map(|x| x.text_style(&l.text_style))
                    {
                        d.text_styles.push(ts.clone());
                    }
                    d.dim_styles.push(l.clone());
                    return Ok(l.name.clone());
                }
            }
        }
    }
    Err(format!("the standards define no {} \"{name}\"", kind.id()))
}

/// Replace `from` (of `kind`) by `to` everywhere in `d` and remove `from`.
fn replace(d: &mut Drawing, kind: Kind, from: &str, to: &str, standards: &[Drawing]) -> std::result::Result<(), String> {
    if kind.built_in(from) {
        return Err(format!("\"{from}\" can't be replaced"));
    }
    let to = ensure(d, kind, to, standards)?;
    if to.eq_ignore_ascii_case(from) {
        return Ok(());
    }
    let sym = kind.sym();
    each_store(d, |store| {
        let hs: Vec<cadaddy_doc::Handle> = store.iter().map(|e| e.handle).collect();
        for h in hs {
            store.modify(h, |e| rename_refs(e, &|s, n| (s == sym && n.eq_ignore_ascii_case(from)).then(|| to.clone())));
        }
    });
    match kind {
        Kind::Layer => d.layers.retain(|l| !l.name.eq_ignore_ascii_case(from)),
        Kind::Linetype => {
            for l in &mut d.layers {
                if l.linetype.eq_ignore_ascii_case(from) {
                    l.linetype = to.clone();
                }
            }
            d.linetypes.retain(|l| !l.name.eq_ignore_ascii_case(from));
        }
        Kind::TextStyle => {
            for ds in &mut d.dim_styles {
                if ds.text_style.eq_ignore_ascii_case(from) {
                    ds.text_style = to.clone();
                }
            }
            d.text_styles.retain(|l| !l.name.eq_ignore_ascii_case(from));
        }
        Kind::DimStyle => d.dim_styles.retain(|l| !l.name.eq_ignore_ascii_case(from)),
    }
    let var = kind.current_var();
    if d.header.str(var, "").eq_ignore_ascii_case(from) {
        d.header.set_str(var, &to);
    }
    Ok(())
}

/// Give `name` of `kind` the standards' definition (its own name kept).
fn take_standard(d: &mut Drawing, kind: Kind, name: &str, standards: &[Drawing]) -> std::result::Result<(), String> {
    let pick = |n: &str| -> Option<Value> {
        standards.iter().find_map(|s| match kind {
            Kind::Layer => s.layer(n).and_then(|l| serde_json::to_value(l).ok()),
            Kind::Linetype => s.linetype(n).and_then(|l| serde_json::to_value(l).ok()),
            Kind::TextStyle => s.text_style(n).and_then(|l| serde_json::to_value(l).ok()),
            Kind::DimStyle => s.dim_style(n).and_then(|l| serde_json::to_value(l).ok()),
        })
    };
    let mut v = pick(name).ok_or_else(|| format!("the standards define no {} \"{name}\"", kind.id()))?;
    let set_name = |v: &mut Value, n: &str| {
        if let Some(o) = v.as_object_mut() {
            o.insert("name".into(), json!(n));
        }
    };
    match kind {
        Kind::Layer => {
            let Some(l) = d.layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(name)) else { return Ok(()) };
            // State (on, frozen, locked) stays the drawing's.
            set_name(&mut v, &l.name);
            let mut new: cadaddy_doc::Layer = serde_json::from_value(v).map_err(|e| e.to_string())?;
            (new.on, new.frozen, new.locked, new.vp_freeze_new) = (l.on, l.frozen, l.locked, l.vp_freeze_new);
            let lt = new.linetype.clone();
            *l = new;
            if d.linetype(&lt).is_none() {
                ensure(d, Kind::Linetype, &lt, standards)?;
            }
        }
        Kind::Linetype => {
            if let Some(l) = d.linetypes.iter_mut().find(|l| l.name.eq_ignore_ascii_case(name)) {
                set_name(&mut v, &l.name);
                *l = serde_json::from_value(v).map_err(|e| e.to_string())?;
            }
        }
        Kind::TextStyle => {
            if let Some(l) = d.text_styles.iter_mut().find(|l| l.name.eq_ignore_ascii_case(name)) {
                set_name(&mut v, &l.name);
                *l = serde_json::from_value(v).map_err(|e| e.to_string())?;
            }
        }
        Kind::DimStyle => {
            if let Some(l) = d.dim_styles.iter_mut().find(|l| l.name.eq_ignore_ascii_case(name)) {
                set_name(&mut v, &l.name);
                *l = serde_json::from_value(v).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn report(path: &str, files: &[String], list: &[Value], fixed: usize) -> String {
    let issue = |p: &Value| {
        format!(
            "{} \"{}\": {}",
            p["kind"].as_str().unwrap_or_default(),
            p["name"].as_str().unwrap_or_default(),
            p["issue"].as_str().unwrap_or_default()
        )
    };
    if path.to_ascii_lowercase().ends_with(".html") || path.to_ascii_lowercase().ends_with(".htm") {
        let mut h = String::from(
            "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Standards report</title></head><body>\n<h1>Standards report</h1>\n",
        );
        h.push_str(&format!("<p>Checked against: {}</p>\n<p>{} problem(s), {fixed} fixed.</p>\n<ul>\n", html_escape(&files.join(", ")), list.len()));
        for p in list {
            h.push_str(&format!("<li>{}</li>\n", html_escape(&issue(p))));
        }
        h.push_str("</ul>\n</body></html>\n");
        h
    } else {
        let mut t = format!("Standards report\nChecked against: {}\n{} problem(s), {fixed} fixed.\n", files.join(", "), list.len());
        for p in list {
            t.push_str(&format!("  {}\n", issue(p)));
        }
        t
    }
}

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "checkstandards";
    let files = associated(s.doc()?);
    if files.is_empty() {
        return Err(bad(cmd, "no standards files are associated (STANDARDS add)"));
    }
    let host = host(s);
    let standards: Vec<Drawing> = files.iter().map(|f| read(cmd, f, host.as_deref())).collect::<Result<_>>()?;
    let mut d = s.doc()?.clone();
    let mut fixed = 0usize;
    let fix_all = p.get("fixAll").and_then(Value::as_bool).unwrap_or(false);
    let fixes: Vec<(String, Option<String>)> = match p.get("fix") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|f| {
                let id = str_param(f, "id").ok_or_else(|| bad(cmd, "each fix needs an `id`"))?;
                Ok((id.to_string(), str_param(f, "replaceWith").map(str::to_string)))
            })
            .collect::<Result<_>>()?,
        Some(_) => return Err(bad(cmd, "`fix` is a list of {id, replaceWith?}")),
    };
    let found = problems(&d, &standards);
    for pr in &found {
        let asked = fixes.iter().find(|(id, _)| id.eq_ignore_ascii_case(&pr.id()));
        let r = match (pr.differs, asked) {
            (true, Some(_)) => take_standard(&mut d, pr.kind, &pr.name, &standards),
            (true, None) if fix_all => take_standard(&mut d, pr.kind, &pr.name, &standards),
            (false, Some((_, Some(to)))) => replace(&mut d, pr.kind, &pr.name, to, &standards),
            (false, Some((_, None))) => Err(format!("{}: choose a standard {} to replace it with (replaceWith)", pr.id(), pr.kind.id())),
            _ => continue,
        };
        r.map_err(|e| bad(cmd, e))?;
        fixed += 1;
    }
    if fixed > 0 {
        *s.doc_mut()? = d;
    }
    // What is left, with the standard names each kind could be replaced with.
    let d = s.doc()?;
    let left = problems(d, &standards);
    let list: Vec<Value> = left
        .iter()
        .map(|pr| {
            let replacements: Vec<String> = if pr.differs { Vec::new() } else { combined(&standards, pr.kind).into_iter().map(|(n, _)| n).collect() };
            json!({
                "id": pr.id(),
                "kind": pr.kind.id(),
                "name": pr.name,
                "issue": if pr.differs { "properties differ from the standards" } else { "not in the standards" },
                "standard": pr.standard,
                "replacements": replacements,
            })
        })
        .collect();
    let checked = [Kind::Layer, Kind::Linetype, Kind::TextStyle, Kind::DimStyle].iter().map(|k| definitions(d, *k).len()).sum::<usize>();
    let mut out = json!({ "problems": list, "fixed": fixed, "checked": checked, "files": files });
    if let Some(path) = str_param(p, "report").map(str::trim).filter(|x| !x.is_empty()) {
        let text = report(path, &files, out["problems"].as_array().map(Vec::as_slice).unwrap_or_default(), fixed);
        store(path, text.as_bytes())?;
        out["report"] = json!(path);
    }
    Ok(out)
}

fn run_laytrans(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "laytrans";
    let host = host(s);
    let mut map: Vec<(String, String)> = match (p.get("map"), str_param(p, "load")) {
        (Some(Value::Array(a)), _) => a
            .iter()
            .map(|m| match (str_param(m, "from"), str_param(m, "to")) {
                (Some(f), Some(t)) if !f.trim().is_empty() && !t.trim().is_empty() => Ok((f.trim().to_string(), t.trim().to_string())),
                _ => Err(bad(cmd, "each map entry is {from, to}")),
            })
            .collect::<Result<_>>()?,
        (Some(_), _) => return Err(bad(cmd, "`map` is a list of {from, to}")),
        (None, Some(path)) => {
            let text = crate::readfile::read_string(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
            let v: Value = serde_json::from_str(&text).map_err(|e| bad(cmd, format!("{path}: not a layer map: {e}")))?;
            v.get("map")
                .and_then(Value::as_array)
                .ok_or_else(|| bad(cmd, format!("{path}: not a layer map")))?
                .iter()
                .filter_map(|m| Some((str_param(m, "from")?.to_string(), str_param(m, "to")?.to_string())))
                .collect()
        }
        (None, None) => Vec::new(),
    };
    map.truncate(10_000);
    if let Some(path) = str_param(p, "save") {
        let v = json!({ "cadaddyLayerMap": 1, "map": map.iter().map(|(f, t)| json!({ "from": f, "to": t })).collect::<Vec<_>>() });
        store(path, serde_json::to_string_pretty(&v).unwrap_or_default().as_bytes())?;
    }
    let standards: Vec<Drawing> = match str_param(p, "standards") {
        Some(path) => vec![read(cmd, path, host.as_deref())?],
        None => Vec::new(),
    };
    let mut translated = 0;
    if p.get("map").is_some() && p.get("translate").and_then(Value::as_bool).unwrap_or(true) {
        let mut d = s.doc()?.clone();
        for (from, to) in &map {
            if d.layer(from).is_none() {
                return Err(bad(cmd, format!("no layer \"{from}\" in the drawing")));
            }
            replace(&mut d, Kind::Layer, from, to, &standards).map_err(|e| bad(cmd, e))?;
            translated += 1;
        }
        *s.doc_mut()? = d;
    }
    let targets: Vec<&str> = standards.iter().flat_map(|d| d.layers.iter().map(|l| l.name.as_str())).filter(|n| !n.contains('|')).collect();
    Ok(json!({ "translated": translated, "map": map.iter().map(|(f, t)| json!({ "from": f, "to": t })).collect::<Vec<_>>(), "targets": targets }))
}
