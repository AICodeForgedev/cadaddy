//! Q341: rates recovered from bid totals. Synthetic "public projects" are the generator's own
//! buildings (their BIM quantities per trade and driver); each project's trade totals come from
//! known rates × its region × its bid date's cost index × the trade's markup, with a few percent
//! of noise. `calibrate.fit` recovers the rates within its stated error, predicts held-out
//! projects, says "insufficient data" where too few projects stand behind a rate, and keeps
//! electrical rates out of a public pack. Certified-payroll hours fit field hours per unit.

use std::collections::BTreeMap;

use cadaddy_engine::Session;
use cadaddy_mepfix::generate;
use serde_json::{Value, json};

/// The true (invented) rates: $ per unit, and field hours per unit.
const RATES: &[(&str, &str, f64, f64)] = &[
    ("electrical", "devices [ea]", 210.0, 1.6),
    ("electrical", "panels [ea]", 5200.0, 30.0),
    ("plumbing", "pipe [lf]", 38.0, 0.30),
    ("plumbing", "fixtures [ea]", 1400.0, 6.0),
    ("mechanical", "duct [lf]", 62.0, 0.40),
    ("mechanical", "equipment [ea]", 9000.0, 24.0),
    ("roofing", "membrane [sq]", 520.0, 2.5),
];

/// Every trade here carries the default 12 % markup (roofing's is given to the fit explicitly).
fn markup(_trade: &str) -> f64 {
    12.0
}

/// A deterministic ±`amp` noise in [-amp, amp].
fn noise(i: u64, amp: f64) -> f64 {
    let x = (i.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) >> 33) as f64 / (1u64 << 31) as f64;
    (x * 2.0 - 1.0) * amp
}

fn quantities(seed: u64) -> BTreeMap<&'static str, BTreeMap<&'static str, f64>> {
    let t = generate(seed).unwrap().truth;
    let mut q: BTreeMap<&str, BTreeMap<&str, f64>> = BTreeMap::new();
    let len = |kind: &str, fp: bool| -> f64 {
        t.segments
            .iter()
            .filter(|s| !s.vertical && s.kind == kind && (s.system == "FP") == fp)
            .map(|s| ((s.path_ft[1][0] - s.path_ft[0][0]).powi(2) + (s.path_ft[1][1] - s.path_ft[0][1]).powi(2)).sqrt())
            .sum::<f64>()
            .round()
    };
    q.entry("electrical").or_default().insert("devices [ea]", t.elec_devices.len() as f64);
    q.entry("electrical").or_default().insert("panels [ea]", t.elec_panels.len() as f64);
    q.entry("plumbing").or_default().insert("pipe [lf]", len("pipe", false));
    q.entry("plumbing").or_default().insert("fixtures [ea]", t.fixtures.len() as f64);
    q.entry("mechanical").or_default().insert("duct [lf]", len("duct", false));
    q.entry("mechanical").or_default().insert("equipment [ea]", t.equipment.len() as f64);
    if let Some(m) = t.envelope.as_ref().and_then(|e| e.takeoff.iter().find(|x| x.code == "roof.membrane")) {
        q.entry("roofing").or_default().insert("membrane [sq]", m.net);
    }
    for ds in q.values_mut() {
        ds.retain(|_, v| *v > 0.0);
    }
    q.retain(|_, ds| !ds.is_empty());
    q
}

