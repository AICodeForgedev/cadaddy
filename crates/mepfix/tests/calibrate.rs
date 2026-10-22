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
    // A retry re-adds P-901 twice more: it replaces the row, so it is still one project.
    for _ in 0..2 {
        let r = s.execute("calibrate.project", &json!({"pack": pack, "id": "P-901", "building_type": "hotel", "quantities": {"elevators": {"stops [ea]": 6}}, "totals": {"elevators": 400000}})).unwrap();
        assert_eq!(r["replaced"], true);
    }
    let rows = std::fs::read_to_string(&pack).unwrap();
    assert_eq!(rows.lines().filter(|l| l.contains("\"P-901\"")).count(), 1, "{rows}");
    assert_eq!(rows.lines().count(), 32);
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
    assert_eq!(el["projects"], 2, "{el}");
    assert!(!fit["warnings"].to_string().contains("rows with this id"), "{}", fit["warnings"]);
    // The same id in a second pack: one project per id across packs (the last wins), and the
    // fit says so.
    let other = dir.join("more-projects.jsonl").display().to_string();
    let _ = std::fs::remove_file(&other);
    s.execute("calibrate.project", &json!({"pack": other, "id": "P-901", "building_type": "hotel", "quantities": {"elevators": {"stops [ea]": 6}}, "totals": {"elevators": 400000}})).unwrap();
    let both = s.execute("calibrate.fit", &json!({"packs": [pack, other], "building_type": "hotel", "regions": regions, "index": index, "base_date": "2026-01", "markups": {"roofing": 12}})).unwrap();
    assert_eq!(both["projects"], 32);
    let el2 = both["rates"].as_array().unwrap().iter().find(|r| r["trade"] == "elevators").unwrap();
    assert_eq!((el2["projects"].clone(), el2["quality"].clone()), (json!(2), json!("insufficient data")), "{el2}");
    let w: Vec<&str> = both["warnings"].as_array().unwrap().iter().filter_map(Value::as_str).filter(|w| w.contains("rows with this id")).collect();
    assert!(w.len() == 1 && w[0].starts_with("P-901: 2 rows"), "{w:?}");
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
    // Straight time + overtime: 40 + 8 = 48 h, never one column picked by its name's order.
    let payroll = |name: &str, text: &str| -> Result<Value, String> {
        let f = dir.join(name);
        std::fs::write(&f, text).unwrap();
        Session::new().execute("calibrate.payroll", &json!({"path": f.display().to_string()})).map_err(|e| e.to_string())
    };
    let p = payroll("st-ot.csv", "Classification,ST Hours,OT Hours\nPlumber,40,8\nPlumber,36,\n").unwrap();
    assert_eq!(p["hours"], json!({"plumbing": 84.0}), "{p}");
    assert_eq!(p["hours_columns"], json!(["ot hours", "st hours"]));
    let p =
        payroll("st-ot-total.csv", "Classification,ST Hours,OT Hours,Total Hours,Hourly Rate\nPlumber,40,8,48,52.5\nRoofer,30,0,30,41\n").unwrap();
    assert_eq!(p["hours"], json!({"plumbing": 48.0, "roofing": 30.0}), "{p}");
    assert_eq!(p["hours_columns"], json!(["total hours"]));
    // Per-day columns (a weekly payroll's Mon … Sun) are added up.
    let p = payroll("days.csv", "Classification,Mon,Tue,Wed,Thu,Fri,Sat,Sun\nRoofer,8,8,8,8,8,4,\n").unwrap();
    assert_eq!(p["hours"], json!({"roofing": 44.0}), "{p}");
    // Can't tell: refused, naming the columns.
    let e = payroll("unclear.csv", "Classification,Shop Hours,Field Hours\nPlumber,4,36\n").unwrap_err();
    assert!(e.contains("\"field hours\"") && e.contains("\"shop hours\"") && e.contains("Total Hours"), "{e}");
    // A cement mason finishes concrete; a brick mason lays masonry.
    let p = payroll("mason.csv", "Classification,Hours\nCement Mason,40\nConcrete Finisher,10\nBrick Mason,20\n").unwrap();
    assert_eq!(p["hours"], json!({"concrete": 50.0, "masonry": 20.0}), "{p}");

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

    // The same case from raw census figures: $10,000,000 of work, $3,000,000 subcontracted out
    // (70 % self-performed), 56,000 worker hours (0.008 h per self-performed $), $2,240,000
    // wages ($40/h). CADaddy works the ratios out; the agent never does.
    let census = json!({"Plumbing": {"value_of_work": 10_000_000.0, "subcontracted_out": 3_000_000.0, "construction_worker_hours": 56_000.0, "construction_worker_wages": 2_240_000.0}});
    let td = s
        .execute(
            "calibrate.topdown",
            &json!({"values": {"plumbing": 1_000_000.0}, "census": census, "wage_escalation": 1.25, "materials_escalation": 1.15, "bottom_up_hours": {"plumbing": 3500.0}}),
        )
        .unwrap();
    let pl = &td["trades"][0];
    assert_eq!(pl["field_hours"], json!(4737.73));
    assert_eq!(pl["wages"], json!(236886.63));
    assert_eq!(pl["gap_pct"], json!(35.36));
    assert_eq!(pl["ratios"], json!({"self_performed_share": 0.7, "hours_per_dollar": 0.008, "wage_per_hour": 40.0, "from": "census"}));
    // Raw figures inside `ratios` work too.
    let td = s
        .execute(
            "calibrate.topdown",
            &json!({"values": {"plumbing": 1_000_000.0}, "ratios": census, "wage_escalation": 1.25, "materials_escalation": 1.15}),
        )
        .unwrap();
    assert_eq!(td["trades"][0]["field_hours"], json!(4737.73));
    // Zero, negative or missing figures are refused with the figures it needs.
    let good = census["Plumbing"].clone();
    for (k, v) in [
        ("value_of_work", json!(0)),
        ("value_of_work", json!(-5)),
        ("subcontracted_out", json!(-1)),
        ("subcontracted_out", json!(10_000_000.0)),
        ("construction_worker_hours", json!(0)),
        ("construction_worker_wages", json!(-2)),
        ("construction_worker_wages", Value::Null),
    ] {
        let mut c = good.clone();
        c[k] = v.clone();
        let e = s.execute("calibrate.topdown", &json!({"values": {"plumbing": 1_000_000.0}, "census": {"plumbing": c}})).unwrap_err().to_string();
        assert!(e.contains("value_of_work > 0") && e.contains("construction_worker_hours > 0"), "{k}={v}: {e}");
    }
}

