//! MEP parts: mechanical (ceiling diffusers, registers and grilles, linear slots, VAV boxes,
//! fans, rooftop and split units, dampers), plumbing (valves and fittings as annotation symbols,
//! pumps, water heaters, tanks, drains, interceptors) and electrical (panels, switchboards,
//! transformers, disconnects, light fixtures, generators, transfer switches, cable tray).
//!
//! Equipment footprints are generic: sized from the nominal rating (tons, kVA, kW, amps) so they
//! read right on a plan, not any maker's catalogue dimensions; check the selected unit's
//! submittal before coordinating clearances.

use super::{ANNO, Args, Catalog, Discipline, Kit, REAL, args, inches, num};

pub(crate) fn add(c: &mut Catalog) {
    mechanical(c);
    plumbing(c);
    electrical(c);
}

/// A filled arrowhead at `tip` pointing along (`ux`, `uy`) (a unit vector), `size` long.
fn head(k: &mut Kit, tip: (f64, f64), ux: f64, uy: f64, size: f64) {
    let (bx, by) = (tip.0 - ux * size, tip.1 - uy * size);
    let w = size * 0.35;
    k.fill(&[tip, (bx - uy * w, by + ux * w), (bx + uy * w, by - ux * w)]);
}

/// An arrow from `a` to `b` with its head at `b`.
fn arrow(k: &mut Kit, a: (f64, f64), b: (f64, f64), size: f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
    k.line(a, b);
    head(k, b, dx / l, dy / l, size);
}

// ---------------------------------------------------------------------------------------------
// Mechanical

const M: Discipline = Discipline::Mechanical;

fn mechanical(c: &mut Catalog) {
    const DF: &str = "Diffusers and grilles";
    for (face, necks) in [(24.0, &[6.0, 8.0, 10.0, 12.0, 14.0][..]), (12.0, &[6.0, 8.0][..])] {
        for &n in necks {
            c.add(
                M,
                DF,
                &format!("DIFF_SQ_{}X{}_{}N", num(face), num(face), num(n)),
                format!("Diffuser, square {}x{} face, {} neck", inches(face), inches(face), inches(n)),
                "supply ceiling louvered",
                diffuser_square,
                args(&[face, n]),
                REAL,
            );
        }
    }
    for n in [6.0, 8.0, 10.0, 12.0, 14.0, 16.0] {
        c.add(
            M,
            DF,
            &format!("DIFF_ROUND_{}", num(n)),
            format!("Diffuser, round {} neck", inches(n)),
            "supply ceiling",
            diffuser_round,
            args(&[n]),
            REAL,
        );
    }
    for (word, label, w, h, ex) in [
        ("RETURN_24X24", "return, perforated 24\"x24\"", 24.0, 24.0, 0.0),
        ("RETURN_24X12", "return, perforated 24\"x12\"", 24.0, 12.0, 0.0),
        ("EXHAUST_24X24", "exhaust, perforated 24\"x24\"", 24.0, 24.0, 1.0),
        ("EXHAUST_12X12", "exhaust, perforated 12\"x12\"", 12.0, 12.0, 1.0),
    ] {
        c.add(M, DF, &format!("GRILLE_{word}"), format!("Grille, {label}"), "ceiling air", grille_ceiling, args(&[w, h, ex]), REAL);
    }
    for (w, h) in [(8.0, 4.0), (10.0, 4.0), (10.0, 6.0), (12.0, 6.0), (14.0, 6.0), (16.0, 8.0), (20.0, 8.0), (24.0, 10.0), (24.0, 12.0), (30.0, 12.0)]
    {
        c.add(
            M,
            DF,
            &format!("REGISTER_SUPPLY_{}X{}", num(w), num(h)),
            format!("Register, sidewall supply {}x{}", inches(w), inches(h)),
            "wall air",
            sidewall,
            args(&[w, h, 0.0]),
            REAL,
        );
        c.add(
            M,
            DF,
            &format!("GRILLE_RETURN_WALL_{}X{}", num(w), num(h)),
            format!("Grille, sidewall return {}x{}", inches(w), inches(h)),
            "wall air",
            sidewall,
            args(&[w, h, 1.0]),
            REAL,
        );
    }
    for l in [24.0, 36.0, 48.0, 60.0, 72.0, 96.0] {
        for slots in 1..=4 {
            let s = f64::from(slots);
            c.add(
                M,
                DF,
                &format!("SLOT_{}_{}S", num(l), slots),
                format!("Linear slot diffuser, {} long, {slots} slot{}", inches(l), if slots > 1 { "s" } else { "" }),
                "supply ceiling linear",
                linear_slot,
                args(&[l, s]),
                REAL,
            );
        }
    }
    const TU: &str = "Terminal units";
    for inlet in [4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 12.0, 14.0, 16.0] {
        for (word, label, coil) in [("", "", 0.0), ("_HW", " with hot water reheat", 1.0), ("_E", " with electric reheat", 2.0)] {
            c.add(
                M,
                TU,
                &format!("VAV_{}{word}", num(inlet)),
                format!("VAV box, single duct {} inlet{label}", inches(inlet)),
                "terminal unit variable air volume reheat",
                vav,
                args(&[inlet, coil, 0.0]),
                REAL,
            );
        }
    }
    for inlet in [6.0, 8.0, 10.0, 12.0, 14.0] {
        c.add(
            M,
            TU,
            &format!("FPB_SERIES_{}", num(inlet)),
            format!("Fan powered box, series {} inlet", inches(inlet)),
            "terminal unit vav fan",
            vav,
            args(&[inlet, 0.0, 1.0]),
            REAL,
        );
        c.add(
            M,
            TU,
            &format!("FPB_PARALLEL_{}", num(inlet)),
            format!("Fan powered box, parallel {} inlet", inches(inlet)),
            "terminal unit vav fan",
            vav,
            args(&[inlet, 0.0, 2.0]),
            REAL,
        );
    }
    for inlet in [6.0, 8.0, 10.0, 12.0] {
        c.add(
            M,
            TU,
            &format!("VAV_DUAL_{}", num(inlet)),
            format!("VAV box, dual duct {} inlets", inches(inlet)),
            "terminal unit hot cold deck",
            vav,
            args(&[inlet, 0.0, 3.0]),
            REAL,
        );
    }
    const EQ: &str = "HVAC equipment";
    for d in [6.0, 8.0, 10.0, 12.0, 14.0, 16.0] {
        c.add(
            M,
            EQ,
            &format!("FAN_INLINE_{}", num(d)),
            format!("Fan, inline {} duct", inches(d)),
            "exhaust supply mixed flow",
            fan_inline,
            args(&[d]),
            REAL,
        );
    }
    for curb in [18.0, 24.0, 30.0] {
        c.add(
            M,
            EQ,
            &format!("FAN_ROOF_{}", num(curb)),
            format!("Fan, roof exhaust {} curb", inches(curb)),
            "power roof ventilator",
            fan_roof,
            args(&[curb]),
            REAL,
        );
    }
    for tons in [3.0, 5.0, 7.5, 10.0, 12.5, 15.0, 20.0, 25.0] {
        c.add(
            M,
            EQ,
            &format!("RTU_{}T", num(tons)),
            format!("Rooftop unit, {tons} ton (generic footprint)"),
            "packaged air conditioner",
            rtu,
            args(&[tons]),
            REAL,
        );
    }
    for cfm in [2000.0, 4000.0, 6000.0, 8000.0, 12000.0, 20000.0] {
        c.add(
            M,
            EQ,
            &format!("AHU_{}CFM", num(cfm)),
            format!("Air handling unit, {cfm} cfm (generic footprint)"),
            "ahu air handler",
            ahu,
            args(&[cfm]),
            REAL,
        );
    }
    for tons in [1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 5.0] {
        c.add(
            M,
            EQ,
            &format!("CONDENSER_{}T", num(tons)),
            format!("Condensing unit, {tons} ton (generic footprint)"),
            "split system outdoor",
            condenser,
            args(&[tons]),
            REAL,
        );
    }
    for (word, w) in [("S", 18.0), ("M", 24.0), ("L", 30.0)] {
        c.add(
            M,
            EQ,
            &format!("UNIT_HEATER_{word}"),
            format!("Unit heater, horizontal {} wide", inches(w)),
            "suspended heating",
            unit_heater,
            args(&[w]),
            REAL,
        );
    }
    for cfm in [200.0, 400.0, 600.0, 800.0] {
        c.add(
            M,
            EQ,
            &format!("FAN_COIL_{}CFM", num(cfm)),
            format!("Fan coil unit, {cfm} cfm (generic footprint)"),
            "fcu horizontal",
            fan_coil,
            args(&[cfm]),
            REAL,
        );
    }
    for (word, label, k) in [
        ("FIRE", "fire damper", 0.0),
        ("SMOKE", "smoke damper", 1.0),
        ("COMBO", "combination fire/smoke damper", 2.0),
        ("VOLUME", "volume damper", 3.0),
    ] {
        c.add(M, "Dampers", &format!("DAMPER_{word}"), format!("Damper, {label}"), "duct symbol", damper, args(&[k]), ANNO);
    }
}

