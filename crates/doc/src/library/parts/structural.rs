//! Structural parts: steel sections by designation (W shapes, channels, angles, HSS, pipe, and the
//! European IPE, HEA, HEB and UPN shapes and metric angles), reinforcing bars (sections and
//! standard hooks), sawn lumber, base plates and masonry units.
//!
//! Section dimensions are the published nominal values for each designation (depth, flange width
//! and the web and flange thicknesses; HSS walls at 0.93 of nominal as design thickness; pipe
//! walls by schedule; bar diameters by bar size; lumber at its dressed size). They are facts
//! from public tables, typed here; check the current steel manual or standard before design.
//! Sections are drawn as their cross-section centred on the origin (channels and angles from the
//! back of the web and the heel).

use std::f64::consts::PI;

use super::{Args, Catalog, Discipline, Kit, METRIC, REAL, args, args_s, inches, num};

const D: Discipline = Discipline::Structural;

pub(crate) fn add(c: &mut Catalog) {
    w_shapes(c);
    channels(c);
    angles(c);
    hss(c);
    pipe(c);
    metric_shapes(c);
    rebar(c);
    lumber(c);
    plates_and_masonry(c);
}

/// A fraction of an inch without the inch mark: `1/2`, `1-1/4`.
fn frac(v: f64) -> String {
    inches(v).trim_end_matches('"').to_string()
}

// ---------------------------------------------------------------------------------------------
// W shapes: (designation, d, bf, tw, tf) in inches.

