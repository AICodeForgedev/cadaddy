//! A shared project page (Q350): the field viewer read-only, on a model a web server serves
//! beside the page (`cargo xtask share-page` writes the folder). The page's `<body
//! data-share="model/share.json">` (or `?share=URL`) names the manifest; the base and the parts
//! it lists are fetched one after another with a progress bar, relative to the page, so the folder
//! works from any sub-path. A 401 or 403 from the server (its passcode gate) shows "access
//! denied". The view in use travels as a `#view=` link: Copy View Link puts it in the address bar
//! and on the clipboard, and a link opened shows that view once the model is in. `?sharetest`
//! tests the page in the browser and reports to `selftest/result` (`cargo xtask webtest --share`).

use std::cell::RefCell;
use std::rc::Rc;

use cadaddy_ui_egui::fieldview::{FieldViewer, Mode};
use cadaddy_walk3d::share::{Loaded, ShareLoad, ShareManifest, ViewLink};
use serde_json::{Value, json};
use wasm_bindgen::JsCast as _;
use web_sys::js_sys;

/// What a fetch brought: the bytes, or the HTTP status (0: no answer) and why.
type Fetched = Result<Vec<u8>, (u16, String)>;

enum Msg {
    Manifest(Fetched),
    File(Fetched),
}

#[derive(Clone)]
struct Net {
    inbox: Rc<RefCell<Vec<Msg>>>,
    ctx: egui::Context,
}

impl Net {
    fn get(&self, url: String, wrap: fn(Fetched) -> Msg) {
        let net = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let r = fetch(&url).await;
            net.inbox.borrow_mut().push(wrap(r));
            net.ctx.request_repaint();
        });
    }
}

async fn fetch(url: &str) -> Fetched {
    let window = web_sys::window().ok_or((0, "no browser window".to_string()))?;
    let resp = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url)).await.map_err(|e| (0, js_error(e)))?;
    let resp: web_sys::Response = resp.dyn_into().map_err(|_| (0, "not a response".to_string()))?;
    if !resp.ok() {
        return Err((resp.status(), format!("HTTP {}", resp.status())));
    }
    let buf = wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(|e| (0, js_error(e)))?).await.map_err(|e| (0, js_error(e)))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

fn js_error(e: wasm_bindgen::JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

fn location() -> Option<web_sys::Location> {
    web_sys::window().map(|w| w.location())
}

fn now_ms() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map_or_else(js_sys::Date::now, |p| p.now())
}

struct ShareShell {
    viewer: FieldViewer,
    net: Net,
    manifest_url: String,
    load: Option<ShareLoad>,
    /// The `#view=` of the address bar last seen (a new one is shown).
    hash: String,
    /// A view waiting for the model.
    view: Option<ViewLink>,
    test: Option<ShareTest>,
    /// Page time (ms since it started loading) when the model was all in.
    loaded_ms: Option<f64>,
}

impl ShareShell {
    fn fail(&mut self, (status, why): (u16, String)) {
        self.viewer.progress = None;
        if status == 401 || status == 403 {
            self.viewer.denied = true;
            self.viewer.pkg = None;
        } else {
            self.viewer.status = format!("{}: {why}", cadaddy_ui_egui::i18n::t("The model could not be loaded"));
        }
    }

    fn next(&mut self) {
        let Some(load) = &self.load else { return };
        match load.next_file() {
            Some(f) => {
                self.viewer.progress = Some(load.progress());
                self.net.get(crate::beside(&self.manifest_url, f), Msg::File);
            }
            None => {
                self.viewer.progress = None;
                self.loaded_ms = Some(now_ms());
                if let Some(v) = self.view.take() {
                    self.viewer.apply_view(&v);
                }
            }
        }
    }

