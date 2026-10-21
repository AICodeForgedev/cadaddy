//! Prompt-sequence fuzzing: every interactive command, on a few drawings, gets random but
//! plausible input — points (picked near the drawing and typed: `x,y`, `@dx,dy`, `@d<a`), numbers,
//! the prompt's keywords (whole and abbreviated), object picks, Enter, Esc, `U` and now and then
//! an agent's JSON call from outside — and then:
//!
//! - no panic (the session's guard catches them; a panic hook counts them anyway);
//! - Esc ×3 leaves no command running (nor one suspended under a transparent command);
//! - no selection leaks: the selection names only objects that exist, and Esc at `Command:`
//!   clears it;
//! - undo returns the drawing to the start exactly (as many undos as steps the command added),
//!   and redo brings back what the command made; a change that added no undo step is an error.
//!
//! Seeded and reproducible: each case's seed comes from `PROMPTFUZZ_SEED` (default 1), the
//! drawing and the command; a failure prints the inputs that made it. The CI run is 30 sequences
//! per command and drawing (about 33,000, some seconds); the nightly mode runs more:
//! `PROMPTFUZZ_ROUNDS=600 PROMPTFUZZ_SEED=<date>` (sequences per command and drawing, first
//! seed), and `PROMPTFUZZ_CMD=trim` runs one command.

use std::collections::HashSet;
use std::sync::Mutex;

use cadaddy_engine::doc::{Drawing, Handle};
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::{Input, Session, command_specs};
use serde_json::json;

mod fuzzkit;
use fuzzkit::{Rng, SKIP, drawings, handles, hash};

/// Commands that save (SAVEAS, DXFOUT): saving stamps the save date (TDUPDATE, and TDCREATE the first time) into
/// the drawing, which is not an edit and has no undo step. Only those two may change.
const SAVES: &[&str] = &["saveas", "dxfout"];

fn without_save_dates(d: &Drawing) -> Drawing {
    let mut d = d.clone();
    d.header.vars.remove("TDUPDATE");
    d.header.vars.remove("TDCREATE");
    d
}

/// Inputs per sequence, at most.
const STEPS: usize = 24;

// The case being run on this thread, for the panic hook.
thread_local! {
    static CASE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Panics seen (caught by the session's guard or not): the case and the message.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Inputs given, and sequences that changed the drawing, over all sequences (the summary line).
static INPUTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static CHANGED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn install_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let case = CASE.with(|c| c.borrow().clone());
            if case.is_empty() {
                default(info);
            } else if let Ok(mut p) = PANICS.lock() {
                p.push(format!("{case}: {info}"));
            }
        }));
    });
}

/// Where two drawings first differ: the line of their pretty `Debug` form, with the field path.
fn what_changed(a: &Drawing, b: &Drawing) -> String {
    let (a, b) = (format!("{a:#?}"), format!("{b:#?}"));
    let mut path: Vec<String> = Vec::new();
    for (x, y) in a.lines().zip(b.lines()) {
        if x != y {
            return format!("at {} — {} → {}", path.join("."), x.trim(), y.trim());
        }
        let depth = x.len() - x.trim_start().len();
        let name = x.trim().split([':', ' ']).next().unwrap_or_default().to_string();
        path.truncate(depth / 4);
        if x.trim_end().ends_with(['{', '[', '(']) {
            path.push(name);
        }
    }
    "in length".into()
}

fn exists(d: &Drawing, h: Handle) -> bool {
    d.entity(h).is_some()
}

/// A number as a user might type it: mostly ordinary, sometimes an edge.
fn number(r: &mut Rng) -> String {
    match r.below(10) {
        0 => "0".into(),
        1 => "-2".into(),
        2 => "1e6".into(),
        3 => "0.001".into(),
        _ => format!("{}", (r.unit() * 20.0).round() / 2.0 + 0.5),
    }
}

