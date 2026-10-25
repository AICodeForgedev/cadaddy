//! The print set's links on a made-up electrical set drawn the way a print comes in (one pen
//! layer, no CALLOUT layer): a first-floor plan split over two sheets by a match line, an
//! enlarged core plan at twice the scale on its own sheet (devices drawn inside its boundary on
//! both sheets), a detail sheet printing a mounting height, and one broken reference.

use cadaddy_doc::{
    Attrib, AttribModes, AttribProps, Block, Circle, Common, Drawing, EntityKind, HAlign, Insert, Line, LwPolyline, Space, Text, VAlign,
};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use cadaddy_mepfix::printlinks::{self, LinkPage, SetSheet, SheetLinks};

const PEN: &str = "PEN-1";

fn common() -> Common {
    Common { layer: PEN.into(), ..Common::default() }
}

fn text_ent(v: &str, x: f64, y: f64, h: f64, rotation: f64) -> Text {
    Text {
        insert: Vec3::new(x, y, 0.0),
        align_pt: None,
        height: h,
        value: v.into(),
        rotation,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::default(),
        valign: VAlign::default(),
    }
}

struct Sheet {
    d: Drawing,
}

impl Sheet {
    fn new() -> Sheet {
        let mut d = Drawing::default();
        let mut b = Block::new("PDF-SYM-1");
        // A duplex receptacle: a circle and its two bars.
        b.entities.push(cadaddy_doc::Entity {
            handle: d.new_handle(),
            common: common(),
            kind: EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 0.1 }),
        });
        for y in [-0.04, 0.04] {
            let kind = EntityKind::Line(Line { a: Vec3::new(-0.15, y, 0.0), b: Vec3::new(0.15, y, 0.0) });
            b.entities.push(cadaddy_doc::Entity { handle: d.new_handle(), common: common(), kind });
        }
        d.blocks.insert("PDF-SYM-1".into(), std::sync::Arc::new(b));
        let mut s = Sheet { d };
        // The sheet's border (36 x 24 paper inches).
        s.rect([0.0, 0.0, 36.0, 24.0]);
        s
    }
    fn add(&mut self, k: EntityKind) {
        self.d.add(&Space::Model, common(), k).ok();
    }
    fn line(&mut self, a: [f64; 2], b: [f64; 2]) {
        self.add(EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }));
    }
    fn rect(&mut self, r: [f64; 4]) {
        let pts = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]];
        let vertices = pts.iter().map(|p| PolyVertex { p: Vec2::new(p[0], p[1]), bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
        self.add(EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }));
    }
    fn circle(&mut self, c: [f64; 2], r: f64) {
        self.add(EntityKind::Circle(Circle { center: Vec3::new(c[0], c[1], 0.0), radius: r }));
    }
    /// Lettering placed by its middle, as a print's text sits (baseline, left end).
    fn text_mid(&mut self, v: &str, mid: [f64; 2], h: f64) {
        let w = 0.35 * h * v.chars().count() as f64;
        self.add(EntityKind::Text(text_ent(v, mid[0] - w, mid[1] - h / 2.0, h, 0.0)));
    }
    fn text(&mut self, v: &str, at: [f64; 2], h: f64, rotation: f64) {
        self.add(EntityKind::Text(text_ent(v, at[0], at[1], h, rotation)));
    }
    /// A callout bubble: detail over sheet, split by a divider.
    fn bubble(&mut self, c: [f64; 2], detail: &str, sheet: &str) {
        let r = 0.35;
        self.circle(c, r);
        self.line([c[0] - r, c[1]], [c[0] + r, c[1]]);
        self.text_mid(detail, [c[0], c[1] + 0.17], 0.12);
        self.text_mid(sheet, [c[0], c[1] - 0.17], 0.12);
    }
    /// Grid lines with a bubble at each end: x lines (label, x) and y lines (label, y).
    fn grid(&mut self, xs: &[(&str, f64)], ys: &[(&str, f64)], span_x: [f64; 2], span_y: [f64; 2]) {
        let r = 0.3;
        for (l, x) in xs {
            self.line([*x, span_y[0]], [*x, span_y[1]]);
            for y in [span_y[0] - r, span_y[1] + r] {
                self.circle([*x, y], r);
                self.text_mid(l, [*x, y], 0.15);
            }
        }
        for (l, y) in ys {
            self.line([span_x[0], *y], [span_x[1], *y]);
            for x in [span_x[0] - r, span_x[1] + r] {
                self.circle([x, *y], r);
                self.text_mid(l, [x, *y], 0.15);
            }
        }
    }
    /// A device symbol as the import makes it: a `PDF-SYM-` insert with its mark as TAG.
    fn device(&mut self, tag: &str, at: [f64; 2]) {
        let attrib = Attrib {
            tag: "TAG".into(),
            text: text_ent(tag, at[0] + 0.3, at[1] - 0.05, 0.1, 0.0),
            invisible: false,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        };
        let ins = Insert {
            block: "PDF-SYM-1".into(),
            insert: Vec3::new(at[0], at[1], 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: vec![attrib],
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        };
        self.add(EntityKind::Insert(ins));
    }
    /// A view title with its number bubble and scale line.
    fn view_title(&mut self, number: &str, sheet: &str, title: &str, scale: &str, at: [f64; 2]) {
        self.bubble([at[0] - 0.8, at[1] + 0.12], number, sheet);
        self.text(title, at, 0.25, 0.0);
        self.text(scale, [at[0], at[1] - 0.3], 0.12, 0.0);
    }
}

