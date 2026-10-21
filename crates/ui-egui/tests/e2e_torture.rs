//! Seeded random UI torture: drives the app with random clicks, keys, typed commands, dialog
//! opens/closes, palette drags, tab switches, window resizes and undo for extended periods,
//! detecting panics (guard reports), hangs (a frame not presented for 5 s wall-clock), stuck
//! modal dialogs, and error spam in the log.
//!
//! `cargo test --test e2e_torture -- --nocapture` shows findings.  Set `CADADDY_TORTURE_SECONDS`
//! to override the default 120 s per seed (prod: 1800+).  `CADADDY_TORTURE_SEEDS` sets how many
//! seeds to run (default 3).

mod e2e_common;

use std::time::Instant;

use e2e_common::{E2e, H, W};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Seeded Xoshiro256** (no external dependency)
// ---------------------------------------------------------------------------
struct Rng {
    s: [u64; 4],
}

impl Rng {
    fn new(seed: u64) -> Self {
        // SplitMix64 to fill state from a single u64 seed.
        let mut z = seed;
        let mut next = || -> u64 {
            z = z.wrapping_add(0x9e3779b97f4a7c15);
            let mut r = z;
            r = (r ^ (r >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            r = (r ^ (r >> 27)).wrapping_mul(0x94d049bb133111eb);
            r ^ (r >> 31)
        };
        Self { s: [next(), next(), next(), next()] }
    }

    fn next_u64(&mut self) -> u64 {
        let result = (self.s[1].wrapping_mul(5)).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in `[0, n)`.
    fn gen_range(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        // Rejection sampling for uniform distribution.
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.next_u64();
            if r >= threshold {
                return r % n;
            }
        }
    }

    fn gen_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.gen_range(items.len() as u64) as usize]
    }
}

// ---------------------------------------------------------------------------
// Action vocabulary
// ---------------------------------------------------------------------------

