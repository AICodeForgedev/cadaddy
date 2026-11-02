//! The mounting heights a set's notes list (Q6158): a general note's list of device heights
//!
//! ```text
//! 17. ELECTRICAL DEVICE MOUNTED HEIGHTS (UNO):
//!     • TOGGLE SWITCH        4'-0" AFF TO TOP
//!     • RECEPTACLES          1'-6" CENTERLINE
//! ```
//!
//! and an accessibility sheet's numbered list of what goes how high
//!
//! ```text
//! 7. WATER CLOSETS (TO TOP OF SEAT): STANDARD = 14" TO 15". ADA (TO TOP OF
//!    SEAT) = 17" TO 19". ADA FLUSH CONTROLS = 44" MAX.
//! ```
//!
//! Each entry keeps what it is for (its subject), the height for a standard room and for an
//! accessible one, what the height is measured to on the item (its middle, top or bottom), and
//! the words read. A keyed note (`10. RECEPTACLE FOR PTAC … MOUNT 18" AFF`) is no list: only a
//! numbered note whose heading says it lists heights, or one written `STANDARD =` / `ADA =`, is
//! read. Every scan is capped; nothing here panics.

use std::collections::BTreeSet;

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::MountNote;

/// Numbered notes looked at, at most.
const MAX_ITEMS: usize = 5_000;
/// Lines one numbered note may run to.
const MAX_LINES: usize = 16;
/// A note's next line starts within this many text heights below the one before.
const LINE_GAP: f64 = 2.5;
/// A note's lines start this far right of its number at most (text heights): its values'
/// column too (`1'-6" CENTERLINE` beside `RECEPTACLES`).
const NOTE_WIDTH: f64 = 30.0;
/// Entries read off one set, at most.
pub const MAX_ENTRIES: usize = 2_000;
/// The heights a list gives, inches: 0 (in the floor) to 20'.
const RANGE: (f64, f64) = (0.0, 240.0);

/// What a height is measured to on the item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Datum {
    /// Its middle (`CENTERLINE`, `TO C.L.`, a plain `48" AFF`).
    #[default]
    Centre,
    /// Its top (`AFF TO TOP`, `TO TOP OF SEAT`, `TO SINK RIM`).
    Top,
    /// Its bottom (`AFF TO BOTTOM`, `TO B.O. …`).
    Bottom,
}

impl Datum {
    /// What `text` (upper case or not) says a height is measured to.
    pub fn of(text: &str) -> Datum {
        let u = norm(text).to_ascii_uppercase();
        let has = |w: &str| u.contains(w);
        if has("TO BOTTOM") || has("TO BOT.") || has("TO BOT ") || has("B.O.") || has("BOTTOM OF") || has("TO BASE") {
            Datum::Bottom
        } else if has("TO TOP") || has("TOP OF") || has("T.O.") || has(" RIM") || has("(RIM") {
            Datum::Top
        } else {
            Datum::Centre
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Datum::Centre => "centre",
            Datum::Top => "top",
            Datum::Bottom => "bottom",
        }
    }
    pub fn parse(s: &str) -> Datum {
        match s.trim().to_ascii_lowercase().as_str() {
            "top" => Datum::Top,
            "bottom" => Datum::Bottom,
            _ => Datum::Centre,
        }
    }
    /// The words a property's note uses: `to its top`.
    pub fn words(self) -> &'static str {
        match self {
            Datum::Centre => "to its middle",
            Datum::Top => "to its top",
            Datum::Bottom => "to its bottom",
        }
    }
}

/// Typographic quotes as the plain ones heights are written with (`18”` → `18"`).
pub fn norm(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{201C}' | '\u{201D}' | '\u{2033}' | '\u{201E}' => '"',
            '\u{2018}' | '\u{2019}' | '\u{2032}' => '\'',
            '\u{2013}' | '\u{2014}' => '-',
            _ => c,
        })
        .collect()
}

/// A height written in a text: inches, and whether `MIN` or `MAX` follows it, and whether `TO`
/// joins it to the next one (a range).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dim {
    pub inches: f64,
    pub min: bool,
    pub max: bool,
    /// `14" TO 15"`: this one is the range's start.
    pub to_next: bool,
}

