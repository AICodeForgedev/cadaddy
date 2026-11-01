//! A print's plumbing fixture schedule, read from its lettering: the table whose header row names
//! a `TAG` column and a `FIXTURE` column (with its maker and model columns). Each row starts at a
//! tag in the tag column (`P-1`, `P-2H`, `WH-1,2,3`); its cells are the texts on the tag's
//! baseline under each header. A fixture's parts listed on the lines under it (a toilet's seat, a
//! shower's trim) stay with it: its first line names it.

use crate::sheet_facts::TextItem;
use cadaddy_doc::{Drawing, EntityKind};

/// Most rows read from one schedule.
const MAX_ROWS: usize = 400;
/// The rows reach at most this many header heights below the header.
const MAX_DEPTH_H: f64 = 400.0;
/// Header cells lie this far right of the tag header at most, in its heights.
const MAX_WIDTH_H: f64 = 400.0;
/// An Arial-like letter's width, in text heights (a cell's middle from its start).
const CHAR_EM: f64 = 0.6;
/// How far the last column's cells reach right of its header's middle, in text heights.
const DESCRIPTION_H: f64 = 40.0;

/// One fixture the schedule lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScheduledFixture {
    pub tag: String,
    /// `LAVATORY`, `KITCHEN SINK`, `ADA WATER CLOSET`.
    pub fixture: String,
    /// The maker, without the part it makes (`BOWL: SIGNATURE HARDWARE` → `SIGNATURE HARDWARE`).
    pub manufacturer: String,
    pub model: String,
    pub description: String,
}

/// A plumbing fixture schedule: its rows and its extent `[x0, y0, x1, y1]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FixtureSchedule {
    pub rows: Vec<ScheduledFixture>,
    pub extent: [f64; 4],
}

/// Whether a text is shaped like a schedule's tag: one to four capitals, then maybe a dash and a
/// number, a suffix letter, more numbers after commas (`EEW`, `P-1H`, `TP-1`, `WH-1,2,3`).
pub fn is_fixture_tag(t: &str) -> bool {
    let t = t.trim();
    let letters = t.chars().take_while(char::is_ascii_uppercase).count();
    (1..=4).contains(&letters)
        && t.len() <= 12
        && t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-' || c == ',')
        && !t.ends_with(['-', ','])
}

/// Most line pieces looked at for a schedule's column rules.
const MAX_SEGMENTS: usize = 200_000;

/// The plumbing fixture schedule on a sheet, if it has one: its lettering, in the columns its
/// vertical rules make.
pub fn read(d: &Drawing) -> Option<FixtureSchedule> {
    let mut verticals: Vec<(f64, f64, f64)> = Vec::new();
    let mut push = |a: [f64; 2], b: [f64; 2]| {
        if (a[0] - b[0]).abs() <= 1e-3 * (a[1] - b[1]).abs() && verticals.len() < MAX_SEGMENTS {
            verticals.push((a[0], a[1].min(b[1]), a[1].max(b[1])));
        }
    };
    for e in d.model.iter() {
        match &e.kind {
            EntityKind::Line(l) => push([l.a.x, l.a.y], [l.b.x, l.b.y]),
            EntityKind::LwPolyline(p) => {
                for w in p.vertices.windows(2) {
                    if let [a, b] = w {
                        push([a.p.x, a.p.y], [b.p.x, b.p.y]);
                    }
                }
            }
            _ => {}
        }
    }
    read_lettering(&crate::sheet_facts::texts(d), &verticals)
}

