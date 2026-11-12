//! The splices in each box (Q6349): no drawing shows them, the field makes them in every box a
//! circuit continues through or ends in. From the conductors that enter a box (each run end's,
//! by circuit and by what each conductor is), the devices in it and the box's material, each
//! splice the trade makes there and the connector its conductors take (rules `splices` in
//! `mepatoms.json`).

use std::collections::BTreeMap;

use serde::Deserialize;

/// Most conductors one splice is worked out for (hostile specs).
const MAX_IN_SPLICE: usize = 200;
/// Most conductors one box is worked out for.
const MAX_IN_BOX: usize = 2_000;

/// One connector of the combination table: the most conductors of each size it is listed for.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ConnectorRow {
    pub size: String,
    /// AWG (`12`) to how many of that size alone it takes.
    pub max: BTreeMap<String, u32>,
}

/// How a wiring method's boxes are made up.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct MethodRule {
    /// `pigtail`: where a circuit continues through a device's box its conductors are spliced
    /// with a pigtail to the device; `device`: a receptacle's second pair of terminals carries
    /// it on.
    pub feed_through: String,
}

/// The rules data's `splices`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SpliceRules {
    pub item: String,
    pub large_item: String,
    pub connectors: Vec<ConnectorRow>,
    pub connectors_source: String,
    pub lead_size: String,
    pub lead_source: String,
    pub terminal_families: Vec<String>,
    pub switch_families: Vec<String>,
    pub lead_families: Vec<String>,
    pub bond_materials: Vec<String>,
    pub bond_source: String,
    pub methods: BTreeMap<String, MethodRule>,
    pub methods_source: String,
    pub source: String,
}

impl SpliceRules {
    fn method(&self, method: &str) -> Option<&MethodRule> {
        self.methods.get(method).or_else(|| self.methods.get("raceway"))
    }

    /// Whether a box of `material` takes a bonding jumper.
    pub(crate) fn bonds(&self, material: &str) -> bool {
        let m = material.trim().to_ascii_lowercase();
        !m.is_empty() && self.bond_materials.iter().any(|b| m.contains(&b.to_ascii_lowercase()))
    }

    fn is(&self, list: &[String], family: &str) -> bool {
        list.iter().any(|f| f.eq_ignore_ascii_case(family))
    }

    /// Whether a device of `family` is wired to the branch circuit (lands conductors).
    pub(crate) fn wired(&self, family: &str) -> bool {
        self.is(&self.terminal_families, family) || self.is(&self.switch_families, family) || self.is(&self.lead_families, family)
    }
}

/// What a conductor in a box is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Role {
    Hot,
    Neutral,
    /// A switch leg's switched conductor or a traveler.
    Switched,
    Ground,
}

impl Role {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Role::Hot => "hot",
            Role::Neutral => "neutral",
            Role::Switched => "switch leg",
            Role::Ground => "ground",
        }
    }
}

/// One conductor entering a box: its circuit (`LP1-5`; a shared neutral's `LP1-1,3,5`), what it
/// is and its size (AWG, `12`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Conductor {
    pub circuit: String,
    pub role: Role,
    pub size: String,
}

/// A circuit label as one key: its panel upper case and its numbers (`lp1-1, 3` → `LP1-1,3`).
fn key(circuit: &str) -> (String, Vec<String>) {
    match super::super::printcond::parse_circuit_list(circuit) {
        Some((p, n)) => (p, n.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).take(64).collect()),
        None => (circuit.trim().to_ascii_uppercase(), Vec::new()),
    }
}

fn label(panel: &str, nums: &[String]) -> String {
    if nums.is_empty() { panel.to_string() } else { format!("{panel}-{}", nums.join(",")) }
}

/// An AWG size as the table keys it (`#12` → `12`).
pub(crate) fn awg(size: &str) -> String {
    size.trim().trim_start_matches('#').trim().to_string()
}