/// A number at `b[i..]` (digits, maybe a decimal part): its value and the end.
fn number(b: &[u8], mut i: usize) -> Option<(f64, usize)> {
    let start = i;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let t = std::str::from_utf8(b.get(start..i)?).ok()?;
    if t.is_empty() || t.len() > 12 {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| (v, i))
}

/// A fraction ` 3/4` at `b[i..]` (after a space): its value and the end.
fn fraction(b: &[u8], i: usize) -> Option<(f64, usize)> {
    let mut j = i;
    while b.get(j) == Some(&b' ') {
        j += 1;
    }
    if j == i {
        return None;
    }
    let (n, k) = number(b, j)?;
    if b.get(k) != Some(&b'/') {
        return None;
    }
    let (d, end) = number(b, k + 1)?;
    (d > 0.0 && n < d).then_some((n / d, end))
}

fn skip_spaces(b: &[u8], mut i: usize) -> usize {
    while b.get(i) == Some(&b' ') {
        i += 1;
    }
    i
}

/// The heights written in `text` (`18"`, `1'-6"`, `4' - 0"`, `43 3/4"`, `46 INCHES`), in order.
pub fn dims(text: &str) -> Vec<Dim> {
    let u = norm(text).to_ascii_uppercase();
    let b = u.as_bytes();
    let mut out: Vec<Dim> = Vec::new();
    let mut i = 0;
    while i < b.len() && out.len() < 64 {
        let starts = b.get(i).is_some_and(u8::is_ascii_digit)
            && (i == 0 || !b.get(i - 1).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'.' || *c == b'/'));
        if !starts {
            i += 1;
            continue;
        }
        let Some((n, j)) = number(b, i) else {
            i += 1;
            continue;
        };
        let mut k = skip_spaces(b, j);
        let inches = if b.get(k) == Some(&b'\'') {
            // Feet, maybe with inches: 4'-0", 4' - 6 1/2".
            k = skip_spaces(b, k + 1);
            if b.get(k) == Some(&b'-') {
                k = skip_spaces(b, k + 1);
            }
            match number(b, k) {
                Some((m, e)) => {
                    let (f, e2) = fraction(b, e).unwrap_or((0.0, e));
                    let e3 = skip_spaces(b, e2);
                    if b.get(e3) == Some(&b'"') {
                        k = e3 + 1;
                        Some(n * 12.0 + m + f)
                    } else {
                        Some(n * 12.0)
                    }
                }
                None => Some(n * 12.0),
            }
        } else {
            let (f, e) = fraction(b, j).unwrap_or((0.0, j));
            let e = skip_spaces(b, e);
            if b.get(e) == Some(&b'"') {
                k = e + 1;
                Some(n + f)
            } else if u.get(e..).is_some_and(|r| r.starts_with("INCHES") || r.starts_with("IN.") || r.starts_with("IN ")) {
                k = e + 2;
                Some(n + f)
            } else {
                None
            }
        };
        let Some(inches) = inches.filter(|v| v.is_finite() && (RANGE.0..=RANGE.1).contains(v)) else {
            i = j.max(i + 1);
            continue;
        };
        // What follows it: MIN / MAX, then maybe TO and the range's end.
        let rest = u.get(k..).unwrap_or("");
        let after: String = rest.trim_start_matches(['.', ',', ' ']).chars().take(12).collect();
        let min = after.starts_with("MIN");
        let max = after.starts_with("MAX");
        let tail = if min || max { after.get(3..).unwrap_or("").trim_start_matches(['.', ',', ' ']).to_string() } else { after.clone() };
        let to_next = tail.starts_with("TO ") && tail.get(3..).is_some_and(|r| r.trim_start().starts_with(|c: char| c.is_ascii_digit()));
        out.push(Dim { inches, min, max, to_next });
        i = k.max(i + 1);
    }
    out
}

