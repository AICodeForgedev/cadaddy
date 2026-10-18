//! Electrical symbols, drawn for CADaddy from common plan conventions. Devices, power, low-voltage
//! and fire alarm symbols are at plotted size (inches on paper) with the insertion point at the
//! wall, the symbol out from it in −Y; light fixtures are at real size (inches), centred.
//! Smart attributes: devices carry CIRCUIT, PANEL and MOUNT_HT; fixtures carry TAG (fixture type)
//! and CIRCUIT; panels carry TAG (panel name) and VOLTAGE. Plans show the circuit and tags; PANEL,
//! MOUNT_HT and a panel's VOLTAGE are hidden (filled in, extracted, scheduled, not plotted).

use super::{Sketch, Symbol};

const DEVICES: &str = "Electrical devices";
const LIGHTING: &str = "Lighting";
const POWER: &str = "Power and distribution";
const LOW_VOLTAGE: &str = "Low voltage";
const FIRE_ALARM: &str = "Fire alarm";

/// Attribute text height on paper.
const NOTE_H: f64 = 0.07;
/// The device circle: diameter 3/16" on paper, centred 0.1" off the wall.
const R: f64 = 0.09375;
const C: (f64, f64) = (0.0, -0.1);

const fn device(name: &'static str, label: &'static str, category: &'static str, keywords: &'static str, draw: fn(&mut Sketch)) -> Symbol {
    Symbol { name, label, category, trade: "ELECTRICAL", annotation: true, keywords, draw }
}
const fn fixture(name: &'static str, label: &'static str, keywords: &'static str, draw: fn(&mut Sketch)) -> Symbol {
    Symbol { name, label, category: LIGHTING, trade: "ELECTRICAL", annotation: false, keywords, draw }
}

