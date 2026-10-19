//! CADaddy's 3D furniture and fixture library: original parametric models, each a block with a 3D
//! body (3DFACE objects) and its 2D plan view, built here in code from boxes, prisms and
//! cylinders at real sizes. Nothing is copied from an outside model library.
//!
//! Sizes are inches. Like the 2D fixtures ([`super::symbols`]), a model's insertion point is the
//! middle of its back edge (the wall it stands or hangs against), the model extending in −Y; free-
//! standing furniture is centred on its insertion point. The plan view lies at Z 0; the body
//! stands at its mounting height ([`Model::mount_in`]: the bottom of the body above the floor).
//! Models with a 2D library symbol name it ([`Model::twin_of`]): a placed symbol can get its 3D
//! twin at that height.

use cadaddy_geom::{PolyVertex, Vec2, Vec3};

use crate::{Circle, Entity, EntityKind, Face3d, Handle, Line, LwPolyline};

/// One library model.
#[derive(Clone, Copy, Debug)]
pub struct Model {
    /// The block name it is placed as (`M3_DESK`).
    pub name: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    /// Bottom of the body above the floor, inches.
    pub mount_in: f64,
    /// The 2D library symbol this is the 3D twin of.
    pub twin_of: Option<&'static str>,
    pub keywords: &'static str,
    plan: fn(&mut Plan),
    body: fn(&mut Body),
}

impl Model {
    /// The plan view (Z 0), handle 0.
    pub fn plan(&self) -> Vec<Entity> {
        let mut p = Plan::default();
        (self.plan)(&mut p);
        p.out
    }
    /// The 3D body at its mounting height, handle 0.
    pub fn body(&self) -> Vec<Entity> {
        let mut b = Body { out: Vec::new(), z0: self.mount_in };
        (self.body)(&mut b);
        b.out
    }
    /// The block's objects: the plan view and the body.
    pub fn entities(&self) -> Vec<Entity> {
        let mut v = self.plan();
        v.extend(self.body());
        v
    }
    pub fn matches(&self, query: &str) -> bool {
        let hay = format!("{} {} {} {}", self.name, self.label, self.category, self.keywords).to_ascii_lowercase();
        query.split_whitespace().all(|w| hay.contains(&w.to_ascii_lowercase()))
    }
}

/// Every model, in palette order.
pub fn models() -> &'static [Model] {
    MODELS
}

/// The model named `name` (any case).
pub fn model(name: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.name.eq_ignore_ascii_case(name.trim()))
}

/// The model that is the 3D twin of 2D symbol `symbol`.
pub fn twin(symbol: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.twin_of.is_some_and(|t| t.eq_ignore_ascii_case(symbol.trim())))
}

/// Builds a plan view.
#[derive(Default)]
pub struct Plan {
    out: Vec<Entity>,
}

impl Plan {
    fn add(&mut self, kind: EntityKind) {
        self.out.push(Entity::new(Handle(0), kind));
    }
    fn line(&mut self, a: (f64, f64), b: (f64, f64)) {
        self.add(EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) }));
    }
    fn circle(&mut self, c: (f64, f64), r: f64) {
        self.add(EntityKind::Circle(Circle { center: Vec3::new(c.0, c.1, 0.0), radius: r }));
    }
    /// A closed outline through `pts`.
    fn outline(&mut self, pts: &[(f64, f64)]) {
        let vertices = pts.iter().map(|&(x, y)| PolyVertex { p: Vec2::new(x, y), ..PolyVertex::default() }).collect();
        self.add(EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }));
    }
    fn rect(&mut self, a: (f64, f64), b: (f64, f64)) {
        self.outline(&[(a.0, a.1), (b.0, a.1), (b.0, b.1), (a.0, b.1)]);
    }
}

/// Builds a 3D body of faces; heights are above the body's bottom ([`Model::mount_in`]).
pub struct Body {
    out: Vec<Entity>,
    z0: f64,
}

/// Facets around a cylinder.
const SIDES: usize = 16;

