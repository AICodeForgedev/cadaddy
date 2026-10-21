//! Stuck prompts: every interactive command is walked forward with plausible answers, and at
//! each prompt on the way it gets, each time from a fresh replay of the walk:
//!
//! - an invalid answer, and an object or point it can't act on: the command ends, or asks again
//!   with a message (a refusal is never silent);
//! - Esc: the command ends;
//! - Enter, a click and a typed value: at least one of them moves it on (a prompt nothing but Esc
//!   gets out of traps the user, as BREAK did on a closed polyline: battle test 0.7.8 #1);
//!
//! and after each, Esc ends whatever is left and a typed `LINE` starts LINE (the next command
//! isn't swallowed). A command never runs without a prompt to answer.
//!
//! Seeded and reproducible like `prompt_fuzz.rs`: `STUCK_SEED` (default 1), `STUCK_WALKS` (walks
//! per command and drawing, default 2), `STUCK_CMD=<id>` for one command.

use std::collections::HashSet;
use std::sync::Mutex;

use cadaddy_engine::doc::{EntityKind, Handle};
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::{Input, Session, command_specs};

mod fuzzkit;
use fuzzkit::{Rng, SKIP, drawings, handles, hash};

/// Prompts walked into each command, at most.
const DEPTH: usize = 6;

/// One input, replayable.
#[derive(Clone, Debug)]
enum Inp {
    Click(Vec2),
    Pick(Vec<Handle>),
    Typed(String),
    Enter,
}

/// Give `i`; whether it was refused with an error (which the command line shows).
fn apply(s: &mut Session, i: &Inp) -> bool {
    match i {
        Inp::Click(p) => s.input(Input::Point(*p)),
        Inp::Pick(hs) => s.input(Input::Pick(hs.clone())),
        Inp::Typed(t) => s.cmdline(t),
        Inp::Enter => s.input(Input::Enter),
    }
    .is_err()
}

/// The prompt as the user sees it, to tell whether an input moved the command on.
fn shown(s: &Session) -> Option<(String, Vec<String>, Option<String>)> {
    s.running.as_ref()?;
    s.current_prompt().map(|p| (p.message, p.keywords, p.default))
}

/// The drawing area the walk clicks in.
fn area(s: &Session) -> (Vec2, Vec2) {
    let b = s.doc().map(|d| d.extents(&s.space())).ok();
    let (lo, hi) = match b {
        Some(b) if b.min.x.is_finite() && b.max.x > b.min.x => (b.min, b.max),
        _ => (Vec2::new(-10.0, -10.0), Vec2::new(30.0, 20.0)),
    };
    let pad = (hi - lo) * 0.1;
    (lo - pad, hi + pad)
}

fn point(r: &mut Rng, (lo, hi): (Vec2, Vec2)) -> Vec2 {
    Vec2::new(lo.x + (hi.x - lo.x) * r.unit(), lo.y + (hi.y - lo.y) * r.unit())
}

/// A plausible answer that moves the command on.
fn advance(s: &Session, r: &mut Rng, last: Option<&Inp>) -> Inp {
    let Some(p) = s.current_prompt() else { return Inp::Enter };
    let a = p.accept;
    if a.select {
        // Pick something, then end the selection.
        return match last {
            Some(Inp::Pick(_)) => Inp::Enter,
            _ => {
                let all = handles(s);
                Inp::Pick((0..1 + r.below(2)).filter_map(|_| r.pick(&all).copied()).collect())
            }
        };
    }
    if !p.keywords.is_empty() && r.chance(30) {
        return if p.default.is_some() { Inp::Enter } else { Inp::Typed(r.pick(&p.keywords).cloned().unwrap_or_default()) };
    }
    if a.point {
        return Inp::Click(point(r, area(s)));
    }
    if a.number {
        return Inp::Typed(format!("{}", 1 + r.below(5)));
    }
    if a.text {
        return Inp::Typed("A".into());
    }
    Inp::Enter
}

/// The session with `cmd` started and `path` given; `None` when it isn't running any more.
fn replay(make: fn() -> Session, cmd: &str, path: &[Inp]) -> Option<Session> {
    let mut s = make();
    s.start(cmd).ok()?;
    for i in path {
        s.running.as_ref()?;
        let _ = apply(&mut s, i);
    }
    s.running.as_ref()?;
    Some(s)
}

