//! What DWG and DXF don't keep, feature by feature with counts and handles: CHECKEXCHANGE without
//! saving, and the first DWG/DXF save of a drawing (`notKeptDetail`, the app's notice).

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
    // DXF saves are written as they are made, as the app writes them.
    cadaddy_engine::cmd::file::set_write_streamed(|d, n, o, open| {
        cadaddy_io::write_streamed(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }, open).map_err(|e| e.to_string())
    });
}

/// A line with transparency, a circle in a true colour, a 3D solid and a multiline.
fn drawing() -> (Session, [String; 4]) {
    let mut s = Session::new();
    let line = s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap()["handles"][0].as_str().unwrap().to_string();
    let circle = s.execute("circle", &json!({"center": [5, 5], "radius": 2})).unwrap()["handle"].as_str().unwrap().to_string();
    let solid =
        s.execute("box", &json!({"corner": [20, 0, 0], "length": 5, "width": 5, "height": 5})).unwrap()["handle"].as_str().unwrap().to_string();
    let mline = s.execute("mline", &json!({"points": [[0, 20], [10, 20]]})).unwrap()["handle"].as_str().unwrap().to_string();
    s.execute("chprop", &json!({"handles": [line], "transparency": 50})).unwrap();
    s.execute("chprop", &json!({"handles": [circle], "color": "10,200,30"})).unwrap();
    (s, [line, circle, solid, mline])
}

fn loss<'a>(r: &'a Value, feature: &str) -> Option<&'a Value> {
    r["losses"].as_array()?.iter().find(|l| l["feature"] == feature)
}

#[test]
fn current_dwg_and_dxf_keep_a_drawing_cadaddy_made() {
    hooks();
    let (mut s, _) = drawing();
    for f in ["dwg", "dxf"] {
        let r = s.execute("checkexchange", &json!({"format": f})).unwrap();
        assert_eq!(r["kept"], true, "{f}: {r}");
        assert!(r["message"].as_str().unwrap().contains("keeps everything"));
    }
}

