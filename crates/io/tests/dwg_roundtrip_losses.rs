//! Data that used to be lost only when a drawing is saved as DWG and reopened (the DWG bridge
//! converts our DXF with acadrust), and the paper-space viewport the DXF writer adds for it.
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_color::{Color, Rgb};
use cadaddy_doc::*;
use cadaddy_geom::{Vec2, Vec3};

fn through_dwg(d: &Drawing) -> Drawing {
    let dwg = cadaddy_io::write(d, "x.dwg").unwrap();
    cadaddy_io::read(&dwg, "x.dwg").unwrap()
}

fn line(d: &mut Drawing, space: &Space, common: Common) -> Handle {
    d.add(space, common, EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 5.0, 0.0) })).unwrap()
}

fn viewport(id: u32) -> EntityKind {
    EntityKind::Viewport(Viewport {
        center: Vec3::new(50.0, 40.0, 0.0),
        width: 80.0,
        height: 60.0,
        view_center: Vec2::new(10.0, 10.0),
        view_height: 50.0,
        id,
        locked: false,
        frozen_layers: vec![],
        layer_colors: vec![],
        layer_linetypes: Vec::new(),
        layer_lineweights: Vec::new(),
        clip: None,
        on: true,
        twist: 0.0,
        anno_scale: None,
    })
}

/// The (code, value) pairs of every `kind` record in DXF text.
fn records(text: &str, kind: &str) -> Vec<Vec<(String, String)>> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut out: Vec<Vec<(String, String)>> = Vec::new();
    let mut inside = false;
    for pair in lines.chunks(2) {
        let [code, value] = pair else { break };
        if *code == "0" {
            inside = *value == kind;
            if inside {
                out.push(Vec::new());
            }
        } else if inside && let Some(rec) = out.last_mut() {
            rec.push((code.to_string(), value.to_string()));
        }
    }
    out
}

fn group<'a>(rec: &'a [(String, String)], code: &str) -> Option<&'a str> {
    rec.iter().find(|(c, _)| c == code).map(|(_, v)| v.as_str())
}

fn viewport_ids(l: &Layout) -> Vec<u32> {
    l.entities.iter().filter_map(|e| if let EntityKind::Viewport(v) = &e.kind { Some(v.id) } else { None }).collect()
}

/// True colours (entity and layer) and entity transparency: a new drawing's DWG is written as
/// AutoCAD 2018 (R2000 DWG has neither), while the DXF the bridge reads stays R2000.
#[test]
fn dwg_keeps_true_colours_and_transparency() {
    let mut d = Drawing::new_metric();
    d.layers.push(Layer { name: "Fancy".into(), color: Color::True(Rgb(12, 200, 99)), ..Layer::default() });
    let h = line(
        &mut d,
        &Space::Model,
        Common { layer: "Fancy".into(), color: Color::True(Rgb(1, 2, 3)), transparency: Transparency::Percent(40), ..Common::default() },
    );

    let dwg = cadaddy_io::write(&d, "x.dwg").unwrap();
    assert_eq!(cadaddy_dwg::version(&dwg).as_deref(), Some("AC1032"));
    let header = records(&cadaddy_io::write_dxf(&d), "SECTION");
    assert_eq!(header.first().and_then(|h| group(h, "1")), Some("AC1015"), "the DXF stays R2000");

    let back = cadaddy_io::read(&dwg, "x.dwg").unwrap();
    let e = back.entity(h).unwrap();
    assert_eq!(e.common.color, Color::True(Rgb(1, 2, 3)));
    assert_eq!(e.common.transparency, Transparency::Percent(40));
    assert_eq!(back.layers.iter().find(|l| l.name == "Fancy").map(|l| l.color), Some(Color::True(Rgb(12, 200, 99))));
}

/// Entities of a layout other than the first stay in that layout.
#[test]
fn dwg_keeps_paper_space_entities_in_their_layout() {
    let mut d = Drawing::new_metric();
    let second = Space::Paper(d.layouts[1].name.clone());
    let h = d.add(&second, Common::default(), EntityKind::Circle(Circle { center: Vec3::new(1.0, 1.0, 0.0), radius: 0.5 })).unwrap();
    let back = through_dwg(&d);
    assert!(back.layouts[1].entities.contains(h), "{:?}", back.layouts.iter().map(|l| l.entities.len()).collect::<Vec<_>>());
    assert!(!back.layouts[0].entities.contains(h));
}

/// Table styles (R2000 DWG has no TABLESTYLE objects).
#[test]
fn dwg_keeps_table_styles() {
    let mut d = Drawing::new_metric();
    let sched = TableStyle { name: "Sched".into(), text_height: 2.5, margin: 1.0, title: false, header: true, ..TableStyle::default() };
    d.table_styles.push(sched.clone());
    let back = through_dwg(&d);
    assert_eq!(back.table_styles.iter().find(|s| s.name == "Sched"), Some(&sched));
}

