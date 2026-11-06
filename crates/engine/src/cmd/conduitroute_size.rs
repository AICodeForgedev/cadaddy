//! Branch conductor sizing (Q6234): each branch circuit's conductors are the smallest its
//! breaker allows (NEC 240.4, Table 310.16) unless the documents give a size (the plan's, the
//! panel schedule's wire column) or a voltage-drop or derating calculation needs more. Every
//! upsizing names what drove it, with the number.
//!
//! - **The circuit's supply** ([`supply`]): the voltage drop is worked at the circuit's own
//!   voltage. A one-pole circuit is line to neutral, a two-pole one line to line (single
//!   phase), a three-pole one line to line three-phase (√3), from its panel's `volts/phase`
//!   header (`208Y/120V`), else the rules' defaults. The schedule's VA is its current at that
//!   voltage.
//! - **The load where it is** ([`moment_ft`]): the homerun carries the circuit's whole current;
//!   past the first box each device carries its own share, so the drop to the farthest device
//!   is each stretch's length at the share of the load beyond it (the load moment), not the
//!   whole load at the end of the whole length.
//! - **The homerun first** ([`for_drop`]): where the drop is over the limit, the trade upsizes
//!   the homerun (the stretch that carries all of it) before the circuit's legs; the legs go
//!   up only when the homerun alone, at most the rules' steps over them, can't hold the drop.
//! - **The breaker's minimum** ([`breaker_minimum`]): a circuit's conductors are never smaller
//!   than its breaker allows (240.4(D) for the small sizes, 240.4(B) for the next standard
//!   rating), in its legs as well as its homerun.

use super::{RouteRules, allowed_a};

/// Why a circuit's conductors are the size they are, for the report's count.
pub(crate) const BREAKER: &str = "breaker minimum";
pub(crate) const DROP_HOME: &str = "voltage drop: homerun";
pub(crate) const DROP_CIRCUIT: &str = "voltage drop: whole circuit";
pub(crate) const LEGEND: &str = "the drawing's length table";
pub(crate) const DERATE: &str = "derating (310.15(C)(1))";
pub(crate) const SCHEDULE: &str = "the panel schedule's wire size";

/// Most devices one circuit's load moment walks (hostile models).
const MAX_NODES: usize = 4096;

/// The supply a branch circuit's voltage drop is worked on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Supply {
    /// The voltage the drop is a percentage of (line to neutral for one pole, line to line
    /// for more).
    pub volts: f64,
    /// 2 for a single-phase circuit (out and back), √3 for a three-phase one.
    pub factor: f64,
    pub poles: u32,
    pub source: String,
}

impl Supply {
    /// The current `va` draws on this supply.
    pub(crate) fn amps(&self, va: f64) -> f64 {
        let three = (self.factor - 3f64.sqrt()).abs() < 1e-6;
        va / (self.volts.max(1.0) * if three { 3f64.sqrt() } else { 1.0 })
    }
}

/// A panel's `volts/phase` text (`208Y/120V, 3PH, 4W`, `120/240V 1PH 3W`, `480Y/277`): its line
/// to neutral and its line to line voltage, each when it says it.
pub(crate) fn system_volts(text: &str) -> (Option<f64>, Option<f64>) {
    let up = text.to_ascii_uppercase();
    // The first group of numbers joined by `/` (a `Y` between them is the wye's mark).
    let mut nums: Vec<f64> = Vec::new();
    let mut cur = String::new();
    let mut joined = true;
    for ch in up.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || (ch == '.' && !cur.is_empty()) {
            cur.push(ch);
            continue;
        }
        if !cur.is_empty() {
            if let Ok(v) = cur.parse::<f64>()
                && (100.0..=1000.0).contains(&v)
                && joined
                && nums.len() < 2
            {
                nums.push(v);
            }
            cur.clear();
            joined = matches!(ch, '/' | 'Y');
            if !joined && !nums.is_empty() {
                break;
            }
        } else if !matches!(ch, '/' | 'Y' | 'V') {
            joined = nums.is_empty();
        }
    }
    match nums.as_slice() {
        [a, b] => (Some(a.min(*b)), Some(a.max(*b))),
        // One voltage: a 120 V two-wire panel is line to neutral; a higher one is line to line.
        [v] if *v <= 150.0 => (Some(*v), None),
        [v] => (None, Some(*v)),
        _ => (None, None),
    }
}

