//! IFC export validity (Q208): a made-up two-storey building (walls with a door and a window,
//! slabs, ceilings, a column, a curtain wall, a stair with its railings, rooms, and duct, pipe and
//! conduit runs) exported to IFC4 and checked against the IFC4 schema's rules by our own code:
//! every entity and attribute as the schema declares it, references to the right types, GlobalIds
//! well formed and unique, the spatial structure whole, every element placed and contained once.
//! GlobalIds stay the same on re-export (and through IFC → drawing → IFC), and the quantities match
//! the drawing.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Mesh};
use serde_json::json;

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// Feet → inches.
fn ft(p: [f64; 2]) -> [f64; 2] {
    [p[0] * 12.0, p[1] * 12.0]
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [ft(a), ft([b[0], a[1]]), ft(b), ft([a[0], b[1]])], "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [ft(a), ft(b)]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": ft(at), "text": t, "height": 6})).unwrap();
}

/// A block `name` of one line `width` inches long (a door or window symbol) inserted at `at`
/// (feet) on `layer`.
fn opening(s: &mut Session, name: &str, width: f64, at: [f64; 2], on: &str) {
    if !s.doc().unwrap().blocks.contains_key(name) {
        layer(s, "0");
        s.execute("line", &json!({"points": [[-width / 2.0, 0], [width / 2.0, 0]]})).unwrap();
        let h = s.doc().unwrap().model.iter().last().unwrap().handle.hex();
        s.execute("block", &json!({"name": name, "base": [0, 0], "handles": [h], "keep": "delete"})).unwrap();
    }
    layer(s, on);
    s.execute("insert", &json!({"name": name, "at": ft(at)})).unwrap();
}

/// "Sample Building": 40' × 24' of 6" walls on two storeys 12' apart (L2's plan drawn 200' east).
/// L1: OFFICE and LOBBY split by a 4⅞" partition, a 36" door and a 48" window in the south wall,
/// a column, a curtain wall inside the north wall, a straight stair of 18 treads, and a duct,
/// a pipe and a conduit run under the ceiling. L2: one room, STUDIO.
fn building() -> Session {
    let mut s = Session::new();
    for (x0, rooms) in [(0.0, vec![("OFFICE", [8.0, 12.0]), ("LOBBY", [28.0, 12.0])]), (200.0, vec![("STUDIO", [220.0, 12.0])])] {
        layer(&mut s, "A-WALL");
        rect(&mut s, [x0, 0.0], [x0 + 40.0, 24.0]);
        rect(&mut s, [x0 + 0.5, 0.5], [x0 + 39.5, 23.5]);
        layer(&mut s, "A-ROOM-TAG");
        for (name, at) in rooms {
            text(&mut s, at, name);
        }
    }
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [16.0, 0.5], [16.0, 23.5]);
    opening(&mut s, "D36", 36.0, [6.0, 0.25], "A-DOOR");
    opening(&mut s, "W48", 48.0, [26.0, 0.25], "A-GLAZ");
    layer(&mut s, "S-COLS");
    s.execute("circle", &json!({"center": ft([24.0, 18.0]), "radius": 9.0})).unwrap();
    layer(&mut s, "A-GLAZ-CURT");
    line(&mut s, [20.0, 23.0], [36.0, 23.0]);
    // The stair: 18 treads 11" deep, 44" wide, climbing east from (2', 14').
    layer(&mut s, "A-FLOR-STRS");
    let (x0, y0) = (24.0, 168.0);
    for i in 0..=18 {
        let x = x0 + 11.0 * f64::from(i);
        s.execute("line", &json!({"points": [[x, y0], [x, y0 + 44.0]]})).unwrap();
    }
    s.execute("line", &json!({"points": [[x0, y0], [x0 + 198.0, y0]]})).unwrap();
    s.execute("line", &json!({"points": [[x0, y0 + 44.0], [x0 + 198.0, y0 + 44.0]]})).unwrap();
    s.execute("text", &json!({"at": [x0 - 20.0, y0 + 20.0], "text": "UP", "height": 6})).unwrap();
    // A supply duct, a cold-water pipe and a conduit under the 9' ceiling.
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [ft([2.0, 4.0]), ft([30.0, 4.0]), ft([30.0, 10.0])]})).unwrap();
    s.execute("text", &json!({"at": ft([20.0, 4.5]), "text": "14x8 BOT @ 9'-6\"", "height": 4})).unwrap();
    layer(&mut s, "P-DOMW-CW");
    line(&mut s, [4.0, 2.0], [36.0, 2.0]);
    s.execute("text", &json!({"at": ft([10.0, 2.3]), "text": "1\" CW CL @ 10'-6\"", "height": 4})).unwrap();
    layer(&mut s, "E-COND");
    line(&mut s, [4.0, 20.0], [36.0, 20.0]);
    s.execute("text", &json!({"at": ft([10.0, 20.3]), "text": "3/4\" C BOT @ 11'-0\"", "height": 4})).unwrap();
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": [-120, -120, 1200, 1200], "base": [0, 0]},
            {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "region": [2280, -120, 3000, 1200], "base": [2400, 0]}
        ],
        "layers": [
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-DOOR", "role": "door"},
            {"layer": "A-GLAZ", "role": "window"},
            {"layer": "A-GLAZ-CURT", "role": "curtain_wall"},
            {"layer": "S-COLS", "role": "column"},
            {"layer": "A-FLOR-STRS", "role": "stair"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6, "from": "pairs"},
            {"name": "Partition", "height_ft": 9, "thickness_in": 4.875, "from": "centerline"}
        ],
        "families": [
            {"name": "Door 3'", "kind": "door", "block": "D*", "width_in": 36, "head_ft": 7},
            {"name": "Window 4'", "kind": "window", "block": "W*", "width_in": 48, "sill_ft": 3, "head_ft": 7}
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("stairs.detect", &json!({"store": true})).unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s.execute("rooms.build", &json!({})).unwrap();
    s
}

/// What the building is made of: WALLGEN part → count.
fn parts(s: &Session) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for e in s.doc().unwrap().model.iter() {
        if let Some(p) = made_as(e) {
            *m.entry(p).or_default() += 1;
        }
    }
    m
}

/// The building exported as IFC, as text (kept at `$IFCTEST_KEEP/<name>` when that is set, for
/// the viewer and validator runs outside CI).
fn export(s: &mut Session, name: &str) -> String {
    // A folder per export: tests run at once, and several export the same name (an export
    // writes beside its file and renames it, so two at one path clash).
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("cadaddy-ifcvalid-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    s.execute("export", &json!({ "path": path })).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    if let Some(keep) = std::env::var_os("IFCTEST_KEEP") {
        let keep = std::path::PathBuf::from(keep);
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join(name), &text).unwrap();
    }
    let _ = std::fs::remove_file(&path);
    text
}

// ---------------- reading the file (our own STEP reader, not the app's) ----------------

#[derive(Clone, Debug, PartialEq)]
enum P {
    Ref(u64),
    Str(String),
    Real(f64),
    Int(i64),
    Enum(String),
    List(Vec<P>),
    Null,
    Derived,
}

struct Lex<'a> {
    b: &'a [u8],
    at: usize,
}