/// Drawing commands that create geometry (safe, always have a complete parameter form).
const DRAW_COMMANDS: &[(&str, &str)] = &[
    ("line", r#"{"points":[[0,0],[10,5]]}"#),
    ("line", r#"{"points":[[20,20],[30,25],[40,20]]}"#),
    ("circle", r#"{"center":[50,50],"radius":5}"#),
    ("arc", r#"{"center":[10,10],"radius":8,"startAngle":0,"endAngle":90}"#),
    ("pline", r#"{"points":[[0,0],[10,0],[10,10],[0,10]],"close":true}"#),
    ("point", r#"{"point":[7,7]}"#),
    ("rectangle", r#"{"corner1":[0,0],"corner2":[20,15]}"#),
    ("mline", r#"{"points":[[0,0],[30,0]]}"#),
    ("ray", r#"{"base":[0,0],"through":[10,10]}"#),
    ("xline", r#"{"base":[0,0],"through":[0,10]}"#),
    ("ellipse", r#"{"center":[0,0],"majorEnd":[10,0],"ratio":0.5}"#),
    ("solid", r#"{"points":[[0,0],[10,0],[5,8]]}"#),
    ("trace", r#"{"points":[[0,0],[10,0],[10,2],[0,2]]}"#),
    ("donut", r#"{"center":[0,0],"innerRadius":2,"outerRadius":5}"#),
    ("region", r#"{"handles":"all"}"#),
    ("spline", r#"{"fitPoints":[[0,0],[5,10],[10,0]]}"#),
    ("mtext", r#"{"insertionPoint":[0,0],"text":"Sample text"}"#),
    ("text", r#"{"insertionPoint":[10,10],"text":"Hello","height":2.5}"#),
];

/// Modify commands (applied to whatever is selected, or to "all").
const MODIFY_COMMANDS: &[(&str, &str)] = &[
    ("erase", r#"{"handles":"all"}"#),
    ("copy", r#"{"handles":"all","base":[0,0],"displacement":[5,5]}"#),
    ("move", r#"{"handles":"all","base":[0,0],"displacement":[3,3]}"#),
    ("rotate", r#"{"handles":"all","base":[0,0],"angle":45}"#),
    ("scale", r#"{"handles":"all","base":[0,0],"factor":1.5}"#),
    ("mirror", r#"{"handles":"all","point1":[0,0],"point2":[0,10]}"#),
    ("offset", r#"{"distance":2,"side":"left","handles":"last"}"#),
    ("array", r#"{"handles":"all","type":"rectangular","rows":2,"columns":2,"rowSpacing":10,"columnSpacing":10}"#),
    ("explode", r#"{"handles":"all"}"#),
    ("chprop", r#"{"handles":"all","color":"red"}"#),
];

/// View/navigation commands.
const VIEW_COMMANDS: &[&str] = &["zoom extents", "zoom all", "zoom window 0,0 100,100", "zoom previous", "pan", "regen", "regenall"];

/// Utility commands.
const UTIL_COMMANDS: &[&str] = &[
    "undo",
    "undo",
    "undo",
    "redo",
    "oops",
    "selectall",
    "deselect",
    "layer new Torture-A",
    "layer set 0",
    "grid on",
    "grid off",
    "snap on",
    "snap off",
    "ortho on",
    "ortho off",
    "osnap on",
    "osnap off",
];

/// Dialog ids (safe to open/close).
const DIALOG_IDS: &[&str] = &[
    "layers",
    "dsettings",
    "units",
    "about",
    "color",
    "options",
    "qselect",
    "hatch",
    "linetype",
    "textstyle",
    "dimstyle",
    "pagesetup",
    "blocks",
    "find",
    "commands",
    "dwgprops",
    "spell",
    "purge",
    "toolpalettes",
    "undohistory",
    "commandhistory",
    "materials",
    "aliasedit",
    "standards",
    "publish",
    "layerstates",
    "plotstamp",
    "tolerance",
    "rooms",
    "secureload",
    "adcenter",
    "actmanager",
    "laywalk",
    "exchangeloss",
];

/// Ribbon tabs.
const RIBBON_TABS: &[&str] =
    &["Home", "Insert", "Annotate", "Parametric", "View", "Manage", "Output", "Express Tools", "Collaborate", "Featured Apps"];

/// Keys to press randomly.
const KEYS: &[&str] = &[
    "escape",
    "enter",
    "delete",
    "backspace",
    "space",
    "tab",
    "A",
    "B",
    "C",
    "D",
    "F",
    "L",
    "M",
    "O",
    "P",
    "R",
    "S",
    "T",
    "U",
    "Z",
    "F1",
    "F2",
    "F3",
    "F5",
    "F7",
    "F8",
    "F9",
];

/// Window sizes to resize to.
const SIZES: &[(f64, f64)] = &[(1280.0, 800.0), (1024.0, 640.0), (1920.0, 1080.0), (800.0, 600.0), (1600.0, 900.0), (2560.0, 1440.0)];

// ---------------------------------------------------------------------------
// Findings tracker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Finding {
    seed: u64,
    lang: &'static str,
    step: usize,
    action: String,
    kind: &'static str,
    detail: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[seed={} lang={} step={}] {}: {} ({})", self.seed, self.lang, self.step, self.kind, self.detail, self.action)
    }
}

// ---------------------------------------------------------------------------
// The torture driver
// ---------------------------------------------------------------------------

fn run_torture(seed: u64, lang: &'static str, seconds: u64) -> Vec<Finding> {
    let mut rng = Rng::new(seed);
    let mut e = E2e::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut step = 0usize;
    let mut consecutive_errors = 0usize;
    let mut last_action = String::new();

    // Set language.
    let _ = e.call("engine.execute", json!({"command": "ui.language", "params": {"lang": lang}}));

    // Seed some geometry so modify commands have targets.
    for (cmd, params) in DRAW_COMMANDS.iter().take(6) {
        let params: Value = serde_json::from_str(params).unwrap_or(json!({}));
        let _ = e.call("engine.execute", json!({"command": cmd, "params": params}));
    }

    let deadline = Instant::now() + std::time::Duration::from_secs(seconds);

    while Instant::now() < deadline {
        step += 1;

        // Periodically check for stuck dialogs (open for >50 actions without closing).
        if step.is_multiple_of(50)
            && let Some(d) = e.dialog()
        {
            // Try to close it.
            let _ = e.call("ui.key", json!({"key": "escape"}));
            e.pump();
            if e.dialog().as_deref() == Some(d.as_str()) {
                findings.push(Finding {
                    seed,
                    lang,
                    step,
                    action: last_action.clone(),
                    kind: "stuck-dialog",
                    detail: format!("dialog '{}' stayed open after Escape", d),
                });
                // Force close.
                let _ = e.call("ui.set", json!({"dialog": null}));
                e.pump();
            }
        }

        // Pick a random action category.
        let category = rng.gen_range(100);
        let action_desc;

        // Measure frame time around the action.
        let t0 = Instant::now();

        match category {
            // 0..25: draw something
            0..25 => {
                let (cmd, params_str) = rng.pick(DRAW_COMMANDS);
                action_desc = format!("draw:{cmd}");
                let params: Value = serde_json::from_str(params_str).unwrap_or(json!({}));
                // Randomize position slightly.
                let ox = (rng.gen_f32() - 0.5) * 200.0;
                let oy = (rng.gen_f32() - 0.5) * 200.0;
                let mut p = params.clone();
                if let Some(obj) = p.as_object_mut() {
                    // Offset insertion/center points.
                    for key in ["center", "insertionPoint", "point", "base"] {
                        if let Some(arr) = obj.get_mut(key).and_then(Value::as_array_mut).filter(|a| a.len() >= 2) {
                            arr[0] = json!(arr[0].as_f64().unwrap_or(0.0) + ox as f64);
                            arr[1] = json!(arr[1].as_f64().unwrap_or(0.0) + oy as f64);
                        }
                    }
                }
                match e.call("engine.execute", json!({"command": cmd, "params": p})) {
                    Ok(_) => consecutive_errors = 0,
                    Err(err) => check_error(&err, &mut findings, seed, lang, step, &action_desc, &mut consecutive_errors),
                }
            }
            // 25..35: modify
            25..35 => {
                let (cmd, params_str) = rng.pick(MODIFY_COMMANDS);
                action_desc = format!("modify:{cmd}");
                let params: Value = serde_json::from_str(params_str).unwrap_or(json!({}));
                match e.call("engine.execute", json!({"command": cmd, "params": params})) {
                    Ok(_) => consecutive_errors = 0,
                    Err(err) => check_error(&err, &mut findings, seed, lang, step, &action_desc, &mut consecutive_errors),
                }
            }
            // 35..42: view/nav command
            35..42 => {
                let cmd = rng.pick(VIEW_COMMANDS);
                action_desc = format!("typed:{cmd}");
                match e.call("cmdline.input", json!({"text": cmd})) {
                    Ok(_) => consecutive_errors = 0,
                    Err(err) => check_error(&err, &mut findings, seed, lang, step, &action_desc, &mut consecutive_errors),
                }
            }
            // 42..52: utility command
            42..52 => {
                let cmd = rng.pick(UTIL_COMMANDS);
                action_desc = format!("typed:{cmd}");
                match e.call("cmdline.input", json!({"text": cmd})) {
                    Ok(_) => consecutive_errors = 0,
                    Err(err) => check_error(&err, &mut findings, seed, lang, step, &action_desc, &mut consecutive_errors),
                }
            }
            // 52..62: open/close dialog
            52..62 => {
                let id = rng.pick(DIALOG_IDS);
                action_desc = format!("dialog:{id}");
                let _ = e.call("ui.set", json!({"dialog": id}));
                e.pump();
                // Read a few frames with the dialog open.
                for _ in 0..rng.gen_range(4) + 1 {
                    e.frame();
                }
                // Close it.
                let _ = e.call("ui.key", json!({"key": "escape"}));
                e.pump();
                // Force close if still open (some dialogs don't close on Escape).
                if e.dialog().is_some() {
                    let _ = e.call("ui.set", json!({"dialog": null}));
                    e.pump();
                }
                consecutive_errors = 0;
            }
            // 62..70: random click on screen
            62..70 => {
                let x = rng.gen_f32() * W;
                let y = rng.gen_f32() * H;
                let button = if rng.gen_range(5) == 0 { "right" } else { "left" };
                let shift = rng.gen_range(8) == 0;
                let ctrl = rng.gen_range(8) == 0;
                action_desc = format!("click:{x:.0},{y:.0},{button}");
                let _ = e.call("ui.click", json!({"x": x, "y": y, "button": button, "shift": shift, "ctrl": ctrl}));
                consecutive_errors = 0;
            }
            // 70..76: key press
            70..76 => {
                let key = rng.pick(KEYS);
                let cmd_mod = rng.gen_range(6) == 0;
                let shift = rng.gen_range(4) == 0;
                action_desc = format!("key:{key} cmd={cmd_mod} shift={shift}");
                let _ = e.call("ui.key", json!({"key": key, "cmd": cmd_mod, "shift": shift}));
                consecutive_errors = 0;
            }
            // 76..80: type text
            76..80 => {
                let texts = ["hello", "123.45", "0,0", "@10<45", "TEST", "*", ""];
                let text = rng.pick(&texts);
                action_desc = format!("text:{text}");
                let _ = e.call("ui.text", json!({"text": text}));
                consecutive_errors = 0;
            }
            // 80..85: resize window
            80..85 => {
                let (w, h) = rng.pick(SIZES);
                action_desc = format!("resize:{w}x{h}");
                let _ = e.call("ui.resize", json!({"width": w, "height": h}));
                e.pump();
                consecutive_errors = 0;
            }
            // 85..90: switch ribbon tab
            85..90 => {
                let tab = rng.pick(RIBBON_TABS);
                action_desc = format!("ribbon:{tab}");
                let _ = e.call("ui.set", json!({"ribbon_tab": tab}));
                e.pump();
                consecutive_errors = 0;
            }
            // 90..94: toggle UI panels
            90..94 => {
                let toggles = [
                    "show_palettes",
                    "show_toolbar",
                    "show_file_tabs",
                    "show_status_bar",
                    "show_command_line",
                    "show_viewcube",
                    "show_ucs_icon",
                    "ribbon",
                    "show_toolsets",
                ];
                let field = rng.pick(&toggles);
                let val = rng.gen_range(2) == 0;
                action_desc = format!("toggle:{field}={val}");
                let _ = e.call("ui.set", json!({(*field): val}));
                e.pump();
                consecutive_errors = 0;
            }
            // 94..97: drag on canvas
            94..97 => {
                let x1 = rng.gen_f32() * W;
                let y1 = rng.gen_f32() * H;
                let x2 = rng.gen_f32() * W;
                let y2 = rng.gen_f32() * H;
                action_desc = format!("drag:{x1:.0},{y1:.0}->{x2:.0},{y2:.0}");
                let _ = e.call("ui.drag", json!({"x": x1, "y": y1, "to": [x2, y2]}));
                consecutive_errors = 0;
            }
            // 97..100: menu invoke (random from the available menu items)
            _ => {
                let menu_commands = [
                    "line",
                    "circle",
                    "arc",
                    "pline",
                    "move",
                    "copy",
                    "erase",
                    "undo",
                    "redo",
                    "zoom.extents",
                    "zoom.all",
                    "regen",
                    "selectall",
                    "deselect",
                    "grid",
                    "ortho",
                ];
                let cmd = rng.pick(&menu_commands);
                action_desc = format!("menu:{cmd}");
                match e.call("ui.menu.invoke", json!({"command": cmd})) {
                    Ok(_) => {
                        // Cancel any interactive command that started.
                        let _ = e.call("cmdline.key", json!({"key": "escape"}));
                        consecutive_errors = 0;
                    }
                    Err(err) => check_error(&err, &mut findings, seed, lang, step, &action_desc, &mut consecutive_errors),
                }
            }
        }

        let elapsed_ms = t0.elapsed().as_millis();

        // Hang detection: any single action+pump cycle taking >5 s.
        if elapsed_ms > 5000 {
            findings.push(Finding { seed, lang, step, action: action_desc.clone(), kind: "hang", detail: format!("action took {} ms", elapsed_ms) });
        }

        // Check the session log for "Internal error" (guard panic reports).
        let history = e.history();
        for line in &history {
            let lower = line.to_ascii_lowercase();
            if lower.contains("internal error") || lower.contains("panicked") {
                findings.push(Finding { seed, lang, step, action: action_desc.clone(), kind: "panic", detail: line.clone() });
            }
        }

        // If there's a running prompt with "Internal error", end it.
        if e.app.session.prompt_text().to_ascii_lowercase().contains("internal error") {
            let _ = e.call("cmdline.key", json!({"key": "enter"}));
        }

        last_action = action_desc;

        // Safety valve: if >20 consecutive errors, something is deeply wrong.
        if consecutive_errors > 20 {
            findings.push(Finding {
                seed,
                lang,
                step,
                action: last_action.clone(),
                kind: "error-storm",
                detail: format!("{consecutive_errors} consecutive errors"),
            });
            consecutive_errors = 0;
            // Reset: cancel everything, close dialogs.
            let _ = e.call("cmdline.key", json!({"key": "escape"}));
            let _ = e.call("cmdline.key", json!({"key": "escape"}));
            let _ = e.call("ui.set", json!({"dialog": null}));
            e.pump();
        }
    }

    eprintln!(
        "  seed={seed} lang={lang}: {step} steps, {} findings (panics={}, hangs={}, stuck={}, storms={})",
        findings.len(),
        findings.iter().filter(|f| f.kind == "panic").count(),
        findings.iter().filter(|f| f.kind == "hang").count(),
        findings.iter().filter(|f| f.kind == "stuck-dialog").count(),
        findings.iter().filter(|f| f.kind == "error-storm").count(),
    );

    findings
}

fn check_error(err: &str, findings: &mut Vec<Finding>, seed: u64, lang: &'static str, step: usize, action: &str, consecutive_errors: &mut usize) {
    let lower = err.to_ascii_lowercase();
    // Guard-reported panics.
    if lower.contains("internal error") || lower.contains("panicked") || lower.contains("panic") {
        findings.push(Finding { seed, lang, step, action: action.to_string(), kind: "panic", detail: err.to_string() });
    }
    *consecutive_errors += 1;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn torture_seconds() -> u64 {
    std::env::var("CADADDY_TORTURE_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(120)
}

fn torture_seeds() -> u64 {
    std::env::var("CADADDY_TORTURE_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(3)
}

#[test]
fn torture_en() {
    let seconds = torture_seconds();
    let seeds = torture_seeds();
    eprintln!("UI torture (en): {seeds} seeds x {seconds}s each");
    let mut all: Vec<Finding> = Vec::new();
    for i in 0..seeds {
        let seed = 0xCAD_0000 + i;
        all.extend(run_torture(seed, "en", seconds));
    }
    report(&all);
    let panics: Vec<_> = all.iter().filter(|f| f.kind == "panic").collect();
    let hangs: Vec<_> = all.iter().filter(|f| f.kind == "hang").collect();
    assert!(panics.is_empty(), "panics found:\n{}", panics.iter().map(|f| f.to_string()).collect::<Vec<_>>().join("\n"));
    assert!(hangs.is_empty(), "hangs found:\n{}", hangs.iter().map(|f| f.to_string()).collect::<Vec<_>>().join("\n"));
}

#[test]
fn torture_ar() {
    let seconds = torture_seconds();
    let seeds = torture_seeds();
    eprintln!("UI torture (ar): {seeds} seeds x {seconds}s each");
    let mut all: Vec<Finding> = Vec::new();
    for i in 0..seeds {
        let seed = 0xCAD_A000 + i;
        all.extend(run_torture(seed, "ar", seconds));
    }
    report(&all);
    let panics: Vec<_> = all.iter().filter(|f| f.kind == "panic").collect();
    let hangs: Vec<_> = all.iter().filter(|f| f.kind == "hang").collect();
    assert!(panics.is_empty(), "panics found:\n{}", panics.iter().map(|f| f.to_string()).collect::<Vec<_>>().join("\n"));
    assert!(hangs.is_empty(), "hangs found:\n{}", hangs.iter().map(|f| f.to_string()).collect::<Vec<_>>().join("\n"));
}

fn report(findings: &[Finding]) {
    if findings.is_empty() {
        eprintln!("  No findings.");
        return;
    }
    eprintln!("  Findings ({}):", findings.len());
    for f in findings {
        eprintln!("    {f}");
    }
}
