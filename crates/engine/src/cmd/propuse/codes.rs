//! The reference tables the property checks and takeoffs use, and the parsers for the values the
//! prints write (`20A`, `2#12, 1#12G`, `3/4"`, `12x8`, `1/4"/FT`, `150 CFM`).
//!
//! The tables are the common US model-code values (NEC 2020/2023, IPC 2021, IFGC 2021, SMACNA
//! low-pressure duct, ASPE hot-water demand). A check built on them is a finding to review, not a
//! code ruling: the project's own code book wins where it differs.

/// A number at the start of `t` (after `$`, `#`, spaces): `20A` → 20, `1,200 VA` → 1200.
pub fn lead_num(t: &str) -> Option<f64> {
    let t = t.trim().trim_start_matches(['$', '#', '+', '~']).trim();
    let mut end = 0;
    for (i, c) in t.char_indices() {
        if c.is_ascii_digit() || c == '.' || c == ',' {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    t.get(..end)?.replace(',', "").parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A length in inches from a size such as `3/4"`, `1-1/4"`, `1 1/2`, `1.5`, `4 in`, `10"Ø`.
pub fn inches(t: &str) -> Option<f64> {
    let t = t.trim().to_ascii_lowercase();
    let t = t.trim_end_matches(|c: char| !c.is_ascii_digit()).trim();
    let t = t.trim_end_matches(['"', '\'']).trim();
    if t.is_empty() {
        return None;
    }
    let frac = |f: &str| -> Option<f64> {
        let (a, b) = f.split_once('/')?;
        let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
        (b > 0.0).then_some(a / b)
    };
    let v = if let Some((whole, f)) = t.split_once(['-', ' ']).filter(|(_, f)| f.contains('/')) {
        whole.trim().parse::<f64>().ok()? + frac(f)?
    } else if t.contains('/') {
        frac(t)?
    } else {
        t.parse::<f64>().ok()?
    };
    (v.is_finite() && v > 0.0 && v < 1_000.0).then_some(v)
}

/// A duct or pipe size: `12x8` (width × height, inches) or one diameter (`10"Ø`, `10 round`, `4"`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Section {
    Rect(f64, f64),
    Round(f64),
}

impl Section {
    pub fn parse(t: &str) -> Option<Section> {
        let l = t.trim().to_ascii_lowercase().replace(['ø', '⌀', '"'], " ").replace("dia", " ").replace("round", " ");
        if let Some((a, b)) = l.split_once(['x', '×', '*']) {
            let (w, h) = (inches(a.trim())?, inches(b.trim())?);
            return Some(Section::Rect(w, h));
        }
        inches(l.trim()).map(Section::Round)
    }

    /// Cross-section area, square feet.
    pub fn area_sf(self) -> f64 {
        match self {
            Section::Rect(w, h) => w * h / 144.0,
            Section::Round(d) => std::f64::consts::PI * (d / 2.0).powi(2) / 144.0,
        }
    }

    /// Perimeter, feet (sheet metal per foot of duct).
    pub fn perimeter_ft(self) -> f64 {
        match self {
            Section::Rect(w, h) => 2.0 * (w + h) / 12.0,
            Section::Round(d) => std::f64::consts::PI * d / 12.0,
        }
    }

    /// The largest dimension, inches (what the gauge goes by).
    pub fn largest_in(self) -> f64 {
        match self {
            Section::Rect(w, h) => w.max(h),
            Section::Round(d) => d,
        }
    }
}

/// Galvanised sheet weight, lb per square foot, by gauge (26 to 16).
pub fn gauge_lb_sf(gauge: u32) -> Option<f64> {
    Some(match gauge {
        26 => 0.906,
        24 => 1.156,
        22 => 1.406,
        20 => 1.656,
        18 => 2.156,
        16 => 2.656,
        _ => return None,
    })
}

/// The low-pressure (2 in. w.g.) rectangular duct gauge for a largest side (SMACNA's common
/// table, unreinforced spans): the rule a takeoff uses when the duct carries no gauge.
pub fn gauge_for(largest_in: f64) -> u32 {
    match largest_in {
        x if x <= 12.0 => 26,
        x if x <= 30.0 => 24,
        x if x <= 54.0 => 22,
        x if x <= 84.0 => 20,
        _ => 18,
    }
}

/// Pipe hanger spacing, feet (IPC Table 308.5, horizontal): by material and size.
pub fn hanger_spacing_ft(material: &str, size_in: Option<f64>) -> f64 {
    let m = material.to_ascii_lowercase();
    let small = size_in.is_some_and(|s| s <= 1.25);
    if m.contains("pvc") || m.contains("abs") || m.contains("cpvc") || m.contains("pex") || m.contains("plastic") {
        if m.contains("pex") { 2.67 } else { 4.0 }
    } else if m.contains("cast") || m.contains("ci") && m.len() <= 3 || m.contains("no-hub") || m.contains("nohub") {
        5.0
    } else if m.contains("copper") || m.starts_with("cu") || m.contains("type l") || m.contains("type k") || m.contains("type m") {
        if small { 6.0 } else { 10.0 }
    } else if m.contains("steel") || m.contains("black") || m.contains("galv") || m.contains("sch") {
        12.0
    } else if small {
        6.0
    } else {
        10.0
    }
}

// ─── Electrical ─────────────────────────────────────────────────────────────

/// A conductor size as written (`12`, `#12`, `12 AWG`, `1/0`, `2/0 AWG`, `250 kcmil`), in the
/// canonical form the tables use (`12`, `1/0`, `250`).
pub fn awg(t: &str) -> Option<String> {
    let t = t.trim().trim_start_matches('#').trim().to_ascii_lowercase();
    let t = t.trim_end_matches("awg").trim_end_matches("kcmil").trim_end_matches("mcm").trim();
    let t = t.trim_end_matches(|c: char| c.is_ascii_alphabetic()).trim();
    let ok = ["14", "12", "10", "8", "6", "4", "3", "2", "1", "1/0", "2/0", "3/0", "4/0", "250", "300", "350", "400", "500"];
    ok.iter().find(|s| **s == t).map(|s| (*s).to_string())
}

/// Copper conductor area, circular mils (NEC Chapter 9 Table 8).
pub fn circular_mils(size: &str) -> Option<f64> {
    Some(match size {
        "14" => 4_110.0,
        "12" => 6_530.0,
        "10" => 10_380.0,
        "8" => 16_510.0,
        "6" => 26_240.0,
        "4" => 41_740.0,
        "3" => 52_620.0,
        "2" => 66_360.0,
        "1" => 83_690.0,
        "1/0" => 105_600.0,
        "2/0" => 133_100.0,
        "3/0" => 167_800.0,
        "4/0" => 211_600.0,
        "250" => 250_000.0,
        "300" => 300_000.0,
        "350" => 350_000.0,
        "400" => 400_000.0,
        "500" => 500_000.0,
        _ => return None,
    })
}

/// THHN/THWN-2 conductor area, square inches (NEC Chapter 9 Table 5).
pub fn thhn_area(size: &str) -> Option<f64> {
    Some(match size {
        "14" => 0.0097,
        "12" => 0.0133,
        "10" => 0.0211,
        "8" => 0.0366,
        "6" => 0.0507,
        "4" => 0.0824,
        "3" => 0.0973,
        "2" => 0.1158,
        "1" => 0.1562,
        "1/0" => 0.1855,
        "2/0" => 0.2223,
        "3/0" => 0.2679,
        "4/0" => 0.3237,
        "250" => 0.3970,
        "300" => 0.4608,
        "350" => 0.5242,
        "400" => 0.5863,
        "500" => 0.7073,
        _ => return None,
    })
}

/// The largest breaker a copper conductor may be protected by, amps: NEC 240.4(D) for the small
/// sizes, else the 75 °C column of Table 310.16.
pub fn max_breaker(size: &str) -> Option<f64> {
    Some(match size {
        "14" => 15.0,
        "12" => 20.0,
        "10" => 30.0,
        "8" => 50.0,
        "6" => 65.0,
        "4" => 85.0,
        "3" => 100.0,
        "2" => 115.0,
        "1" => 130.0,
        "1/0" => 150.0,
        "2/0" => 175.0,
        "3/0" => 200.0,
        "4/0" => 230.0,
        "250" => 255.0,
        "300" => 285.0,
        "350" => 310.0,
        "400" => 335.0,
        "500" => 380.0,
        _ => return None,
    })
}

/// Total internal area of EMT by trade size, square inches (NEC Chapter 9 Table 4).
pub fn emt_area(trade_in: f64) -> Option<f64> {
    let t = [
        (0.5, 0.304),
        (0.75, 0.533),
        (1.0, 0.864),
        (1.25, 1.496),
        (1.5, 2.036),
        (2.0, 3.356),
        (2.5, 5.858),
        (3.0, 8.846),
        (3.5, 11.545),
        (4.0, 14.753),
    ];
    t.iter().find(|(s, _)| (s - trade_in).abs() < 0.01).map(|(_, a)| *a)
}

/// The fill a raceway may have (NEC Chapter 9 Table 1): 53 % for one conductor, 31 % for two,
/// 40 % for more.
pub fn fill_limit(conductors: u32) -> f64 {
    match conductors {
        0 | 1 => 0.53,
        2 => 0.31,
        _ => 0.40,
    }
}

/// The conductors a wire note names: `2#12, 1#12G` → [(2, "12", false), (1, "12", true)];
/// `3#10 + #10G IN 3/4" EMT`; `#12` alone is one conductor. Also the raceway size it names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wire {
    pub conductors: Vec<(u32, String, bool)>,
    pub conduit_in: Option<f64>,
}

impl Wire {
    pub fn parse(t: &str) -> Option<Wire> {
        let up = t.to_ascii_uppercase();
        let mut w = Wire::default();
        let b = up.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b.get(i) == Some(&b'#') {
                // The count before the #.
                let mut j = i;
                while j > 0 && b.get(j - 1).is_some_and(u8::is_ascii_digit) {
                    j -= 1;
                }
                let count = up.get(j..i).and_then(|c| c.parse::<u32>().ok()).filter(|c| *c > 0 && *c < 100).unwrap_or(1);
                let mut k = i + 1;
                while k < b.len() && (b.get(k).is_some_and(u8::is_ascii_digit) || b.get(k) == Some(&b'/')) {
                    k += 1;
                }
                if let Some(size) = up.get(i + 1..k).and_then(awg) {
                    let ground = matches!(b.get(k), Some(b'G'))
                        || up.get(k..).is_some_and(|r| r.trim_start().starts_with("GND") || r.trim_start().starts_with("GR"));
                    w.conductors.push((count, size, ground));
                }
                i = k.max(i + 1);
            } else {
                i += 1;
            }
        }
        if w.conductors.is_empty() {
            // `12 AWG`, `#12` written without the hash.
            let size = up.split([',', '+', ' ']).find_map(|p| p.contains("AWG").then(|| awg(p)).flatten().or_else(|| awg(p)));
            w.conductors.push((1, size?, false));
        }
        // The raceway: a size written in inches and followed by C, EMT or CONDUIT (`3/4"C`,
        // `3/4" EMT`).
        for (at, _) in up.match_indices('"') {
            let tail = up.get(at + 1..).unwrap_or("").trim_start();
            let raceway = tail.starts_with("EMT") || tail.starts_with("CONDUIT") || (tail.starts_with('C') && !tail.starts_with("CU"));
            let head = up.get(..at).unwrap_or("");
            let start = head.rfind(|c: char| !(c.is_ascii_digit() || c == '/' || c == '-' || c == '.')).map_or(0, |p| p + 1);
            if raceway && let Some(v) = head.get(start..).and_then(inches).filter(|v| *v <= 6.0) {
                w.conduit_in = Some(v);
            }
        }
        Some(w)
    }

    /// The current-carrying conductors' size (the largest non-ground), else any.
    pub fn phase_size(&self) -> Option<&str> {
        self.conductors.iter().find(|c| !c.2).or(self.conductors.first()).map(|c| c.1.as_str())
    }

    pub fn count(&self) -> u32 {
        self.conductors.iter().map(|c| c.0).sum()
    }

    /// The conductors' total area, square inches (THHN), when every size is known.
    pub fn area(&self) -> Option<f64> {
        self.conductors.iter().map(|(n, s, _)| thhn_area(s).map(|a| a * f64::from(*n))).sum()
    }
}

