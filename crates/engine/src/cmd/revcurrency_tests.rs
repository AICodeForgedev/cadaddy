//! Q6650: revision currency between disciplines. Synthetic sheets only: made-up numbers, marks
//! and revisions, in more than one layout (header combined or apart, columns in either order,
//! no header read, a cloud plotted as arcs or as one bulged polyline).

use super::*;
use crate::Session;
use crate::cmd::guided::{GuidedJob, JobGoal, SheetRegion};

fn t(v: &str, x: f64, y: f64, h: f64) -> TextItem {
    TextItem { value: v.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
}

/// One scallop from `a` to `b`, standing `out` off its chord (sign: which side), as a PDF plots
/// it: a curve flattened into points.
fn scallop(a: Vec2, b: Vec2, out: f64) -> Vec<Vec2> {
    let d = b - a;
    let len = d.len();
    let n = Vec2::new(d.y / len, -d.x / len);
    (0..=8)
        .map(|k| {
            let s = k as f64 / 8.0;
            let bump = (std::f64::consts::PI * s).sin() * out;
            Vec2::new(a.x + d.x * s + n.x * bump, a.y + d.y * s + n.y * bump)
        })
        .collect()
}

/// A rectangle clouded with scallops of about `chord`, each a curve of its own (outward on a
/// counter-clockwise walk).
fn cloud_pieces(min: [f64; 2], max: [f64; 2], chord: f64) -> Vec<Vec<Vec2>> {
    let corners = [Vec2::new(min[0], min[1]), Vec2::new(max[0], min[1]), Vec2::new(max[0], max[1]), Vec2::new(min[0], max[1])];
    let mut out = Vec::new();
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        let n = ((b - a).len() / chord).round().max(1.0) as usize;
        for k in 0..n {
            let p = |j: usize| Vec2::new(a.x + (b.x - a.x) * j as f64 / n as f64, a.y + (b.y - a.y) * j as f64 / n as f64);
            out.push(scallop(p(k), p(k + 1), 0.3 * chord));
        }
    }
    out
}

fn open(pts: Vec<Vec2>) -> Curve {
    Curve { pts, closed: false, bulges: 0 }
}

#[test]
fn a_revision_table_reads_under_a_header_plotted_as_one_text() {
    let texts = vec![
        t("REV          DESCRIPTION            DATE", 0.0, 100.0, 2.0),
        t("0", 2.0, 94.0, 2.0),
        t("Issued for Permit", 8.0, 94.0, 2.0),
        t("05/02/24", 50.0, 94.0, 2.0),
        // The title block's own date and the set's issue stamp are no rows.
        t("DATE:", 2.0, 20.0, 2.0),
        t("05/02/24", 30.0, 20.0, 2.0),
        t("ISSUED FOR CONSTRUCTION", 5.0, 40.0, 4.0),
        t("12/01/24", 10.0, 35.0, 4.0),
    ];
    let rows = revision_table(&texts);
    assert_eq!(rows, vec![Revision { mark: "0".into(), description: "Issued for Permit".into(), date: "2024-05-02".into() }]);
}

#[test]
fn a_revision_table_reads_with_its_columns_in_another_order_and_rows_over_the_header() {
    // Date first, mark last; the rows grow upward over the header, each row one text.
    let texts = vec![
        t("DATE", 0.0, 10.0, 1.5),
        t("REVISION", 20.0, 10.0, 1.5),
        t("NO.", 60.0, 10.0, 1.5),
        t("2026-01-05", 0.0, 14.0, 1.5),
        t("BULLETIN 2", 20.0, 14.0, 1.5),
        t("B", 60.0, 14.0, 1.5),
        t("2025-11-20", 0.0, 18.0, 1.5),
        t("ADDENDUM 1", 20.0, 18.0, 1.5),
        t("A", 60.0, 18.0, 1.5),
    ];
    let rows = revision_table(&texts);
    let got: Vec<(&str, &str, &str)> = rows.iter().map(|r| (r.mark.as_str(), r.description.as_str(), r.date.as_str())).collect();
    assert_eq!(got, vec![("A", "ADDENDUM 1", "2025-11-20"), ("B", "BULLETIN 2", "2026-01-05")]);
    // A row plotted as one text with its fields apart reads the same.
    let one = vec![t("DATE   DESCRIPTION   REV", 0.0, 10.0, 1.5), t("2025-11-20   ADDENDUM 1   A", 0.0, 6.0, 1.5)];
    assert_eq!(revision_table(&one).first().map(|r| (r.mark.as_str(), r.date.as_str())), Some(("A", "2025-11-20")));
}

