//! 3D solids, regions and bodies (Q5113). From AutoCAD 2013 on their ACIS data is in the data
//! storage section, and the object holds only an empty body's fields: what is written here is
//! byte for byte what another writer (acadrust, a black box) writes for the same entity.
mod common;

use std::io::Cursor;

use cadaddy_dwgcodec::entities::encode::{EntityHead, Handles, Rec};
use cadaddy_dwgcodec::entities::encode_kinds::entity;
use cadaddy_dwgcodec::entities::{Group, Obj, Value, common, decode_all};

fn dxf(kind: &str, history: u64) -> String {
    let tail = if kind == "3DSOLID" { format!("100\nAcDb3dSolid\n350\n{history:X}\n") } else { String::new() };
    format!(
        "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1032\n9\n$HANDSEED\n5\n900\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n\
         0\n{kind}\n5\n2A\n330\n1F\n100\nAcDbEntity\n8\n0\n100\nAcDbModelerGeometry\n290\n0\n2\n{{00000000-0000-0000-0000-000000000000}}\n{tail}\
         0\nENDSEC\n0\nEOF\n"
    )
}

#[test]
fn an_empty_body_is_written_as_another_writer_writes_it() {
    for id in ["AC1027", "AC1032"] {
        for kind in ["3DSOLID", "REGION", "BODY"] {
            for history in [0u64, 0x21] {
                if kind != "3DSOLID" && history != 0 {
                    continue;
                }
                let mut doc = acadrust::DxfReader::from_reader(Cursor::new(dxf(kind, history).into_bytes())).and_then(|r| r.read()).expect("reads the DXF");
                doc.version = if id == "AC1027" { acadrust::DxfVersion::AC1027 } else { acadrust::DxfVersion::AC1032 };
                let bytes = acadrust::DwgWriter::write_to_vec(&doc).expect("writes the DWG");
                let d = cadaddy_dwgcodec::read(&bytes).unwrap();
                let theirs = d.objects.iter().find(|o| o.handle == 0x2A).expect("the entity");
                // The entity's own common data, as the other writer made it.
                let mut o = Obj::open(theirs, d.version).unwrap();
                let c = common(&mut o).unwrap();
                let head = EntityHead {
                    entmode: c.entmode,
                    owner: c.owner,
                    reactors: c.reactors.clone(),
                    xdict: c.xdict,
                    layer: c.layer,
                    ltype: None,
                    ltype_flags: c.ltype_flags,
                    material: None,
                    plotstyle: None,
                    links: None,
                };
                let groups = if history != 0 { vec![Group { code: 350, value: Value::Handle(history) }] } else { Vec::new() };
                let ours = entity(d.version, kind, 0x2A, &Rec(&groups), &head, &Handles::default(), &[], None).expect("encoded");
                assert_eq!(ours.type_code, theirs.type_code, "{id} {kind}");
                assert_eq!(ours.data, theirs.data, "{id} {kind} history {history:x}: the object's bytes");
            }
        }
    }
}

/// The same entity with SAT text, in every version before 2013.
fn sat_dxf(kind: &str, ver: &str) -> String {
    let tail = if kind == "3DSOLID" { "100\nAcDb3dSolid\n350\n0\n" } else { "" };
    format!(
        "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\n{ver}\n9\n$HANDSEED\n5\n900\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n\
         0\n{kind}\n5\n2A\n330\n1F\n100\nAcDbEntity\n8\n0\n100\nAcDbModelerGeometry\n70\n1\n\
         1\n400 26 1 9 0\n1\n16 Autodesk AutoCAD 19 ASM 221.0.0.1871 NT 0\n1\n1 9.9999999999999995e-007 1e-010\n1\nEnd-of-ACIS-data\n{tail}\
         0\nENDSEC\n0\nEOF\n"
    )
}