    fn receive(&mut self) {
        let msgs = std::mem::take(&mut *self.net.inbox.borrow_mut());
        for m in msgs {
            match m {
                Msg::Manifest(Ok(bytes)) => match ShareManifest::from_json(&bytes) {
                    Ok(manifest) => {
                        self.viewer.download_offered = manifest.download.is_some();
                        if let Some(d) = web_sys::window().and_then(|w| w.document()) {
                            d.set_title(&manifest.title);
                        }
                        self.load = Some(ShareLoad::new(manifest));
                        self.next();
                    }
                    Err(e) => self.fail((0, e)),
                },
                Msg::File(Ok(bytes)) => {
                    let r = self.load.as_mut().map(|l| l.add(&bytes));
                    match r {
                        Some(Ok(Loaded::Base(pkg))) => {
                            self.viewer.open_package(pkg);
                            self.next();
                        }
                        Some(Ok(Loaded::Part(els))) => {
                            self.viewer.add_elements(els);
                            self.next();
                        }
                        Some(Err(e)) => self.fail((0, e)),
                        None => {}
                    }
                }
                Msg::Manifest(Err(e)) | Msg::File(Err(e)) => self.fail(e),
            }
        }
    }

    /// A `#view=` typed or pasted into the address bar is shown.
    fn follow_hash(&mut self) {
        let hash = location().and_then(|l| l.hash().ok()).unwrap_or_default();
        if hash == self.hash {
            return;
        }
        self.hash = hash.clone();
        if let Some(v) = ViewLink::decode(&hash) {
            if self.load.as_ref().is_some_and(ShareLoad::done) {
                self.viewer.apply_view(&v);
            } else {
                self.view = Some(v);
            }
        }
    }

    fn hand_out(&mut self, ctx: &egui::Context) {
        if let Some(link) = self.viewer.link_out.take()
            && let Some(loc) = location()
        {
            let hash = format!("#view={link}");
            // Shown in the address bar without a new history entry for each copy.
            let url = format!("{}{}{hash}", loc.pathname().unwrap_or_default(), loc.search().unwrap_or_default());
            if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
                let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
            }
            self.hash = hash;
            let full = loc.href().unwrap_or(url);
            ctx.copy_text(full);
        }
        if std::mem::take(&mut self.viewer.download_requested)
            && let Some(d) = self.load.as_ref().and_then(|l| l.manifest.download.clone())
        {
            let name = d.file.rsplit('/').next().unwrap_or("model").to_string();
            if let Err(e) = save_link(&crate::beside(&self.manifest_url, &d.file), &name) {
                self.viewer.status = e;
            }
        }
    }
}

/// The browser saves the file at `url` as `name` (an `<a download>` clicked).
fn save_link(url: &str, name: &str) -> Result<(), String> {
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no browser document")?;
    let a = document.create_element("a").map_err(js_error)?.dyn_into::<web_sys::HtmlAnchorElement>().map_err(|_| "no link".to_string())?;
    a.set_href(url);
    a.set_download(name);
    a.click();
    Ok(())
}

impl eframe::App for ShareShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive();
        self.follow_hash();
        self.hand_out(ctx);
        if let Some(mut test) = self.test.take() {
            test.tick(self, ctx);
            self.test = Some(test);
        }
    }
    fn persist_egui_memory(&self) -> bool {
        false
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.viewer.ui(ui);
    }
}

pub fn start(canvas_id: &'static str, loading_id: &'static str, manifest: String, test: bool) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
        let Some(canvas) = document.get_element_by_id(canvas_id).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{canvas_id}\">");
            return;
        };
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| {
                    cadaddy_ui_egui::theme::apply(&cc.egui_ctx, egui::Theme::Dark, false);
                    let mut viewer = FieldViewer::new("share");
                    viewer.read_only = true;
                    viewer.progress = Some(0.0);
                    // An absolute, same-site manifest path, found from the page's own folder.
                    let path = location().and_then(|l| l.pathname().ok()).unwrap_or_default();
                    let url = if manifest.contains("://") || manifest.starts_with('/') {
                        manifest
                    } else {
                        format!("{}{manifest}", crate::folder_of(&path))
                    };
                    let net = Net { inbox: Rc::default(), ctx: cc.egui_ctx.clone() };
                    net.get(url.clone(), Msg::Manifest);
                    let mut shell = ShareShell {
                        viewer,
                        net,
                        manifest_url: url,
                        load: None,
                        hash: String::new(),
                        view: None,
                        test: test.then(ShareTest::new),
                        loaded_ms: None,
                    };
                    shell.follow_hash();
                    Ok(Box::new(shell))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(loading_id) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => {
                    el.set_inner_html(&format!("<p>The project page failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>"))
                }
            }
        }
    });
}