/// The plumbing fixture schedule among a sheet's texts, if it has one; `verticals` (x, y0, y1)
/// are the sheet's vertical line pieces: the ones crossing the header row are the columns'
/// rules (without them a column reaches halfway to the next header).
pub fn read_lettering(texts: &[TextItem], verticals: &[(f64, f64, f64)]) -> Option<FixtureSchedule> {
    let header = |t: &TextItem, w: &str| t.value.trim().eq_ignore_ascii_case(w);
    let middle = |t: &TextItem| t.at.x + 0.5 * CHAR_EM * t.height * t.value.trim().chars().count() as f64;
    for tag_head in texts.iter().filter(|t| header(t, "TAG") && t.height.is_finite() && t.height > 0.0) {
        let h = tag_head.height;
        let row_of =
            |t: &TextItem| (t.at.y - tag_head.at.y).abs() <= 0.5 * h && t.at.x >= tag_head.at.x - h && t.at.x <= tag_head.at.x + MAX_WIDTH_H * h;
        let mut heads: Vec<(&str, f64)> = texts.iter().filter(|t| row_of(t)).map(|t| (t.value.trim(), middle(t))).collect();
        if !heads.iter().any(|(v, _)| v.eq_ignore_ascii_case("FIXTURE")) {
            continue;
        }
        heads.sort_by(|a, b| a.1.total_cmp(&b.1));
        // The rules crossing the header row.
        let mut rules: Vec<f64> = verticals
            .iter()
            .filter(|(x, y0, y1)| {
                *y0 <= tag_head.at.y && *y1 >= tag_head.at.y && *x >= tag_head.at.x - 20.0 * h && *x <= tag_head.at.x + MAX_WIDTH_H * h
            })
            .map(|(x, _, _)| *x)
            .collect();
        rules.sort_by(f64::total_cmp);
        // Each column between the rules either side of its header (a cell's text starts there),
        // else from halfway to the header before it to halfway to the one after (the outer ones
        // as wide each side of their header; a cell's text centred there).
        let bounds: Vec<(String, f64, f64, bool)> = heads
            .iter()
            .enumerate()
            .map(|(i, (v, x))| {
                let ruled = (rules.iter().rev().find(|r| *r < x).copied(), rules.iter().find(|r| *r > x).copied());
                if let (Some(lo), Some(hi)) = ruled {
                    return (v.to_ascii_uppercase(), lo, hi, true);
                }
                let lo = i.checked_sub(1).and_then(|j| heads.get(j)).map(|p| (p.1 + x) / 2.0);
                let hi = heads.get(i + 1).map(|n| (n.1 + x) / 2.0);
                let (lo, hi) = match (lo, hi) {
                    (Some(lo), Some(hi)) => (lo, hi),
                    (None, Some(hi)) => (x - (hi - x), hi),
                    (Some(lo), None) => (lo, x + (x - lo)),
                    (None, None) => (f64::NEG_INFINITY, f64::INFINITY),
                };
                (v.to_ascii_uppercase(), lo, hi, false)
            })
            .collect();
        let column = |name: &str| bounds.iter().find(|(v, _, _, _)| v.starts_with(name)).map(|(_, lo, hi, ruled)| (*lo, *hi, *ruled));
        let inside = |t: &TextItem, (lo, hi, ruled): (f64, f64, bool)| {
            let x = if ruled { t.at.x } else { middle(t) };
            x >= lo && x < hi
        };
        let Some(tag_col) = column("TAG") else { continue };
        // The tag column top down, to the first text in it that is no tag (the notes under it).
        let mut below: Vec<&TextItem> =
            texts.iter().filter(|t| inside(t, tag_col) && t.at.y < tag_head.at.y - 0.5 * h && t.at.y > tag_head.at.y - MAX_DEPTH_H * h).collect();
        below.sort_by(|a, b| b.at.y.total_cmp(&a.at.y));
        let tags: Vec<&TextItem> = below.into_iter().take_while(|t| is_fixture_tag(&t.value)).take(MAX_ROWS).collect();
        let cell = |name: &str, y: f64| -> String {
            let Some(col) = column(name) else { return String::new() };
            let mut on: Vec<&TextItem> = texts.iter().filter(|t| (t.at.y - y).abs() <= 0.5 * h && inside(t, col)).collect();
            on.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
            on.iter().map(|t| t.value.trim()).collect::<Vec<_>>().join(" ")
        };
        let rows: Vec<ScheduledFixture> = tags
            .iter()
            .map(|t| {
                let maker = cell("MANUFACTURER", t.at.y);
                ScheduledFixture {
                    tag: t.value.trim().to_string(),
                    fixture: cell("FIXTURE", t.at.y),
                    manufacturer: maker.split_once(':').map_or(maker.as_str(), |(_, m)| m).trim().to_string(),
                    model: cell("MODEL", t.at.y),
                    description: cell("DESCRIPTION", t.at.y),
                }
            })
            .filter(|r| !r.fixture.is_empty())
            .collect();
        if rows.len() < 2 {
            continue;
        }
        let low = tags.last().map_or(tag_head.at.y, |t| t.at.y);
        let right = heads.last().map_or(tag_head.at.x, |(_, x)| *x);
        return Some(FixtureSchedule { rows, extent: [tag_head.at.x - h, low - h, right + DESCRIPTION_H * h, tag_head.at.y + 2.0 * h] });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_geom::Vec2;

    fn t(value: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: value.into(), at: Vec2::new(x, y), height: 0.1, rotation: 0.0 }
    }

    #[test]
    fn a_schedules_rows_are_its_tags_with_the_cells_on_their_line() {
        let texts = [
            t("PLUMBING FIXTURE SCHEDULE", 12.0, 29.0),
            t("TAG", 11.46, 28.7),
            t("FIXTURE", 13.05, 28.7),
            t("MANUFACTURER", 15.6, 28.7),
            t("MODEL NO.", 18.5, 28.7),
            t("DESCRIPTION", 23.5, 28.7),
            t("P-1", 11.5, 25.95),
            t("WATER CLOSET", 12.7, 25.95),
            t("TANK/BOWL: PROFLO", 15.4, 25.95),
            t("PF6112WH/PF1501WH", 18.2, 25.95),
            t("1.28 GPF TWO-PIECE ELONGATED TOILET", 20.5, 25.95),
            // The toilet's seat, on the line under it.
            t("SEAT: PROFLO", 15.9, 25.75),
            t("PFTSHC2000W", 18.4, 25.75),
            t("P-2", 11.5, 24.9),
            t("LAVATORY", 12.9, 24.9),
            t("BOWL: SIGNATURE HARDWARE", 14.9, 24.9),
            t("447961", 18.7, 24.9),
            t("P-5", 11.5, 20.18),
            t("KITCHEN SINK", 12.7, 20.18),
            t("GENERAL NOTES:", 11.03, 11.94),
            t("P-9", 11.5, 11.0),
        ];
        let s = read_lettering(&texts, &[]).expect("schedule");
        let tags: Vec<&str> = s.rows.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(tags, ["P-1", "P-2", "P-5"], "the notes under it end it");
        let wc = &s.rows[0];
        assert_eq!((wc.fixture.as_str(), wc.manufacturer.as_str(), wc.model.as_str()), ("WATER CLOSET", "PROFLO", "PF6112WH/PF1501WH"));
        assert_eq!(s.rows[1].manufacturer, "SIGNATURE HARDWARE");
        assert!(s.extent[1] < 20.18 && s.extent[3] > 28.7);
        // A header without a FIXTURE column (a door schedule's TAG) is no plumbing schedule.
        assert!(read_lettering(&[t("TAG", 0.0, 10.0), t("WIDTH", 2.0, 10.0), t("D1", 0.0, 9.0), t("D2", 0.0, 8.0)], &[]).is_none());
    }

    #[test]
    fn a_ruled_schedules_cells_are_between_its_rules() {
        // A maker set against its column's right rule, a description from its left one.
        let texts = [
            t("TAG", 11.46, 28.7),
            t("FIXTURE", 13.05, 28.7),
            t("MANUFACTURER", 15.6, 28.7),
            t("MODEL NO.", 18.5, 28.7),
            t("DESCRIPTION", 23.5, 28.7),
            t("ET-1", 11.47, 27.99),
            t("EXPANSION TANK", 12.78, 27.99),
            t("AMTROL", 17.33, 27.99),
            t("ST-42V", 18.67, 27.99),
            t("20 GALLON TANK VOLUME.", 19.9, 27.99),
            t("FD", 11.5, 26.96),
            t("FLOOR DRAIN", 12.9, 26.96),
            t("ZURN", 17.4, 26.96),
            t("ZN4153NHBP", 18.4, 26.96),
        ];
        let rules: Vec<(f64, f64, f64)> = [11.3, 12.18, 14.52, 17.93, 19.86, 28.08].iter().map(|x| (*x, 26.0, 28.86)).collect();
        let s = read_lettering(&texts, &rules).expect("schedule");
        let tank = &s.rows[0];
        assert_eq!((tank.manufacturer.as_str(), tank.model.as_str(), tank.description.as_str()), ("AMTROL", "ST-42V", "20 GALLON TANK VOLUME."));
    }

    #[test]
    fn fixture_tags_are_short_capitals_and_numbers() {
        for ok in ["P-1", "P-2H", "EEW", "WH-1,2,3", "TP-1"] {
            assert!(is_fixture_tag(ok), "{ok}");
        }
        for no in ["GENERAL NOTES:", "1.", "P-", "lav", "A VERY LONG TAG"] {
            assert!(!is_fixture_tag(no), "{no}");
        }
    }
}