/// The supply of a circuit of `poles` on a panel whose `volts/phase` reads `header` (none: not
/// read): one pole line to neutral, two poles line to line, three poles three-phase.
pub(crate) fn supply(poles: u32, header: Option<&str>, panel: &str, rl: &RouteRules) -> Supply {
    let v = &rl.vdrop;
    let (ln, ll) = header.map(system_volts).unwrap_or((None, None));
    let poles = poles.clamp(1, 3);
    let (volts, given) = match poles {
        1 => ln.map_or((v.voltage, false), |x| (x, true)),
        _ => ll.map_or((v.voltage_ll, false), |x| (x, true)),
    };
    let factor = if poles >= 3 { 3f64.sqrt() } else { 2.0 };
    let what = match poles {
        1 => "1-pole, line to neutral",
        2 => "2-pole, line to line",
        _ => "3-pole, three-phase",
    };
    let source = if given {
        format!("panel {panel} {}: {volts:.0} V ({what})", header.unwrap_or_default().trim())
    } else {
        format!("{volts:.0} V ({what}; the panel's voltage not read: {})", v.voltage_source)
    };
    Supply { volts, factor, poles, source }
}

/// The voltage drop in percent over `ft` one way at `amps` on copper `size`, on `sup`.
pub(crate) fn drop_pct(size: &str, ft: f64, amps: f64, sup: &Supply, rl: &RouteRules) -> Option<f64> {
    let cm = rl.vdrop.cmil.get(size).copied().filter(|c| *c > 0.0)?;
    (sup.volts > 0.0).then(|| sup.factor * rl.vdrop.k_copper * amps * ft.max(0.0) / cm / sup.volts * 100.0)
}

/// A circuit's length past its first box as its whole current would see it (its load moment
/// over that current): the most, over its devices, of each stretch on the way to the device
/// times the share of the load beyond the stretch. `edges`: the circuit's joints (device
/// index, device index, ft); `share`: each device's part of the load (all zero: equal parts).
pub(crate) fn moment_ft(n: usize, root: usize, edges: &[(usize, usize, f64)], share: &[f64]) -> f64 {
    let n = n.min(MAX_NODES);
    if n == 0 || root >= n {
        return 0.0;
    }
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for (u, v, ft) in edges {
        let ft = if ft.is_finite() { ft.max(0.0) } else { 0.0 };
        if *u < n && *v < n && u != v {
            if let Some(a) = adj.get_mut(*u) {
                a.push((*v, ft));
            }
            if let Some(a) = adj.get_mut(*v) {
                a.push((*u, ft));
            }
        }
    }
    // The tree from the first box, breadth first (a joint back to a reached device is ignored).
    let mut parent: Vec<Option<(usize, f64)>> = vec![None; n];
    let mut seen = vec![false; n];
    let mut order = vec![root];
    if let Some(s) = seen.get_mut(root) {
        *s = true;
    }
    let mut i = 0;
    while let Some(&u) = order.get(i) {
        i += 1;
        for &(v, ft) in adj.get(u).map(Vec::as_slice).unwrap_or_default() {
            if seen.get(v).copied().unwrap_or(true) {
                continue;
            }
            if let Some(s) = seen.get_mut(v) {
                *s = true;
            }
            if let Some(p) = parent.get_mut(v) {
                *p = Some((u, ft));
            }
            order.push(v);
        }
    }
    let given: f64 = order.iter().map(|i| share.get(*i).copied().filter(|s| s.is_finite() && *s > 0.0).unwrap_or(0.0)).sum();
    let part = |i: usize| {
        if given > 0.0 { share.get(i).copied().filter(|s| s.is_finite() && *s > 0.0).unwrap_or(0.0) / given } else { 1.0 / order.len() as f64 }
    };
    // Each device's load and all beyond it, from the far ends in.
    let mut beyond = vec![0.0_f64; n];
    for &v in order.iter().rev() {
        let here = beyond.get(v).copied().unwrap_or(0.0) + part(v);
        if let Some(b) = beyond.get_mut(v) {
            *b = here;
        }
        if let Some(Some((p, _))) = parent.get(v)
            && let Some(b) = beyond.get_mut(*p)
        {
            *b += here;
        }
    }
    let mut at = vec![0.0_f64; n];
    let mut most = 0.0_f64;
    for &v in order.iter().skip(1) {
        let Some(Some((p, ft))) = parent.get(v).copied() else { continue };
        let m = at.get(p).copied().unwrap_or(0.0) + beyond.get(v).copied().unwrap_or(0.0) * ft;
        if let Some(a) = at.get_mut(v) {
            *a = m;
        }
        most = most.max(m);
    }
    most
}

