use super::doc_state::{DocState, Snapshot};
use super::error::{EngineError, Result};
use super::running::{PendingWindow, Running};
use super::view::View;
use super::*;

impl Session {
    /// Append a new untitled drawing (`DrawingN`, metric or imperial default view), end a running command, make it active, bump the document epoch and return its index.
    pub fn new_drawing(&mut self, metric: bool) -> usize {
        self.untitled_counter += 1;
        let d = if metric { Drawing::new_metric() } else { Drawing::new_imperial() };
        let mut st = DocState::new(d, &format!("Drawing{}", self.untitled_counter), None);
        st.set_view_quiet(if metric { View { center: Vec2::new(210.0, 148.5), height: 297.0 } } else { View::default() });
        self.end_running();
        self.docs.push(st);
        self.active = self.docs.len() - 1;
        self.bump_doc_epoch();
        self.active
    }

    /// Append a loaded drawing, fit the view to its model extents (aspect-corrected, minimum height 1e-6) when non-empty, end a running command, make it active, bump the epoch and return its index.
    pub fn open_drawing(&mut self, d: Drawing, title: &str, path: Option<String>) -> usize {
        let mut st = DocState::new(d, title, path);
        let ext = st.doc.extents(&Space::Model);
        if !ext.is_empty() {
            st.set_view_quiet(View {
                center: ext.center(),
                height: (ext.height().max(ext.width() * self.viewport_px.1 / self.viewport_px.0.max(1.0)) * 1.1).max(1e-6),
            });
        }
        self.end_running();
        self.docs.push(st);
        self.active = self.docs.len() - 1;
        self.bump_doc_epoch();
        self.active
    }

    /// Record that which drawing is open or active changed (screenshot binding).
    pub fn bump_doc_epoch(&mut self) {
        self.doc_epoch = self.doc_epoch.wrapping_add(1);
    }

    /// A command still waiting for input belongs to the drawing it started in: end it there
    /// before another drawing becomes current, so its undo step lands in that drawing.
    fn end_running(&mut self) {
        if self.running.is_some() {
            self.cancel();
        }
    }