fn set_sheet(number: &str, title: &str, ty: &str, level: &str) -> SetSheet {
    SetSheet {
        number: number.into(),
        title: title.into(),
        discipline: number.chars().take_while(char::is_ascii_alphabetic).collect(),
        sheet_type: ty.into(),
        level: level.into(),
    }
}

/// E-1: the west half of the first floor, the enlarged core boxed with a callout to 1/E-3,
/// a receptacle pointing at detail 2/E-4, a broken bubble to E-9, and the match line to E-2.
fn e1() -> Drawing {
    let mut s = Sheet::new();
    s.grid(&[("1", 4.0), ("2", 8.0), ("3", 12.0), ("4", 16.0)], &[("A", 4.0), ("B", 8.0), ("C", 12.0)], [2.0, 17.0], [2.0, 14.0]);
    // The enlarged area's boundary and its callout, with a leader to the box's corner.
    s.rect([7.5, 3.5, 16.5, 12.5]);
    let c = [19.0, 15.0];
    s.bubble(c, "1", "E-3");
    let k = 0.35 / 2f64.sqrt();
    s.line([c[0] - k, c[1] - k], [16.5, 12.5]);
    // The overall plan's own copy of the core's devices (the enlarged sheet is the authority).
    for p in [[9.0, 5.0], [11.0, 9.0], [15.0, 11.0]] {
        s.device("A", p);
    }
    for p in [[5.0, 5.0], [5.0, 9.0], [5.0, 13.0], [18.0, 3.0], [17.5, 13.0]] {
        s.device("A", p);
    }
    s.device("F", [13.0, 15.5]);
    s.device("F", [6.5, 15.5]);
    s.bubble([14.2, 16.6], "2", "E-4");
    s.bubble([3.0, 16.0], "5", "E-9");
    s.line([18.5, 1.0], [18.5, 15.0]);
    s.text("MATCH LINE - SEE SHEET E-2", [18.65, 6.0], 0.1, std::f64::consts::FRAC_PI_2);
    s.text("REFER TO ARCHITECTURAL DRAWINGS FOR EXACT LOCATIONS.", [1.0, 22.0], 0.1, 0.0);
    s.text("SEE SHEET E-4 FOR MOUNTING HEIGHTS", [1.0, 21.6], 0.1, 0.0);
    s.view_title("1", "E-1", "FIRST FLOOR POWER PLAN - AREA A", "SCALE: 1/8\" = 1'-0\"", [4.0, 18.5]);
    s.d
}

