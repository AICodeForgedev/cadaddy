//! Fixture sheets laid out the way a real print set lays them out, where that differs from one
//! plan per sheet: two levels' plans stacked on one sheet, each titled under its view
//! (`1ST FLOOR LIGHTING PLAN`), each grid line with one bubble per view, the title block's
//! column headed by a block of general notes, and everything on one pen layer as a printed
//! sheet comes in. The sheet readers must split such a sheet into its views
//! ([`crate::sheet_reader::recognise_views`]) and align each on its own.

use crate::draw;
use crate::layout::{self, Kind};
use crate::style::Style;
use crate::truth::Truth;
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, HAlign, Space, Text, VAlign, entity_bounds};
use cadaddy_geom::{Mat3, Vec2, Vec3};

/// A two-view sheet and what a reader should find on it.
#[derive(Clone, Debug)]
pub struct TwoViewSheet {
    pub drawing: Drawing,
    /// The printed sheet number.
    pub number: String,
    /// The title in the title block (`LIGHTING PLAN - FIRST & SECOND FLOORS`).
    pub title: String,
    /// The levels of the lower and the upper view.
    pub levels: [String; 2],
    /// How far up the upper view was moved, model units: its alignment's offset is the lower
    /// view's plus this.
    pub shift: f64,
}

/// `1` → `1ST`, `2` → `2ND`, `13` → `13TH`.
fn ordinal(code: &str) -> String {
    let n: u32 = code.parse().unwrap_or(0);
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "TH",
        (1, _) => "ST",
        (2, _) => "ND",
        (3, _) => "RD",
        _ => "TH",
    };
    format!("{code}{suffix}")
}

/// `1` → `FIRST` … `10` → `TENTH`.
fn ordinal_word(code: &str) -> String {
    const WORDS: [&str; 10] = ["FIRST", "SECOND", "THIRD", "FOURTH", "FIFTH", "SIXTH", "SEVENTH", "EIGHTH", "NINTH", "TENTH"];
    code.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| WORDS.get(i)).map_or_else(|| ordinal(code), |w| (*w).to_string())
}

/// The plan part of a fixture sheet: everything but the title block, its legend and border.
fn plan_part(d: &Drawing) -> Vec<Entity> {
    d.model.iter().filter(|e| !["TITLE", "BORDER", "LEGEND"].iter().any(|l| e.common.layer.eq_ignore_ascii_case(l))).map(|e| (**e).clone()).collect()
}

/// Drops the grid bubbles at the bottom and the right end of each line (keeping one per line, at
/// the top and the left, as a print with one bubble per line does).
fn one_bubble_per_line(d: &Drawing, items: Vec<Entity>) -> Vec<Entity> {
    let bubble_box = items
        .iter()
        .filter(|e| e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE"))
        .fold(cadaddy_geom::Bounds2::EMPTY, |b, e| b.union(&entity_bounds(d, e, 0)));
    if bubble_box.is_empty() {
        return items;
    }
    let (w, h) = (bubble_box.width(), bubble_box.height());
    items
        .into_iter()
        .filter(|e| {
            if !e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE") {
                return true;
            }
            let c = entity_bounds(d, e, 0).center();
            c.y > bubble_box.min.y + h * 0.05 && c.x < bubble_box.max.x - w * 0.05
        })
        .collect()
}

fn text(at: Vec2, height: f64, s: &str) -> EntityKind {
    EntityKind::Text(Text {
        insert: Vec3::new(at.x, at.y, 0.0),
        align_pt: None,
        height,
        value: s.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "STANDARD".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    })
}

