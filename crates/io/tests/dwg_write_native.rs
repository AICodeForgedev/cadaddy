//! A drawing written as DWG with CADaddy's own codec (Q201) reads back — through the DWG bridge
//! (acadrust, black box) and through the codec — as the same drawing the bridge's own DWG makes,
//! field by field, for every version.

use cadaddy_doc::*;
use cadaddy_geom::{Vec2, Vec3};
use serde_json::Value as J;

const VERSIONS: [&str; 6] = ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"];

/// A made-up drawing with an example of every entity the codec decodes.
fn entities_drawing() -> Drawing {
    let mut e = String::new();
    let mut add = |s: &str| e.push_str(s);
    add("0\nLINE\n8\nWALLS\n62\n3\n10\n1\n20\n2\n30\n0\n11\n9\n21\n7\n31\n0\n");
    add("0\nLINE\n8\n0\n6\nDASHED\n370\n35\n48\n2\n10\n1\n20\n2\n30\n3\n11\n4\n21\n5\n31\n6\n39\n2.5\n");
    add("0\nCIRCLE\n8\n0\n420\n16711680\n10\n5\n20\n5\n30\n0\n40\n2.5\n");
    add("0\nARC\n8\n0\n10\n15\n20\n5\n30\n0\n40\n3\n50\n30\n51\n200\n");
    add("0\nPOINT\n8\n0\n10\n7\n20\n8\n30\n0\n");
    add("0\nELLIPSE\n8\n0\n10\n20\n20\n20\n30\n0\n11\n6\n21\n2\n31\n0\n40\n0.4\n41\n0\n42\n3.14159\n");
    add("0\nTEXT\n8\n0\n10\n1\n20\n30\n30\n0\n40\n2\n1\nRoom 101\n50\n15\n");
    add("0\nTEXT\n8\n0\n10\n1\n20\n35\n30\n0\n40\n2\n1\nCentred\n72\n1\n11\n10\n21\n35\n31\n0\n");
    add("0\nMTEXT\n8\n0\n10\n30\n20\n30\n30\n0\n40\n2.5\n41\n20\n71\n1\n72\n1\n1\nTwo\\Plines of text\n");
    add("0\nLWPOLYLINE\n8\nWALLS\n90\n4\n70\n1\n10\n0\n20\n50\n10\n10\n20\n50\n42\n0.5\n10\n10\n20\n60\n10\n0\n20\n60\n");
    add("0\nLWPOLYLINE\n8\n0\n90\n2\n70\n0\n43\n0.5\n10\n0\n20\n70\n10\n20\n20\n70\n");
    add("0\nSOLID\n8\n0\n10\n40\n20\n0\n30\n0\n11\n45\n21\n0\n31\n0\n12\n40\n22\n5\n32\n0\n13\n45\n23\n5\n33\n0\n");
    add("0\n3DFACE\n8\n0\n10\n50\n20\n0\n30\n0\n11\n55\n21\n0\n31\n1\n12\n55\n22\n5\n32\n2\n13\n50\n23\n5\n33\n3\n");
    add("0\nRAY\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n0\n31\n0\n");
    add("0\nXLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n0\n21\n1\n31\n0\n");
    add(
        "0\nSPLINE\n8\n0\n70\n8\n71\n3\n72\n8\n73\n4\n74\n0\n40\n0\n40\n0\n40\n0\n40\n0\n40\n1\n40\n1\n40\n1\n40\n1\n10\n0\n20\n80\n30\n0\n10\n5\n20\n90\n30\n0\n10\n10\n20\n80\n30\n0\n10\n15\n20\n90\n30\n0\n",
    );
    add(
        "0\nPOLYLINE\n8\n0\n66\n1\n10\n0\n20\n0\n30\n0\n70\n8\n0\nVERTEX\n8\n0\n10\n80\n20\n0\n30\n1\n70\n32\n0\nVERTEX\n8\n0\n10\n90\n20\n0\n30\n2\n70\n32\n0\nSEQEND\n8\n0\n",
    );
    add(
        "0\nINSERT\n8\n0\n66\n1\n2\nTAG\n10\n100\n20\n0\n30\n0\n41\n2\n42\n2\n43\n2\n50\n45\n0\nATTRIB\n8\n0\n10\n100\n20\n0\n30\n0\n40\n1\n1\nA-12\n2\nNUMBER\n70\n0\n0\nSEQEND\n8\n0\n",
    );
    add("0\nINSERT\n8\n0\n2\nTAG\n10\n110\n20\n0\n30\n0\n");
    add(
        "0\nHATCH\n8\n0\n10\n0\n20\n0\n30\n0\n210\n0\n220\n0\n230\n1\n2\nSOLID\n70\n1\n71\n0\n91\n1\n92\n2\n72\n0\n73\n1\n93\n4\n10\n0\n20\n100\n10\n10\n20\n100\n10\n10\n20\n110\n10\n0\n20\n110\n97\n0\n75\n0\n76\n1\n98\n0\n",
    );
    add("0\nLEADER\n8\n0\n3\nStandard\n71\n1\n72\n0\n73\n3\n76\n3\n10\n0\n20\n140\n30\n0\n10\n5\n20\n145\n30\n0\n10\n10\n20\n145\n30\n0\n");
    let dxf = format!(
        "0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLTYPE\n0\nLTYPE\n2\nDASHED\n70\n0\n3\n__ __\n72\n65\n73\n2\n40\n0.75\n49\n0.5\n49\n-0.25\n0\nENDTAB\n\
         0\nTABLE\n2\nLAYER\n0\nLAYER\n2\n0\n70\n0\n62\n7\n6\nCONTINUOUS\n0\nLAYER\n2\nWALLS\n70\n0\n62\n1\n6\nCONTINUOUS\n0\nENDTAB\n0\nENDSEC\n\
         0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n8\n0\n2\nTAG\n70\n2\n10\n0\n20\n0\n30\n0\n3\nTAG\n\
         0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n1\n\
         0\nATTDEF\n8\n0\n10\n0\n20\n0\n30\n0\n40\n1\n1\n?\n3\nNumber?\n2\nNUMBER\n70\n0\n0\nENDBLK\n8\n0\n0\nENDSEC\n\
         0\nSECTION\n2\nENTITIES\n{e}0\nENDSEC\n0\nEOF\n"
    );
    cadaddy_io::read(dxf.as_bytes(), "made-up.dxf").unwrap()
}