impl Lex<'_> {
    fn ws(&mut self) {
        while self.b.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }
    fn param(&mut self) -> Result<P, String> {
        self.ws();
        let c = *self.b.get(self.at).ok_or("cut short")?;
        let start = self.at;
        self.at += 1;
        Ok(match c {
            b'#' => {
                while self.b.get(self.at).is_some_and(u8::is_ascii_digit) {
                    self.at += 1;
                }
                P::Ref(std::str::from_utf8(&self.b[start + 1..self.at]).unwrap().parse().map_err(|_| "bad reference")?)
            }
            b'\'' => {
                let mut v = Vec::new();
                loop {
                    let c = *self.b.get(self.at).ok_or("unterminated string")?;
                    self.at += 1;
                    if c == b'\'' {
                        if self.b.get(self.at) == Some(&b'\'') {
                            self.at += 1;
                        } else {
                            break;
                        }
                    }
                    if !(0x20..0x7f).contains(&c) {
                        return Err(format!("a string holds byte {c:#x}: STEP strings are printable ASCII"));
                    }
                    v.push(c);
                }
                P::Str(String::from_utf8(v).unwrap())
            }
            b'.' => {
                while self.b.get(self.at).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
                    self.at += 1;
                }
                if self.b.get(self.at) != Some(&b'.') {
                    return Err("an enumeration without its closing dot".into());
                }
                self.at += 1;
                P::Enum(String::from_utf8(self.b[start + 1..self.at - 1].to_vec()).unwrap())
            }
            b'$' => P::Null,
            b'*' => P::Derived,
            b'(' => {
                let mut v = Vec::new();
                loop {
                    self.ws();
                    match self.b.get(self.at) {
                        Some(b')') => {
                            self.at += 1;
                            break;
                        }
                        Some(b',') => self.at += 1,
                        _ => v.push(self.param()?),
                    }
                }
                P::List(v)
            }
            c if c == b'-' || c.is_ascii_digit() => {
                while self.b.get(self.at).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b'E' | b'-' | b'+')) {
                    self.at += 1;
                }
                let t = std::str::from_utf8(&self.b[start..self.at]).unwrap();
                if t.contains('.') {
                    // ISO 10303-21: a real has a decimal point; digits after it and the exponent are optional.
                    let ok = t.split_once('.').is_some_and(|(i, rest)| {
                        !i.trim_start_matches('-').is_empty()
                            && i.trim_start_matches('-').bytes().all(|b| b.is_ascii_digit())
                            && rest.split_once('E').map_or(rest.bytes().all(|b| b.is_ascii_digit()), |(f, e)| {
                                f.bytes().all(|b| b.is_ascii_digit()) && e.trim_start_matches(['-', '+']).bytes().all(|b| b.is_ascii_digit())
                            })
                    });
                    if !ok {
                        return Err(format!("`{t}` is not a STEP real"));
                    }
                    P::Real(t.replace("E", "e").parse::<f64>().map_err(|_| format!("`{t}`"))?)
                } else {
                    P::Int(t.parse().map_err(|_| format!("`{t}` is not an integer"))?)
                }
            }
            _ => return Err(format!("unexpected `{}`", c as char)),
        })
    }
}

/// The DATA section: number → (entity, parameters); the header's instances by name.
struct File {
    data: BTreeMap<u64, (String, Vec<P>)>,
    header: BTreeMap<String, Vec<P>>,
}

fn read_step(text: &str) -> Result<File, String> {
    let body = text.strip_prefix("ISO-10303-21;").ok_or("not ISO-10303-21")?;
    let body = body.trim_end().strip_suffix("END-ISO-10303-21;").ok_or("no END-ISO-10303-21")?;
    let (header, data) = body.split_once("DATA;").ok_or("no DATA section")?;
    let header = header.trim().strip_prefix("HEADER;").ok_or("no HEADER section")?;
    let header = header.trim().strip_suffix("ENDSEC;").ok_or("HEADER not closed")?;
    let data = data.trim().strip_suffix("ENDSEC;").ok_or("DATA not closed")?;
    let mut file = File { data: BTreeMap::new(), header: BTreeMap::new() };
    let instance = |t: &str| -> Result<(String, Vec<P>), String> {
        let t = t.trim();
        let open = t.find('(').ok_or_else(|| format!("no parameters: {t}"))?;
        let name = t[..open].trim().to_string();
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') {
            return Err(format!("bad entity name `{name}`"));
        }
        let mut lx = Lex { b: t.as_bytes(), at: open };
        let P::List(params) = lx.param()? else { unreachable!() };
        lx.ws();
        if lx.at != t.len() {
            return Err(format!("text after the parameters: {t}"));
        }
        Ok((name, params))
    };
    for part in split_instances(header) {
        let (n, p) = instance(&part)?;
        file.header.insert(n, p);
    }
    for part in split_instances(data) {
        let (num, rest) = part.trim().split_once('=').ok_or_else(|| format!("not an instance: {part}"))?;
        let num: u64 = num.trim().strip_prefix('#').and_then(|n| n.parse().ok()).ok_or_else(|| format!("bad instance name {num}"))?;
        if file.data.insert(num, instance(rest)?).is_some() {
            return Err(format!("#{num} defined twice"));
        }
    }
    Ok(file)
}

/// The text between `;`s outside strings.
fn split_instances(t: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in t.chars() {
        if c == '\'' {
            quoted = !quoted;
        }
        if c == ';' && !quoted {
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    out
}

// ---------------- the IFC4 schema, as far as the export uses it ----------------

/// One attribute as the schema declares it.
#[derive(Clone, Copy)]
enum A {
    Guid,
    Text { opt: bool },
    Real { opt: bool },
    Int,
    Bool { opt: bool },
    Ref { opt: bool, to: &'static [&'static str] },
    Refs { opt: bool, min: usize, to: &'static [&'static str] },
    Enum { opt: bool, of: &'static [&'static str] },
    Reals { min: usize, max: usize },
    Points3,
    Index,
    Derived,
}

const ELEMENTS: &[&str] = &[
    "IFCWALL",
    "IFCSLAB",
    "IFCCOVERING",
    "IFCDOOR",
    "IFCWINDOW",
    "IFCSTAIR",
    "IFCBUILDINGELEMENTPROXY",
    "IFCCOLUMN",
    "IFCCURTAINWALL",
    "IFCRAILING",
    "IFCRAMP",
    "IFCROOF",
    "IFCBEAM",
    "IFCGEOGRAPHICELEMENT",
    "IFCDUCTSEGMENT",
    "IFCPIPESEGMENT",
    "IFCCABLECARRIERSEGMENT",
];
const SPATIAL: &[&str] = &["IFCSITE", "IFCBUILDING", "IFCBUILDINGSTOREY", "IFCSPACE"];
const OBJECTS: &[&str] = &[
    "IFCPROJECT",
    "IFCSITE",
    "IFCBUILDING",
    "IFCBUILDINGSTOREY",
    "IFCSPACE",
    "IFCWALL",
    "IFCSLAB",
    "IFCCOVERING",
    "IFCDOOR",
    "IFCWINDOW",
    "IFCSTAIR",
    "IFCBUILDINGELEMENTPROXY",
    "IFCCOLUMN",
    "IFCCURTAINWALL",
    "IFCRAILING",
    "IFCRAMP",
    "IFCROOF",
    "IFCBEAM",
    "IFCGEOGRAPHICELEMENT",
    "IFCDUCTSEGMENT",
    "IFCPIPESEGMENT",
    "IFCCABLECARRIERSEGMENT",
];
const PLACEMENT: &[&str] = &["IFCLOCALPLACEMENT"];
const QUANTITIES: &[&str] = &["IFCQUANTITYLENGTH", "IFCQUANTITYAREA", "IFCQUANTITYVOLUME"];

/// IfcRoot's attributes then IfcObject's ObjectType.
const ROOT: [A; 4] = [A::Guid, A::Ref { opt: true, to: &["IFCOWNERHISTORY"] }, A::Text { opt: true }, A::Text { opt: true }];

fn product() -> Vec<A> {
    let mut v = ROOT.to_vec();
    v.extend([A::Text { opt: true }, A::Ref { opt: true, to: PLACEMENT }, A::Ref { opt: true, to: &["IFCPRODUCTDEFINITIONSHAPE"] }]);
    v
}

/// IfcElement and a PredefinedType of `of`.
fn element(of: &'static [&'static str]) -> Vec<A> {
    let mut v = product();
    v.extend([A::Text { opt: true }, A::Enum { opt: true, of }]);
    v
}

fn spatial() -> Vec<A> {
    let mut v = product();
    v.extend([A::Text { opt: true }, A::Enum { opt: true, of: &["COMPLEX", "ELEMENT", "PARTIAL"] }]);
    v
}