/// The NEC 240.6(A) standard rating at or over `a` (none: over the table).
fn standard_at_least(a: f64, rl: &RouteRules) -> Option<f64> {
    rl.derate.standard_ratings.iter().copied().find(|r| *r + 1e-9 >= a)
}

/// Whether `size` copper with `ccc` current-carrying conductors in its raceway may be on a
/// `trip` amp breaker: its allowed current (the adjusted 90 °C column, never above the 75 °C
/// one or 240.4(D)) at least the breaker's, or (240.4(B), not the 240.4(D) sizes, up to 800 A)
/// the next standard rating over it is the breaker's.
pub(crate) fn carries(size: &str, ccc: u32, trip: f64, rl: &RouteRules) -> bool {
    let a = allowed_a(size, ccc, rl);
    if a + 1e-9 >= trip {
        return true;
    }
    let small = rl.derate.small_conductor_max_a.contains_key(size);
    !small && trip <= 800.0 && a > 0.0 && standard_at_least(a, rl).is_some_and(|r| r + 1e-9 >= trip)
}

/// The size `base` goes up to for a `trip` amp breaker with `ccc` current-carrying conductors
/// (none: `base` already carries it, or the breaker is unknown).
pub(crate) fn breaker_minimum(base: &str, ccc: u32, trip: Option<f64>, rl: &RouteRules) -> Option<(String, String)> {
    let trip = trip.filter(|t| t.is_finite() && *t > 0.0)?;
    if carries(base, ccc, trip, rl) {
        return None;
    }
    let sizes = &rl.vdrop.sizes;
    let from = sizes.iter().position(|s| s == base)?;
    sizes.iter().skip(from + 1).find(|s| carries(s, ccc, trip, rl)).map(|s| {
        let why = format!(
            "{BREAKER}: a {trip:.0} A breaker needs #{s} ({:.0} A allowed; #{base} {:.0} A): {}; {}",
            allowed_a(s, ccc, rl),
            allowed_a(base, ccc, rl),
            rl.derate.small_conductor_source,
            rl.derate.source
        );
        (s.clone(), why)
    })
}

/// What a voltage-drop check sized a circuit to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DropSized {
    /// The homerun's conductors and the legs' (past its first box).
    pub home: String,
    pub legs: String,
    /// The drop at the given size and as sized, percent.
    pub before_pct: f64,
    pub after_pct: f64,
    /// [`DROP_HOME`], [`DROP_CIRCUIT`], or empty: the given size holds the drop.
    pub reason: &'static str,
    pub why: String,
}

