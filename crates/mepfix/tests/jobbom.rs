//! The bill of materials of a fixture set read from its prints (Q326) against the truth:
//! conduit by size, luminaires by type, devices by type.

use cadaddy_doc::projrules::ProjectRules;
use cadaddy_engine::Session;
use cadaddy_mepfix::{conduit_reader, spectag};
use serde_json::json;

#[test]
fn the_bom_of_a_read_set_matches_the_truth() {
    let rules = ProjectRules::new();
    let (t, tags) = spectag::read_fixture_tags(3, &rules).unwrap();
    let (_, mut runs) = conduit_reader::read_fixture_conduits(3).unwrap();
    // Every run gets a height so the BOM counts it (heights don't change quantities).
    for c in &mut runs.conduits {
        c.bottom_ft.get_or_insert(9.0);
    }
    let names: Vec<(&str, &str)> = t.building.levels.iter().map(|l| (l.code.as_str(), l.code.as_str())).collect();
    let (systems, mep_runs, _) = conduit_reader::spec_runs(&runs, &names);
    let levels: Vec<serde_json::Value> = t
        .building
        .levels
        .iter()
        .filter(|l| l.code != "R")
        .map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 13}))
        .collect();
    let mut s = Session::new();
    s.execute("bspec.set", &json!({ "replace": true, "spec": { "levels": levels, "mep": { "systems": systems, "runs": mep_runs }, "tags": spectag::spec_patch(&tags) } })).unwrap();
    let bom = s.execute("bim.bom", &json!({})).unwrap();
    let lines = bom["lines"].as_array().unwrap();
    let net = |pred: &dyn Fn(&serde_json::Value) -> bool| lines.iter().filter(|l| pred(l)).map(|l| l["net"].as_f64().unwrap()).sum::<f64>();
    // Conduit by size: the truth's drawn conduit, exactly.
    for (size, want_in) in [("3/4\"", 0.75), ("1\"", 1.0), ("1-1/4\"", 1.25)] {
        let truth: f64 = t
            .elec_conduits
            .iter()
            .filter(|c| !c.sheet.is_empty() && (c.size_in - want_in).abs() < 1e-6)
            .map(|c| (c.to_ft[0] - c.from_ft[0]).hypot(c.to_ft[1] - c.from_ft[1]))
            .sum();
        let got = net(&|l| l["item"] == json!("EMT conduit") && l["size"] == json!(size));
        assert!((got - truth).abs() < 0.5, "{size}: BOM {got} ft vs truth {truth} ft");
    }
    // Luminaires by type (within 10%: the device reader misses a few close-packed troffers) and
    // receptacles (within 5%) against the truth's drawn elements.
    let mut types: std::collections::BTreeMap<String, usize> = Default::default();
    for d in t.elec_devices.iter().filter(|d| d.kind == "light_fixture" && !d.sheet.is_empty() && !t.elec_spec.untagged.contains(&d.id)) {
        *types.entry(d.type_tag.clone()).or_default() += 1;
    }
    for (ty, n) in &types {
        let got = net(&|l| l["item"] == json!(format!("Luminaire type {ty}")));
        assert!((got - *n as f64).abs() <= (*n as f64 * 0.1).max(1.0), "type {ty}: BOM {got} vs truth {n}");
    }
    let recep = t.elec_devices.iter().filter(|d| d.kind == "receptacle" && !d.sheet.is_empty()).count() as f64;
    let got = net(&|l| l["category"] == json!("devices") && l["item"].as_str().unwrap_or_default().ends_with("receptacle"));
    assert!((got - recep).abs() <= recep * 0.05, "receptacles: BOM {got} vs truth {recep}");
    // The GFCI devices the documents call for, and wire from every homerun.
    assert!(net(&|l| l["item"] == json!("GFCI receptacle")) >= 1.0);
    assert!(net(&|l| l["category"] == json!("wire")) > 0.0);
    assert!(bom["required"].as_array().unwrap().iter().any(|r| r["item"] == json!("Knockout set")));
}
