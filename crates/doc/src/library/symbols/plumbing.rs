//! Plumbing symbols: fixtures at real size (inches; insertion point at the middle of the back
//! edge, the fixture extending in −Y), and piping symbols at plotted size (inches on paper;
//! centred, pipe along X). Drawn for CADaddy from common plan conventions.

use super::{Sketch, Symbol};

const FIXTURES: &str = "Plumbing fixtures";
const PIPING: &str = "Plumbing piping";

/// Fixture tag height (inches, real size) and its place to the right of the fixture.
const TAG_H: f64 = 4.0;
/// Piping symbols: the size or tag text height on paper.
const NOTE_H: f64 = 0.08;

const fn fixture(name: &'static str, label: &'static str, keywords: &'static str, draw: fn(&mut Sketch)) -> Symbol {
    Symbol { name, label, category: FIXTURES, trade: "PLUMBING", annotation: false, keywords, draw }
}
const fn piping(name: &'static str, label: &'static str, keywords: &'static str, draw: fn(&mut Sketch)) -> Symbol {
    Symbol { name, label, category: PIPING, trade: "PLUMBING", annotation: true, keywords, draw }
}

pub(super) const SYMBOLS: &[Symbol] = &[
    fixture("PL_WATER_CLOSET", "Water closet (tank)", "toilet wc tank floor mounted", |k| {
        k.rect((-10.0, 0.0), (10.0, -8.0));
        k.rect((-3.0, -8.0), (3.0, -10.0));
        k.ellipse((0.0, -19.0), 7.5, 10.0);
        k.ellipse((0.0, -19.5), 5.5, 7.5);
        k.attdef("TAG", "Fixture tag", "WC-1", (12.0, -16.0), TAG_H);
    }),
    fixture("PL_WATER_CLOSET_WALL", "Water closet (wall hung, flush valve)", "toilet wc flushometer wall hung carrier", |k| {
        k.rect((-7.0, 0.0), (7.0, -3.0));
        k.ellipse((0.0, -14.0), 7.5, 11.0);
        k.ellipse((0.0, -14.5), 5.5, 8.0);
        k.circle((-9.0, -1.5), 1.25);
        k.attdef("TAG", "Fixture tag", "WC-1", (10.0, -14.0), TAG_H);
    }),
    fixture("PL_URINAL", "Urinal (wall hung)", "urinal ur flush valve", |k| {
        k.pline(&[(-9.0, 0.0, 0.0), (-9.0, -9.0, 1.0), (9.0, -9.0, 0.0), (9.0, 0.0, 0.0)], false, 0.0);
        k.pline(&[(-6.5, -2.0, 0.0), (-6.5, -9.0, 1.0), (6.5, -9.0, 0.0), (6.5, -2.0, 0.0)], true, 0.0);
        k.circle((0.0, -10.0), 1.0);
        k.attdef("TAG", "Fixture tag", "UR-1", (11.0, -12.0), TAG_H);
    }),
    fixture("PL_LAVATORY", "Lavatory (wall hung)", "lav sink hand wash basin bathroom", |k| {
        k.rect((-10.0, 0.0), (10.0, -18.0));
        k.ellipse((0.0, -10.0), 7.0, 5.5);
        k.circle((0.0, -2.5), 0.75);
        k.circle((0.0, -10.0), 0.75);
        k.attdef("TAG", "Fixture tag", "L-1", (12.0, -12.0), TAG_H);
    }),
    fixture("PL_LAVATORY_COUNTER", "Lavatory (counter, oval)", "lav vanity counter sink undermount oval basin", |k| {
        k.ellipse((0.0, -10.0), 9.5, 7.5);
        k.ellipse((0.0, -10.5), 8.0, 6.0);
        k.circle((0.0, -1.5), 0.75);
        k.circle((0.0, -10.5), 0.75);
        k.attdef("TAG", "Fixture tag", "L-1", (11.0, -12.0), TAG_H);
    }),
    fixture("PL_SINK", "Sink (single bowl)", "kitchen sink single bowl", |k| {
        k.rect((-12.5, 0.0), (12.5, -22.0));
        k.rect((-10.5, -4.0), (10.5, -20.0));
        k.circle((0.0, -2.0), 0.75);
        k.circle((0.0, -12.0), 1.5);
        k.attdef("TAG", "Fixture tag", "S-1", (14.5, -14.0), TAG_H);
    }),
    fixture("PL_SINK_DOUBLE", "Sink (double bowl)", "kitchen sink double compartment bowl", |k| {
        k.rect((-16.5, 0.0), (16.5, -22.0));
        k.rect((-15.0, -4.0), (-1.0, -20.0));
        k.rect((1.0, -4.0), (15.0, -20.0));
        k.circle((0.0, -2.0), 0.75);
        k.circle((-8.0, -12.0), 1.5);
        k.circle((8.0, -12.0), 1.5);
        k.attdef("TAG", "Fixture tag", "S-2", (18.5, -14.0), TAG_H);
    }),
    fixture("PL_SERVICE_SINK", "Service sink (floor mop basin)", "mop sink basin janitor service floor", |k| {
        k.rect((-12.0, 0.0), (12.0, -24.0));
        k.rect((-10.0, -2.0), (10.0, -22.0));
        k.circle((0.0, -12.0), 1.5);
        k.attdef("TAG", "Fixture tag", "MS-1", (14.0, -14.0), TAG_H);
    }),
    fixture("PL_BATHTUB", "Bathtub (60 in)", "tub bath bathtub", |k| {
        k.rect((-30.0, 0.0), (30.0, -30.0));
        k.pline(
            &[
                (-24.0, -3.0, 0.0),
                (27.0, -3.0, 0.0),
                (27.0, -27.0, 0.0),
                (-24.0, -27.0, -0.414_213_562_373_095),
                (-27.0, -24.0, 0.0),
                (-27.0, -6.0, -0.414_213_562_373_095),
            ],
            true,
            0.0,
        );
        k.circle((-23.0, -15.0), 1.25);
        k.attdef("TAG", "Fixture tag", "T-1", (32.0, -18.0), TAG_H);
    }),
    fixture("PL_SHOWER", "Shower (36 in)", "shower stall receptor pan", |k| {
        k.rect((-18.0, 0.0), (18.0, -36.0));
        k.line((-18.0, 0.0), (18.0, -36.0));
        k.line((18.0, 0.0), (-18.0, -36.0));
        k.circle((0.0, -18.0), 1.5);
        k.attdef("TAG", "Fixture tag", "SH-1", (20.0, -20.0), TAG_H);
    }),
    fixture("PL_DRINKING_FOUNTAIN", "Drinking fountain / water cooler", "ewc fountain bottle filler drinking water cooler", |k| {
        k.rect((-9.0, 0.0), (9.0, -18.0));
        k.pline(&[(-6.5, -4.0, 0.0), (6.5, -4.0, 0.0), (6.5, -12.0, -1.0), (-6.5, -12.0, 0.0)], true, 0.0);
        k.circle((0.0, -9.0), 1.0);
        k.attdef("TAG", "Fixture tag", "DF-1", (11.0, -12.0), TAG_H);
    }),
    fixture("PL_WATER_HEATER", "Water heater (tank)", "wh water heater tank hot water", |k| {
        k.circle((0.0, -11.0), 11.0);
        k.circle((0.0, -11.0), 9.5);
        k.text((0.0, -11.0), 4.0, "WH");
        k.attdef("TAG", "Equipment tag", "WH-1", (13.0, -14.0), TAG_H);
    }),
    fixture("PL_FLOOR_DRAIN", "Floor drain", "fd drain floor grate", |k| {
        k.circle((0.0, 0.0), 4.0);
        k.circle((0.0, 0.0), 3.0);
        k.line((-3.0, 0.0), (3.0, 0.0));
        k.line((0.0, -3.0), (0.0, 3.0));
        k.attdef("TAG", "Fixture tag", "FD-1", (5.5, -1.5), TAG_H);
    }),
    fixture("PL_FLOOR_SINK", "Floor sink", "fs floor sink indirect waste receptor", |k| {
        k.rect((-6.0, 6.0), (6.0, -6.0));
        k.rect((-4.5, 4.5), (4.5, -4.5));
        k.circle((0.0, 0.0), 1.5);
        k.attdef("TAG", "Fixture tag", "FS-1", (7.5, -1.5), TAG_H);
    }),
    piping("PL_GATE_VALVE", "Gate valve", "valve gate shutoff isolation", |k| {
        bowtie(k);
        size_tag(k);
    }),
    piping("PL_BALL_VALVE", "Ball valve", "valve ball shutoff isolation", |k| {
        bowtie(k);
        k.circle((0.0, 0.0), 0.03);
        size_tag(k);
    }),
    piping("PL_GLOBE_VALVE", "Globe valve", "valve globe throttling", |k| {
        bowtie(k);
        k.dot((0.0, 0.0), 0.04);
        size_tag(k);
    }),
    piping("PL_BUTTERFLY_VALVE", "Butterfly valve", "valve butterfly", |k| {
        k.line((-0.06, 0.075), (-0.06, -0.075));
        k.line((0.06, 0.075), (0.06, -0.075));
        k.line((-0.06, -0.075), (0.06, 0.075));
        k.circle((0.0, 0.0), 0.02);
        size_tag(k);
    }),
    piping("PL_CHECK_VALVE", "Check valve", "valve check backflow one way swing", |k| {
        k.line((-0.125, 0.075), (-0.125, -0.075));
        k.line((0.125, 0.075), (0.125, -0.075));
        k.line((-0.125, -0.075), (0.125, 0.075));
        k.pline(&[(0.0, -0.06, 0.0), (0.125, 0.0, 0.0), (0.0, 0.06, 0.0)], false, 0.0);
        size_tag(k);
    }),
    piping("PL_PRESSURE_REDUCING_VALVE", "Pressure reducing valve", "prv valve pressure reducing regulator", |k| {
        bowtie(k);
        k.rect((-0.05, 0.075), (0.05, 0.16));
        k.text((0.0, -0.13), 0.06, "PRV");
        size_tag_at(k, 0.2);
    }),
    piping("PL_BACKFLOW_PREVENTER", "Backflow preventer (RPZ)", "bfp rpz backflow preventer reduced pressure double check", |k| {
        k.rect((-0.2, 0.09), (0.2, -0.09));
        k.line((-0.15, -0.07), (-0.03, 0.07));
        k.line((0.03, -0.07), (0.15, 0.07));
        k.text((0.0, -0.16), 0.06, "RPZ");
        size_tag(k);
    }),
    piping("PL_CLEANOUT_FLOOR", "Floor cleanout", "fco cleanout floor co", |k| {
        k.circle((0.0, 0.0), 0.08);
        k.line((-0.057, -0.057), (0.057, 0.057));
        k.text((0.0, -0.15), 0.06, "FCO");
        size_tag(k);
    }),
    piping("PL_CLEANOUT_WALL", "Wall cleanout", "wco cleanout wall co", |k| {
        k.line((-0.1, 0.0), (0.1, 0.0));
        k.pline(&[(0.08, 0.0, 1.0), (-0.08, 0.0, 0.0)], false, 0.0);
        k.text((0.0, -0.15), 0.06, "WCO");
        size_tag(k);
    }),
    piping("PL_UNION", "Union", "union fitting joint", |k| {
        k.line((-0.03, 0.07), (-0.03, -0.07));
        k.line((0.03, 0.07), (0.03, -0.07));
        k.line((0.0, 0.045), (0.0, -0.045));
        size_tag(k);
    }),
    piping("PL_CAP", "Pipe cap", "cap end plug", |k| {
        k.line((-0.1, 0.0), (0.0, 0.0));
        k.pline(&[(-0.02, 0.06, 0.0), (0.02, 0.06, 0.0), (0.02, -0.06, 0.0), (-0.02, -0.06, 0.0)], false, 0.0);
        size_tag(k);
    }),
    piping("PL_PIPE_UP", "Pipe turns up", "riser up rise turns up", |k| {
        k.line((-0.2, 0.0), (-0.06, 0.0));
        k.circle((0.0, 0.0), 0.06);
        k.dot((0.0, 0.0), 0.02);
        size_tag(k);
    }),
    piping("PL_PIPE_DOWN", "Pipe turns down", "riser down drop turns down", |k| {
        k.line((-0.2, 0.0), (0.0, 0.0));
        k.circle((0.0, 0.0), 0.06);
        k.line((-0.042, -0.042), (0.042, 0.042));
        k.line((-0.042, 0.042), (0.042, -0.042));
        size_tag(k);
    }),
    piping("PL_WATER_METER", "Water meter", "meter wm water service", |k| {
        k.line((-0.25, 0.0), (-0.1, 0.0));
        k.line((0.1, 0.0), (0.25, 0.0));
        k.circle((0.0, 0.0), 0.1);
        k.text((0.0, 0.0), 0.08, "M");
        size_tag(k);
    }),
    piping("PL_VENT_THRU_ROOF", "Vent through roof", "vtr vent roof stack", |k| {
        k.circle((0.0, 0.0), 0.06);
        k.circle((0.0, 0.0), 0.03);
        k.text((0.0, -0.13), 0.06, "VTR");
        size_tag(k);
    }),
    piping("PL_HAMMER_ARRESTOR", "Water hammer arrestor", "wha shock arrestor water hammer", |k| {
        k.line((0.0, 0.0), (0.0, 0.08));
        k.rect((-0.04, 0.08), (0.04, 0.2));
        k.text((0.0, -0.08), 0.06, "WHA");
        size_tag_at(k, 0.24);
    }),
    piping("PL_PUMP", "Pump (circulating)", "pump recirc circulator hot water return", |k| {
        k.circle((0.0, 0.0), 0.12);
        k.pline(&[(-0.06, -0.07, 0.0), (0.09, 0.0, 0.0), (-0.06, 0.07, 0.0)], true, 0.0);
        size_tag_at(k, 0.16);
    }),
    piping("PL_HOSE_BIBB", "Hose bibb / wall hydrant", "hb hose bibb wall hydrant sillcock", |k| {
        k.line((0.0, 0.0), (0.0, -0.1));
        k.rect((-0.06, -0.1), (0.06, -0.16));
        k.text((0.0, -0.24), 0.06, "HB");
        size_tag(k);
    }),
    piping("PL_FLOW_ARROW", "Flow direction arrow", "flow arrow direction slope", |k| {
        k.line((-0.25, 0.0), (0.1, 0.0));
        k.pline(&[(0.1, 0.04, 0.0), (0.25, 0.0, 0.0), (0.1, -0.04, 0.0)], true, 0.0);
        k.attdef("TAG", "Note (e.g. 1/4\" PER FT)", "", (-0.25, 0.05), NOTE_H);
    }),
];

/// The two-triangle valve body (0.25 wide, 0.15 high).
fn bowtie(k: &mut Sketch) {
    k.pline(&[(-0.125, 0.075, 0.0), (0.125, -0.075, 0.0), (0.125, 0.075, 0.0), (-0.125, -0.075, 0.0)], true, 0.0);
}

/// The pipe size, written above the symbol.
fn size_tag(k: &mut Sketch) {
    size_tag_at(k, 0.12);
}

/// The pipe size, its baseline at `y` (above a symbol taller than the valve body).
fn size_tag_at(k: &mut Sketch, y: f64) {
    k.attdef("SIZE", "Pipe size", "3/4\"", (-0.1, y), NOTE_H);
}
