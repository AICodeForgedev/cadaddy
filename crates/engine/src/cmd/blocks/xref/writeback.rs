//! Writing an xref edited in place (REFEDIT on an xref) back to its source drawing.
//!
//! The source drawing is read again and checked against the xref as it was loaded (its model
//! space objects, in order, must be the ones the xref shows), so changes made to the file since
//! it was loaded are never overwritten. Objects left unchanged are kept exactly as the file has
//! them; changed ones keep their handles; new ones get new handles. Names the xref brought as
//! `xref|name` lose the prefix again, and host symbols the new objects use (layers, linetypes,
//! text styles, blocks) are copied into the source drawing. The previous file is kept as `.bak`.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use cadaddy_doc::{Block, Drawing, Entity, EntityKind, EntityStore, Handle};

use super::load::{Sym, has_prefix, rename_refs, shared};

/// Largest source drawing read back.
#[cfg(not(target_arch = "wasm32"))]
const MAX_BYTES: u64 = 512 << 20;
/// Most host blocks copied into the source drawing (nested ones included).
const MAX_BLOCKS: usize = 10_000;

/// The source file of xref `key` in the drawing at `host` (an error says why there is none).
pub(crate) fn source(d: &Drawing, key: &str, host: Option<&str>) -> Result<std::path::PathBuf, String> {
    let b = d.block(key).ok_or_else(|| format!("xref \"{key}\" not found"))?;
    let path = b.xref_path.clone().ok_or_else(|| format!("\"{key}\" is not an xref"))?;
    if b.xref_status != cadaddy_doc::XrefStatus::Loaded {
        return Err(format!("xref \"{key}\" is not loaded; reload it first"));
    }
    if cfg!(target_arch = "wasm32") {
        return Err("files are not available on the web".into());
    }
    let host = host.map(Path::new).filter(|p| p.file_name().is_some());
    super::load::find(&path, host, false).map_err(|_| format!("the source drawing of \"{key}\" ({path}) was not found"))
}

#[cfg(not(target_arch = "wasm32"))]
fn read(file: &Path) -> Result<Drawing, String> {
    use std::io::Read;
    let hooks = crate::cmd::file::io().ok_or("file formats are not available in this build")?;
    let f = crate::readfile::open(file).map_err(|e| e.to_string())?;
    if !f.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular file".into());
    }
    let mut bytes = Vec::new();
    f.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("the file is larger than {} MB", MAX_BYTES >> 20));
    }
    (hooks.read)(&bytes, &file.to_string_lossy())
}
#[cfg(target_arch = "wasm32")]
fn read(_file: &Path) -> Result<Drawing, String> {
    Err("files are not available on the web".into())
}

/// A source object as the xref shows it (as loading makes it), with handle `h`.
fn as_loaded(e: &Entity, key: &str, h: Handle) -> Entity {
    let mut e = e.clone();
    e.handle = h;
    rename_refs(&mut e, &|sym, n| (!shared(sym, n)).then(|| format!("{key}|{n}")));
    if let EntityKind::Dimension(dm) = &mut e.kind {
        dm.assoc.clear();
    }
    e
}

/// The host symbols (kind, name) objects `es` use that aren't the xref's own.
fn host_symbols(es: &[Entity], key: &str) -> BTreeSet<(Sym, String)> {
    let prefix = format!("{key}|");
    let found = std::cell::RefCell::new(BTreeSet::new());
    for e in es {
        let mut c = e.clone();
        rename_refs(&mut c, &|sym, n| {
            if !shared(sym, n) && !has_prefix(n, &prefix) {
                found.borrow_mut().insert((sym, n.to_string()));
            }
            None
        });
    }
    found.into_inner()
}

/// An object of the edited xref as the source drawing has it: `xref|` prefixes dropped.
fn to_source(e: &Entity, key: &str, h: Handle) -> Entity {
    let prefix = format!("{key}|");
    let mut e = e.clone();
    e.handle = h;
    rename_refs(&mut e, &|_, n| has_prefix(n, &prefix).then(|| n.get(prefix.len()..).unwrap_or_default().to_string()));
    e
}