fn objects_drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    let mut walls = Layer::new("WALLS");
    walls.color = cadaddy_color::Color::Index(1);
    walls.lineweight = Lineweight::Mm100(50);
    let mut hidden = Layer::new("HIDDEN");
    hidden.on = false;
    hidden.plot = false;
    let mut frozen = Layer::new("FROZEN");
    frozen.frozen = true;
    frozen.locked = true;
    frozen.color = cadaddy_color::Color::True(cadaddy_color::Rgb(255, 128, 0));
    d.layers.extend([walls.clone(), hidden.clone(), frozen.clone()]);
    d.layer_states.push(LayerState::new("ALL ON", vec![walls.clone(), Layer { on: true, ..hidden.clone() }, frozen.clone()]));
    let mut st = LayerState::new("PLOT", vec![walls, hidden]);
    st.description = "for plotting".into();
    d.layer_states.push(st);
    d.ucss.push(Ucs { name: "TILTED".into(), origin: Vec3::new(5.0, 6.0, 0.0), x_axis: Vec3::new(0.6, 0.8, 0.0), y_axis: Vec3::new(-0.8, 0.6, 0.0) });
    let line = d
        .add(
            &Space::Model,
            Common { layer: "WALLS".into(), ..Common::default() },
            EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 0.0, 0.0) }),
        )
        .unwrap();
    let circle = d.add(&Space::Model, Common::default(), EntityKind::Circle(Circle { center: Vec3::new(5.0, 5.0, 0.0), radius: 2.0 })).unwrap();
    d.groups.push(Group { name: "PAIR".into(), description: "a line and a circle".into(), selectable: true, members: vec![line, circle] });
    // A second layout with a viewport of its own.
    let mut l2 = Layout::new("Sheet 2", 2);
    l2.page.paper = "ISO A3".into();
    d.layouts.push(l2);
    let vp = Viewport {
        center: Vec3::new(150.0, 100.0, 0.0),
        width: 200.0,
        height: 120.0,
        view_center: Vec2::new(5.0, 5.0),
        view_height: 30.0,
        ..Viewport::default()
    };
    d.add(&Space::Paper("Sheet 2".into()), Common::default(), EntityKind::Viewport(vp)).unwrap();
    d
}