fn schema() -> HashMap<&'static str, Vec<A>> {
    let mut m: HashMap<&'static str, Vec<A>> = HashMap::new();
    let ud = |v: &'static [&'static str]| v;
    m.insert(
        "IFCWALL",
        element(ud(&[
            "MOVABLE",
            "PARAPET",
            "PARTITIONING",
            "PLUMBINGWALL",
            "SHEAR",
            "SOLIDWALL",
            "STANDARD",
            "POLYGONAL",
            "ELEMENTEDWALL",
            "USERDEFINED",
            "NOTDEFINED",
        ])),
    );
    m.insert("IFCSLAB", element(&["FLOOR", "ROOF", "LANDING", "BASESLAB", "USERDEFINED", "NOTDEFINED"]));
    m.insert(
        "IFCCOVERING",
        element(&[
            "CEILING",
            "FLOORING",
            "CLADDING",
            "ROOFING",
            "MOLDING",
            "SKIRTINGBOARD",
            "INSULATION",
            "MEMBRANE",
            "SLEEVING",
            "WRAPPING",
            "USERDEFINED",
            "NOTDEFINED",
        ]),
    );
    m.insert(
        "IFCSTAIR",
        element(&[
            "STRAIGHT_RUN_STAIR",
            "TWO_STRAIGHT_RUN_STAIR",
            "QUARTER_WINDING_STAIR",
            "QUARTER_TURN_STAIR",
            "HALF_WINDING_STAIR",
            "HALF_TURN_STAIR",
            "TWO_QUARTER_WINDING_STAIR",
            "TWO_QUARTER_TURN_STAIR",
            "THREE_QUARTER_WINDING_STAIR",
            "THREE_QUARTER_TURN_STAIR",
            "SPIRAL_STAIR",
            "DOUBLE_RETURN_STAIR",
            "CURVED_RUN_STAIR",
            "TWO_CURVED_RUN_STAIR",
            "USERDEFINED",
            "NOTDEFINED",
        ]),
    );
    m.insert(
        "IFCBUILDINGELEMENTPROXY",
        element(&["COMPLEX", "ELEMENT", "PARTIAL", "PROVISIONFORVOID", "PROVISIONFORSPACE", "USERDEFINED", "NOTDEFINED"]),
    );
    m.insert("IFCCOLUMN", element(&["COLUMN", "PILASTER", "USERDEFINED", "NOTDEFINED"]));
    m.insert("IFCCURTAINWALL", element(&["USERDEFINED", "NOTDEFINED"]));
    m.insert("IFCRAILING", element(&["HANDRAIL", "GUARDRAIL", "BALUSTRADE", "USERDEFINED", "NOTDEFINED"]));
    m.insert(
        "IFCRAMP",
        element(&[
            "STRAIGHT_RUN_RAMP",
            "TWO_STRAIGHT_RUN_RAMP",
            "QUARTER_TURN_RAMP",
            "TWO_QUARTER_TURN_RAMP",
            "HALF_TURN_RAMP",
            "SPIRAL_RAMP",
            "USERDEFINED",
            "NOTDEFINED",
        ]),
    );
    m.insert(
        "IFCROOF",
        element(&[
            "FLAT_ROOF",
            "SHED_ROOF",
            "GABLE_ROOF",
            "HIP_ROOF",
            "HIPPED_GABLE_ROOF",
            "GAMBREL_ROOF",
            "MANSARD_ROOF",
            "BARREL_ROOF",
            "RAINBOW_ROOF",
            "BUTTERFLY_ROOF",
            "PAVILION_ROOF",
            "DOME_ROOF",
            "FREEFORM",
            "USERDEFINED",
            "NOTDEFINED",
        ]),
    );
    m.insert("IFCBEAM", element(&["BEAM", "JOIST", "HOLLOWCORE", "LINTEL", "SPANDREL", "T_BEAM", "USERDEFINED", "NOTDEFINED"]));
    m.insert("IFCGEOGRAPHICELEMENT", element(&["TERRAIN", "USERDEFINED", "NOTDEFINED"]));
    m.insert("IFCDUCTSEGMENT", element(&["RIGIDSEGMENT", "FLEXIBLESEGMENT", "USERDEFINED", "NOTDEFINED"]));
    m.insert("IFCPIPESEGMENT", element(&["CULVERT", "FLEXIBLESEGMENT", "RIGIDSEGMENT", "GUTTER", "SPOOL", "USERDEFINED", "NOTDEFINED"]));
    m.insert(
        "IFCCABLECARRIERSEGMENT",
        element(&["CABLELADDERSEGMENT", "CABLETRAYSEGMENT", "CABLETRUNKINGSEGMENT", "CONDUITSEGMENT", "USERDEFINED", "NOTDEFINED"]),
    );
    for (door, of, part) in [
        ("IFCDOOR", &["DOOR", "GATE", "TRAPDOOR", "USERDEFINED", "NOTDEFINED"][..], "door"),
        ("IFCWINDOW", &["WINDOW", "SKYLIGHT", "LIGHTDOME", "USERDEFINED", "NOTDEFINED"][..], "window"),
    ] {
        let mut v = product();
        v.extend([A::Text { opt: true }, A::Real { opt: true }, A::Real { opt: true }, A::Enum { opt: true, of }]);
        let ops: &'static [&'static str] = if part == "door" {
            &[
                "SINGLE_SWING_LEFT",
                "SINGLE_SWING_RIGHT",
                "DOUBLE_DOOR_SINGLE_SWING",
                "DOUBLE_DOOR_SINGLE_SWING_OPPOSITE_LEFT",
                "DOUBLE_DOOR_SINGLE_SWING_OPPOSITE_RIGHT",
                "DOUBLE_SWING_LEFT",
                "DOUBLE_SWING_RIGHT",
                "DOUBLE_DOOR_DOUBLE_SWING",
                "SLIDING_TO_LEFT",
                "SLIDING_TO_RIGHT",
                "DOUBLE_DOOR_SLIDING",
                "FOLDING_TO_LEFT",
                "FOLDING_TO_RIGHT",
                "DOUBLE_DOOR_FOLDING",
                "REVOLVING",
                "ROLLINGUP",
                "SWING_FIXED_LEFT",
                "SWING_FIXED_RIGHT",
                "USERDEFINED",
                "NOTDEFINED",
            ]
        } else {
            &[
                "SINGLE_PANEL",
                "DOUBLE_PANEL_VERTICAL",
                "DOUBLE_PANEL_HORIZONTAL",
                "TRIPLE_PANEL_VERTICAL",
                "TRIPLE_PANEL_BOTTOM",
                "TRIPLE_PANEL_TOP",
                "TRIPLE_PANEL_LEFT",
                "TRIPLE_PANEL_RIGHT",
                "TRIPLE_PANEL_HORIZONTAL",
                "USERDEFINED",
                "NOTDEFINED",
            ]
        };
        v.extend([A::Enum { opt: true, of: ops }, A::Text { opt: true }]);
        m.insert(door, v);
    }
    let mut project = ROOT.to_vec();
    project.extend([
        A::Text { opt: true },
        A::Text { opt: true },
        A::Text { opt: true },
        A::Refs { opt: true, min: 1, to: &["IFCGEOMETRICREPRESENTATIONCONTEXT"] },
        A::Ref { opt: true, to: &["IFCUNITASSIGNMENT"] },
    ]);
    m.insert("IFCPROJECT", project);
    let mut site = spatial();
    site.extend([
        A::Reals { min: 0, max: 0 },
        A::Reals { min: 0, max: 0 },
        A::Real { opt: true },
        A::Text { opt: true },
        A::Ref { opt: true, to: &["IFCPOSTALADDRESS"] },
    ]);
    m.insert("IFCSITE", site);
    let mut building = spatial();
    building.extend([A::Real { opt: true }, A::Real { opt: true }, A::Ref { opt: true, to: &["IFCPOSTALADDRESS"] }]);
    m.insert("IFCBUILDING", building);
    let mut storey = spatial();
    storey.push(A::Real { opt: true });
    m.insert("IFCBUILDINGSTOREY", storey);
    let mut space = spatial();
    space.extend([
        A::Enum { opt: true, of: &["SPACE", "PARKING", "GFA", "INTERNAL", "EXTERNAL", "USERDEFINED", "NOTDEFINED"] },
        A::Real { opt: true },
    ]);
    m.insert("IFCSPACE", space);
    let mut agg = ROOT.to_vec();
    agg.extend([A::Ref { opt: false, to: OBJECTS }, A::Refs { opt: false, min: 1, to: OBJECTS }]);
    m.insert("IFCRELAGGREGATES", agg);
    let mut contained = ROOT.to_vec();
    contained.extend([A::Refs { opt: false, min: 1, to: ELEMENTS }, A::Ref { opt: false, to: SPATIAL }]);
    m.insert("IFCRELCONTAINEDINSPATIALSTRUCTURE", contained);
    let mut defines = ROOT.to_vec();
    defines.extend([A::Refs { opt: false, min: 1, to: OBJECTS }, A::Ref { opt: false, to: &["IFCELEMENTQUANTITY"] }]);
    m.insert("IFCRELDEFINESBYPROPERTIES", defines);
    m.insert("IFCOPENINGELEMENT", element(&["OPENING", "RECESS", "USERDEFINED", "NOTDEFINED"]));
    let mut voids = ROOT.to_vec();
    voids.extend([A::Ref { opt: false, to: ELEMENTS }, A::Ref { opt: false, to: &["IFCOPENINGELEMENT"] }]);
    m.insert("IFCRELVOIDSELEMENT", voids);
    let mut fills = ROOT.to_vec();
    fills.extend([A::Ref { opt: false, to: &["IFCOPENINGELEMENT"] }, A::Ref { opt: false, to: ELEMENTS }]);
    m.insert("IFCRELFILLSELEMENT", fills);
    let mut eq = ROOT.to_vec();
    eq.extend([A::Text { opt: true }, A::Refs { opt: false, min: 1, to: QUANTITIES }]);
    m.insert("IFCELEMENTQUANTITY", eq);
    for q in QUANTITIES {
        m.insert(
            q,
            vec![
                A::Text { opt: false },
                A::Text { opt: true },
                A::Ref { opt: true, to: &["IFCSIUNIT"] },
                A::Real { opt: false },
                A::Text { opt: true },
            ],
        );
    }
    m.insert(
        "IFCSIUNIT",
        vec![
            A::Derived,
            A::Enum { opt: false, of: &["LENGTHUNIT", "AREAUNIT", "VOLUMEUNIT", "PLANEANGLEUNIT"] },
            A::Enum {
                opt: true,
                of: &[
                    "EXA", "PETA", "TERA", "GIGA", "MEGA", "KILO", "HECTO", "DECA", "DECI", "CENTI", "MILLI", "MICRO", "NANO", "PICO", "FEMTO",
                    "ATTO",
                ],
            },
            A::Enum { opt: false, of: &["METRE", "SQUARE_METRE", "CUBIC_METRE", "RADIAN"] },
        ],
    );
    m.insert("IFCUNITASSIGNMENT", vec![A::Refs { opt: false, min: 1, to: &["IFCSIUNIT"] }]);
    m.insert("IFCCARTESIANPOINT", vec![A::Reals { min: 2, max: 3 }]);
    m.insert("IFCDIRECTION", vec![A::Reals { min: 2, max: 3 }]);
    m.insert(
        "IFCAXIS2PLACEMENT3D",
        vec![
            A::Ref { opt: false, to: &["IFCCARTESIANPOINT"] },
            A::Ref { opt: true, to: &["IFCDIRECTION"] },
            A::Ref { opt: true, to: &["IFCDIRECTION"] },
        ],
    );
    m.insert("IFCLOCALPLACEMENT", vec![A::Ref { opt: true, to: PLACEMENT }, A::Ref { opt: false, to: &["IFCAXIS2PLACEMENT3D"] }]);
    m.insert(
        "IFCGEOMETRICREPRESENTATIONCONTEXT",
        vec![
            A::Text { opt: true },
            A::Text { opt: true },
            A::Int,
            A::Real { opt: true },
            A::Ref { opt: false, to: &["IFCAXIS2PLACEMENT3D"] },
            A::Ref { opt: true, to: &["IFCDIRECTION"] },
        ],
    );
    m.insert(
        "IFCGEOMETRICREPRESENTATIONSUBCONTEXT",
        vec![
            A::Text { opt: true },
            A::Text { opt: true },
            A::Derived,
            A::Derived,
            A::Derived,
            A::Derived,
            A::Ref { opt: false, to: &["IFCGEOMETRICREPRESENTATIONCONTEXT"] },
            A::Real { opt: true },
            A::Enum {
                opt: false,
                of: &[
                    "GRAPH_VIEW",
                    "SKETCH_VIEW",
                    "MODEL_VIEW",
                    "PLAN_VIEW",
                    "REFLECTED_PLAN_VIEW",
                    "SECTION_VIEW",
                    "ELEVATION_VIEW",
                    "USERDEFINED",
                    "NOTDEFINED",
                ],
            },
            A::Text { opt: true },
        ],
    );
    m.insert(
        "IFCSHAPEREPRESENTATION",
        vec![
            A::Ref { opt: false, to: &["IFCGEOMETRICREPRESENTATIONSUBCONTEXT", "IFCGEOMETRICREPRESENTATIONCONTEXT"] },
            A::Text { opt: true },
            A::Text { opt: true },
            A::Refs { opt: false, min: 1, to: &["IFCPOLYGONALFACESET"] },
        ],
    );
    m.insert(
        "IFCPRODUCTDEFINITIONSHAPE",
        vec![A::Text { opt: true }, A::Text { opt: true }, A::Refs { opt: false, min: 1, to: &["IFCSHAPEREPRESENTATION"] }],
    );
    m.insert("IFCCARTESIANPOINTLIST3D", vec![A::Points3]);
    m.insert(
        "IFCPOLYGONALFACESET",
        vec![
            A::Ref { opt: false, to: &["IFCCARTESIANPOINTLIST3D"] },
            A::Bool { opt: true },
            A::Refs { opt: false, min: 1, to: &["IFCINDEXEDPOLYGONALFACE"] },
            A::Refs { opt: true, min: 1, to: &[] },
        ],
    );
    m.insert("IFCINDEXEDPOLYGONALFACE", vec![A::Index]);
    m
}