    /// Add a file read off-thread with [`cmd::file::read_file`] as the active drawing (OPEN's fast
    /// half). A panic while adding it is reported as an error, never propagated.
    pub fn finish_open(&mut self, f: cmd::file::ReadFile) -> Result<Value> {
        let n = self.docs.len();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cmd::file::finish_open(self, f))) {
            Ok(r) => r,
            Err(_) => {
                self.docs.truncate(n);
                self.active = self.active.min(self.docs.len().saturating_sub(1));
                self.bump_doc_epoch();
                Err(EngineError::Internal("open".into(), "panic".into()))
            }
        }
    }

    // ---------------- programmatic execution ----------------

    /// Run a command with JSON parameters (no dialogs). Records undo when the drawing changes.
    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Value> {
        let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        (spec.enabled)(self).map_err(|m| EngineError::Disabled(spec.id.into(), m))?;
        if let Some(why) = cmd::blocks::bedit::refused_in_editor(self, spec.id) {
            return Err(EngineError::Disabled(spec.id.into(), why));
        }
        let before = self.state().ok().map(|s| (s.doc.clone(), s.selection.clone(), s.uid));
        let editing = self.state().is_ok_and(|s| s.block_edit.is_some());
        let run = spec.run;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(self, params)));
        let result = match result {
            Ok(r) => r,
            Err(p) => {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(ToString::to_string)
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic".into());
                // Restore the drawing as it was.
                if let Some((doc, sel, uid)) = &before
                    && let Some(st) = self.docs.iter_mut().find(|d| d.uid == *uid)
                {
                    st.doc = doc.clone();
                    st.selection = sel.clone();
                }
                return Err(EngineError::Internal(id.into(), msg));
            }
        };
        let violation = before.as_ref().and_then(|b| cmd::blocks::refedit::after_command(self, &b.0, spec.id));
        let result = match (violation, result) {
            (Some(m), Ok(_)) => Err(EngineError::Other(m)),
            (_, r) => r,
        };
        // Opening or closing the Block Editor swaps the whole drawing: nothing in it changed.
        if self.state().is_ok_and(|s| s.block_edit.is_some()) == editing {
            self.after_command(before.as_ref().map(|b| &b.0), before.as_ref().map(|b| b.2), spec.undoable && result.is_ok());
        }
        let result = match (result, &before) {
            (Ok(v), Some((doc, sel, uid))) if spec.undoable => self.refuse_non_finite(Some(*uid), doc, sel, spec.id).map(|()| v),
            (r, _) => r,
        };
        let grouped = self.lisp_groups_undo();
        if spec.undoable
            && let Some((doc, sel, uid)) = before
            && let Some(st) = self.docs.iter_mut().find(|d| d.uid == uid)
            && !Arc::ptr_eq(&doc, &st.doc)
        {
            // Inside a LISP routine the routine's single undo step records it.
            if !grouped {
                st.push_undo(Snapshot { label: spec.label.to_string(), doc, selection: sel });
            }
            st.revision += 1;
        }
        result
    }

    /// Bump the revision without an undo step (view-only or settings changes that redraw).
    pub fn touch(&mut self) {
        if let Ok(st) = self.state_mut() {
            st.revision += 1;
        }
    }

    // ---------------- interactive commands ----------------

    /// Start a command by name or alias as if typed at the command line.
    pub fn start(&mut self, name: &str) -> Result<()> {
        self.last_polar = None;
        let name = name.trim();
        let (transparent, name) = match name.strip_prefix('\'') {
            Some(n) => (true, n),
            None => (false, name),
        };
        let lower = name.trim_start_matches(['_', '.', '-']).to_ascii_lowercase();
        // A command-line form with an id of its own (-ATTEDIT) is not the command it prefixes.
        let dashed = cmd::resolve_alias(&name.trim_start_matches(['_', '.']).to_ascii_lowercase());
        let id = if find_command(&dashed).is_some() { dashed } else { cmd::resolve_alias(&lower) };
        let Some(spec) = find_command(&id) else {
            // A command defined in LISP: (defun c:NAME …).
            if !transparent && let Some(f) = self.lisp_command(&lower) {
                if self.running.is_some() {
                    self.cancel();
                }
                self.last_command = Some(lower.clone());
                self.echo(format!("Command: {}", lower.to_ascii_uppercase()));
                return self.lisp_run_command(&lower, f);
            }
            // A bare system variable name (ORTHOMODE, GRIDMODE…) acts as SETVAR on that variable.
            let var = lower.to_ascii_uppercase();
            if !name.starts_with(['_', '.', '-']) && sysvars::get(self, &var).is_some() {
                self.start(if transparent { "'setvar" } else { "setvar" })?;
                return self.input(Input::Text(var));
            }
            return Err(EngineError::UnknownCommand(name.to_string()));
        };
        if transparent && spec.transparent && self.running.is_some() {
            // Transparent commands (zoom, pan…) run without cancelling the active one.
            if spec.interactive.is_none() {
                self.echo(format!(">>{}", spec.label));
                return self.execute(spec.id, &Value::Null).map(|_| ());
            }
            if self.suspended.is_some() {
                self.echo("A transparent command is already in progress.");
                return Ok(());
            }
        }
        let nested = transparent && spec.transparent && self.running.is_some() && spec.interactive.is_some();
        // Ctrl+Z / U / UNDO while a command's prompt offers its own Undo keyword (LINE, PLINE…)
        // takes back the last segment and keeps the command running, as the keyword does.
        if !nested && matches!(spec.id, "u" | "undo") && self.current_prompt().is_some_and(|p| p.keywords.iter().any(|k| k == "Undo")) {
            self.echo(format!("Command: {}", spec.id.to_ascii_uppercase()));
            return self.input(Input::Keyword("Undo".into()));
        }
        if !nested && self.running.is_some() {
            self.cancel();
        }
        (spec.enabled)(self).map_err(|m| EngineError::Disabled(spec.id.into(), m))?;
        if let Some(why) = cmd::blocks::bedit::refused_in_editor(self, spec.id) {
            return Err(EngineError::Disabled(spec.id.into(), why));
        }
        if nested {
            // The outer command stays the one Enter repeats.
            self.echo(format!(">>{}", spec.id.to_ascii_uppercase()));
        } else {
            self.last_command = Some(spec.id.to_string());
            self.echo(format!("Command: {}", spec.id.to_ascii_uppercase()));
        }
        match spec.interactive {
            Some(factory) => {
                let machine = guard::Guarded::create(spec.id, factory, self)?;
                let st = self.state()?;
                let before = st.doc.clone();
                let selection_before = st.selection.clone();
                if nested {
                    self.suspended = self.running.take();
                }
                self.running = Some(Running { id: spec.id.to_string(), machine, before, selection_before });
                self.pending_window = None;
                // Some commands complete immediately (e.g. ERASE with a pickfirst selection).
                self.feed(None)?;
                Ok(())
            }
            None => {
                let r = self.execute(spec.id, &Value::Null)?;
                if let Some(s) = r.as_str() {
                    self.echo(s.to_string());
                } else if !r.is_null()
                    && let Some(msg) = r.get("message").and_then(Value::as_str)
                {
                    for l in msg.lines() {
                        self.echo(l.to_string());
                    }
                }
                Ok(())
            }
        }
    }

    /// Feed an object-snap pick, retaining its viewport origin for measurements. Point modifiers
    /// that combine or constrain coordinates produce ordinary drawing-space points.
    pub fn input_snapped(&mut self, hit: snap::SnapHit) -> Result<()> {
        let source = hit.viewport.filter(|_| self.point_mods.iter().all(|f| matches!(f, pointmod::Frame::Snap { mode } if *mode != 0)));
        let previous = std::mem::replace(&mut self.picked_viewport, source);
        let result = self.input(hit.input());
        self.picked_viewport = previous;
        result
    }

    /// Give an input to the running command. With no command running, `Enter` repeats the last
    /// command (as in AutoCAD) and points do nothing. A LISP routine waiting in a `get…`
    /// function takes the input instead; one paused in `command` carries on after it.
    pub fn input(&mut self, input: Input) -> Result<()> {
        if self.lisp_input(input.clone()) {
            return Ok(());
        }
        let r = self.input_to_command(input);
        self.lisp_after_command_input();
        r
    }

    fn input_to_command(&mut self, input: Input) -> Result<()> {
        if self.running.is_none() {
            if input == Input::Enter
                && let Some(last) = self.last_command.clone()
            {
                return self.start(&last);
            }
            return Ok(());
        }
        if !self.point_mods.is_empty() {
            return self.modifier_input(input);
        }
        // A deferred snap is only meaningful where the prompt resolves it; elsewhere it is the
        // point it was picked at.
        let input = match input {
            Input::Deferred(d) if !self.current_prompt().is_some_and(|p| p.deferred) => Input::Point(d.at),
            other => other,
        };
        // Generic selection handling during "Select objects:" prompts.
        let input = self.preprocess_selection(input)?;
        match input {
            Some(i) => {
                // SIngle: the first selection that finds objects also ends the prompt.
                let single = self.select_single && matches!(&i, Input::Pick(hs) if !hs.is_empty());
                self.feed(Some(i))?;
                if single {
                    self.select_single = false;
                    if self.current_prompt().is_some_and(|p| p.accept.select) {
                        self.feed(Some(Input::Enter))?;
                    }
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn preprocess_selection(&mut self, input: Input) -> Result<Option<Input>> {
        // The command's own prompt: a typed selection mode only applies at its "Select objects".
        let Some(prompt) = self.running.as_ref().map(|r| r.machine.prompt(self)) else { return Ok(Some(input)) };
        if !prompt.accept.select {
            return Ok(Some(input));
        }
        if self.select_mode.is_some() {
            return self.select_mode_input(input);
        }
        // `_L`, `_ALL`, `_W`: the English name of a selection keyword, as scripts write it.
        let input = match input {
            Input::Text(t) => Input::Text(t.trim().strip_prefix('_').map(str::to_string).unwrap_or(t)),
            other => other,
        };
        if let Input::Text(t) | Input::Keyword(t) = &input
            && let Some(m) = select::SelectMode::from_keyword(t)
        {
            self.pending_window = None;
            self.select_mode = Some(m);
            return Ok(None);
        }
        if let Input::Text(t) | Input::Keyword(t) = &input
            && !prompt.keywords.iter().any(|k| k.eq_ignore_ascii_case(t.trim()))
        {
            match t.trim().to_ascii_lowercase().as_str() {
                "si" | "single" => {
                    self.select_single = true;
                    return Ok(None);
                }
                // AUto (a pick, or a window where nothing is picked) is how points always select;
                // Multiple picks the same way until Enter.
                "au" | "auto" | "m" | "multiple" => return Ok(None),
                _ => {}
            }
        }
        let space = self.space();
        match input {
            Input::Point(p) => {
                let ap = self.pixel_size() * self.settings.pickbox.max(1.0) * 1.5;
                if let Some(pw) = self.pending_window.take() {
                    let crossing = p.x < pw.corner.x;
                    let hs = select::select_window(self.doc()?, &space, Bounds2::new(pw.corner, p), crossing);
                    return Ok(Some(Input::Pick(cmd::groups::expand(self, hs))));
                }
                match select::pick(self.doc()?, &space, p, ap) {
                    Some(h) => Ok(Some(Input::Pick(cmd::groups::expand(self, vec![h])))),
                    None => {
                        self.pending_window = Some(PendingWindow { corner: p, during_command: true });
                        Ok(None)
                    }
                }
            }
            Input::Text(ref t) | Input::Keyword(ref t) => {
                let tl = t.trim().to_ascii_lowercase();
                let d = self.doc()?;
                let store = d.space(&space);
                let picked = match tl.as_str() {
                    "all" => store.map(|s| s.iter().filter(|e| d.is_visible(e)).map(|e| e.handle).collect()),
                    "l" | "last" => store.and_then(|s| s.last()).map(|e| vec![e.handle]),
                    "p" | "previous" => Some(self.state()?.previous_selection.clone()),
                    _ => None,
                };
                match picked {
                    Some(hs) => Ok(Some(Input::Pick(cmd::groups::expand(self, hs)))),
                    None => Ok(Some(input)),
                }
            }
            Input::Cancel => {
                self.pending_window = None;
                Ok(Some(Input::Cancel))
            }
            other => Ok(Some(other)),
        }
    }

    /// Input while a typed selection mode (W, C, F, WP, CP) collects its points: the selected
    /// objects go to the command as a pick once the mode is complete.
    fn select_mode_input(&mut self, input: Input) -> Result<Option<Input>> {
        let Some(mut mode) = self.select_mode.take() else { return Ok(Some(input)) };
        if mode.kind == select::SelectKind::Group {
            return match input {
                Input::Text(name) | Input::Keyword(name) => match select::select_group(self.doc()?, &self.space(), &name) {
                    Some(hs) => Ok(Some(Input::Pick(hs))),
                    None => {
                        self.echo(format!("No group named \"{}\".", name.trim()));
                        Ok(None)
                    }
                },
                // Enter or Esc leaves the group name prompt.
                Input::Cancel => Ok(Some(input)),
                Input::Enter => Ok(None),
                _ => {
                    self.echo("Invalid group name.");
                    self.select_mode = Some(mode);
                    Ok(None)
                }
            };
        }
        match input {
            Input::Point(p) => {
                if mode.push(p) {
                    return Ok(mode.select(self.doc()?, &self.space()).map(Input::Pick));
                }
            }
            Input::Keyword(k) if k == "Undo" => {
                mode.pts.pop();
            }
            // Enter ends a fence or polygon; with too few points the mode is dropped.
            Input::Enter => return Ok(mode.select(self.doc()?, &self.space()).map(Input::Pick)),
            Input::Cancel | Input::Pick(_) => return Ok(Some(input)),
            _ => self.echo("Invalid point."),
        }
        self.select_mode = Some(mode);
        Ok(None)
    }

    pub(crate) fn feed(&mut self, input: Option<Input>) -> Result<()> {
        let base = if matches!(input, Some(Input::Point(_))) { self.current_prompt().and_then(|p| p.base) } else { None };
        let Some(mut run) = self.running.take() else { return Ok(()) };
        let step = match input {
            Some(Input::Cancel) => Ok(Step::Cancel),
            Some(i) => {
                if let Input::Point(p) = &i {
                    self.last_point = *p;
                    if let Some(b) = base.filter(|b| b.is_finite() && p.is_finite() && b.dist(*p) > 1e-12) {
                        self.last_segment = Some((b, *p));
                    }
                    // A temporary tracking point (TT) serves one point.
                    self.tracking.release_temporary();
                }
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.machine.input(self, i)));
                match r {
                    Ok(r) => r,
                    Err(_) => Err(EngineError::Internal(run.id.clone(), "panic".into())),
                }
            }
            // A command starts with no modifiers pending.
            None => {
                self.point_mods.clear();
                run.machine.begin(self)
            }
        };
        // Redraw what the command has added so far (LINE adds a segment per point), not only when it ends.
        if let Ok(st) = self.state_mut()
            && !Arc::ptr_eq(&run.before, &st.doc)
        {
            st.revision += 1;
        }
        match step {
            Ok(Step::Continue) => {
                self.running = Some(run);
                Ok(())
            }
            Ok(Step::Done) => {
                self.finish(run, false);
                self.resume_suspended();
                Ok(())
            }
            Ok(Step::Cancel) => {
                self.echo("*Cancel*");
                self.finish(run, true);
                self.resume_suspended();
                Ok(())
            }
            Err(EngineError::Internal(id, m)) => {
                // Restore and end the command.
                if let Ok(st) = self.state_mut() {
                    st.doc = run.before.clone();
                }
                self.echo(format!("Internal error in {id}: {m}"));
                self.resume_suspended();
                Err(EngineError::Internal(id, m))
            }
            Err(e) => {
                // Invalid input: report and keep prompting (AutoCAD re-prompts).
                self.echo(e.to_string());
                self.running = Some(run);
                Ok(())
            }
        }
    }

    fn finish(&mut self, run: Running, cancelled: bool) {
        // A typed selection mode or SIngle never outlives its command.
        self.select_mode = None;
        self.select_single = false;
        if find_command(&run.id).is_some_and(|c| c.undoable)
            && let Err(e) = self.refuse_non_finite(None, &run.before, &run.selection_before, &run.id)
        {
            self.echo(e.to_string());
        }
        // A command outside undo that changed the drawing (UNDO, which swapped it for an earlier
        // one) records no undo step, as in `execute`.
        if find_command(&run.id).is_some_and(|c| !c.undoable) && self.state().is_ok_and(|st| !Arc::ptr_eq(&run.before, &st.doc)) {
            self.pending_window = None;
            return;
        }
        self.pending_window = None;
        self.select_mode = None;
        let label = find_command(&run.id).map(|c| c.label).unwrap_or("Command");
        let _ = cancelled;
        if let Some(m) = cmd::blocks::refedit::after_command(self, &run.before, &run.id) {
            self.echo(m);
        }
        self.after_command(Some(&run.before), None, true);
        // A transparent command's changes belong to the command it ran inside, and a command a
        // LISP routine ran belongs to the routine's single undo step.
        if self.suspended.is_some() || self.lisp_groups_undo() {
            if let Ok(st) = self.state_mut()
                && !Arc::ptr_eq(&run.before, &st.doc)
            {
                st.revision += 1;
            }
            return;
        }
        if let Ok(st) = self.state_mut()
            && !Arc::ptr_eq(&run.before, &st.doc)
        {
            st.push_undo(Snapshot { label: label.to_string(), doc: run.before, selection: run.selection_before });
            st.revision += 1;
        }
    }

    /// The post-command passes, in order, run after every command — by [`Session::execute`] and
    /// when an interactive command finishes — so their edits land in the command's own undo step:
    /// group membership is pruned, constraints are inferred, purged and re-solved, and
    /// associative dimensions and hatches follow their sources. A new pass is added here, once.
    ///
    /// `before` is the drawing the command started from; `uid` (when known) is the document it
    /// started in, so associativity is not applied across a document switch; `ok` is false for a
    /// failed or non-undoable programmatic command, which skips the constraint pass.
    fn after_command(&mut self, before: Option<&Arc<Drawing>>, uid: Option<u64>, ok: bool) {
        cmd::groups::after_command(self, before);
        cmd::constraints::after_command(self, before, ok);
        assoc::after_command(self, before, uid);
    }

    fn resume_suspended(&mut self) {
        if let Some(outer) = self.suspended.take() {
            self.echo(format!("Resuming {} command.", outer.id.to_ascii_uppercase()));
            self.running = Some(outer);
        }
    }

    /// Cancel the running command (Esc). With no command, clears the selection. Inside a
    /// transparent command, cancels it and the command it interrupted.
    pub fn cancel(&mut self) {
        self.point_mods.clear();
        // Esc stops a LISP routine waiting for input, and the command it paused in.
        if self.lisp_waiting() {
            if self.running.is_some() {
                let _ = self.feed(Some(Input::Cancel));
            }
            self.echo("*Cancel*");
            self.lisp_abort("Function cancelled");
            return;
        }
        if self.running.is_some() {
            if self.suspended.is_some() {
                if let Some(nested) = self.running.take() {
                    self.finish(nested, true);
                }
                self.running = self.suspended.take();
            }
            let _ = self.feed(Some(Input::Cancel));
        } else {
            self.pending_window = None;
            self.set_selection(Vec::new());
        }
    }

    /// The multiline text the running command is collecting, for the in-place editor
    /// ([`Interactive::editing_mtext`]).
    pub fn editing_mtext(&self) -> Option<cadcraft_doc::MText> {
        self.running.as_ref().and_then(|r| r.machine.editing_mtext())
    }

    pub fn current_prompt(&self) -> Option<Prompt> {
        if let Some(p) = self.lisp_prompt() {
            return Some(p);
        }
        self.running.as_ref().map(|r| {
            let p = r.machine.prompt(self);
            match &self.select_mode {
                Some(m) if p.accept.select => m.prompt(),
                _ => p,
            }
        })
    }

    /// Whether the cursor picks objects right now, so the UI shows the pick box (PICKBOX) at the
    /// crosshair: at prompts that select objects, and with no command running when PICKFIRST is on
    /// (noun-verb selection).
    pub fn picking_objects(&self) -> bool {
        match self.current_prompt() {
            Some(p) => p.picks_objects(),
            None => self.settings.pickfirst,
        }
    }

    /// Whether the object under the cursor is highlighted (SELECTIONPREVIEW): the cursor picks
    /// objects ([`Self::picking_objects`]) and the bit for a command asking for them (2) or for
    /// no command (1) is on.
    pub fn previews_selection(&self) -> bool {
        let bit = if self.current_prompt().is_some() { 2 } else { 1 };
        self.picking_objects() && self.settings.selectionpreview & bit != 0
    }

    /// Whether selected objects show grips while `selected` objects are selected (GRIPS,
    /// GRIPOBJLIMIT).
    pub fn shows_grips(&self, selected: usize) -> bool {
        let limit = self.settings.gripobjlimit;
        self.settings.grips > 0 && (limit == 0 || selected <= limit as usize)
    }

    /// The text shown on the command line: the active prompt, or "Command:".
    pub fn prompt_text(&self) -> String {
        if let Some(p) = self.lisp_prompt() {
            return p.display();
        }
        match &self.running {
            Some(r) if self.suspended.is_some() => format!(">>{} {}", r.id.to_ascii_uppercase(), r.machine.prompt(self).display()),
            Some(r) => format!("{} {}", r.id.to_ascii_uppercase(), r.machine.prompt(self).display()),
            None => "Command:".into(),
        }
    }

    /// Preview entities for the cursor: objects drawn with their own properties
    /// ([`Interactive::preview_objects`]), then the rubber band with the current properties, in
    /// the grip blue over such objects (so it shows over their fills, whatever their colour).
    pub fn preview(&self, cursor: Vec2) -> Vec<Entity> {
        let mut common = self.current_common();
        let (kinds, mut out) = match (self.lisp_preview(cursor), &self.running) {
            (Some(k), _) => (k, Vec::new()),
            (None, Some(r)) => (r.machine.preview(self, cursor), r.machine.preview_objects(self, cursor)),
            (None, None) => (Vec::new(), Vec::new()),
        };
        if !out.is_empty() {
            common.color = cadcraft_doc::color::Color::True(cadcraft_doc::color::Rgb(0x2f, 0x6b, 0xff));
        }
        out.extend(kinds.into_iter().map(|k| Entity { handle: Handle(0), common: common.clone(), kind: k }));
        out
    }

    /// The active command's preview as a transform of existing objects (see
    /// [`Interactive::drag`]), with its extra rubber-band geometry as entities.
    pub fn drag(&self, cursor: Vec2) -> Option<(Drag, Vec<Entity>)> {
        let r = self.running.as_ref()?;
        let d = r.machine.drag(self, cursor)?;
        let common = self.current_common();
        let extra = d.extra.iter().map(|k| Entity { handle: Handle(0), common: common.clone(), kind: k.clone() }).collect();
        Some((d, extra))
    }

    /// Process one line typed at the command line (Enter pressed). Spaces separate inputs as in a
    /// script line (see [`Session::script`]), so `MOVE L  0,0 5,5` ends the selection with the
    /// second space and `ERASE L ` with the trailing one. Input after the command ends is ignored.
    pub fn cmdline(&mut self, text: &str) -> Result<()> {
        let t = text.trim_end_matches(['\r', '\n']);
        // The command line types spaces literally at free-text prompts: the line is one input.
        if self.current_prompt().is_some_and(|p| p.accept.text && !p.accept.point && !p.accept.number) {
            return self.typed(t);
        }
        // At `Command:` leading blanks before a command name are not Enter (`" LINE"` from the
        // control channel starts LINE); a line of only blanks still is.
        let t = if self.running.is_none() && !t.trim().is_empty() { t.trim_start() } else { t };
        self.input_line(t, false)
    }

    /// One line of input: each space ends an input and an empty input (two spaces in a row, a
    /// trailing space, an empty line) is Enter, which at `Command:` repeats the last command. In a
    /// script, a free-text prompt (TEXT/MTEXT contents) takes the rest of the line, a prompt with
    /// keywords still splits so `I 15` reaches the keyword and the value, and the line goes on
    /// after its command ends; at the command line the rest after the command ends is ignored.
    fn input_line(&mut self, line: &str, script: bool) -> Result<()> {
        let mut rest = line;
        let mut first = true;
        loop {
            if self.running.is_none() {
                if !script && !first {
                    return Ok(());
                }
                // `cmd {json}`: programmatic call with parameters.
                if let Some((name, json)) = rest.trim().split_once(' ')
                    && json.trim_start().starts_with('{')
                {
                    let params: Value =
                        serde_json::from_str(json.trim()).map_err(|e| EngineError::BadParams { cmd: name.into(), msg: e.to_string() })?;
                    let id = cmd::resolve_alias(&name.to_ascii_lowercase());
                    let r = self.execute(&id, &params)?;
                    if !r.is_null() {
                        self.echo(r.to_string());
                    }
                    return Ok(());
                }
            } else if script && self.current_prompt().is_some_and(|p| p.accept.text && !p.accept.point && !p.accept.number && p.keywords.is_empty()) {
                return self.typed(rest);
            }
            first = false;
            let (tok, more) = next_token(rest);
            if self.running.is_some() || self.lisp_prompt().is_some() {
                self.typed(tok)?;
            } else if tok.trim().is_empty() {
                self.input(Input::Enter)?;
            } else if lisp_expression(tok) {
                // `(expression)` or `!variable` at Command: evaluates LISP and shows the value.
                let src = tok.trim().strip_prefix('!').unwrap_or(tok.trim()).to_string();
                if let Err(e) = self.lisp_eval(&src, true, false, Vec::new()) {
                    self.echo(e.to_string());
                }
            } else {
                self.start(tok)?;
            }
            match more {
                Some(more) => rest = more,
                None => return Ok(()),
            }
        }
    }

    /// One token typed while a command runs.
    pub(crate) fn typed(&mut self, t: &str) -> Result<()> {
        let Some(prompt) = self.current_prompt() else { return Ok(()) };
        let tt = t.trim();
        if tt.is_empty() {
            return self.input(Input::Enter);
        }
        // FROM, M2P, point filters, `<a` (before `'`: `'_from` is a modifier).
        if let Some(r) = self.typed_modifier(&prompt, tt) {
            return r;
        }
        // Transparent command.
        if tt.starts_with('\'') {
            return self.start(tt);
        }
        // A LISP expression typed at a command's prompt: its value is the answer.
        // Not at a free-text prompt: there the text is the answer (CAL takes "(3+4)*2").
        if lisp_expression(tt) && !prompt.accept.text && self.lisp_prompt().is_none() && !self.lisp_groups_undo() {
            let src = tt.strip_prefix('!').unwrap_or(tt).to_string();
            let v = match self.lisp_eval(&src, false, true, Vec::new()) {
                Ok(Some(Ok(v))) => v,
                Ok(_) => return Ok(()),
                Err(e) => {
                    self.echo(e.to_string());
                    return Ok(());
                }
            };
            return match v {
                lisp::value::Value::Nil => Ok(()),
                lisp::value::Value::Int(i) => self.typed(&i.to_string()),
                lisp::value::Value::Real(r) => self.typed(&format!("{r}")),
                lisp::value::Value::Str(s) => self.typed(&s),
                other => match other.as_point() {
                    Some(p) => self.input(Input::Point(Vec2::new(p[0], p[1]))),
                    None => {
                        self.echo(format!("Invalid input: {}", lisp::value::to_text(&other, true)));
                        Ok(())
                    }
                },
            };
        }
        // At "Select objects" prompts the selection modes (Previous, Last, ALL) win over a command
        // keyword sharing the letter (HATCH's "picK internal point" against `P`).
        let selection_mode = prompt.accept.select
            && matches!(tt.strip_prefix('_').unwrap_or(tt).to_ascii_lowercase().as_str(), "all" | "l" | "last" | "p" | "previous");
        // A leading underscore asks for a keyword by its English name, whatever the interface
        // language (`_M`, `_Yes`): scripts and LISP routines write them that way. (Snap
        // overrides such as `_cen` were taken above.)
        if !selection_mode
            && let Some(k) = prompt.match_keyword(tt).or_else(|| tt.strip_prefix('_').and_then(|r| prompt.match_keyword(r)))
            && !(prompt.accept.number && tt.parse::<f64>().is_ok())
        {
            return self.input(Input::Keyword(k));
        }
        if prompt.accept.point {
            if let Some(p) = prompt::parse_point_with(tt, self.last_point, &self.angle_settings()) {
                self.last_polar = prompt::parse_polar_text(tt, &self.angle_settings());
                return self.input(Input::Point(p));
            }
            // Direct distance entry along the tracking path the cursor is on, from the point it
            // comes from (an acquired or temporary tracking point needs no base point).
            if let Some(dist) = units::parse_distance(tt).filter(|_| !prompt.accept.number)
                && let Some((from, dir)) = self.tracking.last.as_ref().filter(|t| t.point.near(self.cursor, 1e-9)).and_then(|t| t.along())
            {
                return self.input(Input::Point(from + dir * dist));
            }
            // Direct distance entry along the rubber band.
            if let (Some(base), Some(dist)) = (prompt.base, units::parse_distance(tt))
                && !prompt.accept.number
            {
                let mut c = self.cursor;
                if self.settings.orthomode && !prompt.corner {
                    c = snap::iso::constrain(&self.drafting_settings(), base, c);
                }
                let dir = (c - base).normalized();
                let dir = if dir == Vec2::ZERO { Vec2::X } else { dir };
                return self.input(Input::Point(base + dir * dist));
            }
        }
        self.input(Input::Text(tt.to_string()))
    }

    /// Run a script, as AutoCAD reads one: a space or a line end ends an input, and an empty
    /// input (two spaces in a row, a space at the end of a line, an empty line) is Enter. At
    /// free-text prompts spaces are part of the text. Lines starting with `;` are comments.
    ///
    /// A script cannot run a script: the SCRIPT command (or another `cmd {json}` call) inside a
    /// running one is refused, so two files that run each other cannot recurse until the stack dies.
    pub fn script(&mut self, text: &str) -> Result<()> {
        if self.in_script {
            return Err(EngineError::BadParams { cmd: "script".into(), msg: "a script is already running; nested scripts are not supported".into() });
        }
        self.in_script = true;
        let r = self.script_lines(text);
        self.in_script = false;
        r
    }

    fn script_lines(&mut self, text: &str) -> Result<()> {
        for line in text.lines() {
            // Trailing spaces are inputs (Enter); other trailing whitespace (`\r`) is not.
            let line = line.trim_end_matches(|c: char| c.is_whitespace() && c != ' ');
            if line.trim_start().starts_with(';') {
                continue;
            }
            self.input_line(line, true)?;
        }
        Ok(())
    }

    // ---------------- undo ----------------

    pub fn undo(&mut self) -> Result<Option<String>> {
        let st = self.state_mut()?;
        let Some(snap) = st.undo.pop() else { return Ok(None) };
        let cur = Snapshot { label: snap.label.clone(), doc: st.doc.clone(), selection: st.selection.clone() };
        st.doc = snap.doc;
        st.follow_layout(&cur.doc);
        st.selection = Vec::new();
        st.redo.push(cur);
        st.revision += 1;
        Ok(Some(snap.label))
    }

    pub fn redo(&mut self) -> Result<Option<String>> {
        let st = self.state_mut()?;
        let Some(snap) = st.redo.pop() else { return Ok(None) };
        let cur = Snapshot { label: snap.label.clone(), doc: st.doc.clone(), selection: st.selection.clone() };
        st.doc = snap.doc;
        st.follow_layout(&cur.doc);
        st.selection = Vec::new();
        st.undo.push(cur);
        st.revision += 1;
        Ok(Some(snap.label))
    }

    // ---------------- picking without a command ----------------

    /// Open a selection window with one corner at `corner`, for press-and-drag selection
    /// (AutoCAD's PICKDRAG = 2: a drag opens the window wherever it starts, even over an object;
    /// the next point, where the drag ends, closes it). Only while objects are being selected: no
    /// command running, or a command asking for objects. Returns whether a window was opened.
    pub fn begin_window(&mut self, corner: Vec2) -> bool {
        let during_command = self.running.is_some();
        if !self.settings.pickdrag || !corner.is_finite() || (during_command && !self.current_prompt().is_some_and(|p| p.accept.select)) {
            return false;
        }
        self.pending_window = Some(PendingWindow { corner, during_command });
        true
    }

    /// A click with no command running: pick/toggle objects or start a selection window.
    ///
    /// PICKADD on (the default): picks add to the selection and Shift removes. PICKADD off: each pick
    /// replaces the selection and Shift adds (Shift+pick on a selected object removes it).
    pub fn idle_click(&mut self, p: Vec2, shift: bool) -> Result<()> {
        let space = self.space();
        let pickadd = self.settings.pickadd;
        if let Some(pw) = self.pending_window.take() {
            let crossing = p.x < pw.corner.x;
            let hs = select::select_window(self.doc()?, &space, Bounds2::new(pw.corner, p), crossing);
            let hs = cmd::groups::expand(self, hs);
            let mut sel = if pickadd || shift { self.selection() } else { Vec::new() };
            if shift && pickadd {
                sel.retain(|h| !hs.contains(h));
            } else {
                sel.extend(hs);
            }
            self.set_selection(sel);
            return Ok(());
        }
        let ap = self.pixel_size() * self.settings.pickbox.max(1.0) * 1.5;
        match select::pick(self.doc()?, &space, p, ap) {
            Some(h) => self.idle_pick(h, shift),
            None => self.pending_window = Some(PendingWindow { corner: p, during_command: false }),
        }
        Ok(())
    }

    /// An idle click that picked `h`, with [`Self::idle_click`]'s PICKADD and Shift rules.
    fn idle_pick(&mut self, h: Handle, shift: bool) {
        let pickadd = self.settings.pickadd;
        // With group selection the object's selectable groups come (or go) with it.
        let hs = cmd::groups::expand(self, vec![h]);
        let mut sel = if pickadd || shift { self.selection() } else { Vec::new() };
        let had = sel.contains(&h);
        if shift && (pickadd || had) {
            sel.retain(|x| !hs.contains(x));
        } else if !had {
            sel.extend(hs);
        }
        self.set_selection(sel);
    }

    /// Whether a click at `p` selects objects by picking: no command running, or a command at
    /// "Select objects" with no selection window or typed mode under way.
    pub fn click_picks(&self) -> bool {
        self.pending_window.is_none()
            && match self.running.as_ref() {
                None => self.lisp_prompt().is_none(),
                Some(r) => self.select_mode.is_none() && r.machine.prompt(self).accept.select,
            }
    }

    /// The objects under the pick box at `p` when selection cycling is on and more than one is
    /// there: what a click at `p` would choose between.
    fn cycling_hits(&self, p: Vec2) -> Vec<Handle> {
        if !self.settings.selection_cycling {
            return Vec::new();
        }
        let hs = self.pick_candidates(p);
        if hs.len() > 1 { hs } else { Vec::new() }
    }

    /// Every object under the pick box at `p`, nearest first, when a click there picks objects
    /// (empty otherwise): what Shift+Space steps the highlight through.
    pub fn pick_candidates(&self, p: Vec2) -> Vec<Handle> {
        if !self.click_picks() {
            return Vec::new();
        }
        let ap = self.pixel_size() * self.settings.pickbox.max(1.0) * 1.5;
        self.doc().map(|d| select::pick_all(d, &self.space(), p, ap)).unwrap_or_default()
    }

    /// Whether selection cycling marks `p` with its badge (SELECTIONCYCLING 1 or 2): overlapping
    /// objects a click there would choose between.
    pub fn cycling_badge(&self, p: Vec2) -> bool {
        self.settings.selection_cycling_display >= 1 && !self.cycling_hits(p).is_empty()
    }

    /// The objects a click at `p` lists to choose from (SELECTIONCYCLING 2), nearest first; empty
    /// when the click should pick as usual.
    pub fn cycling_candidates(&self, p: Vec2) -> Vec<Handle> {
        if self.settings.selection_cycling_display < 2 {
            return Vec::new();
        }
        self.cycling_hits(p)
    }

    /// Pick `h` from the selection cycling list, as a click on it alone would have: into the
    /// command at "Select objects", else into the selection (Shift as for [`Self::idle_click`]).
    pub fn pick_cycled(&mut self, h: Handle, shift: bool) -> Result<()> {
        // The list may be older than an undo or an erase.
        if self.doc()?.entity(h).is_none() {
            return Ok(());
        }
        if self.running.is_some() {
            let hs = cmd::groups::expand(self, vec![h]);
            return self.input(Input::Pick(hs));
        }
        self.idle_pick(h, shift);
        Ok(())
    }

    /// Remember the selection used by a command as "Previous".
    pub fn remember_selection(&mut self, sel: &[Handle]) {
        if let Ok(st) = self.state_mut() {
            st.previous_selection = sel.to_vec();
        }
    }

    // ---------------- views ----------------

    /// The limits of the space being edited: the sheet in paper space, else LIMMIN/LIMMAX.
    pub fn zoom_limits(&self) -> Result<Bounds2> {
        let d = self.doc()?;
        if let Space::Paper(name) = self.space()
            && let Some(sheet) = cadcraft_render::sheet(d, &name)
        {
            return Ok(sheet.bounds());
        }
        let lo = d.header.point("LIMMIN").map(|p| p.xy()).unwrap_or(Vec2::ZERO);
        let hi = d.header.point("LIMMAX").map(|p| p.xy()).unwrap_or(Vec2::new(12.0, 9.0));
        Ok(Bounds2::new(lo, hi))
    }

    pub fn zoom_extents(&mut self) -> Result<()> {
        let space = self.space();
        let ext = self.doc()?.extents(&space);
        let (w, h) = self.viewport_px;
        let ext = if ext.is_empty() { self.zoom_limits()? } else { ext };
        let st = self.state_mut()?;
        let aspect = st.zoom_aspect(w / h.max(1.0));
        let height = ext.height().max(ext.width() / aspect.max(1e-6)).max(1e-6) * 1.05;
        st.set_zoom_frame(View { center: ext.center(), height });
        Ok(())
    }

    /// Zoom by `factor` (> 1 zooms in) keeping `about` fixed on screen.
    pub fn zoom_about(&mut self, factor: f64, about: Vec2) -> Result<()> {
        if !(factor.is_finite() && factor > 0.0) {
            return Ok(());
        }
        let st = self.state_mut()?;
        let v = st.view();
        let h = (v.height / factor).clamp(1e-9, 1e12);
        let k = h / v.height;
        let c = about + (v.center - about) * k;
        st.set_view_quiet(View { center: c, height: h });
        Ok(())
    }
}

/// Typed text that is LISP: `(expression)` or `!variable`.
fn lisp_expression(t: &str) -> bool {
    let t = t.trim();
    t.starts_with('(') || (t.starts_with('!') && t.len() > 1)
}

/// The next command-line input in `rest` and what follows it: up to a space or tab, except that
/// a LISP expression runs to its closing parenthesis (spaces and quoted strings inside it).
fn next_token(rest: &str) -> (&str, Option<&str>) {
    let lead = rest.len() - rest.trim_start_matches('!').len();
    if rest.get(lead..).is_some_and(|r| r.starts_with('(')) {
        let (mut depth, mut in_str, mut escaped) = (0i32, false, false);
        for (i, c) in rest.char_indices().skip(lead) {
            if in_str {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '"') => in_str = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        let end = i + 1;
                        // As after any input, a space ends it and a trailing one is Enter.
                        let more = rest.get(end..).filter(|m| !m.is_empty()).map(|m| m.strip_prefix([' ', '\t']).unwrap_or(m));
                        return (&rest[..end], more);
                    }
                }
                _ => {}
            }
        }
        // Unbalanced: all of it (the reader reports the error).
        return (rest, None);
    }
    match rest.split_once([' ', '\t']) {
        Some((tok, more)) => (tok, Some(more)),
        None => (rest, None),
    }
}