#[test]
fn rows_with_no_header_read_only_when_two_line_up() {
    let two = vec![
        t("0", 0.0, 20.0, 2.0),
        t("FIRST ISSUE", 6.0, 20.0, 2.0),
        t("01/02/25", 40.0, 20.0, 2.0),
        t("1", 0.0, 16.0, 2.0),
        t("ASI 03", 6.0, 16.0, 2.0),
        t("05/06/25", 40.0, 16.0, 2.0),
    ];
    assert_eq!(revision_table(&two).iter().map(|r| r.mark.as_str()).collect::<Vec<_>>(), vec!["0", "1"]);
    // One such line alone is no table (a stamp, a phone number, a date by a label).
    assert!(revision_table(&two[..3]).is_empty());
}

#[test]
fn a_cloud_of_scallop_arcs_is_found_and_an_insulation_squiggle_or_a_box_is_not() {
    let curves: Vec<Curve> = cloud_pieces([100.0, 100.0], [220.0, 180.0], 12.0).into_iter().map(open).collect();
    let found = clouds(&curves, 3000.0);
    assert_eq!(found.len(), 1, "{found:?}");
    let c = found[0];
    assert!((c.min.x - 100.0).abs() < 6.0 && (c.max.y - 180.0).abs() < 6.0, "{c:?}");
    // Broken where a leader or a masked note crosses it (five scallops left out): still a cloud.
    let mut broken = cloud_pieces([100.0, 100.0], [400.0, 300.0], 12.0);
    broken.drain(10..15);
    assert_eq!(clouds(&broken.into_iter().map(open).collect::<Vec<_>>(), 3000.0).len(), 1);
    // A run of arcs along a wall (batt insulation) never closes.
    let squiggle: Vec<Curve> = (0..30).map(|k| open(scallop(Vec2::new(k as f64 * 12.0, 0.0), Vec2::new((k + 1) as f64 * 12.0, 0.0), 4.0))).collect();
    assert!(clouds(&squiggle, 3000.0).is_empty());
    // A plain rectangle is no cloud.
    let rect = Curve { pts: vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 0.0), Vec2::new(50.0, 30.0), Vec2::new(0.0, 30.0)], closed: true, bulges: 0 };
    assert!(clouds(&[rect], 3000.0).is_empty());
    // A CAD cloud: one closed polyline of bulged segments.
    let pts: Vec<Vec2> = (0..12).map(|k| Vec2::new((k as f64).cos() * 40.0, (k as f64).sin() * 40.0)).collect();
    assert_eq!(clouds(&[Curve { pts, closed: true, bulges: 12 }], 3000.0).len(), 1);
    // One closed outline of scallops (a PDF path in one piece).
    let mut ring: Vec<Vec2> = Vec::new();
    for piece in cloud_pieces([0.0, 0.0], [120.0, 96.0], 12.0) {
        ring.extend(piece.into_iter().skip(1));
    }
    assert_eq!(clouds(&[Curve { pts: ring, closed: true, bulges: 0 }], 3000.0).len(), 1);
}

#[test]
fn equipment_marks_read_as_one_word_or_stacked_and_only_by_the_trades_prefixes() {
    let texts = vec![
        t("CU", 0.0, 10.0, 2.0),
        t("3", 1.0, 7.0, 2.0),
        t("RTU-12", 50.0, 10.0, 2.0),
        t("W-1", 80.0, 10.0, 2.0),
        t("EF2", 100.0, 10.0, 2.0),
        t("CORRIDOR", 120.0, 10.0, 2.0),
    ];
    let got: Vec<String> = marks(&texts, rules()).into_iter().map(|m| m.0).collect();
    assert_eq!(got, vec!["CU-3", "RTU-12", "EF-2"]);
}

#[test]
fn two_sheets_align_by_the_labels_they_share_and_not_when_they_disagree() {
    let grid = ["1", "2", "3", "4", "5", "A", "B", "C"];
    let a: Vec<TextItem> = grid.iter().enumerate().map(|(i, g)| t(g, i as f64 * 100.0, (i % 3) as f64 * 70.0, 2.0)).collect();
    let b: Vec<TextItem> = a.iter().map(|x| t(&x.value, x.at.x + 5120.0, x.at.y + 30.0, 2.0)).collect();
    let off = alignment(&a, &b, 2.0, 4).expect("aligned");
    assert!((off.x - 5120.0).abs() < 1e-6 && (off.y - 30.0).abs() < 1e-6, "{off:?}");
    let scattered: Vec<TextItem> = a.iter().enumerate().map(|(i, x)| t(&x.value, x.at.x + i as f64 * 97.0, x.at.y - i as f64 * 61.0, 2.0)).collect();
    assert!(alignment(&a, &scattered, 2.0, 4).is_none());
}