/// Two JSON values equal but for float rounding; the path of the first difference.
fn diff(a: &J, b: &J, path: &str) -> Option<String> {
    match (a, b) {
        (J::Number(x), J::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            ((x - y).abs() > 1e-9 * x.abs().max(1.0)).then(|| format!("{path}: {x} vs {y}"))
        }
        (J::Array(x), J::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: {} items vs {}", x.len(), y.len()));
            }
            x.iter().zip(y).enumerate().find_map(|(i, (p, q))| diff(p, q, &format!("{path}[{i}]")))
        }
        (J::Object(x), J::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            keys.into_iter().find_map(|k| diff(x.get(k).unwrap_or(&J::Null), y.get(k).unwrap_or(&J::Null), &format!("{path}.{k}")))
        }
        _ => (a != b).then(|| format!("{path}: {a} vs {b}")),
    }
}

fn entities(d: &Drawing) -> Vec<(String, &Entity)> {
    let mut v: Vec<(String, &Entity)> = d.model.iter().map(|e| ("model".to_string(), e.as_ref())).collect();
    for (name, b) in &d.blocks {
        v.extend(b.entities.iter().map(|e| (format!("block {name}"), e.as_ref())));
    }
    v
}

/// The drawing's tables and objects as JSON, part by part.
fn parts(d: &Drawing) -> J {
    serde_json::json!({
        "layers": d.layers,
        "linetypes": d.linetypes,
        "text_styles": d.text_styles,
        "dim_styles": d.dim_styles,
        "layer_states": d.layer_states,
        "layouts": d.layouts,
        "groups": d.groups,
        "views": d.views,
        "ucss": d.ucss,
        "mline_styles": d.mline_styles,
        "mleader_styles": d.mleader_styles,
        "table_styles": d.table_styles,
        "blocks": d.blocks.keys().collect::<Vec<_>>(),
    })
}

/// The bridge's reading of `d` saved through the bridge, and of `d` saved natively.
fn both(d: &Drawing, v: &str) -> (Drawing, Drawing, Vec<u8>) {
    let mut d = d.clone();
    d.header.set_str("ACADVER", v);
    let opts = cadaddy_io::SaveOptions { version: cadaddy_io::Version::for_acadver(v), ..Default::default() };
    let theirs = cadaddy_io::write_with(&d, "made-up.dwg", &opts).unwrap();
    let (ours, report) = cadaddy_io::dwg_write::write(&d, v).unwrap();
    eprintln!("{v}: {} bytes (bridge {}), left out {:?}", ours.len(), theirs.len(), report.skipped);
    let (_, notes) = cadaddy_dwg::dwg_to_dxf_noted(&ours, cadaddy_dwg::Limits::DEFAULT).unwrap();
    // The bridge can't read an R2000 VIEWPORT linked to its neighbours (its own files store none
    // of the links the spec has there); those layouts are checked through our reader.
    let notes: Vec<&String> = notes.iter().filter(|n| !(v == "AC1015" && n.contains("(VIEWPORT × "))).collect();
    assert!(notes.is_empty(), "{v}: {notes:?}");
    (cadaddy_io::read(&theirs, "made-up.dwg").unwrap(), cadaddy_io::read(&ours, "made-up.dwg").unwrap(), ours)
}

fn compare_entities(v: &str, a: &Drawing, b: &Drawing, report: &mut Vec<String>) {
    let (ea, eb) = (entities(a), entities(b));
    if ea.len() != eb.len() {
        report.push(format!("{v}: {} entities vs {}", ea.len(), eb.len()));
        return;
    }
    for ((wa, x), (wb, y)) in ea.iter().zip(&eb) {
        let (jx, jy) = (serde_json::to_value(x).unwrap(), serde_json::to_value(y).unwrap());
        if wa != wb {
            report.push(format!("{v}: {wa} vs {wb}"));
        } else if let Some(d) = diff(&jx, &jy, "") {
            report.push(format!("{v} {wa} {:?}: {d}", x.handle));
        }
    }
}

#[test]
fn entities_written_natively_read_back_as_through_the_bridge() {
    let mut report = Vec::new();
    for v in VERSIONS {
        let (a, b, bytes) = both(&entities_drawing(), v);
        compare_entities(v, &a, &b, &mut report);
        // Our own reader agrees.
        let (native, rep) = cadaddy_io::dwg_native::read_with_native_entities(&bytes).unwrap();
        assert!(rep.failed.is_empty(), "{v}: {:?}", rep.failed);
        compare_entities(&format!("{v} native"), &a, &native, &mut report);
    }
    assert!(report.is_empty(), "{} differences:\n{}", report.len(), report.join("\n"));
}