/// Voltage drop, percent, on a copper two-wire circuit: 2·K·I·L / CM (K = 12.9 Ω·cmil/ft).
pub fn voltage_drop_pct(amps: f64, one_way_ft: f64, size: &str, volts: f64) -> Option<f64> {
    let cm = circular_mils(size)?;
    (volts > 0.0).then(|| 2.0 * 12.9 * amps * one_way_ft / cm / volts * 100.0)
}

// ─── Plumbing and gas ───────────────────────────────────────────────────────

/// A plumbing fixture's drainage fixture units by its type (IPC Table 709.1, public use).
pub fn dfu_of(t: &str) -> Option<f64> {
    let t = t.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| t.contains(x));
    Some(if has(&["water closet", "toilet", "wc", "urinal"]) {
        4.0
    } else if has(&["mop", "service sink", "janitor", "clothes wash", "washing machine", "washer box"]) {
        3.0
    } else if has(&["lavatory", "lav", "basin"]) {
        1.0
    } else if has(&["shower", "bathtub", "tub", "kitchen sink", "sink", "floor drain", "dishwasher", "laundry"]) {
        2.0
    } else if has(&["drinking fountain", "water cooler", "bottle filler"]) {
        0.5
    } else {
        return None;
    })
}

/// Hot water a fixture uses, gallons per hour at 140 °F (ASPE's hotel column).
pub fn hw_gph_of(t: &str) -> Option<f64> {
    let t = t.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| t.contains(x));
    Some(if has(&["water closet", "toilet", "urinal", "floor drain", "drinking fountain", "water cooler", "hose bibb"]) {
        return None;
    } else if has(&["shower"]) {
        75.0
    } else if has(&["bathtub", "tub"]) {
        20.0
    } else if has(&["dishwasher"]) {
        50.0
    } else if has(&["mop", "service sink", "janitor", "kitchen sink"]) {
        30.0
    } else if has(&["clothes wash", "laundry"]) {
        28.0
    } else if has(&["lavatory", "lav", "basin"]) {
        2.0
    } else if has(&["sink"]) {
        10.0
    } else {
        return None;
    })
}

