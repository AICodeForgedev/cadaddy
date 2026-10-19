//! Text tools: SCALETEXT, JUSTIFYTEXT (and the Express Tools' TJUST), TORIENT, TEXTMASK /
//! TEXTUNMASK, TCIRCLE, TEXTFIT and TFRAME.
//!
//! A single-line text is handled through its frame: the baseline start, the reading direction,
//! the width the font gives its contents and its height, placed exactly as the renderer places it
//! (`cadaddy_fonts::place`). Changing a justification, a height or a rotation recomputes the
//! insertion and alignment points from the frame, so the text stays where it reads.

use cadaddy_doc::{Common, EntityKind, HAlign, Handle, LwPolyline, MText, MTextBackground, RawTag, Text, VAlign, Wipeout};
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use super::super::machines::{SelOutcome, SelectPhase};
use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// The xdata application tagging a mask with the text it masks.
pub const MASK_APP: &str = "CADADDY_TEXTMASK";
/// Most objects one of these commands changes at once.
const MAX_TEXTS: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("scaletext", "Scale", run_scaletext)
            .menu(&["Modify", "Object", "Text", "Scale"])
            .params("{handles? (TEXT, MTEXT, ATTDEF; default the selection), base?: existing|left|center|middle|right|tl|tc|tr|ml|mc|mr|bl|bc|br (default existing: each keeps its own justification point), height? | factor? | match?: handle (the height of that text)} → {changed}")
            .interactive(|_| Ok(Box::new(ScaleTextM::default()))),
        CommandSpec::new("justifytext", "Justify", run_justifytext)
            .menu(&["Modify", "Object", "Text", "Justify"])
            .alias(&["tjust"])
            .params("{handles?, justify: left|align|fit|center|middle|right|tl|tc|tr|ml|mc|mr|bl|bc|br} → {changed} (the text stays where it is)")
            .interactive(|_| Ok(Box::new(Asked::new("justifytext", "JUSTIFYTEXT", &[JUSTIFY_Q])))),
        CommandSpec::new("torient", "Rotate Text", run_torient)
            .menu(&["Tools", "Express Tools", "Text", "Rotate Text"])
            .params("{handles?, angle?: degrees (default: most readable — upside-down text turns half a turn about its centre)} → {changed}")
            .interactive(|_| Ok(Box::new(Asked::new("torient", "TORIENT", &[TORIENT_Q])))),
        CommandSpec::new("textmask", "Mask Text", run_textmask)
            .menu(&["Tools", "Express Tools", "Text", "Mask Text"])
            .params("{handles?, offset?: factor of the text height (default 0.35), type?: wipeout|solid|background (default wipeout; MTEXT always takes a background mask)} → {masks}")
            .interactive(|_| Ok(Box::new(TextMaskM::default()))),
        CommandSpec::new("textunmask", "Unmask Text", run_textunmask)
            .menu(&["Tools", "Express Tools", "Text", "Unmask Text"])
            .params("{handles?} → {removed}")
            .interactive(|_| Ok(Box::new(Asked::new("textunmask", "TEXTUNMASK", &[])))),
        CommandSpec::new("tcircle", "Enclose Text with Object", run_tcircle)
            .menu(&["Tools", "Express Tools", "Text", "Enclose Text with Object"])
            .params("{handles?, offset?: factor of the text height (default 0.35), shape?: circles|slots|rectangles (default circles), size?: constant|variable (default variable)} → {handles}")
            .interactive(|_| Ok(Box::new(Asked::new("tcircle", "TCIRCLE", &[OFFSET_Q, SHAPE_Q, SIZE_Q])))),
        CommandSpec::new("textfit", "Text Fit", run_textfit)
            .menu(&["Tools", "Express Tools", "Text", "Text Fit"])
            .params("{handle (TEXT), end?: [x,y] (the new end along the baseline) | length?, start?: [x,y] (a new start, default the text's own)} → {widthFactor}")
            .interactive(|_| Ok(Box::new(TextFitM::default()))),
        CommandSpec::new("tframe", "Toggle Frames", run_tframe)
            .menu(&["Tools", "Express Tools", "Toggle Frames"])
            .params("{on?: bool (default: the opposite of now)} → {IMAGEFRAME, WIPEOUTFRAME}")
            .noundo(),
    ]
}

// ---------- the text frame ----------

/// Where a single-line text sits: its baseline start, reading direction, up direction, width and
/// height (aligned and fit text: as stretched between their points).
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub start: Vec2,
    pub dir: Vec2,
    pub up: Vec2,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    /// A point of the frame in its own coordinates (along the baseline, up from it).
    pub fn at(&self, x: f64, y: f64) -> Vec2 {
        self.start + self.dir * x + self.up * y
    }
    /// The text's box (descenders to cap height), counterclockwise from the bottom left.
    pub fn corners(&self, pad: f64) -> [Vec2; 4] {
        let (lo, hi) = (-self.height / 3.0 - pad, self.height + pad);
        [self.at(-pad, lo), self.at(self.width + pad, lo), self.at(self.width + pad, hi), self.at(-pad, hi)]
    }
    /// The middle of the text (its MC justification point).
    pub fn center(&self) -> Vec2 {
        self.at(self.width / 2.0, self.height / 2.0)
    }
    /// The justification point of `(h, v)`.
    pub fn point(&self, h: HAlign, v: VAlign) -> Vec2 {
        let x = match h {
            HAlign::Left | HAlign::Aligned | HAlign::Fit => 0.0,
            HAlign::Center | HAlign::Middle => self.width / 2.0,
            HAlign::Right => self.width,
        };
        let y = match (h, v) {
            (HAlign::Middle, _) => self.height / 2.0,
            (_, VAlign::Baseline) => 0.0,
            (_, VAlign::Bottom) => -self.height / 3.0,
            (_, VAlign::Middle) => self.height / 2.0,
            (_, VAlign::Top) => self.height,
        };
        self.at(x, y)
    }
}