/// One plausible input for the prompt showing: what the user does, as text for the report.
fn step(s: &mut Session, r: &mut Rng, area: (Vec2, Vec2)) -> String {
    let prompt = s.current_prompt();
    let accept = prompt.as_ref().map(|p| p.accept).unwrap_or_default();
    let keywords = prompt.as_ref().map(|p| p.keywords.clone()).unwrap_or_default();
    let (lo, hi) = area;
    let point = |r: &mut Rng| Vec2::new(lo.x + (hi.x - lo.x) * r.unit(), lo.y + (hi.y - lo.y) * r.unit());
    let roll = r.below(100);
    let typed = |s: &mut Session, text: String| {
        let _ = s.cmdline(&text);
        format!("type {text:?}")
    };
    match roll {
        0..=2 => {
            s.cancel();
            "Esc".into()
        }
        3..=11 => {
            let _ = s.input(Input::Enter);
            "Enter".into()
        }
        // An agent's call from outside while the command runs (its own undo step).
        96..=97 => {
            let p = point(r);
            let _ = s.execute("circle", &json!({ "center": [p.x, p.y], "radius": 0.5 }));
            format!("execute circle at {:.3},{:.3}", p.x, p.y)
        }
        12..=14 => typed(s, "U".into()),
        15..=34 if !keywords.is_empty() => {
            let k = r.pick(&keywords).cloned().unwrap_or_default();
            // Whole, or abbreviated to its first letters as users type them.
            let k = if r.chance(50) { k.chars().take(1 + r.below(2)).collect() } else { k };
            typed(s, k)
        }
        35..=49 if accept.select => {
            let all = handles(s);
            let picked: Vec<Handle> = (0..1 + r.below(3)).filter_map(|_| r.pick(&all).copied()).collect();
            let _ = s.input(Input::Pick(picked.clone()));
            format!("pick {picked:?}")
        }
        50..=54 if accept.select => typed(s, ["ALL", "L", "P", "W"][r.below(4)].into()),
        55..=64 if accept.number || accept.text => typed(s, number(r)),
        65..=69 if accept.text => typed(s, ["A", "Fuzz", "x y", "%%c", "1/2"][r.below(5)].into()),
        70..=79 => {
            let p = point(r);
            let text = match r.below(3) {
                0 => format!("{:.3},{:.3}", p.x, p.y),
                1 => format!("@{:.2},{:.2}", r.unit() * 10.0 - 5.0, r.unit() * 10.0 - 5.0),
                _ => format!("@{:.2}<{}", r.unit() * 10.0, r.below(360)),
            };
            typed(s, text)
        }
        _ => {
            let p = point(r);
            let _ = s.input(Input::Point(p));
            format!("click {:.3},{:.3}", p.x, p.y)
        }
    }
}

/// One sequence: `None` when it held, or what went wrong with the inputs that made it.
fn case(make: fn() -> Session, drawing: &str, cmd: &str, seed: u64) -> Option<String> {
    run(make, drawing, cmd, seed).unwrap_or_else(|| Some("the session has no drawing".into()))
}