// Table values that happen to be near π-based constants (6.28, 0.318) are dimensions.
#[allow(clippy::approx_constant)]
const W: &[(&str, f64, f64, f64, f64)] = &[
    ("W4X13", 4.16, 4.06, 0.280, 0.345),
    ("W5X16", 5.01, 5.00, 0.240, 0.360),
    ("W5X19", 5.15, 5.03, 0.270, 0.430),
    ("W6X9", 5.90, 3.94, 0.170, 0.215),
    ("W6X12", 6.03, 4.00, 0.230, 0.280),
    ("W6X15", 5.99, 5.99, 0.230, 0.260),
    ("W6X16", 6.28, 4.03, 0.260, 0.405),
    ("W6X20", 6.20, 6.02, 0.260, 0.365),
    ("W6X25", 6.38, 6.08, 0.320, 0.455),
    ("W8X10", 7.89, 3.94, 0.170, 0.205),
    ("W8X13", 7.99, 4.00, 0.230, 0.255),
    ("W8X15", 8.11, 4.01, 0.245, 0.315),
    ("W8X18", 8.14, 5.25, 0.230, 0.330),
    ("W8X21", 8.28, 5.27, 0.250, 0.400),
    ("W8X24", 7.93, 6.50, 0.245, 0.400),
    ("W8X28", 8.06, 6.54, 0.285, 0.465),
    ("W8X31", 8.00, 8.00, 0.285, 0.435),
    ("W8X35", 8.12, 8.02, 0.310, 0.495),
    ("W8X40", 8.25, 8.07, 0.360, 0.560),
    ("W8X48", 8.50, 8.11, 0.400, 0.685),
    ("W8X58", 8.75, 8.22, 0.510, 0.810),
    ("W8X67", 9.00, 8.28, 0.570, 0.935),
    ("W10X12", 9.87, 3.96, 0.190, 0.210),
    ("W10X15", 9.99, 4.00, 0.230, 0.270),
    ("W10X19", 10.2, 4.02, 0.250, 0.395),
    ("W10X22", 10.2, 5.75, 0.240, 0.360),
    ("W10X26", 10.3, 5.77, 0.260, 0.440),
    ("W10X30", 10.5, 5.81, 0.300, 0.510),
    ("W10X33", 9.73, 7.96, 0.290, 0.435),
    ("W10X39", 9.92, 7.99, 0.315, 0.530),
    ("W10X45", 10.1, 8.02, 0.350, 0.620),
    ("W10X49", 10.0, 10.0, 0.340, 0.560),
    ("W10X54", 10.1, 10.0, 0.370, 0.615),
    ("W10X60", 10.2, 10.1, 0.420, 0.680),
    ("W10X68", 10.4, 10.1, 0.470, 0.770),
    ("W10X77", 10.6, 10.2, 0.530, 0.870),
    ("W10X88", 10.8, 10.3, 0.605, 0.990),
    ("W12X14", 11.9, 3.97, 0.200, 0.225),
    ("W12X16", 12.0, 3.99, 0.220, 0.265),
    ("W12X19", 12.2, 4.01, 0.235, 0.350),
    ("W12X22", 12.3, 4.03, 0.260, 0.425),
    ("W12X26", 12.2, 6.49, 0.230, 0.380),
    ("W12X30", 12.3, 6.52, 0.260, 0.440),
    ("W12X35", 12.5, 6.56, 0.300, 0.520),
    ("W12X40", 11.9, 8.01, 0.295, 0.515),
    ("W12X45", 12.1, 8.05, 0.335, 0.575),
    ("W12X50", 12.2, 8.08, 0.370, 0.640),
    ("W12X53", 12.1, 10.0, 0.345, 0.575),
    ("W12X58", 12.2, 10.0, 0.360, 0.640),
    ("W12X65", 12.1, 12.0, 0.390, 0.605),
    ("W12X72", 12.3, 12.0, 0.430, 0.670),
    ("W12X79", 12.4, 12.1, 0.470, 0.735),
    ("W12X87", 12.5, 12.1, 0.515, 0.810),
    ("W12X96", 12.7, 12.2, 0.550, 0.900),
    ("W14X22", 13.7, 5.00, 0.230, 0.335),
    ("W14X26", 13.9, 5.03, 0.255, 0.420),
    ("W14X30", 13.8, 6.73, 0.270, 0.385),
    ("W14X34", 14.0, 6.75, 0.285, 0.455),
    ("W14X38", 14.1, 6.77, 0.310, 0.515),
    ("W14X43", 13.7, 8.00, 0.305, 0.530),
    ("W14X48", 13.8, 8.03, 0.340, 0.595),
    ("W14X53", 13.9, 8.06, 0.370, 0.660),
    ("W14X61", 13.9, 10.0, 0.375, 0.645),
    ("W14X68", 14.0, 10.0, 0.415, 0.720),
    ("W14X74", 14.2, 10.1, 0.450, 0.785),
    ("W14X82", 14.3, 10.1, 0.510, 0.855),
    ("W14X90", 14.0, 14.5, 0.440, 0.710),
    ("W14X99", 14.2, 14.6, 0.485, 0.780),
    ("W14X109", 14.3, 14.6, 0.525, 0.860),
    ("W14X120", 14.5, 14.7, 0.590, 0.940),
    ("W14X132", 14.7, 14.7, 0.645, 1.030),
    ("W16X26", 15.7, 5.50, 0.250, 0.345),
    ("W16X31", 15.9, 5.53, 0.275, 0.440),
    ("W16X36", 15.9, 6.99, 0.295, 0.430),
    ("W16X40", 16.0, 7.00, 0.305, 0.505),
    ("W16X45", 16.1, 7.04, 0.345, 0.565),
    ("W16X50", 16.3, 7.07, 0.380, 0.630),
    ("W16X57", 16.4, 7.12, 0.430, 0.715),
    ("W16X67", 16.3, 10.2, 0.395, 0.665),
    ("W16X77", 16.5, 10.3, 0.455, 0.760),
    ("W18X35", 17.7, 6.00, 0.300, 0.425),
    ("W18X40", 17.9, 6.02, 0.315, 0.525),
    ("W18X46", 18.1, 6.06, 0.360, 0.605),
    ("W18X50", 18.0, 7.50, 0.355, 0.570),
    ("W18X55", 18.1, 7.53, 0.390, 0.630),
    ("W18X60", 18.2, 7.56, 0.415, 0.695),
    ("W18X65", 18.4, 7.59, 0.450, 0.750),
    ("W18X71", 18.5, 7.64, 0.495, 0.810),
    ("W18X76", 18.2, 11.0, 0.425, 0.680),
    ("W18X86", 18.4, 11.1, 0.480, 0.770),
    ("W21X44", 20.7, 6.50, 0.350, 0.450),
    ("W21X50", 20.8, 6.53, 0.380, 0.535),
    ("W21X57", 21.1, 6.56, 0.405, 0.650),
    ("W21X62", 21.0, 8.24, 0.400, 0.615),
    ("W21X68", 21.1, 8.27, 0.430, 0.685),
    ("W21X73", 21.2, 8.30, 0.455, 0.740),
    ("W21X83", 21.4, 8.36, 0.515, 0.835),
    ("W21X93", 21.6, 8.42, 0.580, 0.930),
    ("W24X55", 23.6, 7.01, 0.395, 0.505),
    ("W24X62", 23.7, 7.04, 0.430, 0.590),
    ("W24X68", 23.7, 8.97, 0.415, 0.585),
    ("W24X76", 23.9, 8.99, 0.440, 0.680),
    ("W24X84", 24.1, 9.02, 0.470, 0.770),
    ("W24X94", 24.3, 9.07, 0.515, 0.875),
    ("W24X104", 24.1, 12.8, 0.500, 0.750),
    ("W27X84", 26.7, 10.0, 0.460, 0.640),
    ("W27X94", 26.9, 10.0, 0.490, 0.745),
    ("W27X102", 27.1, 10.0, 0.515, 0.830),
    ("W30X90", 29.5, 10.4, 0.470, 0.610),
    ("W30X99", 29.7, 10.5, 0.520, 0.670),
    ("W30X108", 29.8, 10.5, 0.545, 0.760),
    ("W33X118", 32.9, 11.5, 0.550, 0.740),
    ("W33X130", 33.1, 11.5, 0.580, 0.855),
    ("W36X135", 35.6, 12.0, 0.600, 0.790),
    ("W36X150", 35.9, 12.0, 0.625, 0.940),
];

fn w_shapes(c: &mut Catalog) {
    for &(des, d, bf, tw, tf) in W {
        let label = format!("{} wide flange", des.replace('X', "x"));
        c.add(D, "Steel W shapes", des, label, "beam column wide flange section aisc", i_shape, args_s(des, &[d, bf, tw, tf, 0.0]), REAL);
    }
}