/// The height a run of written heights gives: a maximum's (`36" MAX`: what the item is built
/// to), else a range's middle (`14" TO 15"`), else the first.
pub fn pick(ds: &[Dim]) -> Option<f64> {
    if let Some(d) = ds.iter().find(|d| d.max) {
        return Some(d.inches);
    }
    if let Some(w) = ds.windows(2).find(|w| w.first().is_some_and(|a| a.to_next)) {
        let (a, b) = (w.first()?, w.get(1)?);
        return Some((a.inches + b.inches) / 2.0);
    }
    ds.first().map(|d| d.inches)
}

/// The `STANDARD =` and `ADA =` heights of a note's text, and whether it puts the item at the
/// ceiling (`STANDARD = CEILING HEIGHT`). An `ADA` with other words before its `=` (`ADA FLUSH
/// CONTROLS =`) is another item's; `(TO TOP OF SEAT)` or a reach (`ADA SIDE REACH =`) is its own.
fn standard_and_ada(text: &str) -> (Option<f64>, Option<f64>, bool) {
    let u = norm(text).to_ascii_uppercase();
    let mut at: Vec<(usize, bool)> = Vec::new();
    for (word, ada) in [("STANDARD", false), ("ADA", true)] {
        let mut from = 0;
        while let Some(p) = u.get(from..).and_then(|r| r.find(word)).map(|p| p + from) {
            let before_ok = p == 0 || !u.as_bytes().get(p - 1).is_some_and(u8::is_ascii_alphanumeric);
            let after_ok = !u.as_bytes().get(p + word.len()).is_some_and(u8::is_ascii_alphanumeric);
            if before_ok && after_ok {
                at.push((p, ada));
            }
            from = p + word.len();
            if at.len() > 32 {
                break;
            }
        }
    }
    at.sort_by_key(|(p, _)| *p);
    let (mut standard, mut accessible, mut ceiling) = (None, None, false);
    for (i, (p, ada)) in at.iter().enumerate() {
        let end = at.get(i + 1).map_or(u.len(), |(q, _)| *q);
        let Some(seg) = u.get(*p..end) else { continue };
        let Some((head, value)) = seg.split_once('=') else { continue };
        let head = head.get(if *ada { 3 } else { 8 }..).unwrap_or("").trim();
        let own = head.is_empty() || (head.starts_with('(') && head.ends_with(')')) || head.contains("REACH");
        if !own {
            continue;
        }
        let v = value.trim();
        if v.starts_with("CEILING") {
            ceiling = true;
            continue;
        }
        let h = pick(&dims(v));
        match (ada, h) {
            (false, Some(h)) if standard.is_none() => standard = Some(h),
            (true, Some(h)) if accessible.is_none() => accessible = Some(h),
            _ => {}
        }
    }
    (standard, accessible, ceiling)
}

/// A numbered note's start: `17.ELECTRICAL …`, `7. WATER CLOSETS …` (not `1.28 GPF`).
fn numbered(t: &str) -> bool {
    let t = t.trim_start();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    (1..=2).contains(&digits)
        && t.get(digits..).is_some_and(|r| r.starts_with('.') && !r.get(1..).is_some_and(|x| x.starts_with(|c: char| c.is_ascii_digit())))
}

/// The text after a note's number.
fn after_number(t: &str) -> &str {
    let t = t.trim_start();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    t.get(digits + 1..).unwrap_or("").trim()
}

/// A heading over a list of heights with no number of its own (`DEVICE MOUNTING HEIGHTS`,
/// `MOUNTING HEIGHTS (UNO):`).
fn heights_heading(t: &str) -> bool {
    let u = t.trim().to_ascii_uppercase();
    u.len() <= 60 && (u.contains("MOUNTING HEIGHT") || u.contains("MOUNTED HEIGHT")) && dims(&u).is_empty()
}

/// The words of a one-text row before its height (`RECEPTACLES ..... 18" AFF` → `RECEPTACLES`),
/// leaders and punctuation dropped.
fn subject_before_height(t: &str) -> String {
    let cut = t.char_indices().find(|(i, c)| c.is_ascii_digit() && !dims(t.get(*i..).unwrap_or("")).is_empty()).map_or(t.len(), |(i, _)| i);
    let head = t.get(..cut).unwrap_or("");
    let words: Vec<&str> =
        head.split(|c: char| !(c.is_ascii_alphanumeric() || c == '/' || c == '&')).filter(|w| w.chars().any(|c| c.is_ascii_alphabetic())).collect();
    words.join(" ")
}

