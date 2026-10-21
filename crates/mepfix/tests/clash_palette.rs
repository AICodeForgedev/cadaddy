//! Tests for the clash palette data model against Q254 mepfix fixtures:
//! grouping, re-run with preserved statuses, BCF and CSV export on real fixture data.

use cadaddy_mepfix::truth::Truth;
use std::collections::BTreeMap;

/// Build clash items from the fixture truth (as the palette would receive them from the engine).
fn truth_to_items(t: &Truth) -> Vec<Item> {
    t.clashes
        .iter()
        .enumerate()
        .map(|(i, c)| Item {
            id: format!("C{:04}", i + 1),
            kind: c.kind.clone(),
            message: format!("{} clashes with {}", c.a, c.b),
            systems: vec![],
            at: [c.at_ft[0], c.at_ft[1]],
            by_in: c.depth_in,
            handles: vec![c.a.clone(), c.b.clone()],
            status: Status::New,
            trade: String::new(),
            notes: String::new(),
            group_key: if c.kind == "into_structure" { c.b.clone() } else { String::new() },
        })
        .collect()
}

// --- Inline types matching the palette's data model (no ui-egui dependency needed) ---

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Status {
    #[default]
    New,
    Active,
    Resolved,
}

#[derive(Clone, Debug)]
struct Item {
    id: String,
    kind: String,
    message: String,
    systems: Vec<String>,
    at: [f64; 2],
    by_in: f64,
    handles: Vec<String>,
    status: Status,
    trade: String,
    notes: String,
    group_key: String,
}

#[derive(Clone, Debug)]
struct Group {
    #[allow(dead_code)]
    label: String,
    clash_ids: Vec<usize>,
}

/// Group clashes (same algorithm as the palette).
fn group(clashes: &[Item]) -> Vec<Group> {
    let mut groups = Vec::new();
    let mut assigned = vec![false; clashes.len()];

    // By structural element key.
    let mut key_groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, c) in clashes.iter().enumerate() {
        if !c.group_key.is_empty() {
            key_groups.entry(c.group_key.clone()).or_default().push(i);
        }
    }
    for (key, indices) in &key_groups {
        if indices.len() > 1 {
            let label = format!("structure group {key} ({} clashes)", indices.len());
            groups.push(Group { label, clash_ids: indices.clone() });
            for &i in indices {
                if let Some(a) = assigned.get_mut(i) {
                    *a = true;
                }
            }
        }
    }

    // By proximity (2 ft).
    for i in 0..clashes.len() {
        if assigned.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(ci) = clashes.get(i) else { continue };
        let mut nearby = vec![i];
        for j in i + 1..clashes.len() {
            if assigned.get(j).copied().unwrap_or(true) {
                continue;
            }
            if let Some(cj) = clashes.get(j) {
                let dx = ci.at[0] - cj.at[0];
                let dy = ci.at[1] - cj.at[1];
                if (dx * dx + dy * dy).sqrt() < 2.0 {
                    nearby.push(j);
                }
            }
        }
        let label = format!("{}: {}", ci.id, ci.message);
        groups.push(Group { label, clash_ids: nearby.clone() });
        for &idx in &nearby {
            if let Some(a) = assigned.get_mut(idx) {
                *a = true;
            }
        }
    }
    groups
}

/// Re-run preserving statuses.
fn rerun(old: &[Item], new: &mut [Item]) {
    let map: BTreeMap<String, (Status, String, String)> =
        old.iter().map(|c| (format!("{}:{}", c.kind, c.handles.join(",")), (c.status, c.trade.clone(), c.notes.clone()))).collect();
    for c in new.iter_mut() {
        let key = format!("{}:{}", c.kind, c.handles.join(","));
        if let Some((status, trade, notes)) = map.get(&key) {
            c.status = *status;
            c.trade.clone_from(trade);
            c.notes.clone_from(notes);
        }
    }
}

/// CSV export (same algorithm as the palette).
fn export_csv(clashes: &[Item]) -> String {
    let mut out = String::from("ID,Kind,Status,Trade,Systems,Location X,Location Y,Depth (in),Message,Notes\n");
    for c in clashes {
        let status = match c.status {
            Status::New => "New",
            Status::Active => "Active",
            Status::Resolved => "Resolved",
        };
        let systems = c.systems.join("; ");
        let notes = c.notes.replace('"', "\"\"");
        let message = c.message.replace('"', "\"\"");
        out.push_str(&format!(
            "{},{},{},{},\"{}\",{:.3},{:.3},{:.2},\"{}\",\"{}\"\n",
            c.id, c.kind, status, c.trade, systems, c.at[0], c.at[1], c.by_in, message, notes,
        ));
    }
    out
}

