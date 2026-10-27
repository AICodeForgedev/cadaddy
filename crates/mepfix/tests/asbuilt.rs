//! Integration tests for the as-built system on generated fixture sets: synthetic and real (PDF)
//! markups mapped to the exact element they mark, the interpreter's moves scored against the
//! truth, pending changes kept out of the as-built model, as-built sheets and IFC read back, and
//! field-verified studs through the framing.

use cadaddy_doc::markup::{Markup, MarkupKind};
use cadaddy_doc::{EntityKind, Space};
use cadaddy_mepfix::asbuilt::*;
use cadaddy_mepfix::framing::{self, Status};
use cadaddy_mepfix::truth::{Sheet, Truth};

/// Seed 1's office set, planned (sheets and their transforms) but not drawn.
fn fixture_truth() -> Truth {
    cadaddy_mepfix::office_planned(1).0
}

/// The first plan sheet of a level.
fn plan_sheet<'a>(t: &'a Truth, level: &str) -> &'a Sheet {
    t.sheets.iter().find(|s| s.level.as_deref() == Some(level)).expect("the set has a plan sheet of the level")
}

fn mid(p: [[f64; 3]; 2]) -> [f64; 2] {
    [(p[0][0] + p[1][0]) * 0.5, (p[0][1] + p[1][1]) * 0.5]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// A synthetic markup at a building point on a sheet (`to`: an arrow's head, building feet).
fn markup_at(sh: &Sheet, kind: &str, text: &str, at_ft: [f64; 2], to_ft: Option<[f64; 2]>) -> SyntheticMarkup {
    let paper = building_to_paper(sh, at_ft);
    SyntheticMarkup {
        kind: kind.into(),
        text: text.into(),
        author: "Field Reviewer A".into(),
        date: "2026-10-21".into(),
        paper_in: paper,
        paper_rect_in: [[paper[0] - 0.2, paper[1] - 0.2], [paper[0] + 0.2, paper[1] + 0.2]],
        to_in: to_ft.map(|p| building_to_paper(sh, p)),
    }
}

fn one(t: &Truth, sh: &Sheet, m: SyntheticMarkup, id: &str) -> AsBuiltChange {
    let mapped = import_markups(t, sh, &[m]);
    assert_eq!(mapped.len(), 1);
    markup_to_change(&mapped[0], t, id).expect("the markup is on an element")
}

fn decided(mut c: AsBuiltChange) -> AsBuiltChange {
    c.status = ChangeStatus::FieldDecision;
    c
}

fn moved(c: &AsBuiltChange) -> ([f64; 3], [f64; 3]) {
    match c.kind {
        ChangeKind::Moved { from_ft, to_ft } => (from_ft, to_ft),
        ref k => panic!("expected a move, got {k:?}"),
    }
}

#[test]
fn full_workflow_synthetic_markups() {
    let t = fixture_truth();
    let sh = plan_sheet(&t, "1");
    let duct = t.segments.iter().find(|s| s.level == "1" && s.kind == "duct" && !s.vertical).expect("a duct on level 1");
    let rec_dev = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "receptacle").expect("a receptacle on level 1");
    let term = t.terminals.iter().find(|x| x.level == "1").expect("a terminal on level 1");
    let cond = t.elec_conduits.iter().find(|c| c.level == "1").expect("a conduit on level 1");
    let dev_xy = [rec_dev.at_ft[0], rec_dev.at_ft[1]];
    let markups = vec![
        markup_at(sh, "cloud", "Move duct 2 feet north per field decision", mid(duct.path_ft), None),
        markup_at(sh, "arrow", "Relocate receptacle to opposite wall", dev_xy, Some([dev_xy[0] + 3.0, dev_xy[1]])),
        markup_at(sh, "callout", "Verified diffuser location in field", [term.at_ft[0], term.at_ft[1]], None),
        markup_at(
            sh,
            "note",
            "Change conduit type from EMT to RGS per RFI-42",
            [(cond.from_ft[0] + cond.to_ft[0]) * 0.5, (cond.from_ft[1] + cond.to_ft[1]) * 0.5],
            None,
        ),
    ];
    let mapped = import_markups(&t, sh, &markups);
    assert_eq!(mapped.len(), 4);
    let mut record = AsBuiltRecord::new(&t.project);
    for (i, m) in mapped.iter().enumerate() {
        assert_eq!(m.sheet, sh.number);
        assert_eq!(m.level, "1");
        let c = markup_to_change(m, &t, &format!("C-{}", i + 1)).expect("every markup is on an element");
        assert_eq!(c.markup_id.as_deref(), Some(m.id.as_str()));
        assert_eq!(c.status, ChangeStatus::Pending);
        record.add_change(c);
    }
    record.markups.extend(mapped);
    let ids: Vec<&str> = record.changes.iter().map(|c| c.element_id.as_str()).collect();
    assert_eq!(ids, [duct.id.as_str(), rec_dev.id.as_str(), term.id.as_str(), cond.id.as_str()]);

    let (from, to) = moved(&record.changes[0]);
    assert!(dist([to[0] - from[0], to[1] - from[1]], [0.0, 2.0]) < 0.1, "duct moved 2' north: {from:?} -> {to:?}");
    assert!((to[2] - from[2]).abs() < 1e-9, "the duct keeps its height");
    let (from, to) = moved(&record.changes[1]);
    assert!(dist([to[0], to[1]], [dev_xy[0] + 3.0, dev_xy[1]]) < 0.1, "receptacle to the arrow's head: {to:?}");
    assert!((to[2] - rec_dev.at_ft[2]).abs() < 1e-9 && (from[2] - rec_dev.at_ft[2]).abs() < 1e-9, "the receptacle keeps its height");
    assert_eq!(record.changes[2].kind, ChangeKind::FieldVerified { at_ft: term.at_ft });
    match &record.changes[3].kind {
        ChangeKind::TypeChanged { design_type, .. } => assert_eq!(design_type, &cond.conduit_type),
        k => panic!("expected a type change, got {k:?}"),
    }

    // All pending: the as-built model is the design.
    let ab = asbuilt_truth(&t, &record);
    assert_eq!(ab, t);
    // Decided: the moves land exactly.
    for c in &mut record.changes {
        c.status = ChangeStatus::ChangeOrder;
    }
    let ab = asbuilt_truth(&t, &record);
    let d = ab.elec_devices.iter().find(|d| d.id == rec_dev.id).expect("still there");
    assert!(dist([d.at_ft[0], d.at_ft[1]], [dev_xy[0] + 3.0, dev_xy[1]]) < 0.1);
    let s = ab.segments.iter().find(|s| s.id == duct.id).expect("still there");
    assert!(dist(mid(s.path_ft), [mid(duct.path_ft)[0], mid(duct.path_ft)[1] + 2.0]) < 0.1);
    assert_eq!(compare_design_vs_asbuilt(&t, &record).len(), 4);
    let summary = summarize(&record);
    assert_eq!((summary.total_changes, summary.change_orders, summary.moves, summary.verifications, summary.type_changes), (4, 4, 2, 1, 1));
    let json = serde_json::to_string(&record).expect("json");
    let back: AsBuiltRecord = serde_json::from_str(&json).expect("json back");
    assert_eq!(back, record);
}

