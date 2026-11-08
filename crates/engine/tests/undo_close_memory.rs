//! A drawing's undo history and its closing, counted in bytes the program holds (B9-023: a
//! 30-minute session on drawings with dynamic blocks, associative hatches, dimensions and
//! multileaders grew to 3.2 GB and closing didn't give it back).
//!
//! What held it: every undo step keeps the whole old `Drawing` struct, and the block table, the
//! constraint list and the file's own handles were copied into every step (0.4 MB a step on a
//! drawing with 1,100 blocks, 2,000 steps deep), while the step's recorded cost counted only its
//! changed objects, so the memory budget never saw them. They are shared now until an edit
//! changes them, and a step's recorded cost counts what it holds besides objects. Closing was
//! never at fault: the bytes held after a close are the same however many cycles ran (the process's
//! resident size is the allocator's free lists as much as the program's bytes, so this counts the
//! allocator's live bytes instead).
//!
//! A large drawing too (Q6278): one edit in a drawing of a million objects held ~47 MB, a copy
//! of the drawing's project texts and part of its handle index the undo budget didn't count.
//!
//! The counting allocator is the whole test binary's, so this is one test that runs its parts in
//! turn.
#![cfg(not(target_arch = "wasm32"))]

use cadaddy_engine::Session;
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Block, Circle, Entity, EntityKind, EntityStore, Handle};
use serde_json::json;

#[global_allocator]
static ALLOC: cadaddy_memcount::Counting = cadaddy_memcount::Counting::new();

/// Blocks in the drawing (the measured drawing had 1,133).
const BLOCKS: usize = 600;
/// Cycles of open, edit, undo, redo, close.
const CYCLES: usize = 6;
/// Edits per cycle (each of the kinds below, over and over).
const ROUNDS: usize = 30;
/// What the bytes held after a close may grow from one cycle to the next (the command log and
/// such, bounded and small): far under what one cycle's undo history held (megabytes).
const GROWTH_PER_CYCLE: usize = 16 * 1024;
/// The most one undo step may hold on this drawing (measured: 16 KB, mostly the header; each
/// step copying the block table as well would add the table's size).
const BYTES_PER_STEP: usize = 24 * 1024;

fn fresh() -> Session {
    let mut s = Session::new();
    fill(&mut s);
    s
}

/// Hundreds of blocks and a few hundred objects of every kind the soak used.
fn fill(s: &mut Session) {
    let d = s.doc_mut().unwrap();
    for i in 0..BLOCKS {
        let name = format!("BLK{i}");
        let mut b = Block {
            xref_overlay: false,
            xref_status: Default::default(),
            xref_reason: String::new(),
            name: name.clone(),
            annotative: false,
            base: Vec3::default(),
            entities: EntityStore::new(),
            description: String::new(),
            anonymous: false,
            xref_path: None,
            explodable: true,
            units: 0,
            array: None,
        };
        b.entities.push(Entity::new(Handle(1_000_000 + i as u64), EntityKind::Circle(Circle { center: Vec3::default(), radius: 1.0 })));
        d.blocks.insert(name, std::sync::Arc::new(b));
    }
    for i in 0..150 {
        let x = (i % 15) as f64 * 40.0;
        let y = (i / 15) as f64 * 40.0;
        s.execute("line", &json!({"points": [[x, y], [x + 30.0, y + 10.0]]})).unwrap();
        if i % 5 == 0 {
            s.execute("mleader", &json!({"points": [[x, y], [x + 8.0, y + 8.0]], "text": "ML"})).unwrap();
            s.execute("dimlinear", &json!({"p1": [x, y], "p2": [x + 30.0, y], "at": [x + 15.0, y - 6.0]})).unwrap();
            s.execute("circle", &json!({"center": [x + 10.0, y + 10.0], "radius": 3.0})).unwrap();
            s.execute("hatch", &json!({"points": [[x + 10.0, y + 10.0]], "pattern": "ANSI31", "associative": true})).unwrap();
        }
        if i % 7 == 0 {
            s.execute("insert", &json!({"name": format!("BLK{}", i % BLOCKS), "at": [x, y]})).unwrap();
        }
    }
}

fn handles(s: &Session, n: usize) -> Vec<String> {
    let store = s.doc().unwrap().space(&cadaddy_engine::doc::Space::Model).unwrap();
    store.iter().take(n).map(|e| format!("{:X}", e.handle.0)).collect()
}

/// One cycle's edits: moves and new objects, each undone and redone.
fn edit(s: &mut Session) {
    let hs = handles(s, 10);
    for round in 0..ROUNDS {
        let mut done = 0;
        for h in &hs {
            if s.execute("move", &json!({"handles": [h], "from": [0, 0], "to": [1, 1]})).is_ok() {
                done += 1;
            }
        }
        let y = round as f64;
        for (cmd, args) in [
            ("line", json!({"points": [[0, y], [5, y]]})),
            ("mleader", json!({"points": [[0, y], [4, y + 4.0]], "text": "N"})),
            ("insert", json!({"name": "BLK1", "at": [y, y]})),
        ] {
            if s.execute(cmd, &args).is_ok() {
                done += 1;
            }
        }
        s.execute("undo", &json!({"count": done})).unwrap();
        s.execute("redo", &json!({"count": done})).unwrap();
    }
}