pub(super) const SYMBOLS: &[Symbol] = &[
    // ---------- receptacles ----------
    device("EL_RECEPTACLE_DUPLEX", "Duplex receptacle", DEVICES, "receptacle outlet duplex plug 120v power", |k| {
        recept(k);
        circuit_tags(k, "18\"");
    }),
    device("EL_RECEPTACLE_QUAD", "Quadruplex receptacle", DEVICES, "receptacle outlet quad fourplex double duplex", |k| {
        recept(k);
        k.line((-0.03, C.1 - R), (-0.03, C.1 - R - 0.05));
        k.line((0.03, C.1 - R), (0.03, C.1 - R - 0.05));
        circuit_tags(k, "18\"");
    }),
    device("EL_RECEPTACLE_GFCI", "GFCI duplex receptacle", DEVICES, "receptacle outlet gfci gfi ground fault", |k| {
        recept(k);
        k.text((0.0, C.1 - R - 0.07), 0.05, "GFI");
        circuit_tags(k, "18\"");
    }),
    device("EL_RECEPTACLE_WP", "Weatherproof receptacle", DEVICES, "receptacle outlet weatherproof wp exterior outdoor", |k| {
        recept(k);
        k.text((0.0, C.1 - R - 0.07), 0.05, "WP");
        circuit_tags(k, "18\"");
    }),
    device("EL_RECEPTACLE_DEDICATED", "Dedicated receptacle", DEVICES, "receptacle outlet dedicated single isolated", |k| {
        k.line((0.0, 0.0), (0.0, C.1 + R));
        k.circle(C, R);
        k.line((0.0, C.1 + R), (0.0, C.1 - R));
        k.dot(C, 0.03);
        circuit_tags(k, "18\"");
    }),
    device("EL_RECEPTACLE_240V", "Special-purpose receptacle (208/240 V)", DEVICES, "receptacle outlet 240v 208v range dryer special purpose", |k| {
        k.line((0.0, 0.0), (0.0, C.1 + R));
        k.pline(&[(0.0, C.1 + R, 0.0), (R, C.1 - R * 0.6, 0.0), (-R, C.1 - R * 0.6, 0.0)], true, 0.0);
        k.line((-0.03, C.1 - 0.02), (0.03, C.1 - 0.02));
        circuit_tags(k, "18\"");
    }),
    device("EL_FLOOR_BOX", "Floor box (duplex)", DEVICES, "floor box receptacle outlet floor", |k| {
        k.rect((-0.12, 0.12), (0.12, -0.12));
        k.circle((0.0, 0.0), 0.075);
        k.line((-0.025, 0.05), (-0.025, -0.05));
        k.line((0.025, 0.05), (0.025, -0.05));
        k.attdef("CIRCUIT", "Circuit", "1", (0.15, 0.02), NOTE_H);
        k.attdef_hidden("PANEL", "Panel", "LP-1", (0.15, -0.08), NOTE_H);
    }),
    // ---------- switches ----------
    device("EL_SWITCH", "Single-pole switch", DEVICES, "switch single pole light switch s", |k| {
        switch(k, "S");
    }),
    device("EL_SWITCH_3WAY", "3-way switch", DEVICES, "switch three way 3way s3", |k| {
        switch(k, "S3");
    }),
    device("EL_SWITCH_4WAY", "4-way switch", DEVICES, "switch four way 4way s4", |k| {
        switch(k, "S4");
    }),
    device("EL_SWITCH_DIMMER", "Dimmer switch", DEVICES, "switch dimmer sd", |k| {
        switch(k, "SD");
    }),
    device("EL_OCCUPANCY_SENSOR", "Occupancy sensor (ceiling)", DEVICES, "occupancy sensor motion vacancy os ceiling", |k| {
        k.circle((0.0, 0.0), 0.11);
        k.text((0.0, 0.0), 0.07, "OS");
        k.attdef("CIRCUIT", "Circuit", "1", (0.14, 0.02), NOTE_H);
        k.attdef_hidden("PANEL", "Panel", "LP-1", (0.14, -0.08), NOTE_H);
    }),
    // ---------- lighting (real size, inches) ----------
    fixture("EL_LIGHT_2X4", "Troffer 2'x4'", "light fixture troffer 2x4 lay-in recessed lighting luminaire", |k| {
        k.rect((-12.0, 24.0), (12.0, -24.0));
        k.line((-12.0, 24.0), (12.0, -24.0));
        fixture_tags(k, 14.0);
    }),
    fixture("EL_LIGHT_2X2", "Troffer 2'x2'", "light fixture troffer 2x2 lay-in recessed lighting luminaire", |k| {
        k.rect((-12.0, 12.0), (12.0, -12.0));
        k.line((-12.0, 12.0), (12.0, -12.0));
        fixture_tags(k, 14.0);
    }),
    fixture("EL_LIGHT_DOWNLIGHT", "Downlight (6\")", "light fixture downlight can recessed pot lighting luminaire", |k| {
        k.circle((0.0, 0.0), 3.5);
        k.circle((0.0, 0.0), 2.5);
        fixture_tags(k, 5.0);
    }),
    fixture("EL_LIGHT_SCONCE", "Wall sconce", "light fixture wall sconce wall mount lighting", |k| {
        k.pline(&[(-5.0, 0.0, 0.0), (5.0, 0.0, -1.0)], true, 0.0);
        fixture_tags(k, 7.0);
    }),
    fixture("EL_LIGHT_STRIP", "Strip light (4')", "light fixture strip wrap linear industrial lighting", |k| {
        k.rect((-2.5, 24.0), (2.5, -24.0));
        k.line((0.0, 24.0), (0.0, -24.0));
        fixture_tags(k, 5.0);
    }),
    fixture("EL_LIGHT_EXIT", "Exit sign", "exit sign egress emergency lighting", |k| {
        k.rect((-7.0, 4.0), (7.0, -4.0));
        k.text((0.0, 0.0), 3.5, "EXIT");
        k.pline(&[(-1.0, -6.0, 0.0), (1.0, -6.0, 0.0), (0.0, -8.0, 0.0)], true, 0.0);
        fixture_tags(k, 9.0);
    }),
    fixture("EL_LIGHT_EMERGENCY", "Emergency light (twin head)", "emergency light battery unit bug eyes egress lighting", |k| {
        k.rect((-6.0, 2.0), (6.0, -2.0));
        k.circle((-6.0, -4.0), 2.0);
        k.circle((6.0, -4.0), 2.0);
        fixture_tags(k, 10.0);
    }),
    // ---------- power and distribution ----------
    device("EL_PANEL", "Panelboard", POWER, "panel panelboard lighting panel load center distribution", |k| {
        k.rect((-0.25, 0.0), (0.25, -0.1));
        k.pline(&[(-0.25, 0.0, 0.0), (0.25, 0.0, 0.0), (0.25, -0.1, 0.0)], true, 0.0);
        k.attdef("TAG", "Panel name", "LP-1", (-0.25, -0.25), 0.09);
        k.attdef_hidden("VOLTAGE", "Voltage", "120/208V 3PH", (-0.25, -0.38), NOTE_H);
    }),
    device("EL_DISCONNECT", "Disconnect switch", POWER, "disconnect safety switch fused non-fused", |k| {
        k.rect((-0.1, 0.0), (0.1, -0.2));
        k.line((-0.1, 0.0), (0.1, -0.2));
        k.attdef("TAG", "Size and fusing", "30A/2P", (0.13, -0.08), NOTE_H);
        k.attdef("CIRCUIT", "Circuit", "1", (0.13, -0.18), NOTE_H);
    }),
    device("EL_TRANSFORMER", "Transformer", POWER, "transformer xfmr dry type step down", |k| {
        k.rect((-0.15, 0.0), (0.15, -0.2));
        k.pline(&[(-0.08, -0.04, 0.0), (-0.08, -0.16, 0.0)], false, 0.0);
        k.pline(&[(-0.04, -0.04, 0.5), (-0.04, -0.1, 0.5), (-0.04, -0.16, 0.0)], false, 0.0);
        k.pline(&[(0.04, -0.04, -0.5), (0.04, -0.1, -0.5), (0.04, -0.16, 0.0)], false, 0.0);
        k.attdef("TAG", "Transformer tag", "T-1", (0.18, -0.08), NOTE_H);
        k.attdef("VOLTAGE", "Rating", "45KVA", (0.18, -0.18), NOTE_H);
    }),
    device("EL_JUNCTION_BOX", "Junction box", POWER, "junction box j-box j pull box", |k| {
        k.circle(C, R);
        k.text(C, 0.08, "J");
        circuit_tags(k, "");
    }),
    device("EL_MOTOR", "Motor connection", POWER, "motor connection equipment hp mechanical", |k| {
        k.circle(C, R);
        k.text(C, 0.08, "M");
        k.attdef("TAG", "Equipment tag", "EF-1", (0.13, -0.03), NOTE_H);
        k.attdef("CIRCUIT", "Circuit", "1", (0.13, -0.13), NOTE_H);
        k.attdef_hidden("PANEL", "Panel", "LP-1", (0.13, -0.23), NOTE_H);
    }),
    device("EL_HOME_RUN", "Home run arrow", POWER, "home run arrow circuit to panel", |k| {
        k.line((0.0, 0.0), (0.35, 0.0));
        k.pline(&[(0.35, 0.04, 0.0), (0.45, 0.0, 0.0), (0.35, -0.04, 0.0)], true, 0.0);
        k.attdef("CIRCUIT", "Circuits", "LP-1-1,3", (0.0, 0.05), NOTE_H);
    }),
    // ---------- low voltage ----------
    device("EL_DATA", "Data outlet", LOW_VOLTAGE, "data outlet network cat6 ethernet communication", |k| {
        k.line((0.0, 0.0), (0.0, -0.03));
        k.pline(&[(-0.09, -0.03, 0.0), (0.09, -0.03, 0.0), (0.0, -0.18, 0.0)], true, 0.0);
        k.attdef("TAG", "Outlet tag", "D", (0.12, -0.08), NOTE_H);
        k.attdef_hidden("MOUNT_HT", "Mounting height", "18\"", (0.12, -0.18), NOTE_H);
    }),
    device("EL_PHONE", "Telephone outlet", LOW_VOLTAGE, "telephone phone voice outlet communication", |k| {
        k.line((0.0, 0.0), (0.0, -0.03));
        k.pline(&[(-0.09, -0.03, 0.0), (0.09, -0.03, 0.0), (0.0, -0.18, 0.0)], true, 0.0);
        k.dot((0.0, -0.08), 0.03);
        k.attdef("TAG", "Outlet tag", "T", (0.12, -0.08), NOTE_H);
        k.attdef_hidden("MOUNT_HT", "Mounting height", "18\"", (0.12, -0.18), NOTE_H);
    }),
    device("EL_TV", "TV / CATV outlet", LOW_VOLTAGE, "tv catv coax television cable outlet", |k| {
        k.line((0.0, 0.0), (0.0, -0.03));
        k.rect((-0.09, -0.03), (0.09, -0.18));
        k.text((0.0, -0.105), 0.06, "TV");
        k.attdef_hidden("MOUNT_HT", "Mounting height", "18\"", (0.12, -0.18), NOTE_H);
    }),
    // ---------- fire alarm ----------
    device("EL_FA_SMOKE", "Smoke detector", FIRE_ALARM, "fire alarm smoke detector sd ceiling", |k| {
        k.circle((0.0, 0.0), 0.11);
        k.text((0.0, 0.0), 0.07, "SD");
        k.attdef("TAG", "Device address", "", (0.14, -0.03), NOTE_H);
    }),
    device("EL_FA_HEAT", "Heat detector", FIRE_ALARM, "fire alarm heat detector hd thermal", |k| {
        k.circle((0.0, 0.0), 0.11);
        k.text((0.0, 0.0), 0.07, "HD");
        k.attdef("TAG", "Device address", "", (0.14, -0.03), NOTE_H);
    }),
    device("EL_FA_HORN_STROBE", "Horn/strobe", FIRE_ALARM, "fire alarm horn strobe notification nac av visual", |k| {
        k.line((0.0, 0.0), (0.0, -0.03));
        k.rect((-0.1, -0.03), (0.1, -0.23));
        k.text((0.0, -0.13), 0.07, "HS");
        k.attdef("TAG", "Candela", "75cd", (0.13, -0.13), NOTE_H);
        k.attdef_hidden("MOUNT_HT", "Mounting height", "80\"", (0.13, -0.23), NOTE_H);
    }),
    device("EL_FA_PULL", "Manual pull station", FIRE_ALARM, "fire alarm pull station manual f", |k| {
        k.line((0.0, 0.0), (0.0, -0.03));
        k.rect((-0.1, -0.03), (0.1, -0.23));
        k.text((0.0, -0.13), 0.08, "F");
        k.attdef_hidden("MOUNT_HT", "Mounting height", "48\"", (0.13, -0.23), NOTE_H);
    }),
    device("EL_FA_PANEL", "Fire alarm control panel", FIRE_ALARM, "fire alarm control panel facp", |k| {
        k.rect((-0.25, 0.0), (0.25, -0.15));
        k.text((0.0, -0.075), 0.07, "FACP");
        k.attdef("TAG", "Panel name", "", (-0.25, -0.25), NOTE_H);
    }),
];