#[test]
fn markups_on_multiple_seeds() {
    for seed in 1..=3 {
        let t = cadaddy_mepfix::office_planned(seed).0;
        let sh = plan_sheet(&t, "1");
        let dev = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "receptacle").expect("a receptacle");
        let at = [dev.at_ft[0], dev.at_ft[1]];
        let c = one(&t, sh, markup_at(sh, "cloud", "Move receptacle 2 feet east", at, None), "C-1");
        assert_eq!(c.element_id, dev.id, "seed {seed}");
        let (from, to) = moved(&c);
        assert!(dist([to[0] - from[0], to[1] - from[1]], [2.0, 0.0]) < 0.1, "seed {seed}: {from:?} -> {to:?}");
    }
}

#[test]
fn cloud_on_segment_produces_move() {
    let t = fixture_truth();
    let sh = plan_sheet(&t, "1");
    let seg = t.segments.iter().find(|s| s.level == "1" && s.kind == "duct" && !s.vertical).expect("a duct on level 1");
    let c = one(&t, sh, markup_at(sh, "cloud", "Move this duct run 3 feet east", mid(seg.path_ft), None), "C-1");
    assert_eq!(c.element_id, seg.id);
    assert_eq!(c.element_kind, "segment");
    let (from, to) = moved(&c);
    assert!(dist([from[0], from[1]], mid(seg.path_ft)) < 1e-6);
    assert!(dist([to[0] - from[0], to[1] - from[1]], [3.0, 0.0]) < 0.1);
}

#[test]
fn arrow_on_device_produces_move() {
    let t = fixture_truth();
    let sh = plan_sheet(&t, "1");
    let dev = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "switch").expect("a switch on level 1");
    let at = [dev.at_ft[0], dev.at_ft[1]];
    let head = [at[0], at[1] + 4.0];
    let c = one(&t, sh, markup_at(sh, "arrow", "Move switch here", at, Some(head)), "C-1");
    assert_eq!(c.element_id, dev.id);
    let (from, to) = moved(&c);
    assert_eq!(from, dev.at_ft);
    assert!(dist([to[0], to[1]], head) < 0.1, "{to:?}");
    assert_eq!(to[2], dev.at_ft[2]);
}