/// Two plan sheets of the set (`lower` and `upper` sheet numbers, both drawn as `kind`'s
/// discipline) printed as one sheet numbered `number`: `lower`'s plan below, `upper`'s above,
/// each titled `<level> FLOOR <discipline word> PLAN` under it, one bubble per grid line, a block of
/// general notes heading the title column, all on layer `0`. `None` when either sheet isn't in
/// the set or isn't a plan of one level.
pub fn two_view_sheet(t: &Truth, st: &Style, lower: &str, upper: &str, number: &str, discipline_word: &str) -> Option<TwoViewSheet> {
    let kinds = layout::hotel_sheet_list();
    let kind = |n: &str| kinds.iter().find(|(k, _, _, _)| k == n).map(|(_, _, _, k)| k.clone());
    let sheet = |n: &str| t.sheets.iter().find(|s| s.number == n);
    let (sl, su) = (sheet(lower)?, sheet(upper)?);
    let (kl, ku): (Kind, Kind) = (kind(lower)?, kind(upper)?);
    let levels = [sl.level.clone()?, su.level.clone()?];
    let (dl, _, _) = draw::draw(t, st, sl, &kl);
    let (du, _, _) = draw::draw(t, st, su, &ku);
    let lower_items = one_bubble_per_line(&dl, plan_part(&dl));
    let upper_items = one_bubble_per_line(&du, plan_part(&du));
    let extent = |d: &Drawing, items: &[Entity]| items.iter().fold(cadaddy_geom::Bounds2::EMPTY, |b, e| b.union(&entity_bounds(d, e, 0)));
    let (bl, bu) = (extent(&dl, &lower_items), extent(&du, &upper_items));
    if bl.is_empty() || bu.is_empty() {
        return None;
    }
    // The sheet's usual lettering, for the view titles and notes.
    let mut heights: Vec<f64> = lower_items
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some(t.height),
            EntityKind::MText(t) => Some(t.height),
            _ => None,
        })
        .filter(|h| h.is_finite() && *h > 0.0)
        .collect();
    heights.sort_by(f64::total_cmp);
    let h = heights.get(heights.len() / 2).copied().unwrap_or(bl.height() * 0.01);
    let gap = h * 8.0;
    // The upper view sits a title's room above the lower one.
    let shift = bl.max.y - bu.min.y + gap * 2.0;
    let move_up = Mat3::translate(Vec2::new(0.0, shift));

    let mut out = Drawing::new_imperial();
    let mut items: Vec<EntityKind> = Vec::new();
    for e in lower_items {
        items.push(e.kind);
    }
    for e in upper_items {
        let mut kind = e.kind;
        kind.transform(&move_up);
        items.push(kind);
    }
    // View titles under each plan.
    let title_h = h * 2.5;
    let view_title = |level: &str| format!("{} FLOOR {discipline_word} PLAN", ordinal(level));
    let [ll, lu] = &levels;
    items.push(text(Vec2::new(bl.center().x - title_h * 6.0, bl.min.y - gap), title_h, &view_title(ll)));
    items.push(text(Vec2::new(bl.center().x - title_h * 6.0, bu.min.y + shift - gap), title_h, &view_title(lu)));
    // The title column on the right: general notes at the top, the sheet's title and number at
    // the foot.
    let top = bu.max.y + shift;
    let bottom = bl.min.y - gap * 2.0;
    let x = bl.max.x.max(bu.max.x) + (top - bottom) * 0.25;
    items.push(text(Vec2::new(x, top - title_h), title_h, "SHEET NOTES -"));
    items.push(text(Vec2::new(x, top - title_h * 2.4), title_h, "FLOOR PLAN"));
    for (i, note) in ["VERIFY ALL DIMENSIONS IN THE FIELD.", "LEVEL MARKS ARE TO FINISHED FLOOR.", "COORDINATE WITH ALL TRADES."].iter().enumerate() {
        items.push(text(Vec2::new(x, top - title_h * 4.0 - h * 2.0 * i as f64), h, &format!("{}. {note}", i + 1)));
    }
    let title = format!("{discipline_word} PLAN - {} & {} FLOORS", ordinal_word(ll), ordinal_word(lu));
    items.push(text(Vec2::new(x, bottom + title_h * 6.0), title_h * 0.8, &format!("{discipline_word} PLAN -")));
    items.push(text(Vec2::new(x, bottom + title_h * 4.8), title_h * 0.8, &format!("{} & {} FLOORS", ordinal_word(ll), ordinal_word(lu))));
    items.push(text(Vec2::new(x, bottom + title_h * 3.0), h, &format!("SCALE: {}", sl.scale_text.clone().unwrap_or_default())));
    items.push(text(Vec2::new(x, bottom), title_h * 3.0, number));
    for kind in items {
        // A print is one pen layer.
        out.add(&Space::Model, Common { layer: "0".into(), ..Common::default() }, kind).ok()?;
    }
    Some(TwoViewSheet { drawing: out, number: number.into(), title, levels, shift })
}

/// How [`one_view_sheet`] prints its plan, where real sets differ from sheet to sheet.
#[derive(Clone, Debug)]
pub struct OneViewOptions {
    /// How many grid lines are printed, first the lines of constant x then of constant y (a
    /// floor's sheets printed with the grid turned off but for a stray line: its bubbles stand
    /// in a row along the top and a column down the left with nothing leaving them).
    pub grid_lines: usize,
    /// Two pairs of same-label keynote tags, each pair in line (one pair up and down, one
    /// across), drawn the size of the grid bubbles: on their own they read as a 1×1 "grid".
    pub keynote_pairs: bool,
}

impl Default for OneViewOptions {
    fn default() -> Self {
        Self { grid_lines: usize::MAX, keynote_pairs: false }
    }
}

/// A one-view printed sheet and what a reader should find on it.
#[derive(Clone, Debug)]
pub struct OneViewSheet {
    pub drawing: Drawing,
    pub number: String,
    pub level: String,
    /// The grid lines a reader should find (every line of the building's grid).
    pub grid_x: usize,
    pub grid_y: usize,
}

