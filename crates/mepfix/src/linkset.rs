//! A made-up electrical print set for the sheet-map tests ([`crate::printlinks`]), drawn the way
//! a print comes in (one pen layer, no CALLOUT layer): a first-floor power plan split over two
//! sheets by a match line, an enlarged core plan at twice the scale on its own sheet (devices
//! drawn inside its boundary on both sheets), a detail sheet printing a mounting height, a
//! broken reference, and a title block on every sheet. Paper inches, a 36 x 24 sheet.
//!
//! What the links must come out as: every callout resolved but the bubble to E-9; the enlarged
//! plan 1/E-3 registered into its boundary on E-1 by the grid lines both sheets label, its five
//! devices counted and E-1's three inside the boundary left out; E-1 and E-2 stitched into one
//! first floor by their match lines; detail 2/E-4's 18" mounting height on the receptacle its
//! bubble points from.

use crate::printlinks::SetSheet;
use cadaddy_doc::{
    Attrib, AttribModes, AttribProps, Block, Circle, Common, Drawing, EntityKind, HAlign, Insert, Line, LwPolyline, Space, Text, VAlign,
};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};

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
    /// A callout bubble as the import makes a bubble drawn again and again: a `PDF-SYM-` symbol
    /// of its circle and divider with the sheet number as its attribute, the detail number
    /// beside it as plain lettering.
    fn symbol_bubble(&mut self, c: [f64; 2], detail: &str, sheet: &str) {
        let r = 0.35;
        if !self.d.blocks.contains_key("PDF-SYM-2") {
            let mut b = Block::new("PDF-SYM-2");
            b.entities.push(cadaddy_doc::Entity {
                handle: self.d.new_handle(),
                common: common(),
                kind: EntityKind::Circle(Circle { center: Vec3::ZERO, radius: r }),
            });
            let kind = EntityKind::Line(Line { a: Vec3::new(-r, 0.0, 0.0), b: Vec3::new(r, 0.0, 0.0) });
            b.entities.push(cadaddy_doc::Entity { handle: self.d.new_handle(), common: common(), kind });
            self.d.blocks.insert("PDF-SYM-2".into(), std::sync::Arc::new(b));
        }
        let w = 0.35 * 0.12 * sheet.chars().count() as f64;
        let attrib = Attrib {
            tag: "TAG".into(),
            text: text_ent(sheet, c[0] - w, c[1] - 0.17 - 0.06, 0.12, 0.0),
            invisible: false,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        };
        let ins = Insert {
            block: "PDF-SYM-2".into(),
            insert: Vec3::new(c[0], c[1], 0.0),
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
        self.text_mid(detail, [c[0], c[1] + 0.17], 0.12);
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
    /// The title block in the right margin: the label, the title, the sheet number.
    fn title_block(&mut self, number: &str, title: &str) {
        self.line([31.5, 0.5], [31.5, 23.5]);
        self.text("DRAWING TITLE", [32.0, 6.0], 0.1, 0.0);
        self.text(title, [32.0, 5.7], 0.12, 0.0);
        self.text(number, [32.5, 3.0], 0.5, 0.0);
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
    s.title_block("E-1", "FIRST FLOOR POWER PLAN - AREA A");
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

/// E-1 with its 1/E-3 bubble made a symbol by the import (the sheet number its attribute) on
/// its boundary's edge, and a numbered note that refers to the enlarged plans in words.
pub fn e1_symbol_bubble() -> Drawing {
    let mut s = Sheet::new();
    s.title_block("E-1", "FIRST FLOOR POWER PLAN - AREA A");
    s.grid(&[("1", 4.0), ("2", 8.0), ("3", 12.0), ("4", 16.0)], &[("A", 4.0), ("B", 8.0), ("C", 12.0)], [2.0, 17.0], [2.0, 14.0]);
    // The bubble sits on the boundary's top edge, no leader.
    s.rect([7.5, 3.5, 16.5, 12.5]);
    s.symbol_bubble([12.0, 12.85], "1", "E-3");
    s.text("1. CORE CIRCUITS: REFER TO ENLARGED PLANS FOR LAYOUT.", [24.0, 22.0], 0.1, 0.0);
    s.view_title("1", "E-1", "FIRST FLOOR POWER PLAN - AREA A", "SCALE: 1/8\" = 1'-0\"", [4.0, 18.5]);
    s.d
}

/// E-2: the east half, 14 units to the left of where E-1 would draw it.
fn e2() -> Drawing {
    let mut s = Sheet::new();
    s.title_block("E-2", "FIRST FLOOR POWER PLAN - AREA B");
    s.grid(&[("4", 2.0), ("5", 6.0), ("6", 10.0), ("7", 14.0)], &[("A", 4.0), ("B", 8.0), ("C", 12.0)], [1.0, 16.0], [2.0, 14.0]);
    s.line([4.5, 1.0], [4.5, 15.0]);
    s.text("MATCH LINE - SEE SHEET E-1", [4.35, 6.0], 0.1, std::f64::consts::FRAC_PI_2);
    for p in [[8.0, 5.0], [12.0, 9.0], [8.0, 11.0], [14.0, 5.0]] {
        s.device("A", p);
    }
    s.view_title("1", "E-2", "FIRST FLOOR POWER PLAN - AREA B", "SCALE: 1/8\" = 1'-0\"", [4.0, 18.5]);
    s.d
}

/// E-5: a first-floor lighting plan with its fixture schedule in the margin, listing its own
/// types only (`L1`, `L2`, `XA`), none of them the power marks the other sheets print.
pub fn lighting_sheet() -> Drawing {
    let mut s = Sheet::new();
    s.title_block("E-5", "FIRST FLOOR LIGHTING PLAN");
    s.text("LIGHTING FIXTURE SCHEDULE", [24.0, 20.0], 0.2, 0.0);
    s.text("TYPE", [24.0, 19.6], 0.12, 0.0);
    s.text("DESCRIPTION", [25.0, 19.6], 0.12, 0.0);
    for (i, (ty, what)) in [("L1", "2X4 LED TROFFER"), ("L2", "LED DOWNLIGHT"), ("XA", "EXIT SIGN")].iter().enumerate() {
        let y = 19.3 - 0.3 * i as f64;
        s.text(ty, [24.0, y], 0.12, 0.0);
        s.text(what, [25.0, y], 0.12, 0.0);
    }
    for p in [[6.0, 6.0], [10.0, 10.0]] {
        s.device("L1", p);
    }
    s.view_title("1", "E-5", "FIRST FLOOR LIGHTING PLAN", "SCALE: 1/8\" = 1'-0\"", [4.0, 18.5]);
    s.d
}

/// E-3: the enlarged core plan at twice E-1's scale, its grid spaced 8 where E-1's is 4.
fn e3() -> Drawing {
    enlarged_sheet(Some(("ENLARGED FIRST FLOOR CORE PLAN", true)))
}

/// E-3 drawn with its view titled `title` (with or without the scale line under it), or with no
/// view title at all.
pub fn enlarged_sheet(title: Option<(&str, bool)>) -> Drawing {
    let mut s = Sheet::new();
    s.title_block("E-3", "ELECTRICAL ENLARGED PLANS");
    s.grid(&[("2", 4.0), ("3", 12.0), ("4", 20.0)], &[("A", 4.0), ("B", 12.0), ("C", 20.0)], [2.5, 21.5], [2.5, 21.5]);
    // The core's rooms.
    for r in [[4.0, 4.0, 12.0, 12.0], [12.0, 4.0, 20.0, 12.0], [4.0, 12.0, 12.0, 20.0], [12.0, 12.0, 20.0, 20.0]] {
        s.rect(r);
    }
    for p in [[6.0, 6.0], [10.0, 14.0], [14.0, 10.0], [18.0, 19.0], [20.5, 8.0]] {
        s.device("A", p);
    }
    match title {
        Some((t, true)) => s.view_title("1", "E-3", t, "SCALE: 1/4\" = 1'-0\"", [4.0, 1.2]),
        Some((t, false)) => {
            s.bubble([3.2, 1.32], "1", "E-3");
            s.text(t, [4.0, 1.2], 0.25, 0.0);
        }
        None => {}
    }
    s.d
}

/// E-3 the way a real enlarged sheet may print it: the core plan as view 1, titled by its area
/// alone (no level, no "PLAN"), and two small enlarged rooms beside it as titled views 4 and 5
/// with two devices each. With `scale_line` false the core's title and scale share one line
/// beside its bubble (no scale line under the title).
pub fn enlarged_sheet_with_details(scale_line: bool) -> Drawing {
    let mut s = Sheet::new();
    s.title_block("E-3", "ELECTRICAL ENLARGED PLANS");
    s.grid(&[("2", 4.0), ("3", 12.0), ("4", 20.0)], &[("A", 4.0), ("B", 12.0), ("C", 20.0)], [2.5, 21.5], [2.5, 21.5]);
    for r in [[4.0, 4.0, 12.0, 12.0], [12.0, 4.0, 20.0, 12.0], [4.0, 12.0, 12.0, 20.0], [12.0, 12.0, 20.0, 20.0]] {
        s.rect(r);
    }
    for p in [[6.0, 6.0], [10.0, 14.0], [14.0, 10.0], [18.0, 19.0], [20.5, 8.0]] {
        s.device("A", p);
    }
    if scale_line {
        s.view_title("1", "E-3", "COMMON AREA CORE - POWER", "SCALE: 1/4\" = 1'-0\"", [4.0, 1.2]);
    } else {
        s.bubble([3.2, 1.32], "1", "E-3");
        s.text("COMMON AREA CORE - POWER   SCALE: 1/4\" = 1'-0\"", [4.0, 1.2], 0.25, 0.0);
    }
    for (n, y0, title) in [("4", 14.0, "ENLARGED ELECTRICAL ROOM 101"), ("5", 4.0, "ENLARGED ELECTRICAL ROOM 102")] {
        s.rect([24.0, y0, 29.0, y0 + 5.0]);
        s.line([24.0, y0 + 2.5], [29.0, y0 + 2.5]);
        s.device("A", [25.0, y0 + 1.0]);
        s.device("A", [28.0, y0 + 4.0]);
        s.view_title(n, "E-3", title, "SCALE: 1/2\" = 1'-0\"", [24.8, y0 - 1.0]);
    }
    s.d
}

/// E-4: details; detail 2 prints the receptacle's mounting height.
fn e4() -> Drawing {
    let mut s = Sheet::new();
    s.title_block("E-4", "ELECTRICAL DETAILS");
    s.rect([2.0, 4.0, 10.0, 12.0]);
    s.line([4.0, 6.0], [8.0, 6.0]);
    s.line([4.0, 6.0], [4.0, 10.0]);
    s.text("MOUNT AT 18\" AFF", [3.0, 8.0], 0.1, 0.0);
    s.view_title("2", "E-4", "RECEPTACLE MOUNTING DETAIL", "SCALE: NTS", [3.0, 3.0]);
    s.d
}

/// The set's sheets as its index lists them (E-0 and A-101 are listed, not drawn).
pub fn sheets() -> Vec<SetSheet> {
    vec![
        set_sheet("E-0", "ELECTRICAL SYMBOLS AND SCHEDULES", "schedule", ""),
        set_sheet("E-1", "FIRST FLOOR POWER PLAN - AREA A", "plan", "1"),
        set_sheet("E-2", "FIRST FLOOR POWER PLAN - AREA B", "plan", "1"),
        set_sheet("E-3", "ELECTRICAL ENLARGED PLANS", "enlarged_plan", ""),
        set_sheet("E-4", "ELECTRICAL DETAILS", "detail", ""),
        set_sheet("A-101", "FIRST FLOOR PLAN", "plan", "1"),
    ]
}

/// The drawn sheets, by number.
pub fn drawings() -> Vec<(&'static str, Drawing)> {
    vec![("E-1", e1()), ("E-2", e2()), ("E-3", e3()), ("E-4", e4())]
}

/// A sheet printed to a one-page 36 x 24 PDF.
pub fn print(d: &Drawing) -> Result<Vec<u8>, String> {
    let opts = serde_json::json!({"width": 914.4, "height": 609.6, "window": [[0.0, 0.0], [36.0, 24.0]], "fit": true, "margins": [0, 0, 0, 0]});
    cadaddy_io::pdf::plot(d, &Space::Model, &opts).map_err(|e| e.to_string())
}