/// Whether `p` is what `a` declares; `types` names each instance's entity.
fn matches(p: &P, a: A, types: &BTreeMap<u64, (String, Vec<P>)>) -> Result<(), String> {
    let is_type = |r: u64, to: &[&str]| match types.get(&r) {
        None => Err(format!("#{r} isn't in the file")),
        Some((t, _)) if to.contains(&t.as_str()) => Ok(()),
        Some((t, _)) => Err(format!("#{r} is {t}, not one of {to:?}")),
    };
    match (a, p) {
        (
            A::Text { opt: true }
            | A::Real { opt: true }
            | A::Bool { opt: true }
            | A::Ref { opt: true, .. }
            | A::Refs { opt: true, .. }
            | A::Enum { opt: true, .. },
            P::Null,
        ) => Ok(()),
        (A::Reals { min: 0, .. }, P::Null) => Ok(()),
        (A::Guid, P::Str(g)) => {
            let ok =
                g.len() == 22 && g.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$') && (b'0'..=b'3').contains(&g.as_bytes()[0]);
            if ok { Ok(()) } else { Err(format!("'{g}' isn't a GlobalId")) }
        }
        (A::Text { .. }, P::Str(_)) => Ok(()),
        (A::Real { .. }, P::Real(v)) if v.is_finite() => Ok(()),
        (A::Int, P::Int(_)) => Ok(()),
        (A::Bool { .. }, P::Enum(e)) if ["T", "F", "U"].contains(&e.as_str()) => Ok(()),
        (A::Enum { of, .. }, P::Enum(e)) => {
            if of.contains(&e.as_str()) {
                Ok(())
            } else {
                Err(format!(".{e}. isn't one of {of:?}"))
            }
        }
        (A::Ref { to, .. }, P::Ref(r)) => is_type(*r, to),
        (A::Refs { min, to, .. }, P::List(v)) => {
            if v.len() < min {
                return Err(format!("{} items, at least {min}", v.len()));
            }
            let mut seen = BTreeSet::new();
            for x in v {
                let P::Ref(r) = x else { return Err(format!("{x:?} in a list of references")) };
                if !to.is_empty() {
                    is_type(*r, to)?;
                }
                if !seen.insert(*r) {
                    return Err(format!("#{r} twice in a SET"));
                }
            }
            Ok(())
        }
        (A::Reals { min, max }, P::List(v)) if (min..=max).contains(&v.len()) && v.iter().all(|x| matches!(x, P::Real(r) if r.is_finite())) => Ok(()),
        (A::Points3, P::List(v)) if !v.is_empty() => {
            for pt in v {
                match pt {
                    P::List(c) if c.len() == 3 && c.iter().all(|x| matches!(x, P::Real(r) if r.is_finite())) => {}
                    _ => return Err(format!("{pt:?} isn't a 3D point")),
                }
            }
            Ok(())
        }
        (A::Index, P::List(v)) if v.len() >= 3 && v.iter().all(|x| matches!(x, P::Int(i) if *i > 0)) => Ok(()),
        (A::Derived, P::Derived) => Ok(()),
        _ => Err(format!("{p:?} where the schema has {}", describe(a))),
    }
}