/// The width of `t`'s contents in its style's font, at its height and width factor.
fn content_width(d: &cadaddy_doc::Drawing, t: &Text) -> f64 {
    let flat = Text { rotation: 0.0, halign: HAlign::Left, valign: VAlign::Baseline, align_pt: None, ..t.clone() };
    let w = cadaddy_render::place_text_entity(d, &flat, &t.value).0.width;
    if w.is_finite() { w.max(0.0) } else { 0.0 }
}

pub fn frame(d: &cadaddy_doc::Drawing, t: &Text) -> Frame {
    let height = if t.height.is_finite() && t.height > 0.0 { t.height } else { 1.0 };
    let w = content_width(d, t);
    if matches!(t.halign, HAlign::Aligned | HAlign::Fit)
        && let Some(p2) = t.align_pt
    {
        let (a, b) = (t.insert.xy(), p2.xy());
        let len = a.dist(b);
        if len > 1e-12 {
            let dir = (b - a) * (1.0 / len);
            let h = if t.halign == HAlign::Aligned && w > 1e-12 { height * len / w } else { height };
            return Frame { start: a, dir, up: dir.perp(), width: len, height: h };
        }
    }
    let rot = if t.rotation.is_finite() { t.rotation } else { 0.0 };
    let dir = Vec2::from_angle(rot);
    let up = dir.perp();
    let origin = t.justify_point();
    let f = Frame { start: origin, dir, up, width: w, height };
    // The justification point's offset from the baseline start.
    let off = f.point(t.halign, t.valign) - origin;
    Frame { start: origin - off, ..f }
}

/// Place `t` with justification `(h, v)` on `f` (its rotation following the frame).
pub fn place(t: &mut Text, f: &Frame, h: HAlign, v: VAlign) {
    let z = t.insert.z;
    t.halign = h;
    t.valign = if matches!(h, HAlign::Aligned | HAlign::Fit | HAlign::Middle) { VAlign::Baseline } else { v };
    t.rotation = f.dir.angle();
    t.insert = f.start.to3(z);
    match h {
        HAlign::Aligned | HAlign::Fit => {
            t.align_pt = Some(f.at(f.width, 0.0).to3(z));
            if h == HAlign::Fit {
                t.height = f.height;
            }
        }
        HAlign::Left if v == VAlign::Baseline => t.align_pt = None,
        _ => t.align_pt = Some(f.point(h, t.valign).to3(z)),
    }
}

/// A justification by AutoCAD's keyword.
fn justification(k: &str) -> Option<(HAlign, VAlign)> {
    Some(match k.trim().to_ascii_lowercase().as_str() {
        "l" | "left" => (HAlign::Left, VAlign::Baseline),
        "a" | "align" | "aligned" => (HAlign::Aligned, VAlign::Baseline),
        "f" | "fit" => (HAlign::Fit, VAlign::Baseline),
        "c" | "center" => (HAlign::Center, VAlign::Baseline),
        "m" | "middle" => (HAlign::Middle, VAlign::Baseline),
        "r" | "right" => (HAlign::Right, VAlign::Baseline),
        "tl" => (HAlign::Left, VAlign::Top),
        "tc" => (HAlign::Center, VAlign::Top),
        "tr" => (HAlign::Right, VAlign::Top),
        "ml" => (HAlign::Left, VAlign::Middle),
        "mc" => (HAlign::Center, VAlign::Middle),
        "mr" => (HAlign::Right, VAlign::Middle),
        "bl" => (HAlign::Left, VAlign::Bottom),
        "bc" => (HAlign::Center, VAlign::Bottom),
        "br" => (HAlign::Right, VAlign::Bottom),
        _ => return None,
    })
}

/// MTEXT attachment (1–9: TL TC TR ML MC MR BL BC BR) for a justification.
fn attachment(h: HAlign, v: VAlign) -> u8 {
    let col = match h {
        HAlign::Left | HAlign::Aligned | HAlign::Fit => 0,
        HAlign::Center | HAlign::Middle => 1,
        HAlign::Right => 2,
    };
    let row = match (h, v) {
        (HAlign::Middle, _) | (_, VAlign::Middle) => 1,
        (_, VAlign::Top) => 0,
        // Baseline justifications sit at the bottom, as TXT2MTXT maps them.
        (_, VAlign::Bottom | VAlign::Baseline) => 2,
    };
    row * 3 + col + 1
}

/// An MTEXT's laid-out size before its rotation: width (its reference width when it wraps) and
/// height.
fn mtext_size(d: &cadaddy_doc::Drawing, m: &MText) -> (f64, f64) {
    let flat = MText { rotation: 0.0, attach: 1, insert: Vec3::ZERO, ..m.clone() };
    let b = cadaddy_render::layout_mtext_entity(d, &flat).bounds;
    let w = if m.width > 0.0 { m.width } else { b.width() };
    let h = b.height();
    (if w.is_finite() { w } else { 0.0 }, if h.is_finite() { h } else { 0.0 })
}