fn bullet(t: &str) -> bool {
    matches!(t.trim(), "•" | "·" | "-" | "*" | "\u{25AA}" | "\u{25CF}" | "o")
}

/// One list entry read: what it is for, its heights, its datum, the words and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub subject: String,
    pub inches: Option<f64>,
    pub ada_inches: Option<f64>,
    pub datum: Datum,
    pub ceiling: bool,
    pub text: String,
    pub at: Vec2,
}

impl Listed {
    pub fn to_note(&self, sheet: &str) -> MountNote {
        MountNote {
            sheet: sheet.to_string(),
            subject: self.subject.clone(),
            inches: self.inches,
            ada_inches: self.ada_inches,
            datum: self.datum.name().to_string(),
            ceiling: self.ceiling,
            text: self.text.chars().take(240).collect(),
        }
    }
}

/// The subject of a numbered note's heading: before its datum's brackets or its colon.
fn subject_of(head: &str) -> String {
    let end = head.find(['(', ':', '=']).unwrap_or(head.len());
    head.get(..end).unwrap_or(head).trim().trim_end_matches(['-', ',', ' ']).trim().to_string()
}

/// The mounting-height lists in a sheet's texts.
pub fn read_lists(texts: &[TextItem]) -> Vec<Listed> {
    let mut used = vec![false; texts.len()];
    read_lists_marking(texts, &mut used)
}