fn diffuser_square(k: &mut Kit, a: &Args) {
    let (f, n) = (a.get(0), a.get(1));
    let h = f / 2.0;
    k.rect((-h, -h), (h, h));
    for s in [0.75, 0.5] {
        k.rect((-h * s, -h * s), (h * s, h * s));
    }
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        k.line((sx * h * 0.5, sy * h * 0.5), (sx * h, sy * h));
    }
    k.hidden(true);
    k.circle((0.0, 0.0), n / 2.0);
    k.hidden(false);
    // The throw pattern: arrows out of the faces that blow.
    k.visibility("Pattern", &["4-way", "3-way", "2-way", "1-way"]);
    let dirs = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];
    for g in 0..4u8 {
        k.state(Some(g));
        let shown = 4 - usize::from(g);
        for &(dx, dy) in dirs.iter().take(shown) {
            arrow(k, (dx * h * 0.55, dy * h * 0.55), (dx * (h + f * 0.3), dy * (h + f * 0.3)), f * 0.12);
        }
    }
    k.state(None);
    k.attdef_hidden("CFM", "Airflow (cfm)", "", (h + 2.0, h), 2.0);
}

fn diffuser_round(k: &mut Kit, a: &Args) {
    let n = a.get(0);
    let r = n;
    for s in [1.0, 0.75, 0.5] {
        k.circle((0.0, 0.0), r * s);
    }
    k.hidden(true);
    k.circle((0.0, 0.0), n / 2.0 + 0.01);
    k.hidden(false);
    k.attdef_hidden("CFM", "Airflow (cfm)", "", (r + 2.0, 0.0), 2.0);
}

fn grille_ceiling(k: &mut Kit, a: &Args) {
    let (w, h, exhaust) = (a.get(0), a.get(1), a.get(2) > 0.5);
    let (x, y) = (w / 2.0, h / 2.0);
    k.rect((-x, -y), (x, y));
    k.rect((-x + 1.0, -y + 1.0), (x - 1.0, y - 1.0));
    k.line((-x + 1.0, -y + 1.0), (x - 1.0, y - 1.0));
    if !exhaust {
        k.line((x - 1.0, -y + 1.0), (-x + 1.0, y - 1.0));
    }
    k.attdef_hidden("CFM", "Airflow (cfm)", "", (x + 2.0, y), 2.0);
}

fn sidewall(k: &mut Kit, a: &Args) {
    let (w, h, ret) = (a.get(0), a.get(1), a.get(2) > 0.5);
    // On the wall face (Y 0), in plan: the face and its neck into the wall.
    k.rect((0.0, 0.0), (w, -1.5));
    k.hidden(true);
    k.rect((0.75, 0.0), (w - 0.75, h * 0.5));
    k.hidden(false);
    let mid = w / 2.0;
    if ret {
        arrow(k, (mid, -10.0), (mid, -2.0), 2.5);
    } else {
        arrow(k, (mid, -2.0), (mid, -10.0), 2.5);
    }
    k.stretch("Width", (0.0, -1.5), (w, -1.5), (w * 0.75, 7.0), (w + 2.0, -3.0), 4.0, 48.0, 2.0);
    k.attdef_hidden("CFM", "Airflow (cfm)", "", (w + 2.0, 0.0), 2.0);
}

fn linear_slot(k: &mut Kit, a: &Args) {
    let (l, slots) = (a.get(0), a.get(1) as usize);
    let wd = slots as f64 * 1.0 + 2.0;
    k.rect((0.0, 0.0), (l, wd));
    for i in 0..slots {
        let y = 1.0 + i as f64 + 0.5;
        k.line((1.0, y), (l - 1.0, y));
    }
    k.stretch("Length", (0.0, 0.0), (l, 0.0), (l * 0.75, -1.0), (l + 1.0, wd + 1.0), 12.0, 240.0, 12.0);
    k.attdef_hidden("CFM", "Airflow (cfm)", "", (l + 2.0, wd), 2.0);
}

