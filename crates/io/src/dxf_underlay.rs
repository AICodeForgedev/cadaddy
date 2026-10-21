//! PDF underlays in DXF (DXF Reference: PDFUNDERLAY entity, AcDbUnderlayReference; PDFDEFINITION
//! object, AcDbUnderlayDefinition, named in the `ACAD_PDFDEFINITIONS` dictionary). The entity holds
//! the placement (insertion point, scale, rotation), the clip boundary and the display settings, and
//! points (group 340) to its definition, which holds the file and the page. The page size and the
//! layers turned off, which the reference groups don't hold, are kept in `CADADDY` xdata.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cadaddy_doc::{Drawing, EntityKind, EntityStore, Handle, Underlay};
use cadaddy_dxf::{Section, Tag};
use cadaddy_geom::{Vec2, Vec3};

/// Most clip boundary vertices and layer names kept per underlay (hostile files).
const MAX_CLIP: usize = 100_000;
const MAX_LAYERS: usize = 10_000;
/// The xdata list holding the page size and the layers turned off.
const LIST: &str = "UNDERLAY";

/// Flags (group 280): 1 clipping on, 2 shown, 4 monochrome.
const CLIP_ON: i64 = 1;
const SHOWN: i64 = 2;
const MONO: i64 = 4;

/// The PDFUNDERLAY entity's own groups (after the `AcDbUnderlayReference` marker when present).
pub(crate) fn read_entity(tags: &[Tag]) -> Underlay {
    let start = tags.iter().position(|t| t.code == 100 && t.str() == "AcDbUnderlayReference").unwrap_or(0);
    let body = tags.get(start..).unwrap_or(&[]);
    let own: Vec<&Tag> = body.iter().take_while(|t| t.code < 1000).collect();
    let f = |c: i32| own.iter().find(|t| t.code == c).map(|t| t.f64()).filter(|x| x.is_finite());
    let i = |c: i32| own.iter().find(|t| t.code == c).map(|t| t.i64());
    let insert = Vec3::new(f(10).unwrap_or(0.0), f(20).unwrap_or(0.0), f(30).unwrap_or(0.0));
    let (sx, sy) = (f(41).filter(|s| *s != 0.0).unwrap_or(1.0), f(42).filter(|s| *s != 0.0).unwrap_or(1.0));
    let rot = f(50).unwrap_or(0.0).to_radians();
    let (sn, cs) = rot.sin_cos();
    let flags = i(280).unwrap_or(SHOWN);
    let xs = own.iter().filter(|t| t.code == 11).map(|t| t.f64());
    let ys = own.iter().filter(|t| t.code == 21).map(|t| t.f64());
    let clip: Vec<Vec2> = xs.zip(ys).take(MAX_CLIP).map(|(x, y)| Vec2::new(x, y)).filter(|p| p.is_finite()).collect();
    let pct = |c: i32, lo: i64, hi: i64, d: u8| i(c).and_then(|v| u8::try_from(v.clamp(lo, hi)).ok()).unwrap_or(d);
    // CADaddy's own: the page size and the layers turned off.
    let list = crate::dxf_ext::xdata_list(crate::dxf_ext::xdata(tags, crate::dxf_ext::APP), LIST);
    let reals: Vec<f64> = list.iter().filter(|t| t.code == 1040).map(Tag::f64).take(2).collect();
    let size = match reals.as_slice() {
        [w, h] if w.is_finite() && h.is_finite() && *w > 0.0 && *h > 0.0 => Vec2::new(*w, *h),
        _ => Vec2::new(8.5, 11.0),
    };
    let layers_off = list.iter().filter(|t| t.code == 1000).map(Tag::str).filter(|n| !n.is_empty() && n.len() <= 255).take(MAX_LAYERS).collect();
    Underlay {
        insert,
        u: Vec3::new(cs * sx, sn * sx, 0.0),
        v: Vec3::new(-sn * sy, cs * sy, 0.0),
        size,
        path: String::new(),
        page: 1,
        name: String::new(),
        clipping: flags & CLIP_ON != 0 && clip.len() >= 2,
        clip,
        fade: pct(282, 0, 80, 25),
        contrast: pct(281, 20, 100, 20),
        monochrome: flags & MONO != 0,
        layers_off,
    }
}

/// Call `f` with each record (`0 TYPE` and the groups after it) of a section, without copying.
fn each_record<'a>(tags: &'a [Tag], mut f: impl FnMut(&str, &'a [Tag])) {
    let mut open: Option<(String, usize)> = None;
    for (i, t) in tags.iter().enumerate() {
        if t.code == 0 {
            if let Some((k, s)) = open.take() {
                f(&k, tags.get(s..i).unwrap_or(&[]));
            }
            open = Some((t.str(), i + 1));
        }
    }
    if let Some((k, s)) = open {
        f(&k, tags.get(s..).unwrap_or(&[]));
    }
}