fn describe(a: A) -> &'static str {
    match a {
        A::Guid => "a GlobalId",
        A::Text { .. } => "a string",
        A::Real { .. } => "a real",
        A::Int => "an integer",
        A::Bool { .. } => "a boolean",
        A::Ref { .. } => "a reference",
        A::Refs { .. } => "a list of references",
        A::Enum { .. } => "an enumeration",
        A::Reals { .. } => "a list of reals",
        A::Points3 => "a list of 3D points",
        A::Index => "a list of at least 3 positive indices",
        A::Derived => "a derived attribute (*)",
    }
}

fn reals(p: &P) -> Vec<f64> {
    match p {
        P::List(v) => v.iter().filter_map(|x| if let P::Real(r) = x { Some(*r) } else { None }).collect(),
        _ => Vec::new(),
    }
}

fn refs(p: &P) -> Vec<u64> {
    match p {
        P::List(v) => v.iter().filter_map(|x| if let P::Ref(r) = x { Some(*r) } else { None }).collect(),
        P::Ref(r) => vec![*r],
        _ => Vec::new(),
    }
}

/// Everything wrong with an IFC4 file `text`, by the schema's declarations and rules.
fn problems(text: &str) -> Vec<String> {
    let f = match read_step(text) {
        Ok(f) => f,
        Err(e) => return vec![format!("not a STEP file: {e}")],
    };
    let mut out = Vec::new();
    let mut bad = |m: String| out.push(m);
    // The header.
    if f.header.get("FILE_SCHEMA") != Some(&vec![P::List(vec![P::Str("IFC4".into())])]) {
        bad(format!("FILE_SCHEMA isn't (('IFC4')): {:?}", f.header.get("FILE_SCHEMA")));
    }
    if f.header.get("FILE_DESCRIPTION").map(Vec::len) != Some(2) || f.header.get("FILE_NAME").map(Vec::len) != Some(7) {
        bad("FILE_DESCRIPTION needs 2 parameters and FILE_NAME 7".into());
    }
    // Every instance as the schema declares it.
    let schema = schema();
    for (n, (t, params)) in &f.data {
        let Some(attrs) = schema.get(t.as_str()) else {
            bad(format!("#{n}: {t} isn't an IFC4 entity this check knows"));
            continue;
        };
        if attrs.len() != params.len() {
            bad(format!("#{n} {t}: {} attributes, the schema has {}", params.len(), attrs.len()));
            continue;
        }
        for (i, (p, a)) in params.iter().zip(attrs).enumerate() {
            if let Err(e) = matches(p, *a, &f.data) {
                bad(format!("#{n} {t} attribute {}: {e}", i + 1));
            }
        }
    }
    let of = |t: &'static str| f.data.iter().filter(move |(_, (x, _))| x == t);
    let get = |r: u64| f.data.get(&r);
    // IfcRoot: GlobalIds unique (UR1).
    let mut ids: HashMap<&str, u64> = HashMap::new();
    for (n, (t, p)) in &f.data {
        if let (true, Some(P::Str(g))) = (schema.get(t.as_str()).is_some_and(|a| matches!(a.first(), Some(A::Guid))), p.first())
            && let Some(other) = ids.insert(g, *n)
        {
            bad(format!("#{n} and #{other} have the same GlobalId {g}"));
        }
    }
    // One project, named, with a length unit.
    let projects: Vec<_> = of("IFCPROJECT").collect();
    if projects.len() != 1 {
        bad(format!("{} IfcProjects", projects.len()));
    }
    for (n, (_, p)) in &projects {
        if !matches!(p.get(2), Some(P::Str(_))) {
            bad(format!("#{n}: IfcProject HasName"));
        }
        let units: Vec<String> = p
            .get(8)
            .map(refs)
            .unwrap_or_default()
            .into_iter()
            .filter_map(get)
            .flat_map(|(_, u)| refs(&u[0]))
            .filter_map(get)
            .filter_map(|(_, s)| if let Some(P::Enum(e)) = s.get(1) { Some(e.clone()) } else { None })
            .collect();
        if !units.iter().any(|u| u == "LENGTHUNIT") {
            bad("no length unit".into());
        }
    }
    // Decomposition: each object in at most one IfcRelAggregates, the spatial structure whole.
    let mut parent: HashMap<u64, u64> = HashMap::new();
    for (n, (_, p)) in of("IFCRELAGGREGATES") {
        let whole = refs(&p[4]).first().copied().unwrap_or_default();
        for part in refs(&p[5]) {
            if part == whole {
                bad(format!("#{n}: NoSelfReference"));
            }
            if parent.insert(part, whole).is_some() {
                bad(format!("#{part} decomposes two objects"));
            }
        }
    }
    let ty = |r: u64| get(r).map_or("?", |(t, _)| t.as_str());
    for (want, up) in [("IFCSITE", "IFCPROJECT"), ("IFCBUILDING", "IFCSITE"), ("IFCBUILDINGSTOREY", "IFCBUILDING"), ("IFCSPACE", "IFCBUILDINGSTOREY")]
    {
        for (n, _) in of(want) {
            match parent.get(n) {
                Some(p) if ty(*p) == up => {}
                other => bad(format!("#{n} {want} is part of {:?}, not an {up}", other.map(|p| ty(*p)))),
            }
        }
    }
    // Containment: every element in exactly one storey (IfcElement.ContainedInStructure [0:1]),
    // no spatial element contained.
    let mut container: HashMap<u64, u64> = HashMap::new();
    for (n, (_, p)) in of("IFCRELCONTAINEDINSPATIALSTRUCTURE") {
        let st = refs(&p[5]).first().copied().unwrap_or_default();
        for e in refs(&p[4]) {
            if container.insert(e, st).is_some() {
                bad(format!("#{e} is contained twice (#{n})"));
            }
        }
    }
    let placement_of = |r: u64| get(r).and_then(|(_, p)| refs(p.get(5)?).first().copied());
    for t in ELEMENTS {
        for (n, (_, p)) in of(t) {
            let Some(st) = container.get(n) else {
                bad(format!("#{n} {t} isn't in a storey"));
                continue;
            };
            // Placed relative to its storey (the implementer agreement for contained elements).
            let rel = placement_of(*n).and_then(get).and_then(|(_, a)| refs(&a[0]).first().copied());
            if rel != placement_of(*st) {
                bad(format!("#{n} {t} isn't placed relative to its storey"));
            }
            // IfcProduct.PlacementForShapeRepresentation; IfcBuildingElementProxy.HasObjectName.
            if !matches!(p[6], P::Null) && matches!(p[5], P::Null) {
                bad(format!("#{n}: a representation without a placement"));
            }
            if *t == "IFCBUILDINGELEMENTPROXY" && !matches!(p[2], P::Str(_)) {
                bad(format!("#{n}: IfcBuildingElementProxy HasObjectName"));
            }
        }
    }
    // Openings: each voids one element (IfcFeatureElementSubtraction.VoidsElements [1:1]), is
    // filled by at most one (HasFillings), isn't in a storey itself, and is placed relative to the
    // element it voids; a door or window fills at most one opening (FillsVoids [0:1]).
    let mut voided: HashMap<u64, u64> = HashMap::new();
    for (n, (_, p)) in of("IFCRELVOIDSELEMENT") {
        let (host, opening) = (refs(&p[4])[0], refs(&p[5])[0]);
        if voided.insert(opening, host).is_some() {
            bad(format!("#{opening} voids two elements (#{n})"));
        }
    }
    let mut filled: HashMap<u64, u64> = HashMap::new();
    let mut fills: HashMap<u64, u64> = HashMap::new();
    for (n, (_, p)) in of("IFCRELFILLSELEMENT") {
        let (opening, by) = (refs(&p[4])[0], refs(&p[5])[0]);
        if filled.insert(opening, by).is_some() {
            bad(format!("#{opening} is filled twice (#{n})"));
        }
        if fills.insert(by, opening).is_some() {
            bad(format!("#{by} fills two openings (#{n})"));
        }
    }
    for (n, _) in of("IFCOPENINGELEMENT") {
        let Some(host) = voided.get(n) else {
            bad(format!("#{n}: an opening that voids nothing"));
            continue;
        };
        if container.contains_key(n) {
            bad(format!("#{n}: an opening contained in a storey"));
        }
        let rel = placement_of(*n).and_then(get).and_then(|(_, a)| refs(&a[0]).first().copied());
        if rel != placement_of(*host) {
            bad(format!("#{n}: an opening not placed relative to the element it voids"));
        }
    }
    // Placements: directions not zero, axis and reference direction given together and not
    // parallel.
    for (n, (_, p)) in of("IFCAXIS2PLACEMENT3D") {
        let dir = |i: usize| refs(&p[i]).first().and_then(|r| get(*r)).map(|(_, d)| reals(&d[0]));
        match (dir(1), dir(2)) {
            (Some(z), Some(x)) => {
                let cross = [z[1] * x[2] - z[2] * x[1], z[2] * x[0] - z[0] * x[2], z[0] * x[1] - z[1] * x[0]];
                if cross.iter().map(|c| c * c).sum::<f64>() < 1e-12 {
                    bad(format!("#{n}: AxisToRefDirPosition"));
                }
            }
            (None, None) => {}
            _ => bad(format!("#{n}: AxisAndRefDirProvision")),
        }
    }
    for (n, (_, p)) in of("IFCDIRECTION") {
        if reals(&p[0]).iter().map(|c| c * c).sum::<f64>() <= 0.0 {
            bad(format!("#{n}: MagnitudeGreaterZero"));
        }
    }
    // Face sets: every index names a point, no point twice in a face.
    for (n, (_, p)) in of("IFCPOLYGONALFACESET") {
        let points = refs(&p[0]).first().and_then(|r| get(*r)).map_or(0, |(_, l)| if let P::List(v) = &l[0] { v.len() } else { 0 });
        for face in refs(&p[2]) {
            let idx: Vec<i64> = get(face).map_or(Vec::new(), |(_, a)| {
                if let P::List(v) = &a[0] { v.iter().filter_map(|x| if let P::Int(i) = x { Some(*i) } else { None }).collect() } else { Vec::new() }
            });
            if idx.iter().any(|i| *i < 1 || *i as usize > points) {
                bad(format!("#{n}: face #{face} names a point the list hasn't ({points} points)"));
            }
            if idx.iter().collect::<BTreeSet<_>>().len() != idx.len() {
                bad(format!("#{n}: face #{face} repeats a point"));
            }
        }
    }
    for (n, (_, p)) in of("IFCSHAPEREPRESENTATION") {
        if !matches!((&p[1], &p[2]), (P::Str(_), P::Str(_))) {
            bad(format!("#{n}: HasRepresentationIdentifier / HasRepresentationType"));
        }
    }
    // Quantities: unique names in a set, never negative.
    for (n, (_, p)) in of("IFCELEMENTQUANTITY") {
        let mut names = BTreeSet::new();
        for q in refs(&p[5]) {
            let Some((_, a)) = get(q) else { continue };
            if let P::Str(name) = &a[0]
                && !names.insert(name.clone())
            {
                bad(format!("#{n}: UniqueQuantityNames ({name})"));
            }
            if matches!(a[3], P::Real(v) if v < 0.0) {
                bad(format!("#{q}: a negative quantity"));
            }
        }
    }
    out
}