/// A terminal unit: inlet size, coil (0 none, 1 hot water, 2 electric) and kind (0 single duct,
/// 1 series fan, 2 parallel fan, 3 dual duct). Air flows in +X.
fn vav(k: &mut Kit, a: &Args) {
    let (d, coil, kind) = (a.get(0), a.get(1) as i64, a.get(2) as i64);
    let w = (d * 1.5 + 6.0).max(12.0) + if kind == 1 || kind == 2 { 8.0 } else { 0.0 };
    let l = 30.0 + d * 1.5 + if kind > 0 { 12.0 } else { 0.0 };
    let h = w / 2.0;
    // Inlet collar(s).
    if kind == 3 {
        k.rect((-10.0, h * 0.5 - d / 2.0), (0.0, h * 0.5 + d / 2.0));
        k.rect((-10.0, -h * 0.5 - d / 2.0), (0.0, -h * 0.5 + d / 2.0));
    } else {
        k.rect((-10.0, -d / 2.0), (0.0, d / 2.0));
    }
    k.rect((0.0, -h), (l, h));
    k.text((l * 0.4, 0.0), 3.0, if kind == 1 || kind == 2 { "FPB" } else { "VAV" });
    if kind == 1 || kind == 2 {
        k.circle((l * 0.7, 0.0), (h - 2.0).min(8.0));
        if kind == 2 {
            k.rect((l * 0.55, h), (l * 0.85, h + 6.0));
        }
    }
    match coil {
        1 => {
            k.rect((l, -h), (l + 4.0, h));
            k.line((l, -h), (l + 4.0, h));
            k.line((l, h), (l + 4.0, -h));
        }
        2 => {
            k.rect((l, -h), (l + 4.0, h));
            k.text((l + 2.0, 0.0), 2.5, "E");
        }
        _ => {}
    }
    // The controls enclosure, on either side.
    k.visibility("Controls", &["Controls left", "Controls right"]);
    k.state(Some(0));
    k.rect((2.0, h), (14.0, h + 4.0));
    k.state(Some(1));
    k.rect((2.0, -h), (14.0, -h - 4.0));
    k.state(None);
    k.attdef_hidden("TAG", "Unit tag", "", (l / 2.0, -h - 6.0), 2.0);
    k.attdef_hidden("CFM", "Maximum airflow (cfm)", "", (l / 2.0, -h - 9.0), 2.0);
}

fn fan_inline(k: &mut Kit, a: &Args) {
    let d = a.get(0);
    let (l, h) = (d * 1.5 + 6.0, d / 2.0 + 3.0);
    k.rect((0.0, -h), (l, h));
    k.circle((l / 2.0, 0.0), d / 2.0);
    k.rect((-4.0, -d / 2.0), (0.0, d / 2.0));
    k.rect((l, -d / 2.0), (l + 4.0, d / 2.0));
    arrow(k, (l * 0.2, -h - 3.0), (l * 0.8, -h - 3.0), 2.0);
}

fn fan_roof(k: &mut Kit, a: &Args) {
    let c = a.get(0);
    let h = c / 2.0;
    k.rect((-h, -h), (h, h));
    k.circle((0.0, 0.0), h * 1.35);
    k.circle((0.0, 0.0), h * 0.5);
    k.cube((-h, -h), (h, h), 0.0, 12.0);
    k.cylinder((0.0, 0.0), h * 1.35, 12.0, 30.0);
}

fn rtu(k: &mut Kit, a: &Args) {
    let t = a.get(0);
    let (l, w) = (60.0 + t * 6.0, 40.0 + t * 2.5);
    k.rect((0.0, 0.0), (l, w));
    // Condenser fans and the supply and return openings (hidden: on the underside).
    let fans = if t >= 12.5 { 2 } else { 1 };
    for i in 0..fans {
        k.circle((l * 0.75 - i as f64 * w * 0.45, w / 2.0), w * 0.2);
    }
    k.hidden(true);
    k.rect((l * 0.1, w * 0.15), (l * 0.3, w * 0.45));
    k.rect((l * 0.1, w * 0.55), (l * 0.3, w * 0.85));
    k.hidden(false);
    k.text((l * 0.2, w * 0.3), 3.0, "S");
    k.text((l * 0.2, w * 0.7), 3.0, "R");
    k.cube((0.0, 0.0), (l, w), 0.0, 36.0 + t * 1.2);
    k.attdef_hidden("TAG", "Unit tag", "", (0.0, -4.0), 3.0);
}

fn ahu(k: &mut Kit, a: &Args) {
    let cfm = a.get(0);
    let w = 30.0 + (cfm / 100.0).sqrt() * 8.0;
    let l = w * 2.6;
    k.rect((0.0, 0.0), (l, w));
    // Sections: filter, coil, fan.
    for f in [0.2, 0.45, 0.62] {
        k.line((l * f, 0.0), (l * f, w));
    }
    k.line((l * 0.2, 0.0), (l * 0.45, w));
    k.circle((l * 0.8, w / 2.0), w * 0.3);
    k.cube((0.0, 0.0), (l, w), 0.0, w * 0.9);
    k.attdef_hidden("TAG", "Unit tag", "", (0.0, -4.0), 3.0);
}

fn condenser(k: &mut Kit, a: &Args) {
    let t = a.get(0);
    let s = 24.0 + t * 3.0;
    k.rect((0.0, 0.0), (s, s));
    k.circle((s / 2.0, s / 2.0), s * 0.4);
    k.circle((s / 2.0, s / 2.0), 2.0);
    k.cube((0.0, 0.0), (s, s), 0.0, 24.0 + t * 4.0);
    k.attdef_hidden("TAG", "Unit tag", "", (0.0, -4.0), 3.0);
}

fn unit_heater(k: &mut Kit, a: &Args) {
    let w = a.get(0);
    k.rect((0.0, 0.0), (w, w * 0.6));
    k.circle((w / 2.0, w * 0.3), w * 0.22);
    arrow(k, (w / 2.0, 0.0), (w / 2.0, -w * 0.5), 3.0);
}

fn fan_coil(k: &mut Kit, a: &Args) {
    let cfm = a.get(0);
    let l = 30.0 + cfm / 20.0;
    k.rect((0.0, 0.0), (l, 24.0));
    k.rect((l, 2.0), (l + 6.0, 22.0));
    k.line((l, 2.0), (l + 6.0, 22.0));
    k.circle((l * 0.4, 12.0), 7.0);
    k.text((l * 0.75, 12.0), 3.0, "FCU");
}