/// The duplex receptacle: a stub from the wall, the circle and two lines across it.
fn recept(k: &mut Sketch) {
    k.line((0.0, 0.0), (0.0, C.1 + R));
    k.circle(C, R);
    k.line((-0.03, C.1 + R * 0.8), (-0.03, C.1 - R));
    k.line((0.03, C.1 + R * 0.8), (0.03, C.1 - R));
}

/// A wall switch: the letters out from the wall.
fn switch(k: &mut Sketch, letters: &str) {
    k.line((0.0, 0.0), (0.0, -0.04));
    k.text((0.0, -0.11), 0.09, letters);
    k.attdef("CIRCUIT", "Circuit / switch leg", "a", (0.12, -0.08), NOTE_H);
    k.attdef_hidden("MOUNT_HT", "Mounting height", "48\"", (0.12, -0.18), NOTE_H);
}

/// CIRCUIT, PANEL and (when given) MOUNT_HT beside a device.
fn circuit_tags(k: &mut Sketch, mount: &str) {
    k.attdef("CIRCUIT", "Circuit", "1", (0.13, -0.04), NOTE_H);
    k.attdef_hidden("PANEL", "Panel", "LP-1", (0.13, -0.16), NOTE_H);
    if !mount.is_empty() {
        k.attdef_hidden("MOUNT_HT", "Mounting height", mount, (0.13, -0.28), NOTE_H);
    }
}

/// A fixture's type tag and circuit beside it (real size, inches).
fn fixture_tags(k: &mut Sketch, x: f64) {
    k.attdef("TAG", "Fixture type", "A", (x, 0.5), 4.0);
    k.attdef("CIRCUIT", "Circuit", "1", (x, -5.0), 3.0);
}
