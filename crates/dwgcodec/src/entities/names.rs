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

/// Names by handle.
#[derive(Clone, Debug, Default)]
pub struct Names {
    names: HashMap<u64, String>,
    kinds: HashMap<u64, u16>,
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

    /// The name of a block record.
    pub fn block(&self, handle: u64) -> Option<&str> {
        (self.kinds.get(&handle) == Some(&BLOCK_HEADER)).then(|| self.name(handle)).flatten()
    }
}
