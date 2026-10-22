//! Architectural parts: doors, windows, stairs and ramps, furniture, sanitary fixtures and
//! kitchens. Plan views at real size from nominal sizes; doors and windows sit on the wall line
//! (Y 0) with their opening from X 0 to the width; fixtures and cabinets stand against the wall
//! with their back edge on Y 0, extending in −Y; free-standing furniture is centred.

use super::{Args, Catalog, Discipline, Kit, METRIC, REAL, args, inches, num};

const D: Discipline = Discipline::Architectural;

pub(crate) fn add(c: &mut Catalog) {
    doors(c);
    windows(c);
    stairs(c);
    furniture(c);
    sanitary(c);
    kitchen(c);
}

// ---------------------------------------------------------------------------------------------
// Doors

fn doors(c: &mut Catalog) {
    for w in [24.0, 28.0, 30.0, 32.0, 34.0, 36.0, 40.0, 42.0] {
        for (hand, h) in [("LH", 0.0), ("RH", 1.0)] {
            let label = format!("Door, single swing {} {}", inches(w), if h == 0.0 { "left hand" } else { "right hand" });
            c.add(D, "Doors", &format!("DOOR_SGL_{}_{hand}", num(w)), label, "swing hinged leaf", door_single, args(&[w, h]), REAL);
        }
    }
    for w in [700.0, 750.0, 800.0, 850.0, 900.0, 1000.0] {
        for (hand, h) in [("LH", 0.0), ("RH", 1.0)] {
            let label = format!("Door, single swing {w} mm {}", if h == 0.0 { "left hand" } else { "right hand" });
            c.add(D, "Doors", &format!("DOOR_SGL_M{}_{hand}", num(w)), label, "swing hinged leaf metric", door_single, args(&[w, h]), METRIC);
        }
    }
    for w in [48.0, 56.0, 60.0, 64.0, 72.0] {
        c.add(
            D,
            "Doors",
            &format!("DOOR_DBL_{}", num(w)),
            format!("Door, double swing pair {}", inches(w)),
            "pair french",
            door_double,
            args(&[w]),
            REAL,
        );
    }
    for w in [24.0, 28.0, 30.0, 32.0, 36.0] {
        c.add(D, "Doors", &format!("DOOR_POCKET_{}", num(w)), format!("Door, pocket {}", inches(w)), "sliding cavity", door_pocket, args(&[w]), REAL);
    }
    for w in [24.0, 30.0, 36.0, 48.0, 60.0, 72.0] {
        c.add(D, "Doors", &format!("DOOR_BIFOLD_{}", num(w)), format!("Door, bifold {}", inches(w)), "closet folding", door_bifold, args(&[w]), REAL);
    }
    for w in [48.0, 60.0, 72.0, 84.0, 96.0] {
        c.add(
            D,
            "Doors",
            &format!("DOOR_BYPASS_{}", num(w)),
            format!("Door, bypass sliding {}", inches(w)),
            "closet slider",
            door_bypass,
            args(&[w]),
            REAL,
        );
    }
    for w in [30.0, 36.0, 42.0, 48.0] {
        c.add(
            D,
            "Doors",
            &format!("DOOR_BARN_{}", num(w)),
            format!("Door, barn slider {}", inches(w)),
            "surface sliding track",
            door_barn,
            args(&[w]),
            REAL,
        );
    }
    for (w, h) in [(96.0, 84.0), (108.0, 84.0), (120.0, 96.0), (144.0, 96.0), (192.0, 84.0), (216.0, 96.0)] {
        c.add(
            D,
            "Doors",
            &format!("DOOR_OVERHEAD_{}", num(w)),
            format!("Door, overhead sectional {} wide", inches(w)),
            "garage roll-up",
            door_overhead,
            args(&[w, h]),
            REAL,
        );
    }
    for dia in [72.0, 84.0, 96.0] {
        c.add(
            D,
            "Doors",
            &format!("DOOR_REVOLVING_{}", num(dia)),
            format!("Door, revolving {} diameter", inches(dia)),
            "entrance",
            door_revolving,
            args(&[dia]),
            REAL,
        );
    }
}

/// Jamb marks at both sides of an opening `w` wide in a wall `t` thick.
fn jambs(k: &mut Kit, w: f64, t: f64) {
    k.line((0.0, -t / 2.0), (0.0, t / 2.0));
    k.line((w, -t / 2.0), (w, t / 2.0));
}

/// One swinging leaf `w` wide hinged at `hx`, the leaf running toward +X (`dir` 1) or −X, opening
/// to −Y (`side` −1) or +Y.
fn leaf(k: &mut Kit, hx: f64, w: f64, dir: f64, side: f64) {
    let t = k.inch(1.75);
    k.rect((hx, 0.0), (hx + dir * t, side * w));
    let (a0, a1) = match (dir > 0.0, side > 0.0) {
        (true, false) => (270.0, 360.0),
        (true, true) => (0.0, 90.0),
        (false, false) => (180.0, 270.0),
        (false, true) => (90.0, 180.0),
    };
    k.arc((hx, 0.0), w, a0, a1);
}

fn door_single(k: &mut Kit, a: &Args) {
    let (w, rh) = (a.get(0), a.get(1) > 0.5);
    let t = k.inch(4.5);
    jambs(k, w, t);
    let (hx, dir) = if rh { (w, -1.0) } else { (0.0, 1.0) };
    k.visibility("Swing", &["Swing in", "Swing out"]);
    for (g, side) in [(0, -1.0), (1, 1.0)] {
        k.state(Some(g));
        leaf(k, hx, w, dir, side);
    }
    k.state(None);
    k.attdef_hidden("TAG", "Door mark", "", (w / 2.0, 0.0), k.inch(4.0));
}

fn door_double(k: &mut Kit, a: &Args) {
    let w = a.get(0);
    jambs(k, w, 4.5);
    k.visibility("Swing", &["Swing in", "Swing out"]);
    for (g, side) in [(0, -1.0), (1, 1.0)] {
        k.state(Some(g));
        leaf(k, 0.0, w / 2.0, 1.0, side);
        leaf(k, w, w / 2.0, -1.0, side);
    }
    k.state(None);
    k.attdef_hidden("TAG", "Door mark", "", (w / 2.0, 0.0), 4.0);
}

fn door_pocket(k: &mut Kit, a: &Args) {
    let w = a.get(0);
    jambs(k, w, 4.5);
    // The pocket in the wall to the left.
    k.hidden(true);
    k.rect((-w, -1.25), (0.0, 1.25));
    k.hidden(false);
    k.visibility("Position", &["Half open", "Closed"]);
    k.state(Some(0));
    k.rect((-w / 2.0, -0.875), (w / 2.0, 0.875));
    k.state(Some(1));
    k.rect((0.0, -0.875), (w, 0.875));
    k.state(None);
    k.attdef_hidden("TAG", "Door mark", "", (w / 2.0, 0.0), 4.0);
}

fn door_bifold(k: &mut Kit, a: &Args) {
    let w = a.get(0);
    jambs(k, w, 4.5);
    let q = w / 4.0;
    let (ca, sa) = (30f64.to_radians().cos() * q, 30f64.to_radians().sin() * q);
    k.visibility("Position", &["Half open", "Closed"]);
    k.state(Some(0));
    k.path(&[(0.0, 0.0), (ca, -sa), (2.0 * ca, 0.0)]);
    k.path(&[(w, 0.0), (w - ca, -sa), (w - 2.0 * ca, 0.0)]);
    k.state(Some(1));
    k.rect((0.0, -0.5), (w / 2.0, 0.5));
    k.rect((w / 2.0, -0.5), (w, 0.5));
    k.line((q, -0.5), (q, 0.5));
    k.line((3.0 * q, -0.5), (3.0 * q, 0.5));
    k.state(None);
    k.attdef_hidden("TAG", "Door mark", "", (w / 2.0, 0.0), 4.0);
}

