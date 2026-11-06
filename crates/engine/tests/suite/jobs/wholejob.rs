//! Q340 engine checks: bad input refused, never a panic; markups, unpriced lines and the
//! electrical rule; leveling with no complete bid.

use cadaddy_engine::Session;
use serde_json::json;

fn session() -> Session {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"units_per_foot": 12, "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 10}]}}))
        .unwrap();
    s
}

#[test]
fn bad_input_is_refused() {
    let mut s = Session::new();
    assert!(s.execute("wholejob.estimate", &json!({})).is_err(), "no building spec");
    let mut s = session();
    assert!(s.execute("wholejob.estimate", &json!({})).is_err(), "no trades");
    for p in [
        json!({}),
        json!({"trade": "roofing"}),
        json!({"trade": "roofing", "lines": [{"item": "x", "qty": -1}]}),
        json!({"trade": "roofing", "lines": [{"qty": 1}]}),
        json!({"trade": "roofing", "lines": [{"item": "x", "qty": f64::MAX}]}),
    ] {
        assert!(s.execute("wholejob.quantities", &p).is_err(), "{p}");
    }
    assert!(s.execute("wholejob.prices", &json!({"trade": "roofing", "quality": "great", "source": "x", "rows": []})).is_err());
    assert!(
        s.execute("wholejob.prices", &json!({"trade": "roofing", "quality": "public", "source": "x", "rows": [{"item": "x", "unit_cost": 0}]}))
            .is_err()
    );
    assert!(s.execute("wholejob.settings", &json!({"markups": {"roofing": 500}})).is_err());
    assert!(s.execute("wholejob.settings", &json!({"gc": {"nonsense": 1}})).is_err());
    assert!(s.execute("wholejob.bid", &json!({"trade": "roofing", "bidder": "A"})).is_err(), "no total");
    assert!(s.execute("wholejob.level", &json!({"trade": "roofing"})).is_err(), "no quantities");
    assert!(s.execute("wholejob.history", &json!({"pack": "x.txt", "trade": "roofing", "sf": 1, "amount": 1})).is_err());
    assert!(s.execute("wholejob.benchmarks", &json!({"packs": []})).is_err());
}

#[test]
fn markups_unpriced_lines_and_electrical() {
    let mut s = session();
    s.execute("wholejob.quantities", &json!({"trade": "Drywall", "lines": [{"item": "Gypsum board", "unit": "SF", "qty": 1000}, {"item": "Corner bead", "unit": "lf", "qty": 200}]})).unwrap();
    s.execute(
        "wholejob.prices",
        &json!({"trade": "drywall", "quality": "assumed", "source": "invented", "rows": [{"item": "gypsum  BOARD", "unit": "sf", "unit_cost": 2.5}]}),
    )
    .unwrap();
    s.execute("wholejob.quantities", &json!({"trade": "fire alarm", "lines": [{"item": "smoke detector", "unit": "ea", "qty": 10}]})).unwrap();
    assert!(
        s.execute(
            "wholejob.prices",
            &json!({"trade": "fire alarm", "quality": "public", "source": "x", "rows": [{"item": "smoke detector", "unit": "ea", "unit_cost": 100}]})
        )
        .is_err(),
        "division 28 is electrical"
    );
    s.execute("wholejob.prices", &json!({"trade": "fire alarm", "quality": "private", "source": "company", "rows": [{"item": "smoke detector", "unit": "ea", "unit_cost": 100}]})).unwrap();
    s.execute("wholejob.settings", &json!({"markups": {"drywall": 20}})).unwrap();
    let e = s.execute("wholejob.estimate", &json!({})).unwrap();
    let t = |n: &str| e["trades"].as_array().unwrap().iter().find(|x| x["trade"] == n).unwrap().clone();
    assert_eq!(t("drywall")["contract_value"], json!(3000.0));
    assert_eq!(t("drywall")["unpriced"].as_array().unwrap().len(), 1);
    assert_eq!(t("drywall")["division"], "09");
    assert_eq!(t("fire alarm")["contract_value"], json!(1120.0));
    assert_eq!(t("fire alarm")["division"], "28");
    assert_eq!(e["unpriced_lines"], 1);
    assert_eq!(e["general_conditions"]["total"], json!(82.81), "no weeks: insurance 1% of 4120, bond 1% of 4161.20");
}

