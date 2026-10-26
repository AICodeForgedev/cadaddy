//! Q327: a synthetic hotel estimated end to end with invented prices.

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// Three floors of four guest rooms each: per room two duplex receptacles, a switch and a type A
/// luminaire; per floor a branch conduit run with a turn and a 3/0 feeder at 16 ft.
fn hotel(s: &mut Session) {
    let mut tags = Vec::new();
    let mut runs = Vec::new();
    let mut levels = Vec::new();
    for lv in 1..=3 {
        let level = format!("{lv}");
        levels.push(json!({"name": level, "elevation_ft": (lv - 1) * 10, "height_ft": 10}));
        for room in 0..4 {
            let x = 30.0 * f64::from(room);
            let id = |k: &str| format!("{k}-{lv}{:02}", room + 1);
            for (n, dx) in [(1, 2.0), (2, 8.0)] {
                tags.push(json!({"id": id(&format!("R{n}")), "kind": "receptacle", "level": level, "at_ft": [x + dx, 1.0], "fields": {"device_type": {"value": "standard duplex receptacle", "source": "plan"}}}));
            }
            tags.push(json!({"id": id("S"), "kind": "switch", "level": level, "at_ft": [x + 1.0, 10.0], "fields": {"device_type": {"value": "single pole switch", "source": "plan"}}}));
            tags.push(json!({"id": id("L"), "kind": "luminaire", "level": level, "at_ft": [x + 5.0, 6.0], "fields": {"type": {"value": "A", "source": "plan"}, "luminaire": {"value": "LED downlight", "source": "schedule"}}}));
        }
        runs.push(json!({"system": "E-BRANCH-COND", "level": level, "points": [[0, 0], [1200, 0], [1200, 240]], "size_in": [0.75, 0], "bottoms_ft": [9, 9]}));
        let origin = [0.0, 50.0 * f64::from(lv)];
        runs.push(json!({"system": "E-FEEDER", "level": level, "points": [[origin[0] * 12.0, origin[1] * 12.0], [600, origin[1] * 12.0]], "size_in": [2, 0], "bottoms_ft": [16]}));
        tags.push(json!({"id": format!("F-{lv}"), "kind": "homerun", "level": level, "at_ft": origin, "fields": {"conductors": {"value": "4#3/0 + 1#6G", "source": "note"}}}));
    }
    let spec = json!({
        "levels": levels,
        "mep": {
            "systems": [
                {"name": "E-BRANCH-COND", "layer": "E-BRANCH-COND", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9},
                {"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "cable", "size_in": [2, 0], "bottom_ft": 16}
            ],
            "runs": runs
        },
        "tags": tags
    });
    s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
}

/// The hotel's invented baseline: a price per BOM item, a height factor on the feeders, and tool
/// rules for every required item and what the work implies.
fn baseline() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-estimate-hotel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let w = |n: &str, t: &str| std::fs::write(dir.join(n), t).unwrap();
    w(
        "prices.csv",
        "item,size,unit,material,labor_hours,date,source
\"Box, 4\"\" square\",\"2-1/8\"\" deep\",each,2.50,0.20,2026-09-01,Invented Supply
Device plate,1 gang,each,0.75,0.05,2026-09-01,Invented Supply
Plaster ring,1 gang,each,1.00,0.05,2026-09-01,Invented Supply
EMT conduit,3/4\",ft,1.20,0.04,2026-09-01,Invented Supply
Conduit strap,3/4\",each,0.30,0.02,2026-09-01,Invented Supply
EMT 90° elbow,3/4\",each,4.00,0.10,2026-09-01,Invented Supply
EMT connector,3/4\",each,0.80,0.05,2026-09-01,Invented Supply
EMT coupling,3/4\",each,0.60,0.03,2026-09-01,Invented Supply
Insulating bushing (bang-on),,each,1.50,0.05,2026-09-01,Invented Supply
single pole switch,,each,3.00,0.30,2026-09-01,Invented Supply
standard duplex receptacle,,each,2.00,0.30,2026-09-01,Invented Supply
Luminaire type A,LED downlight,each,45.00,0.75,2026-09-01,Invented Lighting Co
Copper ground  green,#6,ft,0.90,0.008,2026-09-01,Invented Supply
THHN copper,#3/0,ft,4.50,0.02,2026-09-01,Invented Supply
"
        .replace("Copper ground  green", "\"Copper ground, green\"")
        .as_str(),
    );
    w("rates.json", "{\"crew_rate\": 90.0}");
    w(
        "factors.csv",
        "factor,kind,multiplier,basis,source,trade,category,item\nHeight 12-16 ft,height,1.25,assumption,invented company table,electrical,wire,THHN\n",
    );
    w(
        "tool_rules.json",
        r#"{"rules": [
            {"when": "large_terminations", "items": [
                {"item": "Crimper with dies", "supply": "rental", "rate": 40, "per": "day", "qty": 1},
                {"item": "Torque wrench", "supply": "owned", "rate": 4, "per": "day", "qty": 1},
                {"item": "Lug, compression 3/0", "supply": "purchase", "price": 12.0, "qty_per": 1},
                {"item": "Lug bolt kit", "supply": "purchase", "price": 1.5, "qty_per": 1}]},
            {"when": "large_pull_ft", "items": [{"item": "Wire tugger", "supply": "rental", "rate": 95, "per": "day", "qty": 1}]},
            {"when": "high_work_ft", "items": [{"item": "Scissor lift", "supply": "rental", "rate": 650, "per": "week", "qty": 1}]},
            {"when": "floors", "items": [
                {"item": "Gang box", "supply": "owned", "rate": 3, "per": "day", "qty_per": 1},
                {"item": "Step ladder, 8'", "supply": "owned", "rate": 1, "per": "day", "qty_per": 1}]},
            {"when": "always", "items": [
                {"item": "Carbide hole saw for 3/4\" conduit", "supply": "purchase", "price": 38.0},
                {"item": "Conduit bender 3/4\"", "supply": "owned", "rate": 2, "per": "day", "qty": 1},
                {"item": "Knockout set", "supply": "owned", "rate": 3, "per": "day", "qty": 1},
                {"item": "Fish tape", "supply": "owned", "rate": 1, "per": "day", "qty": 1},
                {"item": "Wire pulling lube", "supply": "purchase", "price": 14.0, "qty": 1},
                {"item": "Label printer and tape", "supply": "purchase", "price": 120.0, "qty": 1},
                {"item": "Temporary power", "supply": "rental", "rate": 400, "per": "month", "qty": 1}]}
        ]}"#,
    );
    dir
}