// ---------------- the tests ----------------

/// Entity → instances.
fn census(text: &str) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for (t, _) in read_step(text).unwrap().data.values() {
        *m.entry(t.clone()).or_default() += 1;
    }
    m
}

#[test]
fn the_export_is_valid_ifc4_with_every_part_as_its_class() {
    hooks();
    let mut s = building();
    let text = export(&mut s, "Sample Building.ifc");
    let p = problems(&text);
    assert!(p.is_empty(), "{}", p.join("\n"));
    let c = census(&text);
    let n = |t: &str| c.get(t).copied().unwrap_or(0);
    let parts = parts(&s);
    assert_eq!(n("IFCWALL"), parts["wall"]);
    assert_eq!(n("IFCSLAB"), parts["floor"]);
    assert_eq!(n("IFCCOVERING"), parts["ceiling"]);
    assert_eq!((n("IFCDOOR"), n("IFCWINDOW"), n("IFCCOLUMN"), n("IFCCURTAINWALL")), (1, 1, 1, 1));
    assert_eq!((n("IFCSTAIR"), n("IFCRAILING")), (parts["stair"], parts["railing"]));
    assert_eq!((n("IFCDUCTSEGMENT"), n("IFCPIPESEGMENT"), n("IFCCABLECARRIERSEGMENT")), (1, 1, 1), "MEP runs as their IFC classes");
    assert_eq!(n("IFCSPACE"), 3);
    assert_eq!(n("IFCBUILDINGELEMENTPROXY"), 0, "nothing left as a proxy: {c:?}");
    assert_eq!(n("IFCBUILDINGSTOREY"), 2);
    // The door and the window each fill an opening in a wall.
    assert_eq!((n("IFCOPENINGELEMENT"), n("IFCRELVOIDSELEMENT"), n("IFCRELFILLSELEMENT")), (2, 2, 2), "{c:?}");
    {
        let f = read_step(&text).unwrap();
        let ty = |r: u64| f.data[&r].0.clone();
        for (_, p) in f.data.values().filter(|(t, _)| t == "IFCRELVOIDSELEMENT") {
            assert_eq!(ty(refs(&p[4])[0]), "IFCWALL", "the host is a wall");
        }
        let filled: BTreeSet<String> = f.data.values().filter(|(t, _)| t == "IFCRELFILLSELEMENT").map(|(_, p)| ty(refs(&p[5])[0])).collect();
        assert_eq!(filled, ["IFCDOOR".to_string(), "IFCWINDOW".to_string()].into_iter().collect());
        // Each opening reaches through its wall and spans the door or window.
        for (_, p) in f.data.values().filter(|(t, _)| t == "IFCRELFILLSELEMENT") {
            let body = |product: u64| -> ([f64; 3], [f64; 3]) {
                let pa = &f.data[&product].1;
                let shape = &f.data[&refs(&pa[6])[0]].1;
                let rep = &f.data[&refs(&shape[2])[0]].1;
                let set = &f.data[&refs(&rep[3])[0]].1;
                let P::List(coords) = &f.data[&refs(&set[0])[0]].1[0] else { panic!() };
                coords.iter().map(reals).fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), c| {
                    (std::array::from_fn(|i| lo[i].min(c[i])), std::array::from_fn(|i| hi[i].max(c[i])))
                })
            };
            let (opening, filler) = (body(refs(&p[4])[0]), body(refs(&p[5])[0]));
            assert!((0..3).all(|i| opening.0[i] <= filler.0[i] + 1e-9 && opening.1[i] >= filler.1[i] - 1e-9), "{opening:?} holds {filler:?}");
        }
    }
    // The storeys stack: L2's plan is drawn 200' east of L1's, its base point lines it up over L1
    // (every body within the 40' × 24' footprint, slabs' edges aside).
    let f = read_step(&text).unwrap();
    let xs: Vec<f64> = f
        .data
        .values()
        .filter(|(t, _)| t == "IFCCARTESIANPOINTLIST3D")
        .flat_map(|(_, p)| if let P::List(v) = &p[0] { v.iter().map(|c| reals(c)[0]).collect::<Vec<_>>() } else { Vec::new() })
        .collect();
    let (lo, hi) = xs.iter().fold((f64::MAX, f64::MIN), |(l, h), x| (l.min(*x), h.max(*x)));
    assert!(lo > -1.0 && hi < 40.0 * 0.3048 + 1.0, "x from {lo} to {hi} m: the storeys don't stack");
    // A file that breaks the rules is caught (the check itself is checked).
    let broken = [
        text.replacen("IFCWALL('", "IFCWALL('!", 1),
        text.replacen(",.STANDARD.)", ",.WOBBLY.)", 1),
        text.replacen("IFCPROJECT(", "IFCPROJECT($,", 1),
        text.replacen("IFCINDEXEDPOLYGONALFACE((1,", "IFCINDEXEDPOLYGONALFACE((99999,", 1),
    ];
    for b in &broken {
        assert!(!problems(b).is_empty(), "a broken file passed");
    }
}