/// The conductors of one run end: `insulated` of `size` and `grounds` of `ground_size`, on
/// `circuit`. The first of each circuit number's are its hots; a neutral each after them (one
/// shared by all when fewer, a multiwire circuit); any more are its switch leg (a switch loop's
/// switched conductor and travelers, NEC 404.2(C) bringing the neutral with them).
pub(crate) fn entry_conductors(circuit: &str, insulated: u32, grounds: u32, size: &str, ground_size: &str) -> Vec<Conductor> {
    let (panel, nums) = key(circuit);
    let insulated = insulated.min(64) as usize;
    let n = nums.len().max(1);
    let one = |i: usize| match nums.get(i % n) {
        Some(x) => label(&panel, std::slice::from_ref(x)),
        None => label(&panel, &[]),
    };
    let size = awg(size);
    let mut out = Vec::new();
    let hots = n.min(insulated);
    for i in 0..hots {
        out.push(Conductor { circuit: one(i), role: Role::Hot, size: size.clone() });
    }
    let rest = insulated - hots;
    if rest > 0 && rest < n {
        // A multiwire circuit's neutral(s), shared by its circuits.
        for _ in 0..rest {
            out.push(Conductor { circuit: label(&panel, &nums), role: Role::Neutral, size: size.clone() });
        }
    } else {
        for i in 0..rest {
            let role = if i < n { Role::Neutral } else { Role::Switched };
            out.push(Conductor { circuit: one(if i < n { i } else { 0 }), role, size: size.clone() });
        }
    }
    let gs = awg(ground_size);
    for _ in 0..grounds.min(16) {
        out.push(Conductor { circuit: label(&panel, &nums), role: Role::Ground, size: gs.clone() });
    }
    out
}

/// A device in a box: its family (`receptacle`, `switch`, `luminaire`…), its circuit, and the
/// pigtail size its terminals take when its circuit's conductor is larger (Q6243).
#[derive(Clone, Debug, Default)]
pub(crate) struct DeviceIn {
    pub family: String,
    pub circuit: String,
    pub pigtail: Option<String>,
}

/// One connector the box takes: what it is bought as, its size, the splice it makes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Connector {
    pub item: String,
    pub size: String,
    pub role: Role,
    pub circuit: String,
    /// The conductors it joins (AWG), a jumper to the next connector of a split bundle among them.
    pub conductors: Vec<String>,
    /// Why: what enters and what is pigtailed.
    pub why: String,
    /// One conductor capped (a switch box's unused neutral, a dead end).
    pub cap: bool,
}

/// Whether a group's circuit `group` is the device's `device` (or a shared neutral's list holds it).
fn serves(group: &str, device: &str) -> bool {
    if group == device {
        return true;
    }
    let ((pg, ng), (pd, nd)) = (key(group), key(device));
    pg == pd && !nd.is_empty() && nd.iter().all(|n| ng.contains(n))
}

/// Order of AWG sizes, smaller conductors first (`18` before `12`); larger than AWG last.
fn gauge(size: &str) -> i64 {
    match size.parse::<i64>() {
        Ok(n) if n < 100 => -n,
        _ => 1_000,
    }
}

fn fits(row: &ConnectorRow, sizes: &[String]) -> bool {
    let mut share = 0.0_f64;
    let mut by: BTreeMap<&str, u32> = BTreeMap::new();
    for s in sizes {
        *by.entry(s.as_str()).or_default() += 1;
    }
    for (s, n) in by {
        match row.max.get(s).copied().filter(|m| *m > 0) {
            Some(m) => share += f64::from(n) / f64::from(m),
            None => return false,
        }
    }
    share <= 1.0 + 1e-9
}

