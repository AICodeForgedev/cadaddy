//! The Validate panel's direct judge (Q6118): with Options ▸ Automation ▸ "Judge with Anthropic
//! API" on and no agent attached, Start and Resume send each area's packet to Anthropic's API
//! with the user's own key ([`cadaddy_engine::cmd::validate_api`]) instead of waiting for an
//! agent.
//!
//! Before the first request the panel shows how many areas and pictures go out and about how many
//! tokens that is, and waits for "Send to Anthropic". The requests run on a worker thread one area
//! at a time (the key is read from the credential store there); each answer is kept on the main
//! thread through `validate.report`. Cancel stops between areas (and any wait between retries);
//! errors are listed per area, and a refused key or an unknown model stops the run.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cadaddy_engine::cmd::validate_api as api;
use egui::RichText;

use crate::CadApp;
use crate::theme::Tokens;

/// Where the key comes from: the credential store in the app, a made-up one in tests.
pub type KeySource = Arc<dyn Fn() -> Result<Option<api::ApiKey>, String> + Send + Sync>;
/// How the worker waits between tries.
pub type Sleep = Arc<dyn Fn(Duration) + Send + Sync>;

/// Lines of the run's log kept, at most.
const LOG_MAX: usize = 50;

type Outcome = Result<api::Answer, api::ApiError>;

enum State {
    Idle,
    /// The first area is packed: waiting for the user's go.
    Confirm {
        prep: api::Prepared,
        areas: usize,
    },
    /// An area's request is out.
    Running {
        prep: api::Prepared,
        rx: std::sync::mpsc::Receiver<Outcome>,
    },
}

/// The judge's settings (kept in the preferences) and its run.
pub struct ApiJudge {
    /// "Judge with Anthropic API" (off by default).
    pub on: bool,
    /// The model asked for.
    pub model: String,
    pub transport: Arc<dyn api::Transport>,
    pub key: KeySource,
    pub sleep: Sleep,
    state: State,
    cancel: Arc<AtomicBool>,
    /// Areas judged in this run.
    pub judged: usize,
    /// What happened: errors per area, the end of the run.
    pub log: Vec<String>,
}

impl Default for ApiJudge {
    fn default() -> Self {
        ApiJudge {
            on: false,
            model: api::DEFAULT_MODEL.into(),
            transport: Arc::new(api::Curl::default()),
            key: Arc::new(api::keystore::load),
            sleep: Arc::new(std::thread::sleep),
            state: State::Idle,
            cancel: Arc::new(AtomicBool::new(false)),
            judged: 0,
            log: Vec::new(),
        }
    }
}

