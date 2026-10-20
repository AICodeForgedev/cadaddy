//! Field evaluation: the values of the field codes in text ([`cadaddy_doc::field`]), worked out
//! when the drawing is opened, saved, plotted or regenerated (FIELDEVAL bits 1, 2, 4 and 16), by
//! UPDATEFIELD, and for text a command creates or changes. The values are kept in
//! [`Drawing::field_values`]; what isn't worked out keeps the value it had.
//!
//! Fields read: `\AcVar` Date, CreateDate, SaveDate, PlotDate, Filename, Login,
//! CurrentSheetName / CurrentSheetNumber / CurrentSheetTitle (the layout's name, and its two parts
//! — "A-101 Floor Plan" is sheet A-101, "Floor Plan"), DeviceName and PageSetupName; `\AcSm`
//! Sheet.Number / Sheet.Title (the same, from layout names); `\AcObjProp` Object(…).Area,
//! Perimeter, Length, Radius, Diameter, Circumference, Layer, Linetype, Color, TextString, Name
//! and Attribute(TAG) (a block reference's attribute value).

use std::collections::BTreeMap;
use std::sync::Arc;

use cadaddy_doc::field::{self, Code};
use cadaddy_doc::{Drawing, EntityKind, Handle};

use crate::Session;

/// When fields are worked out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Open,
    Save,
    Plot,
    Regen,
    /// UPDATEFIELD, and text a command just made or changed: always.
    Update,
}

impl Event {
    /// The FIELDEVAL bit that turns this event's evaluation on.
    fn bit(self) -> Option<i64> {
        match self {
            Event::Open => Some(1),
            Event::Save => Some(2),
            Event::Plot => Some(4),
            Event::Regen => Some(16),
            Event::Update => None,
        }
    }
}

/// A local date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i64,
    /// 1..=12.
    pub month: u32,
    /// 1..=31.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0 = Sunday.
    pub weekday: u32,
}

impl LocalTime {
    /// The calendar date and time `secs` seconds after 1970-01-01 00:00.
    pub fn from_secs(secs: i64) -> LocalTime {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        // Civil date from a day count (proleptic Gregorian).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        LocalTime {
            year,
            month: u32::try_from(month).unwrap_or(1),
            day: u32::try_from(day).unwrap_or(1),
            hour: u32::try_from(rem / 3600).unwrap_or(0),
            minute: u32::try_from(rem % 3600 / 60).unwrap_or(0),
            second: u32::try_from(rem % 60).unwrap_or(0),
            weekday: u32::try_from((days + 4).rem_euclid(7)).unwrap_or(0),
        }
    }

    /// Seconds after 1970-01-01 00:00 (the inverse of [`LocalTime::from_secs`]).
    pub fn to_secs(&self) -> i64 {
        let (y, m) = (self.year - i64::from(self.month <= 2), i64::from(self.month));
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        days * 86_400 + i64::from(self.hour) * 3600 + i64::from(self.minute) * 60 + i64::from(self.second)
    }

    /// The date as a Julian day number with the time as its fraction (TDUPDATE, TDCREATE).
    pub fn julian(&self) -> f64 {
        2_440_587.5 + self.to_secs() as f64 / 86_400.0
    }

    /// The date of a Julian day number; `None` when it isn't a plausible date.
    pub fn from_julian(j: f64) -> Option<LocalTime> {
        if !(j.is_finite() && (1_721_426.0..5_373_484.0).contains(&j)) {
            return None;
        }
        Some(LocalTime::from_secs(((j - 2_440_587.5) * 86_400.0).round() as i64))
    }
}

/// A clock the host provides (the web build, tests); `None` uses the system clock.
type Clock = fn() -> Option<LocalTime>;
static CLOCK: std::sync::RwLock<Option<Clock>> = std::sync::RwLock::new(None);

/// Use `clock` for the current local time (the web build sets one; tests pin the time).
pub fn set_clock(clock: Option<Clock>) {
    if let Ok(mut c) = CLOCK.write() {
        *c = clock;
    }
}

/// The current local time: the host's clock, else the system clock in the system time zone
/// (`TZ`, else `/etc/localtime`; UTC when neither says).
pub fn now() -> Option<LocalTime> {
    if let Some(f) = CLOCK.read().ok().and_then(|c| *c) {
        return f();
    }
    system_now()
}