/// Where an MTEXT's attachment point `a` is relative to its top left, in its own frame.
fn attach_offset(a: u8, w: f64, h: f64) -> Vec2 {
    let i = a.clamp(1, 9) - 1;
    Vec2::new(f64::from(i % 3) * w / 2.0, -f64::from(i / 3) * h / 2.0)
}

/// The MTEXT's box corners (counterclockwise from the bottom left), padded.
fn mtext_corners(d: &cadaddy_doc::Drawing, m: &MText, pad: f64) -> [Vec2; 4] {
    let (w, h) = mtext_size(d, m);
    let dir = Vec2::from_angle(if m.rotation.is_finite() { m.rotation } else { 0.0 });
    let up = dir.perp();
    let tl = m.insert.xy() - (dir * attach_offset(m.attach, w, h).x + up * attach_offset(m.attach, w, h).y);
    let at = |x: f64, y: f64| tl + dir * x + up * y;
    [at(-pad, -h - pad), at(w + pad, -h - pad), at(w + pad, pad), at(-pad, pad)]
}

/// The text objects among `hs` (TEXT, MTEXT, attribute definitions), unlocked.
fn texts(s: &Session, hs: &[Handle]) -> Result<Vec<Handle>> {
    let d = s.doc()?;
    Ok(hs
        .iter()
        .copied()
        .filter(|h| matches!(d.entity(*h).map(|e| &e.kind), Some(EntityKind::Text(_) | EntityKind::MText(_) | EntityKind::AttDef(_))))
        .filter(|h| !super::super::curves::is_locked(s, *h))
        .take(MAX_TEXTS)
        .collect())
}

fn text_targets(s: &Session, p: &Value, cmd: &str) -> Result<Vec<Handle>> {
    let hs = texts(s, &targets(s, p)?)?;
    if hs.is_empty() {
        return Err(bad(cmd, "no text selected"));
    }
    Ok(hs)
}

fn single_text(kind: &mut EntityKind) -> Option<&mut Text> {
    match kind {
        EntityKind::Text(t) => Some(t),
        EntityKind::AttDef(a) => Some(&mut a.text),
        _ => None,
    }
}

fn hexes(hs: &[Handle]) -> Vec<String> {
    hs.iter().map(|h| h.hex()).collect()
}

// ---------- SCALETEXT ----------

fn run_scaletext(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scaletext";
    let hs = text_targets(s, p, cmd)?;
    let base = match str_param(p, "base") {
        None => None,
        Some(b) if b.eq_ignore_ascii_case("existing") || b.eq_ignore_ascii_case("e") => None,
        Some(b) => Some(justification(b).ok_or_else(|| bad(cmd, "`base` must be existing or a justification (left, tl, mc, br…)"))?),
    };
    let positive = |v: f64| v.is_finite() && v > 0.0 && v < 1e12;
    let height = p.get("height").and_then(Value::as_f64);
    let factor = p.get("factor").and_then(Value::as_f64);
    let matched = match p.get("match").and_then(Value::as_str).and_then(Handle::parse_hex) {
        Some(h) => Some(match s.doc()?.entity(h).map(|e| &e.kind) {
            Some(EntityKind::Text(t)) => t.height,
            Some(EntityKind::AttDef(a)) => a.text.height,
            Some(EntityKind::MText(m)) => m.height,
            _ => return Err(bad(cmd, "`match` must be a text object")),
        }),
        None => None,
    };
    let target = match (height.or(matched), factor) {
        (Some(h), _) if positive(h) => Ok(h),
        (None, Some(k)) if positive(k) => Err(k),
        _ => return Err(bad(cmd, "pass a positive `height`, `factor` or `match`")),
    };
    let mut changed = Vec::new();
    for h in hs {
        let d = s.doc()?;
        let Some(e) = d.entity(h) else { continue };
        let mut kind = e.kind.clone();
        match &mut kind {
            EntityKind::MText(m) => {
                let k = match target {
                    Ok(nh) => nh / m.height.max(1e-12),
                    Err(k) => k,
                };
                // Scaled about the attachment point (Existing) or the chosen point of its box.
                let pivot = base.map(|(hh, vv)| {
                    let (w, ht) = mtext_size(d, m);
                    let dir = Vec2::from_angle(m.rotation);
                    let off = attach_offset(attachment(hh, vv), w, ht) - attach_offset(m.attach, w, ht);
                    m.insert.xy() + dir * off.x + dir.perp() * off.y
                });
                m.height *= k;
                m.width *= k;
                if let Some(c) = m.columns.as_mut() {
                    c.scale(k);
                }
                if let Some(pv) = pivot {
                    let q = pv + (m.insert.xy() - pv) * k;
                    m.insert = q.to3(m.insert.z);
                }
            }
            other => {
                let Some(t) = single_text(other) else { continue };
                let f = frame(d, t);
                let k = match target {
                    Ok(nh) => nh / f.height.max(1e-12),
                    Err(k) => k,
                };
                let pivot = base.map_or_else(|| f.point(t.halign, t.valign), |(hh, vv)| f.point(hh, vv));
                let nf = Frame { start: pivot + (f.start - pivot) * k, width: f.width * k, height: f.height * k, ..f };
                t.height *= k;
                let (hh, vv) = (t.halign, t.valign);
                place(t, &nf, hh, vv);
            }
        }
        s.doc_mut()?.modify_entity(h, |e| e.kind = kind)?;
        changed.push(h);
    }
    Ok(json!({ "changed": hexes(&changed) }))
}