#[test]
fn callout_verified_produces_field_verified() {
    let t = fixture_truth();
    let sh = plan_sheet(&t, "1");
    let term = t.terminals.iter().find(|x| x.level == "1").expect("a terminal on level 1");
    let c = one(&t, sh, markup_at(sh, "callout", "Verified: diffuser at correct location", [term.at_ft[0], term.at_ft[1]], None), "C-1");
    assert_eq!(c.element_id, term.id);
    assert_eq!(c.element_kind, "terminal");
    assert_eq!(c.kind, ChangeKind::FieldVerified { at_ft: term.at_ft });
}

#[test]
fn delete_markup_produces_deletion() {
    let t = fixture_truth();
    let sh = plan_sheet(&t, "1");
    let dev = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "receptacle").expect("a receptacle on level 1");
    let at = [dev.at_ft[0], dev.at_ft[1]];
    for text in ["Delete this receptacle - not needed", "Remove receptacle", "REMOVED"] {
        let c = one(&t, sh, markup_at(sh, "cloud", text, at, None), "C-DEL");
        assert_eq!(c.element_id, dev.id, "{text}");
        assert_eq!(c.kind, ChangeKind::Deleted, "{text}: a removal is a deletion, not a move");
        let mut rec = AsBuiltRecord::new(&t.project);
        rec.add_change(c.clone());
        assert!(asbuilt_truth(&t, &rec).elec_devices.iter().any(|d| d.id == dev.id), "pending: still in the as-built model");
        rec.changes[0] = decided(c);
        let ab = asbuilt_truth(&t, &rec);
        assert!(ab.elec_devices.iter().all(|d| d.id != dev.id));
        assert_eq!(ab.elec_devices.len(), t.elec_devices.len() - 1);
    }
}

#[test]
fn type_change_markup() {
    let t = fixture_truth();
    let sh = plan_sheet(&t, "1");
    let dev = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "light_fixture").expect("a light on level 1");
    let c = one(&t, sh, markup_at(sh, "cloud", "Change type to Type C fixture", [dev.at_ft[0], dev.at_ft[1]], None), "C-TYPE");
    assert_eq!(c.element_id, dev.id);
    assert_eq!(c.kind, ChangeKind::TypeChanged { design_type: dev.type_tag.clone(), asbuilt_type: "Change type to Type C fixture".into() });
}

#[test]
fn rerouted_callout_leaves_path_unchanged() {
    let t = fixture_truth();
    let sh = plan_sheet(&t, "1");
    let seg = t.segments.iter().find(|s| s.level == "1" && s.kind == "duct" && !s.vertical).expect("a duct on level 1");
    let c = one(&t, sh, markup_at(sh, "callout", "Duct rerouted around beam", mid(seg.path_ft), None), "C-1");
    assert_eq!(c.element_id, seg.id);
    assert_eq!(c.kind, ChangeKind::Note, "no route given: a note, never a zero path");
    assert!(c.notes.starts_with("needs value"), "{}", c.notes);
    let mut rec = AsBuiltRecord::new(&t.project);
    rec.add_change(decided(c));
    let ab = asbuilt_truth(&t, &rec);
    assert_eq!(ab.segments.iter().find(|s| s.id == seg.id).map(|s| s.path_ft), Some(seg.path_ft));
}

#[test]
fn raise_without_value_leaves_height_unchanged() {
    let t = fixture_truth();
    let sh = plan_sheet(&t, "1");
    let cond = t.elec_conduits.iter().find(|c| c.level == "1").expect("a conduit on level 1");
    let at = [(cond.from_ft[0] + cond.to_ft[0]) * 0.5, (cond.from_ft[1] + cond.to_ft[1]) * 0.5];
    let c = one(&t, sh, markup_at(sh, "callout", "Raise conduit", at, None), "C-1");
    assert_eq!(c.element_id, cond.id);
    assert_eq!(c.kind, ChangeKind::Note);
    assert!(c.notes.starts_with("needs value"), "{}", c.notes);

    let seg = t.segments.iter().find(|s| s.level == "1" && s.kind == "duct" && s.height.is_some()).expect("a duct with a height");
    let h = seg.height.as_ref().map(|h| h.centre_ft).expect("height");
    let c = one(&t, sh, markup_at(sh, "callout", "Raise duct", mid(seg.path_ft), None), "C-2");
    assert_eq!((c.element_id.as_str(), &c.kind), (seg.id.as_str(), &ChangeKind::Note));
    let mut rec = AsBuiltRecord::new(&t.project);
    rec.add_change(decided(c));
    let ab = asbuilt_truth(&t, &rec);
    assert_eq!(ab.segments.iter().find(|s| s.id == seg.id).and_then(|s| s.height.as_ref()).map(|h| h.centre_ft), Some(h));

    // With its value it is a height change, and decided it is applied.
    let c = one(&t, sh, markup_at(sh, "callout", "Raise duct to 11'-6\"", mid(seg.path_ft), None), "C-3");
    assert_eq!(c.kind, ChangeKind::HeightChanged { design_ft: h, asbuilt_ft: 11.5 });
    let rec = AsBuiltRecord { changes: vec![decided(c)], ..AsBuiltRecord::new(&t.project) };
    let ab = asbuilt_truth(&t, &rec);
    let s = ab.segments.iter().find(|s| s.id == seg.id).expect("still there");
    assert_eq!(s.height.as_ref().map(|h| h.centre_ft), Some(11.5));
    assert!((s.path_ft[0][2] - (seg.path_ft[0][2] + 11.5 - h)).abs() < 1e-9);
}