/// Copy host symbols `syms` (and the symbols and blocks they use in turn) into source drawing `x`
/// when it has none of that name.
fn copy_symbols(x: &mut Drawing, host: &Drawing, syms: BTreeSet<(Sym, String)>, key: &str) -> Result<(), String> {
    let mut todo: Vec<(Sym, String)> = syms.into_iter().collect();
    let mut blocks = 0usize;
    while let Some((sym, n)) = todo.pop() {
        match sym {
            Sym::Layer => {
                if x.layer(&n).is_none()
                    && let Some(l) = host.layer(&n)
                {
                    todo.push((Sym::Linetype, l.linetype.clone()));
                    x.layers.push(l.clone());
                }
            }
            Sym::Linetype => {
                if !shared(sym, &n)
                    && x.linetype(&n).is_none()
                    && let Some(t) = host.linetype(&n)
                {
                    x.linetypes.push(t.clone());
                }
            }
            Sym::Style => {
                if x.text_style(&n).is_none()
                    && let Some(t) = host.text_style(&n)
                {
                    x.text_styles.push(t.clone());
                }
            }
            Sym::DimStyle => {
                if x.dim_style(&n).is_none()
                    && let Some(t) = host.dim_style(&n)
                {
                    todo.push((Sym::Style, t.text_style.clone()));
                    x.dim_styles.push(t.clone());
                }
            }
            Sym::MLeaderStyle => {
                if !x.mleader_styles.iter().any(|o| o.name.eq_ignore_ascii_case(&n))
                    && let Some(t) = host.mleader_styles.iter().find(|o| o.name.eq_ignore_ascii_case(&n))
                {
                    x.mleader_styles.push(t.clone());
                }
            }
            Sym::TableStyle => {
                if !x.table_styles.iter().any(|o| o.name.eq_ignore_ascii_case(&n))
                    && let Some(t) = host.table_styles.iter().find(|o| o.name.eq_ignore_ascii_case(&n))
                {
                    x.table_styles.push(t.clone());
                }
            }
            Sym::Block => {
                if x.block(&n).is_some() {
                    continue;
                }
                let Some(b) = host.block(&n) else { continue };
                if b.xref_path.is_some() {
                    return Err(format!("the edit refers to xref \"{n}\" of this drawing, which the source drawing can't hold"));
                }
                blocks += 1;
                if blocks > MAX_BLOCKS {
                    return Err("the edit refers to too many blocks".into());
                }
                let mut nb = Block::clone(b);
                let es: Vec<Entity> = b.entities.iter().map(|e| (**e).clone()).collect();
                todo.extend(host_symbols(&es, key));
                let mut store = EntityStore::new();
                for e in es {
                    let mut e = e;
                    e.handle = x.new_handle();
                    store.push(e);
                }
                nb.entities = store;
                x.blocks.insert(nb.name.clone(), Arc::new(nb));
            }
        }
    }
    Ok(())
}

/// Write xref `key`'s edited objects back to its source file `file`. `loaded` is the xref block
/// as it was loaded (before the edit), `edited` the block after it, `host` the host drawing.
pub(crate) fn write_back(file: &Path, key: &str, loaded: &Block, edited: &Block, host: &Drawing) -> Result<(), String> {
    let name = file.to_string_lossy().into_owned();
    // The path came from the drawing: only a drawing file outside the system folders is written.
    let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !matches!(ext.as_str(), "dwg" | "dxf" | "cdy") {
        return Err(format!("{name} isn't a drawing file (.dwg, .dxf or .cdy), so the xref isn't written back there"));
    }
    crate::writefile::protected(file).map_err(|e| e.to_string())?;
    let mut x = read(file).map_err(|e| format!("{name}: {e}"))?;
    // The file must still be what the xref shows.
    let current: Vec<Entity> = x.model.iter().map(|e| (**e).clone()).collect();
    let same = current.len() == loaded.entities.len() && current.iter().zip(loaded.entities.iter()).all(|(s, l)| as_loaded(s, key, l.handle) == **l);
    if !same {
        return Err(format!("{name} has changed since \"{key}\" was loaded; reload the xref and edit it again"));
    }
    // Loaded object → source object.
    let source_of: HashMap<Handle, usize> = loaded.entities.iter().enumerate().map(|(i, e)| (e.handle, i)).collect();
    let new_objects: Vec<Entity> = edited.entities.iter().filter(|e| !source_of.contains_key(&e.handle)).map(|e| (**e).clone()).collect();
    let changed: Vec<Entity> = edited
        .entities
        .iter()
        .filter(|e| source_of.contains_key(&e.handle) && loaded.entities.get(e.handle).is_none_or(|l| **l != ***e))
        .map(|e| (**e).clone())
        .collect();
    copy_symbols(&mut x, host, host_symbols(&new_objects.iter().chain(&changed).cloned().collect::<Vec<_>>(), key), key)?;
    let mut store = EntityStore::new();
    for e in edited.entities.iter() {
        match source_of.get(&e.handle).and_then(|i| current.get(*i)) {
            Some(src) => {
                let unchanged = loaded.entities.get(e.handle).is_some_and(|l| **l == **e);
                store.push(if unchanged { src.clone() } else { to_source(e, key, src.handle) });
            }
            None => {
                let h = x.new_handle();
                store.push(to_source(e, key, h));
            }
        }
    }
    x.model = store;
    let hooks = crate::cmd::file::io().ok_or("file formats are not available in this build")?;
    let bytes = (hooks.write)(&x, &name).map_err(|e| format!("{name}: {e}"))?;
    crate::cmd::file::store(&name, &bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_drawing_files_outside_system_folders_are_written_back() {
        let (b, d) = (Block::new("X"), Drawing::default());
        for (path, why) in [
            ("/Users/x/.zshrc", "isn't a drawing"),
            ("/tmp/notes.txt", "isn't a drawing"),
            ("/etc/evil.dwg", "system folder"),
            ("C:\\Windows\\x.dxf", "system folder"),
        ] {
            let e = write_back(Path::new(path), "X", &b, &b, &d).unwrap_err();
            assert!(e.contains(why), "{path}: {e}");
        }
    }
}