#[derive(Default)]
struct ScaleTextM {
    sel: SelectPhase,
    picked: Vec<Handle>,
    stage: u8,
    base: Option<String>,
}

impl Interactive for ScaleTextM {
    fn name(&self) -> &'static str {
        "SCALETEXT"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = self.sel.picked.clone();
            self.stage = 1;
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.stage {
            0 => self.sel.prompt(),
            1 => Prompt::new("Enter a base point option for scaling", Accept::TEXT)
                .kw(&["Existing", "Left", "Center", "Middle", "Right", "TL", "TC", "TR", "ML", "MC", "MR", "BL", "BC", "BR"])
                .default("Existing"),
            2 => {
                let cur = s.doc().map(|d| d.header.f64("TEXTSIZE", 2.5)).unwrap_or(2.5);
                Prompt::new("Specify new model height", Accept::TEXT).kw(&["Match object", "Scale factor"]).default(format!("{cur}"))
            }
            3 => Prompt::new("Select a text object with the desired height", Accept::SELECT),
            _ => Prompt::new("Specify scale factor", Accept::TEXT).default("2"),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        let run = |s: &mut Session, me: &Self, extra: Value| -> Result<Step> {
            let mut p = json!({ "handles": hexes(&me.picked), "base": me.base.clone().unwrap_or_else(|| "existing".into()) });
            if let (Some(o), Some(e)) = (p.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            match run_scaletext(s, &p) {
                Ok(r) => s.echo(format!("{} object(s) scaled.", r["changed"].as_array().map_or(0, Vec::len))),
                Err(e) => s.echo(e.to_string()),
            }
            s.set_selection(Vec::new());
            Ok(Step::Done)
        };
        let text = match &i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            Input::Enter => String::new(),
            _ => String::new(),
        };
        match self.stage {
            0 => match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.picked = hs;
                    self.stage = 1;
                }
            },
            1 => {
                if text.is_empty() || text.eq_ignore_ascii_case("e") || text.eq_ignore_ascii_case("existing") {
                    self.base = None;
                } else if justification(&text).is_some() {
                    self.base = Some(text);
                } else {
                    s.echo("Invalid option keyword.");
                    return Ok(Step::Continue);
                }
                self.stage = 2;
            }
            2 => {
                let low = text.to_ascii_lowercase();
                if low.starts_with('m') {
                    self.stage = 3;
                } else if low.starts_with('s') {
                    self.stage = 4;
                } else {
                    let h = if text.is_empty() {
                        s.doc()?.header.f64("TEXTSIZE", 2.5)
                    } else {
                        super::super::machines::number(&text).unwrap_or(f64::NAN)
                    };
                    if !(h.is_finite() && h > 0.0) {
                        s.echo("Requires a positive number.");
                        return Ok(Step::Continue);
                    }
                    return run(s, self, json!({ "height": h }));
                }
            }
            3 => {
                if let Input::Pick(hs) = &i
                    && let Some(h) = hs.first()
                {
                    return run(s, self, json!({ "match": h.hex() }));
                }
            }
            _ => {
                let k = if text.is_empty() { 2.0 } else { super::super::machines::number(&text).unwrap_or(f64::NAN) };
                if !(k.is_finite() && k > 0.0) {
                    s.echo("Requires a positive number.");
                    return Ok(Step::Continue);
                }
                return run(s, self, json!({ "factor": k }));
            }
        }
        Ok(Step::Continue)
    }
}

// ---------- JUSTIFYTEXT ----------

fn run_justifytext(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "justifytext";
    let hs = text_targets(s, p, cmd)?;
    let (h, v) = str_param(p, "justify")
        .and_then(justification)
        .ok_or_else(|| bad(cmd, "`justify` must be left, align, fit, center, middle, right, tl, tc, tr, ml, mc, mr, bl, bc or br"))?;
    let mut changed = Vec::new();
    for hd in hs {
        let d = s.doc()?;
        let Some(e) = d.entity(hd) else { continue };
        let mut kind = e.kind.clone();
        match &mut kind {
            EntityKind::MText(m) => {
                let a = attachment(h, v);
                let (w, ht) = mtext_size(d, m);
                let dir = Vec2::from_angle(m.rotation);
                let off = attach_offset(a, w, ht) - attach_offset(m.attach, w, ht);
                m.insert = (m.insert.xy() + dir * off.x + dir.perp() * off.y).to3(m.insert.z);
                m.attach = a;
            }
            other => {
                let Some(t) = single_text(other) else { continue };
                let f = frame(d, t);
                place(t, &f, h, v);
            }
        }
        s.doc_mut()?.modify_entity(hd, |e| e.kind = kind)?;
        changed.push(hd);
    }
    Ok(json!({ "changed": hexes(&changed) }))
}

// ---------- TORIENT ----------