#[test]
fn markup_on_device_over_duct_picks_the_device() {
    let mut t = fixture_truth();
    let sh = plan_sheet(&t, "1").clone();
    let dev = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "receptacle").expect("a receptacle").clone();
    // A duct run straight over the receptacle.
    let seg = t.segments.iter_mut().find(|s| s.level == "1" && s.kind == "duct" && !s.vertical).expect("a duct");
    let z = seg.path_ft[0][2];
    seg.path_ft = [[dev.at_ft[0] - 5.0, dev.at_ft[1], z], [dev.at_ft[0] + 5.0, dev.at_ft[1], z]];
    let duct = seg.id.clone();
    let mapped = import_markups(&t, &sh, &[markup_at(&sh, "cloud", "Move receptacle 2 feet east", [dev.at_ft[0], dev.at_ft[1]], None)]);
    assert!(mapped[0].near_elements.contains(&duct), "the duct overlaps the receptacle");
    let c = markup_to_change(&mapped[0], &t, "C-1").expect("a change");
    assert_eq!(c.element_id, dev.id);
    assert_eq!(c.element_kind, "elec_device");
    let c = one(&t, &sh, markup_at(&sh, "cloud", "Move duct 2 feet east", [dev.at_ft[0], dev.at_ft[1]], None), "C-2");
    assert_eq!(c.element_id, duct, "the text names the duct");
}

#[test]
fn pending_move_is_not_applied_until_decided() {
    let t = fixture_truth();
    let dev = t.elec_devices.iter().find(|d| d.level == "1").expect("a device");
    let target = [dev.at_ft[0] + 1.25, dev.at_ft[1] - 2.5, dev.at_ft[2]];
    let change = AsBuiltChange {
        id: "C-1".into(),
        element_id: dev.id.clone(),
        element_kind: "elec_device".into(),
        kind: ChangeKind::Moved { from_ft: dev.at_ft, to_ft: target },
        status: ChangeStatus::Pending,
        author: "Foreman A".into(),
        date: "2026-10-21".into(),
        reason: "Clear of the door swing".into(),
        notes: String::new(),
        level: dev.level.clone(),
        at_ft: [target[0], target[1]],
        markup_id: None,
    };
    let mut rec = AsBuiltRecord::new(&t.project);
    rec.add_change(change.clone());
    let at = |ab: &Truth| ab.elec_devices.iter().find(|d| d.id == dev.id).map(|d| d.at_ft).expect("there");
    assert_eq!(at(&asbuilt_truth(&t, &rec)), dev.at_ft, "pending: design position");
    assert_eq!(at(&asbuilt_truth_with(&t, &rec, true)), target, "asked to include pending");
    rec.changes[0].status = ChangeStatus::FieldDecision;
    assert_eq!(at(&asbuilt_truth(&t, &rec)), target, "a field decision moves it exactly");
    rec.changes[0].status = ChangeStatus::Rejected;
    assert_eq!(at(&asbuilt_truth_with(&t, &rec, true)), dev.at_ft, "rejected: never");
}

#[test]
fn multibyte_text_is_cut_on_a_character_boundary() {
    let t = fixture_truth();
    let sh = plan_sheet(&t, "1");
    let long = "°".repeat(3_000);
    let m = SyntheticMarkup { kind: "note".into(), text: long.clone(), author: long.clone(), ..markup_at(sh, "note", "", [0.0, 0.0], None) };
    let mapped = import_markups(&t, sh, &[m]);
    assert_eq!(mapped.len(), 1);
    assert!(mapped[0].text.len() <= MAX_TEXT && mapped[0].author.len() <= MAX_TEXT);
    assert_eq!(mapped[0].text.chars().count(), MAX_TEXT / 2, "whole two-byte characters only");
    // The same through a PDF page's markups.
    let page = cadaddy_doc::markup::MarkupPage {
        width: sh.paper_in[0] * 72.0,
        height: sh.paper_in[1] * 72.0,
        markups: vec![Markup {
            kind: MarkupKind::Note,
            points: vec![[100.0, 100.0]],
            strokes: Vec::new(),
            rect: [100.0, 80.0, 120.0, 100.0],
            text: format!("x{long}"),
            author: long,
            subject: String::new(),
            color: None,
            date: String::new(),
        }],
    };
    let mapped = map_markups(&t, sh, &page, 1);
    assert!(mapped[0].text.len() <= MAX_TEXT && mapped[0].author.len() <= MAX_TEXT);
    assert!(mapped[0].text.starts_with('x'));
}

