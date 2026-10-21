//! Every command, every option: each command with prompts is typed at the command line on the
//! sample drawing and walked through its prompts again and again, each time answering a keyword
//! option it hasn't answered yet at that prompt (and otherwise a plausible point, number, text
//! or selection), then Escape. After every walk: nothing panicked (the engine's guard reports an
//! internal error), no command is left running, undo returns the drawing exactly to where it was,
//! and no prompt showed an unfilled `{placeholder}`. The keyword options seen but never answered
//! are written to `option-coverage.txt` in cargo's temporary folder for tests (`target/tmp`) (and listed when
//! `CADADDY_SWEEP_REPORT=1`). `CADADDY_SWEEP_ONLY=a,b` sweeps just those commands.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Instant;

use cadaddy_engine::{EngineError, Session, command_specs, sample};

/// Commands that end the session or quit; started, they are not walked.
const SKIP: &[&str] = &["quit", "exit", "close", "closeall", "qquit"];
/// Commands that change the drawing on purpose without an undo step: SAVEAS and DXFOUT record the save
/// (date, fields, version) and LIGHTBAKE keeps its baked light, as AutoCAD keeps a render.
const NOT_UNDOABLE: &[&str] = &["saveas", "dxfout", "lightbake"];
/// Walks per command (each answers at least one new keyword, or stops).
const MAX_WALKS: usize = 64;
/// Inputs per walk before Escape.
const MAX_STEPS: usize = 48;

/// A text with an unfilled `{name}` placeholder.
fn placeholder(t: &str) -> bool {
    let b = t.as_bytes();
    let mut i = 0;
    while let Some(o) = b.get(i..).and_then(|r| r.iter().position(|c| *c == b'{')) {
        let start = i + o + 1;
        let close = b.get(start..).and_then(|r| r.iter().position(|c| *c == b'}'));
        if let Some(c) = close {
            let inner = &t[start..start + c];
            if !inner.is_empty() && inner.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                return true;
            }
        }
        i = start;
    }
    false
}

/// What one command's sweep found.
#[derive(Default)]
struct Found {
    problems: Vec<String>,
    /// (prompt message, keyword) seen and answered.
    seen: BTreeSet<(String, String)>,
    answered: HashSet<(String, String)>,
    /// Where answering a keyword led: (prompt, keyword) → the next prompt's message.
    leads: BTreeMap<(String, String), String>,
    walks: usize,
}

impl Found {
    /// Whether prompt `msg` still has a keyword seen there but not answered.
    fn pending(&self, msg: &str) -> bool {
        self.seen.iter().any(|(m, k)| m == msg && !self.answered.contains(&(m.clone(), k.clone())))
    }
}

fn session() -> Session {
    let mut s = Session::new();
    s.open_drawing(sample::default_sample(), "Sample", None);
    s
}

fn internal(r: &cadaddy_engine::Result<()>) -> Option<String> {
    match r {
        Err(e @ EngineError::Internal(..)) => Some(e.to_string()),
        _ => None,
    }
}

/// A plausible answer to a prompt that has no keyword left to try.
fn answer(s: &mut Session, p: &cadaddy_engine::Prompt, step: usize) -> cadaddy_engine::Result<()> {
    let k = step as f64 + 1.0;
    if p.accept.select && !p.accept.point {
        // "all" selects everything; the next selection prompt ends with Enter.
        return if step.is_multiple_of(2) { s.cmdline("all") } else { s.cmdline("") };
    }
    if p.accept.point {
        return s.cmdline(&format!("{},{}", 3.0 * k, 2.0 * k + 1.0));
    }
    if p.accept.number {
        return s.cmdline("1");
    }
    if p.accept.text {
        return s.cmdline(if step == 0 { "A1" } else { "" });
    }
    s.cmdline("")
}