/// One plan sheet of the set (`number`, drawn as its kind) printed the way a print set does: one
/// bubble per grid line, the view titled `<level> FLOOR <discipline word> PLAN` under it, a title
/// column on the right, all on layer `0`, with `opts` for the ways a real sheet strays. `None`
/// when the sheet isn't in the set or isn't a plan of one level.
pub fn one_view_sheet(t: &Truth, st: &Style, number: &str, discipline_word: &str, opts: &OneViewOptions) -> Option<OneViewSheet> {
    let kinds = layout::hotel_sheet_list();
    let kind = kinds.iter().find(|(k, _, _, _)| k == number).map(|(_, _, _, k)| k.clone())?;
    let sheet = t.sheets.iter().find(|s| s.number == number)?;
    let level = sheet.level.clone()?;
    let (d, _, _) = draw::draw(t, st, sheet, &kind);
    let mut grid_seen = 0usize;
    let part: Vec<Entity> = plan_part(&d)
        .into_iter()
        .filter(|e| {
            if !e.common.layer.eq_ignore_ascii_case("GRID") {
                return true;
            }
            grid_seen += 1;
            grid_seen <= opts.grid_lines
        })
        .collect();
    let items = one_bubble_per_line(&d, part);
    let b = items.iter().fold(cadaddy_geom::Bounds2::EMPTY, |b, e| b.union(&entity_bounds(&d, e, 0)));
    if b.is_empty() {
        return None;
    }
    let bubble_r = items
        .iter()
        .filter(|e| e.common.layer.eq_ignore_ascii_case("GRID-BUBBLE"))
        .find_map(|e| match &e.kind {
            EntityKind::Circle(c) => Some(c.radius),
            _ => None,
        })
        .unwrap_or(b.height() * 0.01);
    let mut heights: Vec<f64> = items
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) => Some(t.height),
            EntityKind::MText(t) => Some(t.height),
            _ => None,
        })
        .filter(|h| h.is_finite() && *h > 0.0)
        .collect();
    heights.sort_by(f64::total_cmp);
    let h = heights.get(heights.len() / 2).copied().unwrap_or(b.height() * 0.01);
    let gap = h * 8.0;
    let mut kinds_out: Vec<EntityKind> = items.into_iter().map(|e| e.kind).collect();
    if opts.keynote_pairs {
        // Labels no grid line has, each tag a grid bubble's size, in the margin under the plan
        // (where nothing leaves them): one pair stacked, one pair side by side.
        let taken: Vec<&str> = t.building.grid.x.iter().chain(&t.building.grid.y).map(|g| g.label.as_str()).collect();
        let mut free = (1..100u32).map(|n| n.to_string()).filter(|l| !taken.contains(&l.as_str()));
        let (Some(l1), Some(l2)) = (free.next(), free.next()) else { return None };
        let y0 = b.min.y - gap * 3.0;
        let x0 = b.min.x + b.width() * 0.3;
        let step = bubble_r * 6.0;
        for (c, l) in [([x0, y0], &l1), ([x0, y0 - step], &l1), ([x0 + step * 2.0, y0], &l2), ([x0 + step * 3.0, y0], &l2)] {
            kinds_out.push(EntityKind::Circle(cadaddy_doc::Circle { center: Vec3::new(c[0], c[1], 0.0), radius: bubble_r }));
            let th = bubble_r * 0.9;
            kinds_out.push(text(Vec2::new(c[0] - th * 0.35 * l.len() as f64, c[1] - th * 0.5), th, l));
        }
    }
    let title_h = h * 2.5;
    kinds_out.push(text(
        Vec2::new(b.center().x - title_h * 6.0, b.min.y - gap),
        title_h,
        &format!("{} FLOOR {discipline_word} PLAN", ordinal(&level)),
    ));
    let (top, bottom) = (b.max.y, b.min.y - gap * 2.0);
    let x = b.max.x + (top - bottom) * 0.25;
    kinds_out.push(text(Vec2::new(x, bottom + title_h * 6.0), title_h * 0.8, &format!("{} FLOOR {discipline_word} PLAN", ordinal_word(&level))));
    kinds_out.push(text(Vec2::new(x, bottom + title_h * 3.0), h, &format!("SCALE: {}", sheet.scale_text.clone().unwrap_or_default())));
    kinds_out.push(text(Vec2::new(x, bottom), title_h * 3.0, number));
    let mut out = Drawing::new_imperial();
    for kind in kinds_out {
        out.add(&Space::Model, Common { layer: "0".into(), ..Common::default() }, kind).ok()?;
    }
    Some(OneViewSheet { drawing: out, number: number.into(), level, grid_x: t.building.grid.x.len(), grid_y: t.building.grid.y.len() })
}
