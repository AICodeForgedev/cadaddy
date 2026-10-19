//! System variables the engine keeps in the drawing header (or in [`crate::Settings`]) and reads
//! with a default, described once in a table: their type, allowed values and default. GETVAR
//! reports the default until a value is set, SETVAR checks the value against the table, and
//! `SETVAR ?` lists them.
//!
//! [`HEADER`] holds the variables commands read; [`STORED`] holds variables scripts commonly set
//! whose behaviour CADaddy doesn't have (yet): they are checked, stored in the drawing and
//! returned, and change nothing else. [`COMPUTED`] are read-only values worked out when asked
//! (TILEMODE can also be set: it switches between the Model tab and a layout).

use serde_json::{Value, json};

use crate::{EngineError, Result, Session, Settings};

/// What a header variable holds and which values it takes.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// An integer from the list.
    Choice(&'static [i64]),
    /// An integer in `min..=max`.
    Int(i64, i64),
    /// A real number of at least `min` (`strict`: greater than `min`).
    Real { min: f64, strict: bool },
    /// Any finite real number.
    AnyReal,
    /// An angle: kept in radians, shown and typed in degrees (a typed text follows AUNITS).
    Angle,
    /// A hatch pattern name from the library.
    Pattern,
    /// A block name; `.` clears it.
    Name,
    /// Text set by its own command, not by SETVAR.
    ReadOnlyStr,
    /// A real number in `min..=max`.
    RealIn(f64, f64),
    /// Any text of up to 255 characters.
    Text,
    /// A point: `[x, y]` or `x,y`.
    Point,
    /// The name of a multiline style of the drawing.
    MlineStyle,
}