impl Body {
    fn face(&mut self, c: [Vec3; 4]) {
        self.out.push(Entity::new(Handle(0), EntityKind::Face3d(Face3d { corners: c, hidden_edges: 0 })));
    }
    /// A box from plan corner `a` to `b`, from height `z0` to `z1`.
    fn cube(&mut self, a: (f64, f64), b: (f64, f64), z0: f64, z1: f64) {
        self.prism(&[(a.0, a.1), (b.0, a.1), (b.0, b.1), (a.0, b.1)], z0, z1);
    }
    /// The plan polygon `pts` (convex) stood up from `z0` to `z1`, with top and bottom.
    fn prism(&mut self, pts: &[(f64, f64)], z0: f64, z1: f64) {
        let (z0, z1) = (self.z0 + z0, self.z0 + z1);
        let n = pts.len();
        if n < 3 {
            return;
        }
        let v = |p: (f64, f64), z: f64| Vec3::new(p.0, p.1, z);
        for i in 0..n {
            let (p, q) = (pts[i], pts[(i + 1) % n]);
            self.face([v(p, z0), v(q, z0), v(q, z1), v(p, z1)]);
        }
        // Caps as fans of three-cornered faces (the fourth corner repeats the third).
        for z in [z0, z1] {
            for i in 1..n - 1 {
                self.face([v(pts[0], z), v(pts[i], z), v(pts[i + 1], z), v(pts[i + 1], z)]);
            }
        }
    }
    /// A cylinder (a many-sided prism) on centre `c` of radius `r`.
    fn cylinder(&mut self, c: (f64, f64), r: f64, z0: f64, z1: f64) {
        let pts: Vec<(f64, f64)> =
            (0..SIDES).map(|i| std::f64::consts::TAU * i as f64 / SIDES as f64).map(|a| (c.0 + r * a.cos(), c.1 + r * a.sin())).collect();
        self.prism(&pts, z0, z1);
    }
    /// Four legs of `s` square under the box from `a` to `b`, `h` high.
    fn legs(&mut self, a: (f64, f64), b: (f64, f64), s: f64, h: f64) {
        let (x0, x1, y0, y1) = (a.0.min(b.0), a.0.max(b.0), a.1.min(b.1), a.1.max(b.1));
        for (x, y) in [(x0, y0), (x1 - s, y0), (x0, y1 - s), (x1 - s, y1 - s)] {
            self.cube((x, y), (x + s, y + s), 0.0, h);
        }
    }
}

macro_rules! model {
    ($name:expr, $label:expr, $cat:expr, $mount:expr, $twin:expr, $kw:expr, $plan:expr, $body:expr) => {
        Model { name: $name, label: $label, category: $cat, mount_in: $mount, twin_of: $twin, keywords: $kw, plan: $plan, body: $body }
    };
}

const FURN: &str = "Furniture";
const PLUMB: &str = "Plumbing fixtures";
const OPEN: &str = "Doors and windows";
const LIGHT: &str = "Lighting";
const DEV: &str = "Power and devices";