fn door_bypass(k: &mut Kit, a: &Args) {
    let w = a.get(0);
    jambs(k, w, 4.5);
    k.rect((0.0, 0.25), (w / 2.0 + 1.0, 1.625));
    k.rect((w / 2.0 - 1.0, -1.625), (w, -0.25));
    k.stretch("Width", (0.0, 0.0), (w, 0.0), (w * 0.75, -6.0), (w + 6.0, 6.0), 36.0, 144.0, 6.0);
    k.attdef_hidden("TAG", "Door mark", "", (w / 2.0, 0.0), 4.0);
}

fn door_barn(k: &mut Kit, a: &Args) {
    let w = a.get(0);
    jambs(k, w, 4.5);
    // The leaf hangs on the wall face, its track twice the opening.
    k.rect((-2.0, 3.0), (w + 2.0, 4.75));
    k.hidden(true);
    k.line((-w, 5.5), (w + 4.0, 5.5));
    k.hidden(false);
    k.stretch("Width", (0.0, 0.0), (w, 0.0), (w * 0.75, -6.0), (w + 8.0, 8.0), 24.0, 72.0, 2.0);
    k.attdef_hidden("TAG", "Door mark", "", (w / 2.0, 0.0), 4.0);
}

fn door_overhead(k: &mut Kit, a: &Args) {
    let (w, h) = (a.get(0), a.get(1));
    jambs(k, w, 8.0);
    k.rect((0.0, -1.0), (w, 1.0));
    // The open door overhead, inside.
    k.hidden(true);
    k.rect((0.0, -h - 1.0), (w, -1.0));
    k.hidden(false);
    k.stretch("Width", (0.0, 0.0), (w, 0.0), (w * 0.75, -h - 12.0), (w + 12.0, 12.0), 72.0, 288.0, 12.0);
    k.attdef_hidden("TAG", "Door mark", "", (w / 2.0, 0.0), 4.0);
}

fn door_revolving(k: &mut Kit, a: &Args) {
    let d = a.get(0);
    let r = d / 2.0;
    // The drum's two curved walls, open toward inside and outside.
    k.arc((0.0, 0.0), r + 1.0, 135.0, 225.0);
    k.arc((0.0, 0.0), r + 1.0, 315.0, 45.0);
    k.arc((0.0, 0.0), r + 3.0, 135.0, 225.0);
    k.arc((0.0, 0.0), r + 3.0, 315.0, 45.0);
    for ang in [45.0f64, 135.0, 225.0, 315.0] {
        let (s, c) = ang.to_radians().sin_cos();
        k.line((0.0, 0.0), (r * c, r * s));
    }
    k.circle((0.0, 0.0), 2.0);
    k.attdef_hidden("TAG", "Door mark", "", (0.0, -r - 8.0), 4.0);
}

// ---------------------------------------------------------------------------------------------
// Windows

/// Window types: (name word, label, code).
const WINDOW_TYPES: [(&str, &str, f64); 5] = [
    ("FIXED", "fixed", 0.0),
    ("CASEMENT", "casement", 1.0),
    ("DH", "double hung", 2.0),
    ("SLIDER", "horizontal slider", 3.0),
    ("AWNING", "awning", 4.0),
];

fn windows(c: &mut Catalog) {
    for (word, label, code) in WINDOW_TYPES {
        for w in [24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0] {
            c.add(
                D,
                "Windows",
                &format!("WIN_{word}_{}", num(w)),
                format!("Window, {label} {}", inches(w)),
                "glazing sash",
                window,
                args(&[w, code, 6.0]),
                REAL,
            );
        }
    }
    for (word, label, code) in [("FIXED", "fixed", 0.0), ("CASEMENT", "casement", 1.0), ("SLIDER", "horizontal slider", 3.0)] {
        for w in [600.0, 900.0, 1200.0, 1500.0, 1800.0, 2100.0, 2400.0] {
            c.add(
                D,
                "Windows",
                &format!("WIN_{word}_M{}", num(w)),
                format!("Window, {label} {w} mm"),
                "glazing sash metric",
                window,
                args(&[w, code, 150.0]),
                METRIC,
            );
        }
    }
    for (w, p) in [(72.0, 18.0), (96.0, 24.0), (120.0, 24.0)] {
        c.add(
            D,
            "Windows",
            &format!("WIN_BAY_{}", num(w)),
            format!("Window, bay {} wide", inches(w)),
            "projecting angled",
            window_bay,
            args(&[w, p]),
            REAL,
        );
    }
}

fn window(k: &mut Kit, a: &Args) {
    let (w, code, t) = (a.get(0), a.get(1) as i64, a.get(2));
    let g = k.inch(0.5);
    jambs(k, w, t);
    k.line((0.0, t / 2.0), (w, t / 2.0));
    k.line((0.0, -t / 2.0), (w, -t / 2.0));
    match code {
        1 => {
            // Casement: one sash per 36 inches, each drawn opening out.
            let n = (w / k.inch(36.0)).ceil().clamp(1.0, 4.0);
            let s = w / n;
            k.line((0.0, g), (w, g));
            k.line((0.0, -g), (w, -g));
            for i in 0..n as usize {
                let x0 = s * i as f64;
                let ang = 30f64.to_radians();
                k.line((x0, t / 2.0), (x0 + s * ang.cos(), t / 2.0 + s * ang.sin()));
                k.arc((x0, t / 2.0), s, 0.0, 30.0);
            }
        }
        2 => {
            for y in [2.0 * g, 0.0, -2.0 * g] {
                k.line((0.0, y), (w, y));
            }
        }
        3 => {
            k.line((0.0, g), (w / 2.0 + g * 2.0, g));
            k.line((w / 2.0 - g * 2.0, -g), (w, -g));
        }
        4 => {
            k.line((0.0, g), (w, g));
            k.line((0.0, -g), (w, -g));
            k.hidden(true);
            k.rect((0.0, t / 2.0), (w, t / 2.0 + w.min(k.inch(48.0)) * 0.25));
            k.hidden(false);
        }
        _ => {
            k.line((0.0, g), (w, g));
            k.line((0.0, -g), (w, -g));
        }
    }
    let (min, max, step) = if t > 20.0 { (300.0, 3600.0, 50.0) } else { (12.0, 144.0, 2.0) };
    k.stretch("Width", (0.0, 0.0), (w, 0.0), (w * 0.7, -t * 3.0), (w + t, t * 3.0 + w), min, max, step);
    k.attdef_hidden("TAG", "Window mark", "", (w / 2.0, 0.0), k.inch(4.0));
}

fn window_bay(k: &mut Kit, a: &Args) {
    let (w, p) = (a.get(0), a.get(1));
    let side = p;
    let pts = [(0.0, 0.0), (side, p), (w - side, p), (w, 0.0)];
    k.path(&pts);
    k.path(&[(0.0, -3.0), (side, p - 3.0), (w - side, p - 3.0), (w, -3.0)]);
    k.line((side, p), (side, p - 3.0));
    k.line((w - side, p), (w - side, p - 3.0));
    k.path(&[(0.0, -1.5), (side, p - 1.5), (w - side, p - 1.5), (w, -1.5)]);
    jambs(k, w, 6.0);
    k.attdef_hidden("TAG", "Window mark", "", (w / 2.0, 0.0), 4.0);
}