#[test]
fn leveling_with_no_complete_bid() {
    let mut s = session();
    s.execute(
        "wholejob.quantities",
        &json!({"trade": "painting", "lines": [{"item": "walls", "unit": "sf", "qty": 100}, {"item": "doors", "unit": "ea", "qty": 4}]}),
    )
    .unwrap();
    s.execute(
        "wholejob.bid",
        &json!({"trade": "painting", "bidder": "A", "total": 500, "lines": [{"item": "walls", "unit": "sf", "qty": 80, "amount": 400}]}),
    )
    .unwrap();
    s.execute(
        "wholejob.bid",
        &json!({"trade": "painting", "bidder": "B", "total": 450, "lines": [{"item": "Doors", "unit": "ea", "qty": 4, "amount": 450}]}),
    )
    .unwrap();
    let lv = s.execute("wholejob.level", &json!({"trade": "painting"})).unwrap();
    assert_eq!(lv["recommended"], json!(null));
    assert!(!lv["note"].as_str().unwrap().is_empty());
    let a = &lv["bids"][0];
    assert_eq!(a["quantity_gaps"][0]["bid_qty"], json!(80.0), "{a}");
    // A's missing doors priced at the other bidder's unit price: 4 × 112.5.
    assert_eq!(a["leveled_total"], json!(950.0));
}

#[test]
fn one_bid_line_covers_at_most_one_model_line() {
    let mut s = session();
    s.execute(
        "wholejob.quantities",
        &json!({"trade": "painting", "lines": [
            {"item": "CEILING PAINT (GYP)", "unit": "sf", "qty": 1000},
            {"item": "CEILING PAINT (ACT)", "unit": "sf", "qty": 500},
            {"item": "PAINT ABOVE TILE", "unit": "sf", "qty": 200},
        ]}),
    )
    .unwrap();
    // A: one generic ceiling line; it names two model lines, so it covers neither.
    s.execute(
        "wholejob.bid",
        &json!({"trade": "painting", "bidder": "A", "total": 1000, "lines": [
            {"item": "Ceiling paint", "unit": "sf", "qty": 1500, "amount": 800},
            {"item": "Paint above tile", "unit": "sf", "qty": 200, "amount": 200},
        ]}),
    )
    .unwrap();
    // B: every line by name, with a one-letter exclusion that must exclude nothing.
    s.execute(
        "wholejob.bid",
        &json!({"trade": "painting", "bidder": "B", "total": 1900, "exclusions": ["a"], "lines": [
            {"item": "Ceiling paint (GYP)", "unit": "sf", "qty": 1000, "amount": 1000},
            {"item": "Ceiling paint (ACT)", "unit": "sf", "qty": 500, "amount": 500},
            {"item": "Paint above tile", "unit": "sf", "qty": 200, "amount": 400},
        ]}),
    )
    .unwrap();
    let lv = s.execute("wholejob.level", &json!({"trade": "painting"})).unwrap();
    let bid = |n: &str| lv["bids"].as_array().unwrap().iter().find(|x| x["bidder"] == n).unwrap().clone();
    let a = bid("A");
    assert_eq!(a["complete"], false, "{a}");
    assert_eq!(
        a["missing_scope"],
        json!([{"item": "CEILING PAINT (GYP)", "why": "ambiguous — review"}, {"item": "CEILING PAINT (ACT)", "why": "ambiguous — review"}]),
        "{a}"
    );
    let b = bid("B");
    assert_eq!(b["complete"], true, "{b}");
    assert_eq!(b["missing_scope"], json!([]), "the exclusion \"a\" excludes nothing: {b}");
    assert_eq!(lv["recommended"], "B", "A is lower but not complete: {lv}");
    // A's ambiguous lines priced at B's unit price (the only other bid): 1000 × 1 + 500 × 1.
    assert_eq!(a["leveled_total"], json!(2500.0));

    // An exclusion that names two lines leaves both for review; one naming a single line excludes it.
    s.execute(
        "wholejob.bid",
        &json!({"trade": "painting", "bidder": "C", "total": 1900, "exclusions": ["ceiling paint", "above tile"], "lines": [
            {"item": "Ceiling paint (GYP)", "unit": "sf", "qty": 1000, "amount": 1000},
            {"item": "Ceiling paint (ACT)", "unit": "sf", "qty": 500, "amount": 500},
            {"item": "Paint above tile", "unit": "sf", "qty": 200, "amount": 400},
        ]}),
    )
    .unwrap();
    let lv = s.execute("wholejob.level", &json!({"trade": "painting"})).unwrap();
    let c = lv["bids"].as_array().unwrap().iter().find(|x| x["bidder"] == "C").unwrap().clone();
    assert_eq!(
        c["missing_scope"],
        json!([
            {"item": "CEILING PAINT (GYP)", "why": "ambiguous — review"},
            {"item": "CEILING PAINT (ACT)", "why": "ambiguous — review"},
            {"item": "PAINT ABOVE TILE", "why": "excluded"},
        ]),
        "{c}"
    );
}

