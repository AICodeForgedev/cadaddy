//! Why an item can't be purged (the Purge dialog's Find Non-Purgeable Items view): reserved
//! names, the current setting, other table entries that name it (a layer's linetype, a
//! dimension style's text style), objects in model space or a layout (grouped by type, layer and
//! space, with their handles to select or zoom to), and block definitions that use it (with the
//! references to those blocks that are placed in the drawing).

use std::collections::{BTreeMap, HashSet};

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, STANDARD_MLINE_STYLE};
use serde_json::{Value, json};

use super::Kind;

/// Handles listed with one reason.
const MAX_HANDLES: usize = 1000;

/// Names of `kind` that entity `e` uses.
fn uses(kind: Kind, e: &Entity) -> Vec<String> {
    match kind {
        Kind::Layers => vec![e.common.layer.clone()],
        Kind::Linetypes => vec![e.common.linetype.clone()],
        Kind::Blocks => match &e.kind {
            EntityKind::Insert(i) => vec![i.block.clone()],
            EntityKind::Dimension(dm) => dm.block.iter().cloned().collect(),
            EntityKind::MLeader(m) => m.block.iter().map(|b| b.block.clone()).collect(),
            _ => Vec::new(),
        },
        Kind::TextStyles => match &e.kind {
            EntityKind::Text(t) => vec![t.style.clone()],
            EntityKind::MText(t) => vec![t.style.clone()],
            EntityKind::AttDef(a) => vec![a.text.style.clone()],
            EntityKind::Insert(i) => i.attribs.iter().map(|a| a.text.style.clone()).collect(),
            EntityKind::MLeader(m) => m.text.iter().map(|t| t.style.clone()).collect(),
            _ => Vec::new(),
        },
        Kind::Dimstyles => match &e.kind {
            EntityKind::Dimension(dm) => vec![dm.style.clone()],
            EntityKind::Tolerance(t) => vec![t.style.clone()],
            _ => Vec::new(),
        },
        Kind::MleaderStyles => match &e.kind {
            EntityKind::MLeader(m) => vec![m.style.clone()],
            _ => Vec::new(),
        },
        Kind::MlineStyles => match &e.kind {
            EntityKind::MLine(m) => vec![m.style.clone()],
            _ => Vec::new(),
        },
        Kind::TableStyles => match &e.kind {
            EntityKind::Table(t) => vec![t.style.clone()],
            _ => Vec::new(),
        },
        Kind::Groups | Kind::ZeroLength | Kind::EmptyText | Kind::Absent | Kind::All => Vec::new(),
    }
}

