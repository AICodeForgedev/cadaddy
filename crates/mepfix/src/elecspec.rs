//! What the electrical plans and schedules say about each element beyond its symbol (Q299):
//! device subscripts (GFCI, WR, USB, IG, DED, TV, CH), the circuit printed beside each
//! receptacle (`P1-5`), each breaker's type in the panel schedules (GFCI, AFCI, DF, GFEP),
//! the conductors noted under each homerun (`2#12, 1#12G`), and planted problems: a luminaire
//! whose type isn't in the schedule, one left untagged, a GFCI receptacle on a GFCI breaker's
//! circuit (double protection), and a wet-location receptacle with no protection at all.
//!
//! From a random stream of its own, after everything else: the rest of a set stays as it was.

use serde::{Deserialize, Serialize};

use crate::rng::Rng;
use crate::truth::Truth;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElecSpec {
    /// Per device: its subscripts and the circuit printed beside it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<DeviceNote>,
    /// Breakers with a type (the rest are plain): panel id, circuit number, type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breakers: Vec<BreakerType>,
    /// The conductors noted under each homerun: circuit id, the note.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conductors: Vec<(String, String)>,
    /// Luminaire types drawn on the plans but left out of the schedule (planted).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unscheduled: Vec<String>,
    /// Luminaires drawn without their type tag (planted): device ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub untagged: Vec<String>,
}

impl ElecSpec {
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.breakers.is_empty() && self.conductors.is_empty() && self.unscheduled.is_empty() && self.untagged.is_empty()
    }

    pub fn note(&self, device: &str) -> Option<&DeviceNote> {
        self.notes.iter().find(|n| n.device == device)
    }

    pub fn breaker(&self, panel: &str, circuit: &str) -> Option<&str> {
        self.breakers.iter().find(|b| b.panel == panel && b.circuit == circuit).map(|b| b.kind.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceNote {
    pub device: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscripts: Vec<String>,
    /// `P1-5`: panel tag, circuit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub circuit_ref: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BreakerType {
    pub panel: String,
    pub circuit: String,
    /// `GFCI`, `AFCI`, `DF` (dual function), `GFEP` (equipment ground-fault).
    pub kind: String,
}

/// Rooms whose receptacles must be ground-fault protected (wet or kitchen-like).
pub fn wet_room(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    ["RESTROOM", "TOILET", "BREAK", "KITCHEN", "JANITOR", "LAUNDRY", "SHOWER", "BATH", "POOL"].iter().any(|w| u.contains(w))
}

/// Whether a device is in a wet location, as the documents put it: the room its circuit serves
/// in the panel schedule (`RECEP/DATA - BREAK ROOM`), else the room it's drawn in.
pub fn device_wet(t: &Truth, d: &crate::truth::ElecDevice) -> bool {
    let served = t
        .elec_circuits
        .iter()
        .find(|c| c.id == d.circuit)
        .and_then(|c| t.elec_panels.iter().find(|p| p.id == c.panel).and_then(|p| p.schedule.iter().find(|r| r.circuit == c.number)))
        .map(|r| r.description.clone());
    match served {
        Some(desc) => wet_room(&desc),
        None => wet_room(room_name(t, &d.room)),
    }
}

/// The room name of a device (by its room number or id).
pub fn room_name<'a>(t: &'a Truth, room: &str) -> &'a str {
    t.building.rooms.iter().find(|r| r.number == room || r.id == room).map_or("", |r| r.name.as_str())
}

/// The conductors a breaker feeds: `2#12, 1#12G` for 20 A single pole, `#10` at 30 A, a
/// conductor per pole plus the neutral, and the ground.
pub fn conductors_for(amps: &str, poles: &str) -> String {
    let size = match amps.trim().parse::<u32>().unwrap_or(20) {
        0..=15 => 14,
        16..=20 => 12,
        21..=30 => 10,
        31..=40 => 8,
        _ => 6,
    };
    let hots = poles.trim().parse::<u32>().unwrap_or(1).clamp(1, 3);
    let wires = if hots == 1 { 2 } else { hots };
    format!("{wires}#{size}, 1#{size}G")
}

