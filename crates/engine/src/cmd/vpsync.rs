//! VPSYNC: line layout viewports up with a master viewport: each takes the master's scale and
//! twist, and its view moves so that model space continues across them as if seen through one
//! window.

use serde_json::{Value, json};

use super::curves::handle_param;
use super::machines::{SelOutcome, SelectPhase};
use super::{CommandSpec, bad, targets};
use crate::{EngineError, Input, Interactive, Prompt, Result, Session, Step};
use cadaddy_doc::{EntityKind, Handle, Space, Viewport};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("vpsync", "Synchronize Viewports", run)
            .params("{master: handle (a layout viewport), handles?: [handle] (the viewports to line up; default: the selection)} → {synced, locked}")
            .interactive(|_| Ok(Box::new(VpsyncM::default()))),
    ]
}

fn viewport(s: &Session, h: Handle) -> Option<Viewport> {
    match s.doc().ok()?.entity(h).map(|e| &e.kind) {
        Some(EntityKind::Viewport(v)) => Some(v.clone()),
        _ => None,
    }
}

/// `v` seen as `master` sees model space: the master's scale and twist, and the view centre the
/// master would show at `v`'s centre.
fn synced(master: &Viewport, v: &Viewport) -> Option<(f64, f64, cadaddy_geom::Vec2)> {
    let k = master.view_height / master.height;
    let view_height = v.height * k;
    let view_center = master.to_model(v.center.xy())?;
    (view_height.is_finite() && view_height > 0.0).then_some((view_height, master.twist, view_center))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let m = handle_param(p, "master").ok_or_else(|| bad("vpsync", "`master` (a viewport) is required"))?;
    let master = viewport(s, m).ok_or_else(|| EngineError::Other("The master is not a viewport.".into()))?;
    let hs: Vec<Handle> = targets(s, p)?.into_iter().filter(|h| *h != m).collect();
    let (mut n, mut locked) = (0usize, 0usize);
    for h in hs {
        let Some(v) = viewport(s, h) else { continue };
        // A display-locked viewport keeps its view, as when zooming in it.
        if v.locked {
            locked += 1;
            continue;
        }
        let Some((view_height, twist, view_center)) = synced(&master, &v) else { continue };
        s.doc_mut()?.modify_entity(h, |e| {
            if let EntityKind::Viewport(v) = &mut e.kind {
                v.view_height = view_height;
                v.twist = twist;
                v.view_center = view_center;
            }
        })?;
        n += 1;
    }
    if n == 0 && locked == 0 {
        return Err(EngineError::Other("No viewports to synchronize.".into()));
    }
    let mut message = format!("{n} viewport(s) synchronized.");
    if locked > 0 {
        message.push_str(&format!(" {locked} locked viewport(s) left as they were."));
    }
    Ok(json!({ "synced": n, "locked": locked, "message": message }))
}

/// VPSYNC at the command line: the master viewport, then the ones to line up with it.
#[derive(Default)]
struct VpsyncM {
    master: Option<Handle>,
    pick: SelectPhase,
    sel: SelectPhase,
}

impl Interactive for VpsyncM {
    fn name(&self) -> &'static str {
        "VPSYNC"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        if matches!(s.layout_space(), Space::Model) {
            s.echo("VPSYNC works on a layout's viewports: switch to a layout tab.");
            return Ok(Step::Done);
        }
        // Viewports are picked in paper space (from inside one, as PSPACE does).
        if matches!(s.space(), Space::Model)
            && let Some(ps) = super::find_command("pspace")
        {
            (ps.run)(s, &Value::Null)?;
        }
        // The pickfirst selection isn't the master: both are asked for.
        s.set_selection(Vec::new());
        self.pick = SelectPhase::single();
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.master {
            None => Prompt { message: "Select master viewport".into(), ..self.pick.prompt() },
            Some(_) => Prompt { message: "Select viewports to be synchronized".into(), ..self.sel.prompt() },
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let Some(m) = self.master else {
            return match self.pick.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    let m = hs.first().copied().filter(|h| viewport(s, *h).is_some());
                    let Some(m) = m else {
                        self.pick = SelectPhase::single();
                        return Err(EngineError::Other("That is not a viewport.".into()));
                    };
                    self.master = Some(m);
                    Ok(Step::Continue)
                }
            };
        };
        match self.sel.feed(s, &i)? {
            SelOutcome::More => Ok(Step::Continue),
            SelOutcome::Empty => Ok(Step::Done),
            SelOutcome::Done(hs) => {
                let hs: Vec<String> = hs.iter().map(|h| h.hex()).collect();
                let r = run(s, &json!({ "master": m.hex(), "handles": hs }))?;
                if let Some(msg) = r["message"].as_str() {
                    s.echo(msg.to_string());
                }
                Ok(Step::Done)
            }
        }
    }
}
