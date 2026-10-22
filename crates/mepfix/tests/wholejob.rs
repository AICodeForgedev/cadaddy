//! Q340: a synthetic building priced end to end across trades — roofing and envelope (Q337),
//! finishes (Q338), sitework (Q339), electrical, mechanical and plumbing — with invented prices
//! (no real price ships with CADaddy). Every contract value, the divisions, the general conditions
//! and the total equal a hand calculation; a sub bid missing scope is caught by leveling and the
//! low complete bid recommended; bid packages, public and private bid history and outcomes.

use std::collections::BTreeMap;

use cadaddy_mepfix::{finishes, generate, rng::Rng, sitework};
use serde_json::{Value, json};

/// An invented unit cost: by unit, nudged by the item's name so lines differ.
fn price(item: &str, unit: &str) -> f64 {
    let base = match unit {
        "sf" => 3.0,
        "sq" => 450.0,
        "lf" | "ft" => 12.0,
        "cy" => 60.0,
        "sy" => 25.0,
        "ton" => 90.0,
        "gal" => 30.0,
        "bf" => 2.0,
        _ => 85.0,
    };
    base * (1.0 + (item.len() % 5) as f64 / 10.0)
}

fn lines(v: &[(String, String, f64)]) -> Value {
    Value::Array(v.iter().map(|(i, u, q)| json!({"item": i, "unit": u, "qty": q})).collect())
}

