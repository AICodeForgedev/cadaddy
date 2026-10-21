//! What other programs' DWG readers need that the DWG bridge (acadrust) doesn't do, done with
//! CADaddy's codec on the bridge's files: found by opening CADaddy's DWG files black-box in the
//! ODA's File Converter and LibreCAD (Q230). Each returns `None` when nothing changes or the file
//! can't be rewritten, so the bridge's file is used as it is.

use std::collections::HashMap;

use cadaddy_dwgcodec::Version;
use cadaddy_dwgcodec::entities::encode::{Handles, Rec};
use cadaddy_dwgcodec::entities::encode_kinds as ek;
use cadaddy_dwgcodec::entities::{self, Group, Obj, Value};

/// The class number of the class with DXF name `name`.
fn class_number(classes: &cadaddy_dwgcodec::classes::Classes, name: &str) -> Option<u16> {
    classes.classes.iter().find(|c| c.dxf_name.to_string_lossy() == name).and_then(|c| u16::try_from(c.number).ok())
}

/// A DWG whose underlay classes are named PDFREFERENCE (DWFREFERENCE, DGNREFERENCE), as the
/// ODA's converter names them when it saves a drawing, with those named PDFUNDERLAY (…), the
/// name readers know. The bridge lost every underlay of a drawing another program had saved.
pub(crate) fn underlay_classes_renamed(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut dwg = cadaddy_dwgcodec::read(bytes).ok()?;
    let mut classes = dwg.classes().ok()?;
    let mut renamed = false;
    for c in classes.classes.iter_mut() {
        let name = c.dxf_name.to_string_lossy();
        if let Some(kind) = name.strip_suffix("REFERENCE").filter(|k| matches!(*k, "PDF" | "DWF" | "DGN")) {
            c.dxf_name = crate::dwg_write::text(dwg.version, &format!("{kind}UNDERLAY"));
            renamed = true;
        }
    }
    if !renamed {
        return None;
    }
    dwg.set_classes(&classes);
    cadaddy_dwgcodec::write(&dwg).ok()
}

/// An R2000 or R2004 DWG with each image definition (IMAGEDEF) listing its reactors, as
/// AutoCAD's and CADaddy's own do: the image dictionary that owns it and its images'
/// IMAGEDEF_REACTORs.
/// acadrust writes none, and LibreCAD's reader then couldn't read the drawing's objects (it
/// opened nothing of a drawing with an image).
pub(crate) fn image_def_reactors(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut dwg = cadaddy_dwgcodec::read(bytes).ok()?;
    let v = dwg.version;
    if v > Version::R2004 {
        return None;
    }
    let classes = dwg.classes().ok()?;
    let def = class_number(&classes, "IMAGEDEF")?;
    // Each definition's reactors: an image names its definition (340) and its reactor (360).
    let mut reactors: HashMap<u64, Vec<u64>> = HashMap::new();
    let (ents, _) = entities::decode_all(&dwg).ok()?;
    for e in ents.iter().filter(|e| e.name == "IMAGE") {
        let handle = |code: i32| {
            e.groups.iter().find(|g| g.code == code).and_then(|g| match g.value {
                Value::Handle(h) => Some(h),
                _ => None,
            })
        };
        if let (Some(d), Some(r)) = (handle(340), handle(360)) {
            reactors.entry(d).or_default().push(r);
        }
    }
    let names = Handles { by_name: HashMap::new(), classes: HashMap::from([("IMAGEDEF".to_string(), def)]) };
    let mut changed = false;
    for o in dwg.objects.iter_mut().filter(|o| o.type_code == def) {
        let Some((owner, xdict, groups)) = image_def(o, v) else { continue };
        let list: Vec<u64> = owner.into_iter().chain(reactors.get(&o.handle).into_iter().flatten().copied()).collect();
        if let Some(new) = ek::object(v, "IMAGEDEF", o.handle, owner, &list, xdict, &Rec(&groups), &names, &[]) {
            *o = new;
            changed = true;
        }
    }
    if !changed {
        return None;
    }
    cadaddy_dwgcodec::write(&dwg).ok()
}

/// An image definition without reactors or extended data, as (owner, dictionary, groups); `None`
/// for one that has either (left as it is) or can't be read.
fn image_def(o: &cadaddy_dwgcodec::objects::RawObject, v: Version) -> Option<(Option<u64>, Option<u64>, Vec<Group>)> {
    let mut ob = Obj::open(o, v).ok()?;
    let c = entities::object_common(&mut ob).ok()?;
    if !c.reactors.is_empty() || !c.eed.is_empty() {
        return None;
    }
    let version = ob.d.bl().ok()?;
    let (w, h) = (ob.d.rd().ok()?, ob.d.rd().ok()?);
    let path = ob.text().ok()?;
    let loaded = ob.d.b().ok()?;
    let units = ob.d.rc().ok()?;
    let (pw, ph) = (ob.d.rd().ok()?, ob.d.rd().ok()?);
    let g = |code: i32, value: Value| Group { code, value };
    let groups = vec![
        g(90, Value::Int(i64::from(version))),
        g(10, Value::Real(w)),
        g(20, Value::Real(h)),
        g(1, Value::Str(path)),
        g(280, Value::Int(i64::from(loaded))),
        g(281, Value::Int(i64::from(units))),
        g(11, Value::Real(pw)),
        g(21, Value::Real(ph)),
    ];
    Some((c.owner, c.xdict, groups))
}
