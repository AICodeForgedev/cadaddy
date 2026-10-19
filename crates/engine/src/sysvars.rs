//! System variables: session settings plus drawing header variables, by name.

use serde_json::{Value, json};

use crate::{EngineError, Result, Session};

mod table;

const SESSION_VARS: &[&str] = &[
    "OSMODE",
    "3DOSMODE",
    "UCSDETECT",
    "OSNAPHATCH",
    "ANNOAUTOSCALE",
    "ORTHOMODE",
    "POLARMODE",
    "POLARANG",
    "POLARADDANG",
    "POLARDIST",
    "SNAPTYPE",
    "AUTOSNAP",
    "QNEWTEMPLATE",
    "GRIDMODE",
    "SNAPMODE",
    "SNAPSTYL",
    "SNAPISOPAIR",
    "SNAPUNIT",
    "GRIDUNIT",
    "UNDOCTL",
    "UNDOMARKS",
    "GRIDMAJOR",
    "DYNDIGRIP",
    "DYNDIVIS",
    "DYNMODE",
    "DYNPIFORMAT",
    "DYNPICOORDS",
    "DYNPROMPT",
    "LWDISPLAY",
    "PLOTTRANSPARENCYOVERRIDE",
    "SELECTSIMILARMODE",
    "OBJECTISOLATIONMODE",
    "QPMODE",
    "HIGHLIGHT",
    "SELECTIONEFFECT",
    "SELECTIONPREVIEW",
    "PREVIEWEFFECT",
    "PREVIEWFILTER",
    "GRIPS",
    "GRIPOBJLIMIT",
    "SELECTIONAREA",
    "WINDOWAREACOLOR",
    "CROSSINGAREACOLOR",
    "SELECTIONAREAOPACITY",
    "PICKBOX",
    "APERTURE",
    "PICKFIRST",
    "ACADLSPASDOC",
    "PICKADD",
    "FILEDIA",
    "CMDECHO",
    "EXPERT",
    "ATTDIA",
    "SELECTIONCYCLING",
    "NAVBARDISPLAY",
    "WSCURRENT",
    "PICKSTYLE",
    "GROUPDISPLAYMODE",
    "GRIPSIZE",
    "CURSORSIZE",
    "MAXARRAY",
    "ZOOMFACTOR",
    "LASTPOINT",
    "FONTALT",
    "FONTFALLBACK",
    "SAVETIME",
    "SAVEFILEPATH",
    "AUDITCTL",
    "DWGCOPY",
    "DCTMAIN",
    "DCTCUST",
];