// ---------------- the page testing itself ----------------

/// `?sharetest`: once the model is in (or refused), the page uses itself as a visitor would and
/// reports each step: the model loaded from its sub-path, a level and a trade toggled, an element
/// clicked and its properties shown, a tag search, the section box, a view link round trip, and
/// the 3D view's frame time.
struct ShareTest {
    results: Vec<Value>,
    started: f64,
    /// Frames drawn in 3D and when that began (ms).
    frames: Option<(u32, f64)>,
    done: bool,
}

impl ShareTest {
    fn new() -> ShareTest {
        ShareTest { results: Vec::new(), started: now_ms(), frames: None, done: false }
    }

    fn record(&mut self, step: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        log::info!("sharetest: {} {step}: {detail}", if ok { "PASS" } else { "FAIL" });
        self.results.push(json!({ "step": step, "ok": ok, "detail": detail }));
    }

    fn tick(&mut self, shell: &mut ShareShell, ctx: &egui::Context) {
        if self.done {
            return;
        }
        ctx.request_repaint();
        let v = &mut shell.viewer;
        if v.denied {
            self.record("denied", true, "the server refused the model: the page says access is denied");
            return self.report(shell);
        }
        let loaded = shell.load.as_ref().is_some_and(ShareLoad::done) && shell.loaded_ms.is_some();
        if !loaded {
            if now_ms() - self.started > 60_000.0 {
                self.record("load", false, format!("the model did not load in 60 s ({})", v.status));
                self.report(shell);
            }
            return;
        }
        let Some((frames, since)) = self.frames else {
            self.steps(shell);
            // Then the whole level in 3D, for the frame time.
            let v = &mut shell.viewer;
            v.room = None;
            v.selected = None;
            v.section = None;
            v.hidden.clear();
            v.mode = Mode::Model;
            self.frames = Some((0, now_ms()));
            return;
        };
        let elapsed = now_ms() - since;
        if elapsed < 3_000.0 {
            self.frames = Some((frames + 1, since));
            return;
        }
        let ms = elapsed / f64::from(frames.max(1));
        let drawn = shell.viewer.drawn_3d;
        self.record("3d", frames > 3 && drawn > 0, format!("{drawn} elements in 3D, {ms:.1} ms a frame ({frames} frames)"));
        self.report(shell);
    }

