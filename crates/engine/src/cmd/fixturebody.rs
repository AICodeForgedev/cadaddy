//! What a built device is, and the body it is built as (Q6315).
//!
//! The build makes every device the readers place as one element. A luminaire (an exit sign, an
//! emergency light) is a `light_fixture` element and a plumbing fixture a `plumbing_fixture`
//! element, not a generic `device`: their schedules, takeoff and the 3D view read those kinds.
//! Which one a device is comes from what it was read as, when it is made: its own kind (its
//! legend row's, its sheet's: `luminaire`, `plumbing_fixture`), or, for a device read only as a
//! device, its mark in the light fixture schedule. The kinds that make a fixture are rules data
//! (`fixturebody.json`, after IFC 4's IfcLightFixture and IfcSanitaryTerminal).
//!
//! A fixture's body is sized from the documents first and says which (`body_provenance`,
//! `body_basis`):
//!
//! 1. **drawn**: the outline the architectural plan draws it as (`drawn_size_in`, the fixture
//!    fit's), set where it is drawn;
//! 2. **derived**: the size its schedule prints (`2X4`, `60X32`: a luminaire's row, a plumbing
//!    fixture's name);
//! 3. **inferred**: its kind's usual size (rules data, trade practice), by the words of its
//!    kind, name and schedule row (`TROFFER`, `WATER CLOSET`).
//!
//! A plumbing fixture of no known kind keeps its rough-in box: no one size fits a hose bibb and a
//! washer box, so none is guessed (its basis says so).

use std::sync::OnceLock;

use serde::Deserialize;

const RULES: &str = include_str!("fixturebody.json");

/// The two kinds of fixture a device can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Light,
    Plumbing,
}

