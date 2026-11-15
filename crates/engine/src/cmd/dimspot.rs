//! Where a dimensioned enlarged plan puts its devices (Q6605): a view that dimensions a device's
//! symbol from a wall face, an opening edge or another fixed line gives the device's spot by the
//! dimension's value, not by where the symbol happens to be drawn. The drafter's number is the
//! document; the symbol's drawn place is a picture of it (often a few inches off, and off by
//! more where a value is marked not to scale).
//!
//! [`dimensioned_spots`] takes the dimension strings a view draws ([`super::dimreader::read`])
//! and the symbols the view's devices stand on (their spots, view coordinates), and gives each
//! symbol a dimension chain measures the spot the chain's values put it at:
//!
//! - A chain's witness positions are laid out by its values from its first witness; a witness
//!   whose extension line ends on a symbol (or runs through one) is that symbol's; one ending on
//!   no symbol is a fixed end (a wall face, an opening edge, a casework edge).
//! - Each symbol in a chain is measured from the chain's nearest fixed end: its spot along the
//!   dimension is that end's place plus the values between them. Across the dimension it stays.
//! - A symbol two chains measure in two directions (along a wall and up from the floor of the
//!   plan) takes both; of two chains in one direction, the one with the nearer fixed end.
//! - A correction larger than `max_move` is left alone and counted (a value read onto the wrong
//!   chain is likelier than a drafter's symbol that far off).
//!
//! General: no job values; tolerances are in the dimensions' text heights, so any scale reads.
//! Every scan is capped; nothing here panics.

use std::collections::BTreeMap;

use cadaddy_geom::Vec2;

use super::dimreader::{DimString, Witness};

/// Dimension strings and symbols looked at, at most.
const MAX_STRINGS: usize = 20_000;
const MAX_SPOTS: usize = 20_000;
/// A symbol is at a witness when its middle is this near the witness line (text heights, along
/// the dimension) and this near the witness's end (text heights).
const ON_LINE_H: f64 = 0.6;
const AT_END_H: f64 = 6.0;
/// Two witnesses of one chain this close along it (text heights) are one.
const SAME_WITNESS_H: f64 = 0.4;
/// A spot moved less than this (inches) is already at its dimensioned spot.
const SETTLED_IN: f64 = 0.25;

/// One symbol's dimensioned spot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DimSpot {
    /// Which of the spots given.
    pub spot: usize,
    /// Where the dimensions put it (view coordinates).
    pub to: Vec2,
    /// How far it moved (view units).
    pub moved: f64,
    /// The dimensions that put it there, as said.
    pub why: String,
}

/// What [`dimensioned_spots`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Found {
    pub spots: Vec<DimSpot>,
    /// Symbols a chain measures that already stand at their dimensioned spot.
    pub settled: usize,
    /// Symbols whose dimensioned spot is farther than the cap: left where drawn.
    pub too_far: usize,
}

/// A length as a drawing writes it (`3'-4 1/2"`), to the nearest eighth.
fn ft_in(inches: f64) -> String {
    let e = (inches.abs() * 8.0).round() as i64;
    let (ft, rest) = (e / 96, e % 96);
    let (whole, eighths) = (rest / 8, rest % 8);
    let frac = match eighths {
        0 => String::new(),
        n => {
            let (mut a, mut b) = (n, 8);
            while a % 2 == 0 {
                a /= 2;
                b /= 2;
            }
            format!(" {a}/{b}")
        }
    };
    if ft > 0 { format!("{ft}'-{whole}{frac}\"") } else { format!("{whole}{frac}\"") }
}

/// The spot standing at witness `w` (of a dimension along `u`, text height `h`): the one its
/// symbol is, else the nearest on its witness line near its end.
fn spot_at(w: &Witness, u: Vec2, h: f64, spots: &[Vec2]) -> Option<usize> {
    let near = |p: Vec2, r: f64| {
        spots.iter().enumerate().filter(|(_, s)| s.is_finite() && s.dist(p) <= r).min_by(|a, b| a.1.dist(p).total_cmp(&b.1.dist(p))).map(|(i, _)| i)
    };
    if let Some(sym) = w.symbol
        && let Some(i) = near(sym, 1.5 * h)
    {
        return Some(i);
    }
    spots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_finite() && (**s - w.on_line).dot(u).abs() <= ON_LINE_H * h && s.dist(w.point) <= AT_END_H * h)
        .min_by(|a, b| a.1.dist(w.point).total_cmp(&b.1.dist(w.point)))
        .map(|(i, _)| i)
}

