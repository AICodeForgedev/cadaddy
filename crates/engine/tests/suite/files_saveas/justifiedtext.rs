//! Justified text (centred, middle, right, or any vertical justification but baseline) is placed
//! by its alignment point (DXF group 11); its first alignment point (group 10) is derived from it,
//! and a DWG/DXF writer works it out again. A file whose stored first point isn't what CADaddy
//! would work out (another program's font metrics) saves clean, for TEXT, an attribute definition
//! and a block reference's attributes. A real move of the alignment point is still reported.

use cadaddy_doc::{Attrib, Block, Common, Drawing, Entity, EntityKind, HAlign, Handle, Insert, Space, Text, VAlign};
use cadaddy_engine::Session;
use cadaddy_geom::Vec3;
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
}

/// Text justified `h`/`v` at `at`, whose stored first point is `first` (not what CADaddy works out).
fn text(value: &str, at: [f64; 2], first: [f64; 2], h: HAlign, v: VAlign) -> Text {
    Text {
        insert: Vec3::new(first[0], first[1], 0.0),
        align_pt: Some(Vec3::new(at[0], at[1], 0.0)),
        height: 2.5,
        value: value.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: h,
        valign: v,
    }
}

fn attrib(tag: &str, t: Text) -> Attrib {
    Attrib {
        tag: tag.into(),
        text: t,
        invisible: false,
        constant: false,
        prompt: String::new(),
        props: Default::default(),
        multiline: None,
        modes: Default::default(),
    }
}

/// A drawing as another program saved it: justified texts, a block with justified attribute
/// definitions and a reference to it, all with a stored first point off by about half a height
/// from what CADaddy would work out.
fn drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    let justs = [
        (HAlign::Middle, VAlign::Baseline),
        (HAlign::Center, VAlign::Baseline),
        (HAlign::Right, VAlign::Baseline),
        (HAlign::Left, VAlign::Middle),
        (HAlign::Center, VAlign::Top),
        (HAlign::Right, VAlign::Bottom),
    ];
    for (i, (h, v)) in justs.into_iter().enumerate() {
        let y = 10.0 * i as f64;
        d.add(&Space::Model, Common::default(), EntityKind::Text(text("Room", [40.0, y], [37.3, y - 1.2], h, v))).unwrap();
    }
    let mut block = Block::new("Tag");
    let def = attrib("NUMBER", text("101", [0.0, 0.0], [-1.6, -1.25], HAlign::Center, VAlign::Baseline));
    block.entities.push(Entity::new(Handle(0x7000), EntityKind::AttDef(Box::new(def))));
    let def = attrib("NAME", text("OFFICE", [0.0, -4.0], [-4.1, -5.2], HAlign::Middle, VAlign::Baseline));
    block.entities.push(Entity::new(Handle(0x7001), EntityKind::AttDef(Box::new(def))));
    d.blocks.insert("Tag".into(), std::sync::Arc::new(block));
    let mut number = attrib("NUMBER", text("101", [100.0, 50.0], [98.4, 48.75], HAlign::Center, VAlign::Baseline));
    // Rounding noise of another program's width factor is not a change either.
    number.text.width_factor = 0.999_999_999_999_999_9;
    let name = attrib("NAME", text("OFFICE", [100.0, 46.0], [95.9, 44.8], HAlign::Middle, VAlign::Baseline));
    let insert = Insert {
        block: "Tag".into(),
        insert: Vec3::new(100.0, 50.0, 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: vec![number, name],
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    d.add(&Space::Model, Common::default(), EntityKind::Insert(insert)).unwrap();
    d
}

/// `d` saved as `name` in a private folder: the save's result.
fn save(d: Drawing, name: &str) -> Value {
    let mut s = Session::new();
    s.open_drawing(d, "justified", None);
    let dir = std::env::temp_dir().join(format!("cadaddy-justifiedtext-{}-{}", std::process::id(), name.replace('.', "-")));
    std::fs::create_dir_all(&dir).unwrap();
    let r = s.execute("saveas", &json!({"path": dir.join(name).display().to_string()})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[test]
fn justified_text_whose_stored_first_point_differs_saves_clean() {
    hooks();
    for f in ["dxf", "dwg"] {
        let r = save(drawing(), &format!("just.{f}"));
        assert!(r.get("notKept").is_none() && r.get("notKeptDetail").is_none(), "{f}: {r}");
    }
}

#[test]
fn a_text_whose_alignment_point_the_file_moved_is_still_reported() {
    hooks();
    // A copy read back with one text's alignment point moved: reported.
    let d = drawing();
    let mut back = d.clone();
    let h = back.model.iter().find(|e| matches!(e.kind, EntityKind::Text(_))).map(|e| e.handle).unwrap();
    back.modify_entity(h, |e| {
        if let EntityKind::Text(t) = &mut e.kind {
            t.align_pt = Some(Vec3::new(41.0, 0.0, 0.0));
        }
    })
    .unwrap();
    let lost = cadaddy_engine::cmd::file::kept::losses_between(&d, &back);
    assert!(lost.iter().any(|l| l.feature == "text align pt" && l.what == "changed"), "{lost:?}");
    // And the first point of left-baseline text is what places it: a change in it is reported.
    let mut d = Drawing::new_imperial();
    let h = d.add(&Space::Model, Common::default(), EntityKind::Text(text("Left", [0.0, 0.0], [5.0, 5.0], HAlign::Left, VAlign::Baseline))).unwrap();
    let mut back = d.clone();
    back.modify_entity(h, |e| {
        if let EntityKind::Text(t) = &mut e.kind {
            t.insert = Vec3::new(6.0, 5.0, 0.0);
        }
    })
    .unwrap();
    let lost = cadaddy_engine::cmd::file::kept::losses_between(&d, &back);
    assert!(lost.iter().any(|l| l.feature == "text insert" && l.what == "changed"), "{lost:?}");
}