#[test]
fn tables_and_objects_written_natively_read_back_as_through_the_bridge() {
    let mut report = Vec::new();
    for v in VERSIONS {
        let (a, b, bytes) = both(&objects_drawing(), v);
        let (pa, pb) = (parts(&a), parts(&b));
        for (k, x) in pa.as_object().into_iter().flatten() {
            if let Some(d) = diff(x, &pb[k], k) {
                report.push(format!("{v}: {d}"));
            }
        }
        // The header, variable by variable.
        let (ha, hb) = (serde_json::to_value(&a.header).unwrap()["vars"].clone(), serde_json::to_value(&b.header).unwrap()["vars"].clone());
        // Those the source drawing has (the rest are each writer's defaults for a new drawing).
        let dxf = cadaddy_io::write_dxf(&objects_drawing());
        let lines: Vec<&str> = dxf.lines().map(str::trim).collect();
        let source: std::collections::HashSet<&str> = lines.windows(2).filter(|w| w[0] == "9").filter_map(|w| w[1].strip_prefix('$')).collect();
        // HANDSEED: each writer's own next handle (ours counts the records it keeps).
        for (k, x) in ha.as_object().into_iter().flatten().filter(|(k, _)| source.contains(k.as_str()) && *k != "HANDSEED") {
            if let Some(d) = diff(x, hb.get(k).unwrap_or(&J::Null), &format!("header {k}")) {
                report.push(format!("{v}: {d}"));
            }
        }
        if v == "AC1015" {
            // Sheet 2's two viewports, as the codec reads them, are both in its paper space.
            let dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
            let (ents, failed) = cadaddy_dwgcodec::entities::decode_all(&dwg).unwrap();
            assert!(failed.is_empty(), "{failed:?}");
            let vps: Vec<_> = ents.iter().filter(|e| e.name == "VIEWPORT").collect();
            assert_eq!(vps.len(), 2, "{v}");
            assert!(vps.iter().all(|e| e.owner.is_some() && e.owner == vps[0].owner), "{v}: {:?}", vps.iter().map(|e| e.owner).collect::<Vec<_>>());
        }
        let b_layouts = if v == "AC1015" { &a.layouts } else { &b.layouts };
        for (la, lb) in a.layouts.iter().zip(b_layouts) {
            let ea: Vec<J> = la.entities.iter().map(|e| serde_json::to_value(e).unwrap()).collect();
            let eb: Vec<J> = lb.entities.iter().map(|e| serde_json::to_value(e).unwrap()).collect();
            if let Some(d) = diff(&J::Array(ea), &J::Array(eb), &format!("layout {}", la.name)) {
                report.push(format!("{v}: {d}"));
            }
        }
        compare_entities(v, &a, &b, &mut report);
    }
    assert!(report.is_empty(), "{} differences:\n{}", report.len(), report.join("\n"));
}

