//! PLOTSTAMP: what the plot stamp says and where it goes, and whether a sheet's plots carry it.
//! The settings are the drawing's ([`cadaddy_doc::plotstamp::PlotStamp`]); "on" belongs to each
//! layout's page setup (model space: the settings' `model`). PLOT fills in the drawing's name and
//! the date and time.

use cadaddy_doc::Space;
use cadaddy_doc::plotstamp::{MAX_STAMP_OFFSET_MM, PlotStamp, STAMP_FONTS, STAMP_HEIGHT_MM, StampCorner};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("plotstamp", "Plot Stamp...", run)
            .menu(&["File", "Plot Stamp..."])
            .params(
                "{on?: bool (this layout's page setup, or model space's), layout?: name | \"Model\" (default the current one), allLayouts?: bool (on/off for every layout), \
                 drawingName?, layoutName?, dateTime?, deviceName?, paperSize?, plotScale?: bool (the fields), userFields?: [text] (up to 8), \
                 corner?: bottomLeft|bottomRight|topLeft|topRight, offset?: [x, y], units?: mm|inches (of offset and height; default mm), fromPaperEdge?: bool, \
                 font?: Helvetica|Times|Courier, height?, singleLine?: bool, vertical?: bool, upsideDown?: bool, \
                 logFile?: bool, logPath?: path ending .log (each stamped plot adds a line; kept for this session, never saved with the drawing), \
                 reset?: bool (the default settings)} → {settings, on, layouts: [names that plot it], logFile, logPath}",
            ),
    ]
}

/// The stamp as it is now, and where it is on.
fn state(s: &Session) -> Result<Value> {
    let d = s.doc()?;
    let settings = d.plot_stamp.clone().unwrap_or_default();
    let mut layouts: Vec<String> = d.layouts.iter().filter(|l| l.page.plot_stamp).map(|l| l.name.clone()).collect();
    if settings.model {
        layouts.insert(0, "Model".into());
    }
    let on = match s.space() {
        Space::Model => settings.model,
        Space::Paper(n) => d.layout(&n).is_some_and(|l| l.page.plot_stamp),
    };
    Ok(json!({ "settings": settings, "on": on, "layouts": layouts, "logFile": settings.log_file, "logPath": settings.log_path }))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plotstamp";
    let d = s.doc()?;
    let mut st = if p.get("reset").and_then(Value::as_bool) == Some(true) { PlotStamp::default() } else { d.plot_stamp.clone().unwrap_or_default() };
    let flag = |k: &str| -> Result<Option<bool>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(bad(cmd, format!("`{k}` must be true or false"))),
        }
    };
    for (k, f) in [
        ("drawingName", &mut st.drawing_name),
        ("layoutName", &mut st.layout_name),
        ("dateTime", &mut st.date_time),
        ("deviceName", &mut st.device_name),
        ("paperSize", &mut st.paper_size),
        ("plotScale", &mut st.plot_scale),
        ("fromPaperEdge", &mut st.from_paper_edge),
        ("singleLine", &mut st.single_line),
        ("vertical", &mut st.vertical),
        ("upsideDown", &mut st.upside_down),
        ("logFile", &mut st.log_file),
    ] {
        if let Some(b) = flag(k)? {
            *f = b;
        }
    }
    if let Some(v) = p.get("userFields").filter(|v| !v.is_null()) {
        let list = v.as_array().ok_or_else(|| bad(cmd, "`userFields` must be a list of text"))?;
        if list.len() > cadaddy_doc::plotstamp::MAX_USER_FIELDS {
            return Err(bad(cmd, format!("at most {} user fields", cadaddy_doc::plotstamp::MAX_USER_FIELDS)));
        }
        st.user_fields = list
            .iter()
            .map(|f| f.as_str().map(str::to_string).ok_or_else(|| bad(cmd, "`userFields` must be a list of text")))
            .collect::<Result<_>>()?;
    }
    if let Some(c) = str_param(p, "corner") {
        st.corner = match c.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
            "bottomleft" | "lowerleft" | "bl" => StampCorner::BottomLeft,
            "bottomright" | "lowerright" | "br" => StampCorner::BottomRight,
            "topleft" | "upperleft" | "tl" => StampCorner::TopLeft,
            "topright" | "upperright" | "tr" => StampCorner::TopRight,
            _ => return Err(bad(cmd, "`corner` must be bottomLeft, bottomRight, topLeft or topRight")),
        };
    }
    let unit = match str_param(p, "units").map(str::to_ascii_lowercase).as_deref() {
        None => 1.0,
        Some("mm" | "millimeters" | "millimetres") => {
            st.inches = false;
            1.0
        }
        Some("in" | "inch" | "inches") => {
            st.inches = true;
            25.4
        }
        Some(_) => return Err(bad(cmd, "`units` must be mm or inches")),
    };
    if let Some(path) = str_param(p, "logPath") {
        let path = path.trim();
        if !path.is_empty() && !path.to_ascii_lowercase().ends_with(".log") {
            return Err(bad(cmd, "`logPath` must be a file ending .log"));
        }
        if path.chars().count() > cadaddy_doc::plotstamp::MAX_LOG_PATH_CHARS || path.chars().any(char::is_control) {
            return Err(bad(cmd, "`logPath` is not a usable path"));
        }
        st.log_path = path.to_string();
    }
    if st.log_file && st.log_path.is_empty() {
        return Err(bad(cmd, "the log file needs `logPath`"));
    }
    if let Some(v) = p.get("offset").filter(|v| !v.is_null()) {
        let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, "`offset` must be [x, y]"))?;
        let mut o = [0.0; 2];
        for (slot, x) in o.iter_mut().zip(a) {
            let mm = x.as_f64().map(|x| x * unit).filter(|x| x.is_finite() && x.abs() <= MAX_STAMP_OFFSET_MM);
            *slot = mm.ok_or_else(|| bad(cmd, format!("`offset` values must be numbers within {MAX_STAMP_OFFSET_MM} mm")))?;
        }
        st.offset_mm = o;
    }
    if let Some(v) = p.get("height").filter(|v| !v.is_null()) {
        let mm = v.as_f64().map(|h| h * unit).filter(|h| h.is_finite() && (STAMP_HEIGHT_MM.0..=STAMP_HEIGHT_MM.1).contains(h));
        st.height_mm = mm.ok_or_else(|| bad(cmd, format!("`height` must be from {} to {} mm", STAMP_HEIGHT_MM.0, STAMP_HEIGHT_MM.1)))?;
    }
    if let Some(f) = str_param(p, "font") {
        st.font = STAMP_FONTS
            .iter()
            .find(|n| n.eq_ignore_ascii_case(f.trim()))
            .map(|n| n.to_string())
            .ok_or_else(|| bad(cmd, format!("`font` must be one of {}", STAMP_FONTS.join(", "))))?;
    }
    let st = st.sanitized();
    // Where "on" goes: a layout's page setup, every layout's, or model space's.
    let target = match str_param(p, "layout") {
        Some(n) if n.eq_ignore_ascii_case("model") => Space::Model,
        Some(n) => Space::Paper(d.layout(n).map(|l| l.name.clone()).ok_or_else(|| bad(cmd, format!("no layout `{n}`")))?),
        None => s.space(),
    };
    let on = flag("on")?;
    let all = flag("allLayouts")?;
    let d = s.doc_mut()?;
    let mut st = st;
    if let (Some(on), Space::Model) = (on, &target) {
        st.model = on;
    }
    d.plot_stamp = Some(st);
    if let Some(on) = all {
        for l in &mut d.layouts {
            l.page.plot_stamp = on;
        }
    }
    if let (Some(on), Space::Paper(n)) = (on, &target)
        && let Some(l) = d.layouts.iter_mut().find(|l| &l.name == n)
    {
        l.page.plot_stamp = on;
    }
    state(s)
}