/// The sizes a circuit of `base` takes for the drop over `home_ft` at its whole current and
/// `rest_ft` (its load moment's length past the homerun) at `amps` on `sup`: the given size
/// if it holds the rules' limit, else the smallest legs and, for them, the smallest homerun
/// that hold it (the homerun first: the legs stay as given while a larger homerun alone is
/// enough). None: `base` is not in the table.
pub(crate) fn for_drop(base: &str, home_ft: f64, rest_ft: f64, amps: f64, sup: &Supply, rl: &RouteRules) -> Option<DropSized> {
    let v = &rl.vdrop;
    let sizes: Vec<&String> = v.sizes.iter().filter(|s| v.cmil.contains_key(*s)).collect();
    let from = sizes.iter().position(|s| *s == base)?;
    let pct = |h: &str, l: &str| drop_pct(h, home_ft, amps, sup, rl).unwrap_or(0.0) + drop_pct(l, rest_ft, amps, sup, rl).unwrap_or(0.0);
    let before = pct(base, base);
    let head =
        format!("{home_ft:.0} ft homerun at {amps:.1} A + {rest_ft:.0} ft past it (the load where it is: {}), {}", v.moment_source, sup.source);
    if before <= v.max_pct + 1e-9 {
        return Some(DropSized {
            home: base.to_string(),
            legs: base.to_string(),
            before_pct: before,
            after_pct: before,
            reason: "",
            why: format!("{before:.1}% on #{base}: {head}"),
        });
    }
    for (li, l) in sizes.iter().enumerate().skip(from) {
        // The homerun at most the rules' steps over the legs (the trade's "#10 homerun").
        let steps = if v.homerun_first { v.homerun_steps } else { 0 };
        for h in sizes.iter().skip(li).take(steps + 1) {
            let p = pct(h, l);
            if p <= v.max_pct + 1e-9 {
                let (reason, what) = if l.as_str() == base {
                    (DROP_HOME, format!("#{h} homerun, #{l} past it"))
                } else {
                    (DROP_CIRCUIT, format!("#{h} homerun, #{l} legs"))
                };
                return Some(DropSized {
                    home: (*h).clone(),
                    legs: (*l).clone(),
                    before_pct: before,
                    after_pct: p,
                    reason,
                    why: format!(
                        "{reason}: {before:.1}% on #{base} over {:.0}%; {what}: {p:.1}% ({head}; {}; {})",
                        v.max_pct,
                        v.source,
                        if reason == DROP_HOME { v.homerun_first_source.as_str() } else { "" }
                    ),
                });
            }
        }
    }
    // Nothing in the table holds it: the largest, and the engineer confirms.
    let last = sizes.last().map_or_else(|| base.to_string(), |s| (*s).clone());
    let p = pct(&last, &last);
    Some(DropSized {
        home: last.clone(),
        legs: last.clone(),
        before_pct: before,
        after_pct: p,
        reason: DROP_CIRCUIT,
        why: format!("{DROP_CIRCUIT}: {before:.1}% on #{base}; even #{last} is {p:.1}%: the engineer confirms ({head}; {})", v.source),
    })
}

#[cfg(test)]
mod tests {
    use super::super::rules;
    use super::*;

    #[test]
    fn a_panels_volts_phase_gives_its_line_to_neutral_and_line_to_line_voltages() {
        assert_eq!(system_volts("208Y/120V, 3PH, 4W"), (Some(120.0), Some(208.0)));
        assert_eq!(system_volts("120/208V 3 PH"), (Some(120.0), Some(208.0)));
        assert_eq!(system_volts("120/240V, 1PH, 3W"), (Some(120.0), Some(240.0)));
        assert_eq!(system_volts("480Y/277"), (Some(277.0), Some(480.0)));
        assert_eq!(system_volts("480V, 3PH, 3W"), (None, Some(480.0)));
        assert_eq!(system_volts("120V 1PH 2W"), (Some(120.0), None));
        // Bus amps and phases are not voltages.
        assert_eq!(system_volts("3PH 4W"), (None, None));
        assert_eq!(system_volts(""), (None, None));
    }

    #[test]
    fn a_circuit_s_poles_choose_its_voltage_and_its_current() {
        let rl = rules();
        let one = supply(1, Some("208Y/120V, 3PH, 4W"), "P", rl);
        assert_eq!((one.volts, one.factor), (120.0, 2.0));
        let two = supply(2, Some("208Y/120V, 3PH, 4W"), "P", rl);
        assert_eq!((two.volts, two.factor), (208.0, 2.0));
        // 4,140 VA on two poles at 208 V is about 20 A, not the 34.5 A it is at 120 V.
        assert!((two.amps(4140.0) - 19.9).abs() < 0.05, "{}", two.amps(4140.0));
        let three = supply(3, Some("480Y/277V"), "P", rl);
        assert_eq!(three.volts, 480.0);
        assert!((three.amps(10_000.0) - 12.03).abs() < 0.01, "{}", three.amps(10_000.0));
        let lit = supply(1, Some("480Y/277V"), "P", rl);
        assert_eq!(lit.volts, 277.0);
        assert!(lit.source.contains("480Y/277V"), "{}", lit.source);
        // Not read: the rules' defaults, said so.
        let d = supply(2, None, "P", rl);
        assert_eq!(d.volts, rl.vdrop.voltage_ll);
        assert!(d.source.contains("not read"), "{}", d.source);
    }