// ---------------------------------------------------------------------------------------------
// Stairs and ramps

/// Riser height and tread depth (inches) the stairs are drawn with.
const RISE: f64 = 7.0;
const TREAD: f64 = 11.0;

fn stairs(c: &mut Catalog) {
    for width in [36.0, 42.0, 48.0] {
        for r in 8..=20 {
            let r = f64::from(r);
            c.add(
                D,
                "Stairs and ramps",
                &format!("STAIR_STRAIGHT_{}_{}R", num(width), r),
                format!("Stair, straight {} wide, {r} risers", inches(width)),
                "flight steps up",
                stair_straight,
                args(&[width, r]),
                REAL,
            );
        }
    }
    for width in [36.0, 42.0] {
        for r in [12.0, 14.0, 16.0, 18.0, 20.0] {
            let l = format!("Stair, L-shaped {} wide, {r} risers", inches(width));
            c.add(D, "Stairs and ramps", &format!("STAIR_L_{}_{}R", num(width), r), l, "landing quarter turn", stair_l, args(&[width, r]), REAL);
            let u = format!("Stair, U-shaped {} wide, {r} risers", inches(width));
            c.add(
                D,
                "Stairs and ramps",
                &format!("STAIR_U_{}_{}R", num(width), r),
                u,
                "landing switchback half turn",
                stair_u,
                args(&[width, r]),
                REAL,
            );
        }
    }
    for d in [48.0, 60.0, 72.0, 84.0] {
        c.add(
            D,
            "Stairs and ramps",
            &format!("STAIR_SPIRAL_{}", num(d)),
            format!("Stair, spiral {} diameter", inches(d)),
            "helical",
            stair_spiral,
            args(&[d]),
            REAL,
        );
    }
    for rise in [6.0, 12.0, 18.0, 24.0, 30.0] {
        c.add(
            D,
            "Stairs and ramps",
            &format!("RAMP_1-12_{}", num(rise)),
            format!("Ramp, 1:12 slope rising {}", inches(rise)),
            "accessible slope",
            ramp,
            args(&[rise]),
            REAL,
        );
    }
}

/// An arrow along `pts` with its head at the last point.
fn arrow_path(k: &mut Kit, pts: &[(f64, f64)], head: f64) {
    k.path(pts);
    let n = pts.len();
    let (Some(&(x1, y1)), Some(&(x0, y0))) = (pts.get(n.wrapping_sub(1)), pts.get(n.wrapping_sub(2))) else { return };
    let (dx, dy) = (x1 - x0, y1 - y0);
    let l = (dx * dx + dy * dy).sqrt().max(1e-9);
    let (ux, uy) = (dx / l, dy / l);
    let (bx, by) = (x1 - ux * head, y1 - uy * head);
    k.fill(&[(x1, y1), (bx - uy * head * 0.35, by + ux * head * 0.35), (bx + uy * head * 0.35, by - ux * head * 0.35)]);
}

fn stair_straight(k: &mut Kit, a: &Args) {
    let (w, r) = (a.get(0), a.get(1));
    let treads = (r - 1.0).max(1.0);
    let run = treads * TREAD;
    k.rect((0.0, 0.0), (w, run));
    for i in 1..treads as usize {
        let y = i as f64 * TREAD;
        k.line((0.0, y), (w, y));
    }
    arrow_path(k, &[(w / 2.0, TREAD / 2.0), (w / 2.0, run - TREAD / 2.0)], 6.0);
    k.text((w / 2.0, -6.0), 4.0, "UP");
    for i in 0..treads as usize {
        let y = i as f64 * TREAD;
        k.cube((0.0, y), (w, y + TREAD), 0.0, (i as f64 + 1.0) * RISE);
    }
}

fn treads_between(k: &mut Kit, x0: f64, x1: f64, y0: f64, n: usize) {
    for i in 1..=n {
        let y = y0 + i as f64 * TREAD;
        k.line((x0, y), (x1, y));
    }
}

fn stair_l(k: &mut Kit, a: &Args) {
    let (w, r) = (a.get(0), a.get(1));
    let first = ((r / 2.0).floor() - 1.0).max(1.0);
    let second = (r - first - 2.0).max(1.0);
    let run1 = first * TREAD;
    // First flight up +Y, landing, second flight along +X.
    k.rect((0.0, 0.0), (w, run1));
    treads_between(k, 0.0, w, 0.0, (first as usize).saturating_sub(1));
    k.rect((0.0, run1), (w, run1 + w));
    let run2 = second * TREAD;
    k.rect((w, run1), (w + run2, run1 + w));
    for i in 1..second as usize {
        let x = w + i as f64 * TREAD;
        k.line((x, run1), (x, run1 + w));
    }
    arrow_path(k, &[(w / 2.0, TREAD / 2.0), (w / 2.0, run1 + w / 2.0), (w + run2 - TREAD / 2.0, run1 + w / 2.0)], 6.0);
    k.text((w / 2.0, -6.0), 4.0, "UP");
}

fn stair_u(k: &mut Kit, a: &Args) {
    let (w, r) = (a.get(0), a.get(1));
    let flight = ((r / 2.0).floor() - 1.0).max(1.0);
    let run = flight * TREAD;
    let gap = 4.0;
    let total = 2.0 * w + gap;
    k.rect((0.0, 0.0), (w, run));
    treads_between(k, 0.0, w, 0.0, (flight as usize).saturating_sub(1));
    k.rect((w + gap, 0.0), (total, run));
    treads_between(k, w + gap, total, 0.0, (flight as usize).saturating_sub(1));
    k.rect((0.0, run), (total, run + w));
    arrow_path(k, &[(w / 2.0, TREAD / 2.0), (w / 2.0, run + w / 2.0), (w + gap + w / 2.0, run + w / 2.0), (w + gap + w / 2.0, TREAD / 2.0)], 6.0);
    k.text((w / 2.0, -6.0), 4.0, "UP");
}

fn stair_spiral(k: &mut Kit, a: &Args) {
    let r = a.get(0) / 2.0;
    k.circle((0.0, 0.0), r);
    k.circle((0.0, 0.0), 2.5);
    let n = 12;
    for i in 0..n {
        let ang = (i as f64 * 30.0).to_radians();
        k.line((2.5 * ang.cos(), 2.5 * ang.sin()), (r * ang.cos(), r * ang.sin()));
    }
    k.arc((0.0, 0.0), r * 0.6, 0.0, 300.0);
    let e = 300f64.to_radians();
    let tip = (r * 0.6 * e.cos(), r * 0.6 * e.sin());
    let back = (r * 0.6 * (e - 0.15).cos(), r * 0.6 * (e - 0.15).sin());
    arrow_path(k, &[back, tip], 5.0);
    k.text((r * 0.6, -4.0), 3.0, "UP");
}

fn ramp(k: &mut Kit, a: &Args) {
    let rise = a.get(0);
    let len = rise * 12.0;
    let w = 48.0;
    k.rect((0.0, 0.0), (w, len));
    arrow_path(k, &[(w / 2.0, 6.0), (w / 2.0, len - 6.0)], 6.0);
    k.text((w / 2.0, -6.0), 4.0, "UP");
    k.text((w * 0.75, len / 2.0), 3.0, "1:12");
    k.face([
        cadaddy_geom::Vec3::new(0.0, 0.0, 0.0),
        cadaddy_geom::Vec3::new(w, 0.0, 0.0),
        cadaddy_geom::Vec3::new(w, len, rise),
        cadaddy_geom::Vec3::new(0.0, len, rise),
    ]);
}

// ---------------------------------------------------------------------------------------------
// Furniture