/// A metric drawing, an imperial one without units and a drawing read from IFC: all valid, in
/// the right units.
#[test]
fn every_kind_of_drawing_exports_valid_ifc_in_the_right_units() {
    hooks();
    let cube = |s: f64| Mesh {
        vertices: [(0., 0., 0.), (s, 0., 0.), (s, s, 0.), (0., s, 0.), (0., 0., s), (s, 0., s), (s, s, s), (0., s, s)]
            .iter()
            .map(|(x, y, z)| cadaddy_engine::geom::Vec3::new(*x, *y, *z))
            .collect(),
        faces: [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]]
            .iter()
            .map(|f| cadaddy_engine::doc::MeshFace { v: f.to_vec(), hidden: 0 })
            .collect(),
        ..Mesh::default()
    };
    // (INSUNITS, MEASUREMENT, a 10-unit cube's edge in the file's coordinates, the length unit).
    for (insunits, measurement, edge, unit) in
        [(4, 1, 10.0, ".MILLI."), (1, 0, 0.254, "$"), (0, 0, 0.254, "$"), (0, 1, 10.0, ".MILLI."), (6, 1, 10.0, "$")]
    {
        let mut d = cadaddy_engine::doc::Drawing::new_imperial();
        d.header.set_i64("INSUNITS", insunits);
        d.header.set_i64("MEASUREMENT", measurement);
        d.add(&cadaddy_engine::doc::Space::Model, Default::default(), EntityKind::Mesh(cube(10.0))).unwrap();
        let mut s = Session::empty();
        s.open_drawing(d, "Units", None);
        let text = export(&mut s, "Units.ifc");
        let p = problems(&text);
        assert!(p.is_empty(), "{insunits}: {}", p.join("\n"));
        assert!(text.contains(&format!("IFCSIUNIT(*,.LENGTHUNIT.,{unit},.METRE.)")), "{insunits}/{measurement}: {text}");
        let f = read_step(&text).unwrap();
        let max = f
            .data
            .values()
            .filter(|(t, _)| t == "IFCCARTESIANPOINTLIST3D")
            .flat_map(|(_, p)| if let P::List(v) = &p[0] { v.iter().flat_map(reals).collect::<Vec<_>>() } else { Vec::new() })
            .fold(0.0, f64::max);
        assert!((max - edge).abs() < 1e-9, "INSUNITS {insunits}, MEASUREMENT {measurement}: the cube is {max} across, not {edge}");
    }
    // Read from IFC and written again.
    let mut s = building();
    let first = export(&mut s, "Sample Building.ifc");
    let mut t = Session::new();
    let dir = std::env::temp_dir().join(format!("cadaddy-ifcvalid-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Sample Building.ifc");
    std::fs::write(&path, &first).unwrap();
    t.execute("import", &json!({ "path": path })).unwrap();
    let again = export(&mut t, "Again.ifc");
    let p = problems(&again);
    assert!(p.is_empty(), "{}", p.join("\n"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Product GlobalIds by (class, name or tag) — the elements' ids in a file.
fn element_ids(text: &str) -> BTreeMap<String, String> {
    let f = read_step(text).unwrap();
    let mut out = BTreeMap::new();
    for (t, p) in f.data.values() {
        if (ELEMENTS.contains(&t.as_str()) || t == "IFCSPACE")
            && let Some(P::Str(g)) = p.first()
        {
            out.insert(g.clone(), t.clone());
        }
    }
    out
}

#[test]
fn global_ids_stay_the_same_on_re_export() {
    hooks();
    let mut s = building();
    let a = export(&mut s, "Sample Building.ifc");
    // The same drawing again, under another name: the same ids, project and storeys included.
    let b = export(&mut s, "Sample Building v2.ifc");
    let ids = |t: &str| {
        read_step(t)
            .unwrap()
            .data
            .values()
            .filter_map(|(_, p)| if let Some(P::Str(g)) = p.first() { Some(g.clone()) } else { None })
            .filter(|g| g.len() == 22)
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(ids(&a), ids(&b));
    // Saved as DWG and opened again: the same ids.
    let dir = std::env::temp_dir().join(format!("cadaddy-ifcvalid-ids-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dwg = dir.join("Sample Building.dwg");
    s.execute("saveas", &json!({ "path": dwg })).unwrap();
    let mut r = Session::new();
    r.execute("open", &json!({ "path": dwg })).unwrap();
    assert_eq!(element_ids(&export(&mut r, "Sample Building.ifc")), element_ids(&a), "after DWG");
    // An edit (a new level's worth: another storey and a wall) leaves the others' ids alone.
    let before = element_ids(&a);
    let wall = s.doc().unwrap().model.iter().find(|e| made_as(e).as_deref() == Some("wall")).unwrap().clone();
    let mut copy = (*wall).clone();
    if let Some(at) = copy.common.xdata.iter().position(|t| t.value == "L1") {
        copy.common.xdata[at].value = "L0".into();
    }
    if let EntityKind::Mesh(m) = &mut copy.kind {
        m.vertices.iter_mut().for_each(|v| v.z -= 144.0);
    }
    s.doc_mut().unwrap().add(&cadaddy_engine::doc::Space::Model, copy.common.clone(), copy.kind.clone()).unwrap();
    let edited = export(&mut s, "Sample Building.ifc");
    let after = element_ids(&edited);
    assert!(before.iter().all(|(g, t)| after.get(g) == Some(t)), "every element keeps its id and class");
    assert_eq!(after.len(), before.len() + 1);
    let storeys = |t: &str| {
        read_step(t)
            .unwrap()
            .data
            .values()
            .filter(|(t, _)| t == "IFCBUILDINGSTOREY")
            .filter_map(|(_, p)| if let (Some(P::Str(g)), Some(P::Str(n))) = (p.first(), p.get(2)) { Some((n.clone(), g.clone())) } else { None })
            .collect::<BTreeMap<_, _>>()
    };
    let (s0, s1) = (storeys(&a), storeys(&edited));
    assert_eq!(s1.len(), 3);
    assert!(s0.iter().all(|(n, g)| s1.get(n) == Some(g)), "a storey added below leaves the others' ids: {s0:?} {s1:?}");
    // IFC → drawing → IFC: the same ids and classes.
    let path = dir.join("In.ifc");
    std::fs::write(&path, &a).unwrap();
    let mut t = Session::new();
    t.execute("import", &json!({ "path": path })).unwrap();
    let round = export(&mut t, "Out.ifc");
    assert_eq!(element_ids(&round), before, "read from IFC and written again");
    // A copy of an imported object gets an id of its own.
    let first = t.doc().unwrap().model.iter().next().unwrap().clone();
    t.doc_mut().unwrap().add(&cadaddy_engine::doc::Space::Model, first.common.clone(), first.kind.clone()).unwrap();
    let copied = export(&mut t, "Out.ifc");
    assert!(problems(&copied).is_empty(), "{:?}", problems(&copied));
    assert_eq!(element_ids(&copied).len(), before.len() + 1);
    // Another drawing (AutoCAD gives each its FINGERPRINTGUID) doesn't share ids with this one.
    let mut other = building();
    other.doc_mut().unwrap().header.set_str("FINGERPRINTGUID", "{0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0}");
    let o = element_ids(&export(&mut other, "Other.ifc"));
    assert!(o.keys().all(|g| !before.contains_key(g)), "two drawings, no shared ids");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The closed mesh's volume (divergence theorem) and its height.
fn volume(points: &[[f64; 3]], faces: &[Vec<usize>]) -> f64 {
    let mut v = 0.0;
    for f in faces {
        let a = points[f[0]];
        for w in f[1..].windows(2) {
            let (b, c) = (points[w[0]], points[w[1]]);
            v += a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]);
        }
    }
    (v / 6.0).abs()
}

#[test]
fn quantities_match_the_drawing() {
    hooks();
    let mut s = building();
    let text = export(&mut s, "Sample Building.ifc");
    let f = read_step(&text).unwrap();
    let get = |r: u64| &f.data[&r];
    // Each product's quantities by name, and its body's volume from the file's own geometry.
    let mut checked = BTreeMap::<String, usize>::new();
    let rooms = s.execute("rooms.list", &json!({})).unwrap();
    let d = s.doc().unwrap();
    let in3 = 0.0254f64.powi(3);
    for (_, (t, p)) in f.data.iter().filter(|(_, (t, _))| t == "IFCRELDEFINESBYPROPERTIES") {
        let _ = t;
        let product = refs(&p[4])[0];
        let (class, pa) = get(product);
        let set = get(refs(&p[5])[0]);
        let q: BTreeMap<String, f64> = refs(&set.1[5])
            .into_iter()
            .map(|r| {
                let (_, a) = get(r);
                let (P::Str(n), P::Real(v)) = (&a[0], &a[3]) else { panic!() };
                (n.clone(), *v)
            })
            .collect();
        // The drawing object (the element's Tag is its handle; a space's name is its room's).
        let mesh = if class == "IFCSPACE" {
            let P::Str(room) = &pa[7] else { panic!() };
            d.model.iter().find_map(|e| match (&e.kind, e.common.xdata.iter().any(|t| &t.value == room)) {
                (EntityKind::Mesh(m), true) if e.common.layer == "3D-Rooms" => Some(m.clone()),
                _ => None,
            })
        } else {
            let P::Str(tag) = &pa[7] else { panic!() };
            d.model.iter().find(|e| &e.handle.hex() == tag).and_then(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None })
        }
        .unwrap_or_else(|| panic!("#{product} {class}: no drawing object"));
        let pts: Vec<[f64; 3]> = mesh.vertices.iter().map(|v| [v.x, v.y, v.z]).collect();
        let faces: Vec<Vec<usize>> = mesh.faces.iter().map(|f| f.v.iter().map(|i| *i as usize).collect()).collect();
        let (lo, hi) = pts.iter().fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p[2]), h.max(p[2])));
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1e-9);
        if let Some(v) = q.get("GrossVolume") {
            assert!(close(*v, volume(&pts, &faces) * in3), "#{product} {class}: GrossVolume {v} vs the drawing's {}", volume(&pts, &faces) * in3);
            // The file's body holds the same volume.
            let shape = get(refs(&pa[6])[0]);
            let rep = get(refs(&shape.1[2])[0]);
            let set = get(refs(&rep.1[3])[0]);
            let P::List(coords) = &get(refs(&set.1[0])[0]).1[0] else { panic!() };
            let ifc_pts: Vec<[f64; 3]> = coords
                .iter()
                .map(|c| {
                    let r = reals(c);
                    [r[0], r[1], r[2]]
                })
                .collect();
            let ifc_faces: Vec<Vec<usize>> = refs(&set.1[2])
                .into_iter()
                .map(|r| {
                    if let P::List(v) = &get(r).1[0] {
                        v.iter().map(|i| if let P::Int(i) = i { *i as usize - 1 } else { 0 }).collect()
                    } else {
                        Vec::new()
                    }
                })
                .collect();
            // Coordinates are written to a micrometre: the same to 1 part in 10⁴.
            let body = volume(&ifc_pts, &ifc_faces);
            assert!((v - body).abs() <= 1e-4 * v.max(body), "#{product} {class}: the body's volume {body} vs {v}");
            *checked.entry("GrossVolume".into()).or_default() += 1;
        }
        for name in ["Height", "Length"] {
            if let Some(h) = q.get(name) {
                assert!(close(*h, (hi - lo) * 0.0254), "#{product} {class}: {name} {h}");
                *checked.entry(name.into()).or_default() += 1;
            }
        }
        if let Some(a) = q.get("GrossFloorArea") {
            // The room's own area (ROOMS measures its outline).
            let P::Str(room) = &pa[7] else { panic!() };
            let want = rooms["rooms"].as_array().unwrap().iter().find(|r| r["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(room))).unwrap()
                ["areaM2"]
                .as_f64()
                .unwrap();
            assert!((a - want).abs() <= 0.01 * want, "#{product} {room}: {a} m² vs ROOMS' {want} m²");
            *checked.entry("GrossFloorArea".into()).or_default() += 1;
        }
    }
    for (q, at_least) in [("GrossVolume", 10), ("Height", 10), ("GrossFloorArea", 3), ("Length", 1)] {
        assert!(checked.get(q).copied().unwrap_or(0) >= at_least, "{q}: {checked:?}");
    }
}

#[test]
fn keep_for_outside_checks() {
    hooks();
    let mut s = building();
    export(&mut s, "Sample Building.ifc");
}

/// The rooms come from the walls and are named by the room tags alone (Q215): the duct, pipe and
/// conduit runs, their tags, the stair and the built 3D parts never split or name a room. Each
/// room ends at the partition's face (4⅞" about its centreline) and the exterior walls' inner face.
#[test]
fn rooms_have_their_names_and_areas() {
    let mut s = building();
    let half_partition = 4.875 / 24.0;
    let want = [
        ("LOBBY", "L1", (39.5 - (16.0 + half_partition)) * 23.0),
        ("OFFICE", "L1", (16.0 - half_partition - 0.5) * 23.0),
        ("STUDIO", "L2", 39.0 * 23.0),
    ];
    // Twice: rooms.build's volumes are in the drawing the second time.
    for _ in 0..2 {
        let rooms = s.execute("rooms.list", &json!({"outline": false})).unwrap();
        let mut got: Vec<(String, String, f64, f64)> = rooms["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["name"].as_str().unwrap().to_string(),
                    r["level"].as_str().unwrap_or("").to_string(),
                    r["areaSqFt"].as_f64().unwrap(),
                    r["areaM2"].as_f64().unwrap(),
                )
            })
            .collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got.len(), want.len(), "{got:?}");
        for ((name, level, sqft, m2), (wname, wlevel, wsqft)) in got.iter().zip(want) {
            assert_eq!((name.as_str(), level.as_str()), (wname, wlevel), "{got:?}");
            assert!((sqft - wsqft).abs() < 0.01, "{name}: {sqft} sq ft, want {wsqft}");
            assert!((m2 - wsqft * 0.092_903_04).abs() < 0.01, "{name}: {m2} m², want {}", wsqft * 0.092_903_04);
        }
        s.execute("rooms.build", &json!({})).unwrap();
    }
    // The ceilings model.build traced follow the same outlines.
    let ceilings = s.doc().unwrap().model.iter().filter(|e| e.common.layer == "3D-Ceilings").count();
    assert_eq!(ceilings, 3);
}