/// An I section `h` deep, `b` wide, web `tw`, flanges `tf`, root fillets `r`, centred, with its
/// designation as hidden text.
fn i_shape(k: &mut Kit, a: &Args) {
    let (h, b, tw, tf, r) = (a.get(0), a.get(1), a.get(2), a.get(3), a.get(4));
    let (x, y, w) = (b / 2.0, h / 2.0, tw / 2.0);
    let f = -(PI / 8.0).tan();
    let (yi, r) = (y - tf, r.min(x - w).min(yi_room(h, tf)).max(0.0));
    let pts = if r > 0.0 {
        vec![
            (-x, -y, 0.0),
            (x, -y, 0.0),
            (x, -yi, 0.0),
            (w + r, -yi, f),
            (w, -yi + r, 0.0),
            (w, yi - r, f),
            (w + r, yi, 0.0),
            (x, yi, 0.0),
            (x, y, 0.0),
            (-x, y, 0.0),
            (-x, yi, 0.0),
            (-w - r, yi, f),
            (-w, yi - r, 0.0),
            (-w, -yi + r, f),
            (-w - r, -yi, 0.0),
            (-x, -yi, 0.0),
        ]
    } else {
        vec![
            (-x, -y, 0.0),
            (x, -y, 0.0),
            (x, -yi, 0.0),
            (w, -yi, 0.0),
            (w, yi, 0.0),
            (x, yi, 0.0),
            (x, y, 0.0),
            (-x, y, 0.0),
            (-x, yi, 0.0),
            (-w, yi, 0.0),
            (-w, -yi, 0.0),
            (-x, -yi, 0.0),
        ]
    };
    k.pline(&pts, true);
    centre_marks(k, x.max(y));
    k.attdef_hidden("MARK", "Member mark", a.s, (x + h * 0.1, y), h * 0.06);
}

fn yi_room(h: f64, tf: f64) -> f64 {
    (h / 2.0 - tf) * 0.5
}

/// Short centre lines through the origin, `size` out.
fn centre_marks(k: &mut Kit, size: f64) {
    k.center_lines(true);
    k.line((-size * 1.15, 0.0), (size * 1.15, 0.0));
    k.line((0.0, -size * 1.15), (0.0, size * 1.15));
    k.center_lines(false);
}

// ---------------------------------------------------------------------------------------------
// Channels: (designation, d, bf, tw, tf).

const C: &[(&str, f64, f64, f64, f64)] = &[
    ("C3X4.1", 3.0, 1.41, 0.170, 0.273),
    ("C3X5", 3.0, 1.50, 0.258, 0.273),
    ("C4X5.4", 4.0, 1.58, 0.184, 0.296),
    ("C4X7.25", 4.0, 1.72, 0.321, 0.296),
    ("C5X6.7", 5.0, 1.75, 0.190, 0.320),
    ("C5X9", 5.0, 1.89, 0.325, 0.320),
    ("C6X8.2", 6.0, 1.92, 0.200, 0.343),
    ("C6X10.5", 6.0, 2.03, 0.314, 0.343),
    ("C6X13", 6.0, 2.16, 0.437, 0.343),
    ("C7X9.8", 7.0, 2.09, 0.210, 0.366),
    ("C7X12.25", 7.0, 2.19, 0.314, 0.366),
    ("C8X11.5", 8.0, 2.26, 0.220, 0.390),
    ("C8X13.75", 8.0, 2.34, 0.303, 0.390),
    ("C8X18.75", 8.0, 2.53, 0.487, 0.390),
    ("C9X13.4", 9.0, 2.43, 0.233, 0.413),
    ("C9X15", 9.0, 2.49, 0.285, 0.413),
    ("C10X15.3", 10.0, 2.60, 0.240, 0.436),
    ("C10X20", 10.0, 2.74, 0.379, 0.436),
    ("C10X25", 10.0, 2.89, 0.526, 0.436),
    ("C12X20.7", 12.0, 2.94, 0.282, 0.501),
    ("C12X25", 12.0, 3.05, 0.387, 0.501),
    ("C12X30", 12.0, 3.17, 0.510, 0.501),
    ("C15X33.9", 15.0, 3.40, 0.400, 0.650),
    ("C15X40", 15.0, 3.52, 0.520, 0.650),
    ("C15X50", 15.0, 3.72, 0.716, 0.650),
];

fn channels(c: &mut Catalog) {
    for &(des, d, bf, tw, tf) in C {
        let label = format!("{} channel", des.replace('X', "x"));
        c.add(D, "Steel channels", des, label, "american standard channel section aisc", channel, args_s(des, &[d, bf, tw, tf]), REAL);
    }
}

/// A channel `d` deep from the back of its web (X 0), flanges toward +X.
fn channel(k: &mut Kit, a: &Args) {
    let (d, bf, tw, tf) = (a.get(0), a.get(1), a.get(2), a.get(3));
    let y = d / 2.0;
    k.poly(&[(0.0, -y), (bf, -y), (bf, -y + tf), (tw, -y + tf), (tw, y - tf), (bf, y - tf), (bf, y), (0.0, y)]);
    k.center_lines(true);
    k.line((-d * 0.1, 0.0), (bf * 1.2, 0.0));
    k.center_lines(false);
    k.attdef_hidden("MARK", "Member mark", a.s, (bf + d * 0.1, y), d * 0.06);
}

// ---------------------------------------------------------------------------------------------
// Angles: legs and thicknesses in sixteenths of an inch.

fn angles(c: &mut Catalog) {
    let equal: &[(f64, &[u32])] = &[
        (2.0, &[2, 3, 4, 5, 6]),
        (2.5, &[3, 4, 5, 6, 8]),
        (3.0, &[3, 4, 5, 6, 7, 8]),
        (3.5, &[4, 5, 6, 7, 8]),
        (4.0, &[4, 5, 6, 7, 8, 10, 12]),
        (5.0, &[5, 6, 7, 8, 10, 12, 14]),
        (6.0, &[5, 6, 7, 8, 9, 10, 12, 14, 16]),
        (8.0, &[8, 9, 10, 12, 14, 16, 18]),
    ];
    for &(leg, ts) in equal {
        for &t16 in ts {
            angle_part(c, leg, leg, f64::from(t16) / 16.0);
        }
    }
    let unequal: &[(f64, f64, &[u32])] = &[
        (3.0, 2.0, &[3, 4, 5, 6, 8]),
        (3.5, 2.5, &[4, 5, 6, 8]),
        (4.0, 3.0, &[4, 5, 6, 8, 10]),
        (5.0, 3.0, &[4, 5, 6, 8]),
        (5.0, 3.5, &[5, 6, 8, 10, 12]),
        (6.0, 4.0, &[5, 6, 7, 8, 9, 10, 12, 14]),
        (7.0, 4.0, &[6, 7, 8, 10, 12]),
        (8.0, 4.0, &[8, 9, 10, 12, 14, 16]),
        (8.0, 6.0, &[8, 9, 10, 12, 14, 16]),
    ];
    for &(long, short, ts) in unequal {
        for &t16 in ts {
            angle_part(c, long, short, f64::from(t16) / 16.0);
        }
    }
}