fn run_torient(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "torient";
    let hs = text_targets(s, p, cmd)?;
    let angle = match p.get("angle") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().filter(|a| a.is_finite()).ok_or_else(|| bad(cmd, "`angle` must be a number of degrees"))?.to_radians()),
    };
    // Most readable: a reading direction pointing left (past 90°, up to 270°) turns over.
    let readable = |rot: f64| {
        let r = rot.rem_euclid(TAU);
        if r > FRAC_PI_2 + 1e-9 && r <= 1.5 * PI + 1e-9 { (r + PI).rem_euclid(TAU) } else { r }
    };
    let mut changed = Vec::new();
    for hd in hs {
        let d = s.doc()?;
        let Some(e) = d.entity(hd) else { continue };
        let mut kind = e.kind.clone();
        match &mut kind {
            EntityKind::MText(m) => {
                let new = angle.unwrap_or_else(|| readable(m.rotation));
                let c = mtext_corners(d, m, 0.0);
                let center = (c[0] + c[2]) * 0.5;
                let turn = new - m.rotation;
                m.insert = (center + (m.insert.xy() - center).rotate(turn)).to3(m.insert.z);
                m.rotation = new;
            }
            other => {
                let Some(t) = single_text(other) else { continue };
                let f = frame(d, t);
                let new = angle.unwrap_or_else(|| readable(f.dir.angle()));
                let turn = new - f.dir.angle();
                let c = f.center();
                let nf = Frame { start: c + (f.start - c).rotate(turn), dir: f.dir.rotate(turn), up: f.up.rotate(turn), ..f };
                let (hh, vv) = (t.halign, t.valign);
                place(t, &nf, hh, vv);
            }
        }
        s.doc_mut()?.modify_entity(hd, |e| e.kind = kind)?;
        changed.push(hd);
    }
    Ok(json!({ "changed": hexes(&changed) }))
}

// ---------- TEXTMASK / TEXTUNMASK ----------

fn mask_tag(of: Handle) -> Vec<RawTag> {
    vec![RawTag { code: 1001, value: MASK_APP.into() }, RawTag { code: 1005, value: of.hex() }]
}

/// The text a mask masks (its xdata).
fn masked_text(c: &Common) -> Option<Handle> {
    let i = c.xdata.iter().position(|t| t.code == 1001 && t.value == MASK_APP)?;
    c.xdata.get(i + 1).filter(|t| t.code == 1005).and_then(|t| Handle::parse_hex(&t.value))
}

/// The masks of `text` in its space.
fn masks_of(s: &Session, text: Handle) -> Result<Vec<Handle>> {
    let d = s.doc()?;
    let Some(sp) = d.space_of(text) else { return Ok(Vec::new()) };
    Ok(d.space(&sp).map(|st| st.iter().filter(|e| masked_text(&e.common) == Some(text)).map(|e| e.handle).collect()).unwrap_or_default())
}

fn run_textmask(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "textmask";
    let hs = text_targets(s, p, cmd)?;
    let offset = p.get("offset").and_then(Value::as_f64).unwrap_or(0.35);
    if !(0.0..=10.0).contains(&offset) {
        return Err(bad(cmd, "`offset` must be from 0 to 10 (times the text height)"));
    }
    let kind = str_param(p, "type").unwrap_or("wipeout").to_ascii_lowercase();
    if !["wipeout", "solid", "background"].contains(&kind.as_str()) {
        return Err(bad(cmd, "`type` must be wipeout, solid or background"));
    }
    let mut masks = Vec::new();
    for h in hs {
        // A text masked again gets a new mask in place of the old.
        for old in masks_of(s, h)? {
            s.doc_mut()?.remove_entity(old);
        }
        let d = s.doc()?;
        let Some(e) = d.entity(h).map(|e| (**e).clone()) else { continue };
        let corners = match &e.kind {
            EntityKind::MText(_) => None,
            EntityKind::Text(t) => Some(frame(d, t)),
            EntityKind::AttDef(a) => Some(frame(d, &a.text)),
            _ => continue,
        }
        .map(|f| f.corners(offset * f.height));
        let Some(corners) = corners.filter(|_| kind != "background") else {
            // MTEXT (and the background type): its own background mask.
            s.doc_mut()?.modify_entity(h, |e| {
                if let EntityKind::MText(m) = &mut e.kind {
                    m.background = Some(MTextBackground { color: None, scale: 1.0 + 2.0 * offset, transparency: 0 });
                }
            })?;
            masks.push(h);
            continue;
        };
        let mask_kind = if kind == "solid" {
            EntityKind::Solid(cadaddy_doc::Solid { corners: [corners[0].to3(0.0), corners[1].to3(0.0), corners[3].to3(0.0), corners[2].to3(0.0)] })
        } else {
            EntityKind::Wipeout(Wipeout { boundary: corners.to_vec() })
        };
        let mut common = Common { layer: e.common.layer.clone(), xdata: mask_tag(h), ..Common::default() };
        if kind == "solid" {
            common.color = cadaddy_doc::color::Color::Index(8);
        }
        let d = s.doc_mut()?;
        let Some(sp) = d.space_of(h) else { continue };
        let mh = d.add(&sp, common, mask_kind)?;
        masks.push(mh);
        // Under the text it masks.
        if let Some(spec) = find_command("draworder.under") {
            (spec.run)(s, &json!({ "handles": [mh.hex()], "reference": [h.hex()] }))?;
        }
    }
    Ok(json!({ "masks": hexes(&masks) }))
}

fn run_textunmask(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = text_targets(s, p, "textunmask")?;
    let mut removed = 0usize;
    for h in hs {
        for m in masks_of(s, h)? {
            s.doc_mut()?.remove_entity(m);
            removed += 1;
        }
        let had_background = matches!(s.doc()?.entity(h).map(|e| &e.kind), Some(EntityKind::MText(m)) if m.background.is_some());
        if had_background {
            s.doc_mut()?.modify_entity(h, |e| {
                if let EntityKind::MText(m) = &mut e.kind {
                    m.background = None;
                }
            })?;
            removed += 1;
        }
    }
    Ok(json!({ "removed": removed }))
}

