//! Express Tools: BURST, TXT2MTXT, BREAKLINE, BLOCKREPLACE, MOCORO, and the text tools TCASE,
//! TCOUNT, TXTEXP and ARCTEXT (each in its own module).
//!
//! BURST explodes block references the way they are drawn: attributes become text with their
//! values (invisible ones are dropped), and objects on layer 0 or with ByBlock properties take the
//! reference's layer, colour, linetype, lineweight and transparency. Nested block references stay
//! block references.
//!
//! TXT2MTXT turns single-line text into multiline text: by default all of it into one MTEXT,
//! lines top to bottom, or one MTEXT per TEXT keeping its justification.

use cadaddy_doc::color::Color;
use cadaddy_doc::{Common, Entity, EntityKind, HAlign, Handle, Lineweight, MText, Text, Transparency, VAlign};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

mod arctext;
mod blockreplace;
mod breakline;
mod mocoro;
mod tcase;
mod tcount;
mod txtexp;

use super::machines::{SelOutcome, SelectPhase, SelectRun};
use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("burst", "Burst", run_burst)
            .menu(&["Modify", "Burst"])
            .params("{handles?} (block references; default the selection) → {handles}: what they burst into")
            .interactive(|_| Ok(Box::new(SelectRun::new("burst", "BURST")))),
        CommandSpec::new("txt2mtxt", "Convert to Mtext", run_txt2mtxt)
            .menu(&["Modify", "Text", "Convert to Mtext"])
            .params("{handles? (TEXT; default the selection), combine?: bool (default true: one MTEXT, lines top to bottom), wordWrap?: bool (combined: one wrapping paragraph as wide as the widest line)} → {handles}")
            .interactive(|_| Ok(Box::new(Txt2MtxtM::default()))),
        CommandSpec::new("breakline", "Break-line Symbol", breakline::run)
            .menu(&["Tools", "Express Tools", "Break-line Symbol"])
            .params("{p1, p2, at? (where the symbol goes; default the midpoint), size? (default 0.5, metric 12.5), extension? (past each point; default 0.125, metric 3)} → {handle}: a polyline on the current layer")
            .interactive(|s| Ok(Box::new(breakline::BreaklineM::new(s)))),
        CommandSpec::new("blockreplace", "Replace Block...", blockreplace::run)
            .menu(&["Tools", "Express Tools", "Replace Block..."])
            .params("{from, to (block names), purge?: bool (default true: purge the old block when nothing refers to it)} → {replaced, purged}")
            .interactive(|_| Ok(Box::new(blockreplace::BlockReplaceM::new()))),
        CommandSpec::new("mocoro", "Move/Copy/Rotate", mocoro::run)
            .menu(&["Tools", "Express Tools", "Move/Copy/Rotate"])
            .params("{handles? (default the selection), base, steps: [{move: [x, y]} | {copy: [x, y]} | {rotate: degrees} | {scale: factor} | {base: [x, y]}, …]} → {handles, base}: one undo step")
            .interactive(|_| Ok(Box::new(mocoro::MocoroM::new()))),
        CommandSpec::new("tcase", "Change Text Case", tcase::run)
            .menu(&["Tools", "Express Tools", "Change Text Case"])
            .params("{handles? (text, mtext, attribute definitions, block references' attributes; default the selection), case?: \"sentence\" (default) | \"lower\" | \"upper\" | \"title\" | \"toggle\"} → {handles}: the objects changed")
            .interactive(|_| Ok(Box::new(tcase::TcaseM::default()))),
        CommandSpec::new("tcount", "Auto Number", tcount::run)
            .menu(&["Tools", "Express Tools", "Auto Number"])
            .params("{handles? (text and mtext; default the selection), sort?: \"y\" (default, top to bottom) | \"x\" (left to right) | \"select\" (selection order), start? (1), increment? (1), placement?: \"prefix\" (default) | \"suffix\" | \"overwrite\" | \"find\", find? (the string the number replaces)} → {handles}: in numbering order")
            .interactive(|_| Ok(Box::new(tcount::TcountM::default()))),
        CommandSpec::new("txtexp", "Explode Text", txtexp::run)
            .menu(&["Tools", "Express Tools", "Explode Text"])
            .params("{handles? (text, mtext, attribute definitions; default the selection)} → {handles}: the polylines they became")
            .interactive(|_| Ok(Box::new(txtexp::interactive()))),
        CommandSpec::new("arctext", "Arc Aligned Text", arctext::run)
            .menu(&["Tools", "Express Tools", "Arc Aligned Text"])
            .params("{arc (handle), text, height? (the style's, else TEXTSIZE), style? (TEXTSTYLE), widthFactor?, side?: \"convex\" (default, outside, reading clockwise) | \"concave\", align?: \"fit\" (default) | \"left\" | \"right\" | \"center\", offset? (from the arc), spacing? (extra between characters)} → {handles, group}: one TEXT per character, as an unnamed group")
            .interactive(|_| Ok(Box::new(arctext::ArctextM::default()))),
    ]
}