#[test]
fn outlier_prices_are_flagged_against_the_median() {
    let mut s = session();
    s.execute(
        "wholejob.quantities",
        &json!({"trade": "flooring", "lines": [{"item": "carpet tile", "unit": "sf", "qty": 1000}, {"item": "rubber base", "unit": "lf", "qty": 400}]}),
    )
    .unwrap();
    // Unit prices: carpet 4.00 / 4.00 / 6.40 (+60 %), base 2.50 / 2.50 / 3.00 (+20 %).
    for (bidder, carpet, base) in [("A", 4000.0, 1000.0), ("B", 4000.0, 1000.0), ("C", 6400.0, 1200.0)] {
        s.execute(
            "wholejob.bid",
            &json!({"trade": "flooring", "bidder": bidder, "total": carpet + base, "lines": [
                {"item": "Carpet tile", "unit": "sf", "qty": 1000, "amount": carpet},
                {"item": "Rubber base", "unit": "lf", "qty": 400, "amount": base},
            ]}),
        )
        .unwrap();
    }
    let lv = s.execute("wholejob.level", &json!({"trade": "flooring"})).unwrap();
    let bid = |n: &str| lv["bids"].as_array().unwrap().iter().find(|x| x["bidder"] == n).unwrap().clone();
    assert_eq!(bid("C")["outlier_prices"], json!([{"item": "carpet tile", "unit_price": 6.4, "median": 4.0}]), "{lv}");
    assert_eq!(bid("A")["outlier_prices"], json!([]));
    assert_eq!(bid("B")["outlier_prices"], json!([]));
}

#[test]
fn electrical_stays_private_at_the_pack_level() {
    let mut s = session();
    let dir = std::env::temp_dir().join(format!("cadaddy-wholejob-privacy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let public = dir.join("public.jsonl").display().to_string();
    let private = dir.join("private.jsonl").display().to_string();
    let _ = (std::fs::remove_file(&public), std::fs::remove_file(&private));

    s.execute("wholejob.history", &json!({"pack": public, "trade": "roofing", "building_type": "hotel", "sf": 1000, "amount": 8000})).unwrap();
    let before = std::fs::read(&public).unwrap();
    // An electrical row, even flagged private, never goes into a public pack; nor an outcome.
    assert!(s.execute("wholejob.history", &json!({"pack": public, "private": true, "trade": "electrical", "sf": 1000, "amount": 20000})).is_err());
    assert!(s.execute("wholejob.outcome", &json!({"pack": public, "private": true, "trade": "roofing", "award": 8000})).is_err());
    assert_eq!(std::fs::read(&public).unwrap(), before, "the public pack is unchanged");

    s.execute(
        "wholejob.history",
        &json!({"pack": private, "private": true, "trade": "electrical", "building_type": "hotel", "sf": 1000, "amount": 20000}),
    )
    .unwrap();
    s.execute(
        "wholejob.history",
        &json!({"pack": private, "private": true, "trade": "roofing", "building_type": "hotel", "sf": 1000, "amount": 9000}),
    )
    .unwrap();
    // A public row never goes into a private pack.
    let before = std::fs::read(&private).unwrap();
    assert!(s.execute("wholejob.history", &json!({"pack": private, "trade": "roofing", "sf": 1000, "amount": 9000})).is_err());
    assert_eq!(std::fs::read(&private).unwrap(), before, "the private pack is unchanged");
    s.execute("wholejob.outcome", &json!({"pack": private, "private": true, "trade": "roofing", "award": 9000})).unwrap();

    // Benchmarks leave private rows out unless asked.
    let bm = s.execute("wholejob.benchmarks", &json!({"packs": [public, private]})).unwrap();
    assert_eq!(bm["benchmarks"], json!([{"trade": "roofing", "count": 1, "per_sf_median": 8.0, "per_sf_min": 8.0, "per_sf_max": 8.0}]), "{bm}");
    assert_eq!(bm["private_rows_left_out"], 2);
    let bm = s.execute("wholejob.benchmarks", &json!({"packs": [public, private], "private": true})).unwrap();
    assert_eq!(
        bm["benchmarks"],
        json!([
            {"trade": "electrical", "count": 1, "per_sf_median": 20.0, "per_sf_min": 20.0, "per_sf_max": 20.0},
            {"trade": "roofing", "count": 2, "per_sf_median": 8.5, "per_sf_min": 8.0, "per_sf_max": 9.0},
        ]),
        "{bm}"
    );
    assert_eq!(bm["private_rows_left_out"], 0);

    // Division 27/28 trades by name are electrical: no public prices, no public history.
    s.execute("wholejob.quantities", &json!({"trade": "low voltage", "lines": [{"item": "data outlet", "unit": "ea", "qty": 10}]})).unwrap();
    assert!(
        s.execute(
            "wholejob.prices",
            &json!({"trade": "low voltage", "quality": "public", "source": "x", "rows": [{"item": "data outlet", "unit": "ea", "unit_cost": 50}]})
        )
        .is_err()
    );
    let other = dir.join("other.jsonl").display().to_string();
    assert!(s.execute("wholejob.history", &json!({"pack": other, "trade": "security", "sf": 1000, "amount": 5000})).is_err());
    assert!(!std::path::Path::new(&other).exists());
    let _ = std::fs::remove_dir_all(&dir);
}