fn furniture(c: &mut Catalog) {
    const F: &str = "Furniture";
    for (word, label, w, l) in [
        ("TWIN", "twin", 39.0, 75.0),
        ("TWIN_XL", "twin XL", 39.0, 80.0),
        ("FULL", "full", 54.0, 75.0),
        ("QUEEN", "queen", 60.0, 80.0),
        ("KING", "king", 76.0, 80.0),
        ("CAL_KING", "California king", 72.0, 84.0),
        ("CRIB", "crib", 28.0, 52.0),
        ("BUNK", "bunk", 41.0, 77.0),
    ] {
        c.add(D, F, &format!("BED_{word}"), format!("Bed, {label} {}x{}", inches(w), inches(l)), "bedroom mattress", bed, args(&[w, l]), REAL);
    }
    for (w, d) in [(48.0, 30.0), (60.0, 30.0), (72.0, 30.0), (60.0, 36.0), (72.0, 36.0), (84.0, 42.0), (96.0, 42.0), (96.0, 48.0)] {
        c.add(
            D,
            F,
            &format!("TABLE_RECT_{}X{}", num(d), num(w)),
            format!("Table, rectangular {}x{}", inches(d), inches(w)),
            "dining work",
            table,
            args(&[w, d, 0.0]),
            REAL,
        );
    }
    for (w, d, n) in [(48.0, 36.0, 4.0), (72.0, 36.0, 6.0), (96.0, 42.0, 8.0), (120.0, 42.0, 10.0)] {
        c.add(
            D,
            F,
            &format!("TABLE_DINING_{}_SEAT", num(n)),
            format!("Table, dining with {n} chairs"),
            "chairs set seats",
            table,
            args(&[w, d, n]),
            REAL,
        );
    }
    for dia in [30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0] {
        c.add(
            D,
            F,
            &format!("TABLE_ROUND_{}", num(dia)),
            format!("Table, round {} diameter", inches(dia)),
            "dining cafe",
            table_round,
            args(&[dia]),
            REAL,
        );
    }
    for s in [30.0, 36.0, 42.0, 48.0] {
        c.add(D, F, &format!("TABLE_SQUARE_{}", num(s)), format!("Table, square {}", inches(s)), "dining cafe", table, args(&[s, s, 0.0]), REAL);
    }
    for (word, label, w, d, style) in [
        ("DINING", "dining", 18.0, 20.0, 0.0),
        ("ARM", "armchair", 26.0, 28.0, 1.0),
        ("TASK", "office task", 26.0, 26.0, 2.0),
        ("LOUNGE", "lounge", 32.0, 34.0, 1.0),
        ("STOOL", "bar stool", 16.0, 16.0, 3.0),
        ("STACKING", "stacking", 20.0, 21.0, 0.0),
    ] {
        c.add(D, F, &format!("CHAIR_{word}"), format!("Chair, {label}"), "seat seating", chair, args(&[w, d, style]), REAL);
    }
    for l in [60.0, 72.0, 84.0, 96.0] {
        c.add(D, F, &format!("SOFA_{}", num(l)), format!("Sofa, {} long", inches(l)), "couch seating living", sofa, args(&[l, 36.0]), REAL);
    }
    c.add(D, F, "LOVESEAT_58", "Loveseat, 58\" long".into(), "sofa couch seating", sofa, args(&[58.0, 34.0]), REAL);
    for (l1, l2) in [(96.0, 84.0), (108.0, 96.0)] {
        c.add(
            D,
            F,
            &format!("SOFA_SECTIONAL_{}X{}", num(l1), num(l2)),
            format!("Sofa, L-sectional {}x{}", inches(l1), inches(l2)),
            "couch corner",
            sectional,
            args(&[l1, l2]),
            REAL,
        );
    }
    for (w, d) in [(48.0, 24.0), (60.0, 30.0), (72.0, 30.0), (72.0, 36.0)] {
        c.add(D, F, &format!("DESK_{}X{}", num(d), num(w)), format!("Desk, {}x{}", inches(d), inches(w)), "office work", desk, args(&[w, d]), REAL);
    }
    for (w, d) in [(60.0, 72.0), (72.0, 72.0)] {
        c.add(
            D,
            F,
            &format!("DESK_L_{}X{}", num(w), num(d)),
            format!("Desk, L-shaped {}x{}", inches(w), inches(d)),
            "office return corner",
            desk_l,
            args(&[w, d]),
            REAL,
        );
    }
    for (w, d) in [(72.0, 72.0), (72.0, 96.0), (96.0, 96.0)] {
        c.add(
            D,
            F,
            &format!("WORKSTATION_{}X{}", num(w / 12.0), num(d / 12.0)),
            format!("Workstation, {}' x {}' cubicle", w / 12.0, d / 12.0),
            "office cubicle panel",
            workstation,
            args(&[w, d]),
            REAL,
        );
    }
    for (word, label, w, d) in [
        ("VERT_2", "vertical 2-drawer", 15.0, 28.0),
        ("VERT_4", "vertical 4-drawer", 15.25, 28.5),
        ("LAT_30", "lateral 30\"", 30.0, 18.0),
        ("LAT_36", "lateral 36\"", 36.0, 18.25),
        ("LAT_42", "lateral 42\"", 42.0, 18.5),
    ] {
        c.add(D, F, &format!("FILE_{word}"), format!("File cabinet, {label}"), "storage office", casework, args(&[w, d, 52.0, 1.0]), REAL);
    }
    for w in [24.0, 30.0, 36.0, 48.0] {
        c.add(
            D,
            F,
            &format!("BOOKCASE_{}", num(w)),
            format!("Bookcase, {} wide", inches(w)),
            "shelves shelving storage",
            casework,
            args(&[w, 12.0, 72.0, 2.0]),
            REAL,
        );
    }
    for w in [36.0, 48.0, 60.0, 72.0] {
        c.add(
            D,
            F,
            &format!("DRESSER_{}", num(w)),
            format!("Dresser, {} wide", inches(w)),
            "bedroom chest drawers",
            casework,
            args(&[w, 20.0, 34.0, 1.0]),
            REAL,
        );
    }
    c.add(D, F, "NIGHTSTAND_24", "Nightstand, 24\" wide".into(), "bedroom bedside", casework, args(&[24.0, 18.0, 26.0, 1.0]), REAL);
    for w in [36.0, 48.0] {
        c.add(
            D,
            F,
            &format!("WARDROBE_{}", num(w)),
            format!("Wardrobe, {} wide", inches(w)),
            "armoire closet",
            casework,
            args(&[w, 24.0, 78.0, 3.0]),
            REAL,
        );
    }
    for w in [60.0, 72.0] {
        c.add(
            D,
            F,
            &format!("MEDIA_CONSOLE_{}", num(w)),
            format!("Media console, {} wide", inches(w)),
            "tv credenza",
            casework,
            args(&[w, 18.0, 24.0, 1.0]),
            REAL,
        );
    }
    for l in [96.0, 120.0, 144.0, 168.0, 192.0, 240.0] {
        let d = if l <= 120.0 { 42.0 } else { 54.0 };
        c.add(
            D,
            F,
            &format!("TABLE_CONFERENCE_{}", num(l / 12.0)),
            format!("Table, conference {}' racetrack", l / 12.0),
            "meeting boardroom",
            conference,
            args(&[l, d]),
            REAL,
        );
    }
}

