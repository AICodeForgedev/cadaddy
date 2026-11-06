//! The codec's groups for entities a CADaddy drawing saves as DWG equal the DWG bridge's groups
//! for the same objects (codec development aid and regression check, Q199).

use std::collections::BTreeMap;

use cadaddy_doc::*;
use cadaddy_dwgcodec::entities::{Value, decode_all};
use cadaddy_geom::Vec3;

/// The bridge's entity groups by handle.
fn bridge(bytes: &[u8]) -> BTreeMap<u64, (String, Vec<(i32, String)>)> {
    let (dxf, _) = cadaddy_dwg::dwg_to_dxf_noted(bytes, cadaddy_dwg::Limits::DEFAULT).unwrap();
    let text = String::from_utf8_lossy(&dxf);
    let lines: Vec<&str> = text.lines().collect();
    let mut map = BTreeMap::new();
    let (mut section, mut want) = (String::new(), false);
    let mut cur: Option<(String, Vec<(i32, String)>)> = None;
    let flush = |cur: &mut Option<(String, Vec<(i32, String)>)>, map: &mut BTreeMap<u64, (String, Vec<(i32, String)>)>| {
        if let Some((n, g)) = cur.take()
            && let Some(h) = g.iter().find(|x| x.0 == 5).and_then(|x| u64::from_str_radix(x.1.trim(), 16).ok())
        {
            map.insert(h, (n, g));
        }
    };
    for p in lines.chunks(2) {
        let [c, v] = p else { break };
        let code: i32 = c.trim().parse().unwrap_or(-1);
        let v = v.trim_end_matches('\r').to_string();
        if code == 0 {
            flush(&mut cur, &mut map);
            want = v == "SECTION";
            if section == "ENTITIES" || section == "BLOCKS" {
                cur = Some((v, Vec::new()));
            }
            continue;
        }
        if want && code == 2 {
            section = v.clone();
            want = false;
        }
        if let Some((_, g)) = cur.as_mut() {
            g.push((code, v));
        }
    }
    flush(&mut cur, &mut map);
    map
}

fn same(a: &Value, b: &str) -> bool {
    let t = b.trim();
    match a {
        Value::Str(s) => s.trim() == t,
        Value::Int(i) => t.parse::<i64>().is_ok_and(|x| x == *i),
        Value::Real(f) => t.parse::<f64>().is_ok_and(|x| (x - f).abs() <= 1e-9 * f.abs().max(1.0)),
        Value::Handle(h) => u64::from_str_radix(t, 16).is_ok_and(|x| x == *h),
    }
}

/// Group-by-group differences between the codec and the bridge for entities named `names`, in
/// order (each group in sequence, skipping handles and owners).
fn differences(d: &Drawing, version: &str, names: &[&str]) -> Vec<String> {
    let mut d = d.clone();
    d.header.set_str("ACADVER", version);
    let bytes = cadaddy_io::write(&d, "made-up.dwg").unwrap();
    let theirs = bridge(&bytes);
    let dwg = cadaddy_dwgcodec::read(&bytes).unwrap();
    let (ents, failed) = decode_all(&dwg).unwrap();
    let mut out: Vec<String> = failed.iter().map(|(h, e)| format!("{version} {h:x}: {e}")).collect();
    let mut seen = 0;
    for e in ents.iter().filter(|e| names.contains(&e.name.as_str())) {
        seen += 1;
        let Some((_, tg)) = theirs.get(&e.handle) else {
            out.push(format!("{version} {} {:x}: not in the bridge's DXF", e.name, e.handle));
            continue;
        };
        let skip = |c: i32| matches!(c, 5 | 330 | 102 | 360 | 347 | 390 | 295);
        let ours: Vec<_> = e.groups.iter().filter(|g| !skip(g.code)).collect();
        let theirs: Vec<_> = tg.iter().filter(|g| !skip(g.0)).collect();
        for (i, pair) in ours.iter().zip(&theirs).enumerate() {
            if pair.0.code != pair.1.0 || !same(&pair.0.value, &pair.1.1) {
                let lo = i.saturating_sub(4);
                let o: Vec<String> = ours.iter().skip(lo).take(8).map(|g| format!("{} {:?}", g.code, g.value)).collect();
                let t: Vec<String> = theirs.iter().skip(lo).take(8).map(|g| format!("{} {}", g.0, g.1.trim())).collect();
                out.push(format!(
                    "{version} {} {:x} at {i}: ours {} {:?}, theirs {} {:?}\n   ours  {o:?}\n   theirs {t:?}",
                    e.name, e.handle, pair.0.code, pair.0.value, pair.1.0, pair.1.1
                ));
                break;
            }
        }
        if ours.len() != theirs.len() {
            out.push(format!("{version} {} {:x}: {} groups, theirs {}", e.name, e.handle, ours.len(), theirs.len()));
        }
    }
    if seen == 0 {
        out.push(format!("{version}: no {names:?} decoded"));
    }
    out
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
fn multileaders_decode_like_the_bridge() {
    let d = mleader_drawing();
    let mut report = Vec::new();
    // 2004 has no multileaders: the drawing saves LEADER and MTEXT there. 2007 stores no leader
    // line properties (spec §20.4.86: R2010), so the bridge's DXF has its own defaults there; the
    // drawing read natively is checked below.
    for v in ["AC1024", "AC1027", "AC1032"] {
        report.extend(differences(&d, v, &["MULTILEADER"]));
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
}

#[test]
fn multileaders_read_natively_make_the_same_drawing() {
    let d = mleader_drawing();
    for v in ["AC1021", "AC1024", "AC1027", "AC1032"] {
        let mut d = d.clone();
        d.header.set_str("ACADVER", v);
        let bytes = cadaddy_io::write(&d, "made-up.dwg").unwrap();
        let bridge = cadaddy_io::read(&bytes, "made-up.dwg").unwrap();
        let (native, rep) = cadaddy_io::dwg_native::read_with_native_entities(&bytes).unwrap();
        assert!(rep.failed.is_empty(), "{v}: {:?}", rep.failed);
        let ours: Vec<_> = native.model.iter().map(|e| serde_json::to_value(e).unwrap()).collect();
        let theirs: Vec<_> = bridge.model.iter().map(|e| serde_json::to_value(e).unwrap()).collect();
        if let Some(d) = diff(&serde_json::Value::Array(ours), &serde_json::Value::Array(theirs), "") {
            panic!("{v}: {d}");
        }
        assert!(native.model.iter().filter(|e| matches!(e.kind, EntityKind::MLeader(_))).count() == 2, "{v}");
    }
}

fn diff(a: &serde_json::Value, b: &serde_json::Value, path: &str) -> Option<String> {
    use serde_json::Value as J;
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