pub fn get(s: &Session, name: &str) -> Option<Value> {
    let n = name.trim().to_ascii_uppercase();
    let st = &s.settings;
    let v = match n.as_str() {
        "OSMODE" => json!(st.osmode),
        "3DOSMODE" => json!(st.osmode3),
        "UCSDETECT" => json!(i32::from(st.ducs)),
        "OSNAPHATCH" => json!(i32::from(st.osnaphatch)),
        // AutoCAD's values: positive on, negative off (4 = every annotative object).
        "ANNOAUTOSCALE" => json!(if st.annoautoscale { 4 } else { -4 }),
        // Paper units per drawing unit of the current annotation scale (1:50 → 0.02).
        "CANNOSCALEVALUE" => json!(1.0 / s.doc().ok()?.current_anno_scale().factor()),
        "ORTHOMODE" => json!(i32::from(st.orthomode)),
        "POLARMODE" => json!(st.polar_flags),
        "POLARADDANG" => json!(format_angle_list(&st.polaraddang)),
        "POLARDIST" => json!(st.polardist),
        "SNAPTYPE" => json!(i32::from(st.polarsnap)),
        // Marker, tooltip and tracking tooltip always on (1 + 4 + 32); polar 8, tracking 16.
        "AUTOSNAP" => json!(37 + 8 * u32::from(st.polarmode) + 16 * u32::from(st.otrack)),
        "POLARANG" => json!(st.polarang.to_degrees()),
        "GRIDMODE" => json!(i32::from(st.gridmode)),
        "SNAPMODE" => json!(i32::from(st.snapmode)),
        "SNAPSTYL" => json!(i32::from(st.isodraft)),
        "SNAPISOPAIR" => json!(st.isoplane.index()),
        "SNAPUNIT" | "GRIDUNIT" => {
            let settings = s.drafting_settings();
            let unit = if n == "SNAPUNIT" { settings.snapunit } else { settings.gridunit };
            json!([unit.x, unit.y])
        }
        "GRIDMAJOR" => json!(st.gridmajor),
        "DYNDIGRIP" => json!(st.dyndigrip),
        "DYNDIVIS" => json!(st.dyndivis),
        "DYNMODE" => json!(i32::from(st.dynmode)),
        "DYNPIFORMAT" => json!(i32::from(st.dynpi_cartesian)),
        "DYNPICOORDS" => json!(i32::from(st.dynpi_absolute)),
        "DYNPROMPT" => json!(i32::from(st.dynprompt)),
        "LWDISPLAY" => json!(i32::from(st.lwdisplay)),
        "PLOTTRANSPARENCYOVERRIDE" => json!(st.plot_transparency_override),
        "SELECTSIMILARMODE" => json!(st.selectsimilarmode),
        // Isolation is temporary: hidden objects show again when the drawing is reopened.
        "OBJECTISOLATIONMODE" => json!(st.objectisolationmode),
        "QPMODE" => json!(i32::from(st.qpmode)),
        "HIGHLIGHT" => json!(i32::from(st.highlight)),
        "SELECTIONEFFECT" => json!(st.selectioneffect),
        "SELECTIONPREVIEW" => json!(st.selectionpreview),
        "PREVIEWEFFECT" => json!(st.previeweffect),
        "PREVIEWFILTER" => json!(st.previewfilter),
        "GRIPS" => json!(st.grips),
        "GRIPOBJLIMIT" => json!(st.gripobjlimit),
        "SELECTIONAREA" => json!(i32::from(st.selectionarea)),
        "WINDOWAREACOLOR" => json!(st.windowareacolor),
        "CROSSINGAREACOLOR" => json!(st.crossingareacolor),
        "SELECTIONAREAOPACITY" => json!(st.selectionareaopacity),
        "PICKBOX" => json!(st.pickbox),
        "APERTURE" => json!(st.aperture),
        "PICKFIRST" => json!(i32::from(st.pickfirst)),
        "ACADLSPASDOC" => json!(i32::from(st.acadlspasdoc)),
        "PICKADD" => json!(i32::from(st.pickadd)),
        "FILEDIA" => json!(st.filedia),
        "CMDECHO" => json!(st.cmdecho),
        "EXPERT" => json!(st.expert),
        "ATTDIA" => json!(st.attdia),
        "SELECTIONCYCLING" => json!(st.selectioncycling),
        "NAVBARDISPLAY" => json!(i32::from(st.navbardisplay)),
        "WSCURRENT" => json!(st.wscurrent),
        "PICKSTYLE" => json!(st.pickstyle),
        "GROUPDISPLAYMODE" => json!(st.groupdisplaymode),
        "GRIPSIZE" => json!(st.gripsize),
        "CURSORSIZE" => json!(st.cursorsize),
        "MAXARRAY" => json!(st.maxarray),
        "ZOOMFACTOR" => json!(st.zoomfactor),
        "LASTPOINT" => json!([s.last_point.x, s.last_point.y, 0.0]),
        // Held in radians (as `$ANGBASE` in DXF); reported in degrees, like POLARANG.
        "ANGBASE" => json!(s.doc().ok()?.header.f64("ANGBASE", 0.0).to_degrees()),
        "CONSTRAINTINFER" => json!(i32::from(s.doc().ok()?.parametric.settings.infer)),
        "CONSTRAINTNAMEFORMAT" => json!(s.doc().ok()?.parametric.settings.name_format),
        // Process-wide font substitution (profile settings, not saved in the drawing).
        "FONTALT" => json!(cadaddy_fonts::ttf::font_alt()),
        "FONTFALLBACK" => json!(cadaddy_fonts::ttf::fallback_fonts()),
        // Autosave (profile settings too).
        "SAVETIME" => json!(crate::autosave::savetime()),
        "SAVEFILEPATH" => json!(crate::autosave::savefilepath()),
        "AUDITCTL" => json!(i32::from(crate::audit::auditctl())),
        // Saving a .cdy file also writes a DWG copy (a profile setting).
        "DWGCOPY" => json!(i32::from(crate::cmd::file::dwg_copy())),
        // Spelling dictionaries: the main one (only American English ships) and the user's file.
        "DCTMAIN" => json!(crate::cmd::spell::MAIN_CODE),
        "DCTCUST" => json!(crate::cmd::spell::dctcust()),
        "SUNSTATUS" => json!(i32::from(s.doc().ok()?.sun.on)),
        "RENDEREXPOSURE" => json!(s.doc().ok()?.sun.exposure),
        "CMDNAMES" => json!(s.running.as_ref().map(|r| r.id.to_ascii_uppercase()).unwrap_or_default()),
        // A drawing not saved yet is named as AutoCAD names it, "Drawing1.dwg".
        "DWGNAME" => json!(
            s.state()
                .map(|d| if d.path.is_none() && !d.title.contains('.') { format!("{}.dwg", d.title) } else { d.title.clone() })
                .unwrap_or_default()
        ),
        // The drawing's folder with its trailing separator; Documents before it is saved.
        "DWGPREFIX" => json!(s.state().ok().map(|d| dwg_prefix(d.path.as_deref())).unwrap_or_default()),
        "DWGTITLED" => json!(i32::from(s.state().is_ok_and(|d| d.path.is_some()))),
        "DBMOD" => json!(s.state().map(|d| i32::from(d.is_dirty())).unwrap_or(0)),
        "BLOCKEDITOR" => json!(i32::from(s.state().is_ok_and(|d| d.block_edit.is_some()))),
        // The template NEW/QNEW start from (a profile setting, like FONTALT; "" = none).
        "QNEWTEMPLATE" => json!(crate::cmd::file::qnew_template()),
        "CTAB" => json!(s.state().map(|d| d.space.tab_name().to_string()).unwrap_or_default()),
        // UNDO state of the drawing: Control/Auto/group bits and the number of marks.
        "UNDOCTL" => json!(s.state().ok()?.undo_ctl.bits()),
        "UNDOMARKS" => json!(s.state().ok()?.undo_marks()),
        "VIEWCTR" => {
            let v = s.state().ok()?.view();
            json!([v.center.x, v.center.y, 0.0])
        }
        "VIEWSIZE" => json!(s.state().ok()?.view().height),
        "EXTMIN" | "EXTMAX" => {
            let d = s.doc().ok()?;
            let e = d.extents(&s.space());
            let p = if n == "EXTMIN" { e.min } else { e.max };
            json!([p.x, p.y, 0.0])
        }
        _ => {
            if let Some(v) = table::get(s, &n) {
                return Some(v);
            }
            let d = s.doc().ok()?;
            match d.header.get(&n) {
                Some(h) => serde_json::to_value(h).ok()?,
                // A dimension variable the header doesn't carry reads from the current style.
                // Variables CADaddy reads with a default when the drawing doesn't set them.
                None if let Some(v) = header_default(&n) => v,
                None => {
                    let field = cadaddy_doc::DIMVARS.iter().find(|(v, _)| *v == n)?.1;
                    let st = d.dim_style(&d.header.str("DIMSTYLE", "Standard"))?;
                    serde_json::to_value(st).ok()?.get(field)?.clone()
                }
            }
        }
    };
    Some(v)
}