fn mleader_drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    d.text_styles.push(TextStyle { name: "Notes".into(), ..TextStyle::default() });
    d.mleader_styles.push(MLeaderStyle { name: "Callout".into(), arrow_size: 0.3, ..MLeaderStyle::default() });
    let m = MLeader {
        leaders: vec![vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 3.0, 0.0)], vec![Vec3::new(1.0, -2.0, 0.0)]],
        landing: Vec3::new(5.0, 4.0, 0.0),
        dogleg: 0.75,
        text: Some(MText {
            background: None,
            insert: Vec3::new(5.9, 4.0, 0.0),
            height: 0.4,
            width: 6.0,
            attach: 4,
            rotation: 0.0,
            style: "Notes".into(),
            contents: "FIRST\\PSECOND".into(),
            line_spacing: 1.25,
            line_spacing_exact: true,
            columns: None,
        }),
        style: "Callout".into(),
        arrow_size: 0.3,
        spline: false,
        block: None,
        roots: Vec::new(),
        leader_color: None,
        leader_linetype: None,
        arrow: None,
        text_frame: false,
    };
    d.add(&Space::Model, Common { layer: "0".into(), ..Common::default() }, EntityKind::MLeader(Box::new(m))).unwrap();
    // Block content with an attribute.
    let mut blk = Block::new("BUBBLE");
    blk.entities.push(Entity::new(Handle(0x200), EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 1.0 })));
    let tag = Text {
        insert: Vec3::new(-0.2, -0.1, 0.0),
        align_pt: None,
        height: 0.25,
        value: "?".into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::default(),
        valign: VAlign::default(),
    };
    let def = Attrib {
        tag: "NUM".into(),
        text: tag.clone(),
        invisible: false,
        constant: false,
        prompt: "Number".into(),
        props: Default::default(),
        multiline: None,
        modes: Default::default(),
    };
    blk.entities.push(Entity::new(Handle(0x201), EntityKind::AttDef(Box::new(def))));
    d.blocks.insert(blk.name.clone(), std::sync::Arc::new(blk));
    let mut value = tag;
    value.insert = Vec3::new(12.0, 3.0, 0.0);
    value.value = "42".into();
    let ins = Insert {
        block: "BUBBLE".into(),
        insert: Vec3::new(12.0, 3.0, 0.0),
        scale: Vec3::new(2.0, 2.0, 1.0),
        rotation: 0.5,
        attribs: vec![Attrib {
            tag: "NUM".into(),
            text: value,
            invisible: false,
            constant: false,
            prompt: String::new(),
            props: Default::default(),
            multiline: None,
            modes: Default::default(),
        }],
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    let m = MLeader {
        leaders: vec![vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 5.0, 0.0), Vec3::new(8.0, 4.0, 0.0)]],
        landing: Vec3::new(10.0, 3.0, 0.0),
        dogleg: 1.0,
        text: None,
        style: "Standard".into(),
        arrow_size: 0.18,
        spline: true,
        block: Some(ins),
        roots: Vec::new(),
        leader_color: None,
        leader_linetype: None,
        arrow: None,
        text_frame: false,
    };
    d.add(&Space::Model, Common { layer: "0".into(), ..Common::default() }, EntityKind::MLeader(Box::new(m))).unwrap();
    d
}

#[test]
fn multileaders_written_natively_read_back_as_through_the_bridge() {
    let mut report = Vec::new();
    // 2004 has no multileaders (the drawing saves LEADER and MTEXT there).
    for v in VERSIONS {
        let (a, mut b, bytes) = both(&mleader_drawing(), v);
        // The bridge's R2000 files keep only the Standard multileader style: the reference is
        // the source drawing there.
        let reference = if v == "AC1015" { cadaddy_io::read_dxf(cadaddy_io::write_dxf(&mleader_drawing()).as_bytes()).unwrap() } else { a.clone() };
        if let Some(d) = diff(&parts(&reference)["mleader_styles"], &parts(&b)["mleader_styles"], "mleader_styles") {
            report.push(format!("{v}: {d}"));
        }
        // The bridge reads a multileader's ByBlock linetype (no override) as ByLayer (observed:
        // its own files store a handle that names no linetype); our reader is checked below.
        for h in b.model.handles() {
            b.model.modify(h, |e| {
                if let EntityKind::MLeader(m) = &mut e.kind
                    && m.leader_linetype.as_deref() == Some("ByLayer")
                {
                    m.leader_linetype = None;
                }
            });
        }
        compare_entities(v, &a, &b, &mut report);
        let (native, rep) = cadaddy_io::dwg_native::read_with_native_entities(&bytes).unwrap();
        assert!(rep.failed.is_empty(), "{v}: {:?}", rep.failed);
        compare_entities(&format!("{v} native"), &a, &native, &mut report);
        if !matches!(v, "AC1015" | "AC1018") {
            assert_eq!(b.model.iter().filter(|e| matches!(e.kind, EntityKind::MLeader(_))).count(), 2, "{v}");
        }
    }
    assert!(report.is_empty(), "{} differences:\n{}", report.len(), report.join("\n"));
}