/// One synthetic project (`seed`) as in the main test: its quantities, its trade totals, the
/// project total (their sum), its region and its bid date.
fn synthetic(seed: u64) -> (Value, Map, f64, &'static str, String) {
    let idx = |m: u32| -> f64 {
        let m = f64::from(m);
        if m <= 12.0 { 100.0 + 4.0 * m / 12.0 } else { 104.0 + 5.2 * (m - 12.0).min(12.0) / 12.0 }
    };
    let q = quantities(seed);
    let month = (seed * 7 % 24) as u32;
    let (region, rf) = if seed.is_multiple_of(2) { ("north", 1.08) } else { ("south", 0.95) };
    let f = rf * idx(month) / 109.2;
    let mut totals = Map::new();
    let mut sum = 0.0;
    for (t, ds) in &q {
        let dollars: f64 =
            ds.iter().map(|(d, v)| v * RATES.iter().find(|r| r.0 == *t && r.1 == *d).unwrap().2).sum::<f64>() * f * (1.0 + markup(t) / 100.0);
        totals.insert(t.to_string(), json!(dollars));
        sum += dollars;
    }
    (serde_json::to_value(&q).unwrap(), totals, sum, region, format!("{}-{:02}", 2024 + month / 12, month % 12 + 1))
}

#[test]
fn rates_from_project_totals_only() {
    // Most public bids publish only the project total: 36 projects, each with its total alone
    // (±3 % noise), fitted per project.
    let dir = std::env::temp_dir().join(format!("cadaddy-calibrate-total-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pack = dir.join("projects.jsonl").display().to_string();
    let _ = std::fs::remove_file(&pack);
    let mut s = Session::new();
    for seed in 1..=36u64 {
        let (q, _, sum, region, bid_date) = synthetic(seed);
        let total = sum * (1.0 + noise(seed * 13 + 5, 0.03));
        s.execute("calibrate.project", &json!({"pack": pack, "id": format!("T-{seed:03}"), "building_type": "school", "region": region, "bid_date": bid_date, "quantities": q, "total": total}))
            .unwrap();
    }
    let rows = std::fs::read_to_string(&pack).unwrap();
    assert!(rows.lines().all(|l| l.contains("\"totals\":{}") && !l.contains("\"total\":null")), "only project totals");
    let fit = s
        .execute(
            "calibrate.fit",
            &json!({"packs": [pack], "regions": {"north": 1.08, "south": 0.95}, "index": {"2024-01": 100.0, "2025-01": 104.0, "2026-01": 109.2}, "base_date": "2026-01", "markups": {"roofing": 12}}),
        )
        .unwrap();
    assert_eq!(fit["projects"], 36);
    assert_eq!(fit["observations"], 36, "one observation per project");
    for (t, d, want, _) in RATES {
        let r = fit["rates"].as_array().unwrap().iter().find(|r| r["trade"] == *t && r["driver"] == *d).unwrap();
        let (got, sd) = (r["rate"].as_f64().unwrap(), r["sd"].as_f64().unwrap());
        assert!((got - want).abs() <= 3.0 * sd, "{t}/{d}: {got} ± {sd} vs {want}");
        assert!(r["quality"] == "fitted" || r["quality"] == "weak", "{r}");
        // One total per project can't always separate the drivers; a rate far from the truth
        // must then say "weak", never "fitted".
        if (got - want).abs() / want > 0.25 {
            assert_eq!(r["quality"], "weak", "{t}/{d}: {r}");
        }
        assert_eq!(r["projects"], 36, "{r}");
    }
    let h = &fit["holdout"];
    assert!(h["projects"].as_u64().unwrap() >= 5 && h["mape_pct"].as_f64().unwrap() < 8.0, "{h}");
}

#[test]
fn a_project_from_the_whole_job_estimate() {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"units_per_foot": 12, "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 10}]}}))
        .unwrap();
    s.execute("wholejob.quantities", &json!({"trade": "Drywall", "lines": [{"item": "Gypsum board", "unit": "SF", "qty": 1000}, {"item": "Corner bead", "unit": "lf", "qty": 200}]})).unwrap();
    s.execute(
        "wholejob.quantities",
        &json!({"trade": "Painting", "lines": [{"item": "Wall paint", "unit": "sf", "qty": 2400}, {"item": "Skipped", "unit": "ea", "qty": 0}]}),
    )
    .unwrap();
    let dir = std::env::temp_dir().join(format!("cadaddy-calibrate-wj-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pack = dir.join("projects.jsonl").display().to_string();
    let _ = std::fs::remove_file(&pack);
    let r = s
        .execute("calibrate.project", &json!({"pack": pack, "id": "W-001", "building_type": "office", "from_wholejob": true, "total": 25_000}))
        .unwrap();
    assert_eq!(r["trades"], 2);
    let rows = std::fs::read_to_string(&pack).unwrap();
    let row: Value = serde_json::from_str(rows.lines().next().unwrap()).unwrap();
    assert_eq!(
        row["quantities"],
        json!({"drywall": {"corner bead [lf]": 200.0, "gypsum board [sf]": 1000.0}, "painting": {"wall paint [sf]": 2400.0}})
    );
    assert_eq!(row["total"], json!(25_000.0));
    // Without a whole-job estimate there is nothing to take.
    let mut empty = Session::new();
    assert!(empty.execute("calibrate.project", &json!({"pack": pack, "id": "W-002", "from_wholejob": true, "total": 1})).is_err());
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