/// Drawing variables that are read with a default when the header doesn't carry them, so they
/// can be typed by name and read with GETVAR before anything sets them.
/// Header variables CADaddy reads (so SETVAR may set them) though no table above describes them:
/// every `header.*("NAME")` read in the code.
const READ_HEADER_VARS: &[&str] = &[
    "ACADVER",
    "ANGBASE",
    "ANGDIR",
    "ANNOALLVISIBLE",
    "ARRAYASSOCIATIVITY",
    "ATTMODE",
    "ATTREQ",
    "AUNITS",
    "AUPREC",
    "CANNOSCALE",
    "CECOLOR",
    "CECOLOR_RGB",
    "CELTYPE",
    "CELWEIGHT",
    "CENTEREXE",
    "CHAMFERA",
    "CHAMFERB",
    "CHAMFERC",
    "CHAMFERD",
    "CHAMMODE",
    "CIRCLERAD",
    "CLAYER",
    "CMLEADERSTYLE",
    "CMLJUST",
    "CMLSCALE",
    "CMLSTYLE",
    "CPLOTSTYLE",
    "CTABLESTYLE",
    "DIMASSOC",
    "DIMCLRD",
    "DIMDSEP",
    "DIMJOGANG",
    "DIMSCALE",
    "DIMSTYLE",
    "DIMTXT",
    "DONUTID",
    "DONUTOD",
    "DWGCODEPAGE",
    "EDGEMODE",
    "FILLETRAD",
    "FILLMODE",
    "FONTALT",
    "GRIDUNIT",
    "HANDSEED",
    "HPANG",
    "HPANNOTATIVE",
    "HPASSOC",
    "HPDLGMODE",
    "HPGAPTOL",
    "HPISLANDDETECTION",
    "HPNAME",
    "HPORIGIN",
    "HPORIGINMODE",
    "HPSCALE",
    "HPSEPARATE",
    "IMAGEFRAME",
    "INSBASE",
    "INSNAME",
    "INSUNITS",
    "LIMMAX",
    "LIMMIN",
    "LTSCALE",
    "LUNITS",
    "LUPREC",
    "LWDEFAULT",
    "MEASUREMENT",
    "OFFSETDIST",
    "OFFSETERASE",
    "OFFSETLAYER",
    "PDFOSNAP",
    "PDMODE",
    "PDSIZE",
    "POLYSIDES",
    "PROJMODE",
    "PSLTSCALE",
    "REVCLOUDARCLEN",
    "SNAPANG",
    "SNAPBASE",
    "SNAPSTYL",
    "SNAPUNIT",
    "SPLINESEGS",
    "SPLKNOTS",
    "STEPSIZE",
    "STEPSPERSEC",
    "TEXTFILL",
    "TEXTSIZE",
    "TEXTSTYLE",
    "TRIMEXTENDMODE",
    "TRIMMODE",
    "WIPEOUTFRAME",
    "XCLIPFRAME",
];