#[test]
fn pdf_markups_on_a_printed_sheet_map_to_their_elements() {
    let (t, st, kinds) = cadaddy_mepfix::office_planned(1);
    let (sh, kind) = t
        .sheets
        .iter()
        .filter(|s| s.level.as_deref() == Some("1") && s.number.starts_with('E'))
        .find_map(|s| kinds.iter().find(|(n, _)| *n == s.number).map(|(_, k)| (s, k)))
        .expect("an electrical plan of level 1");
    let (drawing, _, _) = cadaddy_mepfix::draw::draw(&t, &st, sh, kind);
    let pdf = cadaddy_io::pdf::pdf(&drawing, &Space::Model, &cadaddy_mepfix::pdf_options(sh, &st)).expect("the sheet prints");
    // Page points of a building point.
    let pts = |p: [f64; 2]| {
        let q = building_to_paper(sh, p);
        [q[0] * 72.0, q[1] * 72.0]
    };
    let recep = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "receptacle").expect("a receptacle");
    let switch = t.elec_devices.iter().find(|d| d.level == "1" && d.kind == "switch").expect("a switch");
    let cond = t.elec_conduits.iter().find(|c| c.level == "1").expect("a conduit");
    let cmid = [(cond.from_ft[0] + cond.to_ft[0]) * 0.5, (cond.from_ft[1] + cond.to_ft[1]) * 0.5];
    let c = pts([recep.at_ft[0], recep.at_ft[1]]);
    let cloud = Markup {
        kind: MarkupKind::Cloud,
        points: vec![[c[0] - 9.0, c[1] - 9.0], [c[0] + 9.0, c[1] - 9.0], [c[0] + 9.0, c[1] + 9.0], [c[0] - 9.0, c[1] + 9.0]],
        strokes: Vec::new(),
        rect: [c[0] - 9.0, c[1] - 9.0, c[0] + 9.0, c[1] + 9.0],
        text: "Receptacle removed".into(),
        author: "Field Reviewer A".into(),
        subject: String::new(),
        color: Some([255, 0, 0]),
        date: String::new(),
    };
    let tail = pts([switch.at_ft[0], switch.at_ft[1]]);
    let head = pts([switch.at_ft[0] + 2.0, switch.at_ft[1]]);
    let arrow = Markup {
        kind: MarkupKind::Arrow,
        points: vec![head, tail],
        rect: [tail[0].min(head[0]) - 2.0, tail[1] - 2.0, tail[0].max(head[0]) + 2.0, tail[1] + 2.0],
        text: "Move switch".into(),
        ..cloud.clone()
    };
    let tip = pts(cmid);
    let callout = Markup {
        kind: MarkupKind::Callout,
        points: vec![tip, [tip[0] + 30.0, tip[1] + 30.0]],
        rect: [tip[0] + 30.0, tip[1] + 30.0, tip[0] + 130.0, tip[1] + 60.0],
        text: "Conduit verified in field".into(),
        ..cloud.clone()
    };
    let marked = cadaddy_io::pdf_markups::add_markups(&pdf, 1, &[cloud, arrow, callout]).expect("markups added");
    let mapped = read_pdf_markups(&t, sh, &marked, 1, 1).expect("read back");
    assert_eq!(mapped.iter().map(|m| m.kind.as_str()).collect::<Vec<_>>(), ["cloud", "arrow", "callout"]);
    let want = [([recep.at_ft[0], recep.at_ft[1]], &recep.id), ([switch.at_ft[0], switch.at_ft[1]], &switch.id), (cmid, &cond.id)];
    for (m, (at, id)) in mapped.iter().zip(want) {
        assert!(dist(m.building_ft, at) < 0.5, "{}: {:?} vs {at:?}", m.kind, m.building_ft);
        assert_eq!(m.sheet, sh.number);
        assert_eq!(m.author, "Field Reviewer A");
        let c = markup_to_change(m, &t, "C").expect("a change");
        assert_eq!(&c.element_id, id, "{}", m.text);
    }
    assert!(dist(mapped[1].to_ft.expect("arrow head"), [switch.at_ft[0] + 2.0, switch.at_ft[1]]) < 0.5);
    let kinds: Vec<ChangeKind> = mapped.iter().filter_map(|m| markup_to_change(m, &t, "C")).map(|c| c.kind).collect();
    assert_eq!(kinds[0], ChangeKind::Deleted);
    let (from, to) = match kinds[1] {
        ChangeKind::Moved { from_ft, to_ft } => (from_ft, to_ft),
        ref k => panic!("{k:?}"),
    };
    assert!(dist([to[0] - from[0], to[1] - from[1]], [2.0, 0.0]) < 0.5);
    assert!(matches!(kinds[2], ChangeKind::FieldVerified { .. }));
}