/// The DFU a drain pipe of `size_in` may carry (IPC Tables 710.1(1) and 710.1(2)).
pub fn max_dfu(role: DrainRole, size_in: f64, slope_in_ft: Option<f64>) -> Option<f64> {
    let t: &[(f64, f64)] = match role {
        DrainRole::Branch => &[
            (1.5, 3.0),
            (2.0, 6.0),
            (2.5, 12.0),
            (3.0, 20.0),
            (4.0, 160.0),
            (5.0, 360.0),
            (6.0, 620.0),
            (8.0, 1400.0),
            (10.0, 2500.0),
            (12.0, 3900.0),
        ],
        DrainRole::Stack => &[
            (1.5, 4.0),
            (2.0, 10.0),
            (2.5, 20.0),
            (3.0, 48.0),
            (4.0, 240.0),
            (5.0, 540.0),
            (6.0, 960.0),
            (8.0, 2200.0),
            (10.0, 3800.0),
            (12.0, 6000.0),
        ],
        DrainRole::Main if slope_in_ft.is_some_and(|s| s < 0.24) => {
            &[(3.0, 36.0), (4.0, 180.0), (5.0, 390.0), (6.0, 700.0), (8.0, 1600.0), (10.0, 2900.0), (12.0, 4600.0)]
        }
        DrainRole::Main => {
            &[(2.0, 21.0), (2.5, 24.0), (3.0, 42.0), (4.0, 216.0), (5.0, 480.0), (6.0, 840.0), (8.0, 1920.0), (10.0, 3500.0), (12.0, 5600.0)]
        }
    };
    t.iter().find(|(s, _)| (s - size_in).abs() < 0.01).map(|(_, n)| *n)
}

