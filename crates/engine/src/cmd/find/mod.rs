//! FIND: find and replace text across the drawing.
//!
//! Where: the whole drawing (model space and every layout), the current space, or the selection.
//! What: text, multiline text, block attribute values, dimension text overrides, table cells,
//! multileader text, hyperlinks (their link and description) and, when asked, layer and block
//! names. How: plain text, wildcards or regular expressions ([`pattern`]), match case, whole
//! words. Each hit is listed with its space, type and the text it is in; replacing can be
//! limited to chosen objects (Replace one by one). Without replace, the hits in the current
//! space become the selection.

pub mod pattern;

use cadaddy_doc::{Drawing, EntityKind, Handle, Space, hyperlink};
use serde_json::{Value, json};

use super::{bad, bool_or, str_param};
use crate::{Result, Session};
use pattern::{Mode, Pattern};

const CMD: &str = "find";
/// Most hits listed.
const MAX_HITS: usize = 10_000;

/// What FIND looks in.
#[derive(Clone, Copy, Debug)]
struct Include {
    text: bool,
    attributes: bool,
    dimensions: bool,
    tables: bool,
    mleaders: bool,
    hyperlinks: bool,
    names: bool,
}

fn include(p: &Value) -> Result<Include> {
    let all = Include { text: true, attributes: true, dimensions: true, tables: true, mleaders: true, hyperlinks: true, names: false };
    let Some(list) = p.get("include") else { return Ok(all) };
    let Some(list) = list.as_array() else {
        return Err(bad(CMD, "`include` is a list: text, attributes, dimensions, tables, mleaders, hyperlinks, names"));
    };
    let mut inc = Include { text: false, attributes: false, dimensions: false, tables: false, mleaders: false, hyperlinks: false, names: false };
    for v in list {
        match v.as_str().unwrap_or_default().to_ascii_lowercase().as_str() {
            "text" | "mtext" => inc.text = true,
            "attributes" | "attribute" => inc.attributes = true,
            "dimensions" | "dimension" => inc.dimensions = true,
            "tables" | "table" => inc.tables = true,
            "mleaders" | "multileaders" => inc.mleaders = true,
            "hyperlinks" | "hyperlink" => inc.hyperlinks = true,
            "names" | "layers" | "blocks" => inc.names = true,
            other => return Err(bad(CMD, format!("can't look in `{other}`"))),
        }
    }
    Ok(inc)
}

fn space_name(sp: &Space) -> String {
    match sp {
        Space::Model => "Model".into(),
        Space::Paper(n) => n.clone(),
    }
}

/// The spaces searched: every one, or the current.
fn spaces(d: &Drawing, all: bool, current: &Space) -> Vec<Space> {
    if all { std::iter::once(Space::Model).chain(d.layouts.iter().map(|l| Space::Paper(l.name.clone()))).collect() } else { vec![current.clone()] }
}

/// Apply `f` to each text of `k` FIND looks in; the flag is true for multiline-text contents
/// (MText, MLeader, table cells), which carry format codes that FIND must not match or change.
fn texts(k: &mut EntityKind, inc: Include, f: &mut dyn FnMut(&mut String, bool)) {
    match k {
        EntityKind::Text(t) if inc.text => f(&mut t.value, false),
        EntityKind::MText(t) if inc.text => f(&mut t.contents, true),
        EntityKind::Insert(i) if inc.attributes => i.attribs.iter_mut().for_each(|a| f(&mut a.text.value, false)),
        EntityKind::AttDef(a) if inc.attributes => f(&mut a.text.value, false),
        EntityKind::Dimension(dm) if inc.dimensions => f(&mut dm.text, false),
        EntityKind::MLeader(m) if inc.mleaders => {
            if let Some(t) = &mut m.text {
                f(&mut t.contents, true)
            }
        }
        EntityKind::Table(t) if inc.tables => t.cells.iter_mut().flatten().for_each(|c| f(&mut c.text, true)),
        _ => {}
    }
}