/// An object the command may well not act on (text or a hatch), else any.
fn unfit_object(s: &Session) -> Option<Handle> {
    let d = s.doc().ok()?;
    let st = d.space(&s.space())?;
    st.iter().find(|e| matches!(e.kind, EntityKind::Text(_) | EntityKind::Hatch(_))).or_else(|| st.iter().next()).map(|e| e.handle)
}

/// After a probe: Esc ends what is left, and LINE then starts. `None` when it held.
fn ends_and_line_starts(s: &mut Session) -> Option<String> {
    if s.running.is_some() && s.current_prompt().is_none() {
        return Some("running without a prompt".into());
    }
    s.cancel();
    if s.running.is_some() {
        // A transparent command ends first, then the one it interrupted.
        s.cancel();
    }
    if s.running.is_some() || s.suspended.is_some() {
        return Some(format!("Esc didn't end it: {}", s.prompt_text()));
    }
    // Commands that close or replace the drawing leave nothing to draw in.
    s.doc().ok()?;
    let _ = s.cmdline("line");
    let line = s.running.as_ref().map(|r| r.id.clone());
    s.cancel();
    (line.as_deref() != Some("line")).then(|| format!("a typed LINE didn't start (got {line:?}): {}", s.prompt_text()))
}

/// The probes at the prompt reached by `path`: what went wrong, if anything.
fn probes(make: fn() -> Session, cmd: &str, path: &[Inp], r: &mut Rng) -> Vec<String> {
    let mut out = Vec::new();
    let Some(s0) = replay(make, cmd, path) else { return out };
    let Some((message, keywords, _)) = shown(&s0) else {
        out.push("running without a prompt".into());
        return out;
    };
    let accept = s0.current_prompt().map(|p| p.accept).unwrap_or_default();
    let free_text = accept.text && !accept.point && !accept.number && !accept.select && keywords.is_empty();
    let far = {
        let (lo, hi) = area(&s0);
        Vec2::new(hi.x + (hi.x - lo.x) * 50.0 + 1000.0, lo.y - (hi.y - lo.y) * 50.0 - 1000.0)
    };
    let mut tries: Vec<(&str, Inp)> = Vec::new();
    if !free_text {
        tries.push(("an invalid answer", Inp::Typed("qq#7?".into())));
    }
    if accept.select {
        if let Some(h) = unfit_object(&s0) {
            tries.push(("an object it may not act on", Inp::Pick(vec![h])));
        }
    } else if accept.point && message.starts_with("Select") {
        tries.push(("a point on nothing", Inp::Click(far)));
    }
    for (what, inp) in tries {
        let Some(mut s) = replay(make, cmd, path) else { continue };
        let before = s.log.len();
        let doc = s.doc().ok().cloned();
        let refused = apply(&mut s, &inp);
        let same = shown(&s).is_some_and(|(m, k, _)| m == message && k == keywords) && s.doc().ok().cloned() == doc;
        if same && s.log.len() == before && !refused && !matches!(inp, Inp::Pick(_)) {
            out.push(format!("{what} at \"{message}\" was ignored without a word"));
        }
        if let Some(e) = ends_and_line_starts(&mut s) {
            out.push(format!("after {what} at \"{message}\": {e}"));
        }
    }
    // Esc.
    if let Some(mut s) = replay(make, cmd, path) {
        s.cancel();
        if s.running.is_some() && s.suspended.is_none() {
            out.push(format!("Esc at \"{message}\" didn't end it: {}", s.prompt_text()));
        }
        if let Some(e) = ends_and_line_starts(&mut s) {
            out.push(format!("after Esc at \"{message}\": {e}"));
        }
    }
    // Enter, a click and a typed value: something must move it on (selection prompts collect,
    // so they are judged by Enter alone).
    let mut moves: Vec<Inp> = vec![Inp::Enter];
    if !accept.select {
        if accept.point {
            moves.push(Inp::Click(point(r, area(&s0))));
        }
        moves.push(Inp::Typed(if accept.number || accept.point { "2".into() } else { "A".into() }));
    }
    let mut moved = false;
    let mut said: Vec<String> = Vec::new();
    for inp in &moves {
        let Some(mut s) = replay(make, cmd, path) else { continue };
        let doc = s.doc().ok().cloned();
        let before = s.log.len();
        let refused = s.running.is_some() && {
            let r = match inp {
                Inp::Typed(t) => s.cmdline(t).err().map(|e| e.to_string()),
                Inp::Enter => s.input(Input::Enter).err().map(|e| e.to_string()),
                Inp::Click(p) => s.input(Input::Point(*p)).err().map(|e| e.to_string()),
                Inp::Pick(hs) => s.input(Input::Pick(hs.clone())).err().map(|e| e.to_string()),
            };
            said.extend(r.clone());
            r.is_some()
        };
        said.extend(s.log.iter().skip(before).cloned());
        let _ = refused;
        moved |= shown(&s).is_none_or(|(m, k, _)| m != message || k != keywords) || s.doc().ok().cloned() != doc;
        if matches!(inp, Inp::Enter)
            && let Some(e) = ends_and_line_starts(&mut s)
        {
            out.push(format!("after Enter at \"{message}\": {e}"));
        }
    }
    // Prompts that look alike from one step to the next (SUBTRACT's two selections): Enter, again
    // and again, ends the command.
    if !moved && let Some(mut s) = replay(make, cmd, path) {
        // (Once it ends, Enter at Command: would start it again.)
        for _ in 0..3 {
            if s.running.is_none() {
                break;
            }
            let _ = s.input(Input::Enter);
        }
        moved = s.running.is_none();
    }
    if !moved {
        said.dedup();
        out.push(format!("\"{message}\" can't be left but with Esc: Enter, a click and a typed value all ask it again (said: {said:?})"));
    }
    out
}