/// E-2: the east half, 14 units to the left of where E-1 would draw it.
fn e2() -> Drawing {
    let mut s = Sheet::new();
    s.grid(&[("4", 2.0), ("5", 6.0), ("6", 10.0), ("7", 14.0)], &[("A", 4.0), ("B", 8.0), ("C", 12.0)], [1.0, 16.0], [2.0, 14.0]);
    s.line([4.5, 1.0], [4.5, 15.0]);
    s.text("MATCH LINE - SEE SHEET E-1", [4.35, 6.0], 0.1, std::f64::consts::FRAC_PI_2);
    for p in [[8.0, 5.0], [12.0, 9.0], [8.0, 11.0], [14.0, 5.0]] {
        s.device("A", p);
    }
    s.view_title("1", "E-2", "FIRST FLOOR POWER PLAN - AREA B", "SCALE: 1/8\" = 1'-0\"", [4.0, 18.5]);
    s.d
}

/// E-3: the enlarged core plan at twice E-1's scale, its grid spaced 8 where E-1's is 4.
fn e3() -> Drawing {
    let mut s = Sheet::new();
    s.grid(&[("2", 4.0), ("3", 12.0), ("4", 20.0)], &[("A", 4.0), ("B", 12.0), ("C", 20.0)], [2.5, 21.5], [2.5, 21.5]);
    // The core's rooms.
    for r in [[4.0, 4.0, 12.0, 12.0], [12.0, 4.0, 20.0, 12.0], [4.0, 12.0, 12.0, 20.0], [12.0, 12.0, 20.0, 20.0]] {
        s.rect(r);
    }
    for p in [[6.0, 6.0], [10.0, 14.0], [14.0, 10.0], [18.0, 19.0], [20.5, 8.0]] {
        s.device("A", p);
    }
    s.view_title("1", "E-3", "ENLARGED FIRST FLOOR CORE PLAN", "SCALE: 1/4\" = 1'-0\"", [4.0, 1.2]);
    s.d
}

/// E-4: details; detail 2 prints the receptacle's mounting height.
fn e4() -> Drawing {
    let mut s = Sheet::new();
    s.rect([2.0, 4.0, 10.0, 12.0]);
    s.line([4.0, 6.0], [8.0, 6.0]);
    s.line([4.0, 6.0], [4.0, 10.0]);
    s.text("MOUNT AT 18\" AFF", [3.0, 8.0], 0.1, 0.0);
    s.view_title("2", "E-4", "RECEPTACLE MOUNTING DETAIL", "SCALE: NTS", [3.0, 3.0]);
    s.d
}

fn the_set() -> (Vec<SetSheet>, Vec<LinkPage>) {
    let sheets = vec![
        set_sheet("E-0", "ELECTRICAL SYMBOLS AND SCHEDULES", "schedule", ""),
        set_sheet("E-1", "FIRST FLOOR POWER PLAN - AREA A", "plan", "1"),
        set_sheet("E-2", "FIRST FLOOR POWER PLAN - AREA B", "plan", "1"),
        set_sheet("E-3", "ELECTRICAL ENLARGED PLANS", "enlarged_plan", ""),
        set_sheet("E-4", "ELECTRICAL DETAILS", "detail", ""),
        set_sheet("A-101", "FIRST FLOOR PLAN", "plan", "1"),
    ];
    let drawings = [("E-1", e1()), ("E-2", e2()), ("E-3", e3()), ("E-4", e4())];
    let pages = drawings
        .iter()
        .map(|(n, d)| LinkPage { sheet: sheets.iter().find(|s| s.number == *n).cloned().unwrap_or_default(), read: printlinks::read_page(d, n) })
        .collect();
    (sheets, pages)
}

fn links() -> SheetLinks {
    let (sheets, pages) = the_set();
    printlinks::link_set(&sheets, &pages)
}