/// After a plot to `path` that carries the stamp, a line in the stamp's log file (when it is on):
/// the date and time, the drawing, the layout, the device, the paper and the plot file. Returns
/// the log's path, or why it could not be written (the plot itself stands).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn log_plot(s: &Session, space: &Space, opts: &Value, path: &str) -> Option<std::result::Result<String, String>> {
    let d = s.doc().ok()?;
    let st = d.plot_stamp.as_ref()?.sanitized();
    // Model space plots name no device or paper of their own here.
    let (on, device, paper) = match space {
        Space::Model => (st.model, String::new(), String::new()),
        Space::Paper(n) => {
            let l = d.layout(n)?;
            (l.page.plot_stamp, l.page.device.clone(), l.page.paper.clone())
        }
    };
    let on = opts.get("stamp").and_then(Value::as_bool).unwrap_or(on);
    if !(on && st.log_file) || st.log_path.is_empty() {
        return None;
    }
    let text = |k: &str| opts.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let line = [text("stampDate"), text("stampDrawing"), space.tab_name().to_string(), device, paper, path.to_string()]
        .map(|f| f.replace(['\r', '\n'], " "))
        .join(", ");
    use std::io::Write;
    let wrote = std::fs::OpenOptions::new().create(true).append(true).open(&st.log_path).and_then(|mut f| writeln!(f, "{line}"));
    Some(wrote.map(|()| st.log_path.clone()).map_err(|e| format!("{}: {e}", st.log_path)))
}

/// The plot's stamp facts PLOT adds to its options: the drawing's file name (else its title) and
/// the date and time now, unless given.
pub(crate) fn facts(s: &Session, opts: &mut Value) {
    let name = s.state().ok().map(|st| {
        st.path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| st.title.clone())
    });
    facts_for(name.as_deref().unwrap_or(""), opts);
}

/// [`facts`] for a drawing called `name` (a file plotted without opening it: PUBLISH).
pub(crate) fn facts_for(name: &str, opts: &mut Value) {
    let Some(o) = opts.as_object_mut() else { return };
    if !o.contains_key("stampDrawing") {
        o.insert("stampDrawing".into(), json!(name));
    }
    if !o.contains_key("stampDate")
        && let Some(t) = crate::fields::now()
    {
        o.insert("stampDate".into(), json!(format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute)));
    }
}
