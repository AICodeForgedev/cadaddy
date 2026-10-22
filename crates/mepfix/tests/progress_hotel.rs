//! Q332 on the synthetic hotel: the plan from the hotel's devices, luminaires, conduits and panels
//! per floor; scripted progress (level 1 rough complete, level 2 rough half, level 1 makeup, twenty
//! devices trimmed) and timecard hours; earned value, percent complete and the actual production
//! rates match hand calculations; a phase running over is flagged early; the rates go to a private
//! field observations file, not the drawing.

use std::collections::BTreeMap;

use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

const BOX_KINDS: [&str; 5] = ["receptacle", "switch", "data_outlet", "junction_box", "floor_box"];
const RATES: [(&str, f64); 7] =
    [("boxes", 0.25), ("conduit_ft", 0.04), ("terminations", 0.08), ("devices", 0.2), ("luminaires", 0.5), ("panels", 8.0), ("devices_tested", 0.05)];

fn rate(d: &str) -> f64 {
    RATES.iter().find(|(k, _)| *k == d).map(|(_, r)| *r).unwrap()
}

#[test]
fn earned_value_and_production_rates_match_hand_calculations() {
    let t = cadaddy_mepfix::hotel_truth_for(2, HotelSize::Full);
    // The plan per floor, from the hotel.
    let mut plan: BTreeMap<(String, &str), f64> = BTreeMap::new();
    for d in &t.elec_devices {
        let lv = d.level.clone();
        if BOX_KINDS.contains(&d.kind.as_str()) {
            *plan.entry((lv.clone(), "boxes")).or_default() += 1.0;
            *plan.entry((lv.clone(), "terminations")).or_default() += 3.0;
            *plan.entry((lv.clone(), "devices")).or_default() += 1.0;
            *plan.entry((lv.clone(), "devices_tested")).or_default() += 1.0;
        } else if d.kind == "light_fixture" {
            *plan.entry((lv.clone(), "luminaires")).or_default() += 1.0;
            *plan.entry((lv.clone(), "devices_tested")).or_default() += 1.0;
        }
    }
    for c in &t.elec_conduits {
        let len = ((c.to_ft[0] - c.from_ft[0]).powi(2) + (c.to_ft[1] - c.from_ft[1]).powi(2)).sqrt();
        *plan.entry((c.level.clone(), "conduit_ft")).or_default() += (len * 100.0).round() / 100.0;
    }
    for p in &t.elec_panels {
        *plan.entry((p.level.clone(), "panels")).or_default() += 1.0;
    }
    let levels: Vec<String> = t.building.levels.iter().map(|l| l.code.clone()).collect();
    let (l1, l2) = (levels[0].clone(), levels[1].clone());
    assert!(plan.get(&(l1.clone(), "boxes")).copied().unwrap_or(0.0) > 10.0, "{plan:?}");
    let drivers: Vec<Value> = plan.iter().map(|((lv, d), q)| json!({"level": lv, "driver": d, "qty": q})).collect();
    let mut s = cadaddy_engine::Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"units_per_foot": 12, "levels": levels.iter().map(|c| json!({"name": c, "elevation_ft": 0, "height_ft": 10})).collect::<Vec<_>>()}})).unwrap();
    s.execute("progress.plan", &json!({"drivers": drivers})).unwrap();
    // Scripted progress.
    s.execute("progress.mark", &json!({"phase": "rough", "level": l1, "date": "2026-11-02", "by": "Foreman A"})).unwrap();
    s.execute("progress.mark", &json!({"phase": "rough", "level": l2, "fraction": 0.5, "date": "2026-11-09"})).unwrap();
    s.execute("progress.mark", &json!({"phase": "makeup", "level": l1, "date": "2026-11-16"})).unwrap();
    s.execute("progress.mark", &json!({"phase": "trim", "level": l1, "count": 20, "driver": "devices", "date": "2026-11-20"})).unwrap();
    // Timecards: rough on both floors, makeup on 1, trim with no floor.
    let dir = std::env::temp_dir().join(format!("cadaddy-progress-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("timecards.csv");
    let (h_r1, h_r2, h_m1, h_t) = (25.0, 20.0, 10.0, 5.0);
    std::fs::write(
        &csv,
        format!(
            "Date,Employee,Hours,Cost Code,Floor\n2026-10-30,Crew 1,{h_r1},EL-ROUGH,{l1}\n2026-11-06,Crew 1,{h_r2},EL-ROUGH,{l2}\n2026-11-13,Crew 2,{h_m1},Box make-up,{l1}\n2026-11-20,Crew 3,{h_t},Trim out,\n2026-11-21,Crew 3,8,Lunch,\n"
        ),
    )
    .unwrap();
    let r = s.execute("progress.hours", &json!({"path": csv.display().to_string()})).unwrap();
    assert_eq!((r["added"].as_u64(), r["skipped"].as_array().unwrap().len()), (Some(4), 1), "{r}");

    // Hand calculations.
    let q = |lv: &str, d: &str| plan.get(&(lv.to_string(), d)).copied().unwrap_or(0.0);
    let budget = |drv: &[&str]| -> f64 { plan.iter().filter(|((_, d), _)| drv.contains(d)).map(|((_, d), v)| v * rate(d)).sum() };
    let rough_budget = budget(&["boxes", "conduit_ft"]);
    let rough_earned = (q(&l1, "boxes") + 0.5 * q(&l2, "boxes")) * 0.25 + (q(&l1, "conduit_ft") + 0.5 * q(&l2, "conduit_ft")) * 0.04;
    let trim_earned = 20.0f64.min(q(&l1, "devices")) * 0.2;
    let makeup_earned = q(&l1, "terminations") * 0.08;
    let st = s.execute("progress.status", &json!({"date": "2026-11-30", "crew_rate": 85})).unwrap();
    let phase = |p: &str| st["phases"].as_array().unwrap().iter().find(|x| x["phase"] == p).cloned().unwrap();
    let close = |a: f64, b: f64| (a - b).abs() < 0.011;
    let rough = phase("rough");
    assert!(close(rough["budget_hours"].as_f64().unwrap(), rough_budget), "{rough}");
    assert!(close(rough["earned_hours"].as_f64().unwrap(), rough_earned), "{rough}");
    assert!(close(rough["actual_hours"].as_f64().unwrap(), h_r1 + h_r2));
    assert!(close(rough["percent_complete"].as_f64().unwrap(), rough_earned / rough_budget * 100.0));
    assert!(close(rough["earned_value"].as_f64().unwrap(), rough_earned * 85.0));
    assert!(close(rough["cpi"].as_f64().unwrap(), rough_earned / (h_r1 + h_r2)));
    assert!(close(phase("makeup")["earned_hours"].as_f64().unwrap(), makeup_earned));
    assert!(close(phase("trim")["earned_hours"].as_f64().unwrap(), trim_earned));
    // Rough is over its estimate early (projected > budget + 10% at its share complete); makeup isn't.
    let flags: Vec<&str> = st["flags"].as_array().unwrap().iter().map(|f| f.as_str().unwrap()).collect();
    let rough_over = (h_r1 + h_r2) / (rough_earned / rough_budget) > rough_budget * 1.1;
    assert_eq!(flags.iter().any(|f| f.starts_with("rough is running over")), rough_over, "{flags:?}");
    assert!(rough_over, "the script plants rough running over");
    assert!(!flags.iter().any(|f| f.starts_with("makeup")), "{flags:?}");
    // Before the L2 rough mark, rough was half as far along.
    let early = s.execute("progress.status", &json!({"date": "2026-11-05"})).unwrap();
    let er = early["phases"].as_array().unwrap().iter().find(|x| x["phase"] == "rough").unwrap();
    assert!(close(er["earned_hours"].as_f64().unwrap(), q(&l1, "boxes") * 0.25 + q(&l1, "conduit_ft") * 0.04));
    assert!(close(er["actual_hours"].as_f64().unwrap(), h_r1));
    // Actual rates per driver: the phase's hours shared by earned hours, over the units.
    let rates = s.execute("progress.rates", &json!({"date": "2026-11-30"})).unwrap();
    let rr = |d: &str| rates["rates"].as_array().unwrap().iter().find(|x| x["driver"] == d).cloned().unwrap();
    let boxes_in = q(&l1, "boxes") + 0.5 * q(&l2, "boxes");
    let box_hours = (h_r1 + h_r2) * (boxes_in * 0.25) / rough_earned;
    assert!((rr("boxes")["actual_rate"].as_f64().unwrap() - box_hours / boxes_in).abs() < 1e-3, "{}", rr("boxes"));
    let conduit_in = q(&l1, "conduit_ft") + 0.5 * q(&l2, "conduit_ft");
    assert!((rr("conduit_ft")["actual_rate"].as_f64().unwrap() - ((h_r1 + h_r2) - box_hours) / conduit_in).abs() < 1e-3);
    assert!((rr("terminations")["actual_rate"].as_f64().unwrap() - h_m1 / q(&l1, "terminations")).abs() < 1e-3);
    assert!((rr("devices")["actual_rate"].as_f64().unwrap() - h_t / 20.0f64.min(q(&l1, "devices"))).abs() < 1e-3);
    // To the private field observations: appended, never into the drawing.
    let obs = dir.join("private").join("field_observations.csv");
    std::fs::create_dir_all(obs.parent().unwrap()).unwrap();
    let o = s.execute("progress.observe", &json!({"path": obs.display().to_string(), "date": "2026-11-30", "context": {"building_type": "hotel", "wiring_method": "MC", "working_height": "ceiling 9 ft"}})).unwrap();
    let n = o["appended"].as_u64().unwrap() as usize;
    assert_eq!(n, rates["rates"].as_array().unwrap().len());
    s.execute("progress.observe", &json!({"path": obs.display().to_string(), "date": "2026-11-30"})).unwrap();
    let text = std::fs::read_to_string(&obs).unwrap();
    assert_eq!(text.lines().filter(|l| l.starts_with("date,")).count(), 1, "one header");
    assert_eq!(text.lines().count(), 1 + 2 * n);
    assert!(text.lines().nth(1).unwrap().contains(",hotel,MC,ceiling 9 ft,observed"));
    let spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].to_string();
    assert!(!spec.contains("actual_rate") && !spec.contains("field_observations"), "no production rate in the drawing");
}