fn change(id: &str, element: &str, kind: ChangeKind, status: ChangeStatus, level: &str, at: [f64; 2]) -> AsBuiltChange {
    AsBuiltChange {
        id: id.into(),
        element_id: element.into(),
        element_kind: String::new(),
        kind,
        status,
        author: "Engineer B".into(),
        date: "2026-10-22".into(),
        reason: "Field condition".into(),
        notes: String::new(),
        level: level.into(),
        at_ft: at,
        markup_id: None,
    }
}

#[test]
fn asbuilt_sheet_has_one_mark_per_change_and_reads_back() {
    let t = fixture_truth();
    let dev = t.elec_devices.iter().find(|d| d.level == "1").expect("a device");
    let p = dev.at_ft;
    let mut rec = AsBuiltRecord::new(&t.project);
    rec.add_change(change(
        "C-1",
        &dev.id,
        ChangeKind::Moved { from_ft: p, to_ft: [p[0] + 2.0, p[1], p[2]] },
        ChangeStatus::FieldDecision,
        "1",
        [p[0] + 2.0, p[1]],
    ));
    rec.add_change(change("C-2", &dev.id, ChangeKind::Deleted, ChangeStatus::ChangeOrder, "1", [p[0], p[1]]));
    rec.add_change(change("C-3", &dev.id, ChangeKind::Note, ChangeStatus::Pending, "1", [p[0], p[1] + 3.0]));
    rec.add_change(change("C-4", &dev.id, ChangeKind::Added, ChangeStatus::RfiAnswer, "1", [p[0] + 5.0, p[1]]));
    rec.add_change(change("C-5", &dev.id, ChangeKind::Deleted, ChangeStatus::Rejected, "1", [p[0], p[1]]));
    rec.add_change(change("C-6", &dev.id, ChangeKind::Deleted, ChangeStatus::FieldDecision, "2", [p[0], p[1]]));
    let on = |d: &cadaddy_doc::Drawing, layer: &str| d.model.iter().filter(|e| e.common.layer.to_string().eq_ignore_ascii_case(layer)).count();
    let sheet = asbuilt_sheet(&t, &rec, "1");
    assert_eq!(on(&sheet, LAYER_CHANGES), 4, "one mark per change on level 1 that isn't rejected");
    assert_eq!(on(&sheet, LAYER_LABELS), 4);
    assert!(on(&sheet, LAYER_DESIGN_WALLS) > 0 && on(&sheet, LAYER_DESIGN_ELEC) > 0, "the design under the marks");
    let legend: Vec<String> = sheet
        .model
        .iter()
        .filter(|e| e.common.layer.to_string().eq_ignore_ascii_case(LAYER_LEGEND))
        .filter_map(|e| match &e.kind {
            EntityKind::Text(x) => Some(x.value.clone()),
            _ => None,
        })
        .collect();
    for s in ["PENDING", "FIELD", "RFI", "CO"] {
        assert!(legend.iter().any(|l| l.starts_with(s)), "legend has {s}: {legend:?}");
    }
    // Written and read back.
    let files = asbuilt_sheet_files(&t, &rec);
    let f1 = files.iter().find(|f| f.level == "1").expect("level 1's sheet");
    assert_eq!(f1.marks, 4);
    let back = cadaddy_io::read_dxf(&f1.dxf).expect("reads back");
    assert_eq!(on(&back, LAYER_CHANGES), 4);
    let f2 = files.iter().find(|f| f.level == "2").expect("level 2's sheet");
    let back = cadaddy_io::read_dxf(&f2.dxf).expect("reads back");
    assert_eq!(on(&back, LAYER_CHANGES), 1);
}