#[test]
fn each_lossy_feature_is_listed_with_its_objects() {
    hooks();
    let (mut s, [line, circle, solid, mline]) = drawing();
    // Release 2000 DWG: no transparency, no true colours.
    let r = s.execute("checkexchange", &json!({"format": "dwg", "version": 2000})).unwrap();
    assert_eq!(r["kept"], false);
    assert_eq!(r["version"], "AC1015");
    assert_eq!(
        loss(&r, "object transparency").map(|l| (&l["what"], &l["count"], &l["handles"])),
        Some((&json!("changed"), &json!(1), &json!([line]))),
        "{r}"
    );
    assert_eq!(loss(&r, "object color").map(|l| &l["handles"]), Some(&json!([circle])), "{r}");
    assert!(loss(&r, "solid3d objects").is_none(), "R2000 DWG keeps solids: {r}");
    // Release 12 DXF: no solids, multilines or second layouts either (solids and multilines are
    // written as what R12 has; the line and circle as themselves, not reported).
    let r = s.execute("checkexchange", &json!({"format": "dxf", "version": 12})).unwrap();
    assert_eq!(
        loss(&r, "solid3d objects (written as face3d)").map(|l| (&l["what"], &l["handles"])),
        Some((&json!("changed"), &json!([solid]))),
        "{r}"
    );
    assert_eq!(loss(&r, "mline objects (written as line)").map(|l| &l["handles"]), Some(&json!([mline])), "{r}");
    assert_eq!(r["losses"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(loss(&r, "layouts").map(|l| &l["names"]), Some(&json!(["Layout2"])), "{r}");
    assert!(loss(&r, "linetypes: name").is_none(), "upper-cased names aren't a loss: {r}");
    let msg = r["message"].as_str().unwrap();
    assert!(
        msg.starts_with("Not kept in a DXF file: ") && msg.contains("1 solid3d objects (written as face3d) changed") && msg.contains(".cdy"),
        "{msg}"
    );
    // Bad options.
    assert!(s.execute("checkexchange", &json!({"format": "pdf"})).is_err());
    assert!(s.execute("checkexchange", &json!({"version": 1999})).is_err());
    // Nothing was saved and nothing changed.
    assert!(s.state().unwrap().path.is_none());
    assert!(s.state().unwrap().exchange_notice.is_none());
}

#[test]
fn the_first_lossy_save_leaves_a_notice_for_the_app() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-exchangeloss-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (mut s, [line, ..]) = drawing();
    let path = dir.join("old.dwg").display().to_string();
    let r = s.execute("saveas", &json!({"path": path, "version": 2000})).unwrap();
    let detail = r["notKeptDetail"].as_array().unwrap();
    assert!(detail.iter().any(|l| l["feature"] == "object transparency" && l["handles"] == json!([line])), "{r}");
    let notice = s.state().unwrap().exchange_notice.clone().unwrap();
    assert_eq!(notice["format"], "DWG");
    assert_eq!(notice["losses"], r["notKeptDetail"]);
    assert!(s.log.iter().any(|l| l.contains("Not kept in the DWG file: ") && l.contains("object transparency changed")), "{:?}", s.log);
    // Once per drawing.
    s.state_mut().unwrap().exchange_notice = None;
    let r = s.execute("qsave", &json!({})).unwrap();
    assert!(r.get("notKeptDetail").is_none());
    assert!(s.state().unwrap().exchange_notice.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A drawing is checked once per target (format, version, binary), not once: a save into a
/// lossier version after a clean one is checked too, and saving again into a checked one is not.
#[test]
fn each_new_target_is_checked_once() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-exchangeloss-targets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (mut s, [line, _, solid, mline]) = drawing();
    let at = |name: &str| dir.join(name).display().to_string();
    // A current DWG keeps everything: nothing to tell.
    let r = s.execute("saveas", &json!({"path": at("now.dwg")})).unwrap();
    assert!(r.get("notKeptDetail").is_none(), "{r}");
    assert!(s.state().unwrap().exchange_notice.is_none());
    // The same drawing as a 2000 DWG loses the transparency.
    let r = s.execute("saveas", &json!({"path": at("old.dwg"), "version": 2000})).unwrap();
    let detail = r["notKeptDetail"].as_array().unwrap();
    assert!(detail.iter().any(|l| l["feature"] == "object transparency" && l["handles"] == json!([line])), "{r}");
    // Saving it again where it is (a 2000 DWG now) is a target already checked.
    s.state_mut().unwrap().exchange_notice = None;
    let r = s.execute("qsave", &json!({})).unwrap();
    assert!(r.get("notKeptDetail").is_none(), "{r}");
    assert!(s.state().unwrap().exchange_notice.is_none());
    // A Release 12 DXF is another target: the solid, multiline and second layout are listed.
    let r = s.execute("saveas", &json!({"path": at("r12.dxf"), "version": 12})).unwrap();
    let detail = r["notKeptDetail"].as_array().unwrap();
    let feature = |f: &str| detail.iter().find(|l| l["feature"] == f);
    assert_eq!(feature("solid3d objects (written as face3d)").map(|l| &l["handles"]), Some(&json!([solid])), "{r}");
    assert_eq!(feature("mline objects (written as line)").map(|l| &l["handles"]), Some(&json!([mline])), "{r}");
    assert_eq!(feature("layouts").map(|l| &l["names"]), Some(&json!(["Layout2"])), "{r}");
    // And the first target is still checked.
    let r = s.execute("saveas", &json!({"path": at("again.dwg")})).unwrap();
    assert!(r.get("notKeptDetail").is_none(), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A made-up drawing with another program's data: a custom class's object in the named object
/// dictionary reacting to a line (which has an extension dictionary), and a custom entity.
const ACME: &str = "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n9\n$HANDSEED\n5\n400\n0\nENDSEC\n\
0\nSECTION\n2\nCLASSES\n0\nCLASS\n1\nACME_WIDGET\n2\nAcmeWidget\n3\nAcmeApp\n90\n0\n280\n0\n281\n0\n0\nCLASS\n1\nACME_GADGET\n2\nAcmeGadget\n3\nAcmeApp\n90\n0\n280\n0\n281\n1\n0\nENDSEC\n\
0\nSECTION\n2\nENTITIES\n0\nLINE\n5\n100\n102\n{ACAD_XDICTIONARY\n360\n200\n102\n}\n102\n{ACAD_REACTORS\n330\n300\n102\n}\n100\nAcDbEntity\n8\n0\n100\nAcDbLine\n10\n0\n20\n0\n30\n0\n11\n10\n21\n5\n31\n0\n\
0\nACME_GADGET\n5\n110\n100\nAcDbEntity\n8\n0\n100\nAcmeGadget\n10\n3\n20\n4\n30\n0\n1\ngadget label\n0\nENDSEC\n\
0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n3\nACME_WIDGETS\n350\n300\n0\n\
DICTIONARY\n5\n200\n330\n100\n100\nAcDbDictionary\n3\nACME_DATA\n350\n202\n0\n\
XRECORD\n5\n202\n330\n200\n100\nAcDbXrecord\n1\nper-line payload\n0\n\
ACME_WIDGET\n5\n300\n102\n{ACAD_REACTORS\n330\n100\n102\n}\n330\nC\n100\nAcmeWidget\n1\nspinner\n90\n7\n0\nENDSEC\n0\nEOF\n";

#[test]
fn other_programs_data_goes_through_dwg_and_its_loss_is_listed() {
    hooks();
    let mut s = Session::new();
    s.open_drawing(cadaddy_io::read(ACME.as_bytes(), "acme.dxf").unwrap(), "acme", None);
    // DWG keeps it all: nothing of it to tell. (It is saved as 2004 DWG, which has no fixed
    // extension lines, so the dimension style is listed: a version loss, not this drawing's data.)
    let r = s.execute("checkexchange", &json!({"format": "dwg"})).unwrap();
    let custom = ["other programs", "classes", "dictionary entries", "reactors", "extension dictionaries", "acme"];
    let listed: Vec<&str> = r["losses"].as_array().unwrap().iter().filter_map(|l| l["feature"].as_str()).collect();
    assert!(!listed.iter().any(|f| custom.iter().any(|c| f.contains(c))), "{r}");
    // Release 12 DXF has no objects or classes: each part of it is listed.
    let r = s.execute("checkexchange", &json!({"format": "dxf", "version": 12})).unwrap();
    assert_eq!(r["kept"], false);
    assert_eq!(loss(&r, "acme_widget objects of other programs").map(|l| &l["what"]), Some(&json!("dropped")), "{r}");
    assert!(loss(&r, "classes of other programs' objects").is_some_and(|l| l["names"].as_array().unwrap().len() == 2), "{r}");
    assert!(loss(&r, "named dictionary entries").is_some_and(|l| l["names"] == json!(["ACME_WIDGETS"])), "{r}");
    assert_eq!(loss(&r, "reactors").map(|l| &l["handles"]), Some(&json!(["100"])), "{r}");
    assert_eq!(loss(&r, "acme_gadget objects").map(|l| &l["what"]), Some(&json!("dropped")), "{r}");
}

/// A made-up drawing whose named object dictionary holds another program's dictionaries: an empty
/// one, and one of named records (like a style list) pointing at its records, which react to it.
const STYLES: &str = "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1021\n9\n$HANDSEED\n5\n400\n0\nENDSEC\n\
0\nSECTION\n2\nENTITIES\n0\nLINE\n5\n100\n100\nAcDbEntity\n8\n0\n100\nAcDbLine\n10\n0\n20\n0\n30\n0\n11\n10\n21\n5\n31\n0\n0\nENDSEC\n\
0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n3\nACME_EMPTY\n350\n210\n3\nACME_STYLES\n350\n211\n0\n\
DICTIONARY\n5\n210\n330\nC\n100\nAcDbDictionary\n280\n0\n281\n1\n0\n\
DICTIONARY\n5\n211\n330\nC\n100\nAcDbDictionary\n280\n0\n281\n1\n3\nAlpha\n350\n220\n3\nBeta\n350\n221\n0\n\
XRECORD\n5\n220\n102\n{ACAD_REACTORS\n330\n211\n102\n}\n330\n211\n100\nAcDbXrecord\n280\n1\n90\n0\n0\n\
XRECORD\n5\n221\n102\n{ACAD_REACTORS\n330\n211\n102\n}\n330\n211\n100\nAcDbXrecord\n280\n1\n90\n0\n91\n-2104163840\n340\n100\n0\n\
ENDSEC\n0\nEOF\n";

/// Another program's objects come back from a DWG or DXF under new handles, their references
/// pointing at the renumbered copies: nothing of them is lost, on the first save or any later one
/// (a file CADaddy wrote, saved again, reported the same dictionary changed every time: Q6273).
#[test]
fn other_programs_objects_written_back_are_kept_on_every_save() {
    hooks();
    let mut d = cadaddy_io::read(STYLES.as_bytes(), "styles.dxf").unwrap();
    let kinds = |d: &cadaddy_doc::Drawing| d.foreign.objects.iter().map(|o| o.kind.to_ascii_uppercase()).collect::<Vec<_>>();
    assert_eq!(kinds(&d), ["DICTIONARY", "DICTIONARY", "XRECORD", "XRECORD"]);
    for round in 0..3 {
        let mut s = Session::new();
        s.open_drawing(d.clone(), "styles", None);
        for f in ["dwg", "dxf"] {
            let r = s.execute("checkexchange", &json!({"format": f})).unwrap();
            let listed: Vec<&str> = r["losses"].as_array().unwrap().iter().filter_map(|l| l["feature"].as_str()).collect();
            assert!(
                !listed.iter().any(|f| f.contains("other programs") || f.contains("dictionar") || f.contains("reactors")),
                "round {round}, {f}: {r}"
            );
        }
        d = cadaddy_io::read(&cadaddy_io::write(&d, "styles.dxf").unwrap(), "styles.dxf").unwrap();
        assert_eq!(kinds(&d), ["DICTIONARY", "DICTIONARY", "XRECORD", "XRECORD"], "round {round}");
    }
    // A record whose reference no longer points at the line (it was erased): its copy's reference
    // is nulled, which is no loss of the file's.
    let mut s = Session::new();
    s.open_drawing(d, "styles", None);
    s.execute("erase", &json!({"handles": ["100"]})).unwrap();
    let r = s.execute("checkexchange", &json!({"format": "dxf"})).unwrap();
    assert!(loss(&r, "xrecord objects of other programs").is_none(), "{r}");
}