/// ATTREQ is an integer (AutoCAD returns 1, not 1.0); USERI1–5, USERR1–5 and USERS1–5 are the
/// variables LISP routines keep their own values in.
fn header_default(n: &str) -> Option<Value> {
    match n {
        "HPGAPTOL" => Some(json!(0.0)),
        // 3DWALK / 3DFLY: drawing units a step takes, and steps a second.
        "STEPSIZE" => Some(json!(6.0)),
        "STEPSPERSEC" => Some(json!(2.0)),
        "ATTREQ" => Some(json!(1)),
        "USERI1" | "USERI2" | "USERI3" | "USERI4" | "USERI5" => Some(json!(0)),
        "USERR1" | "USERR2" | "USERR3" | "USERR4" | "USERR5" => Some(json!(0.0)),
        "USERS1" | "USERS2" | "USERS3" | "USERS4" | "USERS5" => Some(json!("")),
        // The base relative hyperlinks resolve against ("": the drawing's folder).
        "HYPERLINKBASE" => Some(json!("")),
        // The material new objects get (MATERIALASSIGN's default).
        "CMATERIAL" => Some(json!("ByLayer")),
        // Rendered views' sun on/off and exposure, kept with the drawing's sun.
        "SUNSTATUS" => Some(json!(1)),
        // Layer notification: evaluate new xref layers; report them on plot, open, xref load and
        // layer state restore.
        "LAYEREVAL" => Some(json!(1)),
        "LAYERNOTIFY" => Some(json!(15)),
        "RENDEREXPOSURE" => Some(json!(0.0)),
        _ => None,
    }
}

fn as_bool(v: &Value) -> Option<bool> {
    v.as_bool().or_else(|| v.as_i64().map(|i| i != 0)).or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()).map(|i| i != 0))
}
/// QPMODE is -1/0 (off), 1 or 2 (on) in AutoCAD; booleans work too.
fn as_i(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_bool().map(i64::from)).or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}
/// A whole number from 0 to `max` (a mode or a small bitcode).
fn small(v: &Value, max: u8) -> Option<u8> {
    as_i(v).filter(|n| (0..=i64::from(max)).contains(n)).and_then(|n| u8::try_from(n).ok())
}
fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(crate::units::parse_distance)).filter(|f| f.is_finite())
}
/// DWGPREFIX: everything up to the last path separator of `path`, or the current folder.
pub(crate) fn dwg_prefix(path: Option<&str>) -> String {
    if let Some(p) = path
        && let Some(i) = p.rfind(['/', '\\'])
    {
        return p.get(..=i).unwrap_or_default().to_string();
    }
    // Unsaved: the user's Documents folder, as AutoCAD (never the program's working folder,
    // which is a system folder when started from the Start menu).
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(std::path::PathBuf::from);
    let docs = home.map(|h| h.join("Documents")).filter(|d| d.is_dir());
    let mut dir = docs.or_else(|| std::env::current_dir().ok()).map(|d| d.display().to_string()).unwrap_or_default();
    if !dir.is_empty() && !dir.ends_with(['/', '\\']) {
        dir.push(std::path::MAIN_SEPARATOR);
    }
    dir
}

fn as_pt(v: &Value) -> Option<cadaddy_geom::Vec2> {
    crate::cmd::point_value(v).or_else(|| as_f64(v).map(|f| cadaddy_geom::Vec2::new(f, f)))
}

/// Variables `get` reports from session or drawing state that `set` can't change.
const READ_ONLY: &[&str] = &[
    "CMDNAMES",
    "DWGNAME",
    "DWGPREFIX",
    "DWGTITLED",
    "DBMOD",
    "BLOCKEDITOR",
    "CTAB",
    "LASTPOINT",
    "VIEWCTR",
    "VIEWSIZE",
    "EXTMIN",
    "EXTMAX",
    "UNDOCTL",
    "UNDOMARKS",
];

pub fn is_read_only(name: &str) -> bool {
    READ_ONLY.iter().any(|r| r.eq_ignore_ascii_case(name.trim())) || table::is_read_only(&name.trim().to_ascii_uppercase())
}

