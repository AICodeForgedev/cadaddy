//! The synthetic hotel as a field package (Q312, Q350): every room, wall, column, run,
//! terminal, fixture, device and casework item, the device types with their spec items, and the
//! planted clashes and print gaps — the model the field viewer and the shared project page are
//! measured with.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_walk3d::field::{FieldElement, FieldIssue, FieldLevel, FieldPackage, FieldRoom, FieldSpec, FieldType, IssueKind, trade_info, trade_of};

use crate::truth::Truth;

fn bx(id: &str, trade: &str, kind: &str, level: &str, min: [f64; 3], max: [f64; 3]) -> FieldElement {
    FieldElement { id: id.into(), trade: trade.into(), kind: kind.into(), level: level.into(), min, max, ..FieldElement::default() }
}

fn around(at: [f64; 3], half: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    ([at[0] - half[0], at[1] - half[1], at[2] - half[2]], [at[0] + half[0], at[1] + half[1], at[2] + half[2]])
}

/// The hotel's model as `fieldview.export` lays it out, made from its truth.
pub fn field_package(t: &Truth) -> FieldPackage {
    let mut p = FieldPackage::new(&t.building.name);
    let lv = &t.building.levels;
    let elev = |name: &str| lv.iter().find(|l| l.name == name || l.code == name).map_or(0.0, |l| l.elevation_ft);
    let name_of = |name: &str| lv.iter().find(|l| l.name == name || l.code == name).map_or(name.to_string(), |l| l.name.clone());
    for (i, l) in lv.iter().enumerate() {
        let next = lv.get(i + 1).map_or(l.elevation_ft + 12.0, |n| n.elevation_ft);
        p.levels.push(FieldLevel { name: l.name.clone(), elevation_ft: l.elevation_ft, height_ft: next - l.elevation_ft });
    }
    let height = |name: &str| p.levels.iter().find(|l| l.name == name_of(name)).map_or(10.0, |l| l.height_ft);
    let units: BTreeMap<(String, String), String> = t
        .unit_instances
        .iter()
        .map(|u| {
            let ty = t.unit_types.iter().find(|x| x.id == u.unit_type).map_or(u.unit_type.clone(), |x| x.name.clone());
            ((name_of(&u.level), u.number.clone()), format!("{ty} ({})", u.hand))
        })
        .collect();
    for r in &t.building.rooms {
        let (a, b) = (r.min_ft, r.max_ft);
        let level = name_of(&r.level);
        p.rooms.push(FieldRoom {
            number: r.number.clone(),
            name: r.name.clone(),
            unit: units.get(&(level.clone(), r.number.clone())).cloned().unwrap_or_default(),
            level,
            outline: vec![[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]],
            at: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5],
        });
    }
    for w in &t.building.walls {
        let z = elev(&w.level);
        let h = w.thickness_in / 24.0;
        let (f, to) = (w.from_ft, w.to_ft);
        let mut e = bx(
            &w.id,
            "arch",
            "wall",
            &name_of(&w.level),
            [f[0].min(to[0]) - h, f[1].min(to[1]) - h, z],
            [f[0].max(to[0]) + h, f[1].max(to[1]) + h, z + height(&w.level)],
        );
        let hh = height(&w.level);
        e.path = vec![[f[0], f[1], z + hh * 0.5], [to[0], to[1], z + hh * 0.5]];
        e.size_in = Some([w.thickness_in, hh * 12.0]);
        e.source = format!("{}\" stud wall", w.thickness_in);
        p.elements.push(e);
    }
    for c in &t.building.columns {
        let (min, max) = around([c.at_ft[0], c.at_ft[1], 0.0], [c.footprint_in[0] / 24.0, c.footprint_in[1] / 24.0, 0.0]);
        let level = lv.iter().rev().find(|l| l.elevation_ft <= c.bottom_ft + 1e-6).map_or(String::new(), |l| l.name.clone());
        p.elements.push(bx(&c.id, "struct", "column", &level, [min[0], min[1], c.bottom_ft], [max[0], max[1], c.top_ft]));
    }
    let kinds: BTreeMap<&str, &str> = t.systems.iter().map(|s| (s.name.as_str(), s.kind.as_str())).collect();
    for s in &t.segments {
        let [a, b] = s.path_ft;
        let kind = kinds.get(s.system.as_str()).copied().unwrap_or(s.kind.as_str());
        let kind = if kind.contains("duct") || kind == "supply" || kind == "return" || kind == "exhaust" { "duct" } else { "pipe" };
        let r = s.size_in[0].max(s.size_in[1]) / 24.0;
        let mut e = bx(
            &s.id,
            trade_of(kind, &s.system),
            kind,
            &name_of(&s.level),
            [a[0].min(b[0]) - r, a[1].min(b[1]) - r, a[2].min(b[2]) - r],
            [a[0].max(b[0]) + r, a[1].max(b[1]) + r, a[2].max(b[2]) + r],
        );
        e.path = vec![a, b];
        e.size_in = Some(s.size_in);
        e.source = format!("{} {}", s.sheet, s.system);
        p.elements.push(e);
    }
    for c in &t.elec_conduits {
        let z = elev(&c.level) + 9.5;
        let (f, to) = (c.from_ft, c.to_ft);
        let r = c.size_in / 24.0;
        let mut e = bx(
            &c.id,
            "elec",
            "conduit",
            &name_of(&c.level),
            [f[0].min(to[0]) - r, f[1].min(to[1]) - r, z - r],
            [f[0].max(to[0]) + r, f[1].max(to[1]) + r, z + r],
        );
        e.path = vec![[f[0], f[1], z], [to[0], to[1], z]];
        e.size_in = Some([c.size_in, 0.0]);
        e.mark = c.conduit_type.clone();
        e.source = c.sheet.clone();
        p.elements.push(e);
    }
    for x in &t.terminals {
        let (min, max) = around(x.at_ft, [x.face_in[0] / 24.0, x.face_in[1] / 24.0, 0.25]);
        let mut e = bx(&x.id, "mech", "terminal", &name_of(&x.level), min, max);
        e.room = x.room.clone();
        e.mark = x.type_tag.clone();
        p.elements.push(e);
    }
    for x in &t.equipment {
        let mut e = bx(&x.id, trade_of("equipment", &x.kind), "equipment", &name_of(&x.level), x.min_ft, x.max_ft);
        e.source = x.tag.clone();
        p.elements.push(e);
    }
    for x in &t.fixtures {
        let (min, max) = around(x.at_ft, [1.0, 1.0, 1.25]);
        let mut e = bx(&x.id, "plumb", "fixture", &name_of(&x.level), min, max);
        e.mark = x.tag.clone();
        p.elements.push(e);
    }
    for x in &t.elec_devices {
        let (min, max) = around(x.at_ft, [0.2, 0.2, 0.2]);
        let mut e = bx(&x.id, "elec", "device", &name_of(&x.level), min, max);
        e.mark = x.type_tag.clone();
        e.source = format!("{} circuit {}-{}", x.sheet, x.panel, x.circuit);
        p.elements.push(e);
    }
    for x in &t.fire_alarm_devices {
        let (min, max) = around(x.at_ft, [0.25, 0.25, 0.25]);
        let mut e = bx(&x.id, "fire", "device", &name_of(&x.level), min, max);
        e.mark = x.tag.clone();
        p.elements.push(e);
    }
    for x in &t.casework {
        let z = elev(&x.level);
        let mut e =
            bx(&x.id, "arch", "casework", &name_of(&x.level), [x.min_ft[0], x.min_ft[1], z], [x.max_ft[0], x.max_ft[1], z + x.height_in / 12.0]);
        e.mark = x.tag.clone();
        p.elements.push(e);
    }
    // Types: the device type tags, with the spec items they point to as their source.
    let mut marks: BTreeSet<String> = p.elements.iter().filter(|e| !e.mark.is_empty()).map(|e| e.mark.clone()).collect();
    marks.retain(|m| m.len() <= 16);
    for m in marks {
        let spec = t.spec_items.iter().find(|s| s.referenced_by.iter().any(|r| r == &m));
        let mut fields = vec![FieldSpec { key: "type_mark".into(), value: m.clone(), source: "plans".into(), confidence: "stated".into() }];
        if let Some(s) = spec {
            fields.push(FieldSpec {
                key: "manufacturer".into(),
                value: s.manufacturer.clone(),
                source: format!("spec {}", s.section),
                confidence: "stated".into(),
            });
            fields.push(FieldSpec {
                key: "model".into(),
                value: s.model.clone(),
                source: format!("spec {}", s.section),
                confidence: "stated".into(),
            });
        }
        p.types.push(FieldType { mark: m, category: String::new(), fields });
    }
    for c in &t.clashes {
        let level = lv.iter().rev().find(|l| l.elevation_ft <= c.at_ft[2] + 1e-6).map_or(String::new(), |l| l.name.clone());
        p.issues.push(FieldIssue {
            id: c.id.clone(),
            kind: IssueKind::Clash,
            title: c.description.clone().unwrap_or_else(|| format!("{} vs {}", c.a, c.b)),
            status: "new".into(),
            level,
            room: String::new(),
            at: c.at_ft,
            elements: vec![c.a.clone(), c.b.clone()],
        });
    }
    for g in &t.gaps {
        let level = name_of(&g.level);
        let z = elev(&g.level);
        p.issues.push(FieldIssue {
            id: g.id.clone(),
            kind: IssueKind::Rfi,
            title: g.description.clone(),
            status: "open".into(),
            level,
            room: String::new(),
            at: [g.at_ft[0], g.at_ft[1], z],
            elements: g.elements.clone(),
        });
    }
    let counts = p.trade_counts();
    p.trades =
        ["arch", "struct", "mech", "plumb", "elec", "fire", "lowvolt"].iter().filter(|k| counts.contains_key(**k)).map(|k| trade_info(k)).collect();
    p
}