fn damper(k: &mut Kit, a: &Args) {
    let kind = a.get(0) as i64;
    // A duct section 0.25" wide on the sheet, the damper across it.
    k.line((-0.15, 0.125), (0.15, 0.125));
    k.line((-0.15, -0.125), (0.15, -0.125));
    match kind {
        0 => {
            k.line((0.0, -0.18), (0.0, 0.18));
            k.fill(&[(0.0, 0.18), (0.08, 0.1), (0.0, 0.1)]);
            k.text((0.0, 0.26), 0.08, "FD");
        }
        1 => {
            k.line((0.0, -0.18), (0.0, 0.18));
            k.text((0.0, 0.26), 0.08, "SD");
        }
        2 => {
            k.line((0.0, -0.18), (0.0, 0.18));
            k.fill(&[(0.0, 0.18), (0.08, 0.1), (0.0, 0.1)]);
            k.text((0.0, 0.26), 0.08, "FSD");
        }
        _ => {
            k.line((-0.08, -0.08), (0.08, 0.08));
            k.circle((0.0, 0.0), 0.02);
            k.text((0.0, 0.26), 0.08, "VD");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Plumbing

const P: Discipline = Discipline::Plumbing;

fn plumbing(c: &mut Catalog) {
    const V: &str = "Valves";
    for (word, label, k) in [
        ("GATE", "gate", 0.0),
        ("GLOBE", "globe", 1.0),
        ("BALL", "ball", 2.0),
        ("BUTTERFLY", "butterfly", 3.0),
        ("CHECK", "check", 4.0),
        ("PLUG", "plug", 5.0),
        ("NEEDLE", "needle", 6.0),
        ("ANGLE", "angle", 7.0),
        ("3WAY", "three-way", 8.0),
        ("PRV", "pressure reducing", 9.0),
        ("RELIEF", "relief", 10.0),
        ("SOLENOID", "solenoid", 11.0),
        ("BALANCING", "balancing", 12.0),
        ("RPZ", "reduced pressure backflow", 13.0),
        ("DCVA", "double check backflow", 14.0),
        ("HOSE_BIBB", "hose bibb", 15.0),
    ] {
        c.add(P, V, &format!("VALVE_{word}"), format!("Valve, {label}"), "pipe symbol", valve, args(&[k]), ANNO);
    }
    const PF: &str = "Pipe fittings";
    for (word, label, k) in [
        ("ELBOW_UP", "elbow turned up", 0.0),
        ("ELBOW_DOWN", "elbow turned down", 1.0),
        ("TEE_UP", "tee outlet up", 2.0),
        ("TEE_DOWN", "tee outlet down", 3.0),
        ("UNION", "union", 4.0),
        ("CAP", "cap", 5.0),
        ("REDUCER", "concentric reducer", 6.0),
        ("FLANGE", "flanged joint", 7.0),
    ] {
        c.add(P, PF, &format!("FITTING_{word}"), format!("Fitting, {label}"), "pipe symbol", fitting, args(&[k]), ANNO);
    }
    const PU: &str = "Pumps and tanks";
    for (s, d) in [(1.5, 1.0), (2.0, 1.5), (3.0, 2.0), (4.0, 3.0), (5.0, 4.0), (6.0, 5.0), (8.0, 6.0)] {
        c.add(
            P,
            PU,
            &format!("PUMP_END_SUCTION_{}X{}", num(s), num(d)),
            format!("Pump, end suction {}x{} (generic footprint)", inches(s), inches(d)),
            "base mounted centrifugal",
            pump_end_suction,
            args(&[s, d]),
            REAL,
        );
    }
    for s in [1.5, 2.0, 3.0, 4.0, 6.0] {
        c.add(
            P,
            PU,
            &format!("PUMP_VERT_INLINE_{}", num(s)),
            format!("Pump, vertical inline {} (generic footprint)", inches(s)),
            "centrifugal",
            pump_inline,
            args(&[s]),
            REAL,
        );
    }
    c.add(P, PU, "PUMP_CIRCULATOR", "Pump, inline circulator".into(), "recirculation", pump_circulator, args(&[]), REAL);
    for (word, label, n, dia) in [
        ("SUMP_DUPLEX", "sump, duplex basin", 2.0, 30.0),
        ("EJECTOR_SIMPLEX", "sewage ejector, simplex", 1.0, 24.0),
        ("EJECTOR_DUPLEX", "sewage ejector, duplex", 2.0, 36.0),
    ] {
        c.add(P, PU, &format!("PUMP_{word}"), format!("Pump, {label}"), "basin pit", pump_basin, args(&[n, dia]), REAL);
    }
    for (gal, dia, ht) in [(30.0, 20.0, 50.0), (40.0, 22.0, 52.0), (50.0, 24.0, 58.0), (75.0, 28.0, 62.0), (100.0, 30.0, 70.0)] {
        c.add(
            P,
            PU,
            &format!("WATER_HEATER_{}GAL", num(gal)),
            format!("Water heater, {gal} gallon tank"),
            "domestic hot water dhw",
            tank,
            args(&[dia, ht, 1.0]),
            REAL,
        );
    }
    c.add(P, PU, "WATER_HEATER_TANKLESS", "Water heater, tankless".into(), "domestic hot water dhw instantaneous", tankless, args(&[]), REAL);
    for (gal, dia, ht) in [(2.0, 8.0, 12.0), (5.0, 11.0, 15.0), (10.0, 14.0, 20.0), (20.0, 16.0, 30.0)] {
        c.add(
            P,
            PU,
            &format!("EXPANSION_TANK_{}GAL", num(gal)),
            format!("Expansion tank, {gal} gallon"),
            "thermal",
            tank,
            args(&[dia, ht, 0.0]),
            REAL,
        );
    }
    for gpm in [20.0, 35.0, 50.0] {
        c.add(
            P,
            PU,
            &format!("GREASE_INTERCEPTOR_{}GPM", num(gpm)),
            format!("Grease interceptor, {gpm} gpm (generic footprint)"),
            "trap",
            interceptor,
            args(&[gpm]),
            REAL,
        );
    }
    const DR: &str = "Drains";
    for d in [4.0, 6.0, 8.0] {
        c.add(
            P,
            DR,
            &format!("FLOOR_DRAIN_ROUND_{}", num(d)),
            format!("Floor drain, round {} strainer", inches(d)),
            "fd",
            drain,
            args(&[d, 0.0]),
            REAL,
        );
    }
    for d in [6.0, 8.0, 12.0] {
        c.add(
            P,
            DR,
            &format!("FLOOR_DRAIN_SQ_{}", num(d)),
            format!("Floor drain, square {} strainer", inches(d)),
            "fd",
            drain,
            args(&[d, 1.0]),
            REAL,
        );
    }
    c.add(P, DR, "FLOOR_SINK_12", "Floor sink, 12\" square".into(), "fs indirect waste", drain, args(&[12.0, 2.0]), REAL);
    c.add(P, DR, "ROOF_DRAIN_15", "Roof drain, 15\" dome".into(), "rd storm", drain, args(&[15.0, 3.0]), REAL);
    c.add(P, DR, "CLEANOUT_FLOOR", "Cleanout, floor".into(), "fco", drain, args(&[5.0, 4.0]), REAL);
    for l in [24.0, 36.0, 48.0, 72.0, 96.0, 120.0] {
        c.add(
            P,
            DR,
            &format!("TRENCH_DRAIN_{}", num(l)),
            format!("Trench drain, {} long", inches(l)),
            "channel linear grate",
            trench_drain,
            args(&[l]),
            REAL,
        );
    }
}

/// A valve symbol 0.25" long on the sheet, its pipe along X.
fn valve(k: &mut Kit, a: &Args) {
    let kind = a.get(0) as i64;
    let (l, h) = (0.125, 0.075);
    let bowtie = |k: &mut Kit| {
        k.poly(&[(-l, h), (0.0, 0.0), (-l, -h)]);
        k.poly(&[(l, h), (0.0, 0.0), (l, -h)]);
    };
    match kind {
        0 => bowtie(k),
        1 => {
            bowtie(k);
            k.dot((0.0, 0.0), 0.035);
        }
        2 => {
            bowtie(k);
            k.circle((0.0, 0.0), 0.035);
        }
        3 => {
            k.line((-0.03, -h), (-0.03, h));
            k.line((0.03, -h), (0.03, h));
            k.line((-0.03, -h * 0.5), (0.03, h * 0.5));
        }
        4 => {
            k.line((-l, -h), (-l, h));
            k.line((l, -h), (l, h));
            k.line((-l, -h), (l, h));
            k.fill(&[(l, h), (l - 0.06, h - 0.015), (l - 0.035, h - 0.06)]);
        }
        5 => {
            k.poly(&[(-l, h), (l, h), (l, -h), (-l, -h)]);
            k.circle((0.0, 0.0), 0.04);
        }
        6 => {
            bowtie(k);
            k.line((0.0, 0.0), (0.0, 0.12));
            k.fill(&[(0.0, 0.0), (-0.025, 0.04), (0.025, 0.04)]);
        }
        7 => {
            k.poly(&[(-l, h), (0.0, 0.0), (-l, -h)]);
            k.poly(&[(-h, -l), (0.0, 0.0), (h, -l)]);
        }
        8 => {
            bowtie(k);
            k.poly(&[(-h, -l), (0.0, 0.0), (h, -l)]);
        }
        9 => {
            bowtie(k);
            k.line((0.0, 0.0), (0.0, 0.1));
            k.arc((0.0, 0.14), 0.04, 180.0, 360.0);
            k.line((-0.04, 0.14), (0.04, 0.14));
        }
        10 => {
            k.poly(&[(-l, h), (0.0, 0.0), (-l, -h)]);
            k.poly(&[(-h, l), (0.0, 0.0), (h, l)]);
            k.path(&[(0.0, l), (0.03, l + 0.02), (-0.03, l + 0.04), (0.03, l + 0.06)]);
        }
        11 => {
            bowtie(k);
            k.line((0.0, 0.0), (0.0, 0.08));
            k.rect((-0.04, 0.08), (0.04, 0.14));
            k.text((0.0, 0.11), 0.04, "S");
        }
        12 => {
            bowtie(k);
            k.line((-0.05, -0.1), (0.05, 0.1));
            k.fill(&[(0.05, 0.1), (0.015, 0.085), (0.045, 0.06)]);
        }
        13 | 14 => {
            k.rect((-0.2, -h), (0.2, h));
            k.line((-0.06, -h), (-0.06, h));
            k.line((0.06, -h), (0.06, h));
            k.text((0.0, -h - 0.06), 0.05, if kind == 13 { "RPZ" } else { "DCVA" });
        }
        _ => {
            k.line((-l, 0.0), (0.0, 0.0));
            k.pline(&[(0.0, h, 0.0), (0.0, -h, 0.0)], false);
            k.arc((0.0, -h - 0.04), 0.04, 270.0, 90.0);
            k.text((0.0, h + 0.05), 0.05, "HB");
        }
    }
    k.attdef("SIZE", "Valve size", "", (0.0, -0.17), 0.06);
}

fn fitting(k: &mut Kit, a: &Args) {
    let kind = a.get(0) as i64;
    let r = 0.06;
    match kind {
        0 => {
            k.line((-0.2, 0.0), (0.0, 0.0));
            k.circle((0.0, 0.0), r);
            k.dot((0.0, 0.0), 0.025);
        }
        1 => {
            k.line((-0.2, 0.0), (-r, 0.0));
            k.arc((0.0, 0.0), r, 0.0, 360.0);
        }
        2 => {
            k.line((-0.2, 0.0), (0.2, 0.0));
            k.circle((0.0, 0.0), r);
            k.dot((0.0, 0.0), 0.025);
        }
        3 => {
            k.line((-0.2, 0.0), (-r, 0.0));
            k.line((r, 0.0), (0.2, 0.0));
            k.arc((0.0, 0.0), r, 180.0, 0.0);
            k.line((-r, 0.0), (r, 0.0));
        }
        4 => {
            k.line((-0.2, 0.0), (0.2, 0.0));
            k.line((-0.03, -0.08), (-0.03, 0.08));
            k.line((0.03, -0.08), (0.03, 0.08));
            k.line((0.0, -0.05), (0.0, 0.05));
        }
        5 => {
            k.line((-0.2, 0.0), (0.0, 0.0));
            k.arc((0.0, 0.0), 0.05, 270.0, 90.0);
        }
        6 => {
            k.line((-0.2, 0.0), (-0.06, 0.0));
            k.line((0.06, 0.0), (0.2, 0.0));
            k.poly(&[(-0.06, 0.08), (0.06, 0.04), (0.06, -0.04), (-0.06, -0.08)]);
        }
        _ => {
            k.line((-0.2, 0.0), (0.2, 0.0));
            k.line((-0.02, -0.09), (-0.02, 0.09));
            k.line((0.02, -0.09), (0.02, 0.09));
        }
    }
}

fn pump_end_suction(k: &mut Kit, a: &Args) {
    let (s, d) = (a.get(0), a.get(1));
    let l = 40.0 + s * 8.0;
    let w = 14.0 + s * 2.5;
    // Base, motor and volute; suction on the end (−X), discharge up (+Y).
    k.rect((0.0, -w / 2.0), (l, w / 2.0));
    k.rect((l * 0.4, -w * 0.35), (l * 0.95, w * 0.35));
    k.circle((l * 0.22, 0.0), w * 0.38);
    k.rect((-6.0, -s / 2.0), (l * 0.22 - w * 0.38, s / 2.0));
    k.rect((l * 0.22 - d / 2.0, w * 0.38), (l * 0.22 + d / 2.0, w * 0.38 + 6.0));
    k.cube((0.0, -w / 2.0), (l, w / 2.0), 0.0, 6.0);
    k.cube((l * 0.4, -w * 0.35), (l * 0.95, w * 0.35), 6.0, 6.0 + w * 0.7);
    k.cylinder((l * 0.22, 0.0), w * 0.38, 6.0, 6.0 + w * 0.76);
    k.attdef_hidden("TAG", "Pump tag", "", (0.0, -w / 2.0 - 4.0), 3.0);
}

fn pump_inline(k: &mut Kit, a: &Args) {
    let s = a.get(0);
    let r = 6.0 + s * 2.0;
    k.circle((0.0, 0.0), r);
    k.circle((0.0, 0.0), r * 0.6);
    k.rect((-r - 6.0, -s / 2.0), (-r, s / 2.0));
    k.rect((r, -s / 2.0), (r + 6.0, s / 2.0));
    k.cylinder((0.0, 0.0), r, 0.0, r * 2.5);
    k.attdef_hidden("TAG", "Pump tag", "", (-r, -r - 4.0), 3.0);
}

fn pump_circulator(k: &mut Kit, _: &Args) {
    k.circle((0.0, 0.0), 3.0);
    k.line((-6.0, 0.0), (-3.0, 0.0));
    k.line((3.0, 0.0), (6.0, 0.0));
    k.rect((-2.0, 3.0), (2.0, 8.0));
    arrow(k, (-1.5, -4.5), (1.5, -4.5), 1.0);
}

fn pump_basin(k: &mut Kit, a: &Args) {
    let (n, dia) = (a.get(0) as usize, a.get(1));
    let r = dia / 2.0;
    k.circle((0.0, 0.0), r);
    k.circle((0.0, 0.0), r + 2.0);
    for i in 0..n {
        let x = if n == 1 {
            0.0
        } else if i == 0 {
            -r * 0.4
        } else {
            r * 0.4
        };
        k.circle((x, 0.0), r * 0.25);
    }
    k.cylinder((0.0, 0.0), r + 2.0, -dia * 1.5, 0.0);
}

/// An upright tank (water heater when `burner`).
fn tank(k: &mut Kit, a: &Args) {
    let (dia, ht, heater) = (a.get(0), a.get(1), a.get(2) > 0.5);
    let r = dia / 2.0;
    k.circle((0.0, 0.0), r);
    if heater {
        k.circle((0.0, 0.0), 2.0);
        k.text((0.0, -r * 0.5), (r * 0.25).min(3.0), "WH");
    } else {
        k.text((0.0, 0.0), (r * 0.35).min(2.5), "ET");
    }
    k.cylinder((0.0, 0.0), r, 0.0, ht);
}

fn tankless(k: &mut Kit, _: &Args) {
    k.rect((-9.0, 0.0), (9.0, -10.0));
    k.text((0.0, -5.0), 2.5, "TWH");
    k.cube((-9.0, 0.0), (9.0, -10.0), 48.0, 76.0);
}

fn interceptor(k: &mut Kit, a: &Args) {
    let g = a.get(0);
    let (l, w) = (20.0 + g * 0.6, 14.0 + g * 0.3);
    k.rect((0.0, 0.0), (l, w));
    k.line((l * 0.33, 0.0), (l * 0.33, w * 0.7));
    k.line((l * 0.66, w * 0.3), (l * 0.66, w));
    k.text((l / 2.0, -3.0), 2.5, "GI");
    k.cube((0.0, 0.0), (l, w), -w, 0.0);
}

/// Drains: round (0), square (1), floor sink (2), roof drain (3), floor cleanout (4).
fn drain(k: &mut Kit, a: &Args) {
    let (d, kind) = (a.get(0), a.get(1) as i64);
    let r = d / 2.0;
    match kind {
        0 => {
            k.circle((0.0, 0.0), r);
            k.circle((0.0, 0.0), r * 0.6);
            k.line((-r, 0.0), (r, 0.0));
        }
        1 => {
            k.rect((-r, -r), (r, r));
            for f in [-0.5, 0.0, 0.5] {
                k.line((-r * 0.8, r * f), (r * 0.8, r * f));
            }
        }
        2 => {
            k.rect((-r, -r), (r, r));
            k.rect((-r + 1.0, -r + 1.0), (r - 1.0, r - 1.0));
            k.line((-r + 1.0, -r + 1.0), (r - 1.0, r - 1.0));
            k.line((r - 1.0, -r + 1.0), (-r + 1.0, r - 1.0));
        }
        3 => {
            k.circle((0.0, 0.0), r);
            k.circle((0.0, 0.0), r * 0.55);
            for i in 0..8 {
                let t = f64::from(i) * std::f64::consts::FRAC_PI_4;
                k.line((r * 0.55 * t.cos(), r * 0.55 * t.sin()), (r * t.cos(), r * t.sin()));
            }
        }
        _ => {
            k.circle((0.0, 0.0), r);
            k.dot((0.0, 0.0), r * 0.4);
        }
    }
}

fn trench_drain(k: &mut Kit, a: &Args) {
    let l = a.get(0);
    let w = 6.0;
    k.rect((0.0, 0.0), (l, w));
    k.line((0.0, 1.0), (l, 1.0));
    k.line((0.0, w - 1.0), (l, w - 1.0));
    k.circle((l / 2.0, w / 2.0), 1.5);
    k.stretch("Length", (0.0, 0.0), (l, 0.0), (l * 0.75, -1.0), (l + 1.0, w + 1.0), 12.0, 480.0, 12.0);
}

// ---------------------------------------------------------------------------------------------
// Electrical

const E: Discipline = Discipline::Electrical;

fn electrical(c: &mut Catalog) {
    const PD: &str = "Power distribution";
    for (word, label, w, d, flush) in [
        ("PANEL_20_SURFACE", "Panelboard, 20\" surface mounted", 20.0, 5.75, 0.0),
        ("PANEL_20_FLUSH", "Panelboard, 20\" flush mounted", 20.0, 5.75, 1.0),
        ("PANEL_24_SURFACE", "Panelboard, 24\" surface mounted", 24.0, 5.75, 0.0),
        ("PANEL_24_FLUSH", "Panelboard, 24\" flush mounted", 24.0, 5.75, 1.0),
        ("PANEL_DIST_32", "Distribution panel, 32\" wide", 32.0, 9.5, 0.0),
        ("PANEL_DIST_44", "Distribution panel, 44\" wide", 44.0, 12.0, 0.0),
        ("LOAD_CENTER_12", "Load center, 12\" residential", 12.0, 3.75, 1.0),
        ("LOAD_CENTER_14", "Load center, 14\" residential", 14.25, 3.75, 1.0),
    ] {
        c.add(E, PD, word, label.into(), "panel board breaker", panel, args(&[w, d, flush]), REAL);
    }
    for n in 1..=6 {
        let n = f64::from(n);
        c.add(
            E,
            PD,
            &format!("SWITCHBOARD_{n}SEC"),
            format!("Switchboard, {n} section{}", if n > 1.0 { "s" } else { "" }),
            "switchgear service",
            sections,
            args(&[n, 36.0, 30.0, 0.0]),
            REAL,
        );
    }
    for n in 1..=4 {
        let n = f64::from(n);
        c.add(
            E,
            PD,
            &format!("MCC_{n}SEC"),
            format!("Motor control center, {n} section{}", if n > 1.0 { "s" } else { "" }),
            "starters mcc",
            sections,
            args(&[n, 20.0, 20.0, 1.0]),
            REAL,
        );
    }
    for kva in [15.0, 30.0, 45.0, 75.0, 112.5, 150.0, 225.0, 300.0, 500.0] {
        c.add(
            E,
            PD,
            &format!("XFMR_{}KVA", num(kva)),
            format!("Transformer, dry type {kva} kVA (generic footprint)"),
            "transformer xfmr",
            transformer,
            args(&[kva]),
            REAL,
        );
    }
    for amps in [30.0, 60.0, 100.0, 200.0, 400.0, 600.0] {
        c.add(E, PD, &format!("DISCONNECT_{}A", num(amps)), format!("Disconnect switch, {amps} A"), "safety switch", disconnect, args(&[amps]), REAL);
    }
    for kw in [20.0, 50.0, 100.0, 200.0, 500.0] {
        c.add(
            E,
            PD,
            &format!("GENERATOR_{}KW", num(kw)),
            format!("Generator, {kw} kW (generic footprint)"),
            "standby emergency genset",
            generator,
            args(&[kw]),
            REAL,
        );
    }
    for amps in [100.0, 200.0, 400.0, 800.0] {
        c.add(E, PD, &format!("ATS_{}A", num(amps)), format!("Automatic transfer switch, {amps} A"), "ats emergency", ats, args(&[amps]), REAL);
    }
    c.add(E, PD, "EV_CHARGER_WALL", "EV charger, wall mounted".into(), "electric vehicle evse", ev_charger, args(&[0.0]), REAL);
    c.add(E, PD, "EV_CHARGER_PEDESTAL", "EV charger, pedestal".into(), "electric vehicle evse", ev_charger, args(&[1.0]), REAL);
    c.add(E, PD, "FLOOR_BOX", "Floor box, 4-gang".into(), "poke-through outlet", floor_box, args(&[]), REAL);
    for w in [6.0, 12.0, 18.0, 24.0, 30.0, 36.0] {
        c.add(
            E,
            PD,
            &format!("CABLE_TRAY_{}", num(w)),
            format!("Cable tray, ladder {} wide, 10' section", inches(w)),
            "raceway",
            cable_tray,
            args(&[w]),
            REAL,
        );
    }
    const LF: &str = "Light fixtures";
    for (word, label, w, l) in
        [("TROFFER_1X4", "Troffer, 1'x4'", 12.0, 48.0), ("TROFFER_2X2", "Troffer, 2'x2'", 24.0, 24.0), ("TROFFER_2X4", "Troffer, 2'x4'", 24.0, 48.0)]
    {
        c.add(E, LF, word, label.into(), "luminaire recessed lay-in", troffer, args(&[w, l]), REAL);
    }
    for d in [4.0, 6.0, 8.0] {
        c.add(E, LF, &format!("DOWNLIGHT_{}", num(d)), format!("Downlight, {} recessed", inches(d)), "luminaire can", downlight, args(&[d]), REAL);
    }
    for (word, label, l, kind) in [
        ("STRIP_4", "Strip light, 4'", 48.0, 0.0),
        ("STRIP_8", "Strip light, 8'", 96.0, 0.0),
        ("PENDANT_LINEAR_4", "Linear pendant, 4'", 48.0, 1.0),
        ("PENDANT_LINEAR_8", "Linear pendant, 8'", 96.0, 1.0),
        ("PENDANT_LINEAR_12", "Linear pendant, 12'", 144.0, 1.0),
        ("TRACK_4", "Track light, 4' with 3 heads", 48.0, 2.0),
        ("TRACK_8", "Track light, 8' with 5 heads", 96.0, 2.0),
        ("HIGH_BAY_LINEAR", "High bay, linear 2'x4'", 48.0, 3.0),
    ] {
        c.add(E, LF, word, label.into(), "luminaire", linear_light, args(&[l, kind]), REAL);
    }
    for (word, label, kind) in [
        ("HIGH_BAY_ROUND", "High bay, round 16\"", 0.0),
        ("WALL_PACK", "Wall pack", 1.0),
        ("EXIT_CEILING", "Exit sign, ceiling mounted", 2.0),
        ("EXIT_WALL", "Exit sign, wall mounted", 3.0),
        ("EMERGENCY_TWIN_HEAD", "Emergency light, twin head", 4.0),
        ("SCONCE", "Wall sconce", 5.0),
    ] {
        c.add(E, LF, word, label.into(), "luminaire", fixture, args(&[kind]), REAL);
    }
}

fn panel(k: &mut Kit, a: &Args) {
    let (w, d, flush) = (a.get(0), a.get(1), a.get(2) > 0.5);
    // Back on the wall face (Y 0); a flush panel sits in the wall.
    let (y0, y1) = if flush { (d - 1.0, -1.0) } else { (0.0, -d) };
    k.rect((-w / 2.0, y0), (w / 2.0, y1));
    k.fill(&[(-w / 2.0, y1), (w / 2.0, y1), (w / 2.0, y1 + 1.0), (-w / 2.0, y1 + 1.0)]);
    k.cube((-w / 2.0, y0), (w / 2.0, y1), 18.0, 18.0 + w * 2.5);
    k.attdef("TAG", "Panel name", "", (-w / 2.0, y1 - 4.0), 3.0);
}

/// Freestanding gear in sections of `w` x `d`; `mcc` gets the bucket lines.
fn sections(k: &mut Kit, a: &Args) {
    let (n, w, d, mcc) = (a.get(0) as usize, a.get(1), a.get(2), a.get(3) > 0.5);
    let l = w * n as f64;
    k.rect((0.0, 0.0), (l, -d));
    for i in 1..n {
        let x = w * i as f64;
        k.line((x, 0.0), (x, -d));
    }
    if mcc {
        k.line((0.0, -d + 2.0), (l, -d + 2.0));
    } else {
        k.fill(&[(0.0, -d), (l, -d), (l, -d + 1.5), (0.0, -d + 1.5)]);
    }
    k.cube((0.0, 0.0), (l, -d), 0.0, 90.0);
    k.attdef("TAG", "Equipment name", "", (0.0, -d - 4.0), 3.0);
}

fn transformer(k: &mut Kit, a: &Args) {
    let kva = a.get(0);
    let w = 20.0 + kva.sqrt() * 2.2;
    let d = 16.0 + kva.sqrt() * 1.3;
    k.rect((0.0, 0.0), (w, -d));
    k.line((0.0, 0.0), (w, -d));
    k.line((w, 0.0), (0.0, -d));
    k.cube((0.0, 0.0), (w, -d), 0.0, 30.0 + kva.sqrt() * 3.0);
    k.attdef("TAG", "Transformer name", "", (0.0, -d - 4.0), 3.0);
}

fn disconnect(k: &mut Kit, a: &Args) {
    let amps = a.get(0);
    let w = 7.0 + amps.sqrt() * 0.6;
    let d = 4.0 + amps.sqrt() * 0.25;
    k.rect((-w / 2.0, 0.0), (w / 2.0, -d));
    k.line((-w / 2.0, -d), (w / 2.0, 0.0));
    k.attdef_hidden("TAG", "Disconnect name", "", (-w / 2.0, -d - 3.0), 2.5);
}

fn generator(k: &mut Kit, a: &Args) {
    let kw = a.get(0);
    let l = 60.0 + kw.sqrt() * 9.0;
    let w = 30.0 + kw.sqrt() * 3.0;
    k.rect((0.0, 0.0), (l, w));
    k.rect((l * 0.05, w * 0.15), (l * 0.6, w * 0.85));
    k.circle((l * 0.8, w / 2.0), w * 0.25);
    k.text((l * 0.33, w / 2.0), (w * 0.15).min(6.0), "GEN");
    k.cube((0.0, 0.0), (l, w), 0.0, 40.0 + kw.sqrt() * 2.5);
    k.attdef("TAG", "Generator name", "", (0.0, -4.0), 3.0);
}

fn ats(k: &mut Kit, a: &Args) {
    let amps = a.get(0);
    let w = 20.0 + amps.sqrt() * 0.9;
    let d = 12.0 + amps.sqrt() * 0.35;
    k.rect((-w / 2.0, 0.0), (w / 2.0, -d));
    k.text((0.0, -d / 2.0), 2.5, "ATS");
    k.cube((-w / 2.0, 0.0), (w / 2.0, -d), if amps > 300.0 { 0.0 } else { 24.0 }, if amps > 300.0 { 84.0 } else { 72.0 });
    k.attdef("TAG", "Switch name", "", (-w / 2.0, -d - 3.0), 2.5);
}

fn ev_charger(k: &mut Kit, a: &Args) {
    if a.get(0) > 0.5 {
        k.rect((-6.0, -6.0), (6.0, 6.0));
        k.circle((0.0, 0.0), 3.0);
        k.text((0.0, -9.0), 2.5, "EV");
        k.cube((-6.0, -6.0), (6.0, 6.0), 0.0, 54.0);
    } else {
        k.rect((-7.0, 0.0), (7.0, -5.0));
        k.text((0.0, -2.5), 2.5, "EV");
        k.cube((-7.0, 0.0), (7.0, -5.0), 36.0, 54.0);
    }
}

fn floor_box(k: &mut Kit, _: &Args) {
    k.rect((-5.0, -5.0), (5.0, 5.0));
    k.rect((-4.0, -4.0), (4.0, 4.0));
    k.line((0.0, -4.0), (0.0, 4.0));
    k.line((-4.0, 0.0), (4.0, 0.0));
}

fn cable_tray(k: &mut Kit, a: &Args) {
    let w = a.get(0);
    let l = 120.0;
    k.line((0.0, 0.0), (l, 0.0));
    k.line((0.0, w), (l, w));
    let rungs = 13;
    for i in 0..rungs {
        let x = 4.0 + i as f64 * 9.0;
        k.line((x, 0.0), (x, w));
    }
    k.stretch("Length", (0.0, -2.0), (l, -2.0), (l - 2.0, -4.0), (l + 2.0, w + 4.0), 12.0, 240.0, 12.0);
}

fn troffer(k: &mut Kit, a: &Args) {
    let (w, l) = (a.get(0), a.get(1));
    k.rect((0.0, 0.0), (l, w));
    k.rect((2.0, 2.0), (l - 2.0, w - 2.0));
    let lamps = if w > 18.0 { 3 } else { 2 };
    for i in 1..=lamps {
        let y = w * i as f64 / (lamps as f64 + 1.0);
        k.line((3.0, y), (l - 3.0, y));
    }
    k.attdef_hidden("TYPE_MARK", "Fixture type", "", (l + 2.0, w), 2.5);
}

fn downlight(k: &mut Kit, a: &Args) {
    let d = a.get(0);
    k.circle((0.0, 0.0), d / 2.0 + 1.0);
    k.circle((0.0, 0.0), d / 2.0);
    k.attdef_hidden("TYPE_MARK", "Fixture type", "", (d / 2.0 + 2.0, 0.0), 2.5);
}

/// Strip (0), pendant (1), track (2) or linear high bay (3), `l` long along X.
fn linear_light(k: &mut Kit, a: &Args) {
    let (l, kind) = (a.get(0), a.get(1) as i64);
    match kind {
        0 => {
            k.rect((0.0, 0.0), (l, 4.0));
            k.line((2.0, 2.0), (l - 2.0, 2.0));
        }
        1 => {
            k.rect((0.0, 0.0), (l, 3.0));
            k.circle((l * 0.2, 1.5), 0.75);
            k.circle((l * 0.8, 1.5), 0.75);
        }
        2 => {
            k.rect((0.0, 0.0), (l, 1.5));
            let heads = if l > 60.0 { 5 } else { 3 };
            for i in 0..heads {
                let x = l * (i as f64 + 0.5) / heads as f64;
                k.circle((x, 0.75), 2.5);
            }
        }
        _ => {
            k.rect((0.0, 0.0), (l, 24.0));
            for f in [0.25, 0.5, 0.75] {
                k.line((2.0, 24.0 * f), (l - 2.0, 24.0 * f));
            }
            k.line((0.0, 0.0), (l, 24.0));
        }
    }
    k.attdef_hidden("TYPE_MARK", "Fixture type", "", (l + 2.0, 0.0), 2.5);
}

fn fixture(k: &mut Kit, a: &Args) {
    match a.get(0) as i64 {
        0 => {
            k.circle((0.0, 0.0), 8.0);
            k.circle((0.0, 0.0), 5.0);
            for i in 0..6 {
                let t = f64::from(i) * std::f64::consts::FRAC_PI_3;
                k.line((5.0 * t.cos(), 5.0 * t.sin()), (8.0 * t.cos(), 8.0 * t.sin()));
            }
        }
        1 => {
            k.rect((-7.0, 0.0), (7.0, -6.0));
            k.poly(&[(-5.0, -6.0), (5.0, -6.0), (3.0, -9.0), (-3.0, -9.0)]);
        }
        2 => {
            k.rect((-6.0, -1.0), (6.0, 1.0));
            k.text((0.0, 4.0), 2.0, "EXIT");
            k.fill(&[(4.0, -2.5), (6.0, -4.0), (4.0, -5.5)]);
            k.fill(&[(-4.0, -2.5), (-6.0, -4.0), (-4.0, -5.5)]);
        }
        3 => {
            k.rect((-6.0, 0.0), (6.0, -1.5));
            k.text((0.0, -4.0), 2.0, "EXIT");
        }
        4 => {
            k.rect((-6.0, 0.0), (6.0, -4.0));
            k.circle((-4.5, -6.0), 2.0);
            k.circle((4.5, -6.0), 2.0);
        }
        _ => {
            k.pline(&[(-5.0, 0.0, 0.0), (5.0, 0.0, 1.0)], true);
        }
    }
    k.attdef_hidden("TYPE_MARK", "Fixture type", "", (8.0, 0.0), 2.5);
}