/// Add the notes, breaker types, conductors and planted problems to a set's truth.
pub fn design(t: &mut Truth) {
    let mut r = Rng::new(t.seed ^ 0x4543_5350_4543).fork(9);
    let mut spec = ElecSpec::default();
    let panel_tag = |t: &Truth, id: &str| t.elec_panels.iter().find(|p| p.id == id).map_or_else(|| id.to_string(), |p| p.tag.clone());
    // Each circuit with a wet room's receptacles is protected at the breaker or at the device.
    let mut device_protected: Vec<String> = Vec::new();
    for c in &t.elec_circuits {
        let wet: Vec<&crate::truth::ElecDevice> =
            t.elec_devices.iter().filter(|d| d.kind == "receptacle" && c.devices.contains(&d.id) && device_wet(t, d)).collect();
        if wet.is_empty() {
            continue;
        }
        if r.chance(0.5) {
            spec.breakers.push(BreakerType { panel: c.panel.clone(), circuit: c.number.clone(), kind: "GFCI".into() });
        } else {
            device_protected.extend(wet.iter().map(|d| d.id.clone()));
        }
    }
    // The other subscripts, on ordinary receptacles.
    let receptacles: Vec<(String, String, String)> =
        t.elec_devices.iter().filter(|d| d.kind == "receptacle").map(|d| (d.id.clone(), d.panel.clone(), d.circuit.clone())).collect();
    let mut extra_done = [false; 3];
    for (id, panel, circuit) in &receptacles {
        let mut subs: Vec<String> = Vec::new();
        if device_protected.contains(id) {
            subs.push("GFCI".into());
        } else if r.chance(0.12) {
            subs.push("USB".into());
        } else if !extra_done[0] && r.chance(0.3) {
            subs.push("DED".into());
            extra_done[0] = true;
        } else if !extra_done[1] && r.chance(0.3) {
            subs.push("IG".into());
            extra_done[1] = true;
        } else if !extra_done[2] && r.chance(0.3) {
            subs.push("TV".into());
            extra_done[2] = true;
        }
        let num = t.elec_circuits.iter().find(|c| c.id == *circuit).map_or_else(|| circuit.clone(), |c| c.number.clone());
        let circuit_ref = if circuit.is_empty() { String::new() } else { format!("{}-{num}", panel_tag(t, panel)) };
        spec.notes.push(DeviceNote { device: id.clone(), subscripts: subs, circuit_ref });
    }
    // Planted: a GFCI receptacle on a GFCI breaker's circuit (double protection) …
    let on_breaker = |spec: &ElecSpec, d: &crate::truth::ElecDevice| {
        t.elec_circuits.iter().find(|c| c.id == d.circuit).is_some_and(|c| spec.breaker(&c.panel, &c.number) == Some("GFCI"))
    };
    if let Some(d) = t.elec_devices.iter().find(|d| d.kind == "receptacle" && device_wet(t, d) && on_breaker(&spec, d))
        && let Some(n) = spec.notes.iter_mut().find(|n| n.device == d.id)
    {
        n.subscripts = vec!["GFCI".into()];
    }
    // … and a wet receptacle with no protection (its GFCI left off a device-protected circuit).
    if let Some(n) = spec.notes.iter_mut().find(|n| device_protected.contains(&n.device)) {
        n.subscripts.clear();
    }
    // A clock hanger and a weatherproof receptacle where the plan has them: in a corridor.
    if let Some(d) = t.elec_devices.iter().find(|d| d.kind == "receptacle" && room_name(t, &d.room).to_ascii_uppercase().contains("CORRIDOR"))
        && let Some(n) = spec.notes.iter_mut().find(|n| n.device == d.id && n.subscripts.is_empty())
    {
        n.subscripts = vec!["CH".into()];
    }
    // The conductors under each homerun.
    for c in &t.elec_circuits {
        let row = t.elec_panels.iter().find(|p| p.id == c.panel).and_then(|p| p.schedule.iter().find(|s| s.circuit == c.number));
        let note = row.map_or_else(|| conductors_for("20", "1"), |s| conductors_for(&s.breaker_amps, &s.poles));
        spec.conductors.push((c.id.clone(), note));
    }
    // Planted luminaires: one of a type the schedule doesn't list, one with no tag.
    let lights: Vec<usize> =
        t.elec_devices.iter().enumerate().filter(|(_, d)| d.kind == "light_fixture" && !d.type_tag.is_empty()).map(|(i, _)| i).collect();
    if lights.len() >= 2 {
        let a = lights.get(r.below(lights.len())).copied().unwrap_or(0);
        let b = lights.iter().copied().find(|i| *i != a).unwrap_or(a);
        if let Some(d) = t.elec_devices.get_mut(a) {
            d.type_tag = "Z".into();
            d.tag = "Z".into();
            spec.unscheduled.push("Z".into());
        }
        if let Some(d) = t.elec_devices.get(b) {
            spec.untagged.push(d.id.clone());
        }
    }
    t.elec_spec = spec;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conductors_follow_the_breaker() {
        assert_eq!(conductors_for("20", "1"), "2#12, 1#12G");
        assert_eq!(conductors_for("30", "2"), "2#10, 1#10G");
        assert_eq!(conductors_for("40", "3"), "3#8, 1#8G");
        assert_eq!(conductors_for("x", ""), "2#12, 1#12G");
        assert!(wet_room("BREAK ROOM") && wet_room("Restroom") && !wet_room("OFFICE"));
    }
}