    #[test]
    fn the_load_moment_counts_each_stretch_at_the_load_beyond_it() {
        // A chain of four equal devices 10 ft apart: 3/4·10 + 2/4·10 + 1/4·10 = 15 ft, not 30.
        let chain = [(0, 1, 10.0), (1, 2, 10.0), (2, 3, 10.0)];
        assert!((moment_ft(4, 0, &chain, &[]) - 15.0).abs() < 1e-9);
        // All the load at the far end: the whole length.
        assert!((moment_ft(4, 0, &chain, &[0.0, 0.0, 0.0, 1.0]) - 30.0).abs() < 1e-9);
        // A branch: the worst of the two ways (devices 1, 2 and 3 are beyond the first stretch).
        let tee = [(0, 1, 10.0), (1, 2, 20.0), (1, 3, 5.0)];
        let m = moment_ft(4, 0, &tee, &[1.0, 1.0, 1.0, 1.0]);
        assert!((m - (0.75 * 10.0 + 0.25 * 20.0)).abs() < 1e-9, "{m}");
        // A loop back and bad input are harmless.
        assert!(moment_ft(3, 0, &[(0, 1, 5.0), (1, 2, 5.0), (2, 0, 5.0)], &[]).is_finite());
        assert_eq!(moment_ft(0, 0, &[], &[]), 0.0);
        assert_eq!(moment_ft(2, 5, &[(0, 1, f64::NAN)], &[]), 0.0);
        assert_eq!(moment_ft(1, 0, &[], &[]), 0.0);
    }

    #[test]
    fn the_homerun_goes_up_before_the_legs() {
        let rl = rules();
        let sup = supply(1, Some("208Y/120V"), "P", rl);
        // 10 A: 40 ft past the homerun is 1.3% on #12; a 75 ft #12 homerun adds 2.5% (over 3%),
        // a #10 one 1.6% (2.9% in all).
        let s = for_drop("12", 75.0, 40.0, 10.0, &sup, rl).expect("sized");
        assert_eq!((s.home.as_str(), s.legs.as_str(), s.reason), ("10", "12", DROP_HOME), "{s:?}");
        assert!(s.before_pct > 3.0 && s.after_pct <= 3.0, "{s:?}");
        assert!(s.why.contains("homerun") && s.why.contains("210.19"), "{}", s.why);
        // 90 ft: a #10 homerun is 3.2% in all, #8 holds it; the legs stay #12.
        let s = for_drop("12", 90.0, 40.0, 10.0, &sup, rl).expect("sized");
        assert_eq!((s.home.as_str(), s.legs.as_str()), ("8", "12"), "{s:?}");
        // Short: as given.
        let s = for_drop("12", 30.0, 20.0, 10.0, &sup, rl).expect("sized");
        assert_eq!((s.home.as_str(), s.legs.as_str(), s.reason), ("12", "12", ""));
        // The legs alone over the limit: the legs go up too.
        let s = for_drop("12", 20.0, 150.0, 10.0, &sup, rl).expect("sized");
        assert_eq!(s.reason, DROP_CIRCUIT, "{s:?}");
        assert_ne!(s.legs, "12");
        assert!(s.after_pct <= 3.0);
        // A homerun six sizes over its legs is no trade's: the legs go up instead (two steps).
        let s = for_drop("12", 31.0, 51.0, 16.0, &sup, rl).expect("sized");
        assert_eq!((s.home.as_str(), s.legs.as_str(), s.reason), ("10", "10", DROP_CIRCUIT), "{s:?}");
        // Not in the table: not sized.
        assert!(for_drop("500", 10.0, 10.0, 1.0, &sup, rl).is_none());
    }

    #[test]
    fn the_breaker_sets_the_smallest_conductor() {
        let rl = rules();
        assert_eq!(breaker_minimum("12", 2, Some(20.0), rl), None);
        assert_eq!(breaker_minimum("12", 2, Some(30.0), rl).map(|s| s.0), Some("10".into()));
        assert_eq!(breaker_minimum("12", 2, Some(40.0), rl).map(|s| s.0), Some("8".into()));
        // 240.4(B): #6 (65 A at 75 °C) on a 70 A breaker, the next standard rating.
        assert_eq!(breaker_minimum("6", 2, Some(70.0), rl), None);
        assert!(carries("6", 3, 70.0, rl));
        // 240.4(D): #10 is 30 A at most, whatever its column says.
        assert!(!carries("10", 2, 35.0, rl));
        let (_, why) = breaker_minimum("12", 2, Some(30.0), rl).expect("sized");
        assert!(why.contains("240.4(D)") && why.contains("30 A"), "{why}");
        assert_eq!(breaker_minimum("12", 2, None, rl), None);
    }
}