/// The connectors a splice of `sizes` takes: the smallest listed for them; past the largest
/// twist-on, the bundle in two or more joined by a jumper; conductors no twist-on takes, an
/// insulated multi-tap connector. Each with the conductors it joins.
pub(crate) fn connectors_for(sizes: &[String], r: &SpliceRules) -> Vec<(String, String, Vec<String>)> {
    let mut sizes: Vec<String> = sizes.iter().take(MAX_IN_SPLICE).cloned().collect();
    sizes.sort_by_key(|s| std::cmp::Reverse(gauge(s)));
    if let Some(row) = r.connectors.iter().find(|row| fits(row, &sizes)) {
        return vec![(r.item.clone(), row.size.clone(), sizes)];
    }
    let multi = |sizes: Vec<String>| {
        let big = sizes.first().cloned().unwrap_or_default();
        vec![(r.large_item.clone(), format!("{} port, #{big} largest", sizes.len()), sizes)]
    };
    let Some(largest) = r.connectors.last() else { return multi(sizes) };
    let jumper = sizes.first().cloned().unwrap_or_default();
    let with = |b: &[String], more: &[&String]| -> Vec<String> { b.iter().cloned().chain(more.iter().map(|s| (*s).clone())).collect() };
    if !fits(largest, &[jumper.clone(), jumper.clone()]) || sizes.iter().any(|s| !fits(largest, &[jumper.clone(), s.clone()])) {
        return multi(sizes);
    }
    let mut out = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let last = sizes.len().saturating_sub(1);
    for (i, s) in sizes.iter().enumerate() {
        let room = if i == last { fits(largest, &with(&cur, &[s])) } else { fits(largest, &with(&cur, &[s, &jumper])) };
        if room || cur.is_empty() {
            cur.push(s.clone());
            continue;
        }
        cur.push(jumper.clone());
        let row = r.connectors.iter().find(|row| fits(row, &cur)).unwrap_or(largest);
        out.push((r.item.clone(), row.size.clone(), std::mem::take(&mut cur)));
        cur = vec![jumper.clone(), s.clone()];
    }
    if !cur.is_empty() {
        let row = r.connectors.iter().find(|row| fits(row, &cur)).unwrap_or(largest);
        out.push((r.item.clone(), row.size.clone(), cur));
    }
    out
}