/// DWG doesn't store viewport ids; readers number a layout's viewports in order, and id 1 is the
/// paper-space viewport (the sheet, not drawn). The writer puts that viewport ahead of a layout's
/// own viewports, so a viewport made in CADaddy (id 2) keeps its id instead of becoming the
/// sheet; our reader drops the added viewport again, so a DXF round trip is unchanged.
#[test]
fn viewport_ids_survive_dwg_and_the_paper_viewport_stays_implicit() {
    let mut d = Drawing::new_metric();
    let first = Space::Paper(d.layouts[0].name.clone());
    let h = d.add(&first, Common::default(), viewport(2)).unwrap();
    d.add(&first, Common::default(), viewport(3)).unwrap();

    let text = cadaddy_io::write_dxf(&d);
    let vps = records(&text, "VIEWPORT");
    assert_eq!(vps.len(), 3, "{text}");
    assert_eq!(group(&vps[0], "69"), Some("1"));
    assert_eq!(group(&vps[0], "1000"), Some("PAPERVIEW"));
    // Viewports are written "on": status 68 positive, and 90 with the always-set bit 32768.
    assert_eq!(group(&vps[1], "68"), Some("2"));
    assert_eq!(group(&vps[1], "90"), Some("32768"));

    let back = cadaddy_io::read_dxf(text.as_bytes()).unwrap();
    assert_eq!(viewport_ids(&back.layouts[0]), [2, 3]);
    assert_eq!(back.layouts[0].entities.len(), 2);

    let back = through_dwg(&d);
    assert_eq!(viewport_ids(&back.layouts[0]), [2, 3]);
    assert!(back.layouts[0].entities.contains(h));

    // A layout that has its paper-space viewport (from another program) gets no second one.
    let mut d = Drawing::new_metric();
    d.add(&first, Common::default(), viewport(1)).unwrap();
    d.add(&first, Common::default(), viewport(2)).unwrap();
    let text = cadaddy_io::write_dxf(&d);
    assert!(!text.contains("PAPERVIEW"));
    assert_eq!(viewport_ids(&cadaddy_io::read_dxf(text.as_bytes()).unwrap().layouts[0]), [1, 2]);
}

/// The current layer and text style, and a linetype spelled the old way ("CONTINUOUS"), used to
/// come back from DWG as layer 0, Standard and "Continuous" (real AutoCAD sample drawings).
#[test]
fn dwg_keeps_current_layer_style_and_linetype_spelling() {
    let mut d = Drawing::default();
    for lt in &mut d.linetypes {
        if lt.name == "Continuous" {
            lt.name = "CONTINUOUS".into();
        }
    }
    for l in &mut d.layers {
        l.linetype = "CONTINUOUS".into();
    }
    let mut lot = d.layers.first().cloned().unwrap();
    lot.name = "LOT".into();
    d.layers.push(lot);
    let mut style = d.text_styles.first().cloned().unwrap();
    style.name = "NOTES".into();
    d.text_styles.push(style);
    d.header.set("CLAYER", HVal::Str("LOT".into()));
    d.header.set("TEXTSTYLE", HVal::Str("NOTES".into()));
    line(&mut d, &Space::Model, Common { layer: "LOT".into(), ..Common::default() });
    let back = through_dwg(&d);
    assert_eq!(back.header.str("CLAYER", ""), "LOT");
    assert_eq!(back.header.str("TEXTSTYLE", ""), "NOTES");
    assert!(back.linetypes.iter().any(|l| l.name == "CONTINUOUS"), "{:?}", back.linetypes.iter().map(|l| &l.name).collect::<Vec<_>>());
    assert_eq!(back.layer("LOT").map(|l| l.linetype.as_str()), Some("CONTINUOUS"));
}

#[test]
fn a_viewport_view_target_offsets_its_view_centre() {
    // AutoCAD keeps a panned viewport's view centre relative to its view target (17/27); the
    // model point shown is target + centre (tablet_overlay.dwg looked at a corner otherwise).
    let mut d = Drawing::new_metric();
    let first = Space::Paper(d.layouts[0].name.clone());
    d.add(&first, Common::default(), viewport(2)).unwrap();
    let text = cadaddy_io::write_dxf(&d);
    // After the last viewport's subclass marker line (the file may use CR LF).
    let m = text.rfind("AcDbViewport").expect("viewport");
    let at = m + text[m..].find('\n').expect("line end") + 1;
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let patched = format!("{} 17{nl}4.5{nl} 27{nl}-4.6{nl} 37{nl}0.0{nl}{}", &text[..at], &text[at..]);
    let back = cadaddy_io::read_dxf(patched.as_bytes()).unwrap();
    let vc = back.layouts[0]
        .entities
        .iter()
        .find_map(|e| if let EntityKind::Viewport(v) = &e.kind { (v.id == 2).then_some(v.view_center) } else { None })
        .expect("viewport 2");
    assert!((vc - Vec2::new(14.5, 5.4)).len() < 1e-9, "{vc:?}");
    // Saved again: the centre is written whole with no target, so it reads back the same.
    let again = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&back).as_bytes()).unwrap();
    let vc2 = again.layouts[0]
        .entities
        .iter()
        .find_map(|e| if let EntityKind::Viewport(v) = &e.kind { (v.id == 2).then_some(v.view_center) } else { None });
    assert_eq!(vc2, Some(vc));
}

