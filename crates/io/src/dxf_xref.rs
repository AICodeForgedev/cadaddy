//! External references in a written DXF (DXF Reference, BLOCK): an xref is a BLOCK flagged 4
//! (8 for an overlay) whose group 1 holds the referenced drawing's path, with no objects; its
//! objects come from that drawing when the host is opened. Symbols that came with a loaded xref
//! (layers, linetypes, styles and blocks named `xref|name`) belong to the referenced drawing and
//! are not written as the host's own.

use std::sync::Arc;

use cadcraft_doc::Drawing;

/// Whether a symbol name belongs to an attached xref (`xref|name`).
fn dependent(name: &str) -> bool {
    name.contains('|')
}

/// The drawing as it is saved: xref blocks emptied, xref-dependent symbols dropped. `None` when
/// the drawing has neither (it is saved as it is).
pub(crate) fn saved_view(d: &Drawing) -> Option<Drawing> {
    let has = d.blocks.values().any(|b| b.xref_path.is_some() || dependent(&b.name))
        || d.layers.iter().any(|l| dependent(&l.name))
        || d.linetypes.iter().any(|l| dependent(&l.name))
        || d.text_styles.iter().any(|t| dependent(&t.name))
        || d.dim_styles.iter().any(|t| dependent(&t.name))
        || d.mleader_styles.iter().any(|t| dependent(&t.name))
        || d.table_styles.iter().any(|t| dependent(&t.name));
    if !has {
        return None;
    }
    let mut v = d.clone();
    v.blocks.retain(|name, _| !dependent(name));
    for b in v.blocks.values_mut() {
        if b.xref_path.is_some() && !b.entities.is_empty() {
            Arc::make_mut(b).entities = Default::default();
        }
    }
    v.layers.retain(|l| !dependent(&l.name));
    v.linetypes.retain(|l| !dependent(&l.name));
    v.text_styles.retain(|t| !dependent(&t.name));
    v.dim_styles.retain(|t| !dependent(&t.name));
    v.mleader_styles.retain(|t| !dependent(&t.name));
    v.table_styles.retain(|t| !dependent(&t.name));
    Some(v)
}
