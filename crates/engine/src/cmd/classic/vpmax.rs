//! VPMAX / VPMIN: a layout viewport made as large as the drawing area to work in its model
//! space, and back. The layout stays current; the sheet's view is fitted to the viewport and
//! its model space made active, and VPMIN puts back the sheet's view and the space it was in.
//! The viewport itself (its scale, view and size) is not changed.

use cadaddy_doc::{EntityKind, Handle, Space, Viewport};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use super::super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step, View};

/// What VPMIN puts back.
#[derive(Clone, Debug, PartialEq)]
pub struct VpMax {
    pub layout: String,
    pub viewport: Handle,
    pub paper_view: View,
    pub mspace: Option<Handle>,
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("vpmax", "Maximize Viewport", run_vpmax)
            .params("{handle?: a viewport of the current layout (default: the active one, else the only one)} → {viewport}")
            .noundo()
            .interactive(|_| Ok(Box::new(VpmaxM))),
        CommandSpec::new("vpmin", "Minimize Viewport", run_vpmin).params("{} (after VPMAX: the layout as it was)").noundo(),
    ]
}

/// The viewports of the current layout that show model space (not the sheet's own).
fn viewports(s: &Session) -> Result<Vec<(Handle, Viewport)>> {
    let Space::Paper(name) = s.layout_space() else {
        return Err(EngineError::Other("VPMAX works in a layout: there are no layout viewports on the Model tab.".into()));
    };
    let d = s.doc()?;
    Ok(d.space(&Space::Paper(name))
        .map(|st| {
            st.iter()
                .filter_map(|e| match &e.kind {
                    EntityKind::Viewport(v) if v.id != 1 && v.on && v.width > 1e-12 && v.height > 1e-12 && v.view_height > 1e-12 => {
                        Some((e.handle, v.clone()))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default())
}

fn contains(v: &Viewport, p: Vec2) -> bool {
    let c = v.center.xy();
    (p.x - c.x).abs() <= v.width / 2.0 && (p.y - c.y).abs() <= v.height / 2.0
}

fn maximize(s: &mut Session, h: Handle, vp: &Viewport) -> Result<Value> {
    let Space::Paper(layout) = s.layout_space() else {
        return Err(EngineError::Other("VPMAX works in a layout.".into()));
    };
    let (w, ph) = s.viewport_px;
    let st = s.state_mut()?;
    if st.vpmax.is_none() {
        st.vpmax = Some(VpMax { layout, viewport: h, paper_view: st.paper_view(), mspace: st.mspace });
    } else if let Some(m) = st.vpmax.as_mut() {
        m.viewport = h;
    }
    // In paper space: fit the sheet's view to the viewport, then make its model space active.
    st.mspace = None;
    let aspect = (w / ph.max(1.0)).max(1e-6);
    let height = vp.height.max(vp.width / aspect);
    st.set_view_quiet(View { center: vp.center.xy(), height });
    st.mspace = Some(h);
    st.selection.clear();
    s.pending_window = None;
    s.touch();
    Ok(json!({ "viewport": h.hex(), "message": "Viewport maximized: VPMIN restores the layout." }))
}

fn run_vpmax(s: &mut Session, p: &Value) -> Result<Value> {
    let vps = viewports(s)?;
    let want = curves::handle_param(p, "handle");
    let pick = match want {
        Some(h) => vps.iter().find(|(x, _)| *x == h).cloned().ok_or_else(|| bad("vpmax", "`handle` is not a viewport of this layout"))?,
        None => default_viewport(s, &vps).ok_or_else(|| {
            if vps.is_empty() {
                EngineError::Other("There are no viewports in this layout to maximize.".into())
            } else {
                bad("vpmax", "this layout has several viewports: give `handle`")
            }
        })?,
    };
    maximize(s, pick.0, &pick.1)
}

/// The active viewport, else the only one.
fn default_viewport(s: &Session, vps: &[(Handle, Viewport)]) -> Option<(Handle, Viewport)> {
    let active = s.state().ok()?.mspace;
    if let Some(v) = active.and_then(|h| vps.iter().find(|(x, _)| *x == h)) {
        return Some(v.clone());
    }
    match vps {
        [one] => Some(one.clone()),
        _ => None,
    }
}

fn run_vpmin(s: &mut Session, _p: &Value) -> Result<Value> {
    let m = s.state_mut()?.vpmax.take().ok_or_else(|| EngineError::Other("No viewport is maximized.".into()))?;
    let space = Space::Paper(m.layout.clone());
    if s.doc()?.layout(&m.layout).is_none() {
        return Err(EngineError::Other(format!("Layout \"{}\" no longer exists.", m.layout)));
    }
    let st = s.state_mut()?;
    st.space = space;
    st.mspace = None;
    st.set_view_quiet(m.paper_view);
    // Back in the space it was in: paper space, or the viewport that was active then.
    st.mspace = m.mspace.filter(|h| st.doc.space(&st.space).is_some_and(|store| store.contains(*h)));
    st.selection.clear();
    s.pending_window = None;
    s.touch();
    Ok(json!({ "viewport": m.viewport.hex(), "message": "Viewport minimized." }))
}

/// VPMAX typed: at once with an active or single viewport, else "Select a viewport to
/// maximize:".
struct VpmaxM;

impl Interactive for VpmaxM {
    fn name(&self) -> &'static str {
        "VPMAX"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        let vps = viewports(s)?;
        if vps.is_empty() {
            return Err(EngineError::Other("There are no viewports in this layout to maximize.".into()));
        }
        match default_viewport(s, &vps) {
            Some((h, v)) => {
                let r = maximize(s, h, &v)?;
                super::echo_message(s, &r);
                Ok(Step::Done)
            }
            None => Ok(Step::Continue),
        }
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Select a viewport to maximize", Accept::POINT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Point(p) => {
                // The pick is on the sheet (paper space).
                let vps = viewports(s)?;
                let (h, v) = vps.into_iter().find(|(_, v)| contains(v, p)).ok_or_else(|| EngineError::Other("No viewport there.".into()))?;
                let r = maximize(s, h, &v)?;
                super::echo_message(s, &r);
                Ok(Step::Done)
            }
            Input::Enter => Ok(Step::Done),
            _ => Ok(Step::Continue),
        }
    }
}