#[cfg(not(target_arch = "wasm32"))]
fn system_now() -> Option<LocalTime> {
    let utc = i64::try_from(web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).ok()?.as_secs()).ok()?;
    Some(LocalTime::from_secs(utc + tz::offset(utc)))
}

#[cfg(target_arch = "wasm32")]
fn system_now() -> Option<LocalTime> {
    None
}

/// The FIELDEVAL bits (31 = every event).
fn fieldeval(d: &Drawing) -> i64 {
    d.header.i64("FIELDEVAL", 31)
}

/// Work out the fields of the active drawing for `event`, if FIELDEVAL says to; the number of
/// objects whose values changed.
pub fn on_event(s: &mut Session, event: Event) -> usize {
    let Ok(st) = s.state() else { return 0 };
    if event.bit().is_some_and(|b| fieldeval(&st.doc) & b == 0) {
        return 0;
    }
    evaluate(s, event, None) + crate::cmd::inquiry::update_count_tables(s)
}

/// On saving: the save date (TDUPDATE) becomes now, then fields are worked out if FIELDEVAL says
/// to.
pub fn on_save(s: &mut Session) {
    // In the Block Editor the drawing open is the editor's: the file's drawing waits untouched.
    if s.state().is_ok_and(|st| st.block_edit.is_some()) {
        return;
    }
    if let Some(t) = now()
        && let Ok(d) = s.doc_mut()
    {
        d.header.set_f64("TDUPDATE", t.julian());
        if d.header.get("TDCREATE").is_none() {
            d.header.set_f64("TDCREATE", t.julian());
        }
    }
    on_event(s, Event::Save);
}

/// Work out the fields of `only` (or of every object) in the active drawing; the number of objects
/// whose values changed.
pub fn evaluate(s: &mut Session, event: Event, only: Option<&[Handle]>) -> usize {
    let Ok(st) = s.state() else { return 0 };
    // Not inside the Block Editor (its drawing is a working copy; closing it brings the blocks
    // back, and their fields are worked out then).
    if st.block_edit.is_some() {
        return 0;
    }
    let env = Env { path: st.path.clone(), title: st.title.clone(), now: now(), event };
    let doc = Arc::clone(&st.doc);
    let mut changes: BTreeMap<Handle, Vec<String>> = BTreeMap::new();
    let mut stale: Vec<Handle> = Vec::new();
    for (sheet, e) in all_entities(&doc) {
        if only.is_some_and(|hs| !hs.contains(&e.handle)) {
            continue;
        }
        let texts = field::texts(&e.kind);
        if !texts.iter().any(|t| field::has_fields(t)) {
            if only.is_none() && doc.field_values.contains_key(&e.handle) {
                stale.push(e.handle);
            }
            continue;
        }
        let old = doc.field_values.get(&e.handle);
        let vals = values(&doc, &env, &texts, &sheet, old);
        if old != Some(&vals) {
            changes.insert(e.handle, vals);
        }
    }
    if only.is_none() {
        stale.extend(doc.field_values.keys().filter(|h| doc.entity(**h).is_none() && !in_blocks(&doc, **h)).copied());
    }
    let n = changes.len();
    if n == 0 && stale.is_empty() {
        return 0;
    }
    if let Ok(d) = s.doc_mut() {
        for h in stale {
            d.field_values.remove(&h);
        }
        d.field_values.extend(changes);
    }
    n
}

/// Whether `h` is an object inside a block definition.
fn in_blocks(d: &Drawing, h: Handle) -> bool {
    d.blocks.values().any(|b| b.entities.contains(h))
}

/// Every object with the sheet its sheet fields read: model space ("Model"), each layout, and
/// block definitions (read as model space).
fn all_entities(d: &Drawing) -> Vec<(String, Arc<cadaddy_doc::Entity>)> {
    let mut out: Vec<(String, Arc<cadaddy_doc::Entity>)> = d.model.iter().map(|e| ("Model".to_string(), Arc::clone(e))).collect();
    for l in &d.layouts {
        out.extend(l.entities.iter().map(|e| (l.name.clone(), Arc::clone(e))));
    }
    for b in d.blocks.values() {
        out.extend(b.entities.iter().map(|e| ("Model".to_string(), Arc::clone(e))));
    }
    out
}