#[test]
fn fixtures_produce_plantable_clashes() {
    // Seeds 1..=10 all have planted clashes; the palette should show them.
    for seed in 1..=10 {
        let t = cadaddy_mepfix::truth_for(seed);
        assert!(!t.clashes.is_empty(), "seed {seed}: no clashes");
        assert!(t.clashes.iter().any(|c| c.planted), "seed {seed}: no planted clashes");
        let items = truth_to_items(&t);
        assert_eq!(items.len(), t.clashes.len());
        // Every clash has a position and positive depth (or zero for touching).
        for it in &items {
            assert!(it.at[0].is_finite() && it.at[1].is_finite(), "seed {seed}: bad position on {}", it.id);
            assert!(it.by_in >= 0.0, "seed {seed}: negative depth on {}", it.id);
        }
    }
}

#[test]
fn grouping_reduces_item_count_on_real_fixtures() {
    let t = cadaddy_mepfix::truth_for(5);
    let items = truth_to_items(&t);
    let groups = group(&items);
    // Every clash is in exactly one group.
    let total: usize = groups.iter().map(|g| g.clash_ids.len()).sum();
    assert_eq!(total, items.len(), "grouping lost or duplicated clashes");
    // At least some grouping should happen when there are structure clashes.
    let structure_count = items.iter().filter(|c| c.kind == "into_structure").count();
    if structure_count > 1 {
        assert!(groups.len() < items.len(), "expected grouping to reduce item count");
    }
}

#[test]
fn rerun_preserves_user_annotations() {
    let t = cadaddy_mepfix::truth_for(3);
    let mut items = truth_to_items(&t);
    // Annotate first two clashes.
    if let Some(c) = items.get_mut(0) {
        c.status = Status::Active;
        c.trade = "Mechanical".into();
        c.notes = "Under review".into();
    }
    if let Some(c) = items.get_mut(1) {
        c.status = Status::Resolved;
        c.notes = "Rerouted the duct".into();
    }
    // "Re-detect" — same truth, same clashes.
    let mut new_items = truth_to_items(&t);
    rerun(&items, &mut new_items);
    assert_eq!(new_items[0].status, Status::Active);
    assert_eq!(new_items[0].trade, "Mechanical");
    assert_eq!(new_items[0].notes, "Under review");
    assert_eq!(new_items[1].status, Status::Resolved);
    assert_eq!(new_items[1].notes, "Rerouted the duct");
}

#[test]
fn csv_export_contains_all_clashes() {
    let t = cadaddy_mepfix::truth_for(7);
    let items = truth_to_items(&t);
    let csv = export_csv(&items);
    let lines: Vec<&str> = csv.lines().collect();
    // Header + one row per clash.
    assert_eq!(lines.len(), items.len() + 1, "CSV should have header + {} rows", items.len());
    assert!(lines[0].starts_with("ID,"));
    for it in &items {
        assert!(csv.contains(&it.id), "CSV missing {}", it.id);
    }
}

#[test]
fn bcf_guid_determinism() {
    // The same clash ID must always produce the same GUID.
    let g1 = clash_guid("C0001");
    let g2 = clash_guid("C0001");
    assert_eq!(g1, g2);
    // Different IDs produce different GUIDs.
    let g3 = clash_guid("C0002");
    assert_ne!(g1, g3);
    // Must be valid UUID format.
    assert_eq!(g1.len(), 36);
    assert_eq!(g1.chars().filter(|c| *c == '-').count(), 4);
}

fn clash_guid(id: &str) -> String {
    let mut h: u64 = 0x517C_C1B7_2722_0A95;
    for b in id.as_bytes() {
        h = h.wrapping_mul(0x0100_0000_01B3).wrapping_add(u64::from(*b));
    }
    let h2 = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", h >> 32, (h >> 16) & 0xFFFF, h & 0x0FFF, 0x8000 | ((h2 >> 48) & 0x3FFF), h2 & 0xFFFF_FFFF_FFFF,)
}