// ─── Through the job: the build raises it, rfi.read lists it ────────────────

/// Where each sheet sits in the job's drawing.
const M_X: f64 = 0.0;
const A_X: f64 = 5000.0;
/// The architect's plan sits this much further on its sheet than the mechanical one.
const A_SHIFT: [f64; 2] = [120.0, 30.0];

fn sheet_region(number: &str, discipline: &str, x: f64) -> SheetRegion {
    SheetRegion {
        number: number.into(),
        level: "R".into(),
        discipline: discipline.into(),
        kind: "plan".into(),
        rect: [x, 0.0, x + 4000.0, 2800.0],
        area: Some([x + 100.0, 50.0, x + 3600.0, 2750.0]),
        title_block: Some([x + 3650.0, 50.0, x + 3950.0, 2750.0]),
        not_built: String::new(),
        levels: Vec::new(),
        view: None,
    }
}

fn text(s: &mut Session, v: &str, at: [f64; 2]) {
    s.execute("text", &json!({"at": at, "text": v, "height": 9})).unwrap();
}

/// A sheet's revision table: the header and its rows (mark, description, date).
fn table(s: &mut Session, x: f64, rows: &[(&str, &str, &str)]) {
    text(s, "REV          DESCRIPTION            DATE", [x + 3660.0, 760.0]);
    for (i, (m, d, date)) in rows.iter().enumerate() {
        let y = 735.0 - 17.0 * i as f64;
        text(s, m, [x + 3668.0, y]);
        text(s, d, [x + 3690.0, y]);
        text(s, date, [x + 3840.0, y]);
    }
}

/// The plan's grid labels, at the same places of the plan on every sheet.
fn grid(s: &mut Session, origin: [f64; 2]) {
    for (i, g) in ["1", "2", "3", "4", "5", "6"].iter().enumerate() {
        text(s, g, [origin[0] + 300.0 + 500.0 * i as f64, origin[1] + 2200.0]);
    }
    for (i, g) in ["A", "B", "C", "D"].iter().enumerate() {
        text(s, g, [origin[0] + 200.0, origin[1] + 2000.0 - 400.0 * i as f64]);
    }
}

struct Case {
    /// The architect's revision rows.
    a_rows: Vec<(&'static str, &'static str, &'static str)>,
    /// Where the architect's sheet draws RTU-1, on its plan (None: it draws no mark).
    a_rtu1: Option<[f64; 2]>,
    /// What else the architect's plan prints.
    a_notes: Vec<&'static str>,
    /// The cloud as one bulged polyline instead of arcs.
    cad_cloud: bool,
}

/// The job's sheets: a mechanical roof plan at revision 1 (ASI 07) clouding RTU-1 at its new
/// place (its delta on the cloud's corner) and RTU-2 left as it was; an architect's roof plan
/// as the case gives it. The build's check, then rfi.read.
fn run(case: Case) -> (Value, Value) {
    let mut s = Session::new();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {"units_per_foot": 12.0, "levels": [
            {"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9},
            {"name": "R", "elevation_ft": 10, "height_ft": 4, "ceiling_ft": 4}
        ]}}),
    )
    .unwrap();
    // The mechanical sheet.
    grid(&mut s, [M_X, 0.0]);
    table(&mut s, M_X, &[("0", "ISSUED FOR PERMIT", "05/02/24"), ("1", "ASI 07", "11/18/24")]);
    text(&mut s, "RTU", [M_X + 1500.0, 1010.0]);
    text(&mut s, "1", [M_X + 1504.0, 995.0]);
    text(&mut s, "RTU-2", [M_X + 2500.0, 1000.0]);
    let (cmin, cmax) = ([M_X + 1400.0, 900.0], [M_X + 1700.0, 1150.0]);
    if case.cad_cloud {
        let vertices: Vec<Value> = (0..16)
            .map(|k| {
                let a = k as f64 / 16.0 * std::f64::consts::TAU;
                json!({"p": [M_X + 1550.0 + 150.0 * a.cos(), 1025.0 + 125.0 * a.sin()], "bulge": 0.5})
            })
            .collect();
        s.execute("pline", &json!({"vertices": vertices, "closed": true})).unwrap();
    } else {
        for piece in cloud_pieces(cmin, cmax, 12.0) {
            let pts: Vec<[f64; 2]> = piece.iter().map(|p| [p.x, p.y]).collect();
            s.execute("pline", &json!({"points": pts})).unwrap();
        }
    }
    text(&mut s, "1", [cmax[0] + 8.0, cmax[1] + 4.0]);
    // The architect's sheet: the same plan, placed a little further on its sheet.
    let ao = [A_X + A_SHIFT[0], A_SHIFT[1]];
    grid(&mut s, ao);
    table(&mut s, A_X, &case.a_rows);
    if let Some(p) = case.a_rtu1 {
        text(&mut s, "RTU-1", [ao[0] + p[0], ao[1] + p[1]]);
    }
    text(&mut s, "RTU-2", [ao[0] + 2500.0, ao[1] + 1000.0]);
    for (i, n) in case.a_notes.iter().enumerate() {
        text(&mut s, n, [ao[0] + 2900.0, ao[1] + 600.0 - 20.0 * i as f64]);
    }
    let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
    job.sheet_regions = vec![sheet_region("M-401", "M", M_X), sheet_region("A-301", "A", A_X)];
    let audit = crate::cmd::guided::revision_currency(&mut s, &job).unwrap();
    let read = s.execute("rfi.read", &json!({})).unwrap();
    (audit, read)
}