fn misc_drawing() -> Drawing {
    use cadaddy_color::Color;
    let mut d = Drawing::new_imperial();
    let wall = MLineStyle {
        name: "Wall".into(),
        description: "outer wall".into(),
        elements: vec![
            MLineElement { offset: 1.5, color: Color::Index(1), linetype: "ByLayer".into() },
            MLineElement { offset: 0.0, color: Color::ByBlock, linetype: "Continuous".into() },
            MLineElement { offset: -1.5, color: Color::ByLayer, linetype: "ByLayer".into() },
        ],
        start_caps: MLineCaps { line: true, outer_arc: false, inner_arcs: true, angle: 60f64.to_radians() },
        end_caps: MLineCaps { line: false, outer_arc: true, inner_arcs: false, angle: std::f64::consts::FRAC_PI_2 },
        joints: true,
        fill: Some(Color::Index(3)),
    };
    d.mline_styles.push(wall.clone());
    let pts = [Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0), Vec2::new(20.0, 10.0)];
    let mut open = MLine::new(&wall, &pts, false, MLineJust::Zero, 2.0);
    open.vertices[0].cuts.push(MLineCut { element: 0, start: 4.0, end: 6.5 });
    d.add(&Space::Model, Common::default(), EntityKind::MLine(open)).unwrap();
    let closed =
        MLine::new(&MLineStyle::default(), &[Vec2::new(0.0, 30.0), Vec2::new(10.0, 30.0), Vec2::new(5.0, 40.0)], true, MLineJust::Bottom, -1.0);
    d.add(&Space::Model, Common::default(), EntityKind::MLine(closed)).unwrap();
    let img = Image {
        insert: Vec3::new(1.0, 1.0, 0.0),
        u: Vec3::new(2.0, 0.0, 0.0),
        v: Vec3::new(0.0, 2.0, 0.0),
        size: Vec2::new(2.0, 1.0),
        path: "pictures/site plan.png".into(),
        ..Image::default()
    };
    d.add(&Space::Model, Common::default(), EntityKind::Image(img)).unwrap();
    let wipe = Wipeout { boundary: vec![Vec2::new(50.0, 0.0), Vec2::new(60.0, 0.0), Vec2::new(60.0, 5.0), Vec2::new(50.0, 5.0)] };
    d.add(&Space::Model, Common::default(), EntityKind::Wipeout(wipe)).unwrap();
    d
}

#[test]
fn multilines_images_and_wipeouts_written_natively_read_back_as_through_the_bridge() {
    let mut report = Vec::new();
    for v in VERSIONS {
        let (mut a, mut b, bytes) = both(&misc_drawing(), v);
        // The bridge stores no multiline style flags (caps, joints, fill: its own files have 0),
        // so for those the reference is the source drawing (through DXF), and the bridge's
        // reading of ours is no evidence; our reader checks them below.
        let src = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&misc_drawing()).as_bytes()).unwrap();
        let same_flags = |to: &mut MLineStyle, from: &MLineStyle| {
            to.start_caps = from.start_caps;
            to.end_caps = from.end_caps;
            to.joints = from.joints;
            to.fill = from.fill;
        };
        for d in [&mut a, &mut b] {
            for (s, t) in d.mline_styles.iter_mut().zip(&src.mline_styles) {
                same_flags(s, t);
            }
            for (h, e) in d.model.handles().into_iter().zip(src.model.iter()) {
                if let EntityKind::MLine(from) = &e.kind {
                    d.model.modify(h, |x| {
                        if let EntityKind::MLine(m) = &mut x.kind {
                            same_flags(&mut m.def, &from.def);
                        }
                    });
                }
            }
        }
        if let Some(d) = diff(&parts(&a)["mline_styles"], &parts(&b)["mline_styles"], "mline_styles") {
            report.push(format!("{v}: {d}"));
        }
        compare_entities(v, &a, &b, &mut report);
        let (native, rep) = cadaddy_io::dwg_native::read_with_native_entities(&bytes).unwrap();
        assert!(rep.failed.is_empty(), "{v}: {:?}", rep.failed);
        compare_entities(&format!("{v} native"), &a, &native, &mut report);
    }
    assert!(report.is_empty(), "{} differences:\n{}", report.len(), report.join("\n"));
}

/// Every reference in the file points at an object in it: dictionary entries, owners and
/// reactors (records the writer leaves out take their references with them).
#[test]
fn references_point_at_written_objects() {
    use cadaddy_dwgcodec::entities::Value as V;
    for d in [entities_drawing(), objects_drawing(), mleader_drawing(), misc_drawing()] {
        for v in VERSIONS {
            let (bytes, _) = cadaddy_io::dwg_write::write(&d, v).unwrap();
            let dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
            let handles: std::collections::HashSet<u64> = dwg.objects.iter().map(|o| o.handle).collect();
            let (recs, failed) = cadaddy_dwgcodec::entities::records::decode_all(&dwg).unwrap();
            assert!(failed.is_empty(), "{v}: {failed:?}");
            for r in &recs {
                for g in &r.groups {
                    if let (330 | 350 | 360, V::Handle(h)) = (g.code, &g.value)
                        && *h != 0
                    {
                        assert!(handles.contains(h), "{v}: {} {:X} refers to {h:X} ({})", r.name, r.handle, g.code);
                    }
                }
            }
        }
    }
}