#[test]
fn a_synthetic_hotel_is_estimated_end_to_end() {
    let mut s = Session::new();
    hotel(&mut s);
    let dir = baseline();
    let r = s.execute("estimate.import", &json!({ "folder": dir.to_string_lossy() })).unwrap();
    assert_eq!(
        (r["prices"].clone(), r["factors"].clone(), r["tool_rules"].clone(), r["notes"].clone()),
        (json!(14), json!(1), json!(5), json!([])),
        "{r}"
    );
    s.execute(
        "estimate.settings",
        &json!({ "today": "2026-10-21", "duration_days": 30, "tax_pct": 8.25, "overhead_pct": 10, "profit_pct": 8, "contingency_pct": 3, "bond_pct": 1 }),
    )
    .unwrap();
    s.execute("estimate.set_scope", &json!({ "trade": "electrical", "scope": "in_contract", "reason": "Division 26 is ours (bid form)" })).unwrap();
    let st = s.execute("estimate.status", &json!({})).unwrap();
    // Every line priced, every required item and implied tool on a line: nothing open.
    assert_eq!(st["ok"], json!(true), "{}", st["findings"]);
    // By hand: BOM material $4,563.04 + equipment $8,856.00 = $13,419.04; labor 64.518 h × $90 =
    // $5,806.62 (the 3/0 feeders at 0.02 h × 1.25 for height). Tax 8.25% on material, then
    // overhead 10%, profit 8%, contingency 3%, bond 1%, compounding.
    assert_eq!(
        st["totals"],
        json!({ "material": 13419.04, "labor": 5806.62, "labor_hours": 64.52, "direct": 19225.66, "tax": 1107.07, "overhead": 2033.27, "profit": 1789.28, "contingency": 724.66, "bond": 248.8, "total": 25128.74 }),
        "{st}"
    );
    // Equipment: 3 floors × (4 × 3/0) × 2 ends = 24 terminations; the lift a week at a time.
    let q = s.execute("estimate.query", &json!({ "trade": "job", "limit": 100 })).unwrap();
    let cost = |k: &str| q["lines"].as_array().unwrap().iter().find(|l| l["key"] == json!(k)).map(|l| l["cost"].clone());
    assert_eq!(cost("job|hardware|Lug, compression 3/0|"), Some(json!(288.0)), "{q}");
    assert_eq!(cost("job|equipment|Scissor lift|"), Some(json!(3250.0)), "{q}");
    assert_eq!(cost("job|tools|Gang box|"), Some(json!(270.0)), "{q}");
    // The breakdowns each sum to the total to the cent.
    let cents = |v: &Value| v.as_object().unwrap().values().map(|x| (x.as_f64().unwrap() * 100.0).round() as i64).sum::<i64>();
    for k in ["by_level", "by_trade", "by_system"] {
        assert_eq!(cents(&st[k]), 2_512_874, "{k}: {}", st[k]);
    }
    let levels: Vec<&String> = st["by_level"].as_object().unwrap().keys().collect();
    assert_eq!(levels, ["1", "2", "3", "job"], "{}", st["by_level"]);
    // The floors are alike: their shares are equal to within a cent.
    let l = |k: &str| st["by_level"][k].as_f64().unwrap();
    assert!((l("1") - l("2")).abs() <= 0.011 && (l("2") - l("3")).abs() <= 0.011, "{}", st["by_level"]);
    // Every in-contract line explains to its math and at least one model element.
    let all = s.execute("estimate.query", &json!({ "limit": 1000 })).unwrap();
    let lines = all["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 14 + 15, "14 BOM lines and 15 job lines: {all}");
    for line in lines.iter().filter(|l| l["scope"] == json!("in_contract")) {
        let k = line["key"].as_str().unwrap();
        let ex = s.execute("estimate.explain", &json!({ "line": k })).unwrap();
        assert_eq!(ex["value"], line["cost"], "{k}");
        assert!(ex["elements"].as_array().is_some_and(|e| !e.is_empty()), "{k} explains to no element: {ex}");
    }
    let feeder = s.execute("estimate.explain", &json!({ "line": "electrical|wire|THHN copper|#3/0" })).unwrap();
    // 624 ft × 0.02 h × 1.25 = 15.6 h × $90 = $1,404; 686.4 ft × $4.50 = $3,088.80.
    assert_eq!((feeder["labor_hours"].clone(), feeder["value"].clone()), (json!(15.6), json!(4492.8)), "{feeder}");
    // The CSV export reopens with every line and the same total.
    let path = dir.join("hotel-estimate.csv");
    let r = s.execute("estimate.export", &json!({ "path": path.to_string_lossy() })).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let rows: Vec<Vec<String>> = text.lines().map(|l| l.split(',').map(String::from).collect()).collect();
    assert_eq!(rows.len(), lines.len() + 2, "header + lines + TOTAL");
    assert_eq!(r["total"], json!(25128.74));
    assert!(rows.last().unwrap().iter().any(|c| c == "25128.74"), "{text}");
    // The bid summary drafts the assumptions from the factor and rule quantities.
    let b = s.execute("estimate.bid_summary", &json!({})).unwrap();
    assert_eq!((b["total"].clone(), b["open_findings"].clone()), (json!(25128.74), json!(0)), "{b}");
    let a = b["assumptions"].to_string();
    assert!(a.contains("Height 12-16 ft") && a.contains("30-day duration") && a.contains("company takeoff rules"), "{a}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Q5278 MUST 3: estimate.status reports ok:false when every line is unpriced.
#[test]
fn estimate_unpriced_reports_not_ok() {
    let mut s = Session::new();
    hotel(&mut s);
    // Build the BOM (creates lines) but do NOT load any cost table.
    s.execute("model.build", &json!({})).unwrap();
    s.execute("bim.bom", &json!({})).unwrap();
    let st = s.execute("estimate.status", &json!({})).unwrap();
    // Without costs loaded, all lines are unpriced: ok must be false.
    assert_eq!(st["ok"], json!(false), "unpriced lines must make ok:false: {st}");
    assert!(st.get("next_step").is_some() || st.get("unpriced").is_some(),
        "should suggest estimate.import_costs: {st}");
}
