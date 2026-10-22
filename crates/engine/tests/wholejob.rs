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