impl ApiJudge {
    /// A request is out (or waiting for the user's go).
    pub fn busy(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// A request is out.
    pub fn running(&self) -> bool {
        matches!(self.state, State::Running { .. })
    }

    /// The first area waits for "Send to Anthropic": (areas, pictures in the first, its tokens).
    pub fn confirming(&self) -> Option<(usize, usize, u64)> {
        match &self.state {
            State::Confirm { prep, areas } => Some((*areas, prep.request.images, prep.request.est_tokens)),
            _ => None,
        }
    }

    fn say(&mut self, line: String) {
        self.log.push(line);
        let n = self.log.len();
        if n > LOG_MAX {
            self.log.drain(..n - LOG_MAX);
        }
    }
}

/// Start and Resume use the API: the option is on and no agent watches the session.
pub fn wanted(app: &CadApp) -> bool {
    app.validate_api.on && !app.session.events.on
}

/// Pack the run's next area and wait for the user's go.
pub fn begin(app: &mut CadApp, run: &str) {
    if app.validate_api.busy() {
        return;
    }
    app.validate_api.cancel.store(false, Ordering::Relaxed);
    app.validate_api.judged = 0;
    app.validate_api.log.clear();
    let model = app.validate_api.model.clone();
    match api::prepare(&mut app.session, Some(run), &model) {
        Ok(Some(prep)) => {
            let areas = prep.remaining + 1;
            app.validate_api.state = State::Confirm { prep, areas };
        }
        Ok(None) => app.validate_api.say(crate::tl!("Every area is judged.").to_string()),
        Err(e) => app.validate_api.say(e.to_string()),
    }
}

/// "Send to Anthropic": the first request goes out.
pub fn confirm(app: &mut CadApp) {
    if let State::Confirm { prep, .. } = std::mem::replace(&mut app.validate_api.state, State::Idle) {
        spawn(&mut app.validate_api, prep);
    }
}

/// Cancel: nothing more is sent (an answer on its way is still kept).
pub fn cancel(app: &mut CadApp) {
    let j = &mut app.validate_api;
    j.cancel.store(true, Ordering::Relaxed);
    if matches!(j.state, State::Confirm { .. }) {
        j.state = State::Idle;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(j: &mut ApiJudge, prep: api::Prepared) {
    let (tx, rx) = std::sync::mpsc::channel();
    let (transport, key, sleep, cancel) = (Arc::clone(&j.transport), Arc::clone(&j.key), Arc::clone(&j.sleep), Arc::clone(&j.cancel));
    let body = prep.request.body.clone();
    let started = std::thread::Builder::new().name("validate api".into()).spawn(move || {
        let outcome = match key() {
            Ok(Some(k)) => api::send(transport.as_ref(), &k, &body, &cancel, &|d| sleep(d)),
            Ok(None) => Err(api::ApiError { status: None, message: "no API key is stored: add one in Options ▸ Automation".into(), fatal: true }),
            Err(e) => Err(api::ApiError { status: None, message: e, fatal: true }),
        };
        let _ = tx.send(outcome);
    });
    match started {
        Ok(_) => j.state = State::Running { prep, rx },
        Err(e) => j.say(format!("{} ({e})", crate::tl!("The request couldn't be started"))),
    }
}

#[cfg(target_arch = "wasm32")]
fn spawn(j: &mut ApiJudge, _prep: api::Prepared) {
    j.say(crate::tl!("Judging with the Anthropic API isn't available in the web app.").to_string());
}

/// Keep an answer that came back and send the next area. Called every frame.
pub fn poll(app: &mut CadApp) {
    let outcome = match &app.validate_api.state {
        State::Running { rx, .. } => match rx.try_recv() {
            Ok(o) => o,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err(api::ApiError { status: None, message: crate::tl!("The request stopped without an answer").to_string(), fatal: true })
            }
        },
        _ => return,
    };
    let State::Running { prep, .. } = std::mem::replace(&mut app.validate_api.state, State::Idle) else { return };
    let area = if prep.label.is_empty() { prep.area.clone() } else { prep.label.clone() };
    let fatal = outcome.as_ref().err().filter(|e| e.fatal).map(|e| e.message.clone());
    if let Err(e) = &outcome {
        app.validate_api.say(format!("{area}: {}", e.message));
    }
    match api::finish(&mut app.session, &prep, outcome) {
        Ok(Some(_)) => app.validate_api.judged += 1,
        Ok(None) => {}
        Err(e) => app.validate_api.say(format!("{area}: {e}")),
    }
    if let Some(m) = fatal {
        app.validate_api.say(crate::tl!("Stopped: every area would fail the same way.").to_string());
        app.set_status(m);
        return;
    }
    if app.validate_api.cancel.load(Ordering::Relaxed) {
        app.validate_api.say(crate::tl!("Cancelled: Resume goes on from here.").to_string());
        return;
    }
    let model = app.validate_api.model.clone();
    match api::prepare(&mut app.session, Some(&prep.run), &model) {
        Ok(Some(next)) => spawn(&mut app.validate_api, next),
        Ok(None) => {
            let n = app.validate_api.judged;
            let line = crate::tf!("Done: {n} areas judged by the Anthropic API.", n = n);
            app.validate_api.say(line.clone());
            app.set_status(line);
        }
        Err(e) => app.validate_api.say(e.to_string()),
    }
}

/// What the panel's API section asked for.
pub enum Choice {
    Send,
    Cancel,
}

/// The panel's API section: the go before the first request, the progress, the log.
pub fn section(app: &CadApp, ui: &mut egui::Ui) -> Option<Choice> {
    let t = Tokens::get();
    let j = &app.validate_api;
    let mut choice = None;
    if !j.on {
        return None;
    }
    if app.session.events.on {
        ui.label(RichText::new(crate::tl!("An agent is attached: it judges, not the Anthropic API.")).small().color(t.text_dim));
        return None;
    }
    if let Some((areas, images, tokens)) = j.confirming() {
        ui.group(|ui| {
            let what = crate::tf!(
                "Judge with the Anthropic API ({model}): {areas} areas × about {images} pictures, about {tokens} input tokens.",
                model = j.model,
                areas = areas,
                images = images,
                tokens = (tokens.saturating_mul(areas as u64)),
            );
            ui.label(what.as_str());
            ui.label(RichText::new(crate::i18n::t(api::CONSENT)).small().color(t.text_dim));
            ui.label(
                RichText::new(crate::tl!("Estimated from the first area; the Anthropic account is charged for what is used."))
                    .small()
                    .color(t.text_faint),
            );
            ui.horizontal(|ui| {
                let send = ui.button(crate::tl!("Send to Anthropic"));
                crate::control::record_widget(ui, &send, crate::tl!("Send to Anthropic"), "button", None);
                if send.clicked() {
                    choice = Some(Choice::Send);
                }
                let no = ui.button(crate::tl!("Don't send"));
                crate::control::record_widget(ui, &no, crate::tl!("Don't send"), "button", None);
                if no.clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        });
    } else if j.running() {
        ui.horizontal(|ui| {
            ui.spinner();
            let n = crate::tf!("Judging with the Anthropic API: {n} areas done", n = j.judged);
            ui.label(n.as_str());
        });
    }
    for line in j.log.iter().rev().take(6) {
        ui.label(RichText::new(line.as_str()).small().color(t.text_dim));
    }
    choice
}