#[test]
fn sat_text_is_written_as_another_writer_writes_it() {
    for id in ["AC1015", "AC1018", "AC1021", "AC1024"] {
        for kind in ["3DSOLID", "REGION", "BODY"] {
            let mut doc = acadrust::DxfReader::from_reader(Cursor::new(sat_dxf(kind, id).into_bytes())).and_then(|r| r.read()).expect("reads the DXF");
            doc.version = match id {
                "AC1015" => acadrust::DxfVersion::AC1015,
                "AC1018" => acadrust::DxfVersion::AC1018,
                "AC1021" => acadrust::DxfVersion::AC1021,
                _ => acadrust::DxfVersion::AC1024,
            };
            let bytes = acadrust::DwgWriter::write_to_vec(&doc).expect("writes the DWG");
            let d = cadaddy_dwgcodec::read(&bytes).unwrap();
            let theirs = d.objects.iter().find(|o| o.handle == 0x2A).expect("the entity");
            let mut o = Obj::open(theirs, d.version).unwrap();
            let c = common(&mut o).unwrap();
            let head = EntityHead {
                entmode: c.entmode,
                owner: c.owner,
                reactors: c.reactors.clone(),
                xdict: c.xdict,
                layer: c.layer,
                ltype: None,
                ltype_flags: c.ltype_flags,
                material: None,
                plotstyle: None,
                links: Some((None, c.next)),
            };
            let lines = ["400 26 1 9 0", "16 Autodesk AutoCAD 19 ASM 221.0.0.1871 NT 0", "1 9.9999999999999995e-007 1e-010", "End-of-ACIS-data"];
            let groups: Vec<Group> = lines.iter().map(|l| Group { code: 1, value: Value::Str((*l).to_string()) }).collect();
            let ours = entity(d.version, kind, 0x2A, &Rec(&groups), &head, &Handles::default(), &[], None).expect("encoded");
            assert_eq!(ours.data, theirs.data, "{id} {kind}: the object's bytes");
        }
    }
}

/// The groups of the entity at handle 2A in the DXF acadrust writes from what it read of `dwg`.
fn their_groups(dwg: &[u8]) -> Vec<(i32, String)> {
    let doc = acadrust::DwgReader::from_stream(Cursor::new(dwg.to_vec())).read().expect("acadrust reads the DWG");
    let mut out = Vec::new();
    acadrust::DxfWriter::new(&doc).write_to_writer(&mut out).expect("acadrust writes DXF");
    let text = String::from_utf8_lossy(&out);
    let lines: Vec<&str> = text.lines().collect();
    let mut groups = Vec::new();
    let mut inside = false;
    for p in lines.chunks(2) {
        let [code, value] = p else { break };
        let code: i32 = code.trim().parse().unwrap_or(-1);
        if code == 0 {
            if inside {
                break;
            }
            inside = matches!(value.trim(), "3DSOLID" | "REGION" | "BODY");
            continue;
        }
        if inside {
            groups.push((code, value.trim().to_string()));
        }
    }
    groups
}

#[test]
fn solids_decode_to_the_groups_another_writer_gives() {
    for id in ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"] {
        for kind in ["3DSOLID", "REGION", "BODY"] {
            let text = if id >= "AC1027" { dxf(kind, 0x21) } else { sat_dxf(kind, id) };
            let mut doc = acadrust::DxfReader::from_reader(Cursor::new(text.into_bytes())).and_then(|r| r.read()).expect("reads the DXF");
            doc.version = match id {
                "AC1015" => acadrust::DxfVersion::AC1015,
                "AC1018" => acadrust::DxfVersion::AC1018,
                "AC1021" => acadrust::DxfVersion::AC1021,
                "AC1024" => acadrust::DxfVersion::AC1024,
                "AC1027" => acadrust::DxfVersion::AC1027,
                _ => acadrust::DxfVersion::AC1032,
            };
            let bytes = acadrust::DwgWriter::write_to_vec(&doc).expect("writes the DWG");
            let d = cadaddy_dwgcodec::read(&bytes).unwrap();
            let (ents, failed) = decode_all(&d).unwrap();
            assert!(failed.is_empty(), "{id} {kind}: {failed:?}");
            let e = ents.iter().find(|e| e.handle == 0x2A).expect("decoded");
            assert_eq!(e.name, kind);
            let mut theirs = their_groups(&bytes);
            // Its reader takes bytes after the last line for another line (a stray "Z1;…" in
            // files before 2013): not the entity's.
            if let Some(end) = theirs.iter().position(|g| g.1 == "End-of-ACIS-data")
                && theirs.get(end + 1).is_some_and(|g| g.0 == 1)
            {
                theirs.remove(end + 1);
            }
            let ours: Vec<(i32, String)> = e
                .groups
                .iter()
                .filter(|g| matches!(g.code, 70 | 1 | 3 | 290 | 2 | 350) || (g.code == 100 && matches!(&g.value, Value::Str(s) if s != "AcDbEntity")))
                .map(|g| {
                    let v = match &g.value {
                        Value::Str(s) => s.clone(),
                        Value::Int(i) => i.to_string(),
                        Value::Real(f) => f.to_string(),
                        Value::Handle(h) => format!("{h:X}"),
                    };
                    (g.code, v)
                })
                .collect();
            let theirs: Vec<(i32, String)> = theirs.into_iter().filter(|g| matches!(g.0, 70 | 1 | 3 | 290 | 2 | 350) || (g.0 == 100 && g.1 != "AcDbEntity")).collect();
            assert_eq!(ours, theirs, "{id} {kind}");
        }
    }
}