/// The shown text of multiline-text `src`: format codes dropped, a paragraph break `\P` a line
/// break. Also, for each shown char, the byte range it covers in `src`.
fn shown(src: &str) -> (String, Vec<(usize, usize)>) {
    let idx: Vec<(usize, char)> = src.char_indices().collect();
    let chars: Vec<char> = idx.iter().map(|(_, c)| *c).collect();
    let mask = super::spell::literal_mask(&chars, true);
    let byte = |i: usize| idx.get(i).map_or(src.len(), |(b, _)| *b);
    let (mut out, mut spans) = (String::new(), Vec::new());
    let mut i = 0;
    while i < chars.len() {
        if mask.get(i) == Some(&true) {
            out.push(chars[i]);
            spans.push((byte(i), byte(i + 1)));
            i += 1;
        } else if chars.get(i) == Some(&'\\') && chars.get(i + 1) == Some(&'P') {
            out.push('\n');
            spans.push((byte(i), byte(i + 2)));
            i += 2;
        } else {
            i += 1;
        }
    }
    (out, spans)
}

/// The matches of `pat` in the shown text of multiline text `src`.
fn count_in(pat: &Pattern, src: &str, coded: bool) -> usize {
    if coded { pat.find_all(&shown(src).0).len() } else { pat.find_all(src).len() }
}

/// `src` with the matches of `pat` in its shown text replaced; format codes stay where they are
/// (codes inside a match stay after the replacement).
fn replace_in(pat: &Pattern, src: &str, rep: &str, groups: bool, coded: bool) -> String {
    if !coded {
        return pat.replace_all(src, rep, groups).0;
    }
    let (vis, spans) = shown(src);
    // Char index of each byte offset of `vis`.
    let mut at = vec![0usize; vis.len() + 1];
    for (k, (b, c)) in vis.char_indices().enumerate() {
        at.iter_mut().skip(b).take(c.len_utf8()).for_each(|a| *a = k);
    }
    at[vis.len()] = vis.chars().count();
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0usize;
    for h in pat.find_all(&vis) {
        let (vs, ve) = (at.get(h.start).copied().unwrap_or(0), at.get(h.end).copied().unwrap_or(0));
        let (Some(first), true) = (spans.get(vs), ve > vs) else { continue };
        out.push_str(src.get(cursor..first.0).unwrap_or(""));
        pat.expand(&vis, &h, rep, groups, &mut out);
        for k in vs..ve - 1 {
            if let (Some(a), Some(b)) = (spans.get(k), spans.get(k + 1)) {
                out.push_str(src.get(a.1..b.0).unwrap_or(""));
            }
        }
        cursor = spans.get(ve - 1).map_or(src.len(), |l| l.1);
    }
    out.push_str(src.get(cursor..).unwrap_or(""));
    out
}

fn snippet(t: &str) -> String {
    let t: String = t.chars().take(120).collect();
    t.replace('\n', " ")
}