/// Every handle in the file is used once.
#[test]
fn handles_are_unique() {
    for v in VERSIONS {
        let (bytes, _) = cadaddy_io::dwg_write::write(&objects_drawing(), v).unwrap();
        let dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
        let mut seen = std::collections::HashSet::new();
        for o in &dwg.objects {
            assert!(seen.insert(o.handle), "{v}: handle {:X} twice", o.handle);
        }
    }
}

/// An empty drawing (each version's defaults alone) reads back through the bridge.
#[test]
fn an_empty_drawing_reads_back() {
    for v in VERSIONS {
        let (bytes, report) = cadaddy_io::dwg_write::write(&Drawing::new_metric(), v).unwrap();
        let (_, notes) = cadaddy_dwg::dwg_to_dxf_noted(&bytes, cadaddy_dwg::Limits::DEFAULT).unwrap();
        assert!(notes.is_empty(), "{v}: {notes:?}");
        let d = cadaddy_io::read(&bytes, "empty.dwg").unwrap();
        assert_eq!(d.entity_count(), 0, "{v}");
        assert!(d.layers.iter().any(|l| l.name == "0"), "{v}");
        assert!(report.skipped.iter().all(|(n, _)| matches!(n.as_str(), "MATERIAL" | "TABLESTYLE")), "{v}: {:?}", report.skipped);
    }
}

/// Versions without a writer say so.
#[test]
fn other_versions_are_refused() {
    for v in ["AC1009", "AC1014", "AC9999"] {
        assert!(cadaddy_io::dwg_write::write(&Drawing::new_metric(), v).is_err(), "{v}");
    }
}

/// Q203: a file read with the codec alone (no bridge) makes the drawing the bridge makes of it:
/// entities, tables and objects, layouts with their entities, and the source's header variables;
/// for the bridge's files and ours.
#[test]
fn native_reading_matches_the_bridge() {
    let mut report = Vec::new();
    let dxf_vars = |d: &Drawing| -> std::collections::HashSet<String> {
        let dxf = cadaddy_io::write_dxf(d);
        let lines: Vec<&str> = dxf.lines().map(str::trim).collect();
        lines.windows(2).filter(|w| w[0] == "9").filter_map(|w| w[1].strip_prefix('$')).map(str::to_string).collect()
    };
    for (what, d) in [("entities", entities_drawing()), ("objects", objects_drawing()), ("mleaders", mleader_drawing()), ("misc", misc_drawing())] {
        let source = dxf_vars(&d);
        for v in VERSIONS {
            let mut d = d.clone();
            d.header.set_str("ACADVER", v);
            let opts = cadaddy_io::SaveOptions { version: cadaddy_io::Version::for_acadver(v), ..Default::default() };
            let theirs = cadaddy_io::write_with(&d, "made-up.dwg", &opts).unwrap();
            let (ours, _) = cadaddy_io::dwg_write::write(&d, v).unwrap();
            for (file, bytes) in [("bridge's", &theirs), ("ours", &ours)] {
                let tag = format!("{what} {v} {file}");
                let Ok(mut a) = cadaddy_io::read(bytes, "x.dwg") else { continue };
                let (b, rep) = cadaddy_io::dwg_read::read(bytes).unwrap_or_else(|e| panic!("{tag}: {e}"));
                // The bridge's readings of our files that aren't evidence (see the tests above):
                // multiline style flags, a ByBlock leader linetype, R2000 viewports with links.
                // The reference there is the source drawing's.
                if file == "ours" {
                    let src = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&d).as_bytes()).unwrap();
                    for (x, y) in a.mline_styles.iter_mut().zip(&src.mline_styles) {
                        (x.start_caps, x.end_caps, x.joints, x.fill) = (y.start_caps, y.end_caps, y.joints, y.fill);
                    }
                    for h in a.model.handles() {
                        let from = src.model.get(h).cloned();
                        a.model.modify(h, |e| match (&mut e.kind, from.as_ref().map(|f| &f.kind)) {
                            (EntityKind::MLine(m), Some(EntityKind::MLine(f))) => {
                                (m.def.start_caps, m.def.end_caps, m.def.joints, m.def.fill) =
                                    (f.def.start_caps, f.def.end_caps, f.def.joints, f.def.fill);
                            }
                            (EntityKind::MLeader(m), _) if m.leader_linetype.as_deref() == Some("ByLayer") => m.leader_linetype = None,
                            _ => {}
                        });
                    }
                    if v == "AC1015" {
                        a.layouts = b.layouts.clone();
                    }
                }
                if !rep.failed.is_empty() {
                    report.push(format!("{tag}: undecoded {:?}", rep.failed));
                }
                let (mut pa, mut pb) = (parts(&a), parts(&b));
                // DIMFXL isn't stored before R2007 (the bridge makes it 0, DXF's default is 1).
                if matches!(v, "AC1015" | "AC1018") {
                    for j in [&mut pa, &mut pb] {
                        for st in j["dim_styles"].as_array_mut().into_iter().flatten() {
                            st.as_object_mut().map(|o| o.remove("fixedExtLength"));
                        }
                    }
                }
                for (k, x) in pa.as_object().into_iter().flatten() {
                    if let Some(diff) = diff(x, &pb[k], k) {
                        report.push(format!("{tag}: {diff}"));
                    }
                }
                compare_entities(&tag, &a, &b, &mut report);
                for (la, lb) in a.layouts.iter().zip(&b.layouts) {
                    let ea: Vec<J> = la.entities.iter().map(|e| serde_json::to_value(e).unwrap()).collect();
                    let eb: Vec<J> = lb.entities.iter().map(|e| serde_json::to_value(e).unwrap()).collect();
                    if let Some(diff) = diff(&J::Array(ea), &J::Array(eb), &format!("layout {}", la.name)) {
                        report.push(format!("{tag}: {diff}"));
                    }
                }
                let (ha, hb) = (serde_json::to_value(&a.header).unwrap()["vars"].clone(), serde_json::to_value(&b.header).unwrap()["vars"].clone());
                // HANDSEED: the bridge reports its own next handle, not the file's.
                for (k, x) in ha.as_object().into_iter().flatten().filter(|(k, _)| source.contains(k.as_str()) && *k != "HANDSEED") {
                    if let Some(diff) = diff(x, hb.get(k).unwrap_or(&J::Null), &format!("header {k}")) {
                        report.push(format!("{tag}: {diff}"));
                    }
                }
            }
        }
    }
    report.sort();
    report.dedup();
    assert!(report.is_empty(), "{} differences:\n{}", report.len(), report.join("\n"));
}