/// Four legs and a top: a table's body.
fn table_body(k: &mut Kit, x0: f64, y0: f64, x1: f64, y1: f64) {
    k.cube((x0, y0), (x1, y1), 28.5, 30.0);
    let l = 2.0;
    for (x, y) in [(x0 + 1.0, y0 + 1.0), (x1 - 1.0 - l, y0 + 1.0), (x1 - 1.0 - l, y1 - 1.0 - l), (x0 + 1.0, y1 - 1.0 - l)] {
        k.cube((x, y), (x + l, y + l), 0.0, 28.5);
    }
}

fn bed(k: &mut Kit, a: &Args) {
    let (w, l) = (a.get(0), a.get(1));
    let h = w / 2.0;
    k.rect((-h, 0.0), (h, -l));
    k.rect((-h, 0.0), (h, -2.0));
    // Pillows, and the turned-down sheet.
    if w >= 50.0 {
        k.round_rect((-h + 3.0, -4.0), (-1.5, -16.0), 3.0);
        k.round_rect((1.5, -4.0), (h - 3.0, -16.0), 3.0);
    } else {
        k.round_rect((-h + 4.0, -4.0), (h - 4.0, -16.0), 3.0);
    }
    k.path(&[(-h, -22.0), (h, -22.0)]);
    k.line((-h, -22.0), (-h + 10.0, -30.0));
    k.cube((-h, 0.0), (h, -l), 0.0, if w < 30.0 { 32.0 } else { 22.0 });
    k.cube((-h, 0.0), (h, -2.0), 0.0, 44.0);
}

fn table(k: &mut Kit, a: &Args) {
    let (w, d, chairs) = (a.get(0), a.get(1), a.get(2) as i64);
    let (hx, hy) = (w / 2.0, d / 2.0);
    k.rect((-hx, -hy), (hx, hy));
    if chairs > 0 {
        let per_side = (chairs / 2).max(1);
        let pitch = w / per_side as f64;
        for i in 0..per_side {
            let x = -hx + pitch * (i as f64 + 0.5);
            k.rect((x - 9.0, hy + 2.0), (x + 9.0, hy + 20.0));
            k.rect((x - 9.0, -hy - 2.0), (x + 9.0, -hy - 20.0));
        }
    }
    table_body(k, -hx, -hy, hx, hy);
}

fn table_round(k: &mut Kit, a: &Args) {
    let r = a.get(0) / 2.0;
    k.circle((0.0, 0.0), r);
    k.cylinder((0.0, 0.0), r, 28.5, 30.0);
    k.cylinder((0.0, 0.0), 2.0, 0.0, 28.5);
}

fn chair(k: &mut Kit, a: &Args) {
    let (w, d, style) = (a.get(0), a.get(1), a.get(2) as i64);
    let (hx, hy) = (w / 2.0, d / 2.0);
    match style {
        2 => {
            k.circle((0.0, 0.0), hx);
            k.round_rect((-hx + 3.0, -hy + 3.0), (hx - 3.0, hy - 6.0), 4.0);
            k.round_rect((-hx + 4.0, hy - 6.0), (hx - 4.0, hy - 2.0), 2.0);
            k.cylinder((0.0, 0.0), hx - 3.0, 17.0, 19.0);
        }
        3 => {
            k.circle((0.0, 0.0), hx);
            k.circle((0.0, 0.0), hx - 2.0);
            k.cylinder((0.0, 0.0), hx, 28.0, 30.0);
        }
        1 => {
            k.round_rect((-hx, -hy), (hx, hy), 3.0);
            k.rect((-hx + 5.0, -hy + 2.0), (hx - 5.0, hy - 7.0));
            k.cube((-hx, -hy), (hx, hy), 0.0, 17.0);
            k.cube((-hx, hy - 7.0), (hx, hy), 17.0, 32.0);
        }
        _ => {
            k.rect((-hx, -hy), (hx, hy - 3.0));
            k.round_rect((-hx, hy - 3.0), (hx, hy), 1.0);
            k.cube((-hx, -hy), (hx, hy - 3.0), 16.0, 18.0);
            k.cube((-hx, hy - 3.0), (hx, hy), 0.0, 34.0);
        }
    }
}

fn sofa(k: &mut Kit, a: &Args) {
    let (l, d) = (a.get(0), a.get(1));
    let h = l / 2.0;
    k.round_rect((-h, 0.0), (h, -d), 2.0);
    k.line((-h + 6.0, -8.0), (h - 6.0, -8.0));
    k.line((-h + 6.0, 0.0), (-h + 6.0, -d));
    k.line((h - 6.0, 0.0), (h - 6.0, -d));
    let seats = ((l - 12.0) / 30.0).round().max(1.0);
    let s = (l - 12.0) / seats;
    for i in 1..seats as usize {
        let x = -h + 6.0 + s * i as f64;
        k.line((x, -8.0), (x, -d));
    }
    k.stretch("Length", (-h, -d - 4.0), (h, -d - 4.0), (0.0, 4.0), (h + 4.0, -d - 8.0), 48.0, 144.0, 6.0);
}

fn sectional(k: &mut Kit, a: &Args) {
    let (l1, l2) = (a.get(0), a.get(1));
    let d = 36.0;
    k.poly(&[(0.0, 0.0), (l1, 0.0), (l1, -d), (d, -d), (d, -l2), (0.0, -l2)]);
    k.path(&[(l1 - 6.0, 0.0), (l1 - 6.0, -d)]);
    k.path(&[(0.0, -l2 + 6.0), (d, -l2 + 6.0)]);
    k.path(&[(8.0, -l2 + 6.0), (8.0, -8.0), (l1 - 6.0, -8.0)]);
    k.line((8.0, -8.0), (d, -d));
}

fn desk(k: &mut Kit, a: &Args) {
    let (w, d) = (a.get(0), a.get(1));
    let h = w / 2.0;
    k.rect((-h, 0.0), (h, -d));
    k.rect((h - 16.0, -1.0), (h - 1.0, -d + 2.0));
    k.cube((-h, 0.0), (h, -d), 28.5, 30.0);
    k.cube((h - 16.0, -1.0), (h - 1.0, -d + 2.0), 0.0, 28.5);
    k.cube((-h, 0.0), (-h + 1.0, -d), 0.0, 28.5);
}

fn desk_l(k: &mut Kit, a: &Args) {
    let (w, d) = (a.get(0), a.get(1));
    let t = 30.0;
    k.poly(&[(0.0, 0.0), (w, 0.0), (w, -t), (t, -t), (t, -d), (0.0, -d)]);
    k.line((t, -t), (0.0, 0.0));
    k.cube((0.0, 0.0), (w, -t), 28.5, 30.0);
    k.cube((0.0, -t), (t, -d), 28.5, 30.0);
}

fn workstation(k: &mut Kit, a: &Args) {
    let (w, d) = (a.get(0), a.get(1));
    // Panels on three sides (2" thick), an L work surface.
    k.path(&[(0.0, -d), (0.0, 0.0), (w, 0.0), (w, -d)]);
    k.path(&[(2.0, -d), (2.0, -2.0), (w - 2.0, -2.0), (w - 2.0, -d)]);
    k.poly(&[(2.0, -2.0), (w - 2.0, -2.0), (w - 2.0, -26.0), (26.0, -26.0), (26.0, -d + 12.0), (2.0, -d + 12.0)]);
    k.circle((w / 2.0, -d / 2.0 - 6.0), 12.0);
    k.cube((0.0, 0.0), (w, -2.0), 0.0, 54.0);
    k.cube((0.0, -2.0), (2.0, -d), 0.0, 54.0);
    k.cube((w - 2.0, -2.0), (w, -d), 0.0, 54.0);
}