/// One witness position of a chain: where it is along the chain (view units, from the chain's
/// line), its place by the values (inches from the first), the spot at it, and its run (strings
/// joined end to end).
struct Pos {
    along: f64,
    inches: f64,
    spot: Option<usize>,
    /// A symbol stands at it (a device's, known or not): never a fixed end.
    symbol: bool,
    run: usize,
}

/// Each spot's dimensioned spot (see the module docs). `units_per_inch`: the view's units per
/// inch of the building (a typical-room view is laid at the model's scale); `max_move_in`: the
/// largest correction taken, inches.
pub(crate) fn dimensioned_spots(strings: &[DimString], spots: &[Vec2], units_per_inch: f64, max_move_in: f64) -> Found {
    let mut out = Found::default();
    if !(units_per_inch.is_finite() && units_per_inch > 0.0) || spots.is_empty() {
        return out;
    }
    let spots = spots.get(..spots.len().min(MAX_SPOTS)).unwrap_or(&[]);
    let mut chains: BTreeMap<usize, Vec<&DimString>> = BTreeMap::new();
    for s in strings.iter().take(MAX_STRINGS) {
        if s.inches.is_finite() && s.inches > 0.0 && s.dir.is_finite() && s.dir.len() > 0.5 {
            chains.entry(s.chain).or_default().push(s);
        }
    }
    // Per spot, per direction (its unit, the inches from the fixed end): the candidates.
    type Cand = (Vec2, f64, f64, String);
    let mut cands: BTreeMap<usize, Vec<Cand>> = BTreeMap::new();
    for (_, mut ch) in chains {
        ch.sort_by_key(|s| s.index);
        let Some(first) = ch.first() else { continue };
        let u = first.dir.normalized();
        let h = ch.iter().map(|s| s.h).fold(0.0, f64::max).max(1e-9);
        let mut pos: Vec<Pos> = Vec::new();
        let find = |pos: &[Pos], along: f64| pos.iter().position(|p| (p.along - along).abs() <= SAME_WITNESS_H * h);
        let mut runs = 0usize;
        for s in &ch {
            let (a, b) = (&s.ends.0, &s.ends.1);
            let (ta, tb) = (a.on_line.dot(u), b.on_line.dot(u));
            let sign = if tb >= ta { 1.0 } else { -1.0 };
            let ia = match find(&pos, ta) {
                Some(i) => i,
                None => {
                    runs += 1;
                    pos.push(Pos { along: ta, inches: 0.0, spot: spot_at(a, u, h, spots), symbol: a.symbol.is_some(), run: runs });
                    pos.len() - 1
                }
            };
            let Some((base, run)) = pos.get(ia).map(|p| (p.inches, p.run)) else { continue };
            if find(&pos, tb).is_none() {
                pos.push(Pos { along: tb, inches: base + sign * s.inches, spot: spot_at(b, u, h, spots), symbol: b.symbol.is_some(), run });
            }
        }
        for p in pos.iter() {
            let Some(si) = p.spot else { continue };
            // The nearest fixed end of its run, by the values.
            let Some(fixed) = pos
                .iter()
                .filter(|f| f.run == p.run && f.spot.is_none() && !f.symbol)
                .min_by(|x, y| (x.inches - p.inches).abs().total_cmp(&(y.inches - p.inches).abs()))
            else {
                continue;
            };
            let Some(spot) = spots.get(si) else { continue };
            let said = p.inches - fixed.inches;
            let target = fixed.along + said * units_per_inch;
            let delta = target - spot.dot(u);
            let why = format!("dimensioned {} from the fixed end of its dimension string", ft_in(said));
            cands.entry(si).or_default().push((u, delta, said.abs(), why));
        }
    }
    for (si, list) in cands {
        let Some(spot) = spots.get(si).copied() else { continue };
        // One per direction: the nearest fixed end's.
        let mut picked: Vec<&Cand> = Vec::new();
        for c in &list {
            match picked.iter().position(|p| p.0.dot(c.0).abs() > 0.9) {
                Some(k) => {
                    if picked.get(k).is_some_and(|p| c.2 < p.2)
                        && let Some(slot) = picked.get_mut(k)
                    {
                        *slot = c;
                    }
                }
                None => picked.push(c),
            }
        }
        let shift = picked.iter().fold(Vec2::ZERO, |acc, c| acc + c.0 * c.1);
        let moved = shift.len();
        if !moved.is_finite() {
            continue;
        }
        if moved / units_per_inch < SETTLED_IN {
            out.settled += 1;
            continue;
        }
        if moved / units_per_inch > max_move_in {
            out.too_far += 1;
            continue;
        }
        let why = picked.iter().map(|c| c.3.as_str()).collect::<Vec<_>>().join("; ");
        out.spots.push(DimSpot { spot: si, to: spot + shift, moved, why: format!("{why} (drawn {} off it)", ft_in(moved / units_per_inch)) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::dimreader::{Items, read};
    use super::*;
    use cadaddy_doc::HAlign;

    /// A view at full scale (1 unit = 1 in): a wall face at x = 0, a dimension line at y = 80
    /// with ticks, from the wall to a round device symbol drawn at x = 30 but dimensioned 3'-0",
    /// then on to a second symbol drawn at x = 60, dimensioned 2'-6" on from the first (the
    /// extension lines run to the symbols as drawn; the values say where they go).
    fn view(first: &str, second: &str) -> Items {
        let mut it = Items::default();
        let y = 80.0;
        it.add_path(&[Vec2::new(-2.0, y), Vec2::new(70.0, y)]);
        for x in [0.0, 30.0, 60.0] {
            it.add_path(&[Vec2::new(x - 1.0, y - 1.0), Vec2::new(x, y), Vec2::new(x + 1.0, y + 1.0)]);
        }
        // The wall's extension line down to the wall; the devices' down to their symbols.
        it.add_path(&[Vec2::new(0.0, y + 2.0), Vec2::new(0.0, 4.0)]);
        it.add_path(&[Vec2::new(30.0, y + 2.0), Vec2::new(30.0, 14.0)]);
        it.add_path(&[Vec2::new(60.0, y + 2.0), Vec2::new(60.0, 14.0)]);
        it.add_text(first, Vec2::new(10.0, y + 1.0), None, HAlign::Left, 3.0, 0.0, 1.0);
        it.add_text(second, Vec2::new(40.0, y + 1.0), None, HAlign::Left, 3.0, 0.0, 1.0);
        it
    }

    #[test]
    fn a_symbol_dimensioned_from_a_wall_takes_the_dimensions_spot_not_where_it_is_drawn() {
        let strings = read(&view("3'-0\"", "2'-6\""), Some(1.0)).strings;
        assert_eq!(strings.len(), 2, "{strings:?}");
        // The symbols: drawn 6 in short of their dimensioned spots (x = 36, 66), and one the
        // chain doesn't measure.
        let spots = [Vec2::new(30.0, 10.0), Vec2::new(60.0, 10.0), Vec2::new(10.0, 40.0)];
        let f = dimensioned_spots(&strings, &spots, 1.0, 24.0);
        let mut got: Vec<(usize, f64, f64)> = f.spots.iter().map(|d| (d.spot, (d.to.x * 100.0).round() / 100.0, d.to.y)).collect();
        got.sort_by_key(|g| g.0);
        assert_eq!(got, vec![(0, 36.0, 10.0), (1, 66.0, 10.0)], "{f:?}");
        assert!(f.spots.iter().any(|d| d.spot == 0 && d.why.contains("3'-0\"")), "{f:?}");
        assert!(f.spots.iter().any(|d| d.spot == 1 && d.why.contains("5'-6\"")), "{f:?}");
    }

    #[test]
    fn a_symbol_at_its_spot_settles_and_one_too_far_off_is_left() {
        let strings = read(&view("2'-6\"", "2'-6\""), Some(1.0)).strings;
        let spots = [Vec2::new(30.0, 10.0), Vec2::new(60.0, 10.0)];
        let f = dimensioned_spots(&strings, &spots, 1.0, 24.0);
        assert_eq!((f.spots.len(), f.settled, f.too_far), (0, 2, 0), "{f:?}");
        // Read at ten units an inch: the dimensioned spots are feet off the symbols: left.
        let f = dimensioned_spots(&strings, &spots, 10.0, 24.0);
        assert_eq!((f.spots.len(), f.too_far), (0, 2), "{f:?}");
        // Hostile: no scale, no spots.
        assert_eq!(dimensioned_spots(&strings, &spots, f64::NAN, 24.0), Found::default());
        assert_eq!(dimensioned_spots(&strings, &[], 1.0, 24.0), Found::default());
    }

    #[test]
    fn lengths_are_written_as_a_drawing_writes_them() {
        assert_eq!(ft_in(40.5), "3'-4 1/2\"");
        assert_eq!(ft_in(6.0), "6\"");
        assert_eq!(ft_in(-30.0), "2'-6\"");
    }
}
