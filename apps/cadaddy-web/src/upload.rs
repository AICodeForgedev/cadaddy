//! Browser uploads: File > Open… on the web, through web-sys only. There is no file system to pick
//! a path from, so the picker is a hidden `<input type="file">`; the chosen file (or one dropped on
//! the page) is read in the background and handed to the app on its next frame, which opens it
//! with OPEN `{data, name}` ([`CadApp::open_bytes`]).

use std::cell::Cell;
use std::rc::Rc;

use cadaddy_ui_egui::CadApp;
use wasm_bindgen::JsCast as _;
use web_sys::js_sys;

/// The hidden file input (one at a time; a cancelled pick leaves it until the next one).
const INPUT_ID: &str = "cadaddy_open_input";
/// What the picker offers: the formats OPEN reads (and walkthrough packages, opened in Walk).
const ACCEPT: &str = ".dxf,.dwg,.cdy,.cdywalk";

/// A file read in the browser (name, contents), or why it couldn't be read.
type Upload = Result<(String, Vec<u8>), String>;

/// Files read in the background, waiting for the app's next frame.
#[derive(Clone)]
pub struct Inbox {
    files: Rc<Cell<Vec<Upload>>>,
    ctx: egui::Context,
}

impl Inbox {
    pub fn new(ctx: &egui::Context) -> Self {
        Inbox { files: Rc::default(), ctx: ctx.clone() }
    }

    /// Show the browser's file picker and return at once. A chosen file arrives through
    /// [`Self::drain`]; cancelling the picker does nothing.
    pub fn pick(&self) {
        if let Err(e) = self.show_picker() {
            self.push(Err(format!("Open: the browser's file picker is not available ({e})")));
        }
    }

    fn show_picker(&self) -> Result<(), String> {
        let document = web_sys::window().and_then(|w| w.document()).ok_or("no browser document")?;
        let body = document.body().ok_or("the page has no body")?;
        if let Some(old) = document.get_element_by_id(INPUT_ID) {
            old.remove();
        }
        let input = document
            .create_element("input")
            .map_err(js_error)?
            .dyn_into::<web_sys::HtmlInputElement>()
            .map_err(|_| "could not create a file input".to_string())?;
        input.set_type("file");
        input.set_accept(ACCEPT);
        input.set_id(INPUT_ID);
        input.set_attribute("style", "display:none").map_err(js_error)?;
        let (inbox, chosen) = (self.clone(), input.clone());
        // Called once, when a file was chosen (browsers don't fire `change` on cancel).
        let on_change = wasm_bindgen::closure::Closure::once_into_js(move || {
            if let Some(file) = chosen.files().and_then(|f| f.get(0)) {
                inbox.read(file);
            }
            chosen.remove();
        });
        input.set_onchange(Some(on_change.unchecked_ref()));
        body.append_child(&input).map_err(js_error)?;
        input.click();
        Ok(())
    }

    /// Read the files dropped on the page this frame (eframe hands them over unread on the web).
    pub fn take_dropped(&self, ctx: &egui::Context) {
        for f in ctx.input(|i| i.raw.dropped_files.clone()) {
            if let Some(file) = f.web_file() {
                self.read(file.clone());
            }
        }
    }

    /// Read `file` in the background; it arrives through [`Self::drain`]. A file too large to
    /// open is refused before it is read.
    fn read(&self, file: web_sys::File) {
        let name = file.name();
        // `File.size` is a whole number of bytes; anything else counts as too large.
        let size = file.size();
        let len = if size.is_finite() && size >= 0.0 { size as u64 } else { u64::MAX };
        if let Some(e) = CadApp::open_size_error(&name, len) {
            return self.push(Err(e));
        }
        let inbox = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let r = match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
                Ok(buf) => Ok((name, js_sys::Uint8Array::new(&buf).to_vec())),
                Err(e) => Err(format!("Open: could not read {name}: {}", js_error(e))),
            };
            inbox.push(r);
        });
    }

    /// A file read elsewhere (sent by the page embedding CADaddy): opened on the next frame like
    /// a picked file, or refused when too large to open.
    pub fn receive(&self, name: String, bytes: Vec<u8>) {
        match CadApp::open_size_error(&name, bytes.len() as u64) {
            Some(e) => self.push(Err(e)),
            None => self.push(Ok((name, bytes))),
        }
    }

    /// Fetch the walkthrough package (or drawing) at `url` (this site, or one that allows it) and
    /// open it on a later frame: `?walk=URL` links straight to a walk.
    pub fn fetch(&self, url: String) {
        let inbox = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let name = crate::file_name_of(&url);
            let r = async {
                let window = web_sys::window().ok_or("no browser window")?;
                let resp = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(&url)).await.map_err(js_error)?;
                let resp: web_sys::Response = resp.dyn_into().map_err(|_| "not a response".to_string())?;
                if !resp.ok() {
                    return Err(format!("HTTP {}", resp.status()));
                }
                let buf = wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(js_error)?).await.map_err(js_error)?;
                Ok::<_, String>(js_sys::Uint8Array::new(&buf).to_vec())
            }
            .await;
            match r {
                Ok(bytes) => inbox.receive(name, bytes),
                Err(e) => inbox.refuse(format!("Open: could not fetch {url}: {e}")),
            }
        });
    }

    /// A file from elsewhere refused before it was read (`error` says why).
    pub fn refuse(&self, error: String) {
        self.push(Err(error));
    }

    fn push(&self, upload: Upload) {
        let mut files = self.files.take();
        files.push(upload);
        self.files.set(files);
        self.ctx.request_repaint();
    }

    /// Open what arrived since the last frame (or report why it couldn't be read).
    pub fn drain(&self, app: &mut CadApp) {
        for upload in self.files.take() {
            match upload {
                Ok((name, bytes)) => app.open_bytes(&name, &bytes),
                Err(e) => app.open_failed(e),
            }
        }
    }
}

fn js_error(e: wasm_bindgen::JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}