/// A default that may depend on the drawing's units (MEASUREMENT 1 = metric).
#[derive(Clone, Copy, Debug)]
enum Def {
    Int(i64),
    Real(f64),
    /// Imperial, metric.
    Units(f64, f64),
    Str(&'static str),
    Point(f64, f64),
}

/// Header variables: name, kind, the default the commands use while the header has none.
const HEADER: &[(&str, Kind, Def)] = &[
    ("CENTEREXE", Kind::Real { min: 0.0, strict: false }, Def::Units(0.12, 3.5)),
    ("CHAMFERA", Kind::Real { min: 0.0, strict: false }, Def::Real(0.0)),
    ("CHAMFERB", Kind::Real { min: 0.0, strict: false }, Def::Real(0.0)),
    ("CHAMFERC", Kind::Real { min: 0.0, strict: false }, Def::Real(0.0)),
    ("CHAMFERD", Kind::Angle, Def::Real(0.0)),
    ("CHAMMODE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("CIRCLERAD", Kind::Real { min: 0.0, strict: false }, Def::Real(0.0)),
    ("CMLJUST", Kind::Choice(&[0, 1, 2]), Def::Int(0)),
    ("CMLSCALE", Kind::AnyReal, Def::Units(1.0, 20.0)),
    ("CPLOTSTYLE", Kind::ReadOnlyStr, Def::Str("ByLayer")),
    ("DIMASSOC", Kind::Choice(&[0, 1, 2]), Def::Int(2)),
    ("DONUTID", Kind::Real { min: 0.0, strict: false }, Def::Real(0.5)),
    ("DONUTOD", Kind::Real { min: 0.0, strict: true }, Def::Real(1.0)),
    ("EDGEMODE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("HPANG", Kind::Angle, Def::Real(0.0)),
    // 1: HATCH and GRADIENT typed or from a menu open the Hatch and Gradient dialog; 0: the command line.
    ("HPDLGMODE", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("HPNAME", Kind::Pattern, Def::Str("ANSI31")),
    ("HPSCALE", Kind::Real { min: 0.0, strict: true }, Def::Real(1.0)),
    ("INSNAME", Kind::Name, Def::Str("")),
    ("LWDEFAULT", Kind::Choice(&LINEWEIGHTS), Def::Int(25)),
    // Negative: Through (OFFSET picks a point).
    ("OFFSETDIST", Kind::AnyReal, Def::Real(-1.0)),
    ("OFFSETERASE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("OFFSETLAYER", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("POLYSIDES", Kind::Int(3, 1024), Def::Int(4)),
    ("PROJMODE", Kind::Choice(&[0, 1, 2]), Def::Int(1)),
    ("REVCLOUDARCLEN", Kind::Real { min: 0.0, strict: true }, Def::Units(0.5, 15.0)),
    ("SPLINESEGS", Kind::Int(-32768, 32767), Def::Int(8)),
    ("SPLKNOTS", Kind::Choice(&[0, 1, 2]), Def::Int(0)),
    ("TEXTFILL", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("TRIMEXTENDMODE", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("TRIMMODE", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("WIPEOUTFRAME", Kind::Choice(&[0, 1, 2]), Def::Int(1)),
    ("XCLIPFRAME", Kind::Choice(&[0, 1, 2]), Def::Int(2)),
    // Image frames: 0 hidden, 1 shown and plotted, 2 shown, not plotted.
    ("IMAGEFRAME", Kind::Choice(&[0, 1, 2]), Def::Int(1)),
    // Image display quality: 0 draft, 1 high (CADaddy always draws at full quality; kept for scripts).
    ("IMAGEQUALITY", Kind::Choice(&[0, 1]), Def::Int(1)),
    // HATCH and the Hatch and Gradient dialog read these.
    ("HPASSOC", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("HPSEPARATE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("HPISLANDDETECTION", Kind::Choice(&[0, 1, 2]), Def::Int(0)),
    ("HPORIGINMODE", Kind::Choice(&[0, 1, 2, 3, 4, 5]), Def::Int(0)),
    ("HPORIGIN", Kind::Point, Def::Point(0.0, 0.0)),
    // MLINE's current style.
    ("CMLSTYLE", Kind::MlineStyle, Def::Str("STANDARD")),
    // The drawing's code page (set when a file is read).
    ("DWGCODEPAGE", Kind::ReadOnlyStr, Def::Str("ANSI_1252")),
    // When fields are worked out: 1 open, 2 save, 4 plot, 8 eTransmit, 16 regen.
    ("FIELDEVAL", Kind::Int(0, 31), Def::Int(31)),
    // 1: fields show on a grey background on screen (never plotted).
    ("FIELDDISPLAY", Kind::Choice(&[0, 1]), Def::Int(1)),
];

/// Variables scripts commonly set that CADaddy keeps without the behaviour behind them (yet):
/// checked like [`HEADER`]'s, stored in the drawing, returned by GETVAR.
const STORED: &[(&str, Kind, Def)] = &[
    ("BLIPMODE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("CETRANSPARENCY", Kind::Text, Def::Str("ByLayer")),
    ("CMDDIA", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("COORDS", Kind::Choice(&[0, 1, 2, 3]), Def::Int(1)),
    ("COPYMODE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("DBLCLKEDIT", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("DELOBJ", Kind::Int(-3, 3), Def::Int(3)),
    ("DRAGMODE", Kind::Choice(&[0, 1, 2]), Def::Int(2)),
    ("DRAWORDERCTL", Kind::Int(0, 3), Def::Int(3)),
    ("EXPLMODE", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("FACETRES", Kind::RealIn(0.01, 10.0), Def::Real(0.5)),
    ("GRIDDISPLAY", Kind::Int(0, 15), Def::Int(2)),
    ("GRIPBLOCK", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("GRIPCOLOR", Kind::Int(1, 255), Def::Int(150)),
    ("GRIPHOT", Kind::Int(1, 255), Def::Int(12)),
    ("GRIPHOVER", Kind::Int(1, 255), Def::Int(11)),
    ("GRIPTIPS", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("HALOGAP", Kind::Int(0, 100), Def::Int(0)),
    ("HIDEPRECISION", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("HPBOUND", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("HPDOUBLE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("HPDRAWORDER", Kind::Int(0, 4), Def::Int(3)),
    ("HPLAYER", Kind::Text, Def::Str(".")),
    ("HPOBJWARNING", Kind::Int(1, 1_073_741_823), Def::Int(10_000)),
    ("HPSPACE", Kind::Real { min: 0.0, strict: true }, Def::Real(1.0)),
    ("HPTRANSPARENCY", Kind::Text, Def::Str("use current")),
    ("ISAVEBAK", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("ISAVEPERCENT", Kind::Int(0, 100), Def::Int(50)),
    ("ISOLINES", Kind::Int(0, 2047), Def::Int(4)),
    ("LENSLENGTH", Kind::Real { min: 0.0, strict: true }, Def::Real(50.0)),
    ("LIMCHECK", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("LWUNITS", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("MAXSORT", Kind::Int(0, 32767), Def::Int(1000)),
    ("MBUTTONPAN", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("MEASUREINIT", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("MENUECHO", Kind::Int(0, 15), Def::Int(0)),
    ("MIRRHATCH", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("MLEADERSCALE", Kind::Real { min: 0.0, strict: false }, Def::Real(1.0)),
    ("MSLTSCALE", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("MTEXTED", Kind::Text, Def::Str("Internal")),
    ("NOMUTT", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("OFFSETGAPTYPE", Kind::Choice(&[0, 1, 2]), Def::Int(0)),
    ("OSNAPCOORD", Kind::Choice(&[0, 1, 2]), Def::Int(2)),
    ("OSNAPZ", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("PAPERUPDATE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("PEDITACCEPT", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("PELLIPSE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("PICKAUTO", Kind::Int(0, 5), Def::Int(5)),
    // CADaddy's selection windows behave as PICKDRAG 2.
    ("PICKDRAG", Kind::Choice(&[0, 1, 2]), Def::Int(2)),
    ("PLINEGEN", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("PLINETYPE", Kind::Choice(&[0, 1, 2]), Def::Int(2)),
    ("PLINEWID", Kind::Real { min: 0.0, strict: false }, Def::Real(0.0)),
    ("PLOTROTMODE", Kind::Choice(&[0, 1, 2]), Def::Int(2)),
    ("PLQUIET", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("PROXYNOTICE", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("PROXYSHOW", Kind::Choice(&[0, 1, 2]), Def::Int(1)),
    ("PSTYLEPOLICY", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("PSVPSCALE", Kind::Real { min: 0.0, strict: false }, Def::Real(0.0)),
    ("QTEXTMODE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("REGENMODE", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("REPORTERROR", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("ROLLOVERTIPS", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("SHORTCUTMENU", Kind::Int(0, 31), Def::Int(11)),
    ("SKETCHINC", Kind::Real { min: 0.0, strict: true }, Def::Real(0.1)),
    ("SKPOLY", Kind::Choice(&[0, 1, 2]), Def::Int(0)),
    ("SORTENTS", Kind::Int(0, 127), Def::Int(127)),
    ("SPLFRAME", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("SPLINETYPE", Kind::Choice(&[5, 6]), Def::Int(6)),
    ("SURFTAB1", Kind::Int(2, 32766), Def::Int(6)),
    ("SURFTAB2", Kind::Int(2, 32766), Def::Int(6)),
    ("TEXTEVAL", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("TEXTQLTY", Kind::Int(0, 100), Def::Int(50)),
    ("TOOLTIPS", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("TRAYICONS", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("TREEDEPTH", Kind::Int(-12000, 12000), Def::Int(3020)),
    ("TSTACKALIGN", Kind::Choice(&[0, 1, 2]), Def::Int(1)),
    ("TSTACKSIZE", Kind::Int(25, 125), Def::Int(70)),
    ("UCSFOLLOW", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("UNITMODE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("USERI1", Kind::Int(-32768, 32767), Def::Int(0)),
    ("USERI2", Kind::Int(-32768, 32767), Def::Int(0)),
    ("USERI3", Kind::Int(-32768, 32767), Def::Int(0)),
    ("USERI4", Kind::Int(-32768, 32767), Def::Int(0)),
    ("USERI5", Kind::Int(-32768, 32767), Def::Int(0)),
    ("USERR1", Kind::AnyReal, Def::Real(0.0)),
    ("USERR2", Kind::AnyReal, Def::Real(0.0)),
    ("USERR3", Kind::AnyReal, Def::Real(0.0)),
    ("USERR4", Kind::AnyReal, Def::Real(0.0)),
    ("USERR5", Kind::AnyReal, Def::Real(0.0)),
    ("USERS1", Kind::Text, Def::Str("")),
    ("USERS2", Kind::Text, Def::Str("")),
    ("USERS3", Kind::Text, Def::Str("")),
    ("USERS4", Kind::Text, Def::Str("")),
    ("USERS5", Kind::Text, Def::Str("")),
    ("VISRETAIN", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("VPROTATEASSOC", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("WHIPARC", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("XEDIT", Kind::Choice(&[0, 1]), Def::Int(1)),
    ("XFADECTL", Kind::Int(0, 90), Def::Int(50)),
    ("XLOADCTL", Kind::Choice(&[0, 1, 2]), Def::Int(2)),
    ("XREFNOTIFY", Kind::Choice(&[0, 1, 2]), Def::Int(2)),
    ("XREFTYPE", Kind::Choice(&[0, 1]), Def::Int(0)),
    ("ZOOMWHEEL", Kind::Choice(&[0, 1]), Def::Int(0)),
];

/// The local date and time as AutoCAD's CDATE (YYYYMMDD.HHMMSSmsec) and DATE (Julian day and
/// fraction); `None` where there is no clock (the web build).
fn now() -> Option<(f64, f64)> {
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
        let ms = i64::try_from(ms).ok()?;
        let (days, rest) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
        // Civil date from days since 1970-01-01 (proleptic Gregorian, UTC).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        let (h, m, sec, msec) = (rest / 3_600_000, rest / 60_000 % 60, rest / 1000 % 60, rest % 1000);
        let cdate = (year * 10_000 + month * 100 + day) as f64 + (h * 10_000_000 + m * 100_000 + sec * 1000 + msec) as f64 / 1e9;
        let date = 2_440_587.5 + ms as f64 / 86_400_000.0;
        Some((cdate, date))
    }
}

/// Read-only values worked out when asked: name, value. TILEMODE can be set too ([`set`]).
type Computed = (&'static str, fn(&Session) -> Value);
const COMPUTED: &[Computed] = &[
    ("TILEMODE", |s| json!(i32::from(s.layout_space() == cadaddy_doc::Space::Model))),
    // 2 for model space; in a layout the active viewport's id, 1 for the sheet itself.
    ("CVPORT", |s| match s.state().ok().and_then(|st| st.active_viewport()) {
        Some((_, v)) => json!(v.id),
        None if s.layout_space() == cadaddy_doc::Space::Model => json!(2),
        None => json!(1),
    }),
    ("CMDACTIVE", |s| json!(i32::from(s.running.is_some()))),
    ("AREA", |s| json!(s.last_used.area)),
    ("PERIMETER", |s| json!(s.last_used.perimeter)),
    ("DISTANCE", |s| json!(s.last_used.distance)),
    ("CDATE", |_| json!(now().map_or(0.0, |n| n.0))),
    ("DATE", |_| json!(now().map_or(0.0, |n| n.1))),
    ("LASTANGLE", |_| json!(0.0)),
    ("ERRNO", |_| json!(0)),
    ("DIASTAT", |_| json!(1)),
    ("PLATFORM", |_| json!(std::env::consts::OS)),
    ("PRODUCT", |_| json!("CADaddy")),
    ("PROGRAM", |_| json!("cadaddy")),
    ("LOCALE", |_| json!("ENU")),
    ("SYSCODEPAGE", |_| json!("ANSI_1252")),
    // The current UCS, from the drawing's header (UCS, PLAN, a restored named view). These
    // said "world" always, from before CADaddy had a UCS (UI review 0.7.7c).
    ("WORLDUCS", |s| json!(i32::from(s.doc().map_or(true, |d| ucs_now(d).is_world())))),
    ("UCSNAME", |s| json!(s.doc().map(|d| ucs_now(d).name).unwrap_or_default())),
    ("UCSORG", |s| {
        let o = s.doc().map(|d| ucs_now(d).origin).unwrap_or(cadaddy_geom::Vec3::ZERO);
        json!([o.x, o.y, o.z])
    }),
    ("UCSXDIR", |s| {
        let x = s.doc().map(|d| ucs_now(d).x_axis).unwrap_or(cadaddy_geom::Vec3::new(1.0, 0.0, 0.0));
        json!([x.x, x.y, x.z])
    }),
    ("UCSYDIR", |s| {
        let y = s.doc().map(|d| ucs_now(d).y_axis).unwrap_or(cadaddy_geom::Vec3::new(0.0, 1.0, 0.0));
        json!([y.x, y.y, y.z])
    }),
    ("TARGET", |_| json!([0.0, 0.0, 0.0])),
    ("VIEWDIR", |s| {
        let d = s.state().map(|st| st.view_dir).unwrap_or(cadaddy_geom::Vec3::Z);
        json!([d.x, d.y, d.z])
    }),
];

/// The drawing's current UCS as its header holds it.
fn ucs_now(d: &cadaddy_doc::Drawing) -> cadaddy_doc::ViewUcs {
    cadaddy_doc::ViewUcs::current(&d.header)
}

fn computed(name: &str) -> Option<&'static Computed> {
    COMPUTED.iter().find(|(n, _)| *n == name)
}

/// The standard lineweights in hundredths of a millimetre.
const LINEWEIGHTS: [i64; 24] = [0, 5, 9, 13, 15, 18, 20, 25, 30, 35, 40, 50, 53, 60, 70, 80, 90, 100, 106, 120, 140, 158, 200, 211];

/// On/off session settings: name, read, write.
type Toggle = (&'static str, fn(&Settings) -> bool, fn(&mut Settings, bool));
const SETTINGS: &[Toggle] = &[("TRANSPARENCYDISPLAY", |s| s.transparency_display, |s, v| s.transparency_display = v)];

fn header_var(name: &str) -> Option<&'static (&'static str, Kind, Def)> {
    HEADER.iter().chain(STORED).find(|(n, _, _)| *n == name)
}

fn toggle(name: &str) -> Option<&'static Toggle> {
    SETTINGS.iter().find(|(n, _, _)| *n == name)
}

/// Whether `name` (upper case) is a table variable SETVAR can't change.
pub(super) fn is_read_only(name: &str) -> bool {
    header_var(name).is_some_and(|(_, k, _)| matches!(k, Kind::ReadOnlyStr)) || (computed(name).is_some() && name != "TILEMODE")
}

/// The value of a table variable (upper-case `name`), or None when it isn't one.
pub(super) fn get(s: &Session, name: &str) -> Option<Value> {
    if let Some((_, read, _)) = toggle(name) {
        return Some(json!(i32::from(read(&s.settings))));
    }
    if let Some((_, value)) = computed(name) {
        return Some(value(s));
    }
    let (_, kind, def) = header_var(name)?;
    let d = s.doc().ok()?;
    let h = &d.header;
    let real_def = match *def {
        Def::Real(f) => f,
        Def::Units(imperial, metric) => {
            if h.i64("MEASUREMENT", 0) == 1 {
                metric
            } else {
                imperial
            }
        }
        Def::Int(i) => i as f64,
        Def::Str(_) | Def::Point(..) => 0.0,
    };
    Some(match (kind, def) {
        (_, Def::Point(x, y)) => {
            let p = h.point(name).map_or(cadaddy_geom::Vec2::new(*x, *y), |p| p.xy());
            json!([p.x, p.y, 0.0])
        }
        (Kind::Choice(_) | Kind::Int(..), Def::Int(i)) => json!(h.i64(name, *i)),
        (Kind::Angle, _) => json!(h.f64(name, real_def).to_degrees()),
        (_, Def::Str(t)) => json!(h.str(name, t)),
        _ => json!(h.f64(name, real_def)),
    })
}

fn int_value(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 1e15).map(|f| f as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::String(t) => t.trim().parse().ok(),
        _ => None,
    }
}

/// Set a table variable (upper-case `name`); None when it isn't one.
pub(super) fn set(s: &mut Session, name: &str, v: &Value) -> Option<Result<()>> {
    let bad = |allowed: String| EngineError::BadParams { cmd: "setvar".into(), msg: format!("invalid value for {name} ({allowed})") };
    if let Some((_, _, write)) = toggle(name) {
        let r = match int_value(v) {
            Some(i @ (0 | 1)) => {
                write(&mut s.settings, i == 1);
                Ok(())
            }
            _ => Err(bad("0 or 1".into())),
        };
        return Some(r);
    }
    if name == "TILEMODE" {
        // 1: the Model tab; 0: the first layout (as switching tabs does).
        let r = match int_value(v) {
            Some(1) => s.execute("layout.set", &json!({ "name": "Model" })).map(|_| ()),
            Some(0) => match s.doc().ok().and_then(|d| d.layouts.iter().min_by_key(|l| l.tab_order).map(|l| l.name.clone())) {
                Some(l) => s.execute("layout.set", &json!({ "name": l })).map(|_| ()),
                None => Err(bad("the drawing has no layout".into())),
            },
            _ => Err(bad("0 or 1".into())),
        };
        return Some(r);
    }
    if computed(name).is_some() {
        return Some(Err(EngineError::BadParams { cmd: "setvar".into(), msg: format!("{name} is read-only") }));
    }
    let (_, kind, _) = header_var(name)?;
    Some(set_header(s, name, *kind, v, bad))
}

fn set_header(s: &mut Session, name: &str, kind: Kind, v: &Value, bad: impl Fn(String) -> EngineError) -> Result<()> {
    let real = || super::as_f64(v);
    match kind {
        Kind::Choice(allowed) => {
            let i = int_value(v).filter(|i| allowed.contains(i));
            let list = || allowed.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
            let i = i.ok_or_else(|| bad(list()))?;
            s.doc_mut()?.header.set_i64(name, i);
        }
        Kind::Int(min, max) => {
            let i = int_value(v).filter(|i| (min..=max).contains(i)).ok_or_else(|| bad(format!("{min} to {max}")))?;
            s.doc_mut()?.header.set_i64(name, i);
        }
        Kind::Real { min, strict } => {
            let f = real().filter(|f| if strict { *f > min } else { *f >= min });
            let f = f.ok_or_else(|| bad(format!("a number {} {min}", if strict { "above" } else { "of at least" })))?;
            s.doc_mut()?.header.set_f64(name, f);
        }
        Kind::AnyReal => {
            let f = real().ok_or_else(|| bad("a number".into()))?;
            s.doc_mut()?.header.set_f64(name, f);
        }
        Kind::Angle => {
            let a = match v {
                Value::String(t) => s.angle_settings().amount(t),
                _ => v.as_f64().map(f64::to_radians),
            };
            let a = a.filter(|a| a.is_finite()).ok_or_else(|| bad("an angle".into()))?;
            s.doc_mut()?.header.set_f64(name, cadaddy_geom::norm_angle(a));
        }
        Kind::Pattern => {
            let p = v.as_str().map(|t| t.trim().to_ascii_uppercase()).filter(|p| cadaddy_doc::library::pattern(p).is_some());
            let p = p.ok_or_else(|| bad("a hatch pattern name".into()))?;
            s.doc_mut()?.header.set_str(name, &p);
        }
        Kind::Name => {
            let t = v.as_str().map(str::trim).filter(|t| t.chars().count() <= 255).ok_or_else(|| bad("a name, or . for none".into()))?;
            let t = if t == "." { "" } else { t };
            s.doc_mut()?.header.set_str(name, t);
        }
        Kind::ReadOnlyStr => return Err(EngineError::BadParams { cmd: "setvar".into(), msg: format!("{name} is read-only") }),
        Kind::RealIn(min, max) => {
            let f = real().filter(|f| (min..=max).contains(f)).ok_or_else(|| bad(format!("{min} to {max}")))?;
            s.doc_mut()?.header.set_f64(name, f);
        }
        Kind::Text => {
            let t = match v {
                Value::String(t) => t.clone(),
                Value::Number(n) => n.to_string(),
                _ => return Err(bad("text".into())),
            };
            if t.chars().count() > 255 {
                return Err(bad("at most 255 characters".into()));
            }
            s.doc_mut()?.header.set_str(name, &t);
        }
        Kind::Point => {
            let p = crate::cmd::point_value(v).filter(|p| p.is_finite()).ok_or_else(|| bad("a point x,y".into()))?;
            s.doc_mut()?.header.set(name, cadaddy_doc::HVal::Point(p.to3(0.0)));
        }
        Kind::MlineStyle => {
            let t = v.as_str().map(str::trim).unwrap_or_default();
            let d = s.doc()?;
            let found = d.mline_styles.iter().find(|m| m.name.eq_ignore_ascii_case(t)).map(|m| m.name.clone());
            let found =
                found.or_else(|| t.eq_ignore_ascii_case(cadaddy_doc::STANDARD_MLINE_STYLE).then(|| cadaddy_doc::STANDARD_MLINE_STYLE.to_string()));
            let name_found = found.ok_or_else(|| bad("the name of a multiline style of the drawing".into()))?;
            s.doc_mut()?.header.set_str(name, &name_found);
        }
    }
    Ok(())
}

/// Add the table variables and the dimension variables of the current style to a `SETVAR ?` list.
pub(super) fn list(s: &Session, m: &mut serde_json::Map<String, Value>) {
    let names = SETTINGS.iter().map(|(n, _, _)| *n).chain(HEADER.iter().chain(STORED).map(|(n, _, _)| *n)).chain(COMPUTED.iter().map(|(n, _)| *n));
    for n in names.chain(cadaddy_doc::DIMVARS.iter().map(|(n, _)| *n)) {
        if !m.contains_key(n)
            && let Some(v) = super::get(s, n)
        {
            m.insert(n.to_string(), v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_names_are_upper_case_and_unique() {
        let mut names: Vec<&str> = HEADER
            .iter()
            .chain(STORED)
            .map(|(n, _, _)| *n)
            .chain(SETTINGS.iter().map(|(n, _, _)| *n))
            .chain(COMPUTED.iter().map(|(n, _)| *n))
            .collect();
        assert!(names.iter().all(|n| n.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())));
        let len = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), len);
    }
}