/// Storage furniture against the wall: `style` 1 drawers, 2 open shelves, 3 doors.
fn casework(k: &mut Kit, a: &Args) {
    let (w, d, ht, style) = (a.get(0), a.get(1), a.get(2), a.get(3) as i64);
    let h = w / 2.0;
    k.rect((-h, 0.0), (h, -d));
    match style {
        2 => k.line((-h + 1.0, -1.0), (h - 1.0, -1.0)),
        3 => {
            k.line((0.0, -d), (0.0, -d + 2.0));
            k.arc((-h, -d), h, 270.0, 360.0);
            k.arc((h, -d), h, 180.0, 270.0);
        }
        _ => k.line((-h, -d + 1.5), (h, -d + 1.5)),
    }
    k.cube((-h, 0.0), (h, -d), 0.0, ht);
}

fn conference(k: &mut Kit, a: &Args) {
    let (l, d) = (a.get(0), a.get(1));
    let (h, r) = (l / 2.0, d / 2.0);
    k.pline(&[(-h + r, -r, 0.0), (h - r, -r, 1.0), (h - r, r, 0.0), (-h + r, r, 1.0)], true);
    // Chairs along the straight sides only, so stretching the far end leaves them whole.
    let n = ((l - d) / 24.0).floor().max(1.0) as usize;
    let pitch = (l - d) / n as f64;
    for i in 0..n {
        let x = -h + r + pitch * (i as f64 + 0.5);
        k.rect((x - 10.0, r + 4.0), (x + 10.0, r + 24.0));
        k.rect((x - 10.0, -r - 4.0), (x + 10.0, -r - 24.0));
    }
    k.stretch("Length", (-h, -r - 30.0), (h, -r - 30.0), (h - r - 1.0, r + 30.0), (h + 6.0, -r - 30.0), 72.0, 360.0, 12.0);
}

// ---------------------------------------------------------------------------------------------
// Sanitary

fn sanitary(c: &mut Catalog) {
    const S: &str = "Sanitary fixtures";
    for (word, label, style) in [("TANK", "tank type", 0.0), ("WALL", "wall hung", 1.0), ("ADA", "accessible, elongated tank", 2.0)] {
        c.add(D, S, &format!("WC_{word}"), format!("Water closet, {label}"), "toilet wc", water_closet, args(&[style]), REAL);
    }
    c.add(D, S, "URINAL_WALL", "Urinal, wall hung".into(), "men", urinal, args(&[0.0]), REAL);
    c.add(D, S, "URINAL_STALL", "Urinal, stall type".into(), "men floor", urinal, args(&[1.0]), REAL);
    c.add(D, S, "BIDET", "Bidet".into(), "", bidet, args(&[]), REAL);
    for (w, d) in [(18.0, 15.0), (20.0, 18.0), (22.0, 19.0)] {
        c.add(
            D,
            S,
            &format!("LAV_WALL_{}X{}", num(w), num(d)),
            format!("Lavatory, wall hung {}x{}", inches(w), inches(d)),
            "sink basin hand",
            lav_wall,
            args(&[w, d]),
            REAL,
        );
    }
    for (w, n) in [(24.0, 1.0), (30.0, 1.0), (36.0, 1.0), (48.0, 1.0), (60.0, 2.0), (72.0, 2.0)] {
        let l = if n > 1.0 { format!("Vanity, {} with two lavatories", inches(w)) } else { format!("Vanity, {} with lavatory", inches(w)) };
        c.add(D, S, &format!("VANITY_{}", num(w)), l, "lavatory counter sink basin", vanity, args(&[w, n]), REAL);
    }
    for (w, d) in [(60.0, 30.0), (60.0, 32.0), (66.0, 32.0), (72.0, 36.0)] {
        for (hand, h) in [("LH", 0.0), ("RH", 1.0)] {
            let label = format!("Bathtub, {}x{} {} drain", inches(w), inches(d), if h == 0.0 { "left" } else { "right" });
            c.add(D, S, &format!("TUB_{}X{}_{hand}", num(w), num(d)), label, "bath", tub, args(&[w, d, h]), REAL);
        }
    }
    c.add(D, S, "TUB_CORNER_60", "Bathtub, corner 60\"".into(), "bath whirlpool", tub_corner, args(&[60.0]), REAL);
    for (w, d) in [(32.0, 32.0), (36.0, 36.0), (42.0, 42.0), (48.0, 36.0), (60.0, 32.0), (60.0, 36.0)] {
        c.add(
            D,
            S,
            &format!("SHOWER_{}X{}", num(w), num(d)),
            format!("Shower, {}x{}", inches(w), inches(d)),
            "stall receptor roll-in",
            shower,
            args(&[w, d]),
            REAL,
        );
    }
    for l in [18.0, 24.0, 36.0, 42.0] {
        c.add(D, S, &format!("GRAB_BAR_{}", num(l)), format!("Grab bar, {}", inches(l)), "accessible ada rail", grab_bar, args(&[l]), REAL);
    }
    for (w, d) in [(24.0, 24.0), (36.0, 24.0)] {
        c.add(
            D,
            S,
            &format!("MOP_SINK_{}X{}", num(w), num(d)),
            format!("Mop sink, {}x{}", inches(w), inches(d)),
            "service basin janitor",
            mop_sink,
            args(&[w, d]),
            REAL,
        );
    }
    c.add(D, S, "DRINKING_FOUNTAIN", "Drinking fountain, single".into(), "water cooler", fountain, args(&[1.0]), REAL);
    c.add(D, S, "DRINKING_FOUNTAIN_HI_LO", "Drinking fountain, hi-lo pair".into(), "water cooler accessible", fountain, args(&[2.0]), REAL);
    for (w, label) in [(36.0, "standard"), (60.0, "accessible")] {
        c.add(
            D,
            S,
            &format!("TOILET_PARTITION_{}", num(w)),
            format!("Toilet partition, {label} {} stall", inches(w)),
            "compartment stall",
            partition,
            args(&[w]),
            REAL,
        );
    }
}

fn water_closet(k: &mut Kit, a: &Args) {
    let style = a.get(0) as i64;
    let bowl = if style == 0 { 14.0 } else { 15.5 };
    if style != 1 {
        k.round_rect((-10.0, 0.0), (10.0, -8.5), 1.5);
    } else {
        k.rect((-8.0, 0.0), (8.0, -3.0));
    }
    let y0 = if style == 1 { -3.0 } else { -8.5 };
    k.ellipse((0.0, y0 - bowl / 2.0), 7.5, bowl / 2.0);
    k.ellipse((0.0, y0 - bowl / 2.0 - 0.5), 5.5, bowl / 2.0 - 2.0);
    k.cube((-10.0, 0.0), (10.0, y0), if style == 1 { 12.0 } else { 15.0 }, if style == 1 { 24.0 } else { 30.0 });
    k.cube((-7.5, y0), (7.5, y0 - bowl), if style == 1 { 12.0 } else { 0.0 }, if style == 2 { 17.5 } else { 15.0 });
}

fn urinal(k: &mut Kit, a: &Args) {
    if a.get(0) > 0.5 {
        k.pline(&[(-9.0, 0.0, 0.0), (-9.0, -10.0, 1.0), (9.0, -10.0, 0.0), (9.0, 0.0, 0.0)], false);
        k.line((-9.0, 0.0), (9.0, 0.0));
        k.circle((0.0, -8.0), 1.0);
        k.cube((-9.0, 0.0), (9.0, -14.0), 0.0, 42.0);
    } else {
        k.pline(&[(-9.0, 0.0, 0.0), (-9.0, -9.0, 1.0), (9.0, -9.0, 0.0), (9.0, 0.0, 0.0)], false);
        k.line((-9.0, 0.0), (9.0, 0.0));
        k.pline(&[(-6.0, -1.0, 0.0), (-6.0, -7.0, 1.0), (6.0, -7.0, 0.0), (6.0, -1.0, 0.0)], false);
        k.cube((-9.0, 0.0), (9.0, -13.0), 17.0, 39.0);
    }
}