impl Class {
    /// The model element kind of a device of this class.
    pub(crate) fn element_kind(self) -> &'static str {
        match self {
            Class::Light => "light_fixture",
            Class::Plumbing => "plumbing_fixture",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Family {
    pub name: String,
    pub words: Vec<String>,
    pub size_in: [f64; 3],
    pub basis: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    light_kinds: Vec<String>,
    plumbing_kinds: Vec<String>,
    light_families: Vec<Family>,
    light_default: Family,
    light_wall_default: Family,
    plumbing_families: Vec<Family>,
    size_limits_in: [f64; 2],
}

/// The rules data (empty lists when it can't be read: nothing classed, no bodies).
pub(crate) fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// A device kind as the rules list it: lower case, spaces and dashes as underscores.
fn norm(kind: &str) -> String {
    kind.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

/// The class of fixture a device of kind `kind` (as read) is, if it is one.
pub(crate) fn class_of_kind(kind: &str) -> Option<Class> {
    let k = norm(kind);
    if k.is_empty() {
        return None;
    }
    let r = rules();
    if r.light_kinds.iter().any(|x| norm(x) == k) {
        Some(Class::Light)
    } else if r.plumbing_kinds.iter().any(|x| norm(x) == k) {
        Some(Class::Plumbing)
    } else {
        None
    }
}

/// The words of `text`, upper case, split on anything not a letter or digit.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect()
}

/// Whether `have` says `phrase` (whole words; its last word may be plural).
fn says(have: &[String], phrase: &str) -> bool {
    let want = tokens(phrase);
    let n = want.len();
    n > 0
        && have.windows(n).any(|w| {
            w.iter()
                .zip(&want)
                .enumerate()
                .all(|(i, (a, b))| a == b || (i + 1 == n && (a.strip_suffix('S') == Some(b.as_str()) || a.strip_suffix("ES") == Some(b.as_str()))))
        })
}

/// The family of `class` whose words `text` says first.
pub(crate) fn family_of(class: Class, text: &str) -> Option<&'static Family> {
    let have = tokens(text);
    let r = rules();
    let list = match class {
        Class::Light => &r.light_families,
        Class::Plumbing => &r.plumbing_families,
    };
    list.iter().find(|f| f.words.iter().any(|w| says(&have, w)))
}

/// A size printed in `text` (`2X4`, `2'X4'`, `24"X48"`, `60 X 32`): its two sides, inches, and
/// the words as printed. Bare numbers of 8 or less are feet (a luminaire's nominal `2X4`), larger
/// ones inches; a side outside the rules' limits (a lamp count and wattage `2X32W` is refused by
/// the letter after it) is no size.
pub(crate) fn printed_size(text: &str) -> Option<([f64; 2], String)> {
    let chars: Vec<char> = text.chars().take(2_000).collect();
    let [lo, hi] = rules().size_limits_in;
    let is_mark = |c: char| c == '\'' || c == '"';
    for (i, c) in chars.iter().enumerate() {
        if !matches!(c, 'X' | 'x' | '×') {
            continue;
        }
        // The number before it.
        let mut j = i;
        while j > 0 && chars.get(j - 1).is_some_and(|c| *c == ' ') {
            j -= 1;
        }
        let mut left_mark = None;
        if j > 0 && chars.get(j - 1).is_some_and(|c| is_mark(*c)) {
            left_mark = chars.get(j - 1).copied();
            j -= 1;
        }
        let end_a = j;
        while j > 0 && chars.get(j - 1).is_some_and(|c| c.is_ascii_digit() || *c == '.') {
            j -= 1;
        }
        let start_a = j;
        if start_a == end_a || (start_a > 0 && chars.get(start_a - 1).is_some_and(|c| c.is_ascii_alphanumeric())) {
            continue;
        }
        // The number after it.
        let mut k = i + 1;
        while chars.get(k).is_some_and(|c| *c == ' ') {
            k += 1;
        }
        let start_b = k;
        while chars.get(k).is_some_and(|c| c.is_ascii_digit() || *c == '.') {
            k += 1;
        }
        let end_b = k;
        if start_b == end_b {
            continue;
        }
        let right_mark = chars.get(k).copied().filter(|c| is_mark(*c));
        if right_mark.is_some() {
            k += 1;
        }
        if chars.get(k).is_some_and(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let num = |a: usize, b: usize| chars.get(a..b).map(|s| s.iter().collect::<String>()).and_then(|s| s.parse::<f64>().ok());
        let (Some(a), Some(b)) = (num(start_a, end_a), num(start_b, end_b)) else { continue };
        let feet = match (left_mark, right_mark) {
            (Some('\''), _) | (_, Some('\'')) => true,
            (Some('"'), _) | (_, Some('"')) => false,
            _ => a <= 8.0 && b <= 8.0,
        };
        let (a, b) = if feet { (a * 12.0, b * 12.0) } else { (a, b) };
        if [a, b].iter().all(|v| v.is_finite() && *v >= lo && *v <= hi) {
            let said: String = chars.get(start_a..k).map(|s| s.iter().collect()).unwrap_or_default();
            return Some(([a, b], said.trim().to_string()));
        }
    }
    None
}

/// The body a fixture is built as: its size (inches: width along its wall, depth out from it,
/// height), where it came from (`drawn`, `derived`, `inferred`) and why. (Its height above the
/// floor stays its mounting height's, the documents' first.)
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Body {
    pub size_in: [f64; 3],
    pub provenance: &'static str,
    pub basis: String,
}

/// What a fixture is built as.
pub(crate) struct Ask<'a> {
    pub class: Class,
    /// Its kind, name, type and schedule words (`WATER CLOSET, FLOOR MOUNTED`).
    pub words: &'a str,
    /// What its schedule row prints (a luminaire's description and model; a plumbing fixture's
    /// name).
    pub schedule: &'a str,
    /// Its drawn outline's plan size (inches, along plan x and y), the fixture fit's.
    pub drawn_in: Option<[f64; 2]>,
    /// Set in a wall's face.
    pub on_wall: bool,
}

/// The drawn size a fixture fit recorded (`21, 30`: inches along plan x and y).
pub(crate) fn drawn_size(text: &str) -> Option<[f64; 2]> {
    let [lo, hi] = rules().size_limits_in;
    let v: Vec<f64> = text.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
    match v.as_slice() {
        [a, b] if [*a, *b].iter().all(|x| x.is_finite() && *x >= lo && *x <= hi) => Some([*a, *b]),
        _ => None,
    }
}

/// The body of the fixture `ask` describes; `None`: it keeps its rough-in box (a plumbing fixture
/// of no known kind).
pub(crate) fn body(ask: &Ask) -> Option<Body> {
    let all = format!("{} {}", ask.words, ask.schedule);
    let family = family_of(ask.class, &all);
    let r = rules();
    let fallback = match ask.class {
        Class::Light if ask.on_wall => Some(&r.light_wall_default),
        Class::Light => Some(&r.light_default),
        Class::Plumbing => None,
    };
    let kind = family.or(fallback);
    let height = kind.map_or(6.0, |f| f.size_in[2]);
    if let Some([x, y]) = ask.drawn_in {
        return Some(Body {
            size_in: [x, y, height],
            provenance: "drawn",
            basis: format!(
                "its outline as the architectural plan draws it, {x:.0} by {y:.0} in{}",
                kind.map(|f| format!("; its height a {}'s ({})", f.name, f.basis)).unwrap_or_default()
            ),
        });
    }
    if let Some(([a, b], said)) = printed_size(ask.schedule) {
        // A wall fixture's longer side runs along its wall, unless its kind's usual body is
        // deeper than wide (a tub is long along its wall; a water closet deep).
        let deep = kind.is_some_and(|f| f.size_in[1] > f.size_in[0]);
        let (w, d) = if deep == (a > b) { (b, a) } else { (a, b) };
        let (w, d) = if !ask.on_wall && ask.class == Class::Light { (a.min(b), a.max(b)) } else { (w, d) };
        return Some(Body {
            size_in: [w, d, height],
            provenance: "derived",
            basis: format!(
                "the size its schedule prints, {said}{}",
                kind.map(|f| format!("; its height a {}'s ({})", f.name, f.basis)).unwrap_or_default()
            ),
        });
    }
    let f = kind?;
    (f.size_in.iter().all(|v| v.is_finite() && *v > 0.0)).then(|| Body {
        size_in: f.size_in,
        provenance: "inferred",
        basis: format!("no size drawn or printed: a {}'s usual size, {}", f.name, f.basis),
    })
}

/// Why a plumbing fixture of no known kind keeps its rough-in box.
pub(crate) const NO_BODY: &str = "no size drawn or printed and no usual size for its kind (rules data): built as its rough-in box";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_as_read_make_fixtures() {
        assert_eq!(class_of_kind("luminaire"), Some(Class::Light));
        assert_eq!(class_of_kind("wall luminaire"), Some(Class::Light));
        assert_eq!(class_of_kind("Exit_Sign"), Some(Class::Light));
        assert_eq!(class_of_kind("plumbing fixture"), Some(Class::Plumbing));
        assert_eq!(class_of_kind("water_closet"), Some(Class::Plumbing));
        for k in ["", "device", "receptacle", "switch", "equipment connection", "water_heater", "fire alarm device"] {
            assert_eq!(class_of_kind(k), None, "{k}");
        }
        assert_eq!(Class::Light.element_kind(), "light_fixture");
        assert_eq!(Class::Plumbing.element_kind(), "plumbing_fixture");
    }