/// New drawings get a FINGERPRINTGUID of their own (a template's isn't shared), kept by DXF, DWG and
/// .cdy; two new drawings with the same objects export IFC GlobalIds that never collide.
#[test]
fn new_drawings_have_their_own_fingerprint() {
    hooks();
    let fp = |s: &Session| s.doc().unwrap().header.str("FINGERPRINTGUID", "");
    let (mut a, mut b) = (Session::new(), Session::new());
    let (fa, fb) = (fp(&a), fp(&b));
    let ok = |g: &str| g.len() == 38 && g.starts_with('{') && g.ends_with('}') && g.matches('-').count() == 4;
    assert!(ok(&fa) && ok(&fb), "{fa} {fb}");
    assert_ne!(fa, fb);
    let dir = std::env::temp_dir().join(format!("cadaddy-ifcvalid-fp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for ext in ["dxf", "dwg", "cdy"] {
        let path = dir.join(format!("Project A.{ext}"));
        a.execute("saveas", &json!({ "path": path })).unwrap();
        let mut r = Session::new();
        r.execute("open", &json!({ "path": path })).unwrap();
        assert_eq!(fp(&r), fa, "{ext} keeps it");
    }
    // NEW from a template: not the template's.
    let mut t = Session::new();
    t.execute("new", &json!({ "template": dir.join("Project A.dxf") })).unwrap();
    assert!(ok(&fp(&t)) && fp(&t) != fa, "a new drawing from a template gets its own");
    // The same objects in two new drawings: no shared GlobalIds.
    for s in [&mut a, &mut b] {
        s.execute("box", &json!({ "corner": [0, 0, 0], "length": 5, "width": 4, "height": 3 })).unwrap();
        s.execute("mesh.convert", &json!({})).ok();
    }
    let mesh = |s: &mut Session| {
        let d = s.doc_mut().unwrap();
        let m = Mesh {
            vertices: [(0., 0., 0.), (1., 0., 0.), (0., 1., 0.), (0., 0., 1.)]
                .iter()
                .map(|(x, y, z)| cadaddy_engine::geom::Vec3::new(*x, *y, *z))
                .collect(),
            faces: [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]].iter().map(|f| cadaddy_engine::doc::MeshFace { v: f.to_vec(), hidden: 0 }).collect(),
            ..Mesh::default()
        };
        d.add(&cadaddy_engine::doc::Space::Model, Default::default(), EntityKind::Mesh(m)).unwrap();
    };
    mesh(&mut a);
    mesh(&mut b);
    let ids = |t: &str| {
        read_step(t)
            .unwrap()
            .data
            .values()
            .filter_map(|(_, p)| if let Some(P::Str(g)) = p.first() { Some(g.clone()) } else { None })
            .filter(|g| g.len() == 22)
            .collect::<BTreeSet<_>>()
    };
    let (ia, ib) = (ids(&export(&mut a, "A.ifc")), ids(&export(&mut b, "B.ifc")));
    assert!(!ia.is_empty() && ia.is_disjoint(&ib), "two drawings share GlobalIds: {:?}", ia.intersection(&ib).collect::<Vec<_>>());
    let _ = std::fs::remove_dir_all(&dir);
}