/// Where a drain pipe sits in the drainage system: what its fixture-unit limit goes by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainRole {
    Branch,
    Stack,
    Main,
}

impl DrainRole {
    pub fn as_str(self) -> &'static str {
        match self {
            DrainRole::Branch => "horizontal branch",
            DrainRole::Stack => "stack",
            DrainRole::Main => "building drain",
        }
    }
}

/// The least slope of a horizontal drain, inches per foot (IPC Table 704.1).
pub fn min_slope(size_in: f64) -> f64 {
    if size_in <= 2.5 {
        0.25
    } else if size_in < 8.0 {
        0.125
    } else {
        0.0625
    }
}

/// A slope as written (`1/4"/FT`, `1/8 IN/FT`, `2%`, `0.0208` ft/ft), inches per foot.
pub fn slope_in_ft(t: &str) -> Option<f64> {
    let l = t.trim().to_ascii_lowercase();
    if let Some(p) = l.strip_suffix('%') {
        return p.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v * 0.12);
    }
    if l.contains(['"', '/']) || l.contains("in") || l.contains("ft") {
        let n = l.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '/' | ' ' | '-'))).unwrap_or(l.len());
        let v = inches(l.get(..n)?.trim().trim_end_matches(['/', '-']).trim())?;
        return (v < 12.0).then_some(v);
    }
    let v = l.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)?;
    // A bare decimal under 0.1 is a ratio (ft/ft); else inches per foot.
    Some(if v < 0.1 { v * 12.0 } else { v })
}