#[test]
fn the_building_is_priced_across_trades_and_bids_are_leveled() {
    let set = generate(3).unwrap();
    let mut t = set.truth.clone();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for r in &t.building.rooms {
        lo = [lo[0].min(r.min_ft[0]), lo[1].min(r.min_ft[1])];
        hi = [hi[0].max(r.max_ft[0]), hi[1].max(r.max_ft[1])];
    }
    let (bw, bd) = (hi[0] - lo[0], hi[1] - lo[1]);
    sitework::design_sitework(&mut t, bw, bd, &mut Rng::new(77));
    // Every trade's takeoff.
    let mut trades: BTreeMap<String, Vec<(String, String, f64)>> = BTreeMap::new();
    let env = t.envelope.as_ref().expect("the envelope's takeoff");
    for q in &env.takeoff {
        trades.entry("roofing".into()).or_default().push((q.description.clone(), q.unit.to_ascii_lowercase(), q.order));
    }
    for i in finishes::compute_takeoff(&t).items {
        let e = trades.entry(i.trade.to_ascii_lowercase()).or_default();
        match e.iter_mut().find(|(d, u, _)| *d == i.description && *u == i.unit.to_ascii_lowercase()) {
            Some(x) => x.2 += i.with_waste,
            None => e.push((i.description.clone(), i.unit.to_ascii_lowercase(), i.with_waste)),
        }
    }
    for q in &t.site_quantities {
        let trade = match q.csi.get(..2) {
            Some("31") => "earthwork",
            Some("33") => "utilities",
            _ => "site",
        };
        trades.entry(trade.into()).or_default().push((format!("{} ({})", q.description, q.csi), q.unit.to_ascii_lowercase(), q.quantity));
    }
    let mut count: BTreeMap<String, f64> = BTreeMap::new();
    for d in &t.elec_devices {
        *count.entry(d.kind.replace('_', " ")).or_default() += 1.0;
    }
    trades.insert("electrical".into(), count.into_iter().map(|(k, v)| (k, "ea".into(), v)).collect());
    let mut len: BTreeMap<(String, String), f64> = BTreeMap::new();
    for s in t.segments.iter().filter(|s| !s.vertical) {
        let l = ((s.path_ft[1][0] - s.path_ft[0][0]).powi(2) + (s.path_ft[1][1] - s.path_ft[0][1]).powi(2)).sqrt();
        let trade = if s.kind == "duct" {
            "mechanical"
        } else if s.system == "FP" {
            "fire protection"
        } else {
            "plumbing"
        };
        *len.entry((trade.into(), format!("{} {}", s.system, s.kind))).or_default() += (l * 10.0).round() / 10.0;
    }
    for ((trade, item), l) in len {
        trades.entry(trade).or_default().push((item, "lf".into(), l));
    }
    for (k, v) in &trades {
        assert!(!v.is_empty(), "{k}");
    }
    for need in ["roofing", "painting", "flooring", "ceilings", "earthwork", "site", "electrical", "mechanical", "plumbing"] {
        assert!(trades.contains_key(need), "{need}: {:?}", trades.keys().collect::<Vec<_>>());
    }

    let mut s = cadaddy_engine::Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"units_per_foot": 12, "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 10}]}}))
        .unwrap();
    for (trade, ls) in &trades {
        s.execute("wholejob.quantities", &json!({"trade": trade, "lines": lines(ls)})).unwrap();
        let quality = match trade.as_str() {
            "electrical" => "private",
            "earthwork" => "assumed",
            _ => "public",
        };
        let rows: Vec<Value> = ls.iter().map(|(i, u, _)| json!({"item": i, "unit": u, "unit_cost": price(i, u)})).collect();
        s.execute("wholejob.prices", &json!({"trade": trade, "quality": quality, "source": "invented test prices", "rows": rows})).unwrap();
    }
    assert!(
        s.execute(
            "wholejob.prices",
            &json!({"trade": "electrical", "quality": "public", "source": "x", "rows": [{"item": "receptacle", "unit": "ea", "unit_cost": 1}]})
        )
        .is_err(),
        "no public electrical prices"
    );
    s.execute("wholejob.settings", &json!({"markups": {"roofing": 15}, "gc": {"weeks": 40}})).unwrap();
    let e = s.execute("wholejob.estimate", &json!({})).unwrap();
    assert_eq!(e["unpriced_lines"], 0, "{e}");
    // Each trade's contract value by hand: Σ qty × price × (1 + markup).
    let default_markup = |t: &str| match t {
        "roofing" => 15.0,
        "painting" | "plumbing" | "mechanical" | "electrical" | "tile" | "landscaping" => 12.0,
        _ => 10.0,
    };
    let mut subtotal = 0.0;
    for (trade, ls) in &trades {
        let direct: f64 = ls.iter().map(|(i, u, q)| q * price(i, u)).sum();
        let want = direct * (1.0 + default_markup(trade) / 100.0);
        let got = e["trades"].as_array().unwrap().iter().find(|x| x["trade"] == trade.as_str()).unwrap();
        assert!((got["contract_value"].as_f64().unwrap() - want).abs() < 0.02, "{trade}: {} vs {want}", got["contract_value"]);
        subtotal += (want * 100.0).round() / 100.0;
    }
    let quality = |t: &str| e["trades"].as_array().unwrap().iter().find(|x| x["trade"] == t).unwrap()["quality"].clone();
    assert_eq!((quality("electrical"), quality("roofing"), quality("earthwork")), (json!("private"), json!("public"), json!("assumed")));
    assert!((e["subtotal"].as_f64().unwrap() - subtotal).abs() < 0.1);
    // Divisions: 07, 09, 22, 23, 26, 31, 32 — and they sum to the subtotal.
    let divs = e["by_division"].as_object().unwrap();
    for d in ["07", "09", "22", "23", "26", "31", "32"] {
        assert!(divs.contains_key(d), "{d}: {divs:?}");
    }
    assert!((divs.values().map(|v| v.as_f64().unwrap()).sum::<f64>() - e["subtotal"].as_f64().unwrap()).abs() < 0.1);
    // General conditions: 40 weeks × the weekly rates, then insurance and bond.
    let weekly = 40.0 * (3000.0 + 600.0 + 900.0 + 400.0);
    let sub = e["subtotal"].as_f64().unwrap();
    let ins = (sub + weekly) * 0.01;
    let bond = (sub + weekly + ins) * 0.01;
    assert!((e["general_conditions"]["total"].as_f64().unwrap() - (weekly + ins + bond)).abs() < 0.05, "{}", e["general_conditions"]);
    assert!((e["total"].as_f64().unwrap() - (sub + weekly + ins + bond)).abs() < 0.05);

    // Leveling painting: A complete, B cheaper but missing a line and excluding another.
    let paint = &trades["painting"];
    assert!(paint.len() >= 3, "{paint:?}");
    let bid = |factor: f64, skip: &[usize]| -> (Vec<Value>, f64) {
        let ls: Vec<Value> = paint
            .iter()
            .enumerate()
            .filter(|(k, _)| !skip.contains(k))
            .map(|(_, (i, u, q))| json!({"item": i, "unit": u, "qty": q, "amount": q * price(i, u) * factor}))
            .collect();
        let total = ls.iter().map(|l| l["amount"].as_f64().unwrap()).sum();
        (ls, total)
    };
    let (a_lines, a_total) = bid(1.08, &[]);
    let (b_lines, b_total) = bid(1.0, &[0]);
    s.execute("wholejob.bid", &json!({"trade": "painting", "bidder": "Bidder A", "total": a_total, "lines": a_lines})).unwrap();
    s.execute("wholejob.bid", &json!({"trade": "painting", "bidder": "Bidder B", "total": b_total, "lines": b_lines, "exclusions": [paint[1].0]}))
        .unwrap();
    let lv = s.execute("wholejob.level", &json!({"trade": "painting"})).unwrap();
    let b = lv["bids"].as_array().unwrap().iter().find(|x| x["bidder"] == "Bidder B").unwrap();
    assert_eq!(b["complete"], false);
    let missing: Vec<&str> = b["missing_scope"].as_array().unwrap().iter().map(|m| m["item"].as_str().unwrap()).collect();
    assert_eq!(missing, [paint[0].0.as_str(), paint[1].0.as_str()], "{b}");
    assert_eq!(b["missing_scope"][1]["why"], "excluded");
    assert!(b["leveled_total"].as_f64().unwrap() > b_total, "missing scope priced in");
    assert!(b_total < a_total, "B is lower on paper");
    assert_eq!(lv["recommended"], "Bidder A", "the low COMPLETE bid: {lv}");

    // The bid package; bid history; outcomes.
    let dir = std::env::temp_dir().join(format!("cadaddy-wholejob-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pkg = dir.join("roofing.csv");
    let p = s.execute("wholejob.package", &json!({"trade": "roofing", "path": pkg.display().to_string()})).unwrap();
    assert_eq!(p["division"], "07");
    assert!(std::fs::read_to_string(&pkg).unwrap().contains(&trades["roofing"][0].0));
    let public = dir.join("public-bids.jsonl");
    let private = dir.join("private-bids.jsonl");
    for (amount, sf) in [(450_000.0, 60_000.0), (510_000.0, 60_000.0), (300_000.0, 30_000.0)] {
        s.execute("wholejob.history", &json!({"pack": public.display().to_string(), "trade": "roofing", "building_type": "hotel", "region": "south", "date": "2026-06-01", "sf": sf, "amount": amount, "source": "a published bid tabulation"})).unwrap();
    }
    assert!(
        s.execute("wholejob.history", &json!({"pack": public.display().to_string(), "trade": "electrical", "sf": 60000, "amount": 900000})).is_err(),
        "electrical only to a private pack"
    );
    s.execute("wholejob.history", &json!({"pack": private.display().to_string(), "private": true, "trade": "electrical", "building_type": "hotel", "sf": 60000, "amount": 900000})).unwrap();
    let bm = s.execute("wholejob.benchmarks", &json!({"packs": [public.display().to_string()], "building_type": "Hotel"})).unwrap();
    assert_eq!(bm["benchmarks"], json!([{"trade": "roofing", "count": 3, "per_sf_median": 8.5, "per_sf_min": 7.5, "per_sf_max": 10.0}]));
    assert!(
        s.execute("wholejob.outcome", &json!({"pack": public.display().to_string(), "trade": "roofing", "award": 480000})).is_err(),
        "outcomes are private"
    );
    s.execute("wholejob.outcome", &json!({"pack": private.display().to_string(), "private": true, "trade": "roofing", "award": 480000, "final_cost": 492000, "change_orders": 12000})).unwrap();
    assert_eq!(std::fs::read_to_string(&private).unwrap().lines().count(), 2);
}