#[derive(Default)]
struct TextMaskM {
    sel: SelectPhase,
    offset: Option<f64>,
    solid: bool,
    ask: u8,
}

impl Interactive for TextMaskM {
    fn name(&self) -> &'static str {
        "TEXTMASK"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        s.echo(format!(
            "Current settings: Offset factor = {}, Mask type = {}",
            self.offset.unwrap_or(0.35),
            if self.solid { "Solid" } else { "Wipeout" }
        ));
        Ok(Step::Continue)
    }
    fn prompt(&self, _: &Session) -> Prompt {
        match self.ask {
            1 => Prompt::new("Specify mask type", Accept::TEXT).kw(&["Wipeout", "Solid"]).default(if self.solid { "Solid" } else { "Wipeout" }),
            2 => Prompt::new("Enter offset factor relative to text height", Accept::TEXT).default(format!("{}", self.offset.unwrap_or(0.35))),
            _ => Prompt::new("Select text objects to mask or", Accept::SELECT).kw(&["Masktype", "Offset"]),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        let text = match &i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_ascii_lowercase(),
            _ => String::new(),
        };
        match self.ask {
            1 => {
                self.solid = text.starts_with('s');
                self.ask = 0;
            }
            2 => {
                match (text.is_empty(), super::super::machines::number(&text)) {
                    (true, _) => {}
                    (false, Some(k)) if (0.0..=10.0).contains(&k) => self.offset = Some(k),
                    _ => {
                        s.echo("Requires a number from 0 to 10.");
                        return Ok(Step::Continue);
                    }
                }
                self.ask = 0;
            }
            _ if text.starts_with('m') => self.ask = 1,
            _ if text.starts_with('o') => self.ask = 2,
            _ => match self.sel.feed(s, &i)? {
                SelOutcome::More => {}
                SelOutcome::Empty => return Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    let p =
                        json!({ "handles": hexes(&hs), "offset": self.offset.unwrap_or(0.35), "type": if self.solid { "solid" } else { "wipeout" } });
                    match run_textmask(s, &p) {
                        Ok(r) => s.echo(format!("{} text item(s) masked.", r["masks"].as_array().map_or(0, Vec::len))),
                        Err(e) => s.echo(e.to_string()),
                    }
                    s.set_selection(Vec::new());
                    return Ok(Step::Done);
                }
            },
        }
        Ok(Step::Continue)
    }
}

// ---------- TCIRCLE ----------

fn run_tcircle(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "tcircle";
    let hs = text_targets(s, p, cmd)?;
    let offset = p.get("offset").and_then(Value::as_f64).unwrap_or(0.35);
    if !(0.0..=1.0).contains(&offset) {
        return Err(bad(cmd, "`offset` must be from 0 to 1 (times the text height)"));
    }
    let shape = str_param(p, "shape").unwrap_or("circles").to_ascii_lowercase();
    let constant = str_param(p, "size").is_some_and(|v| v.to_ascii_lowercase().starts_with('c'));
    // Each text's box: centre, direction, half width and half height (padded).
    let mut boxes = Vec::new();
    for h in &hs {
        let d = s.doc()?;
        let Some(e) = d.entity(*h) else { continue };
        let (c, dir, hw, hh, pad) = match &e.kind {
            EntityKind::MText(m) => {
                let k = mtext_corners(d, m, 0.0);
                ((k[0] + k[2]) * 0.5, Vec2::from_angle(m.rotation), k[0].dist(k[1]) / 2.0, k[1].dist(k[2]) / 2.0, offset * m.height)
            }
            EntityKind::Text(t) | EntityKind::AttDef(cadaddy_doc::Attrib { text: t, .. }) => {
                let f = frame(d, t);
                let lo = -f.height / 3.0;
                (f.at(f.width / 2.0, (lo + f.height) / 2.0), f.dir, f.width / 2.0, (f.height - lo) / 2.0, offset * f.height)
            }
            _ => continue,
        };
        boxes.push((e.common.layer.clone(), c, dir, hw + pad, hh + pad));
    }
    if constant {
        let (mw, mh) = boxes.iter().fold((0.0f64, 0.0f64), |(a, b), x| (a.max(x.3), b.max(x.4)));
        for b in &mut boxes {
            b.3 = mw;
            b.4 = mh;
        }
    }
    let mut out = Vec::new();
    let space = s.space();
    for (layer, c, dir, hw, hh) in boxes {
        let up = dir.perp();
        let kind = match shape.chars().next() {
            Some('s') => {
                // A slot: straight sides with half-circle ends as tall as the box.
                let r = hh;
                let half = (hw - r).max(0.0);
                let v = |p: Vec2, bulge: f64| PolyVertex { p, bulge, start_width: 0.0, end_width: 0.0 };
                let pts = [c - dir * half - up * r, c + dir * half - up * r, c + dir * half + up * r, c - dir * half + up * r];
                EntityKind::LwPolyline(LwPolyline {
                    vertices: vec![v(pts[0], 0.0), v(pts[1], 1.0), v(pts[2], 0.0), v(pts[3], 1.0)],
                    closed: true,
                    const_width: 0.0,
                    elevation: 0.0,
                    plinegen: false,
                })
            }
            Some('r') => {
                let v = |p: Vec2| PolyVertex { p, bulge: 0.0, start_width: 0.0, end_width: 0.0 };
                let pts = [c - dir * hw - up * hh, c + dir * hw - up * hh, c + dir * hw + up * hh, c - dir * hw + up * hh];
                EntityKind::LwPolyline(LwPolyline {
                    vertices: pts.iter().map(|p| v(*p)).collect(),
                    closed: true,
                    const_width: 0.0,
                    elevation: 0.0,
                    plinegen: false,
                })
            }
            _ => EntityKind::Circle(cadaddy_doc::Circle { center: c.to3(0.0), radius: (hw * hw + hh * hh).sqrt() }),
        };
        let common = Common { layer, ..s.current_common() };
        let h = s.doc_mut()?.add(&space, common, kind)?;
        out.push(h);
    }
    Ok(json!({ "handles": hexes(&out) }))
}

