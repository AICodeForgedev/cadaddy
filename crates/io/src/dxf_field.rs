//! Fields in DXF (DXF Reference, FIELD): a text, multiline text, attribute or attribute
//! definition holding fields has an extension dictionary with an `ACAD_FIELD` dictionary, whose
//! `TEXT` entry is a `_text` FIELD: the text with each field as `%<\_FldIdx i>%`, owning one FIELD
//! per field (its evaluator, code and cached value, group 301). The text's own group 1 holds what
//! it shows (the values in place of the codes), so programs that don't read fields show that.
//!
//! In the drawing the codes are inline in the text and the values in
//! [`cadaddy_doc::Drawing::field_values`]; an object a code names is `%<\_ObjId N>%` there and
//! `%<\_ObjIdx i>%` with group 331 here.

use std::collections::{HashMap, HashSet};

use cadaddy_doc::field;
use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_dxf::Tag;

/// Fields followed inside one field (nested codes).
const MAX_DEPTH: usize = 8;
/// FIELD objects read per file.
pub(crate) const MAX_FIELDS: usize = 1_000_000;
/// Longest field code or value read.
const MAX_TEXT: usize = 65_536;

/// Which text of an object a field dictionary belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// A text's, multiline text's or attribute definition's own.
    Own,
    /// A block reference's attribute, by its index.
    Attrib(usize),
}

/// An object with an extension dictionary that may hold fields.
#[derive(Clone, Debug)]
pub(crate) struct Pending {
    pub entity: Handle,
    pub slot: Slot,
    pub xdict: String,
}

/// The extension dictionary handle (upper case) of a record.
pub(crate) fn xdict_of(tags: &[Tag]) -> Option<String> {
    let i = tags.iter().position(|t| t.code == 102 && t.str().trim().eq_ignore_ascii_case("{ACAD_XDICTIONARY"))?;
    tags.get(i + 1).filter(|t| t.code == 360).map(|t| t.str().trim().to_ascii_uppercase())
}

/// The entry `key` of a DICTIONARY (handle, upper case).
fn entry(tags: &[Tag], key: &str) -> Option<String> {
    let mut name: Option<String> = None;
    for t in tags {
        match t.code {
            3 => name = Some(t.str()),
            350 | 360 => {
                // Each name goes with the handle after it, matched or not.
                let n = name.take();
                if n.is_some_and(|n| n.trim().eq_ignore_ascii_case(key)) {
                    return Some(t.str().trim().to_ascii_uppercase());
                }
            }
            _ => {}
        }
    }
    None
}

/// One FIELD object read.
struct Field {
    evaluator: String,
    code: String,
    children: Vec<String>,
    objects: Vec<Handle>,
    value: String,
}

fn read_field(tags: &[Tag]) -> Field {
    let start = tags.iter().position(|x| x.code == 100 && x.str() == "AcDbField").unwrap_or(0);
    let tags = tags.get(start..).unwrap_or(&[]);
    let evaluator = tags.iter().find(|t| t.code == 1).map(|t| t.str()).unwrap_or_default();
    let mut code = String::new();
    if let Some(i) = tags.iter().position(|t| t.code == 2) {
        for t in tags.get(i..).unwrap_or(&[]).iter().take_while(|t| t.code == 2 || t.code == 3) {
            if code.len() < MAX_TEXT {
                code.push_str(&t.str());
            }
        }
    }
    // The children follow the child count (the first 90); the objects the object count (97).
    let after = |code: i32, item: i32| -> Vec<String> {
        let Some(i) = tags.iter().position(|t| t.code == code) else { return Vec::new() };
        tags.get(i + 1..).unwrap_or(&[]).iter().take_while(|t| t.code == item).map(|t| t.str().trim().to_ascii_uppercase()).take(10_000).collect()
    };
    let children = after(90, 360);
    let objects = after(97, 331).iter().filter_map(|h| Handle::parse_hex(h)).collect();
    // The cached value: the last 301 (with its 9 overflow).
    let mut value = String::new();
    if let Some(i) = tags.iter().rposition(|t| t.code == 301) {
        for t in tags.get(i..).unwrap_or(&[]).iter().take_while(|t| t.code == 301 || t.code == 9) {
            if value.len() < MAX_TEXT {
                value.push_str(&t.str());
            }
        }
    }
    if value.is_empty() {
        value = stored_value(tags);
    }
    Field { evaluator, code, children, objects, value }
}