/// Complex linetypes keep their text through the native writer: the text, its style, scale,
/// rotation and offset (the writer used to keep only the dash lengths, so a fence line lost its
/// marks). Both the narrow (R2004) and the wide (R2007+) strings area.
#[test]
fn complex_linetype_text_survives_the_native_writer() {
    let mut d = Drawing::new_metric();
    d.text_styles.push(TextStyle { name: "Marks".into(), ..TextStyle::default() });
    let mark = DashElement {
        text: Some("GAS".into()),
        style: Some("Marks".into()),
        scale: 0.1,
        rotation: 0.25,
        offset: Vec2::new(-0.1, -0.05),
        ..DashElement::dash(-0.25)
    };
    d.linetypes.push(Linetype {
        name: "MADEUP_GAS".into(),
        description: "Gas line ---- GAS ----".into(),
        pattern: vec![DashElement::dash(0.5), mark],
    });
    for v in ["AC1018", "AC1024", "AC1032"] {
        let (ours, _) = cadaddy_io::dwg_write::write(&d, v).unwrap();
        let back = cadaddy_io::read(&ours, "made-up.dwg").unwrap();
        let lt = back.linetypes.iter().find(|l| l.name == "MADEUP_GAS").unwrap_or_else(|| panic!("{v}: linetype missing"));
        let t = lt.pattern.iter().find(|e| e.text.is_some()).unwrap_or_else(|| panic!("{v}: the text dash was lost: {:?}", lt.pattern));
        assert_eq!(t.text.as_deref(), Some("GAS"), "{v}");
        assert_eq!(t.style.as_deref().map(str::to_ascii_uppercase).as_deref(), Some("MARKS"), "{v}");
        assert!((t.scale - 0.1).abs() < 1e-9 && (t.rotation - 0.25).abs() < 1e-9, "{v}: {t:?}");
        assert!((t.offset.x + 0.1).abs() < 1e-9 && (t.offset.y + 0.05).abs() < 1e-9, "{v}: {t:?}");
        assert!((t.length + 0.25).abs() < 1e-9, "{v}: {t:?}");
    }
}