// ---------- BURST ----------

/// `c` with what follows the block reference taken from `r`: layer 0 becomes the reference's
/// layer, ByBlock colour, linetype, lineweight and transparency become the reference's.
fn inherit(mut c: Common, r: &Common) -> Common {
    if c.layer == "0" {
        c.layer = r.layer.clone();
    }
    if c.color == Color::ByBlock {
        c.color = r.color;
    }
    if c.linetype.eq_ignore_ascii_case("ByBlock") {
        c.linetype = r.linetype.clone();
    }
    if c.lineweight == Lineweight::ByBlock {
        c.lineweight = r.lineweight;
    }
    if c.transparency == Transparency::ByBlock {
        c.transparency = r.transparency;
    }
    c
}

/// What block reference `e` bursts into (handles not yet assigned); `None` for other objects.
fn burst_parts(d: &cadaddy_doc::Drawing, e: &Entity) -> Option<Vec<Entity>> {
    let EntityKind::Insert(ins) = &e.kind else { return None };
    let parts = super::modify::explode_kind(d, e)?;
    let mut out: Vec<Entity> = parts
        .into_iter()
        // Attribute definitions aren't drawn; the reference's attributes are.
        .filter(|p| !matches!(p.kind, EntityKind::AttDef(_)) && p.common.visible)
        .map(|mut p| {
            p.common = inherit(p.common, &e.common);
            p
        })
        .collect();
    let blk = d.block(&ins.block);
    for a in ins.attribs.iter().filter(|a| !a.invisible) {
        // The attribute's own properties over its definition's (ByBlock without one), as drawn.
        let def = blk.and_then(|b| b.entities.iter().find(|be| matches!(&be.kind, EntityKind::AttDef(x) if x.tag.eq_ignore_ascii_case(&a.tag))));
        let base = def.map(|x| x.common.clone()).unwrap_or_else(|| Common {
            color: Color::ByBlock,
            linetype: "ByBlock".into(),
            lineweight: Lineweight::ByBlock,
            transparency: Transparency::ByBlock,
            ..Common::default()
        });
        let mut common = inherit(a.props.over(&base), &e.common);
        common.xdata.clear();
        let kind = match a.multiline_text() {
            Some(m) => EntityKind::MText(m),
            None => EntityKind::Text(a.text.clone()),
        };
        out.push(Entity { handle: Handle(0), common, kind });
    }
    Some(out)
}

fn run_burst(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let space = s.space();
    let mut out = Vec::new();
    let mut burst = 0usize;
    for h in hs {
        if super::curves::is_locked(s, h) {
            continue;
        }
        let Some(e) = s.doc()?.entity(h).map(|e| (**e).clone()) else { continue };
        let Some(parts) = burst_parts(s.doc()?, &e) else { continue };
        let d = s.doc_mut()?;
        d.remove_entity(h);
        burst += 1;
        for mut part in parts {
            part.handle = d.new_handle();
            d.ensure_layer(&part.common.layer);
            out.push(part.handle);
            if let Some(st) = d.space_mut(&space) {
                st.push(part);
            }
        }
    }
    if burst == 0 {
        return Err(bad("burst", "no block references selected"));
    }
    s.set_selection(Vec::new());
    Ok(json!({ "burst": burst, "handles": out.iter().map(|h| h.hex()).collect::<Vec<_>>() }))
}

// ---------- TXT2MTXT ----------

/// TEXT contents as MTEXT contents: backslashes and braces are literal.
fn escape(v: &str) -> String {
    let mut o = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '{' => o.push_str("\\{"),
            '}' => o.push_str("\\}"),
            c => o.push(c),
        }
    }
    o
}