/// The value a field keeps when its formatted copy (301) is empty, as a DWG through the bridge
/// gives it back: the formatted value (302), else the value data of its `ACFD_FIELD_VALUE` block
/// (data type 90: 4 text in group 1, 1 a whole number in 91, 2 a number in 140).
fn stored_value(tags: &[Tag]) -> String {
    let Some(start) = tags.iter().position(|t| t.code == 7 && t.str().trim() == "ACFD_FIELD_VALUE") else { return String::new() };
    let block: Vec<&Tag> = tags.get(start + 1..).unwrap_or(&[]).iter().take_while(|t| !(t.code == 304 && t.str().trim() == "ACVALUE_END")).collect();
    if let Some(t) = block.iter().find(|t| t.code == 302).filter(|t| !t.str().is_empty()) {
        return t.str().chars().take(MAX_TEXT).collect();
    }
    let kind = block.iter().find(|t| t.code == 90).and_then(|t| t.str().trim().parse::<i64>().ok());
    let group = match kind {
        Some(4) => 1,
        Some(1) => 91,
        Some(2) => 140,
        _ => return String::new(),
    };
    let mut v = String::new();
    for t in block.iter().skip_while(|t| t.code != group).take_while(|t| t.code == group || (group == 1 && t.code == 3)) {
        if v.len() < MAX_TEXT {
            v.push_str(t.str().trim_end_matches('\0'));
        }
    }
    v
}

/// A field's code with its children's codes put in place of `%<\_FldIdx i>%` and its objects as
/// `%<\_ObjId N>%`.
fn inline(fields: &HashMap<String, Vec<Tag>>, f: &Field, seen: &mut HashSet<String>, depth: usize) -> String {
    let code = field::from_indexed(&f.code, &f.objects);
    if f.children.is_empty() || depth >= MAX_DEPTH {
        return code;
    }
    replace_indexes(&code, |k| {
        let h = f.children.get(k)?;
        if !seen.insert(h.clone()) {
            return None;
        }
        let child = read_field(fields.get(h)?);
        Some(format!("%<{}>%", inline(fields, &child, seen, depth + 1)))
    })
}