fn key(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

fn has_underlays(store: &EntityStore) -> bool {
    store.iter().any(|e| matches!(e.kind, EntityKind::Underlay(_)))
}

/// A PDFDEFINITION's file, page and name.
#[derive(Clone, Default)]
struct Def {
    name: String,
    path: String,
    page: u32,
}

/// Fill in each underlay's file, page and name from the PDFDEFINITION it points to.
pub(crate) fn resolve(secs: &[Section], d: &mut Drawing) {
    if !has_underlays(&d.model) && !d.layouts.iter().any(|l| has_underlays(&l.entities)) && !d.blocks.values().any(|b| has_underlays(&b.entities)) {
        return;
    }
    let mut refs: HashMap<Handle, String> = HashMap::new();
    for s in secs.iter().filter(|s| s.name == "ENTITIES" || s.name == "BLOCKS") {
        each_record(&s.tags, |kind, tags| {
            if !matches!(kind, "PDFUNDERLAY" | "PDFREFERENCE") {
                return;
            }
            let h = tags.iter().find(|t| t.code == 5).and_then(|t| Handle::parse_hex(&t.str()));
            let def = tags.iter().find(|t| t.code == 340).map(|t| key(&t.str()));
            if let (Some(h), Some(def)) = (h, def) {
                refs.insert(h, def);
            }
        });
    }
    let mut defs: HashMap<String, Def> = HashMap::new();
    let mut names: HashMap<String, String> = HashMap::new();
    for s in secs.iter().filter(|s| s.name == "OBJECTS") {
        each_record(&s.tags, |kind, tags| match kind {
            "PDFDEFINITION" => {
                let Some(h) = tags.iter().find(|t| t.code == 5).map(|t| key(&t.str())) else { return };
                let start = tags.iter().position(|t| t.code == 100 && t.str() == "AcDbUnderlayDefinition").unwrap_or(0);
                let body = tags.get(start..).unwrap_or(&[]);
                let path = body.iter().find(|t| t.code == 1).map(Tag::str).unwrap_or_default();
                let page = body.iter().find(|t| t.code == 2).and_then(|t| t.str().trim().parse::<u32>().ok()).filter(|p| *p >= 1).unwrap_or(1);
                defs.insert(h, Def { path, page, ..Def::default() });
            }
            "DICTIONARY" => {
                let mut name: Option<String> = None;
                for t in tags {
                    match t.code {
                        3 => name = Some(t.str()),
                        350 | 360 => {
                            if let Some(n) = name.take() {
                                names.insert(key(&t.str()), n);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        });
    }
    for (h, def) in &mut defs {
        if let Some(n) = names.get(h) {
            def.name = n.clone();
        }
    }
    let by_ref: HashMap<Handle, Def> = refs.iter().filter_map(|(r, dh)| Some((*r, defs.get(dh)?.clone()))).collect();
    if by_ref.is_empty() {
        return;
    }
    fill(&mut d.model, &by_ref);
    for l in &mut d.layouts {
        fill(&mut l.entities, &by_ref);
    }
    for b in d.blocks.values_mut() {
        if b.entities.iter().any(|e| by_ref.contains_key(&e.handle)) {
            fill(&mut Arc::make_mut(b).entities, &by_ref);
        }
    }
}

fn fill(store: &mut EntityStore, by_ref: &HashMap<Handle, Def>) {
    let hs: Vec<Handle> =
        store.iter().filter(|e| matches!(e.kind, EntityKind::Underlay(_)) && by_ref.contains_key(&e.handle)).map(|e| e.handle).collect();
    for h in hs {
        let Some(def) = by_ref.get(&h) else { continue };
        store.modify(h, |e| {
            if let EntityKind::Underlay(u) = &mut e.kind {
                u.path = def.path.clone();
                u.page = def.page;
                u.name = def.name.clone();
            }
        });
    }
}

/// One PDFDEFINITION to write, shared by the underlays of the same file and page.
struct PlannedDef {
    handle: String,
    name: String,
    path: String,
    page: u32,
    users: Vec<Handle>,
}

/// The objects behind a drawing's underlays, with handles allocated before anything is written.
#[derive(Default)]
pub(crate) struct Plan {
    /// PDFUNDERLAY entity → its PDFDEFINITION handle.
    pub by_entity: HashMap<Handle, String>,
    defs: Vec<PlannedDef>,
    /// The `ACAD_PDFDEFINITIONS` dictionary.
    pub dict: String,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
}

/// The definition's name: its own, else the file name (without extension) and the page.
fn def_name(u: &Underlay) -> String {
    let n = u.name.trim();
    if !n.is_empty() {
        return n.to_string();
    }
    let file = u.path.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file).trim();
    format!("{} - {}", if stem.is_empty() { "PDF" } else { stem }, u.page.max(1))
}

/// Allocate the PDFDEFINITION and dictionary handles for every underlay.
pub(crate) fn plan<'a>(entities: impl Iterator<Item = &'a cadaddy_doc::Entity>, mut next: impl FnMut() -> String) -> Plan {
    let mut p = Plan::default();
    let mut index: HashMap<(String, u32, String), usize> = HashMap::new();
    let mut used: HashSet<String> = HashSet::new();
    for e in entities {
        let EntityKind::Underlay(u) = &e.kind else { continue };
        if p.by_entity.contains_key(&e.handle) {
            continue;
        }
        let k = (u.path.clone(), u.page, u.name.clone());
        let at = match index.get(&k) {
            Some(&at) => at,
            None => {
                let base = def_name(u);
                let mut name = base.clone();
                let mut n = 2usize;
                while !used.insert(key(&name)) {
                    name = format!("{base}_{n}");
                    n = n.saturating_add(1);
                }
                p.defs.push(PlannedDef { handle: next(), name, path: u.path.clone(), page: u.page.max(1), users: Vec::new() });
                let at = p.defs.len().saturating_sub(1);
                index.insert(k, at);
                at
            }
        };
        if let Some(def) = p.defs.get_mut(at) {
            def.users.push(e.handle);
            p.by_entity.insert(e.handle, def.handle.clone());
        }
    }
    if !p.defs.is_empty() {
        p.dict = next();
    }
    p
}

/// The PDFUNDERLAY entity's groups after its common entity groups, then its `CADADDY` xdata.
pub(crate) fn entity_tags(u: &Underlay, def: &str) -> Vec<Tag> {
    let (ux, vy) = (u.u.xy(), u.v.xy());
    let rot = ux.y.atan2(ux.x);
    let sx = ux.len();
    // A mirrored underlay (v on the other side of u) has a negative y scale.
    let cross = ux.x * vy.y - ux.y * vy.x;
    let sy = if cross < 0.0 { -vy.len() } else { vy.len() };
    let mut t = vec![Tag::s(100, "AcDbUnderlayReference"), Tag::s(340, def)];
    t.extend([Tag::f(10, u.insert.x), Tag::f(20, u.insert.y), Tag::f(30, u.insert.z)]);
    t.extend([Tag::f(41, sx), Tag::f(42, sy), Tag::f(43, 1.0), Tag::f(50, rot.to_degrees())]);
    t.extend([Tag::f(210, 0.0), Tag::f(220, 0.0), Tag::f(230, 1.0)]);
    let flags = SHOWN | if u.clipping && u.clip.len() >= 2 { CLIP_ON } else { 0 } | if u.monochrome { MONO } else { 0 };
    t.extend([Tag::i(280, flags), Tag::i(281, i64::from(u.contrast.clamp(20, 100))), Tag::i(282, i64::from(u.fade.min(80)))]);
    for q in u.clip.iter().take(MAX_CLIP) {
        t.extend([Tag::f(11, q.x), Tag::f(21, q.y)]);
    }
    t.extend([Tag::s(1001, crate::dxf_ext::APP), Tag::s(1000, LIST), Tag::s(1002, "{"), Tag::f(1040, u.size.x), Tag::f(1040, u.size.y)]);
    t.extend(u.layers_off.iter().take(MAX_LAYERS).map(|n| Tag::s(1000, n)));
    t.push(Tag::s(1002, "}"));
    t
}

/// The `ACAD_PDFDEFINITIONS` dictionary and its PDFDEFINITION objects; `root` is the named object
/// dictionary, which lists the dictionary.
pub(crate) fn objects(p: &Plan, root: &str) -> Vec<Tag> {
    let mut t = Vec::new();
    if p.is_empty() {
        return t;
    }
    t.extend([Tag::s(0, "DICTIONARY"), Tag::s(5, &p.dict), Tag::s(330, root), Tag::s(100, "AcDbDictionary"), Tag::i(281, 1)]);
    for d in &p.defs {
        t.extend([Tag::s(3, &d.name), Tag::s(350, &d.handle)]);
    }
    for d in &p.defs {
        t.extend([Tag::s(0, "PDFDEFINITION"), Tag::s(5, &d.handle), Tag::s(102, "{ACAD_REACTORS")]);
        t.extend(d.users.iter().map(|h| Tag::s(330, h.hex())));
        t.extend([Tag::s(102, "}"), Tag::s(330, &p.dict), Tag::s(100, "AcDbUnderlayDefinition"), Tag::s(1, &d.path), Tag::s(2, d.page.to_string())]);
    }
    t
}