fn angle_part(c: &mut Catalog, long: f64, short: f64, t: f64) {
    let des = format!("L{}X{}X{}", frac(long), frac(short), frac(t));
    let label = format!("{} angle", des.replace('X', "x"));
    let kw = if long == short { "equal leg angle section aisc" } else { "unequal leg angle section aisc" };
    c.add(D, "Steel angles", &des, label, kw, angle, args(&[long, short, t, 0.0]), REAL);
}

/// An angle with its heel at the origin: long leg up (Y), short leg along X, root fillet `r`.
fn angle(k: &mut Kit, a: &Args) {
    let (long, short, t, r) = (a.get(0), a.get(1), a.get(2), a.get(3));
    if r > 0.0 {
        let f = -(PI / 8.0).tan();
        k.pline(&[(0.0, 0.0, 0.0), (short, 0.0, 0.0), (short, t, 0.0), (t + r, t, f), (t, t + r, 0.0), (t, long, 0.0), (0.0, long, 0.0)], true);
    } else {
        k.poly(&[(0.0, 0.0), (short, 0.0), (short, t), (t, t), (t, long), (0.0, long)]);
    }
    k.attdef_hidden("MARK", "Member mark", "", (short * 1.1, long), long * 0.06);
}

// ---------------------------------------------------------------------------------------------
// HSS

fn hss(c: &mut Catalog) {
    let square: &[(f64, &[u32])] = &[
        (2.0, &[2, 3, 4]),
        (2.5, &[2, 3, 4, 5]),
        (3.0, &[2, 3, 4, 5, 6]),
        (3.5, &[2, 3, 4, 5]),
        (4.0, &[2, 3, 4, 5, 6, 8]),
        (5.0, &[2, 3, 4, 5, 6, 8]),
        (6.0, &[2, 3, 4, 5, 6, 8, 10]),
        (7.0, &[3, 4, 5, 6, 8, 10]),
        (8.0, &[3, 4, 5, 6, 8, 10]),
        (10.0, &[4, 5, 6, 8, 10]),
        (12.0, &[4, 5, 6, 8, 10]),
    ];
    for &(s, ts) in square {
        for &t16 in ts {
            hss_rect_part(c, s, s, f64::from(t16) / 16.0);
        }
    }
    let rect: &[(f64, f64, &[u32])] = &[
        (4.0, 2.0, &[2, 3, 4]),
        (5.0, 3.0, &[2, 3, 4, 5]),
        (6.0, 2.0, &[3, 4]),
        (6.0, 3.0, &[3, 4, 5, 6]),
        (6.0, 4.0, &[3, 4, 5, 6, 8]),
        (8.0, 4.0, &[3, 4, 5, 6, 8]),
        (8.0, 6.0, &[3, 4, 5, 6, 8]),
        (10.0, 4.0, &[3, 4, 5, 6, 8]),
        (10.0, 6.0, &[3, 4, 5, 6, 8]),
        (12.0, 4.0, &[3, 4, 5, 6, 8]),
        (12.0, 6.0, &[3, 4, 5, 6, 8]),
        (12.0, 8.0, &[4, 5, 6, 8, 10]),
    ];
    for &(h, b, ts) in rect {
        for &t16 in ts {
            hss_rect_part(c, h, b, f64::from(t16) / 16.0);
        }
    }
    let round: &[(f64, &[f64])] = &[
        (4.0, &[0.250]),
        (4.5, &[0.188, 0.250, 0.375]),
        (5.0, &[0.250, 0.375, 0.500]),
        (6.0, &[0.250, 0.375, 0.500]),
        (7.0, &[0.250, 0.375, 0.500]),
        (8.0, &[0.250, 0.375, 0.500]),
        (10.0, &[0.250, 0.375, 0.500]),
        (12.0, &[0.250, 0.375, 0.500]),
    ];
    for &(od, ts) in round {
        for &t in ts {
            let des = format!("HSS{od:.3}X{t:.3}");
            let label = format!("{} round HSS", des.replace('X', "x"));
            c.add(D, "Steel HSS", &des, label, "round hollow structural section tube aisc", tube, args(&[od, t * 0.93]), REAL);
        }
    }
}

fn hss_rect_part(c: &mut Catalog, h: f64, b: f64, t: f64) {
    let des = format!("HSS{}X{}X{}", frac(h), frac(b), frac(t));
    let label = format!("{} {} HSS", des.replace('X', "x"), if h == b { "square" } else { "rectangular" });
    c.add(D, "Steel HSS", &des, label, "hollow structural section tube aisc", hss_rect, args(&[h, b, t * 0.93]), REAL);
}

/// A rectangular hollow section `h` deep, `b` wide, wall `t`: outer corners of radius 2t, inner t.
fn hss_rect(k: &mut Kit, a: &Args) {
    let (h, b, t) = (a.get(0), a.get(1), a.get(2));
    let (x, y) = (b / 2.0, h / 2.0);
    k.round_rect((-x, -y), (x, y), 2.0 * t);
    k.round_rect((-x + t, -y + t), (x - t, y - t), t);
    centre_marks(k, x.max(y));
    k.attdef_hidden("MARK", "Member mark", "", (x * 1.2, y), h * 0.06);
}