/// One walk into `cmd`: the findings, each with the inputs that led there.
fn walk(make: fn() -> Session, cmd: &str, seed: u64) -> Vec<String> {
    let mut r = Rng(seed ^ hash(cmd).rotate_left(17));
    let mut findings = Vec::new();
    let mut s = make();
    if s.start(cmd).is_err() || s.running.is_none() {
        // Refused or done at once: the session is still usable.
        if let Some(e) = ends_and_line_starts(&mut s) {
            findings.push(format!("after starting: {e}"));
        }
        return findings;
    }
    let mut path: Vec<Inp> = Vec::new();
    for _ in 0..DEPTH {
        for f in probes(make, cmd, &path, &mut r) {
            findings.push(format!("{f}\n      inputs: {path:?}"));
        }
        let Some(s) = replay(make, cmd, &path) else { break };
        let next = advance(&s, &mut r, path.last());
        path.push(next);
        if replay(make, cmd, &path).is_none() {
            break;
        }
    }
    findings
}

#[test]
fn no_command_traps_the_command_line() {
    // Anything a command writes to a relative path lands in a scratch folder.
    let scratch = std::env::temp_dir().join(format!("cadaddy-stuck-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    std::env::set_current_dir(&scratch).unwrap();
    let env = |k: &str| std::env::var(k).ok();
    let seed: u64 = env("STUCK_SEED").and_then(|v| v.parse().ok()).unwrap_or(1);
    let walks: u64 = env("STUCK_WALKS").and_then(|v| v.parse().ok()).unwrap_or(2);
    let only = env("STUCK_CMD");
    let skip: HashSet<&str> = SKIP.iter().map(|(id, _)| *id).collect();
    let cmds: Vec<&'static str> = command_specs()
        .iter()
        .filter(|c| c.interactive.is_some() && !skip.contains(c.id))
        .map(|c| c.id)
        .filter(|id| only.as_deref().is_none_or(|o| o.eq_ignore_ascii_case(id)))
        .collect();
    let jobs: Vec<(&str, fn() -> Session, &str, u64)> = drawings()
        .into_iter()
        .flat_map(|(name, make)| cmds.iter().flat_map(move |c| (0..walks).map(move |k| (name, make, *c, seed.wrapping_add(k)))))
        .collect();
    let started = std::time::Instant::now();
    let found: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                while let Some(&(name, make, cmd, seed)) = jobs.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    let fs = std::panic::catch_unwind(|| walk(make, cmd, seed)).unwrap_or_else(|_| vec!["panicked outside the guard".into()]);
                    let mut found = found.lock().unwrap();
                    found.extend(fs.into_iter().map(|f| format!("{cmd} ({name}, seed {seed}): {f}")));
                }
            });
        }
    });
    let mut found = found.into_inner().unwrap();
    found.sort();
    found.dedup();
    eprintln!(
        "stuck prompts: {} walks ({} commands × {} drawings × {walks}) in {:.1} s, seed {seed}",
        jobs.len(),
        cmds.len(),
        drawings().len(),
        started.elapsed().as_secs_f64()
    );
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(found.is_empty(), "{} finding(s); rerun one with STUCK_CMD=<id> STUCK_SEED=<seed>:\n  {}", found.len(), found.join("\n  "));
}