    #[test]
    fn printed_sizes_are_read_and_lamps_and_models_are_not() {
        let s = |t: &str| printed_size(t).map(|(v, _)| v);
        assert_eq!(s("2X4 LED TROFFER"), Some([24.0, 48.0]));
        assert_eq!(s("2'X2' FLAT PANEL"), Some([24.0, 24.0]));
        assert_eq!(s("24\" x 48\" panel"), Some([24.0, 48.0]));
        assert_eq!(s("BATHTUB 60 X 32"), Some([60.0, 32.0]));
        assert_eq!(s("1X4 WRAP"), Some([12.0, 48.0]));
        for t in ["(2) 2X32W T8", "MODEL K2X4", "4X 1000", "X4", "2X", "2X400", "0X0", "TROFFER"] {
            assert_eq!(s(t), None, "{t}");
        }
    }

    #[test]
    fn families_by_whole_words() {
        let name = |c, t: &str| family_of(c, t).map(|f| f.name.as_str());
        assert_eq!(name(Class::Plumbing, "WATER CLOSETS, FLOOR MOUNTED"), Some("water closet"));
        assert_eq!(name(Class::Plumbing, "TUB/SHOWER COMBINATION"), Some("tub"));
        assert_eq!(name(Class::Plumbing, "SHOWER DRAIN"), Some("floor drain"));
        assert_eq!(name(Class::Plumbing, "COPPER TUBING"), None);
        assert_eq!(name(Class::Light, "RECESSED DOWNLIGHT"), Some("downlight"));
        assert_eq!(name(Class::Light, "EDGE LIT EXIT SIGN"), Some("exit sign"));
        assert_eq!(name(Class::Light, "CANOPY"), None);
    }

    #[test]
    fn bodies_drawn_then_printed_then_usual() {
        let ask = |class, words, schedule, drawn_in, on_wall| Ask { class, words, schedule, drawn_in, on_wall };
        let b = body(&ask(Class::Plumbing, "WATER CLOSET", "", Some([21.0, 30.0]), true)).unwrap();
        assert_eq!((b.size_in, b.provenance), ([21.0, 30.0, 30.0], "drawn"));
        let b = body(&ask(Class::Plumbing, "", "BATHTUB 60X32", None, true)).unwrap();
        assert_eq!((b.size_in, b.provenance), ([60.0, 32.0, 20.0], "derived"));
        assert!(b.basis.contains("60X32"), "{}", b.basis);
        let b = body(&ask(Class::Plumbing, "WATER CLOSET", "", None, true)).unwrap();
        assert_eq!((b.size_in, b.provenance), ([20.0, 28.0, 30.0], "inferred"));
        let b = body(&ask(Class::Plumbing, "LAVATORY", "", None, true)).unwrap();
        assert_eq!(b.size_in, [20.0, 18.0, 8.0]);
        assert!(body(&ask(Class::Plumbing, "HOSE BIBB", "", None, true)).is_none());
        let b = body(&ask(Class::Light, "luminaire", "2X4 TROFFER", None, false)).unwrap();
        assert_eq!((b.size_in, b.provenance), ([24.0, 48.0, 5.0], "derived"));
        let b = body(&ask(Class::Light, "luminaire", "", None, false)).unwrap();
        assert_eq!((b.size_in, b.provenance), ([12.0, 12.0, 6.0], "inferred"));
        let b = body(&ask(Class::Light, "wall luminaire", "", None, true)).unwrap();
        assert_eq!(b.size_in, [6.0, 6.0, 10.0]);
    }

    #[test]
    fn hostile_text_never_panics() {
        for t in ["", "X", "XXXX", "9999999999999999999X9", "1.2.3X4", "'X'", "\"x\"", "××××", "2 X 4 X 6", &"9X".repeat(5000)] {
            let _ = printed_size(t);
            let _ = family_of(Class::Light, t);
            let _ = drawn_size(t);
        }
        assert_eq!(drawn_size("21, 30"), Some([21.0, 30.0]));
        assert_eq!(drawn_size("NaN, 3"), None);
        assert_eq!(drawn_size("1e300, 3"), None);
    }
}