/// The post-command pass: text the command made or changed gets its fields worked out (a copy
/// keeps nothing of its original's values, a new FIELD needs its first value).
pub fn after_command(s: &mut Session, before: Option<&Arc<Drawing>>) {
    let Ok(st) = s.state() else { return };
    let after = Arc::clone(&st.doc);
    if before.is_some_and(|b| Arc::ptr_eq(b, &after)) {
        return;
    }
    let mut touched: Vec<Handle> = Vec::new();
    let mut look = |store: &cadaddy_doc::EntityStore, prev: Option<&cadaddy_doc::EntityStore>| {
        let known: std::collections::HashSet<usize> = prev.map(|p| p.chunk_slices().map(|(k, _)| k).collect()).unwrap_or_default();
        for (key, ents) in store.chunk_slices() {
            if known.contains(&key) {
                continue;
            }
            for e in ents {
                if !field::kind_has_fields(&e.kind) {
                    continue;
                }
                let same = before.and_then(|b| b.entity(e.handle)).is_some_and(|p| Arc::ptr_eq(p, e));
                if !same || !after.field_values.contains_key(&e.handle) {
                    touched.push(e.handle);
                }
            }
        }
    };
    look(&after.model, before.map(|b| &b.model));
    for l in &after.layouts {
        let prev = before.and_then(|b| b.layouts.iter().find(|x| x.name == l.name)).map(|x| &x.entities);
        look(&l.entities, prev);
    }
    for (name, b) in &after.blocks {
        if before.and_then(|p| p.blocks.get(name)).is_some_and(|p| Arc::ptr_eq(p, b)) {
            continue;
        }
        touched.extend(b.entities.iter().filter(|e| field::kind_has_fields(&e.kind)).map(|e| e.handle));
    }
    if !touched.is_empty() {
        evaluate(s, Event::Update, Some(&touched));
    }
}

/// What a field reads besides the drawing.
struct Env {
    path: Option<String>,
    title: String,
    now: Option<LocalTime>,
    event: Event,
}

/// The values of the fields in `texts`, in order; `old` holds the values they had (kept by
/// fields this event doesn't work out, such as PlotDate outside a plot).
fn values(d: &Drawing, env: &Env, texts: &[&str], sheet: &str, old: Option<&Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    for t in texts {
        for code in field::codes(t) {
            let prev = old.and_then(|o| o.get(out.len())).map(String::as_str);
            out.push(value(d, env, code, sheet, prev));
        }
    }
    out
}

/// The value of one field code (without its `%<` `>%`).
fn value(d: &Drawing, env: &Env, code: &str, sheet: &str, prev: Option<&str>) -> String {
    let c = Code::parse(code);
    let raw = match c.evaluator.to_ascii_lowercase().as_str() {
        "acvar" => var(d, env, &c, sheet, prev),
        "acsm" => match c.expr.to_ascii_lowercase().as_str() {
            "sheet.number" => Some(sheet_parts(sheet).0),
            "sheet.title" => Some(sheet_parts(sheet).1),
            _ => prev.map(str::to_string),
        },
        "acobjprop" => object_property(d, &c),
        _ => prev.map(str::to_string),
    };
    match raw {
        Some(v) if v.is_empty() => "----".into(),
        Some(v) => text_case(&v, c.format.as_deref()),
        None => field::NO_VALUE.into(),
    }
}

/// A sheet's number and title from its layout name: "A-101 Floor Plan" → ("A-101", "Floor
/// Plan"); "A-101 - Floor Plan" too; a name of one word is the number and the title.
fn sheet_parts(name: &str) -> (String, String) {
    let name = name.trim();
    for sep in [" - ", " – ", " — ", ": "] {
        if let Some((a, b)) = name.split_once(sep) {
            return (a.trim().to_string(), b.trim().to_string());
        }
    }
    match name.split_once(char::is_whitespace) {
        Some((a, b)) => (a.to_string(), b.trim().to_string()),
        None => (name.to_string(), name.to_string()),
    }
}