fn uses_name(kind: Kind, e: &Entity, name: &str) -> bool {
    uses(kind, e).iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// Names of block definitions that contain `name` (directly), and the references placed in
/// model space or a layout to them or to blocks that nest them.
fn in_blocks(d: &Drawing, kind: Kind, name: &str) -> Vec<(String, Vec<Handle>)> {
    let mut out = Vec::new();
    for b in d.blocks.values().take(100_000) {
        if b.xref_path.is_some() || !b.entities.iter().any(|e| uses_name(kind, e, name)) {
            continue;
        }
        out.push((b.name.clone(), placed(d, &b.name)));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The references in model space and the layouts that show block `name`, through nesting.
fn placed(d: &Drawing, name: &str) -> Vec<Handle> {
    // Blocks that contain `name`, transitively (bounded).
    let mut wanted: HashSet<String> = HashSet::from([name.to_ascii_uppercase()]);
    for _ in 0..cadaddy_doc::MAX_BLOCK_DEPTH {
        let before = wanted.len();
        for b in d.blocks.values() {
            if !wanted.contains(&b.name.to_ascii_uppercase())
                && b.entities.iter().any(|e| uses(Kind::Blocks, e).iter().any(|n| wanted.contains(&n.to_ascii_uppercase())))
            {
                wanted.insert(b.name.to_ascii_uppercase());
            }
        }
        if wanted.len() == before {
            break;
        }
    }
    top(d)
        .filter(|(_, e)| uses(Kind::Blocks, e).iter().any(|n| wanted.contains(&n.to_ascii_uppercase())))
        .map(|(_, e)| e.handle)
        .take(MAX_HANDLES)
        .collect()
}

/// Model space and layout objects with their space's name.
fn top(d: &Drawing) -> impl Iterator<Item = (&str, &std::sync::Arc<Entity>)> {
    d.model.iter().map(|e| ("Model", e)).chain(d.layouts.iter().flat_map(|l| l.entities.iter().map(move |e| (l.name.as_str(), e)))).take(5_000_000)
}

fn reason(text: String, handles: Vec<Handle>) -> Value {
    json!({ "text": text, "handles": handles.iter().map(|h| h.hex()).collect::<Vec<_>>() })
}

/// Why `name` (of `kind`) can't be purged; empty when nothing holds it.
pub(super) fn why(d: &Drawing, kind: Kind, name: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let fixed = |t: &str| reason(t.to_string(), Vec::new());
    let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    if name.contains('|') && kind != Kind::Groups {
        out.push(fixed("Comes from an xref: detach or bind the xref instead"));
    }
    match kind {
        Kind::Layers => {
            if eq(name, "0") {
                out.push(fixed("Layer 0 is always kept"));
            }
            if eq(name, "Defpoints") {
                out.push(fixed("Defpoints is kept for dimension definition points"));
            }
            if eq(name, &d.header.str("CLAYER", "0")) {
                out.push(fixed("The current layer"));
            }
        }
        Kind::Linetypes => {
            if ["ByLayer", "ByBlock", "Continuous"].iter().any(|r| eq(r, name)) {
                out.push(fixed("A standard linetype that is always kept"));
            }
            if eq(name, &d.header.str("CELTYPE", "ByLayer")) {
                out.push(fixed("The current linetype"));
            }
            for l in d.layers.iter().filter(|l| eq(&l.linetype, name)) {
                out.push(reason(format!("The linetype of layer {}", l.name), Vec::new()));
            }
        }
        Kind::TextStyles => {
            if eq(name, "Standard") {
                out.push(fixed("The Standard style is always kept"));
            }
            if eq(name, &d.header.str("TEXTSTYLE", "Standard")) {
                out.push(fixed("The current text style"));
            }
            for s in d.dim_styles.iter().filter(|s| eq(&s.text_style, name)) {
                out.push(reason(format!("Referenced by dimension style {}", s.name), Vec::new()));
            }
            for s in d.mleader_styles.iter().filter(|s| eq(&s.text_style, name)) {
                out.push(reason(format!("Referenced by multileader style {}", s.name), Vec::new()));
            }
        }
        Kind::Dimstyles => {
            if eq(name, "Standard") {
                out.push(fixed("The Standard style is always kept"));
            }
            if eq(name, &d.header.str("DIMSTYLE", "Standard")) {
                out.push(fixed("The current dimension style"));
            }
        }
        Kind::MleaderStyles => {
            if eq(name, "Standard") {
                out.push(fixed("The Standard style is always kept"));
            }
            if eq(name, &d.header.str("CMLEADERSTYLE", "Standard")) {
                out.push(fixed("The current multileader style"));
            }
        }
        Kind::MlineStyles => {
            if eq(name, STANDARD_MLINE_STYLE) {
                out.push(fixed("The Standard style is always kept"));
            }
            if eq(name, &d.header.str("CMLSTYLE", STANDARD_MLINE_STYLE)) {
                out.push(fixed("The current multiline style"));
            }
        }
        Kind::TableStyles => {
            if eq(name, "Standard") {
                out.push(fixed("The Standard style is always kept"));
            }
            if eq(name, &d.header.str("CTABLESTYLE", "Standard")) {
                out.push(fixed("The current table style"));
            }
        }
        Kind::Blocks => {
            for s in d.dim_styles.iter().filter(|s| eq(&s.arrow_block, name)) {
                out.push(reason(format!("The arrowhead of dimension style {}", s.name), Vec::new()));
            }
            for s in d.mleader_styles.iter().filter(|s| eq(&s.block, name)) {
                out.push(reason(format!("The content block of multileader style {}", s.name), Vec::new()));
            }
            if d.block(name).is_some_and(|b| b.xref_path.is_some()) {
                out.push(fixed("An attached xref: detach it in External References"));
            }
        }
        Kind::Groups => {
            if let Some(g) = d.groups.iter().find(|g| eq(&g.name, name)) {
                let live: Vec<Handle> = g.members.iter().copied().filter(|h| d.entity(*h).is_some()).take(MAX_HANDLES).collect();
                if !live.is_empty() {
                    out.push(reason(format!("Has {} member object(s)", live.len()), live));
                }
            }
        }
        Kind::ZeroLength | Kind::EmptyText | Kind::Absent | Kind::All => {}
    }
    // Objects placed in the drawing, grouped by type, layer and space.
    let mut groups: BTreeMap<(String, String, String), Vec<Handle>> = BTreeMap::new();
    for (space, e) in top(d) {
        if uses_name(kind, e, name) {
            let g = groups.entry((space.to_string(), e.kind.type_name().to_string(), e.common.layer.clone())).or_default();
            if g.len() < MAX_HANDLES {
                g.push(e.handle);
            }
        }
    }
    for ((space, ty, layer), hs) in groups {
        let n = hs.len();
        let what = if n == 1 { format!("a {ty} object") } else { format!("{n} {ty} objects") };
        out.push(reason(format!("Used by {what} on layer {layer} in {space}"), hs));
    }
    // Block definitions.
    for (block, refs) in in_blocks(d, kind, name) {
        if kind == Kind::Blocks && block.eq_ignore_ascii_case(name) {
            continue;
        }
        out.push(reason(format!("Used in the definition of block {}", cadaddy_doc::dynblock::display_name(d, &block)), refs));
    }
    out
}

/// Every name of a named kind.
pub(super) fn all_names(d: &Drawing, kind: Kind) -> Vec<String> {
    match kind {
        Kind::Blocks => d.blocks.values().map(|b| b.name.clone()).collect(),
        Kind::Layers => d.layers.iter().map(|l| l.name.clone()).collect(),
        Kind::Linetypes => d.linetypes.iter().map(|t| t.name.clone()).collect(),
        Kind::TextStyles => d.text_styles.iter().map(|t| t.name.clone()).collect(),
        Kind::Dimstyles => d.dim_styles.iter().map(|t| t.name.clone()).collect(),
        Kind::MleaderStyles => d.mleader_styles.iter().map(|t| t.name.clone()).collect(),
        Kind::MlineStyles => d.mline_styles.iter().map(|t| t.name.clone()).collect(),
        Kind::TableStyles => d.table_styles.iter().map(|t| t.name.clone()).collect(),
        Kind::Groups => d.groups.iter().map(|g| g.name.clone()).collect(),
        Kind::ZeroLength | Kind::EmptyText | Kind::Absent | Kind::All => Vec::new(),
    }
}
