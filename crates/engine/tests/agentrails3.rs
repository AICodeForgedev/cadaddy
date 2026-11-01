//! The agent rails hold on every route to a command, not only the JSON one: a command typed at
//! the command line, LISP `(command …)`, a batch, a script. SAVEAS / WBLOCK / EXPORT never replace
//! another file, CLOSE never drops unsaved work, and `project.delete` never takes unsaved
//! drawings or files the project doesn't manage, without the user being asked.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

struct Bench {
    dir: std::path::PathBuf,
    s: Session,
}

impl Bench {
    /// A saved drawing `me.dxf` with a circle, and another drawing's file `other.dxf`.
    fn new(tag: &str, headless: bool) -> Bench {
        hooks();
        let dir = std::env::temp_dir().join(format!("cadaddy-rails3-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("other.dxf"), "somebody else's drawing").unwrap();
        let mut s = Session::new();
        s.execute("circle", &json!({"center": [0, 0], "radius": 3})).unwrap();
        s.execute("saveas", &json!({"path": dir.join("me.dxf").display().to_string()})).unwrap();
        if headless {
            s.agent.headless = true;
            s.execute("agent.confirm", &json!({"on": false})).unwrap();
        }
        Bench { dir, s }
    }

    fn p(&self, name: &str) -> String {
        self.dir.join(name).display().to_string()
    }

    fn other(&self) -> String {
        std::fs::read_to_string(self.dir.join("other.dxf")).unwrap_or_default()
    }

    /// Type lines at the command line as an agent does.
    fn typed(&mut self, lines: &[&str]) -> Vec<String> {
        let was = std::mem::replace(&mut self.s.agent.from_agent, true);
        let errs = lines.iter().filter_map(|l| self.s.cmdline(l).err().map(|e| e.to_string())).collect();
        self.s.agent.from_agent = was;
        errs
    }

    fn lisp(&mut self, code: &str) -> Result<Value, String> {
        self.s.execute_agent("lisp", &json!({ "code": code })).map_err(|e| e.to_string())
    }

    fn dirty(&mut self) {
        self.s.execute("circle", &json!({"center": [9, 9], "radius": 1})).unwrap();
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const SEED: &str = "somebody else's drawing";

#[test]
fn a_typed_saveas_over_another_file_is_refused_headless_and_asks_in_the_app() {
    for headless in [true, false] {
        let mut b = Bench::new("typed-saveas", headless);
        let other = b.p("other.dxf");
        let errs = b.typed(&["saveas", "dxf", "2018", &other]);
        assert_eq!(b.other(), SEED, "headless {headless}: the other drawing is untouched ({errs:?})");
        if !headless {
            assert!(b.s.agent.pending.as_ref().is_some_and(|p| p.command == "saveas"), "the user is asked: {errs:?}");
        }
        b.s.cancel();
    }
}

#[test]
fn lisp_wblock_and_export_over_another_file_are_refused_headless_and_ask_in_the_app() {
    for headless in [true, false] {
        let mut b = Bench::new("lisp-out", headless);
        let other = b.p("other.dxf");
        let r = b.lisp(&format!("(command \"_.-wblock\" \"{other}\" \"*\")"));
        assert_eq!(b.other(), SEED, "headless {headless}: -wblock left it ({r:?})");
        b.s.cancel();
        let r = b.lisp(&format!("(command \"_.export\" \"{other}\")"));
        assert_eq!(b.other(), SEED, "headless {headless}: export left it ({r:?})");
        b.s.cancel();
        if !headless {
            assert!(b.s.agent.pending.is_some(), "the user is asked");
        }
        // The same through a typed command line.
        let errs = b.typed(&["export", &other]);
        assert_eq!(b.other(), SEED, "typed export left it ({errs:?})");
        let errs = b.typed(&["-wblock", &other, "*"]);
        assert_eq!(b.other(), SEED, "typed -wblock left it ({errs:?})");
        b.s.cancel();
    }
}

#[test]
fn json_wblock_and_export_over_another_file_are_refused_and_force_is_the_way_when_nobody_can_be_asked() {
    let mut b = Bench::new("json-out", true);
    let other = b.p("other.dxf");
    for (cmd, params) in [("wblock", json!({"path": other, "handles": []})), ("export", json!({"path": other, "format": "dxf"}))] {
        let e = b.s.execute_agent(cmd, &params).unwrap_err().to_string();
        assert!(e.contains("already exists"), "{cmd}: {e}");
        assert_eq!(b.other(), SEED, "{cmd}");
    }
    // A new file is no question, and force:true replaces (the agent said so, nobody to ask).
    b.s.execute_agent("export", &json!({"path": b.p("new.dxf"), "format": "dxf"})).unwrap();
    assert!(b.dir.join("new.dxf").is_file());
    b.s.execute_agent("export", &json!({"path": other, "format": "dxf", "force": true})).unwrap();
    assert_ne!(b.other(), SEED);
}

#[test]
fn close_through_lisp_a_typed_line_or_a_batch_keeps_unsaved_work_headless_and_asks_in_the_app() {
    for headless in [true, false] {
        let mut b = Bench::new("close", headless);
        b.dirty();
        let r = b.lisp("(command \"_.close\")");
        assert_eq!(b.s.docs.len(), 1, "headless {headless}: LISP close kept the drawing ({r:?})");
        let r = b.lisp("(command \"_.closeall\")");
        assert_eq!(b.s.docs.len(), 1, "headless {headless}: LISP closeall kept it ({r:?})");
        let errs = b.typed(&["close"]);
        assert_eq!(b.s.docs.len(), 1, "typed close kept it ({errs:?})");
        let was = std::mem::replace(&mut b.s.agent.from_agent, true);
        let r = b.s.execute_many_agent(&[json!({"command": "close", "params": {}})], false);
        b.s.agent.from_agent = was;
        assert_eq!(b.s.docs.len(), 1, "batch close kept it ({r:?})");
        if !headless {
            assert!(b.s.agent.pending.as_ref().is_some_and(|p| p.question.contains("without saving")));
        }
        // A drawing without changes closes as ever.
        b.s.execute("qsave", &json!({})).unwrap();
        b.lisp("(command \"_.close\")").unwrap_or_default();
        b.s.cancel();
    }
}

#[test]
fn the_users_own_typed_commands_are_not_stopped() {
    let mut b = Bench::new("user", false);
    let other = b.p("other.dxf");
    b.s.execute("export", &json!({"path": other, "format": "dxf"})).unwrap();
    assert_ne!(b.other(), SEED, "the user's export replaces the file as ever");
    b.dirty();
    b.s.execute("close", &json!({})).unwrap();
    assert!(b.s.docs.is_empty());
}

fn project(b: &mut Bench) -> String {
    let proj = b.p("GP");
    b.s.execute("project.create", &json!({"path": proj, "name": "GP"})).unwrap();
    proj
}

#[test]
fn project_delete_by_an_agent_keeps_unsaved_work_and_files_the_project_doesnt_manage() {
    let mut b = Bench::new("projdel", true);
    let proj = project(&mut b);
    std::fs::write(format!("{proj}/my_notes.txt"), "notes").unwrap();
    // An open drawing of the project with a change that was never saved.
    b.s.execute("saveas", &json!({"path": format!("{proj}/a.dxf")})).unwrap();
    b.s.execute("project.add", &json!({"path": format!("{proj}/a.dxf")})).unwrap();
    b.dirty();
    let e = b.s.execute_agent("project.delete", &json!({"path": proj, "confirm": true})).unwrap_err().to_string();
    assert!(e.contains("unsaved changes") && e.contains("a.dxf"), "{e}");
    assert!(std::path::Path::new(&proj).join("a.dxf").is_file(), "nothing deleted");
    assert_eq!(b.s.docs.len(), 1, "the drawing is still open");
    // Saved, the unmanaged file still stops it, naming the file.
    b.s.execute("qsave", &json!({})).unwrap();
    let e = b.s.execute_agent("project.delete", &json!({"path": proj, "confirm": true})).unwrap_err().to_string();
    assert!(e.contains("my_notes.txt") && e.contains("deleteUnmanaged"), "{e}");
    assert!(std::path::Path::new(&proj).join("my_notes.txt").is_file());
    // A dry run says what it would take.
    b.s.execute_agent("agent.dryrun", &json!({"on": true})).unwrap();
    let r = b.s.execute_agent("project.delete", &json!({"path": proj, "confirm": true})).unwrap();
    assert!(r["result"]["wouldDelete"]["bytes"].as_u64().unwrap() > 0, "{r}");
    assert_eq!(r["result"]["wouldDelete"]["unmanagedFiles"], json!(["my_notes.txt"]), "{r}");
    b.s.execute("agent.dryrun", &json!({"on": false})).unwrap();
    assert!(std::path::Path::new(&proj).join("my_notes.txt").is_file(), "the dry run took nothing");
    // Only when it names the files is the folder deleted.
    let r = b.s.execute_agent("project.delete", &json!({"path": proj, "confirm": true, "deleteUnmanaged": true})).unwrap();
    assert_eq!(r["deleted"], "GP");
    assert!(!std::path::Path::new(&proj).exists());
}

#[test]
fn project_delete_in_the_app_waits_for_the_user() {
    let mut b = Bench::new("projdel-gui", false);
    let proj = project(&mut b);
    let e = b.s.execute_agent("project.delete", &json!({"path": proj, "confirm": true})).unwrap_err().to_string();
    assert!(e.contains("Waiting for the user"), "{e}");
    assert!(b.s.agent.pending.as_ref().is_some_and(|p| p.command == "project.delete" && p.question.contains("delete the project")));
    assert!(std::path::Path::new(&proj).join("project.json").is_file());
    // Without confirm the agent still hears how, not a question.
    b.s.agent.pending = None;
    let e = b.s.execute_agent("project.delete", &json!({"path": proj})).unwrap_err().to_string();
    assert!(e.contains("confirm: true"), "{e}");
}

#[test]
fn project_delete_by_the_user_refuses_unsaved_work_unless_discarded() {
    let mut b = Bench::new("projdel-user", false);
    let proj = project(&mut b);
    b.s.execute("saveas", &json!({"path": format!("{proj}/a.dxf")})).unwrap();
    b.dirty();
    let e = b.s.execute("project.delete", &json!({"path": proj, "confirm": true})).unwrap_err().to_string();
    assert!(e.contains("discard: true"), "{e}");
    b.s.execute("project.delete", &json!({"path": proj, "confirm": true, "discard": true})).unwrap();
    assert!(!std::path::Path::new(&proj).exists());
}