/// An `\AcVar` field.
fn var(d: &Drawing, env: &Env, c: &Code, sheet: &str, prev: Option<&str>) -> Option<String> {
    let fmt = c.format.as_deref();
    let date = |t: Option<LocalTime>| t.map(|t| format_date(&t, fmt.filter(|f| !f.starts_with('%')).unwrap_or("M/d/yyyy")));
    let julian = |name: &str| d.header.get(name).and_then(cadaddy_doc::HVal::as_f64).and_then(LocalTime::from_julian);
    let page = d.layouts.iter().find(|l| l.name == sheet).map(|l| &l.page);
    match c.expr.to_ascii_lowercase().as_str() {
        "date" => date(env.now),
        "createdate" => date(julian("TDCREATE")),
        "savedate" => date(julian("TDUPDATE")),
        "plotdate" if env.event == Event::Plot => date(env.now),
        "plotdate" => prev.map(str::to_string),
        "filename" => Some(filename(env, fmt)),
        "login" => Some(login()),
        "currentsheetname" => Some(sheet.to_string()),
        "currentsheetnumber" => Some(sheet_parts(sheet).0),
        "currentsheettitle" => Some(sheet_parts(sheet).1),
        "devicename" => Some(page.map(|p| p.device.clone()).unwrap_or_default()),
        "pagesetupname" => Some(String::new()),
        _ => prev.map(str::to_string),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn login() -> String {
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default()
}

#[cfg(target_arch = "wasm32")]
fn login() -> String {
    String::new()
}

/// The drawing's file name as `%fnN` asks: bit 1 the folder only, bit 2 with the extension,
/// bit 4 without the folder (`%fn2`, the default: the whole path).
fn filename(env: &Env, fmt: Option<&str>) -> String {
    let full = env.path.clone().unwrap_or_else(|| env.title.clone());
    let n = fmt.and_then(|f| f.trim().strip_prefix("%fn")).and_then(|d| d.chars().next()).and_then(|c| c.to_digit(10)).unwrap_or(2);
    let p = std::path::Path::new(&full);
    let dir = p.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    if n & 1 != 0 {
        return dir;
    }
    let name = if n & 2 != 0 { p.file_name() } else { p.file_stem() }.map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if n & 4 != 0 || dir.is_empty() { name } else { p.with_file_name(&name).to_string_lossy().to_string() }
}

/// `%tc1` upper case, `%tc2` lower case, `%tc3` the first letter capital, `%tc4` each word's first
/// letter capital.
fn text_case(v: &str, fmt: Option<&str>) -> String {
    let Some(f) = fmt else { return v.to_string() };
    let case = f.find("%tc").and_then(|i| f.get(i + 3..i + 4)).and_then(|d| d.parse::<u8>().ok());
    match case {
        Some(1) => v.to_uppercase(),
        Some(2) => v.to_lowercase(),
        Some(3) => {
            let mut cs = v.chars();
            cs.next().map(|c| c.to_uppercase().chain(cs).collect()).unwrap_or_default()
        }
        Some(4) => {
            let mut out = String::with_capacity(v.len());
            let mut start = true;
            for c in v.chars() {
                if start && c.is_alphabetic() {
                    out.extend(c.to_uppercase());
                } else {
                    out.push(c);
                }
                start = c.is_whitespace();
            }
            out
        }
        _ => v.to_string(),
    }
}

/// A date in a .NET-style pattern: d dd ddd dddd, M MM MMM MMMM, yy yyyy, h hh H HH, m mm, s ss,
/// tt; other characters as they are, text between single quotes literally.
pub fn format_date(t: &LocalTime, fmt: &str) -> String {
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    let month = MONTHS.get(t.month.saturating_sub(1) as usize).copied().unwrap_or("");
    let wday = DAYS.get(t.weekday as usize).copied().unwrap_or("");
    let h12 = if t.hour.is_multiple_of(12) { 12 } else { t.hour % 12 };
    let chars: Vec<char> = fmt.chars().take(256).collect();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let run = chars.get(i..).map_or(0, |r| r.iter().take_while(|x| **x == c).count()).max(1);
        match c {
            '\'' => {
                let lit: String = chars.get(i + 1..).unwrap_or(&[]).iter().take_while(|x| **x != '\'').collect();
                i += lit.chars().count() + 2;
                out.push_str(&lit);
                continue;
            }
            'd' => out.push_str(&match run {
                1 => t.day.to_string(),
                2 => format!("{:02}", t.day),
                3 => wday.chars().take(3).collect(),
                _ => wday.to_string(),
            }),
            'M' => out.push_str(&match run {
                1 => t.month.to_string(),
                2 => format!("{:02}", t.month),
                3 => month.chars().take(3).collect(),
                _ => month.to_string(),
            }),
            'y' => out.push_str(&if run <= 2 { format!("{:02}", t.year.rem_euclid(100)) } else { format!("{:04}", t.year) }),
            'h' => out.push_str(&if run == 1 { h12.to_string() } else { format!("{h12:02}") }),
            'H' => out.push_str(&if run == 1 { t.hour.to_string() } else { format!("{:02}", t.hour) }),
            'm' => out.push_str(&if run == 1 { t.minute.to_string() } else { format!("{:02}", t.minute) }),
            's' => out.push_str(&if run == 1 { t.second.to_string() } else { format!("{:02}", t.second) }),
            't' => out.push_str(if t.hour < 12 {
                if run == 1 { "A" } else { "AM" }
            } else if run == 1 {
                "P"
            } else {
                "PM"
            }),
            _ => {
                for _ in 0..run {
                    out.push(c);
                }
            }
        }
        i += run;
    }
    out
}

/// An `\AcObjProp` field: a property of the object the code names.
fn object_property(d: &Drawing, c: &Code) -> Option<String> {
    let h = *c.objects().first()?;
    let e = d.entity(h).cloned().or_else(|| d.blocks.values().find_map(|b| b.entities.get(h).cloned()))?;
    let prop = c.expr.rsplit_once(").").map(|(_, p)| p.trim()).unwrap_or("");
    let lower = prop.to_ascii_lowercase();
    let fmt = c.format.as_deref();
    let num = |v: f64, area: bool| Some(format_number(d, v, fmt, area));
    if let Some(tag) = lower.strip_prefix("attribute(").and_then(|t| t.strip_suffix(')')) {
        let EntityKind::Insert(ins) = &e.kind else { return None };
        return ins.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case(tag.trim())).map(|a| a.text.value.clone());
    }
    match lower.as_str() {
        "area" => num(crate::cmd::inquiry::kind_area(&e.kind).ok()?.0, true),
        "perimeter" => num(crate::cmd::inquiry::kind_area(&e.kind).ok()?.1, false),
        "length" => num(length(&e.kind)?, false),
        "radius" | "diameter" | "circumference" => {
            let r = match &e.kind {
                EntityKind::Circle(c) => c.radius,
                EntityKind::Arc(a) => a.radius,
                _ => return None,
            };
            num(
                match lower.as_str() {
                    "radius" => r,
                    "diameter" => 2.0 * r,
                    _ => cadaddy_geom::TAU * r,
                },
                false,
            )
        }
        "layer" => Some(e.common.layer.clone()),
        "linetype" => Some(e.common.linetype.clone()),
        "color" => Some(e.common.color.name()),
        "textstring" | "contents" | "value" => match &e.kind {
            EntityKind::Text(t) => Some(t.value.clone()),
            EntityKind::MText(m) => Some(cadaddy_fonts::plain_mtext(&m.contents)),
            _ => None,
        },
        "name" | "effectivename" => match &e.kind {
            EntityKind::Insert(ins) => Some(ins.block.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// The length of a curve.
fn length(k: &EntityKind) -> Option<f64> {
    use cadaddy_geom::Polyline;
    Some(match k {
        EntityKind::Line(l) => l.a.xy().dist(l.b.xy()),
        EntityKind::Arc(a) => a.radius * (a.end - a.start).rem_euclid(cadaddy_geom::TAU),
        EntityKind::Circle(c) => cadaddy_geom::TAU * c.radius,
        EntityKind::LwPolyline(p) => Polyline { vertices: p.vertices.clone(), closed: p.closed }.len(),
        EntityKind::Spline(s) => s.length(),
        EntityKind::Ellipse(_) => crate::cmd::inquiry::kind_area(k).ok()?.1,
        _ => return None,
    })
}

/// A number in a field's format: `%lu` units (1 scientific, 2 decimal, 3 engineering, 4
/// architectural, 5 fractional; default LUNITS), `%pr` precision (default LUPREC), `%ct8[k]` a
/// factor, `%ps[before,after]` text around it. An area is shown as a decimal number.
fn format_number(d: &Drawing, v: f64, fmt: Option<&str>, area: bool) -> String {
    let f = fmt.unwrap_or("");
    let digit_after = |key: &str| f.find(key).and_then(|i| f.get(i + key.len()..)).and_then(|r| r.chars().next()).and_then(|c| c.to_digit(10));
    let lu = digit_after("%lu").map_or(d.header.i64("LUNITS", 2), i64::from);
    let pr = digit_after("%pr").map_or(d.header.i64("LUPREC", 4), i64::from).clamp(0, 8);
    let bracket = |key: &str| {
        f.find(key).and_then(|i| f.get(i + key.len()..)).and_then(|r| r.strip_prefix('[')).and_then(|r| r.split_once(']')).map(|(a, _)| a)
    };
    let k = bracket("%ct8").and_then(|x| x.trim().parse::<f64>().ok()).filter(|k| k.is_finite()).unwrap_or(1.0);
    let v = v * k;
    let body = if area || lu == 2 {
        let p = usize::try_from(pr).unwrap_or(4);
        if lu == 1 { format!("{v:.p$E}") } else { format!("{v:.p$}") }
    } else {
        crate::units::format_distance(v, lu, pr)
    };
    match bracket("%ps").and_then(|x| x.split_once(',')) {
        Some((a, b)) => format!("{a}{body}{b}"),
        None => body,
    }
}

/// Seconds to add to UTC `utc` (seconds since 1970) for the system's local time (0 where it is
/// unknown, the web build).
pub fn local_offset(utc: i64) -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tz::offset(utc)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = utc;
        0
    }
}

/// The local time-zone offset, from the POSIX `TZ` variable or the system's time-zone file
/// (`/etc/localtime`, TZif); on Windows, the system's time zone.
#[cfg(not(target_arch = "wasm32"))]
mod tz {
    /// Seconds to add to UTC `utc` for local time.
    pub fn offset(utc: i64) -> i64 {
        #[cfg(windows)]
        if std::env::var_os("TZ").is_none() {
            use chrono::{Offset, TimeZone};
            return chrono::DateTime::from_timestamp(utc, 0)
                .map_or(0, |t| i64::from(chrono::Local.offset_from_utc_datetime(&t.naive_utc()).fix().local_minus_utc()));
        }
        if let Ok(tz) = std::env::var("TZ")
            && let Some(r) = Rule::parse(tz.trim_start_matches(':'))
        {
            return r.offset(utc);
        }
        std::fs::read("/etc/localtime").ok().and_then(|b| tzif(&b, utc)).unwrap_or(0)
    }

    /// The offset a TZif file gives for `utc`: the type of the last transition before it, else
    /// its footer rule.
    pub fn tzif(b: &[u8], utc: i64) -> Option<i64> {
        if b.get(..4) != Some(b"TZif") {
            return None;
        }
        let v2 = b.get(4).is_some_and(|v| *v >= b'2');
        let counts = |at: usize| -> Option<[usize; 6]> {
            let mut c = [0usize; 6];
            for (k, slot) in c.iter_mut().enumerate() {
                let o = at + 20 + 4 * k;
                *slot = u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize;
            }
            Some(c)
        };
        let [isut, isstd, leap, timecnt, typecnt, charcnt] = counts(0)?;
        let v1_len = timecnt * 5 + typecnt * 6 + charcnt + leap * 8 + isstd + isut;
        let (base, tsize, [isut, isstd, leap, timecnt, typecnt, charcnt]) = if v2 {
            let at = 44 + v1_len;
            (at + 44, 8, counts(at)?)
        } else {
            (44, 4, [isut, isstd, leap, timecnt, typecnt, charcnt])
        };
        if timecnt > 1_000_000 || typecnt == 0 || typecnt > 256 {
            return None;
        }
        let time = |i: usize| -> Option<i64> {
            let o = base + i * tsize;
            let s = b.get(o..o + tsize)?;
            Some(if tsize == 8 { i64::from_be_bytes(s.try_into().ok()?) } else { i64::from(i32::from_be_bytes(s.try_into().ok()?)) })
        };
        let idx_at = base + timecnt * tsize;
        let types_at = idx_at + timecnt;
        let utoff = |t: usize| -> Option<i64> {
            let o = types_at + t * 6;
            Some(i64::from(i32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?)))
        };
        let mut found = None;
        for i in 0..timecnt {
            if time(i)? > utc {
                break;
            }
            found = Some(i);
        }
        // Past the last transition: the footer's rule (v2+), which slim files rely on.
        if v2 && (found == Some(timecnt.saturating_sub(1)) || timecnt == 0) {
            let foot = types_at + typecnt * 6 + charcnt + leap * (tsize + 4) + isstd + isut;
            if let Some(rest) = b.get(foot..)
                && let Ok(text) = std::str::from_utf8(rest)
                && let Some(rule) = text.trim_matches('\n').lines().next().and_then(Rule::parse)
            {
                return Some(rule.offset(utc));
            }
        }
        match found {
            Some(i) => utoff(usize::from(*b.get(idx_at + i)?)),
            None => utoff(0),
        }
    }

    /// A POSIX TZ rule: `CST6CDT,M3.2.0,M11.1.0`.
    #[derive(Debug, PartialEq)]
    pub struct Rule {
        std: i64,
        dst: Option<(i64, Change, Change)>,
    }

    /// When daylight time starts or ends: month, week (5 = last), weekday, seconds after
    /// midnight local time.
    #[derive(Debug, PartialEq)]
    struct Change {
        month: u32,
        week: u32,
        weekday: u32,
        secs: i64,
    }

    impl Rule {
        pub fn parse(s: &str) -> Option<Rule> {
            let s = s.trim();
            let (rest, _) = name(s)?;
            let (rest, std_off) = offset_of(rest)?;
            let std = -std_off;
            if rest.is_empty() {
                return Some(Rule { std, dst: None });
            }
            let (rest, _) = name(rest)?;
            let (rest, dst) = match offset_of(rest) {
                Some((r, o)) if !rest.starts_with(',') => (r, -o),
                _ => (rest, std + 3600),
            };
            let mut parts = rest.strip_prefix(',')?.split(',');
            let start = change(parts.next()?)?;
            let end = change(parts.next()?)?;
            Some(Rule { std, dst: Some((dst, start, end)) })
        }

        pub fn offset(&self, utc: i64) -> i64 {
            let Some((dst, start, end)) = &self.dst else { return self.std };
            let year = super::LocalTime::from_secs(utc + self.std).year;
            // Both changes in UTC: the start in standard time, the end in daylight time.
            let on = start.at(year) - self.std;
            let off = end.at(year) - dst;
            let in_dst = if on < off { utc >= on && utc < off } else { utc >= on || utc < off };
            if in_dst { *dst } else { self.std }
        }
    }

    impl Change {
        /// Local seconds of the change in `year`.
        fn at(&self, year: i64) -> i64 {
            let first = super::LocalTime { year, month: self.month, day: 1, hour: 0, minute: 0, second: 0, weekday: 0 };
            let first_secs = first.to_secs();
            let first_wday = super::LocalTime::from_secs(first_secs).weekday;
            let mut day = 1 + (self.weekday + 7 - first_wday) % 7 + 7 * (self.week.saturating_sub(1));
            let next_month = if self.month == 12 {
                super::LocalTime { year: year + 1, month: 1, ..first }
            } else {
                super::LocalTime { month: self.month + 1, ..first }
            };
            let days_in = u32::try_from((next_month.to_secs() - first_secs) / 86_400).unwrap_or(31);
            while day > days_in {
                day -= 7;
            }
            first_secs + i64::from(day - 1) * 86_400 + self.secs
        }
    }

    /// A zone name (`CST`, or `<+0530>`); the rest of `s`.
    fn name(s: &str) -> Option<(&str, &str)> {
        if let Some(r) = s.strip_prefix('<') {
            let end = r.find('>')?;
            return Some((r.get(end + 1..)?, r.get(..end)?));
        }
        let end = s.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(s.len());
        (end >= 3).then(|| (s.get(end..).unwrap_or(""), s.get(..end).unwrap_or("")))
    }

    /// A `[+-]hh[:mm[:ss]]` offset in seconds; the rest of `s`.
    fn offset_of(s: &str) -> Option<(&str, i64)> {
        let (sign, body) = match s.chars().next()? {
            '-' => (-1, s.get(1..)?),
            '+' => (1, s.get(1..)?),
            _ => (1, s),
        };
        let end = body.find(|c: char| !(c.is_ascii_digit() || c == ':')).unwrap_or(body.len());
        let mut secs = 0i64;
        for (k, p) in body.get(..end)?.split(':').enumerate().take(3) {
            let v: i64 = p.parse().ok()?;
            secs += v * [3600, 60, 1].get(k).copied().unwrap_or(0);
        }
        Some((body.get(end..).unwrap_or(""), sign * secs))
    }

    /// `Mm.w.d[/time]`.
    fn change(s: &str) -> Option<Change> {
        let (date, time) = match s.split_once('/') {
            Some((d, t)) => (d, offset_of(t).map(|(_, v)| v)?),
            None => (s, 7200),
        };
        let mut p = date.strip_prefix('M')?.split('.');
        let month: u32 = p.next()?.parse().ok().filter(|m| (1..=12).contains(m))?;
        let week: u32 = p.next()?.parse().ok().filter(|w| (1..=5).contains(w))?;
        let weekday: u32 = p.next()?.parse().ok().filter(|d| *d <= 6)?;
        Some(Change { month, week, weekday, secs: time })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        let t = LocalTime::from_secs(1_165_317_339); // 2006-12-05 11:15:39, a Tuesday
        assert_eq!((t.year, t.month, t.day, t.hour, t.minute, t.second, t.weekday), (2006, 12, 5, 11, 15, 39, 2));
        assert_eq!(t.to_secs(), 1_165_317_339);
        assert_eq!(format_date(&t, "M/d/yyyy h:mm:ss tt"), "12/5/2006 11:15:39 AM");
        assert_eq!(format_date(&t, "dddd, MMMM dd, yy 'at' HH"), "Tuesday, December 05, 06 at 11");
        assert_eq!(format_date(&t, "MMM-d"), "Dec-5");
        assert_eq!(LocalTime::from_julian(t.julian()), Some(t));
        assert_eq!(LocalTime::from_julian(f64::NAN), None);
        let leap = LocalTime::from_secs(951_782_400); // 2000-02-29
        assert_eq!((leap.year, leap.month, leap.day), (2000, 2, 29));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn time_zone_rules() {
        let r = tz::Rule::parse("CST6CDT,M3.2.0,M11.1.0").unwrap();
        // 2026-07-01 12:00 UTC: daylight time; 2026-01-01: standard time.
        assert_eq!(r.offset(1_782_907_200), -5 * 3600);
        assert_eq!(r.offset(1_767_268_800), -6 * 3600);
        // The change: 2026-03-08 02:00 CST = 08:00 UTC.
        assert_eq!(r.offset(1_772_956_800 - 1), -6 * 3600);
        assert_eq!(r.offset(1_772_956_800), -5 * 3600);
        assert_eq!(tz::Rule::parse("UTC0").unwrap().offset(0), 0);
        assert_eq!(tz::Rule::parse("<+0530>-5:30").unwrap().offset(0), 5 * 3600 + 1800);
        assert!(tz::Rule::parse("").is_none() && tz::Rule::parse("X1Y,Q").is_none());
        assert!(tz::tzif(b"nope", 0).is_none() && tz::tzif(b"TZif2", 0).is_none());
    }

    #[test]
    fn sheet_names_and_cases() {
        assert_eq!(sheet_parts("A-101 Floor Plan"), ("A-101".into(), "Floor Plan".into()));
        assert_eq!(sheet_parts("E2 - Lighting"), ("E2".into(), "Lighting".into()));
        assert_eq!(sheet_parts("Layout1"), ("Layout1".into(), "Layout1".into()));
        assert_eq!(text_case("dwf6 ePlot.pc3", Some("%tc4")), "Dwf6 EPlot.pc3");
        assert_eq!(text_case("abc", Some("%tc1")), "ABC");
        assert_eq!(text_case("ABC def", Some("%tc2")), "abc def");
        assert_eq!(text_case("abc def", Some("%tc3")), "Abc def");
    }
}