#[test]
fn fitted_rates_recover_the_truth_from_noisy_bid_totals() {
    let dir = std::env::temp_dir().join(format!("cadaddy-calibrate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pack = dir.join("projects.jsonl").display().to_string();
    let index = json!({"2024-01": 100.0, "2025-01": 104.0, "2026-01": 109.2});
    let idx = |m: u32| -> f64 {
        // Months from 2024-01 → the index, linear between the points.
        let m = f64::from(m);
        if m <= 12.0 { 100.0 + 4.0 * m / 12.0 } else { 104.0 + 5.2 * (m - 12.0).min(12.0) / 12.0 }
    };
    let regions = json!({"north": 1.08, "south": 0.95});
    let mut s = Session::new();
    let mut n = 0;
    for seed in 1..=30u64 {
        let q = quantities(seed);
        let month = (seed * 7 % 24) as u32;
        let (region, rf) = if seed % 2 == 0 { ("north", 1.08) } else { ("south", 0.95) };
        let f = rf * idx(month) / 109.2;
        let mut totals = Map::new();
        let mut hours = Map::new();
        for (t, ds) in &q {
            let dollars: f64 = ds.iter().map(|(d, v)| v * RATES.iter().find(|r| r.0 == *t && r.1 == *d).unwrap().2).sum();
            let h: f64 = ds.iter().map(|(d, v)| v * RATES.iter().find(|r| r.0 == *t && r.1 == *d).unwrap().3).sum();
            totals.insert(t.to_string(), json!(dollars * f * (1.0 + markup(t) / 100.0) * (1.0 + noise(seed * 31 + t.len() as u64, 0.03))));
            hours.insert(t.to_string(), json!(h * (1.0 + noise(seed * 17 + t.len() as u64, 0.03))));
        }
        let quantities: Value = serde_json::to_value(&q).unwrap();
        s.execute(
            "calibrate.project",
            &json!({"pack": pack, "id": format!("P-{seed:03}"), "building_type": "hotel", "region": region, "bid_date": format!("{}-{:02}", 2024 + month / 12, month % 12 + 1), "quantities": quantities, "totals": totals, "hours": hours}),
        )
        .unwrap();
        n += 1;
    }
    // A trade only two projects bid: not enough to fit.
    for id in ["P-901", "P-902"] {
        s.execute("calibrate.project", &json!({"pack": pack, "id": id, "building_type": "hotel", "quantities": {"elevators": {"stops [ea]": 6}}, "totals": {"elevators": 400000}})).unwrap();
    }
    assert!(n == 30);
    let public = dir.join("public-rates.jsonl").display().to_string();
    let fit = s
        .execute("calibrate.fit", &json!({"packs": [pack], "building_type": "hotel", "regions": regions, "index": index, "base_date": "2026-01", "markups": {"roofing": 12}, "out": public}))
        .unwrap();
    assert_eq!(fit["projects"], 32);
    let rate = |t: &str, d: &str| fit["rates"].as_array().unwrap().iter().find(|r| r["trade"] == t && r["driver"] == d).unwrap().clone();
    for (t, d, want, _) in RATES {
        let r = rate(t, d);
        let (got, sd) = (r["rate"].as_f64().unwrap(), r["sd"].as_f64().unwrap());
        assert!((got - want).abs() <= 3.0 * sd + 1e-9, "{t}/{d}: {got} ± {sd} vs {want}");
        assert!((got - want).abs() / want < 0.10, "{t}/{d}: {got} vs {want}");
        assert_eq!(r["projects"], 30, "{r}");
    }
    let el = rate("elevators", "stops [ea]");
    assert_eq!(el["quality"], "insufficient data");
    assert_eq!(el["rate"], Value::Null);
    let noise_pct = fit["noise_pct"].as_f64().unwrap();
    assert!(noise_pct > 0.5 && noise_pct < 4.0, "{noise_pct}");
    let h = &fit["holdout"];
    assert!(h["projects"].as_u64().unwrap() >= 5 && h["mape_pct"].as_f64().unwrap() < 5.0, "{h}");
    // Electrical stays out of the public pack.
    let written = std::fs::read_to_string(&public).unwrap();
    assert!(!written.contains("electrical") && written.contains("plumbing"), "{written}");
    assert_eq!(fit["written"]["withheld_electrical"].as_array().unwrap().len(), 2);
    let private = dir.join("private-rates.jsonl").display().to_string();
    s.execute(
        "calibrate.fit",
        &json!({"packs": [pack], "regions": regions, "index": index, "base_date": "2026-01", "out": private, "private": true}),
    )
    .unwrap();
    assert!(std::fs::read_to_string(&private).unwrap().contains("electrical"));

    // Hours from certified payrolls: field hours per unit.
    let hf = s.execute("calibrate.fit", &json!({"packs": [pack], "target": "hours"})).unwrap();
    for (t, d, _, want) in RATES {
        let r = hf["rates"].as_array().unwrap().iter().find(|r| r["trade"] == *t && r["driver"] == *d).unwrap();
        let (got, sd) = (r["rate"].as_f64().unwrap(), r["sd"].as_f64().unwrap());
        assert!((got - want).abs() <= 3.0 * sd && sd / want < 0.15, "{t}/{d} hours: {r}");
    }
}

type Map = serde_json::Map<String, Value>;

#[test]
fn payroll_hours_and_the_top_down_estimate() {
    let mut s = Session::new();
    let p = s
        .execute(
            "calibrate.payroll",
            &json!({"rows": [
                {"classification": "Electrician (Journeyman)", "hours": 40}, {"classification": "Electrician Apprentice", "hours": 32},
                {"classification": "Plumber", "hours": 38}, {"classification": "Sheet Metal Worker", "hours": 40}, {"classification": "Clerk", "hours": 8},
                {"classification": "Groundman", "hours": 10}],
                "map": {"Groundman": "electrical"}}),
        )
        .unwrap();
    assert_eq!(p["hours"], json!({"electrical": 82.0, "mechanical": 40.0, "plumbing": 38.0}));
    assert_eq!(p["unmapped"], json!({"Clerk": 8.0}));
    let dir = std::env::temp_dir().join(format!("cadaddy-payroll-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("payroll.csv");
    std::fs::write(&csv, "Week,Craft,Hrs\n2026-01-05,Roofer,40\n2026-01-05,\"Painter, brush\",20\n").unwrap();
    let p = s.execute("calibrate.payroll", &json!({"path": csv.display().to_string()})).unwrap();
    assert_eq!(p["hours"], json!({"painting": 20.0, "roofing": 40.0}));

    // Top-down: $1,000,000 of plumbing, 70 % self-performed, 0.008 field hours per census $,
    // $40/h: wage share 0.32; escalation 0.32 × 1.25 + 0.68 × 1.15 = 1.182; hours = 700,000 /
    // 1.182 × 0.008 = 4737.73; wages = 4737.73 × 40 × 1.25 = 236,886.63.
    let td = s
        .execute(
            "calibrate.topdown",
            &json!({"values": {"plumbing": 1_000_000.0, "painting": 50_000.0}, "ratios": {"Plumbing": {"self_performed_share": 0.7, "hours_per_dollar": 0.008, "wage_per_hour": 40.0}},
                "wage_escalation": 1.25, "materials_escalation": 1.15, "bottom_up_hours": {"plumbing": 3500.0}}),
        )
        .unwrap();
    let pl = &td["trades"][0];
    assert_eq!(pl["field_hours"], json!(4737.73));
    assert_eq!(pl["wages"], json!(236886.63));
    assert_eq!(pl["gap_pct"], json!(35.36));
    assert_eq!(pl["flag"], json!(true));
    assert_eq!(td["no_ratios"], json!(["painting"]));
}

#[test]
fn bad_input_is_refused() {
    let mut s = Session::new();
    for (c, p) in [
        ("calibrate.project", json!({"pack": "x.jsonl", "quantities": {"a": {"b": 1}}, "total": 5})),
        ("calibrate.project", json!({"pack": "x.txt", "id": "P1", "quantities": {"a": {"b": 1}}, "total": 5})),
        ("calibrate.project", json!({"pack": "x.jsonl", "id": "P1", "quantities": {"a": {"b": -1}}, "total": 5})),
        ("calibrate.project", json!({"pack": "x.jsonl", "id": "P1", "quantities": {"a": {"b": 1}}})),
        ("calibrate.fit", json!({"packs": []})),
        ("calibrate.fit", json!({"packs": ["/nonexistent/x.jsonl"]})),
        ("calibrate.fit", json!({"packs": ["x.jsonl"], "target": "joy"})),
        ("calibrate.payroll", json!({})),
        ("calibrate.topdown", json!({"values": {"a": 1}})),
        ("calibrate.topdown", json!({"values": {"a": 1}, "ratios": {"a": {"self_performed_share": 2, "hours_per_dollar": 1, "wage_per_hour": 1}}})),
    ] {
        assert!(s.execute(c, &p).is_err(), "{c} {p}");
    }
}