/// `s` with each `%<\_FldIdx k>%` replaced by `f(k)` (left as it is when `f` gives nothing).
fn replace_indexes(s: &str, mut f: impl FnMut(usize) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("%<\\_FldIdx") {
        out.push_str(rest.get(..i).unwrap_or(""));
        let after = rest.get(i + 10..).unwrap_or("");
        let close = after.find(">%");
        let k = close.and_then(|c| after.get(..c)).and_then(|n| n.trim().parse::<usize>().ok());
        match (k.and_then(&mut f), close) {
            (Some(v), Some(c)) => {
                out.push_str(&v);
                rest = after.get(c + 2..).unwrap_or("");
            }
            _ => {
                out.push_str("%<\\_FldIdx");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The text with its codes inline and the field values in order, from an object's extension
/// dictionary `xdict`; `None` when it holds no fields.
pub(crate) fn resolve(fields: &HashMap<String, Vec<Tag>>, dicts: &HashMap<String, Vec<Tag>>, xdict: &str) -> Option<(String, Vec<String>)> {
    let inner = entry(dicts.get(xdict)?, "ACAD_FIELD")?;
    let root_h = entry(dicts.get(&inner)?, "TEXT")?;
    let root = read_field(fields.get(&root_h)?);
    if !root.evaluator.trim().eq_ignore_ascii_case("_text") {
        return None;
    }
    let mut values = Vec::new();
    let mut seen = HashSet::from([root_h]);
    let text = replace_indexes(&root.code, |k| {
        let h = root.children.get(k)?;
        if !seen.insert(h.clone()) {
            return None;
        }
        let child = read_field(fields.get(h)?);
        let code = inline(fields, &child, &mut seen, 1);
        if child.evaluator.is_empty() || code.is_empty() {
            return None;
        }
        values.push(child.value.clone());
        Some(format!("%<{code}>%"))
    });
    (!values.is_empty()).then_some((text, values))
}

/// Put the fields read into the drawing: each object's text gets its codes back, and its values
/// go into [`Drawing::field_values`] in text order.
pub(crate) fn apply(d: &mut Drawing, pending: &[Pending], fields: &HashMap<String, Vec<Tag>>, dicts: &HashMap<String, Vec<Tag>>) {
    let mut by_entity: HashMap<Handle, Vec<(Slot, String, Vec<String>)>> = HashMap::new();
    for p in pending {
        if let Some((text, values)) = resolve(fields, dicts, &p.xdict) {
            by_entity.entry(p.entity).or_default().push((p.slot, text, values));
        }
    }
    for (h, slots) in by_entity {
        let set = |k: &mut EntityKind| {
            for (slot, text, _) in &slots {
                match (slot, &mut *k) {
                    (Slot::Own, EntityKind::Text(t)) => t.value = text.clone(),
                    (Slot::Own, EntityKind::MText(m)) => m.contents = text.clone(),
                    (Slot::Own, EntityKind::AttDef(a)) => a.text.value = text.clone(),
                    (Slot::Attrib(i), EntityKind::Insert(ins)) => {
                        if let Some(a) = ins.attribs.get_mut(*i) {
                            a.text.value = text.clone();
                        }
                    }
                    _ => {}
                }
            }
        };
        let changed = if d.entity(h).is_some() { d.modify_entity(h, |e| set(&mut e.kind)).is_ok() } else { modify_block_entity(d, h, set) };
        if !changed {
            continue;
        }
        // The values in text order: each text's own, else `####` for codes nothing gave a value.
        let kind = d.entity(h).map(|e| e.kind.clone()).or_else(|| d.blocks.values().find_map(|b| b.entities.get(h).map(|e| e.kind.clone())));
        let Some(kind) = kind else { continue };
        let texts = field::texts(&kind);
        let mut values = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            let slot = if matches!(kind, EntityKind::Insert(_)) { Slot::Attrib(i) } else { Slot::Own };
            let n = field::codes(t).len();
            let given = slots.iter().find(|(s, _, _)| *s == slot).map(|(_, _, v)| v.as_slice()).unwrap_or(&[]);
            values.extend((0..n).map(|k| given.get(k).cloned().unwrap_or_else(|| field::NO_VALUE.into())));
        }
        if !values.is_empty() {
            d.field_values.insert(h, values);
        }
    }
}

/// Change entity `h` inside whichever block definition holds it.
fn modify_block_entity(d: &mut Drawing, h: Handle, f: impl FnOnce(&mut EntityKind)) -> bool {
    let Some(name) = d.blocks.iter().find(|(_, b)| b.entities.contains(h)).map(|(n, _)| n.clone()) else { return false };
    let Some(b) = d.blocks.get_mut(&name) else { return false };
    std::sync::Arc::make_mut(b).entities.modify(h, |e| f(&mut e.kind))
}

/// The FIELD objects of one text, to write: the `_text` field and one per code.
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    /// The object the text belongs to.
    pub owner: String,
    pub xdict: String,
    pub dict: String,
    pub root: String,
    /// The text with `%<\_FldIdx i>%` for each field.
    pub template: String,
    pub children: Vec<Child>,
}

#[derive(Clone, Debug)]
pub(crate) struct Child {
    pub handle: String,
    pub evaluator: String,
    /// The code with its objects as `%<\_ObjIdx i>%`.
    pub code: String,
    pub objects: Vec<Handle>,
    pub format: String,
    pub value: String,
}

/// The FIELD objects for `text` (its codes with `values`, in order) belonging to `owner`, with
/// handles from `next`, and the text as it shows; `None` when it holds no fields.
pub(crate) fn plan(text: &str, values: &[String], owner: String, mut next: impl FnMut() -> String) -> Option<(Plan, String)> {
    let spans = field::spans(text);
    if spans.is_empty() {
        return None;
    }
    let shown = field::substitute(text, &mut values.iter().map(String::as_str), None);
    let mut template = String::with_capacity(text.len());
    let mut children = Vec::new();
    let mut last = 0;
    let (xdict, dict, root) = (next(), next(), next());
    for (k, r) in spans.iter().enumerate() {
        template.push_str(text.get(last..r.start).unwrap_or(""));
        template.push_str(&format!("%<\\_FldIdx {k}>%"));
        last = r.end;
        let inner = text.get(r.start + 2..r.end - 2).unwrap_or("");
        let c = field::Code::parse(inner);
        let (code, objects) = field::to_indexed(inner);
        children.push(Child {
            handle: next(),
            evaluator: c.evaluator,
            code,
            objects,
            format: c.format.unwrap_or_default(),
            value: values.get(k).cloned().unwrap_or_else(|| field::NO_VALUE.into()),
        });
    }
    template.push_str(text.get(last..).unwrap_or(""));
    Some((Plan { owner, xdict, dict, root, template, children }, shown))
}

/// `s` in pieces of at most 250 bytes (whole characters), for a code and its overflow groups.
pub(crate) fn chunks(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let mut end = rest.len().min(250);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (a, b) = rest.split_at(end);
        out.push(a);
        rest = b;
    }
    if out.is_empty() {
        out.push("");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_and_resolve_agree() {
        let text = "Area: %<\\AcObjProp Object(%<\\_ObjId 26>%).Area \\f \"%lu2%pr1\">% on %<\\AcVar Date>%";
        let mut n = 0x100;
        let (p, shown) = plan(text, &["12.5".into()], "1A".into(), || {
            n += 1;
            format!("{n:X}")
        })
        .unwrap();
        assert_eq!(shown, "Area: 12.5 on ####");
        assert_eq!(p.template, "Area: %<\\_FldIdx 0>% on %<\\_FldIdx 1>%");
        assert_eq!(p.children[0].code, "\\AcObjProp Object(%<\\_ObjIdx 0>%).Area \\f \"%lu2%pr1\"");
        assert_eq!(p.children[0].objects, [Handle(26)]);
        assert_eq!(p.children[1].evaluator, "AcVar");
        assert_eq!(replace_indexes("a %<\\_FldIdx 1>% %<\\_FldIdx x>%", |k| Some(k.to_string())), "a 1 %<\\_FldIdx x>%");
        assert_eq!(chunks(&"é".repeat(200)).iter().map(|c| c.len()).collect::<Vec<_>>(), [250, 150]);
    }
}