/// A round tube or pipe `od` across with wall `t`.
fn tube(k: &mut Kit, a: &Args) {
    let (od, t) = (a.get(0), a.get(1));
    k.circle((0.0, 0.0), od / 2.0);
    k.circle((0.0, 0.0), (od / 2.0 - t).max(od * 0.05));
    centre_marks(k, od / 2.0);
    k.attdef_hidden("MARK", "Member mark", "", (od * 0.6, od / 2.0), od * 0.06);
}

// ---------------------------------------------------------------------------------------------
// Pipe: (nominal size, OD, STD wall, XS wall, XXS wall or 0).

#[allow(clippy::approx_constant)]
const PIPE: &[(f64, f64, f64, f64, f64)] = &[
    (0.5, 0.840, 0.109, 0.147, 0.294),
    (0.75, 1.050, 0.113, 0.154, 0.308),
    (1.0, 1.315, 0.133, 0.179, 0.358),
    (1.25, 1.660, 0.140, 0.191, 0.382),
    (1.5, 1.900, 0.145, 0.200, 0.400),
    (2.0, 2.375, 0.154, 0.218, 0.436),
    (2.5, 2.875, 0.203, 0.276, 0.552),
    (3.0, 3.500, 0.216, 0.300, 0.600),
    (3.5, 4.000, 0.226, 0.318, 0.0),
    (4.0, 4.500, 0.237, 0.337, 0.674),
    (5.0, 5.563, 0.258, 0.375, 0.750),
    (6.0, 6.625, 0.280, 0.432, 0.864),
    (8.0, 8.625, 0.322, 0.500, 0.875),
    (10.0, 10.750, 0.365, 0.500, 0.0),
    (12.0, 12.750, 0.375, 0.500, 0.0),
];