static MODELS: &[Model] = &[
    // ---------- furniture (free-standing: centred on the insertion point) ----------
    model!("M3_DESK", "Desk 60×30", FURN, 0.0, None, "table office workstation", |p| p.rect((-30.0, -15.0), (30.0, 15.0)), |b| {
        b.cube((-30.0, -15.0), (30.0, 15.0), 28.5, 30.0);
        b.cube((-29.0, -14.0), (-27.0, 14.0), 0.0, 28.5);
        b.cube((27.0, -14.0), (29.0, 14.0), 0.0, 28.5);
        b.cube((-27.0, 12.0), (27.0, 13.0), 10.0, 28.5);
    }),
    model!(
        "M3_CHAIR_TASK",
        "Task chair",
        FURN,
        0.0,
        None,
        "office seat swivel",
        |p| {
            p.circle((0.0, 0.0), 13.0);
            p.rect((-9.0, 8.0), (9.0, 11.0));
        },
        |b| {
            b.cylinder((0.0, 0.0), 12.0, 0.0, 2.0);
            b.cylinder((0.0, 0.0), 1.2, 2.0, 16.0);
            b.cube((-9.5, -9.5), (9.5, 9.5), 16.0, 19.0);
            b.cube((-9.0, 8.0), (9.0, 10.0), 19.0, 36.0);
        }
    ),
    model!("M3_CHAIR", "Side chair", FURN, 0.0, None, "seat guest dining", |p| p.rect((-9.0, -9.0), (9.0, 9.0)), |b| {
        b.legs((-9.0, -9.0), (9.0, 9.0), 1.25, 17.0);
        b.cube((-9.0, -9.0), (9.0, 9.0), 17.0, 18.5);
        b.cube((-9.0, 7.5), (9.0, 9.0), 18.5, 34.0);
    }),
    model!("M3_TABLE_DINING", "Dining table 60×36", FURN, 0.0, None, "table conference", |p| p.rect((-30.0, -18.0), (30.0, 18.0)), |b| {
        b.legs((-28.0, -16.0), (28.0, 16.0), 2.5, 28.5);
        b.cube((-30.0, -18.0), (30.0, 18.0), 28.5, 30.0);
    }),
    model!("M3_TABLE_COFFEE", "Coffee table 42×20", FURN, 0.0, None, "table low lounge", |p| p.rect((-21.0, -10.0), (21.0, 10.0)), |b| {
        b.legs((-20.0, -9.0), (20.0, 9.0), 2.0, 16.0);
        b.cube((-21.0, -10.0), (21.0, 10.0), 16.0, 17.5);
    }),
    model!(
        "M3_SOFA",
        "Sofa 84×36",
        FURN,
        0.0,
        None,
        "couch lounge seat",
        |p| {
            p.rect((-42.0, -18.0), (42.0, 18.0));
            p.line((-36.0, 10.0), (36.0, 10.0));
        },
        |b| {
            b.cube((-42.0, -18.0), (42.0, 18.0), 0.0, 17.0);
            b.cube((-42.0, 10.0), (42.0, 18.0), 17.0, 32.0);
            b.cube((-42.0, -18.0), (-36.0, 10.0), 17.0, 25.0);
            b.cube((36.0, -18.0), (42.0, 10.0), 17.0, 25.0);
        }
    ),
    model!(
        "M3_BED_QUEEN",
        "Queen bed 60×80",
        FURN,
        0.0,
        None,
        "bedroom mattress",
        |p| {
            p.rect((-30.0, -80.0), (30.0, 0.0));
            p.rect((-26.0, -20.0), (-2.0, -4.0));
            p.rect((2.0, -20.0), (26.0, -4.0));
        },
        |b| {
            b.cube((-31.0, -2.0), (31.0, 0.0), 0.0, 44.0);
            b.cube((-30.0, -80.0), (30.0, -2.0), 0.0, 12.0);
            b.cube((-30.0, -80.0), (30.0, -2.0), 12.0, 24.0);
            b.cube((-26.0, -20.0), (-2.0, -4.0), 24.0, 29.0);
            b.cube((2.0, -20.0), (26.0, -4.0), 24.0, 29.0);
        }
    ),
    model!(
        "M3_BED_KING",
        "King bed 76×80",
        FURN,
        0.0,
        None,
        "bedroom mattress",
        |p| {
            p.rect((-38.0, -80.0), (38.0, 0.0));
            p.rect((-34.0, -20.0), (-2.0, -4.0));
            p.rect((2.0, -20.0), (34.0, -4.0));
        },
        |b| {
            b.cube((-39.0, -2.0), (39.0, 0.0), 0.0, 44.0);
            b.cube((-38.0, -80.0), (38.0, -2.0), 0.0, 12.0);
            b.cube((-38.0, -80.0), (38.0, -2.0), 12.0, 24.0);
            b.cube((-34.0, -20.0), (-2.0, -4.0), 24.0, 29.0);
            b.cube((2.0, -20.0), (34.0, -4.0), 24.0, 29.0);
        }
    ),
    model!(
        "M3_CABINET_BASE",
        "Base cabinet 24 with counter",
        FURN,
        0.0,
        None,
        "kitchen casework counter",
        |p| p.rect((-12.0, -25.0), (12.0, 0.0)),
        |b| {
            b.cube((-12.0, -21.0), (12.0, 0.0), 0.0, 4.0);
            b.cube((-12.0, -24.0), (12.0, 0.0), 4.0, 34.5);
            b.cube((-12.0, -25.0), (12.0, 0.0), 34.5, 36.0);
        }
    ),
    model!("M3_CABINET_WALL", "Wall cabinet 30×12", FURN, 54.0, None, "kitchen casework upper", |p| p.rect((-15.0, -12.0), (15.0, 0.0)), |b| {
        b.cube((-15.0, -12.0), (15.0, 0.0), 0.0, 30.0)
    }),
    model!("M3_BOOKCASE", "Bookcase 36×12", FURN, 0.0, None, "shelves shelving", |p| p.rect((-18.0, -12.0), (18.0, 0.0)), |b| {
        b.cube((-18.0, -12.0), (-17.25, 0.0), 0.0, 72.0);
        b.cube((17.25, -12.0), (18.0, 0.0), 0.0, 72.0);
        for z in [0.0, 18.0, 36.0, 54.0, 71.25] {
            b.cube((-17.25, -12.0), (17.25, 0.0), z, z + 0.75);
        }
    }),
    // ---------- plumbing fixtures (against the wall at Y 0) ----------
    model!(
        "M3_WATER_CLOSET",
        "Water closet (tank)",
        PLUMB,
        0.0,
        Some("PL_WATER_CLOSET"),
        "toilet wc",
        |p| {
            p.rect((-10.0, -8.0), (10.0, 0.0));
            p.outline(&[(-7.0, -8.0), (7.0, -8.0), (8.0, -18.0), (5.0, -27.0), (-5.0, -27.0), (-8.0, -18.0)]);
        },
        |b| {
            b.prism(&[(-6.0, -9.0), (6.0, -9.0), (7.0, -18.0), (4.0, -26.0), (-4.0, -26.0), (-7.0, -18.0)], 0.0, 15.0);
            b.prism(&[(-7.0, -8.0), (7.0, -8.0), (8.0, -18.0), (5.0, -27.0), (-5.0, -27.0), (-8.0, -18.0)], 15.0, 16.0);
            b.cube((-10.0, -8.0), (10.0, 0.0), 15.0, 30.0);
        }
    ),
    model!(
        "M3_LAVATORY",
        "Lavatory (wall-hung)",
        PLUMB,
        28.0,
        Some("PL_LAVATORY"),
        "sink basin washbasin",
        |p| {
            p.rect((-10.0, -18.0), (10.0, 0.0));
            p.circle((0.0, -9.0), 6.0);
        },
        |b| {
            b.prism(&[(-10.0, 0.0), (10.0, 0.0), (10.0, -15.0), (6.0, -18.0), (-6.0, -18.0), (-10.0, -15.0)], 0.0, 6.0);
            b.cylinder((0.0, -2.0), 0.6, 6.0, 10.0);
        }
    ),
    model!(
        "M3_SINK_KITCHEN",
        "Kitchen sink in counter",
        PLUMB,
        0.0,
        Some("PL_SINK"),
        "counter sink single bowl",
        |p| {
            p.rect((-15.0, -25.0), (15.0, 0.0));
            p.rect((-12.0, -21.0), (12.0, -5.0));
        },
        |b| {
            b.cube((-15.0, -21.0), (15.0, 0.0), 0.0, 4.0);
            b.cube((-15.0, -24.0), (15.0, 0.0), 4.0, 34.5);
            b.prism(&[(-15.0, 0.0), (15.0, 0.0), (15.0, -25.0), (-15.0, -25.0)], 34.5, 36.0);
            b.cylinder((0.0, -3.0), 0.75, 36.0, 46.0);
        }
    ),
    model!(
        "M3_URINAL",
        "Urinal",
        PLUMB,
        17.0,
        Some("PL_URINAL"),
        "wall hung",
        |p| p.outline(&[(-9.0, 0.0), (9.0, 0.0), (8.0, -12.0), (4.0, -15.0), (-4.0, -15.0), (-8.0, -12.0)]),
        |b| b.prism(&[(-9.0, 0.0), (9.0, 0.0), (8.0, -12.0), (4.0, -15.0), (-4.0, -15.0), (-8.0, -12.0)], 0.0, 24.0)
    ),
    model!(
        "M3_SHOWER",
        "Shower 36×36",
        PLUMB,
        0.0,
        Some("PL_SHOWER"),
        "stall receptor",
        |p| {
            p.rect((-18.0, -36.0), (18.0, 0.0));
            p.circle((0.0, -18.0), 1.5);
            p.line((-18.0, -36.0), (18.0, 0.0));
        },
        |b| {
            b.cube((-18.0, -36.0), (18.0, 0.0), 0.0, 4.0);
            b.cube((-18.0, -2.0), (18.0, 0.0), 4.0, 78.0);
            b.cube((-18.0, -36.0), (-16.0, -2.0), 4.0, 78.0);
            b.cylinder((0.0, -3.0), 1.0, 70.0, 72.0);
        }
    ),
    model!(
        "M3_BATHTUB",
        "Bathtub 60×30",
        PLUMB,
        0.0,
        Some("PL_BATHTUB"),
        "tub bath",
        |p| {
            p.rect((-30.0, -30.0), (30.0, 0.0));
            p.rect((-26.0, -26.0), (26.0, -4.0));
        },
        |b| {
            b.cube((-30.0, -30.0), (30.0, -26.0), 0.0, 15.0);
            b.cube((-30.0, -4.0), (30.0, 0.0), 0.0, 15.0);
            b.cube((-30.0, -26.0), (-26.0, -4.0), 0.0, 15.0);
            b.cube((26.0, -26.0), (30.0, -4.0), 0.0, 15.0);
            b.cube((-26.0, -26.0), (26.0, -4.0), 0.0, 2.0);
        }
    ),
    // ---------- doors and windows (in a wall along X at Y 0; the wall's room face) ----------
    model!(
        "M3_DOOR_36",
        "Door 36×84 with frame",
        OPEN,
        0.0,
        None,
        "leaf swing single",
        |p| {
            p.line((-18.0, 0.0), (-18.0, -36.0));
            p.add(EntityKind::Arc(crate::Arc { center: Vec3::new(-18.0, 0.0, 0.0), radius: 36.0, start: -std::f64::consts::FRAC_PI_2, end: 0.0 }));
        },
        |b| {
            b.cube((-20.0, -1.0), (-18.0, 5.0), 0.0, 86.0);
            b.cube((18.0, -1.0), (20.0, 5.0), 0.0, 86.0);
            b.cube((-20.0, -1.0), (20.0, 5.0), 84.0, 86.0);
            b.cube((-18.0, -36.0), (-16.25, 0.0), 0.0, 84.0);
        }
    ),
    model!(
        "M3_WINDOW_36",
        "Window 36×48",
        OPEN,
        36.0,
        None,
        "glazing opening sill",
        |p| {
            p.rect((-18.0, 0.0), (18.0, 4.0));
            p.line((-18.0, 2.0), (18.0, 2.0));
        },
        |b| {
            b.cube((-18.0, 0.0), (18.0, 4.0), 0.0, 1.5);
            b.cube((-18.0, 0.0), (18.0, 4.0), 46.5, 48.0);
            b.cube((-18.0, 0.0), (-16.5, 4.0), 1.5, 46.5);
            b.cube((16.5, 0.0), (18.0, 4.0), 1.5, 46.5);
            b.cube((-16.5, 1.75), (16.5, 2.25), 1.5, 46.5);
        }
    ),
    // ---------- lighting ----------
    model!(
        "M3_TROFFER_2X4",
        "Troffer 2×4 (lay-in)",
        LIGHT,
        102.0,
        Some("EL_LIGHT_2X4"),
        "fluorescent led fixture ceiling recessed",
        |p| p.rect((-12.0, -24.0), (12.0, 24.0)),
        |b| {
            b.cube((-12.0, -24.0), (12.0, 24.0), 0.0, 0.5);
            b.cube((-10.0, -22.0), (10.0, 22.0), 0.5, 6.0);
        }
    ),
    model!(
        "M3_TROFFER_2X2",
        "Troffer 2×2 (lay-in)",
        LIGHT,
        102.0,
        Some("EL_LIGHT_2X2"),
        "led fixture ceiling recessed",
        |p| p.rect((-12.0, -12.0), (12.0, 12.0)),
        |b| {
            b.cube((-12.0, -12.0), (12.0, 12.0), 0.0, 0.5);
            b.cube((-10.0, -10.0), (10.0, 10.0), 0.5, 5.0);
        }
    ),
    model!("M3_DOWNLIGHT", "Downlight 6\"", LIGHT, 102.0, Some("EL_LIGHT_DOWNLIGHT"), "can recessed pot", |p| p.circle((0.0, 0.0), 3.75), |b| {
        b.cylinder((0.0, 0.0), 3.75, 0.0, 0.5);
        b.cylinder((0.0, 0.0), 3.0, 0.5, 6.0);
    }),
    model!(
        "M3_SCONCE",
        "Wall sconce",
        LIGHT,
        68.0,
        Some("EL_LIGHT_SCONCE"),
        "wall light",
        |p| p.outline(&[(-5.0, 0.0), (5.0, 0.0), (3.0, -4.0), (-3.0, -4.0)]),
        |b| {
            b.cube((-2.5, -0.5), (2.5, 0.0), 0.0, 8.0);
            b.prism(&[(-5.0, -0.5), (5.0, -0.5), (3.0, -4.0), (-3.0, -4.0)], 2.0, 10.0);
        }
    ),
    model!("M3_STRIP", "Strip light 4'", LIGHT, 99.0, Some("EL_LIGHT_STRIP"), "linear surface", |p| p.rect((-2.5, -24.0), (2.5, 24.0)), |b| {
        b.cube((-2.5, -24.0), (2.5, 24.0), 0.0, 3.0)
    }),
    model!("M3_EXIT", "Exit sign", LIGHT, 84.0, Some("EL_LIGHT_EXIT"), "egress emergency", |p| p.rect((-6.0, -1.5), (6.0, 0.0)), |b| b.cube(
        (-6.0, -1.5),
        (6.0, 0.0),
        0.0,
        8.0
    )),
    // ---------- power and devices (cover plates on the wall at Y 0) ----------
    model!(
        "M3_RECEPTACLE",
        "Duplex receptacle",
        DEV,
        15.75,
        Some("EL_RECEPTACLE_DUPLEX"),
        "outlet plug",
        |p| p.rect((-1.4, -0.3), (1.4, 0.0)),
        |b| plate(b, 2.75, 4.5)
    ),
    model!(
        "M3_RECEPTACLE_GFCI",
        "GFCI receptacle",
        DEV,
        15.75,
        Some("EL_RECEPTACLE_GFCI"),
        "outlet ground fault",
        |p| p.rect((-1.4, -0.3), (1.4, 0.0)),
        |b| plate(b, 2.75, 4.5)
    ),
    model!(
        "M3_RECEPTACLE_QUAD",
        "Quad receptacle",
        DEV,
        15.75,
        Some("EL_RECEPTACLE_QUAD"),
        "outlet fourplex",
        |p| p.rect((-2.3, -0.3), (2.3, 0.0)),
        |b| plate(b, 4.5, 4.5)
    ),
    model!(
        "M3_RECEPTACLE_WP",
        "Weatherproof receptacle",
        DEV,
        15.75,
        Some("EL_RECEPTACLE_WP"),
        "outlet exterior",
        |p| p.rect((-2.0, -1.5), (2.0, 0.0)),
        |b| b.cube((-2.0, -1.5), (2.0, 0.0), 0.0, 5.0)
    ),
    model!("M3_SWITCH", "Switch", DEV, 45.75, Some("EL_SWITCH"), "light toggle", |p| p.rect((-1.4, -0.3), (1.4, 0.0)), |b| plate(b, 2.75, 4.5)),
    model!("M3_SWITCH_3WAY", "3-way switch", DEV, 45.75, Some("EL_SWITCH_3WAY"), "light toggle three", |p| p.rect((-1.4, -0.3), (1.4, 0.0)), |b| {
        plate(b, 2.75, 4.5)
    }),
    model!("M3_SWITCH_DIMMER", "Dimmer switch", DEV, 45.75, Some("EL_SWITCH_DIMMER"), "light dimmer", |p| p.rect((-1.4, -0.3), (1.4, 0.0)), |b| {
        plate(b, 2.75, 4.5)
    }),
    model!("M3_DATA", "Data outlet", DEV, 15.75, Some("EL_DATA"), "network jack", |p| p.rect((-1.4, -0.3), (1.4, 0.0)), |b| plate(b, 2.75, 4.5)),
    model!(
        "M3_PANEL",
        "Panelboard 20×6×48",
        DEV,
        30.0,
        Some("EL_PANEL"),
        "electrical panel distribution",
        |p| p.rect((-10.0, -6.0), (10.0, 0.0)),
        |b| b.cube((-10.0, -6.0), (10.0, 0.0), 0.0, 48.0)
    ),
    model!("M3_FA_PULL", "Fire alarm pull station", DEV, 45.75, Some("EL_FA_PULL"), "manual station", |p| p.rect((-2.5, -1.5), (2.5, 0.0)), |b| b
        .cube((-2.5, -1.5), (2.5, 0.0), 0.0, 5.5)),
    model!(
        "M3_FA_HORN_STROBE",
        "Horn/strobe",
        DEV,
        80.0,
        Some("EL_FA_HORN_STROBE"),
        "notification appliance",
        |p| p.rect((-2.5, -2.0), (2.5, 0.0)),
        |b| b.cube((-2.5, -2.0), (2.5, 0.0), 0.0, 5.0)
    ),
];

/// A cover plate `w` wide and `h` tall on the wall (its bottom at the mounting height: a 4½"
/// plate centred 18" above the floor mounts at 15¾").
fn plate(b: &mut Body, w: f64, h: f64) {
    b.cube((-w / 2.0, -0.3), (w / 2.0, 0.0), 0.0, h);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_twins_exist_in_the_2d_library() {
        let mut names: Vec<&str> = models().iter().map(|m| m.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), models().len());
        for m in models() {
            if let Some(t) = m.twin_of {
                assert!(super::super::symbols::symbol(t).is_some(), "{} names a 2D symbol {t} that doesn't exist", m.name);
            }
        }
    }
}