#[test]
fn asbuilt_ifc_carries_the_property_set_on_changed_elements() {
    let t = fixture_truth();
    let dev = t.elec_devices.iter().find(|d| d.level == "1").expect("a device");
    let gone = t.elec_devices.iter().find(|d| d.level == "1" && d.id != dev.id).expect("another device");
    let p = dev.at_ft;
    let mut rec = AsBuiltRecord::new(&t.project);
    let mut c =
        change("C-1", &dev.id, ChangeKind::Moved { from_ft: p, to_ft: [p[0] + 2.0, p[1], p[2]] }, ChangeStatus::ChangeOrder, "1", [p[0] + 2.0, p[1]]);
    c.element_kind = "elec_device".into();
    c.author = "Engineer C".into();
    rec.add_change(c);
    let mut del = change("C-2", &gone.id, ChangeKind::Deleted, ChangeStatus::FieldDecision, "1", [0.0, 0.0]);
    del.element_kind = "elec_device".into();
    rec.add_change(del);
    let (bytes, report) = asbuilt_ifc(&t, &rec, false, "asbuilt.ifc").expect("IFC");
    assert_eq!(report.changed, vec![dev.id.clone()]);
    assert_eq!(report.not_written, vec![gone.id.clone()], "deleted: not in the as-built model");
    let back = cadaddy_io::ifc::read_ifc(&bytes).expect("reads back");
    let tags = |e: &cadaddy_doc::Entity| e.common.xdata.iter().map(|x| x.value.clone()).collect::<Vec<_>>();
    let named = |id: &str| back.model.iter().find(|e| tags(e).get(3).is_some_and(|n| n == id)).map(|e| tags(e));
    let x = named(&dev.id).expect("the changed element, by its id");
    assert!(x.iter().any(|v| v == "prop:CaDaddy_AsBuilt.AsBuiltStatus=Change Order"), "{x:?}");
    assert!(x.iter().any(|v| v == "prop:CaDaddy_AsBuilt.AsBuiltAuthor=Engineer C"), "{x:?}");
    assert!(x.iter().any(|v| v == &format!("prop:CaDaddy_AsBuilt.AsBuiltElementId={}", dev.id)), "{x:?}");
    assert!(named(&gone.id).is_none());
    let other = t.elec_devices.iter().find(|d| d.id != dev.id && d.id != gone.id).expect("a third device");
    let y = named(&other.id).expect("unchanged elements are written too");
    assert!(!y.iter().any(|v| v.starts_with("prop:CaDaddy_AsBuilt")), "{y:?}");
}

#[test]
fn field_verified_stud_snaps_the_wall_and_survives_a_rebuild() {
    let t = fixture_truth();
    let fresh = || framing::frame(&framing::walls_from_truth(&t), &framing::posts_from_truth(&t), &[], &[]);
    let mut fr = fresh();
    let wf = fr.walls.iter().find(|w| w.wall.stud_in.is_some() && w.wall.len_in() > 96.0 && w.wall.openings.is_empty()).expect("a long stud wall");
    let (wall, centre) = (wf.wall.id.clone(), 40.0);
    // Walls are searchable: a markup on one finds it.
    let wmid = [(wf.wall.a_ft[0] + wf.wall.b_ft[0]) * 0.5, (wf.wall.a_ft[1] + wf.wall.b_ft[1]) * 0.5];
    assert!(find_near_elements(&t, &wf.wall.level, wmid, 3.0).contains(&wall));
    let who = Recorded {
        id: "C-1".into(),
        status: ChangeStatus::FieldDecision,
        author: "Carpenter A".into(),
        date: "2026-10-23".into(),
        reason: "tape from the corner".into(),
    };
    let mut rec = AsBuiltRecord::new(&t.project);
    let (c, snap) = verify_stud(&mut rec, &mut fr, &wall, centre, &who).expect("the stud is on the wall");
    assert_eq!((c.element_id.as_str(), c.element_kind.as_str()), (wall.as_str(), "wall"));
    assert!(matches!(c.kind, ChangeKind::FieldVerified { .. }));
    assert!(snap.walls.iter().any(|i| fr.walls.get(*i).is_some_and(|w| w.wall.id == wall)));
    let verified_at = |f: &framing::Framing| -> Vec<f64> {
        f.walls
            .iter()
            .find(|w| w.wall.id == wall)
            .map(|w| w.members.iter().filter(|m| m.status == Status::FieldVerified).map(|m| m.s_in).collect())
            .unwrap_or_default()
    };
    assert!(verified_at(&fr).iter().any(|s| (s - centre).abs() < 1.0 / 32.0), "the snap reports the measured stud: {:?}", verified_at(&fr));
    // A framing rebuilt from the prints gets it back from the record.
    let mut again = fresh();
    assert!(verified_at(&again).is_empty());
    let applied = apply_studs(&rec, &mut again, false);
    assert_eq!(applied.len(), 1);
    assert!(applied[0].1.is_ok());
    assert!(verified_at(&again).iter().any(|s| (s - centre).abs() < 1.0 / 32.0));
    // A stud off the wall changes nothing.
    let before = fr.clone();
    assert!(verify_stud(&mut rec, &mut fr, &wall, -5.0, &who).is_err());
    assert_eq!(fr, before);
    assert_eq!(rec.changes.len(), 1);
}