/// POLARADDANG as typed: up to ten angles in degrees separated by `;` (radians out); `None` when
/// any is not a number or there are too many.
pub fn parse_angle_list(text: &str) -> Option<Vec<f64>> {
    let angles: Option<Vec<f64>> =
        text.split(';').map(str::trim).filter(|t| !t.is_empty()).map(|t| t.parse::<f64>().ok().filter(|a| a.is_finite())).collect();
    Some(angles.filter(|a| a.len() <= 10)?.into_iter().map(f64::to_radians).collect())
}

/// POLARADDANG as shown: degrees separated by `;`.
pub fn format_angle_list(angles: &[f64]) -> String {
    angles.iter().map(|a| format!("{}", (a.to_degrees() * 1e6).round() / 1e6)).collect::<Vec<_>>().join(";")
}

pub fn set(s: &mut Session, name: &str, v: &Value) -> Result<()> {
    let n = name.trim().to_ascii_uppercase();
    if is_read_only(&n) {
        return Err(EngineError::BadParams { cmd: "setvar".into(), msg: format!("{n} is read-only") });
    }
    let bad = || EngineError::BadParams { cmd: "setvar".into(), msg: format!("invalid value for {n}") };
    let st = &mut s.settings;
    match n.as_str() {
        "OSMODE" => st.osmode = v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).ok_or_else(bad)? as u32 & 0x7fff,
        "3DOSMODE" => {
            st.osmode3 = v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).ok_or_else(bad)? as u32 & crate::snap3::mode::MASK
        }
        "UCSDETECT" => st.ducs = as_bool(v).ok_or_else(bad)?,
        "OSNAPHATCH" => st.osnaphatch = as_bool(v).ok_or_else(bad)?,
        "ANNOAUTOSCALE" => st.annoautoscale = as_i(v).filter(|i| (-4..=4).contains(i)).ok_or_else(bad)? > 0,
        // A scale of the drawing's list; ANNOAUTOSCALE adds it to the annotative objects.
        "CANNOSCALE" => crate::cmd::annoscale::set_current(s, v.as_str().ok_or_else(bad)?)?,
        "CANNOSCALEVALUE" => return Err(EngineError::BadParams { cmd: "setvar".into(), msg: format!("{n} is read-only") }),
        "ORTHOMODE" => {
            st.orthomode = as_bool(v).ok_or_else(bad)?;
            // Ortho and polar tracking exclude each other.
            st.polarmode &= !st.orthomode;
        }
        "POLARMODE" => st.polar_flags = as_i(v).filter(|b| (0..=15).contains(b)).ok_or_else(bad)? as u32,
        "POLARADDANG" => {
            let text = v.as_str().map(str::to_string).or_else(|| as_f64(v).map(|f| f.to_string())).ok_or_else(bad)?;
            st.polaraddang = parse_angle_list(&text).ok_or_else(bad)?;
        }
        "POLARDIST" => st.polardist = as_f64(v).filter(|d| *d >= 0.0 && *d < 1e12).ok_or_else(bad)?,
        "SNAPTYPE" => st.polarsnap = as_i(v).filter(|b| (0..=1).contains(b)).ok_or_else(bad)? == 1,
        "AUTOSNAP" => {
            let bits = as_i(v).filter(|b| (0..=63).contains(b)).ok_or_else(bad)?;
            st.polarmode = bits & 8 != 0;
            st.otrack = bits & 16 != 0;
            st.orthomode &= !st.polarmode;
        }
        "QNEWTEMPLATE" => crate::cmd::file::set_qnew_template(v.as_str().ok_or_else(bad)?),
        "POLARANG" => st.polarang = as_f64(v).filter(|a| *a > 0.0).ok_or_else(bad)?.to_radians(),
        "GRIDMODE" => st.gridmode = as_bool(v).ok_or_else(bad)?,
        "SNAPMODE" => st.snapmode = as_bool(v).ok_or_else(bad)?,
        "SNAPSTYL" => st.isodraft = as_i(v).filter(|b| (0..=1).contains(b)).ok_or_else(bad)? == 1,
        "SNAPISOPAIR" => st.isoplane = as_i(v).and_then(crate::snap::iso::IsoPlane::from_index).ok_or_else(bad)?,
        "SNAPUNIT" | "GRIDUNIT" => {
            let unit = as_pt(v)
                .filter(|p| p.is_finite() && if n == "GRIDUNIT" { p.x >= 0.0 && p.y >= 0.0 } else { p.x > 0.0 && p.y > 0.0 })
                .ok_or_else(bad)?;
            s.doc_mut()?.header.set(&n, cadaddy_doc::HVal::Point(unit.to3(0.0)));
        }
        "GRIDMAJOR" => st.gridmajor = as_f64(v).ok_or_else(bad)?.clamp(1.0, 100.0) as u32,
        "DYNDIGRIP" => st.dyndigrip = as_i(v).filter(|i| (0..=31).contains(i)).and_then(|i| u32::try_from(i).ok()).ok_or_else(bad)?,
        "DYNDIVIS" => st.dyndivis = as_i(v).filter(|i| (0..=2).contains(i)).and_then(|i| u8::try_from(i).ok()).ok_or_else(bad)?,
        "DYNMODE" => st.dynmode = as_bool(v).ok_or_else(bad)?,
        "DYNPIFORMAT" => st.dynpi_cartesian = as_bool(v).ok_or_else(bad)?,
        "DYNPICOORDS" => st.dynpi_absolute = as_bool(v).ok_or_else(bad)?,
        "DYNPROMPT" => st.dynprompt = as_bool(v).ok_or_else(bad)?,
        "LWDISPLAY" => st.lwdisplay = as_bool(v).ok_or_else(bad)?,
        "PLOTTRANSPARENCYOVERRIDE" => {
            st.plot_transparency_override = as_i(v).filter(|i| (0..=2).contains(i)).and_then(|i| u8::try_from(i).ok()).ok_or_else(bad)?
        }
        "SELECTSIMILARMODE" => st.selectsimilarmode = as_i(v).filter(|b| (0..=255).contains(b)).ok_or_else(bad)? as u32,
        "OBJECTISOLATIONMODE" => {
            st.objectisolationmode = small(v, 1).ok_or_else(bad)?;
            // The open drawings save their hidden objects from now on (or stop saving them).
            let keep = st.objectisolationmode == 1;
            for d in &mut s.docs {
                if d.doc.hidden_saved != keep {
                    std::sync::Arc::make_mut(&mut d.doc).hidden_saved = keep;
                }
            }
        }
        "QPMODE" => st.qpmode = as_i(v).ok_or_else(bad)? > 0,
        "HIGHLIGHT" => st.highlight = as_bool(v).ok_or_else(bad)?,
        "SELECTIONEFFECT" => st.selectioneffect = small(v, 1).ok_or_else(bad)?,
        "SELECTIONPREVIEW" => st.selectionpreview = small(v, 3).ok_or_else(bad)?,
        "PREVIEWEFFECT" => st.previeweffect = small(v, 2).ok_or_else(bad)?,
        "PREVIEWFILTER" => st.previewfilter = small(v, 63).ok_or_else(bad)?,
        "GRIPS" => st.grips = small(v, 2).ok_or_else(bad)?,
        "GRIPOBJLIMIT" => st.gripobjlimit = as_i(v).filter(|n| (0..=32_767).contains(n)).and_then(|n| u32::try_from(n).ok()).ok_or_else(bad)?,
        "SELECTIONAREA" => st.selectionarea = as_bool(v).ok_or_else(bad)?,
        "WINDOWAREACOLOR" => st.windowareacolor = small(v, 255).filter(|c| *c >= 1).ok_or_else(bad)?,
        "CROSSINGAREACOLOR" => st.crossingareacolor = small(v, 255).filter(|c| *c >= 1).ok_or_else(bad)?,
        "SELECTIONAREAOPACITY" => st.selectionareaopacity = small(v, 100).ok_or_else(bad)?,
        "PICKBOX" => st.pickbox = as_f64(v).ok_or_else(bad)?.clamp(0.0, 50.0),
        "APERTURE" => st.aperture = as_f64(v).ok_or_else(bad)?.clamp(1.0, 50.0),
        "PICKFIRST" => st.pickfirst = as_bool(v).ok_or_else(bad)?,
        "ACADLSPASDOC" => st.acadlspasdoc = as_bool(v).ok_or_else(bad)?,
        "PICKADD" => st.pickadd = as_bool(v).ok_or_else(bad)?,
        "FILEDIA" => st.filedia = as_i(v).filter(|i| (0..=1).contains(i)).ok_or_else(bad)? as u8,
        "CMDECHO" => st.cmdecho = as_i(v).filter(|i| (0..=1).contains(i)).ok_or_else(bad)? as u8,
        "EXPERT" => st.expert = as_i(v).filter(|i| (0..=5).contains(i)).ok_or_else(bad)? as u8,
        "ATTDIA" => st.attdia = as_i(v).filter(|i| (0..=1).contains(i)).ok_or_else(bad)? as u8,
        "NAVBARDISPLAY" => st.navbardisplay = as_i(v).filter(|i| (0..=1).contains(i)).ok_or_else(bad)? == 1,
        // A workspace name: the interface checks it names a workspace and applies it.
        "WSCURRENT" => {
            st.wscurrent = v
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty() && n.chars().count() <= 64 && !n.chars().any(char::is_control))
                .ok_or_else(bad)?
                .to_string()
        }
        "SELECTIONCYCLING" => st.selectioncycling = as_i(v).filter(|n| (-2..=2).contains(n)).and_then(|n| i8::try_from(n).ok()).ok_or_else(bad)?,
        "PICKSTYLE" => st.pickstyle = as_i(v).filter(|b| (0..=3).contains(b)).and_then(|b| u8::try_from(b).ok()).ok_or_else(bad)?,
        "GROUPDISPLAYMODE" => st.groupdisplaymode = small(v, 2).ok_or_else(bad)?,
        "GRIPSIZE" => st.gripsize = as_f64(v).ok_or_else(bad)?.clamp(1.0, 255.0),
        "CURSORSIZE" => st.cursorsize = as_f64(v).ok_or_else(bad)?.clamp(1.0, 100.0),
        "MAXARRAY" => st.maxarray = as_f64(v).ok_or_else(bad)?.clamp(100.0, 10_000_000.0) as u64,
        "ZOOMFACTOR" => st.zoomfactor = as_i(v).filter(|z| (3..=100).contains(z)).ok_or_else(bad)? as u32,
        "FONTALT" => cadaddy_fonts::ttf::set_font_alt(v.as_str().ok_or_else(bad)?),
        "FONTFALLBACK" => cadaddy_fonts::ttf::set_fallback_fonts(v.as_str().ok_or_else(bad)?),
        "SAVETIME" => crate::autosave::set_savetime(
            as_f64(v).filter(|m| *m >= 0.0 && *m <= f64::from(crate::autosave::MAX_SAVETIME)).ok_or_else(bad)?.round() as u32,
        ),
        "SAVEFILEPATH" => crate::autosave::set_savefilepath(v.as_str().ok_or_else(bad)?),
        "AUDITCTL" => crate::audit::set_auditctl(as_i(v).filter(|b| (0..=1).contains(b)).ok_or_else(bad)? == 1),
        "DWGCOPY" => crate::cmd::file::set_dwg_copy(as_bool(v).ok_or_else(bad)?),
        "DCTMAIN" => {
            let code = v.as_str().ok_or_else(bad)?.trim();
            if !code.eq_ignore_ascii_case(crate::cmd::spell::MAIN_CODE) {
                return Err(crate::EngineError::Other(format!(
                    "Only the American English dictionary (\"{}\") is available.",
                    crate::cmd::spell::MAIN_CODE
                )));
            }
        }
        "DCTCUST" => {
            let path = v.as_str().ok_or_else(bad)?;
            if path.len() > 4096 {
                return Err(bad());
            }
            crate::cmd::spell::set_dctcust(path);
        }
        // A number is in degrees; text is an angle typed in AUNITS (SETVAR at the command line).
        "ANGBASE" => {
            let a = match v {
                Value::String(t) => s.angle_settings().amount(t),
                _ => as_f64(v).map(f64::to_radians),
            };
            let a = a.filter(|a| a.is_finite()).ok_or_else(bad)?;
            s.doc_mut()?.header.set_f64("ANGBASE", cadaddy_geom::norm_angle(a));
        }
        // AutoCAD's range: gaps up to 5000 drawing units close a hatch boundary.
        "HPGAPTOL" => s.doc_mut()?.header.set_f64("HPGAPTOL", as_f64(v).filter(|g| (0.0..=5000.0).contains(g)).ok_or_else(bad)?),
        "CONSTRAINTINFER" => s.doc_mut()?.parametric.settings.infer = as_bool(v).ok_or_else(bad)?,
        // Text, whatever it looks like (a base such as "2024" stays text).
        "HYPERLINKBASE" => {
            let t = match v {
                Value::String(t) => t.trim().chars().take(cadaddy_doc::hyperlink::MAX_LEN).collect(),
                Value::Number(x) => x.to_string(),
                _ => return Err(bad()),
            };
            s.doc_mut()?.header.set("HYPERLINKBASE", cadaddy_doc::HVal::Str(t));
        }
        "SUNSTATUS" => s.doc_mut()?.sun.on = as_bool(v).ok_or_else(bad)?,
        "RENDEREXPOSURE" => s.doc_mut()?.sun.exposure = as_f64(v).filter(|e| (-20.0..=20.0).contains(e)).ok_or_else(bad)?,
        // ByLayer, ByBlock or one of the drawing's materials.
        "CMATERIAL" => {
            let t = v.as_str().map(str::trim).ok_or_else(bad)?;
            let d = s.doc()?;
            let name = if t.eq_ignore_ascii_case(cadaddy_doc::material::BY_LAYER) || t.is_empty() {
                cadaddy_doc::material::BY_LAYER.to_string()
            } else if t.eq_ignore_ascii_case(cadaddy_doc::material::BY_BLOCK) {
                cadaddy_doc::material::BY_BLOCK.to_string()
            } else {
                d.material(t).map(|m| m.name.clone()).ok_or_else(bad)?
            };
            s.doc_mut()?.header.set_str("CMATERIAL", &name);
        }
        "CONSTRAINTNAMEFORMAT" => {
            let f = as_f64(v).filter(|f| [0.0, 1.0, 2.0].contains(f)).ok_or_else(bad)?;
            s.doc_mut()?.parametric.settings.name_format = f as u8;
        }
        _ => {
            if let Some(r) = table::set(s, &n, v) {
                return r;
            }
            // Only variables that exist: one the drawing carries, a dimension variable, or one read
            // with a default. Anything else is refused (it used to land in the header unasked).
            let known = s.doc().is_ok_and(|d| d.header.get(&n).is_some())
                || cadaddy_doc::DIMVARS.iter().any(|(v, _)| *v == n)
                || header_default(&n).is_some()
                || READ_HEADER_VARS.contains(&n.as_str());
            if !known {
                return Err(EngineError::BadParams { cmd: "setvar".into(), msg: format!("Unknown variable name: {n}") });
            }
            let d = s.doc_mut()?;
            let val = match v {
                Value::Number(x) if x.is_i64() => cadaddy_doc::HVal::Int(x.as_i64().unwrap_or(0)),
                Value::Number(x) => cadaddy_doc::HVal::Real(x.as_f64().filter(|f| f.is_finite()).ok_or_else(bad)?),
                Value::String(t) => match t.trim().parse::<i64>() {
                    Ok(i) => cadaddy_doc::HVal::Int(i),
                    Err(_) => match t.trim().parse::<f64>() {
                        Ok(f) if f.is_finite() => cadaddy_doc::HVal::Real(f),
                        _ => cadaddy_doc::HVal::Str(t.clone()),
                    },
                },
                Value::Bool(b) => cadaddy_doc::HVal::Int(i64::from(*b)),
                Value::Array(_) | Value::Object(_) => cadaddy_doc::HVal::Point(as_pt(v).ok_or_else(bad)?.to3(0.0)),
                _ => return Err(bad()),
            };
            // Keep the header type stable for known numeric vars.
            match d.header.get(&n) {
                Some(cadaddy_doc::HVal::Real(_)) => {
                    d.header.set_f64(&n, val.as_f64().ok_or_else(bad)?);
                    return Ok(());
                }
                Some(cadaddy_doc::HVal::Int(_)) => {
                    let i = match val {
                        cadaddy_doc::HVal::Int(i) => i,
                        cadaddy_doc::HVal::Real(f) if f.fract() == 0.0 && f.abs() < 1e15 => f as i64,
                        _ => return Err(bad()),
                    };
                    d.header.set_i64(&n, i);
                    return Ok(());
                }
                // A text variable (CLAYER...) keeps what was typed, so a layer named `10` stays a string.
                Some(cadaddy_doc::HVal::Str(_)) => {
                    let text = match v {
                        Value::String(t) => t.clone(),
                        Value::Number(x) => x.to_string(),
                        _ => return Err(bad()),
                    };
                    d.header.set(&n, cadaddy_doc::HVal::Str(text));
                    return Ok(());
                }
                // A point variable takes `x,y` typed at the command line too.
                Some(cadaddy_doc::HVal::Point(_)) => {
                    d.header.set(&n, cadaddy_doc::HVal::Point(as_pt(v).ok_or_else(bad)?.to3(0.0)));
                    return Ok(());
                }
                _ => {}
            }
            d.header.set(&n, val);
        }
    }
    Ok(())
}

pub fn list(s: &Session) -> Value {
    let mut m = serde_json::Map::new();
    for n in SESSION_VARS {
        if let Some(v) = get(s, n) {
            m.insert((*n).to_string(), v);
        }
    }
    if let Ok(d) = s.doc() {
        for (k, v) in &d.header.vars {
            if matches!(k.as_str(), "ANGBASE" | "SNAPUNIT" | "GRIDUNIT") {
                m.insert(k.clone(), get(s, k).unwrap_or(Value::Null));
            } else if !k.starts_with("CADADDY_") {
                m.insert(k.clone(), serde_json::to_value(v).unwrap_or(Value::Null));
            }
        }
    }
    table::list(s, &mut m);
    Value::Object(m)
}