/// One line of text as MTEXT contents, with inline codes for what differs from the MTEXT's own
/// height, width factor and obliquing.
fn line(t: &Text, height: f64) -> String {
    let mut codes = String::new();
    if (t.height - height).abs() > 1e-9 * height.abs().max(1.0) {
        codes.push_str(&format!("\\H{};", fmt(t.height)));
    }
    if (t.width_factor - 1.0).abs() > 1e-9 {
        codes.push_str(&format!("\\W{};", fmt(t.width_factor)));
    }
    if t.oblique.abs() > 1e-9 {
        codes.push_str(&format!("\\Q{};", fmt(t.oblique.to_degrees())));
    }
    if codes.is_empty() { escape(&t.value) } else { format!("{{{codes}{}}}", escape(&t.value)) }
}

fn fmt(x: f64) -> String {
    let s = format!("{x:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// MTEXT attachment (1..=9: TL TC TR ML MC MR BL BC BR) for a TEXT justification.
fn attachment(t: &Text) -> u8 {
    let col = match t.halign {
        HAlign::Center | HAlign::Middle => 2,
        HAlign::Right => 3,
        HAlign::Left | HAlign::Aligned | HAlign::Fit => 1,
    };
    let row = match (t.halign, t.valign) {
        (HAlign::Middle, _) | (_, VAlign::Middle) => 1,
        (_, VAlign::Top) => 0,
        _ => 2,
    };
    row * 3 + col
}

fn one_mtext(t: &Text) -> MText {
    MText {
        insert: t.justify_point().to3(t.insert.z),
        height: t.height,
        width: 0.0,
        attach: attachment(t),
        rotation: t.rotation,
        style: t.style.clone(),
        contents: line(t, t.height),
        line_spacing: 1.0,
        line_spacing_exact: false,
        columns: None,
        background: None,
    }
}

/// The width of a line of text, measured unrotated.
fn text_width(d: &cadaddy_doc::Drawing, t: &Text) -> f64 {
    let flat = Text { rotation: 0.0, halign: HAlign::Left, valign: VAlign::Baseline, align_pt: None, ..t.clone() };
    let w = cadaddy_render::place_text_entity(d, &flat, &t.value).1.width();
    if w.is_finite() { w } else { 0.0 }
}

/// All the texts as one MTEXT: top to bottom along the first one's rotation, the top-left corner
/// at the top line's start point raised by its height, so the text stays where it was.
fn combined(d: &cadaddy_doc::Drawing, mut ts: Vec<(Common, Text)>, wrap: bool) -> Option<(Common, MText)> {
    let rot = ts.first()?.1.rotation;
    let up = Vec2::from_angle(rot + std::f64::consts::FRAC_PI_2);
    ts.sort_by(|a, b| b.1.insert.xy().dot(up).total_cmp(&a.1.insert.xy().dot(up)));
    let (common, top) = ts.first().cloned()?;
    let h = top.height;
    // Line spacing: the average baseline-to-baseline distance over MTEXT's 5/3 of the height.
    let spacing = match (ts.first(), ts.last()) {
        (Some(a), Some(b)) if ts.len() > 1 && h > 0.0 => {
            let gap = (a.1.insert.xy().dot(up) - b.1.insert.xy().dot(up)) / (ts.len() - 1) as f64;
            (gap / (h * 5.0 / 3.0)).clamp(0.25, 4.0)
        }
        _ => 1.0,
    };
    let spacing = if spacing.is_finite() { spacing } else { 1.0 };
    let lines: Vec<String> = ts.iter().map(|(_, t)| line(t, h)).collect();
    let (contents, width) = if wrap {
        let w = ts.iter().map(|(_, t)| text_width(d, t)).fold(0.0, f64::max);
        (lines.join(" "), w * 1.01)
    } else {
        (lines.join("\\P"), 0.0)
    };
    let corner = top.insert.xy() + up * h;
    let m = MText {
        insert: corner.to3(top.insert.z),
        height: h,
        width,
        attach: 1,
        rotation: rot,
        style: top.style.clone(),
        contents,
        line_spacing: spacing,
        line_spacing_exact: false,
        columns: None,
        background: None,
    };
    Some((common, m))
}

fn run_txt2mtxt(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let combine = bool_or(p, "combine", true);
    let wrap = bool_or(p, "wordWrap", false);
    let space = s.space();
    let mut texts: Vec<(Handle, Common, Text)> = Vec::new();
    for h in hs {
        if super::curves::is_locked(s, h) {
            continue;
        }
        if let Some(e) = s.doc()?.entity(h)
            && let EntityKind::Text(t) = &e.kind
        {
            texts.push((h, e.common.clone(), t.clone()));
        }
    }
    if texts.is_empty() {
        return Err(bad("txt2mtxt", "no text selected"));
    }
    let made: Vec<(Common, MText)> = if combine {
        let d = s.doc()?;
        combined(d, texts.iter().map(|(_, c, t)| (c.clone(), t.clone())).collect(), wrap).into_iter().collect()
    } else {
        texts.iter().map(|(_, c, t)| (c.clone(), one_mtext(t))).collect()
    };
    let n = texts.len();
    let d = s.doc_mut()?;
    for (h, ..) in &texts {
        d.remove_entity(*h);
    }
    let mut out = Vec::new();
    for (common, m) in made {
        let h = d.new_handle();
        out.push(h);
        if let Some(st) = d.space_mut(&space) {
            st.push(Entity { handle: h, common, kind: EntityKind::MText(m) });
        }
    }
    s.set_selection(Vec::new());
    s.echo(format!("{n} text object(s) converted to {} Mtext object(s).", out.len()));
    Ok(json!({ "converted": n, "handles": out.iter().map(|h| h.hex()).collect::<Vec<_>>() }))
}

/// Keywords and Enter only.
const YES_NO: Accept = Accept { point: false, number: false, text: false, select: false, enter: true };

#[derive(Default, Clone, Copy, PartialEq)]
enum Stage {
    #[default]
    Select,
    Combine,
    Wrap,
}

/// Typed TXT2MTXT: "Select objects or [SEttings]", SEttings asks whether to combine and to wrap.
struct Txt2MtxtM {
    sel: SelectPhase,
    stage: Stage,
    combine: bool,
    wrap: bool,
}

impl Default for Txt2MtxtM {
    fn default() -> Self {
        Txt2MtxtM { sel: SelectPhase::default(), stage: Stage::Select, combine: true, wrap: false }
    }
}

impl Txt2MtxtM {
    fn run(&self, s: &mut Session, hs: &[Handle]) -> Result<Step> {
        let p = json!({ "handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>(), "combine": self.combine, "wordWrap": self.wrap });
        run_txt2mtxt(s, &p)?;
        Ok(Step::Done)
    }
}

/// Yes/No answer: `Some(true)` for Y…, `Some(false)` for N…, `None` for Enter.
fn yes_no(i: &Input) -> Result<Option<bool>> {
    match i {
        Input::Enter => Ok(None),
        Input::Text(t) | Input::Keyword(t) => match t.trim().to_ascii_lowercase().chars().next() {
            Some('y') => Ok(Some(true)),
            Some('n') => Ok(Some(false)),
            None => Ok(None),
            _ => Err(EngineError::Other("Enter Yes or No.".into())),
        },
        _ => Err(EngineError::Other("Enter Yes or No.".into())),
    }
}

impl Interactive for Txt2MtxtM {
    fn name(&self) -> &'static str {
        "TXT2MTXT"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            let hs = self.sel.picked.clone();
            return self.run(s, &hs);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        let yn = |b: bool| if b { "Yes" } else { "No" };
        match self.stage {
            Stage::Select if self.sel.removing => self.sel.prompt(),
            Stage::Select => Prompt::new("Select objects", Accept::SELECT).kw(&["SEttings"]),
            Stage::Combine => Prompt::new("Combine text into one Mtext?", YES_NO).kw(&["Yes", "No"]).default(yn(self.combine)),
            Stage::Wrap => Prompt::new("Word-wrap the combined Mtext?", YES_NO).kw(&["Yes", "No"]).default(yn(self.wrap)),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        match self.stage {
            Stage::Select => {
                if let Input::Text(t) | Input::Keyword(t) = &i
                    && matches!(t.trim().to_ascii_lowercase().as_str(), "se" | "settings")
                {
                    self.stage = Stage::Combine;
                    return Ok(Step::Continue);
                }
                match self.sel.feed(s, &i)? {
                    SelOutcome::More => Ok(Step::Continue),
                    SelOutcome::Empty => Ok(Step::Done),
                    SelOutcome::Done(hs) => self.run(s, &hs),
                }
            }
            Stage::Combine => {
                if let Some(b) = yes_no(&i)? {
                    self.combine = b;
                }
                self.stage = if self.combine { Stage::Wrap } else { Stage::Select };
                Ok(Step::Continue)
            }
            Stage::Wrap => {
                if let Some(b) = yes_no(&i)? {
                    self.wrap = b;
                }
                self.stage = Stage::Select;
                Ok(Step::Continue)
            }
        }
    }
}