/// Natural gas capacity of schedule 40 pipe, cubic feet per hour, by nominal size and length
/// (IFGC Table 402.4(2): under 2 psi, 0.5 in. w.c. drop, specific gravity 0.60), lengths 10 to
/// 100 ft. `None` past the table (size not listed, or longer than 100 ft).
pub fn gas_cfh(size_in: f64, length_ft: f64) -> Option<f64> {
    const LEN: [f64; 10] = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0];
    const CAP: [(f64, [f64; 10]); 6] = [
        (0.5, [172.0, 118.0, 95.0, 81.0, 72.0, 65.0, 60.0, 56.0, 52.0, 50.0]),
        (0.75, [360.0, 247.0, 199.0, 170.0, 151.0, 137.0, 126.0, 117.0, 110.0, 104.0]),
        (1.0, [678.0, 466.0, 374.0, 320.0, 284.0, 257.0, 237.0, 220.0, 207.0, 195.0]),
        (1.25, [1390.0, 957.0, 768.0, 657.0, 583.0, 528.0, 486.0, 452.0, 424.0, 400.0]),
        (1.5, [2090.0, 1430.0, 1150.0, 985.0, 873.0, 791.0, 728.0, 677.0, 635.0, 600.0]),
        (2.0, [4020.0, 2760.0, 2220.0, 1900.0, 1680.0, 1520.0, 1400.0, 1300.0, 1220.0, 1160.0]),
    ];
    let col = LEN.iter().position(|l| *l + 1e-9 >= length_ft)?;
    CAP.iter().find(|(s, _)| (s - size_in).abs() < 0.01).and_then(|(_, c)| c.get(col).copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsers_read_what_prints_write() {
        assert_eq!(lead_num("20A"), Some(20.0));
        assert_eq!(lead_num("1,200 VA"), Some(1200.0));
        assert_eq!(inches("3/4\""), Some(0.75));
        assert_eq!(inches("1-1/4\""), Some(1.25));
        assert_eq!(inches("1 1/2"), Some(1.5));
        assert_eq!(Section::parse("12x8"), Some(Section::Rect(12.0, 8.0)));
        assert_eq!(Section::parse("10\"Ø"), Some(Section::Round(10.0)));
        let w = Wire::parse("2#12, 1#12G, 3/4\"C").unwrap();
        assert_eq!(w.conductors, vec![(2, "12".into(), false), (1, "12".into(), true)]);
        assert_eq!(w.conduit_in, Some(0.75));
        let w = Wire::parse("TYPICAL: 2#12 + #12G IN 3/4\" EMT").unwrap();
        assert_eq!(w.count(), 3);
        assert_eq!(w.conduit_in, Some(0.75));
        assert_eq!(Wire::parse("12 AWG").unwrap().phase_size(), Some("12"));
        assert_eq!(slope_in_ft("1/4\"/FT"), Some(0.25));
        assert_eq!(slope_in_ft("1/8 IN/FT"), Some(0.125));
        assert!((slope_in_ft("2%").unwrap() - 0.24).abs() < 1e-9);
        assert!((slope_in_ft("0.0104").unwrap() - 0.1248).abs() < 1e-6);
    }

    #[test]
    fn tables_give_the_code_values() {
        assert_eq!(max_breaker("12"), Some(20.0));
        assert_eq!(max_breaker("10"), Some(30.0));
        assert_eq!(emt_area(0.75), Some(0.533));
        assert_eq!(max_dfu(DrainRole::Branch, 2.0, None), Some(6.0));
        assert_eq!(max_dfu(DrainRole::Main, 4.0, Some(0.125)), Some(180.0));
        assert_eq!(gas_cfh(1.0, 45.0), Some(284.0));
        assert_eq!(gas_cfh(1.0, 150.0), None);
        assert_eq!(gauge_for(20.0), 24);
        let vd = voltage_drop_pct(16.0, 100.0, "12", 120.0).unwrap();
        assert!((vd - 5.27).abs() < 0.01, "{vd}");
    }
}