fn ours(read: &Value) -> Vec<Value> {
    read["items"].as_array().into_iter().flatten().filter(|i| i["topic"].as_str() == Some(TOPIC)).cloned().collect()
}

#[test]
fn a_sheet_left_at_its_first_issue_showing_the_unit_at_its_old_place_is_asked_about() {
    let (audit, read) =
        run(Case { a_rows: vec![("0", "ISSUED FOR PERMIT", "05/02/24")], a_rtu1: Some([1100.0, 1000.0]), a_notes: vec![], cad_cloud: false });
    let items = ours(&read);
    assert_eq!(items.len(), 1, "{audit:#}\n{read:#}");
    let it = &items[0];
    assert_eq!(it["kind"], json!("conflict"), "{it:#}");
    assert_eq!(it["about"], json!("A-301: not revised for revision 1 (ASI 07, 2024-11-18)"), "{it:#}");
    let issue = it["issue"].as_str().unwrap_or("");
    assert!(issue.contains("M-401 carries revision 1 (ASI 07, 2024-11-18), which moves or adds RTU-1"), "{issue}");
    assert!(issue.contains("A-301 (latest revision 0 (ISSUED FOR PERMIT, 2024-05-02)) is not revised for it"), "{issue}");
    assert!(issue.contains("shows RTU-1 at the old place (RTU-1 32.9 ft off)") && !issue.contains("RTU-2"), "{issue}");
    assert_eq!(it["sheets"], json!(["M-401", "A-301"]), "{it:#}");
    assert_eq!(audit["clouds"], json!(1), "{audit:#}");
    // Run again: the item is replaced, not added twice.
    let (_, again) =
        run(Case { a_rows: vec![("0", "ISSUED FOR PERMIT", "05/02/24")], a_rtu1: Some([1100.0, 1000.0]), a_notes: vec![], cad_cloud: false });
    assert_eq!(ours(&again).len(), 1);
}

#[test]
fn a_sheet_with_no_mark_that_draws_the_units_curbs_is_asked_about_too() {
    let (audit, read) = run(Case {
        a_rows: vec![("0", "ISSUED FOR PERMIT", "05/02/24")],
        a_rtu1: None,
        a_notes: vec!["ROOF CURBS BY MECHANICAL", "WALK PAD"],
        cad_cloud: true,
    });
    let items = ours(&read);
    assert_eq!(items.len(), 1, "{audit:#}\n{read:#}");
    let issue = items[0]["issue"].as_str().unwrap_or("");
    assert!(issue.contains("draws what carries RTU-1 (curb, walk pad)"), "{issue}");
}

#[test]
fn a_sheet_revised_too_or_showing_the_same_place_or_drawing_nothing_of_it_is_not_asked_about() {
    // Revised for the same revision.
    let (_, read) = run(Case {
        a_rows: vec![("0", "ISSUED FOR PERMIT", "05/02/24"), ("1", "ASI 07", "11/18/24")],
        a_rtu1: Some([1100.0, 1000.0]),
        a_notes: vec!["ROOF CURBS"],
        cad_cloud: false,
    });
    assert!(ours(&read).is_empty(), "{read:#}");
    // At its first issue, but the unit where the revision puts it.
    let (_, read) =
        run(Case { a_rows: vec![("0", "ISSUED FOR PERMIT", "05/02/24")], a_rtu1: Some([1500.0, 1010.0]), a_notes: vec![], cad_cloud: false });
    assert!(ours(&read).is_empty(), "{read:#}");
    // At its first issue, drawing neither the unit nor anything carrying it.
    let (_, read) =
        run(Case { a_rows: vec![("0", "ISSUED FOR PERMIT", "05/02/24")], a_rtu1: None, a_notes: vec!["GENERAL NOTES"], cad_cloud: false });
    assert!(ours(&read).is_empty(), "{read:#}");
}