fn pipe(c: &mut Catalog) {
    for &(nps, od, std, xs, xxs) in PIPE {
        for (word, wall) in [("STD", std), ("XS", xs), ("XXS", xxs)] {
            if wall <= 0.0 {
                continue;
            }
            let des = format!("PIPE_{}_{word}", frac(nps));
            let label = format!("Pipe {} {word} ({od:.3}\" OD)", frac(nps));
            c.add(D, "Steel pipe", &des, label, "round pipe section standard extra strong", tube, args(&[od, wall]), REAL);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// European shapes, millimetres: (designation, h, b, tw, tf, r).

const IPE: &[(&str, f64, f64, f64, f64, f64)] = &[
    ("IPE80", 80.0, 46.0, 3.8, 5.2, 5.0),
    ("IPE100", 100.0, 55.0, 4.1, 5.7, 7.0),
    ("IPE120", 120.0, 64.0, 4.4, 6.3, 7.0),
    ("IPE140", 140.0, 73.0, 4.7, 6.9, 7.0),
    ("IPE160", 160.0, 82.0, 5.0, 7.4, 9.0),
    ("IPE180", 180.0, 91.0, 5.3, 8.0, 9.0),
    ("IPE200", 200.0, 100.0, 5.6, 8.5, 12.0),
    ("IPE220", 220.0, 110.0, 5.9, 9.2, 12.0),
    ("IPE240", 240.0, 120.0, 6.2, 9.8, 15.0),
    ("IPE270", 270.0, 135.0, 6.6, 10.2, 15.0),
    ("IPE300", 300.0, 150.0, 7.1, 10.7, 15.0),
    ("IPE330", 330.0, 160.0, 7.5, 11.5, 18.0),
    ("IPE360", 360.0, 170.0, 8.0, 12.7, 18.0),
    ("IPE400", 400.0, 180.0, 8.6, 13.5, 21.0),
    ("IPE450", 450.0, 190.0, 9.4, 14.6, 21.0),
    ("IPE500", 500.0, 200.0, 10.2, 16.0, 21.0),
    ("IPE550", 550.0, 210.0, 11.1, 17.2, 24.0),
    ("IPE600", 600.0, 220.0, 12.0, 19.0, 24.0),
];

const HEA: &[(&str, f64, f64, f64, f64, f64)] = &[
    ("HEA100", 96.0, 100.0, 5.0, 8.0, 12.0),
    ("HEA120", 114.0, 120.0, 5.0, 8.0, 12.0),
    ("HEA140", 133.0, 140.0, 5.5, 8.5, 12.0),
    ("HEA160", 152.0, 160.0, 6.0, 9.0, 15.0),
    ("HEA180", 171.0, 180.0, 6.0, 9.5, 15.0),
    ("HEA200", 190.0, 200.0, 6.5, 10.0, 18.0),
    ("HEA220", 210.0, 220.0, 7.0, 11.0, 18.0),
    ("HEA240", 230.0, 240.0, 7.5, 12.0, 21.0),
    ("HEA260", 250.0, 260.0, 7.5, 12.5, 24.0),
    ("HEA280", 270.0, 280.0, 8.0, 13.0, 24.0),
    ("HEA300", 290.0, 300.0, 8.5, 14.0, 27.0),
    ("HEA320", 310.0, 300.0, 9.0, 15.5, 27.0),
    ("HEA340", 330.0, 300.0, 9.5, 16.5, 27.0),
    ("HEA360", 350.0, 300.0, 10.0, 17.5, 27.0),
    ("HEA400", 390.0, 300.0, 11.0, 19.0, 27.0),
    ("HEA450", 440.0, 300.0, 11.5, 21.0, 27.0),
    ("HEA500", 490.0, 300.0, 12.0, 23.0, 27.0),
    ("HEA550", 540.0, 300.0, 12.5, 24.0, 27.0),
    ("HEA600", 590.0, 300.0, 13.0, 25.0, 27.0),
    ("HEA650", 640.0, 300.0, 13.5, 26.0, 27.0),
    ("HEA700", 690.0, 300.0, 14.5, 27.0, 27.0),
    ("HEA800", 790.0, 300.0, 15.0, 28.0, 30.0),
    ("HEA900", 890.0, 300.0, 16.0, 30.0, 30.0),
    ("HEA1000", 990.0, 300.0, 16.5, 31.0, 30.0),
];

const HEB: &[(&str, f64, f64, f64, f64, f64)] = &[
    ("HEB100", 100.0, 100.0, 6.0, 10.0, 12.0),
    ("HEB120", 120.0, 120.0, 6.5, 11.0, 12.0),
    ("HEB140", 140.0, 140.0, 7.0, 12.0, 12.0),
    ("HEB160", 160.0, 160.0, 8.0, 13.0, 15.0),
    ("HEB180", 180.0, 180.0, 8.5, 14.0, 15.0),
    ("HEB200", 200.0, 200.0, 9.0, 15.0, 18.0),
    ("HEB220", 220.0, 220.0, 9.5, 16.0, 18.0),
    ("HEB240", 240.0, 240.0, 10.0, 17.0, 21.0),
    ("HEB260", 260.0, 260.0, 10.0, 17.5, 24.0),
    ("HEB280", 280.0, 280.0, 10.5, 18.0, 24.0),
    ("HEB300", 300.0, 300.0, 11.0, 19.0, 27.0),
    ("HEB320", 320.0, 300.0, 11.5, 20.5, 27.0),
    ("HEB340", 340.0, 300.0, 12.0, 21.5, 27.0),
    ("HEB360", 360.0, 300.0, 12.5, 22.5, 27.0),
    ("HEB400", 400.0, 300.0, 13.5, 24.0, 27.0),
    ("HEB450", 450.0, 300.0, 14.0, 26.0, 27.0),
    ("HEB500", 500.0, 300.0, 14.5, 28.0, 27.0),
    ("HEB550", 550.0, 300.0, 15.0, 29.0, 27.0),
    ("HEB600", 600.0, 300.0, 15.5, 30.0, 27.0),
    ("HEB650", 650.0, 300.0, 16.0, 31.0, 27.0),
    ("HEB700", 700.0, 300.0, 17.0, 32.0, 27.0),
    ("HEB800", 800.0, 300.0, 17.5, 33.0, 30.0),
    ("HEB900", 900.0, 300.0, 18.5, 35.0, 30.0),
    ("HEB1000", 1000.0, 300.0, 19.0, 36.0, 30.0),
];

/// UPN channels, millimetres: (designation, h, b, tw, tf).
const UPN: &[(&str, f64, f64, f64, f64)] = &[
    ("UPN80", 80.0, 45.0, 6.0, 8.0),
    ("UPN100", 100.0, 50.0, 6.0, 8.5),
    ("UPN120", 120.0, 55.0, 7.0, 9.0),
    ("UPN140", 140.0, 60.0, 7.0, 10.0),
    ("UPN160", 160.0, 65.0, 7.5, 10.5),
    ("UPN180", 180.0, 70.0, 8.0, 11.0),
    ("UPN200", 200.0, 75.0, 8.5, 11.5),
    ("UPN220", 220.0, 80.0, 9.0, 12.5),
    ("UPN240", 240.0, 85.0, 9.5, 13.0),
    ("UPN260", 260.0, 90.0, 10.0, 14.0),
    ("UPN280", 280.0, 95.0, 10.0, 15.0),
    ("UPN300", 300.0, 100.0, 10.0, 16.0),
];

/// Metric equal angles (leg, thickness, root radius), millimetres.
const L_METRIC: &[(f64, f64, f64)] = &[
    (20.0, 3.0, 3.5),
    (25.0, 3.0, 3.5),
    (25.0, 4.0, 3.5),
    (30.0, 3.0, 5.0),
    (30.0, 4.0, 5.0),
    (35.0, 4.0, 5.0),
    (40.0, 4.0, 6.0),
    (40.0, 5.0, 6.0),
    (45.0, 5.0, 7.0),
    (50.0, 5.0, 7.0),
    (50.0, 6.0, 7.0),
    (60.0, 6.0, 8.0),
    (60.0, 8.0, 8.0),
    (70.0, 7.0, 9.0),
    (80.0, 8.0, 10.0),
    (90.0, 9.0, 11.0),
    (100.0, 10.0, 12.0),
    (120.0, 12.0, 13.0),
    (150.0, 15.0, 16.0),
    (200.0, 20.0, 18.0),
];

fn metric_shapes(c: &mut Catalog) {
    const MS: &str = "Steel metric shapes";
    for (table, kind) in [(IPE, "IPE beam"), (HEA, "HEA wide flange (light)"), (HEB, "HEB wide flange")] {
        for &(des, h, b, tw, tf, r) in table {
            c.add(D, MS, des, format!("{des} {kind}"), "european section en 10365 metric", i_shape, args_s(des, &[h, b, tw, tf, r]), METRIC);
        }
    }
    for &(des, h, b, tw, tf) in UPN {
        c.add(D, MS, des, format!("{des} channel"), "european section metric", channel, args_s(des, &[h, b, tw, tf]), METRIC);
    }
    for &(leg, t, r) in L_METRIC {
        let des = format!("L{}X{}X{}", num(leg), num(leg), num(t));
        c.add(D, MS, &des, format!("L{leg}x{leg}x{t} equal angle"), "european angle en 10056 metric", angle, args(&[leg, leg, t, r]), METRIC);
    }
}

// ---------------------------------------------------------------------------------------------
// Reinforcing bars

/// US bar sizes and their nominal diameters (inches).
const US_BARS: &[(u32, f64)] =
    &[(3, 0.375), (4, 0.500), (5, 0.625), (6, 0.750), (7, 0.875), (8, 1.000), (9, 1.128), (10, 1.270), (11, 1.410), (14, 1.693), (18, 2.257)];

/// Canadian metric bars and their nominal diameters (mm).
const CA_BARS: &[(&str, f64)] =
    &[("10M", 11.3), ("15M", 16.0), ("20M", 19.5), ("25M", 25.2), ("30M", 29.9), ("35M", 35.7), ("45M", 43.7), ("55M", 56.4)];

/// European bar diameters (mm).
const EU_BARS: &[f64] = &[6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 20.0, 25.0, 28.0, 32.0, 40.0];

fn rebar(c: &mut Catalog) {
    const RB: &str = "Rebar";
    for &(n, d) in US_BARS {
        c.add(
            D,
            RB,
            &format!("REBAR_US{n}"),
            format!("Rebar #{n} section ({}\" diameter)", frac(d)),
            "reinforcing bar astm a615",
            bar_section,
            args_s(US_MARKS.get(n as usize).copied().unwrap_or(""), &[d]),
            REAL,
        );
    }
    for &(n, d) in CA_BARS {
        c.add(
            D,
            RB,
            &format!("REBAR_{n}"),
            format!("Rebar {n} section ({d} mm)"),
            "reinforcing bar csa metric",
            bar_section,
            args_s(n, &[d]),
            METRIC,
        );
    }
    for &d in EU_BARS {
        let mark = EU_MARKS.iter().find(|(v, _)| *v == d).map_or("", |(_, m)| *m);
        c.add(
            D,
            RB,
            &format!("REBAR_D{}", num(d)),
            format!("Rebar Ø{d} section"),
            "reinforcing bar en metric diameter",
            bar_section,
            args_s(mark, &[d]),
            METRIC,
        );
    }
    // Standard hooks (ACI 318): bend diameter 6 bar diameters through #8, 8 for #9–#11; a 90°
    // hook extends 12 diameters, a 180° hook 4 diameters but at least 2-1/2".
    for &(n, d) in US_BARS.iter().filter(|(n, _)| *n <= 11) {
        let bend = if n <= 8 { 6.0 } else { 8.0 } * d;
        c.add(
            D,
            RB,
            &format!("HOOK90_US{n}"),
            format!("Rebar #{n} standard 90° hook"),
            "reinforcing bar bend aci",
            hook,
            args(&[d, bend, 90.0, 12.0 * d]),
            REAL,
        );
        c.add(
            D,
            RB,
            &format!("HOOK180_US{n}"),
            format!("Rebar #{n} standard 180° hook"),
            "reinforcing bar bend aci",
            hook,
            args(&[d, bend, 180.0, (4.0 * d).max(2.5)]),
            REAL,
        );
    }
    // Stirrups and ties with 135° seismic hooks (bend 4 diameters, extension 6 but at least 3").
    for &(n, d) in US_BARS.iter().filter(|(n, _)| *n <= 5) {
        c.add(
            D,
            RB,
            &format!("STIRRUP_US{n}"),
            format!("Rebar #{n} closed tie, 135° hooks, 12\"x18\""),
            "reinforcing bar stirrup tie hoop",
            stirrup,
            args(&[d, 12.0, 18.0]),
            REAL,
        );
    }
}

/// US bar marks by bar size (the label beside a section).
const US_MARKS: [&str; 19] = ["", "", "", "#3", "#4", "#5", "#6", "#7", "#8", "#9", "#10", "#11", "", "", "#14", "", "", "", "#18"];

/// European bar marks by diameter.
const EU_MARKS: &[(f64, &str)] = &[
    (6.0, "Ø6"),
    (8.0, "Ø8"),
    (10.0, "Ø10"),
    (12.0, "Ø12"),
    (14.0, "Ø14"),
    (16.0, "Ø16"),
    (20.0, "Ø20"),
    (25.0, "Ø25"),
    (28.0, "Ø28"),
    (32.0, "Ø32"),
    (40.0, "Ø40"),
];

/// A bar's section, filled, with its size beside it.
fn bar_section(k: &mut Kit, a: &Args) {
    let d = a.get(0);
    k.dot((0.0, 0.0), d / 2.0);
    k.circle((0.0, 0.0), d / 2.0);
    k.text_left((d, 0.0), d * 0.8, a.s);
}

/// A bar drawn along its centre line with its own width.
fn bar_path(k: &mut Kit, pts: &[(f64, f64, f64)], d: f64) {
    k.pline(pts, false);
    k.set_last_width(d);
}

/// A hooked bar: a straight length along −X to the bend at the origin, the bend of inside
/// diameter `bend` turning `angle` degrees up, then the extension.
fn hook(k: &mut Kit, a: &Args) {
    let (d, bend, angle, ext) = (a.get(0), a.get(1), a.get(2), a.get(3));
    let r = bend / 2.0 + d / 2.0;
    let straight = 20.0 * d;
    let t = angle.to_radians();
    let bulge = (t / 4.0).tan();
    let end_bend = (r * t.sin(), r - r * t.cos());
    let dir = (t.cos(), t.sin());
    let tail = (end_bend.0 + dir.0 * ext, end_bend.1 + dir.1 * ext);
    bar_path(k, &[(-straight, 0.0, 0.0), (0.0, 0.0, bulge), (end_bend.0, end_bend.1, 0.0), (tail.0, tail.1, 0.0)], d);
    k.attdef("MARK", "Bar mark", "", (-straight, d * 2.0), d * 1.5);
}

fn stirrup(k: &mut Kit, a: &Args) {
    let (d, w, h) = (a.get(0), a.get(1), a.get(2));
    let r = 2.0 * d + d / 2.0;
    let k90 = (PI / 8.0).tan();
    // Round the rectangle's corners; the hooks at the top left bend 135° into the core.
    let pts = [
        (r, 0.0, 0.0),
        (w - r, 0.0, k90),
        (w, r, 0.0),
        (w, h - r, k90),
        (w - r, h, 0.0),
        (r, h, k90),
        (0.0, h - r, 0.0),
        (0.0, r, k90),
        (r, 0.0, 0.0),
    ];
    bar_path(k, &pts, d);
    let ext = (6.0 * d).max(3.0);
    let s = std::f64::consts::FRAC_1_SQRT_2;
    bar_path(k, &[(r, h, 0.0), (r + ext * s, h - ext * s, 0.0)], d);
    bar_path(k, &[(0.0, h - r, 0.0), (ext * s, h - r - ext * s, 0.0)], d);
    k.attdef("MARK", "Bar mark", "", (w + d * 3.0, h), 1.0);
}

// ---------------------------------------------------------------------------------------------
// Lumber, base plates, masonry

fn lumber(c: &mut Catalog) {
    for (nom_t, t) in [(1.0, 0.75), (2.0, 1.5), (4.0, 3.5), (6.0, 5.5), (8.0, 7.5)] {
        let widths: &[(f64, f64)] = match nom_t as i64 {
            1 => &[(4.0, 3.5), (6.0, 5.5), (8.0, 7.25)],
            2 | 4 => &[(4.0, 3.5), (6.0, 5.5), (8.0, 7.25), (10.0, 9.25), (12.0, 11.25)],
            6 => &[(6.0, 5.5), (8.0, 7.5), (10.0, 9.5), (12.0, 11.5)],
            _ => &[(8.0, 7.5)],
        };
        for &(nom_w, w) in widths {
            c.add(
                D,
                "Lumber",
                &format!("LUMBER_{}X{}", num(nom_t), num(nom_w)),
                format!("Lumber {}x{} ({}x{} actual)", nom_t, nom_w, frac(t), frac(w)),
                "wood sawn dimensional timber stud joist post",
                lumber_section,
                args(&[t, w]),
                REAL,
            );
        }
    }
}

fn lumber_section(k: &mut Kit, a: &Args) {
    let (t, w) = (a.get(0), a.get(1));
    let (x, y) = (t / 2.0, w / 2.0);
    k.rect((-x, -y), (x, y));
    k.line((-x, -y), (x, y));
    k.line((x, -y), (-x, y));
}

fn plates_and_masonry(c: &mut Catalog) {
    for s in [10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0] {
        let bolt = if s <= 14.0 { 0.75 } else { 1.0 };
        c.add(
            D,
            "Base plates",
            &format!("BASE_PLATE_{}X{}", num(s), num(s)),
            format!("Base plate, {}x{} with 4 anchor rods {} diameter", inches(s), inches(s), inches(bolt)),
            "column anchor bolt",
            base_plate,
            args(&[s, bolt]),
            REAL,
        );
    }
    for w in [4.0, 6.0, 8.0, 10.0, 12.0] {
        c.add(
            D,
            "Masonry units",
            &format!("CMU_{}", num(w)),
            format!("Concrete block, {} nominal (plan)", inches(w)),
            "cmu masonry unit",
            cmu,
            args(&[w]),
            REAL,
        );
    }
    for (word, label, l, w) in [("MODULAR", "modular", 7.625, 3.625), ("NORMAN", "Norman", 11.625, 3.625), ("KING", "king", 9.625, 2.75)] {
        c.add(D, "Masonry units", &format!("BRICK_{word}"), format!("Brick, {label} (plan)"), "masonry unit clay", brick, args(&[l, w]), REAL);
    }
}

fn base_plate(k: &mut Kit, a: &Args) {
    let (s, bolt) = (a.get(0), a.get(1));
    let h = s / 2.0;
    k.rect((-h, -h), (h, h));
    let e = h - (bolt * 2.0).max(1.5);
    for (x, y) in [(-e, -e), (e, -e), (e, e), (-e, e)] {
        k.circle((x, y), bolt / 2.0 + 1.0 / 16.0 + 0.25);
        k.circle((x, y), bolt / 2.0);
    }
    centre_marks(k, h);
}

fn cmu(k: &mut Kit, a: &Args) {
    let nominal = a.get(0);
    let (l, w) = (15.625, nominal - 0.375);
    let shell = if nominal <= 4.0 { 0.75 } else { 1.25 };
    k.rect((0.0, 0.0), (l, w));
    let cell = (l - 3.0 * shell) / 2.0;
    if w > 2.0 * shell + 0.5 {
        for i in 0..2 {
            let x0 = shell + i as f64 * (cell + shell);
            k.round_rect((x0, shell), (x0 + cell, w - shell), 0.5);
        }
    }
}

fn brick(k: &mut Kit, a: &Args) {
    let (l, w) = (a.get(0), a.get(1));
    k.rect((0.0, 0.0), (l, w));
    k.rect((l + 0.375, 0.0), (2.0 * l + 0.375, w));
}