/// [`read_lists`], marking in `used` the texts its entries were read from (they are no plan
/// notes of their own).
pub fn read_lists_marking(texts: &[TextItem], used: &mut [bool]) -> Vec<Listed> {
    // By x, for each note's column.
    let mut by_x: Vec<usize> = (0..texts.len()).collect();
    by_x.sort_by(|a, b| {
        let (ta, tb) = (texts.get(*a).map_or(0.0, |t| t.at.x), texts.get(*b).map_or(0.0, |t| t.at.x));
        ta.total_cmp(&tb)
    });
    let xs: Vec<f64> = by_x.iter().map(|i| texts.get(*i).map_or(0.0, |t| t.at.x)).collect();
    let in_x = |lo: f64, hi: f64| -> &[usize] {
        let a = xs.partition_point(|x| *x < lo);
        let b = xs.partition_point(|x| *x <= hi);
        by_x.get(a..b.max(a)).unwrap_or(&[])
    };
    let starts: Vec<usize> = texts
        .iter()
        .enumerate()
        .filter(|(_, t)| (numbered(&t.value) || heights_heading(&t.value)) && t.height.is_finite() && t.height > 0.0 && t.at.is_finite())
        .map(|(i, _)| i)
        .take(MAX_ITEMS)
        .collect();
    let mut out: Vec<Listed> = Vec::new();
    for &si in &starts {
        if out.len() >= MAX_ENTRIES {
            break;
        }
        let Some(s) = texts.get(si) else { continue };
        let h = s.height;
        let (lo, hi) = (s.at.x - 0.5 * h, s.at.x + NOTE_WIDTH * h);
        let column = in_x(lo, hi);
        // The next numbered note of the column below this one ends it.
        let stop = column
            .iter()
            .filter_map(|i| texts.get(*i))
            .filter(|t| (numbered(&t.value) || heights_heading(&t.value)) && (t.at.x - s.at.x).abs() <= h && t.at.y < s.at.y - 0.3 * h)
            .map(|t| t.at.y)
            .fold(f64::NEG_INFINITY, f64::max);
        let mut lines: Vec<(usize, &TextItem)> = column
            .iter()
            .filter_map(|i| texts.get(*i).map(|t| (*i, t)))
            .filter(|(_, t)| t.at.y <= s.at.y + 0.3 * h && t.at.y > stop + 0.3 * h && t.at.y >= s.at.y - (MAX_LINES as f64) * LINE_GAP * h)
            .filter(|(_, t)| (t.height - h).abs() <= 0.5 * h)
            .collect();
        lines.sort_by(|(_, a), (_, b)| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
        // Rows, top down, until a gap.
        let mut rows: Vec<Vec<&TextItem>> = Vec::new();
        let mut taken: Vec<usize> = Vec::new();
        let mut last_y = s.at.y;
        for (i, t) in lines {
            if last_y - t.at.y > LINE_GAP * h {
                break;
            }
            match rows.last_mut() {
                Some(row) if row.first().is_some_and(|f| (f.at.y - t.at.y).abs() <= 0.5 * h) => row.push(t),
                _ => {
                    if rows.len() >= MAX_LINES {
                        break;
                    }
                    rows.push(vec![t]);
                }
            }
            taken.push(i);
            last_y = t.at.y;
        }
        let before = out.len();
        for row in &mut rows {
            row.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
        }
        let Some(first) = rows.first() else { continue };
        let head: String = first.iter().map(|t| norm(&t.value)).collect::<Vec<_>>().join(" ");
        let head_up = head.to_ascii_uppercase();
        // A list under a heading that says it lists heights: each row an item.
        if head_up.contains("HEIGHT") && rows.len() > 1 {
            for row in rows.iter().skip(1) {
                let (vals, words): (Vec<&&TextItem>, Vec<&&TextItem>) =
                    row.iter().filter(|t| !bullet(&t.value)).partition(|t| !dims(&t.value).is_empty());
                // A row written in one text (`RECEPTACLES ..... 18" AFF`): its words before the
                // height are what it is for.
                let lead: Vec<String> = vals.iter().map(|t| subject_before_height(&norm(&t.value))).filter(|w| !w.is_empty()).collect();
                let subject = words.iter().map(|t| norm(&t.value)).chain(lead).collect::<Vec<_>>().join(" ").trim().to_string();
                let value = vals.iter().map(|t| norm(&t.value)).collect::<Vec<_>>().join(" ");
                let Some(inches) = pick(&dims(&value)) else { continue };
                if subject.is_empty() || numbered(&subject) {
                    continue;
                }
                let at = row.first().map_or(s.at, |t| t.at);
                out.push(Listed {
                    text: format!("{}: {subject} {value}", subject_of(after_number(&head))),
                    subject,
                    inches: Some(inches),
                    ada_inches: None,
                    datum: Datum::of(&value),
                    ceiling: false,
                    at,
                });
            }
            if out.len() > before {
                for i in &taken {
                    if let Some(u) = used.get_mut(*i) {
                        *u = true;
                    }
                }
            }
            continue;
        }
        // A note written `SUBJECT (TO …): STANDARD = …. ADA = ….`
        let joined: String = rows.iter().map(|r| r.iter().map(|t| norm(&t.value)).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(" ");
        let body = after_number(&joined).to_string();
        let up = body.to_ascii_uppercase();
        if !(up.contains("STANDARD =") || up.contains("STANDARD=") || up.contains("ADA =") || up.contains("ADA=") || up.contains("ADA (")) {
            continue;
        }
        let subject = subject_of(&body);
        if subject.is_empty() || subject.len() > 80 {
            continue;
        }
        let (inches, ada_inches, ceiling) = standard_and_ada(&body);
        if inches.is_none() && ada_inches.is_none() && !ceiling {
            continue;
        }
        // Its datum: the heading's brackets (`(TO TOP OF SEAT)`), else the value's (`(TO TOP)`).
        let head_part = body.split_once(':').map_or(body.as_str(), |(a, _)| a);
        let datum = if head_part.contains('(') { Datum::of(head_part) } else { Datum::of(&body) };
        out.push(Listed { subject, inches, ada_inches, datum, ceiling, text: body.chars().take(240).collect(), at: s.at });
        for i in &taken {
            if let Some(u) = used.get_mut(*i) {
                *u = true;
            }
        }
    }
    out
}

// ---------------------------------------------------------------- what an entry is for

/// The items a list names, by the words it uses: (topic, words). A device's own words (its
/// kind, its type, its mark) name its topics the same way.
const TOPICS: &[(&str, &[&str])] = &[
    ("receptacle", &["RECEPTACLE", "OUTLET", "REC", "GFCI", "DUPLEX", "QUAD"]),
    ("switch", &["SWITCH", "DIMMER", "TOGGLE"]),
    ("data", &["DATA", "VOICE", "TELEPHONE", "TEL", "LOW VOLTAGE", "TV", "CATV", "LOW_VOLTAGE"]),
    ("card_reader", &["CARD READER", "CR"]),
    ("thermostat", &["THERMOSTAT", "TSTAT"]),
    ("panel", &["PANELBOARD", "PANEL"]),
    ("emergency_light", &["EMERGENCY LIGHT", "EMERGENCY LIGHTS", "EMERGENCY"]),
    ("exit", &["EXIT"]),
    ("pull_station", &["PULL STATION", "PULL"]),
    ("notification", &["HORN", "STROBE", "SPEAKER", "VISUAL", "SIGNAL"]),
    ("detector", &["SMOKE", "DETECTOR", "HEAT DETECTOR"]),
    ("water_closet", &["WATER CLOSET", "TOILET", "WC"]),
    ("urinal", &["URINAL"]),
    ("lavatory", &["LAVATORY", "LAV", "VANITY"]),
    ("sink", &["SINK", "COUNTERTOP", "COUNTER"]),
    ("drinking_fountain", &["DRINKING FOUNTAIN", "EWC"]),
    ("shower", &["SHOWER HEAD", "SHOWERHEAD"]),
    ("extinguisher", &["EXTINGUISHER"]),
    ("door_holder", &["DOOR HOLD", "HOLD OPEN", "MAGNETIC"]),
];

/// Words of `text` as `_A_B_` (upper case, singular).
fn word_line(text: &str) -> String {
    let up = norm(text).to_ascii_uppercase();
    let ws: Vec<String> = up
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            if w.len() > 4 && (w.ends_with("CHES") || w.ends_with("SHES") || w.ends_with("XES")) {
                w.get(..w.len() - 2).unwrap_or(w).to_string()
            } else if w.len() > 3 && w.ends_with("IES") {
                format!("{}Y", w.get(..w.len() - 3).unwrap_or(w))
            } else if w.len() > 3 && w.ends_with('S') && !w.ends_with("SS") {
                w.get(..w.len() - 1).unwrap_or(w).to_string()
            } else {
                w.to_string()
            }
        })
        .collect();
    format!("_{}_", ws.join("_"))
}