#[test]
fn every_callout_resolves_but_the_broken_one() {
    let l = links();
    let got: Vec<(String, String, String, String, bool)> =
        l.callouts.iter().map(|c| (c.on_sheet.clone(), c.kind.clone(), c.detail.clone(), c.target_sheet.clone(), c.resolved)).collect();
    let want: Vec<(String, String, String, String, bool)> = [
        ("E-1", "enlarged_plan", "1", "E-3", true),
        ("E-1", "detail_bubble", "2", "E-4", true),
        ("E-1", "detail_bubble", "5", "E-9", false),
        ("E-1", "discipline_ref", "", "A", true),
        ("E-1", "sheet_ref", "", "E-4", true),
        ("E-1", "match_line", "", "E-2", true),
        ("E-2", "match_line", "", "E-1", true),
    ]
    .iter()
    .map(|(a, b, c, d, e)| (a.to_string(), b.to_string(), c.to_string(), d.to_string(), *e))
    .collect();
    let mut got_sorted = got.clone();
    got_sorted.sort();
    let mut want_sorted = want.clone();
    want_sorted.sort();
    assert_eq!(got_sorted, want_sorted, "{got:#?}");
    assert_eq!(l.resolved, 6);
    assert_eq!(l.resolved_pct, 0.857);
    assert_eq!(l.unresolved.len(), 1);
    assert_eq!((l.unresolved[0].on_sheet.as_str(), l.unresolved[0].target_sheet.as_str(), l.unresolved[0].detail.as_str()), ("E-1", "E-9", "5"));
    // The details the bubbles name are on their sheets.
    let detail = l.callouts.iter().find(|c| c.detail == "2").map(|c| c.detail_found);
    assert_eq!(detail, Some(Some(true)));
    // View-number bubbles (each view title's own) are not links.
    assert!(l.callouts.iter().all(|c| c.on_sheet != "E-3" && c.on_sheet != "E-4"), "{:#?}", l.callouts);
    assert_eq!(l.sheets.len(), 6);
}

#[test]
fn the_enlarged_plan_registers_into_its_boundary_and_its_devices_count_once() {
    let l = links();
    assert_eq!(l.enlarged.len(), 1, "{:#?}", l.enlarged);
    let e = &l.enlarged[0];
    assert_eq!((e.on_sheet.as_str(), e.target_sheet.as_str(), e.detail.as_str(), e.level.as_str()), ("E-1", "E-3", "1", "1"));
    assert!(e.registered);
    assert_eq!(e.method, "grids");
    assert_eq!(e.scale, Some(2.0));
    assert_eq!(e.boundary, Some([7.5, 3.5, 16.5, 12.5]));
    assert_eq!(e.devices, 5);
    assert_eq!(e.overall_ignored, 3);
    assert_eq!(l.enlarged_registered, 1);
    assert_eq!(l.double_counted, 0);
    // Level 1: E-1's 7 outside the core, E-2's 4, the enlarged plan's 5.
    assert_eq!(l.levels.len(), 1, "{:#?}", l.levels);
    assert_eq!(l.levels[0].devices, 16);
    assert_eq!(l.levels[0].sheets, vec!["E-1".to_string(), "E-2".to_string()]);
}

#[test]
fn the_match_line_stitches_one_level() {
    let l = links();
    assert_eq!(l.match_lines.len(), 1, "{:#?}", l.match_lines);
    let m = &l.match_lines[0];
    assert_eq!(m.sheets, ["E-1".to_string(), "E-2".to_string()]);
    assert_eq!(m.level, "1");
    assert!(m.stitched);
    assert_eq!(m.offset, Some([14.0, 0.0]));
    assert!(l.levels[0].stitched);
}

#[test]
fn the_detail_height_lands_on_the_device_its_bubble_points_from() {
    let l = links();
    assert_eq!(l.facts.len(), 1, "{:#?}", l.facts);
    let f = &l.facts[0];
    assert_eq!((f.on_sheet.as_str(), f.target_sheet.as_str(), f.detail.as_str(), f.kind.as_str()), ("E-1", "E-4", "2", "mounting_height"));
    assert_eq!(f.value_ft, 1.5);
    assert_eq!(f.applied_to.len(), 1);
    assert_eq!(f.applied_to[0]["tag"], "F");
}

/// Without the enlarged sheet's registration the overall plan's copy would count: a set whose
/// enlarged plan names no grid and draws no boundary is reported unregistered, never silently
/// added twice.
#[test]
fn an_enlarged_plan_that_cannot_be_placed_is_reported_unregistered() {
    let (sheets, mut pages) = the_set();
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-3") {
        for v in &mut p.read.views {
            v.grid_x.clear();
            v.grid_y.clear();
        }
    }
    for p in pages.iter_mut().filter(|p| p.sheet.number == "E-1") {
        p.read.boxes.clear();
        for c in &mut p.read.callouts {
            c.boundary = None;
        }
    }
    let l = printlinks::link_set(&sheets, &pages);
    let e = &l.enlarged[0];
    assert!(!e.registered, "{e:#?}");
    assert_eq!(e.on_sheet, "E-1");
    assert_eq!(l.enlarged_registered, 0);
}