/// [`case`]; `None` when the session lost its drawing.
fn run(make: fn() -> Session, drawing: &str, cmd: &str, seed: u64) -> Option<Option<String>> {
    let mut s = make();
    let mut r = Rng(seed ^ hash(drawing) ^ hash(cmd).rotate_left(17));
    let uid = s.state().ok()?.uid;
    let start = s.doc().ok()?.clone();
    let undo_start = s.state().ok()?.undo.len();
    let area = {
        let b = start.extents(&s.space());
        let (lo, hi) = if b.min.x.is_finite() && b.max.x > b.min.x { (b.min, b.max) } else { (Vec2::new(-10.0, -10.0), Vec2::new(30.0, 20.0)) };
        let pad = (hi - lo) * 0.1;
        (lo - pad, hi + pad)
    };
    let mut log = vec![format!("start {cmd}")];
    if s.start(cmd).is_err() {
        // Refused here (nothing to act on): fine, as long as nothing changed.
        return Some(s.running.is_some().then(|| format!("{cmd} refused but left running")));
    }
    for _ in 0..1 + r.below(STEPS) {
        if s.running.is_none() {
            break;
        }
        log.push(step(&mut s, &mut r, area));
        INPUTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let fail = |why: String, log: &[String]| Some(Some(format!("{why}\n      inputs: {}", log.join(" | "))));
    for _ in 0..3 {
        s.cancel();
    }
    if s.running.is_some() || s.suspended.is_some() {
        return fail(format!("still running after Esc ×3: {}", s.prompt_text()), &log);
    }
    // The Block Editor is a mode, not a command: leave it, discarding what wasn't saved into the
    // block (BSAVE inside it is an undo step of the drawing, checked below).
    if s.state().ok()?.block_edit.is_some() {
        log.push("BCLOSE discard".into());
        if let Err(e) = s.execute("bclose", &json!({ "save": false })) {
            return fail(format!("BCLOSE refused: {e}"), &log);
        }
    }
    // Commands that open another drawing leave it; the start drawing is checked.
    if s.state().ok()?.uid != uid {
        let Some(i) = s.docs.iter().position(|d| d.uid == uid) else { return fail("the start drawing was closed".into(), &log) };
        s.active = i;
    }
    let norm = |d: &Drawing| if SAVES.contains(&cmd) { without_save_dates(d) } else { d.clone() };
    let (start, doc) = (norm(&start), norm(s.doc().ok()?));
    if let Some(h) = s.selection().into_iter().find(|h| !exists(&doc, *h)) {
        return fail(format!("the selection names {h:?}, which isn't in the drawing"), &log);
    }
    if !s.selection().is_empty() {
        return fail(format!("Esc at Command: left {} selected", s.selection().len()), &log);
    }
    let steps = s.state().ok()?.undo.len().saturating_sub(undo_start);
    if steps == 0 && doc != start {
        return fail(format!("the drawing changed without an undo step: {}", what_changed(&start, &doc)), &log);
    }
    if steps > 0 {
        CHANGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    for _ in 0..steps {
        let _ = s.undo();
    }
    if norm(s.doc().ok()?) != start {
        return fail(format!("{steps} undo(s) didn't bring the drawing back to the start: {}", what_changed(&start, s.doc().ok()?)), &log);
    }
    for _ in 0..steps {
        let _ = s.redo();
    }
    if norm(s.doc().ok()?) != doc {
        return fail(format!("{steps} redo(s) didn't bring back what the command made: {}", what_changed(&doc, s.doc().ok()?)), &log);
    }
    Some(None)
}

#[test]
fn the_drawings_have_what_the_commands_act_on() {
    for (name, make) in drawings() {
        let s = make();
        let kinds: HashSet<String> = s
            .doc()
            .ok()
            .and_then(|d| d.space(&s.space()))
            .map(|st| st.iter().map(|e| format!("{:?}", e.kind).split(['(', ' ', '{']).next().unwrap_or_default().to_string()).collect())
            .unwrap_or_default();
        let want: &[&str] = match name {
            "empty" => &[],
            "bracket" => &["Line", "Circle", "Hatch", "Dimension"],
            _ => &["Line", "LwPolyline", "Circle", "Arc", "Text", "Insert", "Dimension", "Hatch"],
        };
        let missing: Vec<&&str> = want.iter().filter(|k| !kinds.contains(**k)).collect();
        assert!(missing.is_empty(), "{name} lacks {missing:?}; has {kinds:?}");
    }
}

#[test]
fn every_interactive_command_survives_random_input() {
    install_hook();
    // Anything a command writes to a relative path (SAVEAS "1") lands in a scratch folder.
    let scratch = std::env::temp_dir().join(format!("cadaddy-promptfuzz-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    std::env::set_current_dir(&scratch).unwrap();
    let env = |k: &str| std::env::var(k).ok();
    let seed: u64 = env("PROMPTFUZZ_SEED").and_then(|v| v.parse().ok()).unwrap_or(1);
    let rounds: u64 = env("PROMPTFUZZ_ROUNDS").and_then(|v| v.parse().ok()).unwrap_or(30);
    let only = env("PROMPTFUZZ_CMD");
    let skip: HashSet<&str> = SKIP.iter().map(|(id, _)| *id).collect();
    let cmds: Vec<&'static str> = command_specs()
        .iter()
        .filter(|c| c.interactive.is_some() && !skip.contains(c.id))
        .map(|c| c.id)
        .filter(|id| only.as_deref().is_none_or(|o| o.eq_ignore_ascii_case(id)))
        .collect();
    let started = std::time::Instant::now();
    let failures = Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let jobs: Vec<(&str, fn() -> Session, &str, u64)> = drawings()
        .into_iter()
        .flat_map(|(name, make)| cmds.iter().flat_map(move |c| (0..rounds).map(move |k| (name, make, *c, seed.wrapping_add(k)))))
        .collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                while let Some(&(name, make, cmd, seed)) = jobs.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    let label = format!("{name} {cmd} seed {seed}");
                    CASE.with(|c| *c.borrow_mut() = label.clone());
                    let t = std::time::Instant::now();
                    let result =
                        std::panic::catch_unwind(|| case(make, name, cmd, seed)).unwrap_or_else(|_| Some("panicked outside the guard".into()));
                    CASE.with(|c| c.borrow_mut().clear());
                    let slow = t.elapsed().as_secs_f64();
                    if let Some(why) = result {
                        failures.lock().unwrap().push(format!("{label}: {why}"));
                    } else if slow > 5.0 {
                        eprintln!("promptfuzz: {label} took {slow:.1} s");
                    }
                }
            });
        }
    });
    let mut failures = failures.into_inner().unwrap();
    failures.extend(PANICS.lock().unwrap().drain(..).map(|p| format!("panic: {p}")));
    failures.sort();
    eprintln!(
        "promptfuzz: {} sequences ({} commands × {} drawings × {rounds}), {} inputs, {} changed the drawing, in {:.1} s, seed {seed}",
        jobs.len(),
        cmds.len(),
        drawings().len(),
        INPUTS.load(std::sync::atomic::Ordering::Relaxed),
        CHANGED.load(std::sync::atomic::Ordering::Relaxed),
        started.elapsed().as_secs_f64()
    );
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(
        failures.is_empty(),
        "{} failure(s); rerun one with PROMPTFUZZ_CMD=<id> PROMPTFUZZ_SEED=<seed>:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