// ---------- TEXTFIT ----------

fn run_textfit(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "textfit";
    let hs = text_targets(s, p, cmd)?;
    let Some(h) = hs.first().copied() else { return Err(bad(cmd, "no text selected")) };
    let d = s.doc()?;
    let Some(EntityKind::Text(t)) = d.entity(h).map(|e| e.kind.clone()) else { return Err(bad(cmd, "TEXTFIT fits single-line text (TEXT)")) };
    let f = frame(d, &t);
    let start = match p.get("start") {
        Some(v) => point_value(v).ok_or_else(|| bad(cmd, "`start` must be [x, y]"))?,
        None => f.start,
    };
    let len = match (p.get("end").and_then(point_value), p.get("length").and_then(Value::as_f64)) {
        (Some(e), _) => (e - start).dot(f.dir),
        (None, Some(l)) => l,
        _ => return Err(bad(cmd, "pass `end` (a point along the baseline) or `length`")),
    };
    if !(len.is_finite() && len > 1e-9 && f.width > 1e-12) {
        return Err(bad(cmd, "the new end must be past the start, along the text"));
    }
    let k = len / f.width;
    let mut t2 = t.clone();
    // The width factor of the text; aligned and fit text span their points instead.
    let (hh, vv) = match t.halign {
        HAlign::Aligned | HAlign::Fit => (HAlign::Fit, VAlign::Baseline),
        other => (other, t.valign),
    };
    if !matches!(t.halign, HAlign::Aligned | HAlign::Fit) {
        t2.width_factor = (t.width_factor * k).clamp(0.01, 100.0);
    }
    // A new start slides along the baseline.
    let nf = Frame { start: f.start + f.dir * (start - f.start).dot(f.dir), width: len, ..f };
    place(&mut t2, &nf, hh, vv);
    let wf = t2.width_factor;
    s.doc_mut()?.modify_entity(h, |e| e.kind = EntityKind::Text(t2))?;
    Ok(json!({ "widthFactor": wf, "handle": h.hex() }))
}

#[derive(Default)]
struct TextFitM {
    sel: SelectPhase,
    text: Option<Handle>,
    start: Option<Vec2>,
    asking_start: bool,
}

impl Interactive for TextFitM {
    fn name(&self) -> &'static str {
        "TEXTFIT"
    }
    fn begin(&mut self, _s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::single();
        Ok(Step::Continue)
    }
    fn prompt(&self, s: &Session) -> Prompt {
        match self.text {
            None => Prompt::new("Select text to stretch or shrink", Accept::SELECT),
            Some(_) if self.asking_start => Prompt::new("Specify new starting point", Accept::POINT),
            Some(h) => {
                let base = s.doc().ok().and_then(|d| match d.entity(h).map(|e| &e.kind) {
                    Some(EntityKind::Text(t)) => Some(self.start.unwrap_or(frame(d, t).start)),
                    _ => None,
                });
                let p = Prompt::new("Specify end point or", Accept::POINT).kw(&["Start point"]);
                match base {
                    Some(b) => p.base(b),
                    None => p,
                }
            }
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) || (matches!(i, Input::Enter) && self.text.is_some()) {
            return Ok(Step::Cancel);
        }
        match self.text {
            None => {
                if let SelOutcome::Done(hs) = self.sel.feed(s, &i)? {
                    let d = s.doc()?;
                    match hs.iter().copied().find(|h| matches!(d.entity(*h).map(|e| &e.kind), Some(EntityKind::Text(_)))) {
                        Some(h) => self.text = Some(h),
                        None => {
                            s.echo("Select a single-line text (TEXT).");
                            self.sel = SelectPhase::single();
                        }
                    }
                }
                Ok(Step::Continue)
            }
            Some(h) => match i {
                Input::Keyword(k) | Input::Text(k) if k.trim().to_ascii_lowercase().starts_with('s') => {
                    self.asking_start = true;
                    Ok(Step::Continue)
                }
                Input::Point(p) if self.asking_start => {
                    self.start = Some(p);
                    self.asking_start = false;
                    Ok(Step::Continue)
                }
                Input::Point(p) => {
                    let mut params = json!({ "handle": h.hex(), "end": [p.x, p.y] });
                    if let (Some(st), Some(o)) = (self.start, params.as_object_mut()) {
                        o.insert("start".into(), json!([st.x, st.y]));
                    }
                    if let Err(e) = run_textfit(s, &params) {
                        s.echo(e.to_string());
                        return Ok(Step::Continue);
                    }
                    s.set_selection(Vec::new());
                    Ok(Step::Done)
                }
                _ => Ok(Step::Continue),
            },
        }
    }
}

// ---------- TFRAME ----------