    fn steps(&mut self, shell: &mut ShareShell) {
        let elements = shell.load.as_ref().map_or(0, |l| l.manifest.elements);
        let loaded_ms = shell.loaded_ms.unwrap_or_default();
        let v = &mut shell.viewer;
        let Some(p) = v.pkg.clone() else {
            return self.record("load", false, "no model");
        };
        let path = location().and_then(|l| l.pathname().ok()).unwrap_or_default();
        self.record(
            "load",
            p.elements.len() == elements && elements > 0,
            format!(
                "{} elements, {} rooms, {} levels in {loaded_ms:.0} ms after the page started (from {path})",
                p.elements.len(),
                p.rooms.len(),
                p.levels.len()
            ),
        );
        self.record(
            "subpath",
            path.matches('/').count() >= 3 && shell.manifest_url.starts_with(&crate::folder_of(&path)),
            format!("manifest {}", shell.manifest_url),
        );

        // A level.
        let before = v.level_name();
        v.level = 1.min(p.levels.len().saturating_sub(1));
        v.room = None;
        let name = v.level_name();
        let vis = v.visible();
        let on_level = vis.iter().filter_map(|i| p.elements.get(*i)).all(|e| e.level.eq_ignore_ascii_case(&name));
        self.record("level", name != before && !vis.is_empty() && on_level, format!("{before} -> {name}: {} elements shown", vis.len()));

        // A trade off and on.
        let trade = p.trade_counts().into_iter().max_by_key(|(_, n)| *n).map(|(k, _)| k).unwrap_or_default();
        v.hidden.insert(trade.clone());
        let without = v.visible();
        let none_left = without.iter().filter_map(|i| p.elements.get(*i)).all(|e| e.trade != trade);
        v.hidden.remove(&trade);
        self.record("trade", none_left && without.len() < vis.len(), format!("{trade} hidden: {} -> {} shown", vis.len(), without.len()));

        // Click an element (in plan, at its centre) and read its properties.
        let target = vis.iter().copied().find(|i| p.elements.get(*i).is_some_and(|e| !e.props.is_empty() || !e.mark.is_empty()));
        match target.and_then(|i| p.elements.get(i).map(|e| (i, e))) {
            Some((i, e)) => {
                v.mode = Mode::Plan;
                v.tool = cadaddy_ui_egui::fieldview::Tool::Select;
                v.tap_plan([(e.min[0] + e.max[0]) * 0.5, (e.min[1] + e.max[1]) * 0.5], 0.05);
                let props = v.properties();
                let ok = v.selected.and_then(|s| p.elements.get(s)).is_some_and(|x| x.id == e.id || x.level == e.level) && !props.is_empty();
                let text: Vec<String> = props.iter().map(|r| format!("{}={}", r.key, r.value)).take(8).collect();
                self.record("click", ok, format!("{} {}: {}", e.kind, e.id, text.join("; ")));
                v.selected = Some(i);
            }
            None => self.record("click", false, "no element with properties on the level"),
        }

        // A tag search.
        let mark = p.elements.iter().find(|e| !e.mark.is_empty()).map(|e| e.mark.clone()).unwrap_or_default();
        let hits = v.search_elements(&mark);
        self.record("search", !hits.is_empty(), format!("tag {mark:?}: {} found", hits.len()));

        // The section box: a lower half of the level.
        let all = v.visible().len();
        let (a, mut b) = v.default_section();
        b[0] = (a[0] + b[0]) * 0.5;
        v.section = Some((a, b));
        let cut = v.visible().len();
        self.record("section", cut < all, format!("{all} -> {cut} shown"));

        // A view link round trip.
        v.mode = Mode::Model;
        v.hidden.insert(trade.clone());
        let link = v.view_link();
        let text = link.encode();
        let back = ViewLink::decode(&format!("#view={text}"));
        let mut other = FieldViewer::new("check");
        other.open_package(p.clone());
        if let Some(l) = &back {
            other.apply_view(l);
        }
        let same = other.level == v.level && other.mode == v.mode && other.hidden == v.hidden && other.section.is_some();
        self.record("viewlink", same && back.as_ref() == Some(&link), format!("#view={text}"));
        v.hidden.clear();
        v.section = None;
    }

    fn report(&mut self, shell: &ShareShell) {
        self.done = true;
        let passed = self.results.iter().filter(|r| r["ok"] == true).count();
        let report = json!({ "steps": self.results, "passed": passed, "total": self.results.len(), "loaded_ms": shell.loaded_ms }).to_string();
        let url = format!(
            "{}selftest/result?data={}",
            crate::folder_of(&location().and_then(|l| l.pathname().ok()).unwrap_or_default()),
            js_sys::encode_uri_component(&report)
        );
        if let Some(w) = web_sys::window() {
            let _ = w.fetch_with_str(&url);
            if let Some(d) = w.document() {
                d.set_title(&format!("sharetest: {passed}/{} passed", self.results.len()));
            }
        }
    }
}