pub(crate) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let find = str_param(p, "find").ok_or_else(|| bad(CMD, "`find` is required"))?.to_string();
    let rep = str_param(p, "replace").map(str::to_string);
    let mode = match str_param(p, "mode").unwrap_or(if bool_or(p, "wildcards", false) { "wildcard" } else { "plain" }).to_ascii_lowercase().as_str() {
        "plain" | "text" => Mode::Plain,
        "wildcard" | "wildcards" => Mode::Wildcard,
        "regex" | "regexp" => Mode::Regex,
        other => return Err(bad(CMD, format!("unknown mode `{other}` (plain, wildcard or regex)"))),
    };
    let pat = Pattern::new(&find, mode, bool_or(p, "matchCase", false), bool_or(p, "wholeWord", false)).map_err(|e| bad(CMD, e))?;
    let groups = mode == Mode::Regex;
    let inc = include(p)?;
    let scope = str_param(p, "in").unwrap_or("drawing").to_ascii_lowercase();
    let only: Option<Vec<Handle>> = match p.get("handles") {
        Some(Value::Array(a)) => Some(a.iter().filter_map(|v| v.as_str().and_then(Handle::parse_hex)).collect()),
        _ => None,
    };
    let current = s.space();
    let selection: Vec<Handle> = s.state()?.selection.clone();
    let d = s.doc()?;
    let candidates: Vec<(Space, Handle)> = match scope.as_str() {
        "drawing" | "all" => spaces(d, true, &current)
            .into_iter()
            .flat_map(|sp| d.space(&sp).map(|st| st.handles()).unwrap_or_default().into_iter().map(move |h| (sp.clone(), h)))
            .collect(),
        "space" | "current" => d.space(&current).map(|st| st.handles()).unwrap_or_default().into_iter().map(|h| (current.clone(), h)).collect(),
        "selection" => selection.iter().map(|h| (d.space_of(*h).unwrap_or_else(|| current.clone()), *h)).collect(),
        other => return Err(bad(CMD, format!("`in` is drawing, space or selection, not `{other}`"))),
    };
    let mut hits: Vec<Value> = Vec::new();
    let mut total = 0usize;
    let mut edits: Vec<(Handle, EntityKind, Option<hyperlink::Hyperlink>)> = Vec::new();
    for (sp, h) in candidates {
        let Some(e) = d.entity(h) else { continue };
        let mut k = e.kind.clone();
        let mut n = 0usize;
        let mut found_in: Option<String> = None;
        let replacing = rep.is_some() && only.as_ref().is_none_or(|o| o.contains(&h));
        texts(&mut k, inc, &mut |t: &mut String, coded: bool| {
            let c = count_in(&pat, t, coded);
            if c > 0 {
                n += c;
                found_in.get_or_insert_with(|| snippet(&if coded { shown(t).0 } else { t.clone() }));
                if replacing && let Some(r) = &rep {
                    *t = replace_in(&pat, t, r, groups, coded);
                }
            }
        });
        let mut link = None;
        if inc.hyperlinks
            && let Some(mut l) = hyperlink::read(&e.common.xdata)
        {
            let mut changed = false;
            for t in [&mut l.url, &mut l.description] {
                let c = pat.find_all(t).len();
                if c > 0 {
                    n += c;
                    found_in.get_or_insert_with(|| snippet(t));
                    if replacing && let Some(r) = &rep {
                        *t = pat.replace_all(t, r, groups).0;
                        changed = true;
                    }
                }
            }
            if changed {
                link = Some(l);
            }
        }
        if n == 0 {
            continue;
        }
        total += n;
        if hits.len() < MAX_HITS {
            hits.push(json!({ "handle": h.hex(), "space": space_name(&sp), "type": e.kind.type_name(), "text": found_in, "count": n }));
        }
        if replacing {
            edits.push((h, k, link));
        }
    }
    // Layer and block names.
    let mut renames: Vec<(&str, String, String)> = Vec::new();
    if inc.names {
        let layers = d.layers.iter().map(|l| ("layer", l.name.clone())).filter(|(_, n)| n != "0" && !n.eq_ignore_ascii_case("defpoints"));
        let blocks = d.blocks.iter().filter(|(n, b)| !n.starts_with('*') && b.xref_path.is_none()).map(|(n, _)| ("block", n.clone()));
        for (table, name) in layers.chain(blocks) {
            let c = pat.find_all(&name).len();
            if c == 0 {
                continue;
            }
            total += c;
            hits.push(json!({ "name": name, "type": if table == "layer" { "Layer name" } else { "Block name" }, "text": name, "count": c }));
            // Limited to chosen objects (`handles`) and no `names` given: names stay as they are.
            let wanted = match p.get("names").and_then(Value::as_array) {
                Some(a) => a.iter().any(|v| v.as_str() == Some(name.as_str())),
                None => only.is_none(),
            };
            if let (Some(r), true) = (&rep, wanted) {
                let to = pat.replace_all(&name, r, groups).0;
                if to != name && !to.trim().is_empty() {
                    renames.push((table, name, to));
                }
            }
        }
    }
    let objects = edits.len();
    if rep.is_some() {
        let doc = s.doc_mut()?;
        for (h, k, link) in edits {
            doc.modify_entity(h, |e| {
                e.kind = k;
                if let EntityKind::Dimension(dm) = &mut e.kind {
                    dm.block = None;
                }
                if let Some(l) = &link {
                    hyperlink::write(&mut e.common.xdata, Some(l));
                }
            })?;
        }
        for (table, from, to) in &renames {
            // In this command's undo step.
            super::props::run_rename(s, &json!({ "table": table, "from": from, "to": to }))?;
        }
    } else if bool_or(p, "select", true) {
        let sel: Vec<Handle> = hits
            .iter()
            .filter(|v| v["space"].as_str() == Some(space_name(&current).as_str()))
            .filter_map(|v| v["handle"].as_str().and_then(Handle::parse_hex))
            .collect();
        s.set_selection(sel);
    }
    let msg = if rep.is_some() {
        format!("{total} replacement(s) in {} object(s)", objects + renames.len())
    } else {
        format!("{total} match(es) in {} place(s)", hits.len())
    };
    Ok(json!({ "matches": hits, "total": total, "message": msg }))
}