/// The same set printed to PDF and imported page by page, as `printset.links` reads a job.
fn printed_set() -> (Vec<SetSheet>, Vec<LinkPage>) {
    let (sheets, _) = the_set();
    let drawings = [("E-1", e1()), ("E-2", e2()), ("E-3", e3()), ("E-4", e4())];
    let pages = drawings
        .iter()
        .map(|(n, d)| {
            let opts =
                serde_json::json!({"width": 914.4, "height": 609.6, "window": [[0.0, 0.0], [36.0, 24.0]], "fit": true, "margins": [0, 0, 0, 0]});
            let pdf = cadaddy_io::pdf::plot(d, &Space::Model, &opts).unwrap_or_else(|e| panic!("{n}: {e}"));
            let (page, _) = cadaddy_io::pdf_read::read_json(&pdf, &serde_json::json!({"page": 1, "images": false, "fills": "merge"}))
                .unwrap_or_else(|e| panic!("{n}: {e}"));
            LinkPage { sheet: sheets.iter().find(|s| s.number == *n).cloned().unwrap_or_default(), read: printlinks::read_page(&page, n) }
        })
        .collect();
    (sheets, pages)
}

/// Printed and imported, the pages come back at their own scale notes (E-1 and E-2 at 1/8",
/// E-3 at 1/4": building inches on every sheet), the bubbles' circles taken for lettering by the
/// import and the text doubled (hidden text and strokes read back): the links come out the same.
#[test]
fn a_printed_set_links_the_same_way() {
    let (sheets, pages) = printed_set();
    let l = printlinks::link_set(&sheets, &pages);
    let mut got: Vec<(String, String, String, String, bool)> =
        l.callouts.iter().map(|c| (c.on_sheet.clone(), c.kind.clone(), c.detail.clone(), c.target_sheet.clone(), c.resolved)).collect();
    got.sort();
    let mut want: Vec<(String, String, String, String, bool)> = [
        ("E-1", "detail_bubble", "2", "E-4", true),
        ("E-1", "detail_bubble", "5", "E-9", false),
        ("E-1", "discipline_ref", "", "A", true),
        ("E-1", "enlarged_plan", "1", "E-3", true),
        ("E-1", "match_line", "", "E-2", true),
        ("E-1", "sheet_ref", "", "E-4", true),
        ("E-2", "match_line", "", "E-1", true),
    ]
    .iter()
    .map(|(a, b, c, d, e)| (a.to_string(), b.to_string(), c.to_string(), d.to_string(), *e))
    .collect();
    want.sort();
    assert_eq!(got, want);
    assert_eq!(l.unresolved.len(), 1);
    let e = &l.enlarged[..];
    assert_eq!(e.len(), 1, "{e:#?}");
    let e = &e[0];
    assert_eq!((e.on_sheet.as_str(), e.target_sheet.as_str(), e.detail.as_str(), e.level.as_str()), ("E-1", "E-3", "1", "1"));
    assert!(e.registered);
    assert_eq!(e.method, "grids");
    // Both sheets in building inches: one to one.
    assert_eq!(e.scale, Some(1.0));
    assert_eq!(e.boundary, Some([7.5 * 96.0, 3.5 * 96.0, 16.5 * 96.0, 12.5 * 96.0]));
    assert_eq!((e.devices, e.overall_ignored), (5, 3));
    assert_eq!(l.double_counted, 0);
    assert_eq!(l.levels.len(), 1);
    assert_eq!(l.levels[0].devices, 16);
    assert!(l.levels[0].stitched);
    assert_eq!(l.match_lines.len(), 1);
    assert_eq!(l.match_lines[0].offset, Some([14.0 * 96.0, 0.0]));
    assert_eq!(l.facts.len(), 1);
    assert_eq!(l.facts[0].value_ft, 1.5);
    assert_eq!(l.facts[0].applied_to.first().map(|a| a["tag"].clone()), Some(serde_json::json!("F")));
}