fn bidet(k: &mut Kit, _: &Args) {
    k.ellipse((0.0, -11.0), 7.0, 11.0);
    k.ellipse((0.0, -12.0), 4.5, 7.0);
    k.circle((0.0, -3.0), 0.75);
    k.cube((-7.0, 0.0), (7.0, -22.0), 0.0, 15.0);
}

fn lav_wall(k: &mut Kit, a: &Args) {
    let (w, d) = (a.get(0), a.get(1));
    let h = w / 2.0;
    k.round_rect((-h, 0.0), (h, -d), 2.0);
    k.ellipse((0.0, -d / 2.0 - 1.0), h - 3.0, d / 2.0 - 3.0);
    k.circle((0.0, -2.5), 0.75);
    k.cube((-h, 0.0), (h, -d), 28.0, 34.0);
}

fn vanity(k: &mut Kit, a: &Args) {
    let (w, n) = (a.get(0), a.get(1) as usize);
    let (h, d) = (w / 2.0, 22.0);
    k.rect((-h, 0.0), (h, -d));
    let pitch = w / n as f64;
    for i in 0..n {
        let x = -h + pitch * (i as f64 + 0.5);
        k.ellipse((x, -11.0), 8.5, 6.5);
        k.circle((x, -2.5), 0.75);
    }
    k.cube((-h, 0.0), (h, -d), 0.0, 34.0);
}

fn tub(k: &mut Kit, a: &Args) {
    let (w, d, rh) = (a.get(0), a.get(1), a.get(2) > 0.5);
    k.rect((0.0, 0.0), (w, -d));
    k.round_rect((3.0, -3.0), (w - 3.0, -d + 3.0), 6.0);
    let x = if rh { w - 8.0 } else { 8.0 };
    k.circle((x, -d / 2.0), 1.0);
    k.cube((0.0, 0.0), (w, -d), 0.0, 20.0);
}

fn tub_corner(k: &mut Kit, a: &Args) {
    let s = a.get(0);
    k.poly(&[(0.0, 0.0), (s, 0.0), (s, -18.0), (18.0, -s), (0.0, -s)]);
    k.circle((s * 0.42, -s * 0.42), s * 0.3);
    k.circle((s * 0.42, -s * 0.42), 1.0);
}

fn shower(k: &mut Kit, a: &Args) {
    let (w, d) = (a.get(0), a.get(1));
    k.rect((0.0, 0.0), (w, -d));
    k.rect((1.5, -1.5), (w - 1.5, -d + 1.5));
    k.line((1.5, -1.5), (w - 1.5, -d + 1.5));
    k.line((w - 1.5, -1.5), (1.5, -d + 1.5));
    k.circle((w / 2.0, -d / 2.0), 1.5);
    k.cube((0.0, 0.0), (w, -d), 0.0, 4.0);
}

fn grab_bar(k: &mut Kit, a: &Args) {
    let l = a.get(0);
    k.rect((0.0, 0.0), (l, -3.0));
    k.line((1.5, -3.0), (l - 1.5, -3.0));
    k.circle((1.5, -1.5), 1.0);
    k.circle((l - 1.5, -1.5), 1.0);
}

fn mop_sink(k: &mut Kit, a: &Args) {
    let (w, d) = (a.get(0), a.get(1));
    k.rect((-w / 2.0, 0.0), (w / 2.0, -d));
    k.rect((-w / 2.0 + 2.0, -2.0), (w / 2.0 - 2.0, -d + 2.0));
    k.circle((0.0, -d / 2.0), 1.5);
    k.cube((-w / 2.0, 0.0), (w / 2.0, -d), 0.0, 10.0);
}

fn fountain(k: &mut Kit, a: &Args) {
    let n = a.get(0) as usize;
    for i in 0..n {
        let x = i as f64 * 19.0;
        k.round_rect((x, 0.0), (x + 18.0, -19.0), 3.0);
        k.ellipse((x + 9.0, -9.0), 6.0, 5.0);
        k.cube((x, 0.0), (x + 18.0, -19.0), if i == 0 { 27.0 } else { 34.0 }, if i == 0 { 36.0 } else { 43.0 });
    }
}

fn partition(k: &mut Kit, a: &Args) {
    let w = a.get(0);
    let d = if w > 40.0 { 60.0 } else { 60.0 - 6.0 };
    k.rect((0.0, 0.0), (1.0, -d));
    k.rect((w - 1.0, 0.0), (w, -d));
    let door = if w > 40.0 { 36.0 } else { 24.0 };
    k.rect((1.0, -d), (w - door - 1.0, -d - 1.0));
    k.arc((w - 1.0, -d), door, 180.0, 270.0);
    k.line((w - 1.0, -d), (w - 1.0, -d - door));
}

// ---------------------------------------------------------------------------------------------
// Kitchen

fn kitchen(c: &mut Catalog) {
    const K: &str = "Kitchen";
    for w in (3..=16).map(|i| f64::from(i) * 3.0) {
        c.add(
            D,
            K,
            &format!("CAB_BASE_{}", num(w)),
            format!("Cabinet, base {} wide", inches(w)),
            "kitchen casework lower",
            cabinet,
            args(&[w, 24.0, 0.0]),
            REAL,
        );
        c.add(
            D,
            K,
            &format!("CAB_WALL_{}", num(w)),
            format!("Cabinet, wall {} wide", inches(w)),
            "kitchen casework upper",
            cabinet,
            args(&[w, 12.0, 1.0]),
            REAL,
        );
    }
    for w in [18.0, 24.0, 30.0, 36.0] {
        c.add(
            D,
            K,
            &format!("CAB_TALL_{}", num(w)),
            format!("Cabinet, tall pantry {} wide", inches(w)),
            "kitchen casework utility",
            cabinet,
            args(&[w, 24.0, 2.0]),
            REAL,
        );
    }
    c.add(D, K, "CAB_CORNER_BLIND_36", "Cabinet, blind corner base 36\"".into(), "kitchen casework", cab_corner, args(&[0.0]), REAL);
    c.add(D, K, "CAB_CORNER_LAZY_36", "Cabinet, lazy susan corner base 36\"".into(), "kitchen casework carousel", cab_corner, args(&[1.0]), REAL);
    for (word, label, w, bowls) in [
        ("SGL_25", "single bowl 25\"", 25.0, 1.0),
        ("SGL_30", "single bowl 30\"", 30.0, 1.0),
        ("DBL_33", "double bowl 33\"", 33.0, 2.0),
        ("DBL_36", "double bowl 36\"", 36.0, 2.0),
        ("TRIPLE_42", "triple bowl 42\"", 42.0, 3.0),
        ("FARM_30", "farmhouse apron 30\"", 30.0, -1.0),
        ("FARM_36", "farmhouse apron 36\"", 36.0, -1.0),
        ("BAR_15", "bar 15\"", 15.0, 1.0),
    ] {
        c.add(D, K, &format!("SINK_{word}"), format!("Kitchen sink, {label}"), "basin", kitchen_sink, args(&[w, bowls]), REAL);
    }
    for w in [24.0, 30.0, 36.0, 48.0] {
        c.add(D, K, &format!("RANGE_{}", num(w)), format!("Range, {}", inches(w)), "stove oven cooking appliance", range, args(&[w, 0.0]), REAL);
    }
    for w in [30.0, 36.0] {
        c.add(D, K, &format!("COOKTOP_{}", num(w)), format!("Cooktop, {}", inches(w)), "hob cooking appliance", range, args(&[w, 1.0]), REAL);
        c.add(D, K, &format!("HOOD_{}", num(w)), format!("Range hood, {}", inches(w)), "exhaust vent appliance", hood, args(&[w]), REAL);
    }
    for w in [24.0, 30.0, 33.0, 36.0, 42.0, 48.0] {
        c.add(
            D,
            K,
            &format!("REFRIGERATOR_{}", num(w)),
            format!("Refrigerator, {}", inches(w)),
            "fridge appliance",
            appliance,
            args(&[w, 30.0, 70.0, 0.0]),
            REAL,
        );
    }
    for w in [18.0, 24.0] {
        c.add(
            D,
            K,
            &format!("DISHWASHER_{}", num(w)),
            format!("Dishwasher, {}", inches(w)),
            "appliance",
            appliance,
            args(&[w, 24.0, 34.0, 1.0]),
            REAL,
        );
    }
    for w in [27.0, 30.0] {
        c.add(D, K, &format!("WALL_OVEN_{}", num(w)), format!("Wall oven, {}", inches(w)), "appliance", appliance, args(&[w, 24.0, 84.0, 2.0]), REAL);
    }
    c.add(D, K, "WASHER_27", "Clothes washer, 27\"".into(), "laundry appliance", appliance, args(&[27.0, 30.0, 38.0, 3.0]), REAL);
    c.add(D, K, "DRYER_27", "Clothes dryer, 27\"".into(), "laundry appliance", appliance, args(&[27.0, 30.0, 38.0, 4.0]), REAL);
    for (w, d) in [(72.0, 36.0), (84.0, 42.0), (96.0, 48.0)] {
        c.add(
            D,
            K,
            &format!("ISLAND_{}X{}", num(d), num(w)),
            format!("Kitchen island, {}x{}", inches(d), inches(w)),
            "counter",
            island,
            args(&[w, d]),
            REAL,
        );
    }
}