/// The hotel as a shared project page's model (Q350), stripped as `share.export` strips: written
/// into `dir` (`model/`, `thumbs/`). `wings` copies of the building stand side by side (the
/// second's rooms numbered `B…`, and so on), for a job larger than one hotel. Returns the
/// manifest and the strip report.
pub fn write_share(
    t: &Truth,
    dir: &std::path::Path,
    title: &str,
    wings: usize,
) -> Result<(cadaddy_walk3d::share::ShareManifest, cadaddy_walk3d::share::StripReport), String> {
    use cadaddy_walk3d::share;
    let mut pkg = field_package(t);
    let (rooms, elements, issues) = (pkg.rooms.clone(), pkg.elements.clone(), pkg.issues.clone());
    let span = rooms.iter().flat_map(|r| r.outline.iter().map(|q| q[0])).fold(0.0_f64, f64::max) + 60.0;
    for w in 1..wings.clamp(1, 8) {
        let dx = span * w as f64;
        let tag = char::from(b'A' + u8::try_from(w).unwrap_or(0));
        let id = |s: &str| format!("{tag}{s}");
        pkg.rooms.extend(rooms.iter().map(|r| FieldRoom {
            number: id(&r.number),
            outline: r.outline.iter().map(|q| [q[0] + dx, q[1]]).collect(),
            at: [r.at[0] + dx, r.at[1]],
            ..r.clone()
        }));
        pkg.elements.extend(elements.iter().map(|e| FieldElement {
            id: id(&e.id),
            room: String::new(),
            min: [e.min[0] + dx, e.min[1], e.min[2]],
            max: [e.max[0] + dx, e.max[1], e.max[2]],
            path: e.path.iter().map(|q| [q[0] + dx, q[1], q[2]]).collect(),
            ..e.clone()
        }));
        pkg.issues.extend(issues.iter().map(|i| FieldIssue {
            id: id(&i.id),
            room: String::new(),
            at: [i.at[0] + dx, i.at[1], i.at[2]],
            elements: i.elements.iter().map(|x| id(x)).collect(),
            ..i.clone()
        }));
    }
    pkg.assign_rooms();
    let known = [t.building.name.clone()].into_iter().chain(t.spec_items.iter().map(|s| s.manufacturer.clone()));
    let report = share::strip(&mut pkg, title, &share::Scrubber::new(known));
    let (manifest, _) = share::write_bundle(dir, &pkg, title, share::DEFAULT_CHUNK, true, None)?;
    Ok((manifest, report))
}
