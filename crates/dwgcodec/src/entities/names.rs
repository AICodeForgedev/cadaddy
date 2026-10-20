//! The names entities refer to by handle: table records (spec §20.4.47–§20.4.65: block records,
//! layers, text styles, linetypes, views, UCSs, viewports, applications, dimension styles) and
//! multiline styles, each read only as far as its name, which every record starts with.

use std::collections::HashMap;

use super::{Obj, object_common};
use crate::Dwg;
use crate::classes::Classes;

/// Object types of table records and named objects (spec §20.3).
const BLOCK_HEADER: u16 = 49;
const LAYER: u16 = 51;
const STYLE: u16 = 53;
const LTYPE: u16 = 57;
const VIEW: u16 = 61;
const UCS: u16 = 63;
const VPORT: u16 = 65;
const APPID: u16 = 67;
const DIMSTYLE: u16 = 69;
const MLINESTYLE: u16 = 73;

/// The entities a block record holds (spec §20.4.49): from R2004 their handles, in R2000 the
/// first and last of a chain the entities' next links make.
#[derive(Clone, Debug)]
pub enum Contents {
    Owned(Vec<u64>),
    Chain(u64, u64),
}

/// Names by handle.
#[derive(Clone, Debug, Default)]
pub struct Names {
    names: HashMap<u64, String>,
    kinds: HashMap<u64, u16>,
    blocks: HashMap<u64, Contents>,
}

impl Names {
    /// The names of `dwg`'s table records and multiline styles; a record that can't be read is
    /// left out (entities pointing at it get the default name).
    pub fn read(dwg: &Dwg, _classes: &Classes) -> Names {
        let mut n = Names::default();
        for raw in &dwg.objects {
            if !matches!(raw.type_code, BLOCK_HEADER | LAYER | STYLE | LTYPE | VIEW | UCS | VPORT | APPID | DIMSTYLE | MLINESTYLE) {
                continue;
            }
            let Ok(mut o) = Obj::open(raw, dwg.version) else { continue };
            if object_common(&mut o).is_err() {
                continue;
            }
            if let Ok(name) = o.text() {
                n.kinds.insert(o.handle, raw.type_code);
                n.names.insert(o.handle, name);
                if raw.type_code == BLOCK_HEADER
                    && let Ok(c) = contents(&mut o)
                {
                    n.blocks.insert(o.handle, c);
                }
            }
        }
        n
    }

    /// The name of the record with `handle`.
    pub fn name(&self, handle: u64) -> Option<&str> {
        self.names.get(&handle).map(String::as_str)
    }

    /// The application name of an APPID record.
    pub fn appid(&self, handle: u64) -> Option<&str> {
        (self.kinds.get(&handle) == Some(&APPID)).then(|| self.name(handle)).flatten()
    }

    /// Every block record's entities.
    pub fn block_contents(&self) -> impl Iterator<Item = (&u64, &Contents)> {
        self.blocks.iter()
    }

    /// The name of a block record.
    pub fn block(&self, handle: u64) -> Option<&str> {
        (self.kinds.get(&handle) == Some(&BLOCK_HEADER)).then(|| self.name(handle)).flatten()
    }
}

/// A block record's entities, read on from after its name (spec §20.4.49; the same fields as
/// [`super::records`] decodes for the BLOCK_RECORD).
fn contents(o: &mut Obj) -> crate::Result<Contents> {
    use crate::Version;
    o.d.b()?;
    if !o.at_least(Version::R2007) {
        o.d.bs()?;
    }
    o.d.b()?;
    o.href()?;
    o.d.b()?;
    o.d.b()?;
    let is_xref = o.d.b()?;
    let overlaid = o.d.b()?;
    o.d.b()?;
    let owned = if o.at_least(Version::R2004) && !is_xref && !overlaid { super::count(o.d.bl()?, "block entities")? } else { 0 };
    o.d.bd3()?;
    o.text()?;
    let mut inserts = 0usize;
    while o.d.rc()? != 0 {
        inserts += 1;
        if inserts > super::MAX_ITEMS {
            return Err(crate::Error::TooLarge("a block's insert count"));
        }
    }
    // The BLOCK entity, then the entities.
    o.href()?;
    if o.at_least(Version::R2004) {
        let mut list = Vec::with_capacity(owned.min(1 << 16));
        for _ in 0..owned {
            if let Some(h) = o.href()? {
                list.push(h);
            }
        }
        Ok(Contents::Owned(list))
    } else {
        match (o.href()?, o.href()?) {
            (Some(first), Some(last)) => Ok(Contents::Chain(first, last)),
            _ => Ok(Contents::Owned(Vec::new())),
        }
    }
}