/// Every splice in one box: from the conductors of each run end that enters it (`entries`), the
/// devices in it, whether it is a metal box (a bonding jumper) and its wiring method.
pub(crate) fn box_splices(entries: &[Vec<Conductor>], devices: &[DeviceIn], metal: bool, method: &str, r: &SpliceRules) -> Vec<Connector> {
    let all: Vec<&Conductor> = entries.iter().flatten().take(MAX_IN_BOX).collect();
    if all.is_empty() {
        return Vec::new();
    }
    let mut groups: BTreeMap<(String, Role), Vec<String>> = BTreeMap::new();
    let mut grounds: Vec<String> = Vec::new();
    for c in &all {
        if c.role == Role::Ground {
            grounds.push(c.size.clone());
        } else {
            groups.entry((c.circuit.clone(), c.role)).or_default().push(c.size.clone());
        }
    }
    // The circuit each device is on: its own, else the box's one circuit.
    let hot_circuits: Vec<&String> = {
        let mut v: Vec<&String> = groups.keys().filter(|(_, r)| *r == Role::Hot).map(|(c, _)| c).collect();
        v.dedup();
        v
    };
    let dev_circuit = |d: &DeviceIn| -> String {
        let (p, n) = key(&d.circuit);
        let own = label(&p, &n);
        if !d.circuit.trim().is_empty() && groups.keys().any(|(c, _)| serves(c, &own)) {
            own
        } else if let [only] = hot_circuits.as_slice() {
            (*only).clone()
        } else {
            own
        }
    };
    let devs: Vec<(String, &DeviceIn)> = devices.iter().filter(|d| r.wired(&d.family)).map(|d| (dev_circuit(d), d)).collect();
    let feed = r.method(method).map(|m| m.feed_through.trim().to_ascii_lowercase()).unwrap_or_default();
    let mut out = Vec::new();
    let push = |out: &mut Vec<Connector>, role: Role, circuit: &str, sizes: Vec<String>, why: String, cap: bool| {
        for (item, size, conductors) in connectors_for(&sizes, r) {
            out.push(Connector { item, size, role, circuit: circuit.to_string(), conductors, why: why.clone(), cap });
        }
    };
    for ((circuit, role), sizes) in &groups {
        let k = sizes.len();
        let switched_here = groups.contains_key(&(circuit.clone(), Role::Switched));
        let (mut terminals, mut leads, mut pig) = (0usize, 0usize, None::<String>);
        let mut through_ok = true;
        for (dc, d) in &devs {
            if !serves(circuit, dc) {
                continue;
            }
            let lands = if r.is(&r.terminal_families, &d.family) {
                matches!(role, Role::Hot | Role::Neutral)
            } else if r.is(&r.switch_families, &d.family) {
                matches!(role, Role::Hot | Role::Switched)
            } else {
                // A luminaire's line lead goes to its switch leg when one is in the box.
                *role == Role::Neutral || *role == if switched_here { Role::Switched } else { Role::Hot }
            };
            if !lands {
                continue;
            }
            if r.is(&r.lead_families, &d.family) {
                leads += 1;
            } else {
                terminals += 1;
                through_ok &= r.is(&r.terminal_families, &d.family);
                if d.pigtail.is_some() {
                    pig = d.pigtail.clone();
                }
            }
        }
        let what = format!("{} {circuit}", role.as_str());
        if leads == 0 && terminals == 0 {
            if k == 1 {
                push(&mut out, *role, circuit, sizes.clone(), format!("{what}: one conductor nothing lands, capped ({})", r.methods_source), true);
            } else {
                push(&mut out, *role, circuit, sizes.clone(), format!("{what}: {k} conductors spliced through the box"), false);
            }
            continue;
        }
        let pigtailed = pig.is_some();
        if leads == 0 && terminals == 1 && k == 1 && !pigtailed {
            continue; // the circuit ends here: it lands on the device's terminals
        }
        if leads == 0 && terminals == 1 && k == 2 && !pigtailed && through_ok && feed == "device" {
            continue; // fed through the receptacle's second pair of terminals (project rule)
        }
        let pig_size = pig.clone().unwrap_or_else(|| sizes.iter().min_by_key(|s| gauge(s)).cloned().unwrap_or_default());
        let mut joined = sizes.clone();
        joined.extend(std::iter::repeat_n(pig_size.clone(), terminals));
        joined.extend(std::iter::repeat_n(awg(&r.lead_size), leads));
        let mut why = format!("{what}: {k} #{} entering", sizes.first().cloned().unwrap_or_default());
        if terminals > 0 {
            why.push_str(&format!(", {terminals} #{pig_size} pigtail(s) to the device(s)"));
        }
        if leads > 0 {
            why.push_str(&format!(", {leads} luminaire lead(s) ({})", r.lead_source));
        }
        push(&mut out, *role, circuit, joined, why, false);
    }
    // The grounds: one bundle with a pigtail to each device and, in a metal box, to the box.
    if !grounds.is_empty() {
        let lead_devs = devs.iter().filter(|(_, d)| r.is(&r.lead_families, &d.family)).count();
        let term_devs = devs.len() - lead_devs;
        let bond = usize::from(metal);
        let others = term_devs + lead_devs + bond;
        if grounds.len() >= 2 || others >= 2 {
            let g = grounds.iter().min_by_key(|s| gauge(s)).cloned().unwrap_or_default();
            let mut joined = grounds.clone();
            joined.extend(std::iter::repeat_n(g.clone(), term_devs + bond));
            joined.extend(std::iter::repeat_n(awg(&r.lead_size), lead_devs));
            let circuit = all.iter().find(|c| c.role == Role::Ground).map(|c| c.circuit.clone()).unwrap_or_default();
            let why = format!(
                "grounds: {} entering, {} pigtail(s) to the device(s){} ({})",
                grounds.len(),
                term_devs + lead_devs,
                if metal { ", 1 bonding jumper to the box" } else { "" },
                r.bond_source
            );
            push(&mut out, Role::Ground, &circuit, joined, why, false);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r() -> &'static SpliceRules {
        &super::super::rules().splices
    }

    fn sizes(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    fn dev(family: &str, circuit: &str) -> DeviceIn {
        DeviceIn { family: family.into(), circuit: circuit.into(), pigtail: None }
    }

    /// A circuit's run of `2#12 + 1#12G`.
    fn run(c: &str) -> Vec<Conductor> {
        entry_conductors(c, 2, 1, "#12", "#12")
    }

    fn roles(v: &[Connector]) -> Vec<(Role, usize)> {
        v.iter().map(|c| (c.role, c.conductors.len())).collect()
    }

    #[test]
    fn a_runs_conductors_are_hots_then_neutrals_then_its_switch_leg() {
        let one = entry_conductors("lp1-5", 3, 1, "#12", "#12");
        let got: Vec<(Role, &str)> = one.iter().map(|c| (c.role, c.circuit.as_str())).collect();
        assert_eq!(got, vec![(Role::Hot, "LP1-5"), (Role::Neutral, "LP1-5"), (Role::Switched, "LP1-5"), (Role::Ground, "LP1-5")]);
        // Three circuits, three neutrals; a multiwire circuit's one neutral is theirs together.
        let three = entry_conductors("LP1-1,3,5", 6, 1, "#12", "#12");
        assert_eq!(three.iter().filter(|c| c.role == Role::Neutral).map(|c| c.circuit.as_str()).collect::<Vec<_>>(), vec!["LP1-1", "LP1-3", "LP1-5"]);
        let mwbc = entry_conductors("LP1-1,3,5", 4, 1, "#12", "#12");
        assert_eq!(mwbc.iter().filter(|c| c.role == Role::Neutral).map(|c| c.circuit.as_str()).collect::<Vec<_>>(), vec!["LP1-1,3,5"]);
        assert!(entry_conductors("", 0, 0, "", "").is_empty());
    }

    #[test]
    fn a_receptacle_the_circuit_feeds_through_is_pigtailed_and_one_it_ends_at_lands_on_its_terminals() {
        let metal = true;
        // Feed-through: hot, neutral and the grounds each spliced.
        let through = box_splices(&[run("LP1-1"), run("LP1-1")], &[dev("receptacle", "LP1-1")], metal, "raceway", r());
        assert_eq!(roles(&through), vec![(Role::Hot, 3), (Role::Neutral, 3), (Role::Ground, 4)], "{through:#?}");
        assert!(through.iter().all(|c| c.item == r().item && !c.cap));
        // End of the circuit: hot and neutral on the terminals; the ground, the device's pigtail
        // and the box's bonding jumper in one connector.
        let end = box_splices(&[run("LP1-1")], &[dev("receptacle", "LP1-1")], metal, "raceway", r());
        assert_eq!(roles(&end), vec![(Role::Ground, 3)]);
        // In a nonmetallic box the one ground lands on the device: nothing to splice.
        assert!(box_splices(&[run("LP1-1")], &[dev("receptacle", "LP1-1")], false, "NM", r()).is_empty());
    }

    #[test]
    fn a_luminaires_leads_are_spliced_and_a_switch_leg_splits_its_hot() {
        // A luminaire fed and its switch leg leaving: line on to the switch, the switch leg to
        // the luminaire's lead, the neutrals with its other lead, the grounds.
        let leg = entry_conductors("LP1-7", 3, 1, "#12", "#12");
        let got = box_splices(&[run("LP1-7"), leg.clone()], &[dev("luminaire", "LP1-7")], true, "raceway", r());
        assert_eq!(roles(&got), vec![(Role::Hot, 2), (Role::Neutral, 3), (Role::Switched, 2), (Role::Ground, 4)], "{got:#?}");
        // The switch at the leg's end: its line and switch leg on its terminals, the neutral
        // brought to it capped, the ground spliced.
        let sw = box_splices(std::slice::from_ref(&leg), &[dev("switch", "LP1-7")], true, "raceway", r());
        assert_eq!(roles(&sw), vec![(Role::Neutral, 1), (Role::Ground, 3)], "{sw:#?}");
        assert!(sw.first().is_some_and(|c| c.cap));
        // The last luminaire of a run: still its leads spliced.
        let last = box_splices(&[run("LP1-7")], &[dev("luminaire", "")], true, "raceway", r());
        assert_eq!(roles(&last), vec![(Role::Hot, 2), (Role::Neutral, 2), (Role::Ground, 3)], "{last:#?}");
    }

    #[test]
    fn a_junction_box_splices_each_circuit_it_carries_on() {
        let got = box_splices(&[run("LP1-1"), run("LP1-1"), run("LP1-1"), run("LP1-3"), run("LP1-3")], &[], true, "raceway", r());
        // Two circuits' hots and neutrals, one bundle of grounds (five and the bonding jumper).
        let hot_neutral: Vec<(Role, usize)> = roles(&got).into_iter().filter(|(r, _)| *r != Role::Ground).collect();
        assert_eq!(hot_neutral, vec![(Role::Hot, 3), (Role::Neutral, 3), (Role::Hot, 2), (Role::Neutral, 2)]);
        let ground: usize = got.iter().filter(|c| c.role == Role::Ground).map(|c| c.conductors.len()).sum();
        assert!(ground >= 6, "{got:#?}");
    }

    #[test]
    fn the_connector_is_the_smallest_listed_for_its_conductors() {
        let size = |v: &[&str]| connectors_for(&sizes(v), r()).into_iter().map(|c| (c.0, c.1)).collect::<Vec<_>>();
        let small = r().connectors.first().map(|c| c.size.clone()).unwrap_or_default();
        assert_eq!(size(&["12", "12"]), vec![(r().item.clone(), small)]);
        // More conductors, a bigger one.
        let three = size(&["12", "12", "12"]);
        assert_eq!(three.len(), 1);
        assert_ne!(three.first().map(|c| c.1.clone()), size(&["12", "12"]).first().map(|c| c.1.clone()));
        // Past the largest twist-on: two joined by a jumper.
        let many = connectors_for(&sizes(&["12"; 9]), r());
        assert!(many.len() >= 2 && many.iter().all(|c| c.0 == r().item), "{many:?}");
        assert_eq!(many.iter().map(|c| c.2.len()).sum::<usize>(), 9 + 2 * (many.len() - 1));
        // A conductor no twist-on takes: a multi-tap connector.
        let big = size(&["4", "12"]);
        assert_eq!(big.first().map(|c| c.0.clone()), Some(r().large_item.clone()));
        assert!(connectors_for(&[], r()).len() <= 1);
    }

    #[test]
    fn a_project_rule_may_feed_receptacles_through_their_terminals() {
        let mut rr = r().clone();
        rr.methods.insert("NM".into(), MethodRule { feed_through: "device".into() });
        let got = box_splices(&[run("LP1-1"), run("LP1-1")], &[dev("receptacle", "LP1-1")], false, "NM", &rr);
        assert_eq!(roles(&got), vec![(Role::Ground, 3)]);
        // A pigtailed device (a larger conductor) is still spliced.
        let mut pd = dev("receptacle", "LP1-1");
        pd.pigtail = Some("12".into());
        let big = |c: &str| entry_conductors(c, 2, 1, "#8", "#10");
        let got = box_splices(&[big("LP1-1")], &[pd], true, "raceway", r());
        assert_eq!(roles(&got), vec![(Role::Hot, 2), (Role::Neutral, 2), (Role::Ground, 3)], "{got:#?}");
        assert!(got.first().is_some_and(|c| c.conductors.contains(&"8".to_string()) && c.conductors.contains(&"12".to_string())));
    }
}