#[test]
fn viewport_frozen_layers_use_group_331() {
    // The DXF Reference keeps a viewport's frozen layers in group 331; other programs read only
    // that (CADaddy wrote 341 before 0.7.4, which nothing else understood).
    let mut d = Drawing::new_metric();
    d.ensure_layer("HIDE");
    let first = Space::Paper(d.layouts[0].name.clone());
    let mut vp = viewport(2);
    if let EntityKind::Viewport(v) = &mut vp {
        v.frozen_layers = vec!["HIDE".into()];
    }
    d.add(&first, Common::default(), vp).unwrap();
    let text = cadaddy_io::write_dxf(&d);
    let vps = records(&text, "VIEWPORT");
    let ours = vps.iter().find(|r| group(r, "69") == Some("2")).expect("viewport 2");
    assert!(group(ours, "331").is_some(), "{ours:?}");
    assert!(group(ours, "341").is_none(), "{ours:?}");
    // Without CADaddy's own xdata (a file from another program), 331 alone freezes the layer.
    // Drop CADaddy's own xdata (1001 CADADDY up to the next app or record) from every record.
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut skipping = false;
    for pair in lines.chunks(2) {
        let [code, value] = pair else { break };
        if *code == "0" || *code == "1001" {
            skipping = *code == "1001" && *value == "CADADDY";
        }
        if !skipping {
            kept.extend([*code, *value]);
        }
    }
    let plain = kept.join("\n");
    let back = cadaddy_io::read_dxf(plain.as_bytes()).unwrap();
    let frozen: Vec<String> = back.layouts[0]
        .entities
        .iter()
        .find_map(|e| if let EntityKind::Viewport(v) = &e.kind { (v.id == 2).then(|| v.frozen_layers.clone()) } else { None })
        .unwrap();
    assert_eq!(frozen, ["HIDE"]);
}

#[test]
fn the_current_dimension_style_survives_dwg_without_header_overrides() {
    // A current architectural style and no DIM* header values (none overridden): the DWG must not
    // reopen with the converter's defaults (decimal units, arrows) overriding the style.
    let mut d = Drawing::new_imperial();
    let mut a = d.dim_style("Standard").cloned().unwrap();
    a.name = "A".into();
    a.linear_unit = 4;
    a.zero_suppression = 3;
    a.tick_size = 0.1;
    d.dim_styles.push(a);
    d.header.set_str("DIMSTYLE", "A");
    for (var, _) in cadaddy_doc::DIMVARS {
        d.header.vars.remove(*var);
    }
    let back = through_dwg(&d);
    assert_eq!(back.header.str("DIMSTYLE", ""), "A");
    assert!(back.dim_overrides().is_empty(), "overrides after DWG: {:?}", back.dim_overrides());
    let dxf = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&d).as_bytes()).unwrap();
    assert!(dxf.dim_overrides().is_empty(), "overrides after DXF: {:?}", dxf.dim_overrides());
}

#[test]
fn mtext_background_masks_survive_dxf_and_dwg() {
    // A note's background mask (MTEXT 90/63/45/441) used to be dropped on open and on save.
    let mut d = Drawing::new_imperial();
    let mt = |bg: Option<MTextBackground>, y: f64| {
        EntityKind::MText(MText {
            background: bg,
            insert: Vec3::new(0.0, y, 0.0),
            height: 2.5,
            width: 40.0,
            attach: 1,
            rotation: 0.0,
            style: "Standard".into(),
            contents: "NOTE OVER HATCH".into(),
            line_spacing: 1.0,
            line_spacing_exact: false,
            columns: None,
        })
    };
    let coloured = Some(MTextBackground { color: Some(Color::Index(1)), scale: 1.25, transparency: 0 });
    let window = Some(MTextBackground { color: None, scale: 1.5, transparency: 0 });
    let a = d.add(&Space::Model, Common::default(), mt(coloured.clone(), 0.0)).unwrap();
    let b = d.add(&Space::Model, Common::default(), mt(window.clone(), 10.0)).unwrap();
    let c = d.add(&Space::Model, Common::default(), mt(None, 20.0)).unwrap();
    let bg = |d: &Drawing, h: Handle| match &d.entity(h).unwrap().kind {
        EntityKind::MText(m) => m.background.clone(),
        k => panic!("{k:?}"),
    };
    let dxf = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&d).as_bytes()).unwrap();
    assert_eq!((bg(&dxf, a), bg(&dxf, b), bg(&dxf, c)), (coloured.clone(), window.clone(), None));
    let dwg = through_dwg(&d);
    let find = |d: &Drawing, y: f64| {
        d.model
            .iter()
            .find_map(|e| if let EntityKind::MText(m) = &e.kind { ((m.insert.y - y).abs() < 1e-9).then(|| m.background.clone()) } else { None })
            .unwrap()
    };
    assert_eq!(find(&dwg, 0.0).map(|b| (b.color, (b.scale * 100.0).round())), Some((Some(Color::Index(1)), 125.0)));
    assert_eq!(find(&dwg, 10.0).map(|b| b.color), Some(None));
    assert_eq!(find(&dwg, 20.0), None);
}