/// Base (0), wall (1, drawn hidden: it is above the cut) or tall (2) cabinet, back on Y 0 from X 0.
fn cabinet(k: &mut Kit, a: &Args) {
    let (w, d, kind) = (a.get(0), a.get(1), a.get(2) as i64);
    k.hidden(kind == 1);
    k.rect((0.0, 0.0), (w, -d));
    match kind {
        1 => k.line((0.0, 0.0), (w, -d)),
        2 => {
            k.line((0.0, 0.0), (w, -d));
            k.line((w, 0.0), (0.0, -d));
        }
        _ => k.line((0.0, -d + 1.0), (w, -d + 1.0)),
    }
    k.hidden(false);
    k.stretch("Width", (0.0, -d), (w, -d), (w * 0.6, 2.0), (w + 2.0, -d - 2.0), 6.0, 60.0, 3.0);
    k.attdef_hidden("TAG", "Cabinet code", "", (w / 2.0, -d / 2.0), 2.0);
}

fn cab_corner(k: &mut Kit, a: &Args) {
    k.poly(&[(0.0, 0.0), (36.0, 0.0), (36.0, -24.0), (24.0, -24.0), (24.0, -36.0), (0.0, -36.0)]);
    if a.get(0) > 0.5 {
        k.arc((0.0, 0.0), 30.0, 270.0, 360.0);
    } else {
        k.line((24.0, -24.0), (0.0, -24.0));
    }
    k.cube((0.0, 0.0), (36.0, -24.0), 0.0, 34.5);
    k.cube((0.0, -24.0), (24.0, -36.0), 0.0, 34.5);
}

fn kitchen_sink(k: &mut Kit, a: &Args) {
    let (w, bowls) = (a.get(0), a.get(1) as i64);
    let (h, d) = (w / 2.0, 22.0);
    k.rect((-h, -1.0), (h, -1.0 - d));
    if bowls < 0 {
        k.round_rect((-h + 1.5, -3.0), (h - 1.5, -1.0 - d + 0.5), 2.0);
        k.circle((0.0, -d / 2.0 - 2.0), 1.75);
    } else {
        let n = bowls.max(1) as usize;
        let inner = (w - 3.0 - (n as f64 - 1.0) * 2.0) / n as f64;
        for i in 0..n {
            let x0 = -h + 1.5 + i as f64 * (inner + 2.0);
            k.round_rect((x0, -4.0), (x0 + inner, -1.0 - d + 2.0), 2.0);
            k.circle((x0 + inner / 2.0, -d / 2.0 - 2.0), 1.75);
        }
    }
    k.circle((0.0, -2.5), 0.75);
}

fn range(k: &mut Kit, a: &Args) {
    let (w, cooktop) = (a.get(0), a.get(1) > 0.5);
    let (h, d) = (w / 2.0, if cooktop { 21.0 } else { 26.0 });
    k.rect((-h, 0.0), (h, -d));
    let burners: &[(f64, f64)] = if w >= 36.0 {
        &[(-0.33, 0.3), (0.0, 0.3), (0.33, 0.3), (-0.33, 0.7), (0.0, 0.7), (0.33, 0.7)]
    } else {
        &[(-0.25, 0.3), (0.25, 0.3), (-0.25, 0.7), (0.25, 0.7)]
    };
    for (fx, fy) in burners {
        k.circle((fx * w, -fy * (d - 4.0) - 1.0), (w / 10.0).min(5.0));
    }
    if !cooktop {
        k.line((-h, -d + 3.0), (h, -d + 3.0));
        k.cube((-h, 0.0), (h, -d), 0.0, 36.0);
    }
}

fn hood(k: &mut Kit, a: &Args) {
    let w = a.get(0);
    k.hidden(true);
    k.rect((-w / 2.0, 0.0), (w / 2.0, -20.0));
    k.line((-w / 2.0, 0.0), (0.0, -10.0));
    k.line((w / 2.0, 0.0), (0.0, -10.0));
    k.hidden(false);
}

/// Appliances: refrigerator (0), dishwasher (1), wall oven (2), washer (3), dryer (4).
fn appliance(k: &mut Kit, a: &Args) {
    let (w, d, ht, kind) = (a.get(0), a.get(1), a.get(2), a.get(3) as i64);
    let h = w / 2.0;
    k.rect((-h, 0.0), (h, -d));
    match kind {
        0 => {
            k.line((-h, -d + 2.0), (h, -d + 2.0));
            if w >= 33.0 {
                k.line((0.0, -d + 2.0), (0.0, -d));
            }
            k.text((0.0, -d / 2.0), 4.0, "REF");
        }
        1 => k.text((0.0, -d / 2.0), 4.0, "DW"),
        2 => k.text((0.0, -d / 2.0), 4.0, "OVEN"),
        3 => {
            k.circle((0.0, -d / 2.0), w * 0.3);
            k.text((0.0, -d / 2.0), 3.0, "W");
        }
        _ => {
            k.circle((0.0, -d / 2.0), w * 0.3);
            k.text((0.0, -d / 2.0), 3.0, "D");
        }
    }
    k.cube((-h, 0.0), (h, -d), 0.0, ht);
}

fn island(k: &mut Kit, a: &Args) {
    let (w, d) = (a.get(0), a.get(1));
    let (h, e) = (w / 2.0, d / 2.0);
    k.rect((-h, -e), (h, e));
    k.rect((-h + 1.5, -e + 12.0), (h - 1.5, e - 1.5));
    k.cube((-h, -e), (h, e), 34.5, 36.0);
    k.cube((-h + 1.5, -e + 12.0), (h - 1.5, e - 1.5), 0.0, 34.5);
}