/// The topics `text` names.
pub fn topics(text: &str) -> BTreeSet<&'static str> {
    let w = word_line(text);
    TOPICS.iter().filter(|(_, ws)| ws.iter().any(|x| w.contains(&format!("_{}_", word_line(x).trim_matches('_'))))).map(|(t, _)| *t).collect()
}

/// Is a list entry about `subject` for an item of `kind` (the model's kind words), type `name`
/// and mark `mark`? Their topics meet; a fire alarm detector, an exit sign or a horn is not a
/// plain light or device.
pub fn names_item(subject: &str, kind: &str, name: Option<&str>, mark: Option<&str>) -> bool {
    let want = topics(subject);
    if want.is_empty() {
        return false;
    }
    let own = format!("{} {}", kind.replace('_', " "), name.unwrap_or_default());
    let mut have = topics(&own);
    // Its mark says what it is only when its kind says little (`CR` a card reader among the low
    // voltage outlets; a receptacle marked `TV` stays a receptacle).
    let generic = matches!(kind, "device" | "low_voltage_outlet" | "fixture" | "plumbing_fixture" | "equipment_connection");
    if let Some(m) = mark.filter(|_| generic || have.is_empty()) {
        have.extend(topics(m));
    }
    // A data outlet is no receptacle, though it is an outlet.
    if have.contains("data") && !word_line(&own).contains("_RECEPTACLE_") {
        have.remove("receptacle");
    }
    !want.is_disjoint(&have)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(value: &str, x: f64, y: f64, h: f64) -> TextItem {
        TextItem { value: value.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
    }

    #[test]
    fn heights_as_lists_write_them() {
        let d = dims("STANDARD = 14\u{201D} TO 15\". ADA (TO TOP OF SEAT) = 17\" TO 19\u{201D}.");
        assert_eq!(d.iter().map(|d| d.inches).collect::<Vec<_>>(), vec![14.0, 15.0, 17.0, 19.0]);
        assert!(d.first().is_some_and(|d| d.to_next));
        assert_eq!(pick(&dims("14\" TO 15\"")), Some(14.5));
        assert_eq!(pick(&dims("38\u{201D} MIN., 43\" MAX.")), Some(43.0));
        assert_eq!(pick(&dims("1'-6\" CENTERLINE")), Some(18.0));
        assert_eq!(pick(&dims("4' - 0\" AFF TO TOP")), Some(48.0));
        assert_eq!(pick(&dims("43 3/4\" AFF")), Some(43.75));
        assert_eq!(pick(&dims("MOUNT AT 46 INCHES")), Some(46.0));
        assert_eq!(pick(&dims("1.28 GPF TOILET")), None, "a flow is no height");
        assert_eq!(pick(&dims("4\" SQUARE BOX")), Some(4.0), "a size reads as one; the lists' subjects keep sizes out");
        assert_eq!(Datum::of("4'-0\" AFF TO TOP"), Datum::Top);
        assert_eq!(Datum::of("7'-6\" AFF TO BOTTOM"), Datum::Bottom);
        assert_eq!(Datum::of("1'-6\" CENTERLINE"), Datum::Centre);
        assert_eq!(Datum::of("LAVATORIES (TO SINK RIM/ COUNTERTOP)"), Datum::Top);
        assert_eq!(Datum::of("(TO C.L. OF OUTLET)"), Datum::Centre);
    }

    #[test]
    fn standard_and_accessible_heights() {
        assert_eq!(
            standard_and_ada(
                "WATER CLOSETS (TO TOP OF SEAT): STANDARD = 14\" TO 15\". ADA (TO TOP OF SEAT) = 17\" TO 19\". ADA FLUSH CONTROLS = 44\" MAX."
            ),
            (Some(14.5), Some(18.0), false)
        );
        assert_eq!(
            standard_and_ada("LAVATORIES (TO SINK RIM/ COUNTERTOP): STANDARD = 36\" MAX. ADA = 34\" MAX. (29\" MIN. CLEAR KNEE SPACE)"),
            (Some(36.0), Some(34.0), false)
        );
        assert_eq!(standard_and_ada("COUNTERTOPS (TO SINK RIM/ COUNTERTOP): ADA = 28\" MIN. TO 34\" MAX."), (None, Some(34.0), false));
        assert_eq!(standard_and_ada("SMOKE AND/OR HEAT DETECTORS: STANDARD = CEILING HEIGHT"), (None, None, true));
        assert_eq!(
            standard_and_ada("LIGHT SWITCHES & CARD READERS (TO C.L.): LOCATE 6\" FROM DOOR JAMB. ADA = 48\" MAX."),
            (None, Some(48.0), false)
        );
        assert_eq!(standard_and_ada("MIRRORS (TO B.O. REFLECTIVE SURFACE): STANDARD = VARIES. ADA = 40\" MAX."), (None, Some(40.0), false));
    }

    #[test]
    fn a_general_notes_height_list_and_an_accessibility_list() {
        // A general note listing heights, its values in a column right of the items.
        let h = 1.0;
        let mut texts = vec![
            t("16. SWITCHES AND RECEPTACLES SHALL NOT BE MOUNTED BACK TO BACK.", 0.0, 10.0, h),
            t("17.ELECTRICAL DEVICE MOUNTED HEIGHTS (UNO):", 0.0, 8.0, h),
            t("\u{2022}", 2.0, 6.5, h),
            t("TOGGLE SWITCH", 4.0, 6.5, h),
            t("4'-0\" AFF TO TOP", 23.0, 6.5, h),
            t("\u{2022}", 2.0, 5.0, h),
            t("RECEPTACLES", 4.0, 5.0, h),
            t("1'-6\" CENTERLINE", 23.0, 5.0, h),
            t("18.PROVIDE TYPE-WRITTEN PANELBOARD SCHEDULES.", 0.0, 2.0, h),
            // A keyed note with a height: not a list.
            t("10. 20A/2P RECEPTACLE FOR PTAC IN WALL. MOUNT 18\" AFF.", 100.0, 10.0, h),
            // An accessibility sheet's list, wrapped.
            t("7. WATER CLOSETS (TO TOP OF SEAT):", 200.0, 10.0, h),
            t("STANDARD = 14\u{201D} TO 15\". ADA (TO TOP OF", 202.6, 8.4, h),
            t("SEAT) = 17\" TO 19\u{201D}. ADA FLUSH CONTROLS =", 202.6, 6.8, h),
            t("44\" MAX.", 202.6, 5.2, h),
            t("8. LAVATORIES (TO SINK RIM/ COUNTERTOP):", 200.0, 3.6, h),
            t("STANDARD = 36\" MAX. ADA = 34\" MAX.", 202.6, 2.0, h),
        ];
        // Text far right (a title block) is no part of a note.
        texts.push(t("11500 SOME DRIVE, STE 330", 60.0, 6.5, h));
        let l = read_lists(&texts);
        let by = |s: &str| l.iter().find(|x| x.subject == s).cloned();
        let sw = by("TOGGLE SWITCH").expect("switch row");
        assert_eq!((sw.inches, sw.datum), (Some(48.0), Datum::Top));
        let rec = by("RECEPTACLES").expect("receptacle row");
        assert_eq!((rec.inches, rec.datum), (Some(18.0), Datum::Centre));
        let wc = by("WATER CLOSETS").expect("water closets");
        assert_eq!((wc.inches, wc.ada_inches, wc.datum), (Some(14.5), Some(18.0), Datum::Top));
        let lav = by("LAVATORIES").expect("lavatories");
        assert_eq!((lav.inches, lav.ada_inches, lav.datum), (Some(36.0), Some(34.0), Datum::Top));
        assert!(!l.iter().any(|x| x.text.contains("20A/2P")), "a keyed note is no list: {l:?}");
        assert_eq!(l.len(), 4, "{l:?}");
    }

    #[test]
    fn a_heights_list_in_other_layouts() {
        let h = 2.0;
        let texts = vec![
            // An unnumbered heading, each row one text with leaders.
            t("DEVICE MOUNTING HEIGHTS:", 0.0, 20.0, h),
            t("THERMOSTATS ........ 4'-0\" AFF TO TOP", 0.0, 17.0, h),
            t("RECEPTACLES ........ 18\" AFF", 0.0, 14.0, h),
            // The value left of the item.
            t("+46\"", 0.0, 11.0, h),
            t("DOORBELL", 8.0, 11.0, h),
        ];
        let l = read_lists(&texts);
        let by = |s: &str| l.iter().find(|x| x.subject == s).cloned();
        assert_eq!(by("THERMOSTATS").map(|x| (x.inches, x.datum)), Some((Some(48.0), Datum::Top)), "{l:?}");
        assert_eq!(by("RECEPTACLES").map(|x| x.inches), Some(Some(18.0)), "{l:?}");
        assert_eq!(by("DOORBELL").map(|x| x.inches), Some(Some(46.0)), "{l:?}");
        assert!(names_item("THERMOSTATS", "thermostat", None, Some("T")));
    }

    #[test]
    fn entries_name_items_by_their_words() {
        assert!(names_item("RECEPTACLES", "receptacle", None, Some("REC")));
        assert!(names_item("TOGGLE SWITCH", "switch", Some("switch SD"), Some("SD")));
        assert!(names_item("WATER CLOSETS", "plumbing_fixture", Some("water closet P-1"), Some("P-1")));
        assert!(names_item("LAVATORIES", "plumbing_fixture", Some("lavatory P-2"), Some("P-2")));
        assert!(names_item("COUNTERTOPS", "plumbing_fixture", Some("kitchen sink P-5"), Some("P-5")));
        assert!(!names_item("WATER CLOSETS", "plumbing_fixture", Some("lavatory P-2"), Some("P-2")));
        assert!(!names_item("RECEPTACLES", "low_voltage_outlet", Some("data outlet"), Some("DA")), "a data outlet is no receptacle");
        assert!(names_item("LIGHT SWITCHES & CARD READERS", "low_voltage_outlet", None, Some("CR")));
        assert!(names_item("FIRE ALARM PULL STATIONS", "pull_station", None, Some("P")));
        assert!(!names_item("PANELBOARDS", "receptacle", None, Some("REC")));
    }
}