#[test]
fn wall_markups_change_the_wall() {
    let t = fixture_truth();
    let sh = plan_sheet(&t, "1");
    let w = t.plan_walls.iter().find(|w| w.level == "1").expect("a wall on level 1");
    let wmid = [(w.a_ft[0] + w.b_ft[0]) * 0.5, (w.a_ft[1] + w.b_ft[1]) * 0.5];
    let c = one(&t, sh, markup_at(sh, "cloud", "Partition not built", wmid, None), "C-1");
    assert_eq!((c.element_id.as_str(), c.element_kind.as_str(), &c.kind), (w.id.as_str(), "wall", &ChangeKind::Deleted));
    let rec = AsBuiltRecord { changes: vec![decided(c)], ..AsBuiltRecord::new(&t.project) };
    assert!(asbuilt_truth(&t, &rec).plan_walls.iter().all(|x| x.id != w.id));
    let c = one(&t, sh, markup_at(sh, "cloud", "Wall moved 1 foot north", wmid, None), "C-2");
    assert_eq!(c.element_id, w.id);
    let rec = AsBuiltRecord { changes: vec![decided(c)], ..AsBuiltRecord::new(&t.project) };
    let ab = asbuilt_truth(&t, &rec);
    let m = ab.plan_walls.iter().find(|x| x.id == w.id).expect("still there");
    assert!(dist(m.a_ft, [w.a_ft[0], w.a_ft[1] + 1.0]) < 0.1 && dist(m.b_ft, [w.b_ft[0], w.b_ft[1] + 1.0]) < 0.1);
}

#[test]
fn asbuilt_record_json_round_trip_full() {
    let mut rec = AsBuiltRecord::new("Project A");
    let changes = [
        ChangeKind::Moved { from_ft: [10.0, 20.0, 8.0], to_ft: [12.0, 20.0, 8.0] },
        ChangeKind::Rerouted { design_path_ft: [[5.0, 5.0, 8.0], [15.0, 5.0, 8.0]], asbuilt_path_ft: [[5.0, 7.0, 8.0], [15.0, 7.0, 8.0]] },
        ChangeKind::TypeChanged { design_type: "EMT".into(), asbuilt_type: "RGS".into() },
        ChangeKind::SizeChanged { design_size: "3/4\"".into(), asbuilt_size: "1\"".into() },
        ChangeKind::Added,
        ChangeKind::Deleted,
        ChangeKind::FieldVerified { at_ft: [25.0, 30.0, 4.0] },
        ChangeKind::HeightChanged { design_ft: 10.5, asbuilt_ft: 11.0 },
        ChangeKind::Note,
    ];
    let statuses = [ChangeStatus::Pending, ChangeStatus::FieldDecision, ChangeStatus::RfiAnswer, ChangeStatus::ChangeOrder, ChangeStatus::Rejected];
    for (i, kind) in changes.into_iter().enumerate() {
        let mut c = change(&format!("C-{i}"), &format!("e-{i}"), kind, statuses[i % 5], "1", [i as f64, 0.0]);
        c.notes = format!("Notes {i}");
        c.markup_id = (i % 2 == 0).then(|| format!("MK-{i}"));
        rec.add_change(c);
    }
    let json = serde_json::to_string_pretty(&rec).expect("json");
    let back: AsBuiltRecord = serde_json::from_str(&json).expect("json back");
    assert_eq!(rec, back);
}

#[test]
fn coordinate_transform_consistency() {
    let t = fixture_truth();
    let mut checked = 0;
    for sh in t.sheets.iter().filter(|s| s.level.is_some()) {
        let level = sh.level.as_deref().expect("a plan");
        for seg in t.segments.iter().filter(|s| s.level == level).take(5) {
            let bldg = mid(seg.path_ft);
            let back = paper_to_building(sh, building_to_paper(sh, bldg));
            assert!(dist(back, bldg) < 0.01, "sheet {}: {bldg:?} -> {back:?}", sh.number);
            checked += 1;
        }
    }
    assert!(checked >= 10, "{checked}");
}

#[test]
fn mixed_statuses_in_comparison() {
    let t = fixture_truth();
    let mut rec = AsBuiltRecord::new(&t.project);
    let statuses = [ChangeStatus::FieldDecision, ChangeStatus::Rejected, ChangeStatus::RfiAnswer, ChangeStatus::ChangeOrder, ChangeStatus::Pending];
    for (i, status) in statuses.iter().enumerate() {
        rec.add_change(change(&format!("C-{i}"), &format!("e-{i}"), ChangeKind::Note, *status, "1", [0.0, 0.0]));
    }
    let devs = compare_design_vs_asbuilt(&t, &rec);
    assert_eq!(devs.iter().filter_map(|d| d.change_id.as_deref()).collect::<Vec<_>>(), ["C-0", "C-2", "C-3", "C-4"]);
}