fn sweep(id: &str) -> Found {
    let mut f = Found::default();
    let mut s = session();
    for walk in 0..MAX_WALKS {
        f.walks = walk + 1;
        let start_doc = Arc::clone(s.state().map(|st| &st.doc).expect("a drawing"));
        let depth = s.state().map_or(0, |st| st.undo.len());
        let before_new = f.answered.len();
        let r = s.cmdline(id);
        if let Some(e) = internal(&r) {
            f.problems.push(format!("{id}: start: {e}"));
            break;
        }
        let mut last = String::new();
        let mut same = 0;
        for step in 0..MAX_STEPS {
            let Some(p) = s.current_prompt() else { break };
            let shown = p.display();
            if placeholder(&shown) {
                f.problems.push(format!("{id}: placeholder in prompt \"{shown}\""));
            }
            for k in &p.keywords {
                f.seen.insert((p.message.clone(), k.clone()));
            }
            same = if shown == last { same + 1 } else { 0 };
            if same >= 3 {
                break;
            }
            last = shown;
            let fresh = p.keywords.iter().find(|k| !f.answered.contains(&(p.message.clone(), (*k).clone()))).cloned();
            // Every keyword here answered: go back through one that led to a prompt with
            // options still to answer.
            let via = fresh
                .is_none()
                .then(|| p.keywords.iter().find(|k| f.leads.get(&(p.message.clone(), (*k).clone())).is_some_and(|m| f.pending(m))).cloned())
                .flatten();
            let chosen = fresh.clone().or(via);
            let r = match &chosen {
                Some(k) => {
                    f.answered.insert((p.message.clone(), k.clone()));
                    s.cmdline(k)
                }
                None => answer(&mut s, &p, step),
            };
            if let Some(k) = chosen
                && let Some(n) = s.current_prompt()
                && n.message != p.message
            {
                f.leads.insert((p.message.clone(), k), n.message);
            }
            if let Some(e) = internal(&r) {
                f.problems.push(format!("{id}: after \"{}\": {e}", p.message));
                break;
            }
        }
        s.cancel();
        s.cancel();
        if s.running.is_some() || s.prompting() {
            f.problems.push(format!("{id}: still running after Escape: \"{}\"", s.prompt_text()));
            s.cancel();
        }
        for line in s.log.iter().filter(|l| placeholder(l)) {
            f.problems.push(format!("{id}: placeholder in output \"{line}\""));
        }
        s.log.clear();
        // Undo back to where the walk started: the drawing must be exactly as it was.
        let mut guard = 0;
        while s.state().map_or(0, |st| st.undo.len()) > depth && guard < 200 {
            if s.undo().is_err() {
                break;
            }
            guard += 1;
        }
        let back = s.state().map(|st| Arc::ptr_eq(&st.doc, &start_doc) || *st.doc == *start_doc).unwrap_or(false);
        if !back && !NOT_UNDOABLE.contains(&id) {
            f.problems.push(format!("{id}: undo did not restore the drawing (walk {})", walk + 1));
            s = session();
        } else if !back {
            s = session();
        }
        // A walk that answered nothing new ends the sweep; so does a command without prompts.
        if f.answered.len() == before_new {
            break;
        }
    }
    f
}

#[test]
fn every_command_every_option() {
    let dir = std::env::temp_dir().join(format!("cadaddy-option-sweep-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary folder");
    // Commands may write files beside the drawing or in the working folder.
    std::env::set_current_dir(&dir).expect("the temporary folder");
    let only = std::env::var("CADADDY_SWEEP_ONLY").ok();
    let ids: Vec<&str> = command_specs()
        .iter()
        .filter(|c| c.interactive.is_some() && !SKIP.contains(&c.id))
        .map(|c| c.id)
        .filter(|id| only.as_deref().is_none_or(|o| o.split(',').any(|x| x == *id)))
        .collect();
    let mut problems = Vec::new();
    let mut unanswered: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut seen, mut answered, mut slow) = (0usize, 0usize, Vec::new());
    for id in &ids {
        let t = Instant::now();
        let f = sweep(id);
        if t.elapsed().as_secs_f64() > 5.0 {
            slow.push(format!("{id} {:.1}s", t.elapsed().as_secs_f64()));
        }
        problems.extend(f.problems);
        seen += f.seen.len();
        let missed: Vec<String> = f.seen.iter().filter(|k| !f.answered.contains(*k)).map(|(m, k)| format!("{k} at \"{m}\"")).collect();
        answered += f.seen.len() - missed.len();
        if !missed.is_empty() {
            unanswered.insert((*id).to_string(), missed);
        }
    }
    let mut report = format!(
        "Option sweep: {} commands, {seen} keyword options seen, {answered} answered, {} never answered (walk limit {MAX_WALKS}).\n",
        ids.len(),
        seen - answered
    );
    for (id, ks) in &unanswered {
        report.push_str(&format!("{id}: {}\n", ks.join("; ")));
    }
    if !slow.is_empty() {
        report.push_str(&format!("slow: {}\n", slow.join(", ")));
    }
    let out = option_env!("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from).join("option-coverage.txt");
    let _ = std::fs::write(&out, &report);
    if std::env::var("CADADDY_SWEEP_REPORT").is_ok() {
        println!("{report}\n(report: {})", out.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(only.is_some() || ids.len() > 200, "only {} commands with prompts: the catalog looks empty", ids.len());
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn placeholders_are_found() {
    assert!(placeholder("Select {name} to move"));
    assert!(placeholder("{0} objects"));
    assert!(!placeholder("Enter angle <{}>") && !placeholder("no braces") && !placeholder("{ spaced }"));
}
