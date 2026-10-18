//! Multileader depth through DXF and DWG: several landings (MULTILEADER `LEADER{` roots), the
//! per-object overrides (leader colour 91, linetype 341, arrowhead 342, text frame 292) and the
//! style's leader type (173) and block content (170, 343, 177).

use cadaddy_doc::color::Color;
use cadaddy_doc::*;
use cadaddy_geom::Vec3;

fn mleaders(d: &Drawing) -> Vec<&MLeader> {
    d.model.iter().filter_map(|e| if let EntityKind::MLeader(m) = &e.kind { Some(m) } else { None }).collect()
}

fn drawing() -> (Drawing, MLeader) {
    let mut d = Drawing::new_metric();
    // A block for the style's block content.
    let mut b = Block::new("TAGB");
    b.entities.push(Entity::new(Handle(0x900), EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 1.0 })));
    d.blocks.insert("TAGB".into(), std::sync::Arc::new(b));
    d.mleader_styles.push(MLeaderStyle {
        name: "Tags".into(),
        leader_type: "spline".into(),
        block: "TAGB".into(),
        block_center: false,
        ..MLeaderStyle::default()
    });
    let lt = d.linetypes.iter().find(|l| !l.pattern.is_empty()).map(|l| l.name.clone());
    let m = MLeader {
        leaders: vec![vec![Vec3::new(0.0, 0.0, 0.0)]],
        landing: Vec3::new(5.0, 5.0, 0.0),
        dogleg: 1.0,
        text: Some(MText {
            background: None,
            insert: Vec3::new(6.5, 5.0, 0.0),
            height: 0.5,
            width: 0.0,
            attach: 4,
            rotation: 0.0,
            style: "Standard".into(),
            contents: "NOTE".into(),
            line_spacing: 1.0,
            line_spacing_exact: false,
            columns: None,
        }),
        style: "Standard".into(),
        arrow_size: 0.4,
        spline: false,
        block: None,
        roots: vec![LeaderRoot {
            landing: Vec3::new(12.0, 5.0, 0.0),
            dogleg: 1.0,
            dir: -1.0,
            leaders: vec![vec![Vec3::new(20.0, 0.0, 0.0)], vec![Vec3::new(20.0, 9.0, 0.0)]],
        }],
        leader_color: Some(Color::Index(1)),
        leader_linetype: lt,
        arrow: Some("_DOT".into()),
        text_frame: true,
    };
    d.add(&Space::Model, Common::default(), EntityKind::MLeader(m.clone())).unwrap();
    (d, m)
}

fn same(a: &MLeader, b: &MLeader) {
    assert_eq!(a.leaders, b.leaders);
    assert_eq!(a.landing, b.landing);
    assert_eq!(a.roots, b.roots);
    assert_eq!(a.leader_color, b.leader_color);
    assert_eq!(a.leader_linetype.as_deref().map(str::to_ascii_uppercase), b.leader_linetype.as_deref().map(str::to_ascii_uppercase));
    assert_eq!(a.arrow.as_deref().map(str::to_ascii_uppercase), b.arrow.as_deref().map(str::to_ascii_uppercase));
    assert_eq!(a.text_frame, b.text_frame);
}

#[test]
fn landings_and_overrides_through_dxf() {
    let (d, m) = drawing();
    let opts = cadaddy_io::SaveOptions { version: Some(cadaddy_io::Version::R2010), binary: false };
    let text = String::from_utf8(cadaddy_io::write_with(&d, "x.dxf", &opts).unwrap()).unwrap();
    assert_eq!(text.matches("LEADER{").count(), 2, "two leader roots");
    let back = cadaddy_io::read_dxf(text.as_bytes()).unwrap();
    let got = mleaders(&back);
    assert_eq!(got.len(), 1);
    same(got[0], &m);
    // The style's leader type and block content.
    let st = back.mleader_styles.iter().find(|s| s.name == "Tags").unwrap();
    assert_eq!((st.leader_type.as_str(), st.block.as_str(), st.block_center), ("spline", "TAGB", false));
}

#[test]
fn colours_pack_into_32_bits() {
    let (mut d, _) = drawing();
    for c in [Color::Index(5), Color::True(cadaddy_color::Rgb(10, 200, 30)), Color::ByLayer] {
        let h = d.model.iter().next().unwrap().handle;
        d.modify_entity(h, |e| {
            if let EntityKind::MLeader(m) = &mut e.kind {
                m.leader_color = Some(c);
            }
        })
        .unwrap();
        let opts = cadaddy_io::SaveOptions { version: Some(cadaddy_io::Version::R2010), binary: false };
        let back = cadaddy_io::read(&cadaddy_io::write_with(&d, "x.dxf", &opts).unwrap(), "x.dxf").unwrap();
        assert_eq!(mleaders(&back)[0].leader_color, Some(c));
    }
}

/// Through the DWG bridge.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn landings_and_overrides_through_dwg() {
    let (d, m) = drawing();
    let dwg = cadaddy_io::write(&d, "x.dwg").unwrap();
    let back = cadaddy_io::read(&dwg, "x.dwg").unwrap();
    let got = mleaders(&back);
    assert_eq!(got.len(), 1, "{:?}", back.model.iter().map(|e| e.kind.dxf_name()).collect::<Vec<_>>());
    same(got[0], &m);
}

#[test]
fn an_older_file_gets_every_landings_leaders() {
    let (d, _) = drawing();
    let opts = cadaddy_io::SaveOptions { version: Some(cadaddy_io::Version::R2000), binary: false };
    let back = cadaddy_io::read(&cadaddy_io::write_with(&d, "x.dxf", &opts).unwrap(), "x.dxf").unwrap();
    let leaders = back.model.iter().filter(|e| matches!(e.kind, EntityKind::Leader(_))).count();
    assert_eq!(leaders, 3);
}