#[test]
fn undo_holds_what_it_counts_and_close_gives_it_all_back() {
    // --- Closing: the bytes held after each close don't grow cycle over cycle.
    let mut s = Session::new();
    let mut held = Vec::new();
    for _ in 0..CYCLES {
        s.execute("new", &json!({})).unwrap();
        fill(&mut s);
        edit(&mut s);
        s.execute("closeall", &json!({"discard": true})).unwrap();
        // The session's own history (command line log, replay journal) is bounded and meant to
        // outlive a drawing; it is not what this counts.
        s.log.clear();
        s.journal.clear();
        held.push(ALLOC.live());
    }
    // The first cycle sets up one-time state (caches, thread-local buffers): compare the rest.
    let (first, last) = (held.get(1).copied().unwrap_or(0), held.last().copied().unwrap_or(0));
    assert!(last <= first + GROWTH_PER_CYCLE * (CYCLES - 2), "bytes held after each close: {held:?}");

    // --- Undo: what a step holds, and what it says it holds.
    let mut s = fresh();
    let before = ALLOC.live();
    let hs = handles(&s, 1);
    let steps = 200;
    for i in 0..steps {
        let to = if i % 2 == 0 { [1, 1] } else { [0, 0] };
        s.execute("move", &json!({"handles": hs, "from": [0, 0], "to": to})).unwrap();
    }
    let held = ALLOC.live().saturating_sub(before);
    let st = s.state().unwrap();
    let said: usize = st.undo.iter().map(|x| x.bytes).sum();
    assert!(held / steps <= BYTES_PER_STEP, "{} bytes a step on a drawing with {BLOCKS} blocks", held / steps);
    assert!(said >= held / 2, "undo says its {steps} steps hold {said} bytes, they hold {held}: the budget can't see the memory");

    large_drawing_edits_hold_what_they_change();
}

/// Objects in the large drawing, and the size of each of its two project texts (a whole job
/// keeps a building spec and an agent log of many megabytes beside a million objects).
const LARGE_OBJECTS: usize = 200_000;
const LARGE_TEXT: usize = 2 * 1024 * 1024;
/// The most one small edit (a line) may hold on the large drawing: its own object, the chunk
/// and index shard it touched and the drawing's header and tables, not a share of the whole.
const LARGE_BYTES_PER_EDIT: usize = 160 * 1024;

/// A drawing of [`LARGE_OBJECTS`] points with a building spec and agent state of
/// [`LARGE_TEXT`] each.
fn large() -> Session {
    use cadaddy_engine::doc::{Point, geom::Vec2};
    let mut s = Session::new();
    let d = s.doc_mut().unwrap();
    let items = (0..LARGE_OBJECTS as u64)
        .map(|i| {
            Entity::new(Handle(0x10_0000 + i), EntityKind::Point(Point { p: Vec2::new((i % 1000) as f64, (i / 1000) as f64).to3(0.0), angle: 0.0 }))
        })
        .collect();
    d.model.push_many(items);
    d.handseed = 0x10_0000 + LARGE_OBJECTS as u64;
    let text = |key: &str| json!({ key: "x".repeat(LARGE_TEXT) }).to_string();
    d.building_spec = Some(text("note")).into();
    d.agent = Some(text("notes")).into();
    s
}

/// The bytes held once the undo thread has freed what it was handed (it frees dropped steps off
/// the command path), waiting up to a few seconds for them to come down to `want`.
fn settled(want: usize) -> usize {
    let mut live = ALLOC.live();
    for _ in 0..200 {
        if live <= want {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        live = ALLOC.live();
    }
    live
}

/// Q6278: in a drawing with a million objects every edit held another ~47 MB, whatever it was
/// (a line, a move, the current layer), and the undo budget never saw it: every step kept its
/// own copy of the drawing's project texts (the building spec, the agent's log) and a
/// 64th of the handle index, and counted neither.
fn large_drawing_edits_hold_what_they_change() {
    // What an edit holds follows what it changed, not the drawing's size.
    let mut s = large();
    s.execute("line", &json!({"points": [[0, -10], [120, -10]]})).unwrap();
    let before = ALLOC.live();
    let edits = 40;
    for i in 0..edits {
        let y = -20.0 - i as f64;
        s.execute("line", &json!({"points": [[0, y], [120, y]]})).unwrap();
    }
    let held = ALLOC.live().saturating_sub(before);
    let st = s.state().unwrap();
    let said: usize = st.undo.iter().skip(1).map(|x| x.bytes).sum();
    assert!(held / edits <= LARGE_BYTES_PER_EDIT, "{} bytes an edit on a drawing of {LARGE_OBJECTS} objects", held / edits);
    assert!(said >= held / 2, "undo says its {edits} steps hold {said} bytes, they hold {held}: the budget can't see the memory");
    for (cmd, p) in [("layer.current", json!({"name": "0"})), ("move", json!({"handles": ["100000"], "from": [0, 0], "to": [1, 1]}))] {
        let before = ALLOC.live();
        s.execute(cmd, &p).unwrap();
        let held = ALLOC.live().saturating_sub(before);
        assert!(held <= LARGE_BYTES_PER_EDIT, "{cmd} held {held} bytes on a drawing of {LARGE_OBJECTS} objects");
    }
    drop(s);

    // Many edits stay within the undo budget plus the edits' own objects.
    let mut s = large();
    let budget = 1024 * 1024;
    s.state_mut().unwrap().undo_ctl.budget = budget;
    s.execute("line", &json!({"points": [[0, -10], [120, -10]]})).unwrap();
    let before = ALLOC.live();
    let edits = 100;
    for i in 0..edits {
        let y = -20.0 - i as f64;
        s.execute("line", &json!({"points": [[0, y], [120, y]]})).unwrap();
    }
    s.log.clear();
    s.journal.clear();
    // Each line leaves its object in the drawing and a line in the activity log: a few KB.
    let bound = before + budget + edits * 8 * 1024;
    let live = settled(bound);
    assert!(live <= bound, "{edits} edits under a {budget}-byte undo budget hold {} bytes", live.saturating_sub(before));
}