fn run_tframe(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc_mut()?;
    let now = d.header.i64("IMAGEFRAME", 1) != 0 || d.header.i64("WIPEOUTFRAME", 1) != 0;
    let on = p.get("on").and_then(Value::as_bool).unwrap_or(!now);
    let v = i64::from(on);
    d.header.set_i64("IMAGEFRAME", v);
    d.header.set_i64("WIPEOUTFRAME", v);
    s.echo(if on { "Image and wipeout frames are on." } else { "Image and wipeout frames are off." });
    Ok(json!({ "IMAGEFRAME": v, "WIPEOUTFRAME": v }))
}

// ---------- prompts after a selection ----------

/// A question asked after the selection: the parameter it sets, the prompt, its keywords (none:
/// a number) and its default.
#[derive(Clone, Copy)]
struct Question {
    key: &'static str,
    message: &'static str,
    keywords: &'static [&'static str],
    default: &'static str,
}

const JUSTIFY_Q: Question = Question {
    key: "justify",
    message: "Enter a justification option",
    keywords: &["Left", "Align", "Fit", "Center", "Middle", "Right", "TL", "TC", "TR", "ML", "MC", "MR", "BL", "BC", "BR"],
    default: "Existing",
};
const TORIENT_Q: Question = Question { key: "angle", message: "New absolute rotation", keywords: &["Most readable"], default: "Most readable" };
const OFFSET_Q: Question = Question { key: "offset", message: "Enter distance offset factor [0 - 1]", keywords: &[], default: "0.35" };
const SHAPE_Q: Question = Question { key: "shape", message: "Enclose text with", keywords: &["Circles", "Slots", "Rectangles"], default: "Circles" };
const SIZE_Q: Question =
    Question { key: "size", message: "Create circles of constant or variable size?", keywords: &["Constant", "Variable"], default: "Variable" };

/// "Select objects:", then the questions, then the JSON command with `{handles, …answers}`.
struct Asked {
    id: &'static str,
    name: &'static str,
    questions: &'static [Question],
    sel: SelectPhase,
    picked: Vec<Handle>,
    answers: serde_json::Map<String, Value>,
    at: usize,
}

impl Asked {
    fn new(id: &'static str, name: &'static str, questions: &'static [Question]) -> Asked {
        Asked { id, name, questions, sel: SelectPhase::default(), picked: Vec::new(), answers: serde_json::Map::new(), at: 0 }
    }

    fn finish(&mut self, s: &mut Session) -> Result<Step> {
        let spec = find_command(self.id).ok_or_else(|| EngineError::UnknownCommand(self.id.into()))?;
        let mut p = self.answers.clone();
        p.insert("handles".into(), json!(hexes(&self.picked)));
        match (spec.run)(s, &Value::Object(p)) {
            Ok(r) => {
                let n = ["changed", "handles", "masks"].iter().find_map(|k| r.get(*k).and_then(Value::as_array).map(Vec::len));
                match (n, r.get("removed").and_then(Value::as_u64)) {
                    (Some(n), _) => s.echo(format!("{n} object(s) changed.")),
                    (None, Some(n)) => s.echo(format!("{n} mask(s) removed.")),
                    _ => {}
                }
            }
            Err(e) => s.echo(e.to_string()),
        }
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
}

impl Interactive for Asked {
    fn name(&self) -> &'static str {
        self.name
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = self.sel.picked.clone();
            if self.questions.is_empty() {
                return self.finish(s);
            }
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _: &Session) -> Prompt {
        match self.questions.get(self.at).filter(|_| !self.picked.is_empty()) {
            None => self.sel.prompt(),
            // A number question takes a number, so a script line goes on to the next answer.
            Some(q) if q.keywords.is_empty() => Prompt::new(q.message, Accept::NUMBER).default(q.default),
            Some(q) => Prompt::new(q.message, Accept::TEXT).kw(q.keywords).default(q.default),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.picked.is_empty() {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.picked = hs;
                    if self.questions.is_empty() { self.finish(s) } else { Ok(Step::Continue) }
                }
            };
        }
        let Some(q) = self.questions.get(self.at).copied() else { return self.finish(s) };
        let t = match &i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            _ => String::new(),
        };
        let low = t.to_ascii_lowercase();
        let value = if t.is_empty() || low == q.default.to_ascii_lowercase() {
            None
        } else if q.keywords.is_empty() || q.key == "angle" {
            match super::super::machines::number(&t) {
                Some(n) => Some(json!(n)),
                None if q.key == "angle" && "most readable".starts_with(&low) => None,
                None => {
                    s.echo("Requires a number.");
                    return Ok(Step::Continue);
                }
            }
        } else if q.key == "justify" {
            if justification(&t).is_none() && !"existing".starts_with(&low) {
                s.echo("Invalid option keyword.");
                return Ok(Step::Continue);
            }
            justification(&t).map(|_| json!(low))
        } else {
            match q.keywords.iter().find(|k| k.to_ascii_lowercase().starts_with(&low)) {
                Some(k) => Some(json!(k.to_ascii_lowercase())),
                None => {
                    s.echo("Invalid option keyword.");
                    return Ok(Step::Continue);
                }
            }
        };
        match value {
            Some(v) => {
                self.answers.insert(q.key.into(), v);
            }
            // "Existing" justification: nothing to change.
            None if q.key == "justify" => {
                s.set_selection(Vec::new());
                return Ok(Step::Done);
            }
            None => {}
        }
        self.at += 1;
        if self.at >= self.questions.len() { self.finish(s) } else { Ok(Step::Continue) }
    }
}
