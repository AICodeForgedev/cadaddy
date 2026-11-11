//! Q6484: the set's symbol table on every plan — synthetic sets in paper inches (one drawing unit
//! is a paper inch), sheets 36" × 24" side by side.

use serde_json::json;

use super::super::symtable::{self, Stroke};
use super::*;

/// A set being drawn: the session and its job, and the next sheet's place.
struct Set {
    s: crate::Session,
    job: GuidedJob,
    next_x: f64,
}

const SHEET_W: f64 = 36.0;
const SHEET_H: f64 = 24.0;
const TEXT_H: f64 = 0.094;

impl Set {
    fn new() -> Set {
        Set { s: crate::Session::new(), job: GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff), next_x: 0.0 }
    }

    fn line(&mut self, a: [f64; 2], b: [f64; 2]) {
        self.s.execute("line", &json!({"points": [a, b]})).unwrap();
    }

    fn text(&mut self, at: [f64; 2], v: &str) {
        self.s.execute("text", &json!({"at": at, "text": v, "height": TEXT_H})).unwrap();
    }

    /// A drawing's strokes (about its middle) turned `k` quarters, its middle at `o`.
    fn draw(&mut self, strokes: &[Stroke], o: [f64; 2], k: u8) {
        for st in strokes {
            match st.turned(k) {
                Stroke::Seg([a, b, c, d]) => self.line([o[0] + a, o[1] + b], [o[0] + c, o[1] + d]),
                Stroke::Round([x, y, r]) => {
                    self.s.execute("circle", &json!({"center": [o[0] + x, o[1] + y], "radius": r})).unwrap();
                }
            }
        }
    }

    /// One sheet: drawn by `f` at its offset (x0), its region, span and classification; with a
    /// paper scale (one unit an inch) when `scaled`.
    fn sheet(&mut self, number: &str, kind: &str, scaled: bool, f: impl FnOnce(&mut Set, f64)) {
        let x0 = self.next_x;
        self.next_x += SHEET_W + 4.0;
        let lo = self.s.doc().unwrap().handseed;
        f(self, x0);
        let hi = self.s.doc().unwrap().handseed;
        self.job.sheet_regions.push(SheetRegion {
            number: number.into(),
            level: "1".into(),
            discipline: "E".into(),
            kind: kind.into(),
            rect: [x0, 0.0, x0 + SHEET_W, SHEET_H],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        });
        self.job.sheet_spans.push(SheetSpan { sheet: number.into(), lo, hi });
        let mut list = self.job.sheets.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array).cloned().unwrap_or_default();
        list.push(json!({"number": number, "type": kind, "discipline": "E"}));
        self.job.sheets = Some(json!({"sheets": list}));
        if scaled {
            self.job.sheet_scales.push(SheetScale {
                sheet: number.into(),
                kind: kind.into(),
                scale: 1.0,
                source: "option".into(),
                plan_scale: None,
                paper: Some([SHEET_W, SHEET_H]),
            });
        }
    }

    /// The last sheet made is of `discipline`, and printed as views `views` (a region each).
    fn retag(&mut self, discipline: &str, views: &[[f64; 4]]) {
        let Some(last) = self.job.sheet_regions.pop() else { return };
        let number = last.number.clone();
        if let Some(list) = self.job.sheets.as_mut().and_then(|v| v.get_mut("sheets")).and_then(Value::as_array_mut)
            && let Some(e) = list.iter_mut().find(|e| e["number"] == number.as_str())
        {
            e["discipline"] = json!(discipline);
        }
        if views.is_empty() {
            self.job.sheet_regions.push(SheetRegion { discipline: discipline.into(), ..last });
            return;
        }
        for v in views {
            self.job.sheet_regions.push(SheetRegion {
                discipline: discipline.into(),
                view: Some(SheetView { title: "PLAN".into(), rect: *v }),
                ..last.clone()
            });
        }
    }

    /// A SYMBOL | DESCRIPTION legend at `x0`: each row's words and drawing.
    fn legend(&mut self, x0: f64, rows: &[(&str, Vec<Stroke>)]) {
        self.text([x0 + 1.0, 20.0], "SYMBOL");
        self.text([x0 + 1.6, 20.0], "DESCRIPTION");
        for (i, (words, strokes)) in rows.iter().enumerate() {
            let y = 19.7 - 0.3 * i as f64;
            self.text([x0 + 1.6, y], words);
            self.draw(strokes, [x0 + 1.2, y + 0.04], 0);
        }
    }

    /// A room's walls (both faces, 0.06 apart) from `a` to `b`: the inner face's corners.
    fn room(&mut self, a: [f64; 2], b: [f64; 2]) -> ([f64; 2], [f64; 2]) {
        for off in [0.0, 0.06] {
            let (p, q) = ([a[0] + off, a[1] + off], [b[0] - off, b[1] - off]);
            self.line(p, [q[0], p[1]]);
            self.line([q[0], p[1]], q);
            self.line(q, [p[0], q[1]]);
            self.line([p[0], q[1]], p);
        }
        ([a[0] + 0.06, a[1] + 0.06], [b[0] - 0.06, b[1] - 0.06])
    }

    /// The plan search as the build runs it, with `lib` the shared library.
    fn search(&mut self, lib: &[(symtable::LibSymbol, bool)], promote_to: Option<&std::path::Path>) -> PlanSymbols {
        let pages = legend_pages(self.s.doc().unwrap(), &self.job);
        plan_symbols(&mut self.s, &mut self.job, &pages, &TypicalMarks::default(), lib, promote_to).unwrap()
    }
}

fn shipped() -> Vec<(symtable::LibSymbol, bool)> {
    symtable::library(Some(std::path::Path::new("/nonexistent/symbols")))
}

fn lib_strokes(name: &str) -> Vec<Stroke> {
    shipped().into_iter().find(|(s, _)| s.name == name).map(|(s, _)| s.strokes).unwrap_or_default()
}

fn seg(a: f64, b: f64, c: f64, d: f64) -> Stroke {
    Stroke::Seg([a, b, c, d])
}

/// A special purpose receptacle: a circle with a cross in it.
fn special() -> Vec<Stroke> {
    vec![Stroke::Round([0.0, 0.0, 0.06]), seg(-0.042, -0.042, 0.042, 0.042), seg(-0.042, 0.042, 0.042, -0.042)]
}

/// A data outlet: a plain triangle (point up).
fn triangle() -> Vec<Stroke> {
    vec![seg(-0.06, -0.052, 0.06, -0.052), seg(0.06, -0.052, 0.0, 0.052), seg(0.0, 0.052, -0.06, -0.052)]
}

/// A junction box drawn as a hexagon.
fn hexagon() -> Vec<Stroke> {
    let r = 0.07;
    let p = |i: usize| {
        let a = std::f64::consts::FRAC_PI_3 * i as f64;
        (r * a.cos(), r * a.sin())
    };
    (0..6)
        .map(|i| {
            let (a, b) = (p(i), p(i + 1));
            seg(a.0, a.1, b.0, b.1)
        })
        .collect()
}

/// A TV outlet: a square with its diagonal.
fn tv() -> Vec<Stroke> {
    let h = 0.06;
    vec![seg(-h, -h, h, -h), seg(h, -h, h, h), seg(h, h, -h, h), seg(-h, h, -h, -h), seg(-h, -h, h, h)]
}

/// A card reader: a square with a circle in it.
fn card_reader() -> Vec<Stroke> {
    let h = 0.07;
    vec![seg(-h, -h, h, -h), seg(h, -h, h, h), seg(h, h, -h, h), seg(-h, h, -h, -h), Stroke::Round([0.0, 0.0, 0.035])]
}

/// Lettering drawn as strokes (a `1` and an `E` beside a symbol), as an outline-font PDF does.
fn stroked_letters(set: &mut Set, at: [f64; 2]) {
    let [x, y] = at;
    set.line([x, y], [x, y + 0.09]);
    set.line([x + 0.04, y], [x + 0.04, y + 0.09]);
    set.line([x + 0.04, y + 0.09], [x + 0.1, y + 0.09]);
    set.line([x + 0.04, y + 0.045], [x + 0.09, y + 0.045]);
    set.line([x + 0.04, y], [x + 0.1, y]);
}

/// Test A: a legend of four loose-linework symbols; a floor plan (no typical room's view) with 3,
/// 2, 1 and 0 of them, turned and touching its walls, its lettering strokes, and linework that
/// looks like symbols (door swings, column bubbles, a north arrow, section marks). Exactly six
/// devices of their rows' kinds, each with its kind's form; the row no plan draws is reported;
/// the look-alikes give none.
#[test]
fn a_legends_loose_symbols_are_found_on_an_ordinary_floor_plan() {
    let mut set = Set::new();
    set.sheet("E-001", "legend", false, |set, x0| {
        set.legend(
            x0,
            &[("SPECIAL PURPOSE RECEPTACLE", special()), ("DATA OUTLET", triangle()), ("JUNCTION BOX", hexagon()), ("TELEVISION OUTLET", tv())],
        );
    });
    set.sheet("E-101", "plan", false, |set, x0| {
        let (lo, hi) = set.room([x0 + 2.0, 2.0], [x0 + 30.0, 20.0]);
        // Three special purpose receptacles on the left, top and right walls (turned).
        set.draw(&special(), [lo[0] + 0.06, 6.0], 0);
        set.draw(&special(), [x0 + 10.0, hi[1] - 0.06], 1);
        set.draw(&special(), [hi[0] - 0.06, 12.0], 2);
        // Two data outlets on the bottom and left walls.
        set.draw(&triangle(), [x0 + 14.0, lo[1] + 0.052], 0);
        set.draw(&triangle(), [lo[0] + 0.052, 15.0], 3);
        // A junction box in the room.
        set.draw(&hexagon(), [x0 + 16.0, 11.0], 1);
        // Lettering as strokes by each.
        for at in [[lo[0] + 0.2, 6.1], [x0 + 10.2, hi[1] - 0.25], [hi[0] - 0.4, 12.1], [x0 + 14.2, lo[1] + 0.1], [x0 + 16.2, 11.1]] {
            stroked_letters(set, at);
        }
        // Look-alikes: a door swing and its leaf, column grid bubbles on their lines, a north
        // arrow (a data outlet's triangle in a circle), section marks (a cut line, a bubble and
        // a data outlet's triangle as its pointer).
        set.s.execute("arc", &json!({"center": [x0 + 20.0, lo[1]], "radius": 0.375, "start": 0.0, "end": 90.0})).unwrap();
        set.line([x0 + 20.0, lo[1]], [x0 + 20.0, lo[1] + 0.375]);
        for gx in [x0 + 5.0, x0 + 25.0] {
            set.line([gx, 21.0], [gx, 1.0]);
            set.s.execute("circle", &json!({"center": [gx, 21.2], "radius": 0.07})).unwrap();
            set.s.execute("circle", &json!({"center": [gx, 21.2], "radius": 0.06})).unwrap();
        }
        set.s.execute("circle", &json!({"center": [x0 + 33.0, 3.0], "radius": 0.08})).unwrap();
        set.draw(&triangle(), [x0 + 33.0, 3.0], 0);
        for (cx, cy) in [(x0 + 8.0, 9.0), (x0 + 24.0, 9.0)] {
            set.line([cx - 1.0, cy], [cx + 1.0, cy]);
            set.s.execute("circle", &json!({"center": [cx - 1.08, cy], "radius": 0.08})).unwrap();
            set.draw(&triangle(), [cx - 1.08, cy + 0.13], 0);
        }
    });
    // Before (Q6484): the legend's drawings were looked for in typical rooms' views only, so an
    // ordinary floor plan gave none of them.
    let pages = legend_pages(set.s.doc().unwrap(), &set.job);
    assert!(typical_view_marks(set.s.doc().unwrap(), &set.job, &pages).added.is_empty());
    let found = set.search(&shipped(), None);
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for dv in &found.added {
        *kinds.entry(dv.kind.clone()).or_default() += 1;
    }
    assert_eq!(
        kinds,
        BTreeMap::from([("receptacle".to_string(), 3), ("data_outlet".to_string(), 2), ("junction_box".to_string(), 1)]),
        "{:#?}",
        found.added
    );
    assert!(
        found.added.iter().all(|dv| dv.sheet == "E-101" && dv.level == "1" && dv.kind_source.starts_with("E-001 legend: ")),
        "{:#?}",
        found.added
    );
    assert!(found.added.iter().all(|dv| dv.symbol.as_ref().is_some_and(|y| y.status == "documented" && !y.as_drawn.is_empty())));
    // The look-alikes: triangles with no second cue were left out.
    assert!(found.uncued >= 3, "{}", found.uncued);
    // The row no plan draws.
    assert_eq!(found.zero_rows, ["E-001 legend: TELEVISION OUTLET"]);
    assert_eq!(found.report.by_row.get("legend:e-001:special-purpose-receptacle").and_then(|m| m.get("E-101")), Some(&3));
    // Each device's kind has its form (the fields it must carry).
    let ts = super::super::forms::templates();
    for k in kinds.keys() {
        assert!(super::super::forms::template_for(&ts, k).is_some(), "no form for {k}");
    }
    // The objects drawing them are objects of their kinds in the print inventory.
    assert!(found.added.iter().all(|dv| job_fate_is_object(&set.job, &dv.handle, &dv.kind)), "{:?}", set.job.mark_fates);
    // Searched again with what it found: no more.
    set.job.devices.extend(found.added.iter().cloned());
    let again = set.search(&shipped(), None);
    assert!(again.added.is_empty(), "{:#?}", again.added);
    assert_eq!(again.read_already, 6);
}

fn job_fate_is_object(job: &GuidedJob, handle: &str, kind: &str) -> bool {
    job.mark_fates.get(&handle.to_ascii_uppercase()).is_some_and(|f| f.object && f.kind == kind)
}

/// The shared library on a set with no legend: its receptacles by their drawing and its switches
/// by their `S` on a wall, each derived (no document of the set lists them); an `S` on no wall is
/// no switch.
fn library_plan(set: &mut Set) {
    let duplex = lib_strokes("duplex receptacle");
    set.sheet("E-101", "plan", true, |set, x0| {
        let (lo, hi) = set.room([x0 + 2.0, 2.0], [x0 + 30.0, 20.0]);
        set.draw(&duplex, [lo[0] + 0.09, 6.0], 1);
        set.draw(&duplex, [x0 + 10.0, hi[1] - 0.09], 0);
        set.draw(&duplex, [hi[0] - 0.09, 12.0], 3);
        set.text([lo[0] + 0.03, 9.0], "S");
        set.text([x0 + 12.0, hi[1] - 0.15], "S3");
        set.text([x0 + 15.0, 11.0], "S");
    });
}

#[test]
fn a_set_with_no_legend_gets_its_receptacles_and_switches_from_the_shared_library() {
    let mut set = Set::new();
    library_plan(&mut set);
    let found = set.search(&shipped(), None);
    let mut got: Vec<String> = found.added.iter().map(|dv| format!("{} {}", dv.kind, dv.kind_source)).collect();
    got.sort();
    assert_eq!(
        got,
        [
            "3way_switch symbol library: three way switch",
            "receptacle symbol library: duplex receptacle",
            "receptacle symbol library: duplex receptacle",
            "receptacle symbol library: duplex receptacle",
            "switch symbol library: single pole switch",
        ],
        "{:#?}",
        found.added
    );
    assert!(found.added.iter().all(|dv| dv.derived.starts_with("derived: ") && dv.symbol.as_ref().is_some_and(|y| y.status == "derived")));
    assert_eq!(found.uncued, 1);
    // The table is in the drawing: the library's entries, derived.
    let t = symtable::stored(set.s.doc().unwrap());
    assert!(t.get("library:duplex-receptacle").is_some_and(|e| e.status() == "derived" && !e.off));
}

/// Turning a symbol off for one set removes its devices there (logged, revertible) and leaves
/// another set as it was.
#[test]
fn turning_a_symbol_off_on_one_set_leaves_another_set_unchanged() {
    let mut one = Set::new();
    library_plan(&mut one);
    assert_eq!(one.search(&shipped(), None).added.iter().filter(|dv| dv.kind == "receptacle").count(), 3);
    let r = one
        .s
        .execute("bim.symbol.set", &json!({"action": "off", "id": "library:duplex-receptacle", "reason": "the bars are a wall's hatch here"}))
        .unwrap();
    let change = r["change"].as_u64().unwrap();
    let off = one.search(&shipped(), None);
    assert_eq!(off.added.iter().filter(|dv| dv.kind == "receptacle").count(), 0, "{:#?}", off.added);
    assert_eq!(off.added.iter().filter(|dv| dv.kind.contains("switch")).count(), 2);
    let log = one.s.execute("bim.symbol.log", &json!({"id": "library:duplex-receptacle"})).unwrap();
    assert!(log["changes"].as_array().is_some_and(|c| c.iter().any(|x| x["action"] == "off" && x["who"] == "user")), "{log}");
    let mut two = Set::new();
    library_plan(&mut two);
    assert_eq!(two.search(&shipped(), None).added.iter().filter(|dv| dv.kind == "receptacle").count(), 3);
    // Reverted: back.
    one.s.execute("bim.symbol.revert", &json!({"change": change})).unwrap();
    assert_eq!(one.search(&shipped(), None).added.iter().filter(|dv| dv.kind == "receptacle").count(), 3);
}

/// A set's legend that draws a receptacle its own odd way, gives the library's duplex drawing
/// another meaning, and lists a kind the library doesn't draw: each found by the set's drawing;
/// the receptacles shown with the library's duplex (the print's drawing kept beside it), the
/// card reader with its own.
#[test]
fn a_sets_own_drawings_win_and_universal_kinds_are_shown_with_the_librarys_symbol() {
    let duplex = lib_strokes("duplex receptacle");
    let odd: Vec<Stroke> =
        vec![Stroke::Round([0.0, 0.0, 0.06]), seg(-0.09, 0.03, 0.09, 0.03), seg(-0.09, 0.0, 0.09, 0.0), seg(-0.09, -0.03, 0.09, -0.03)];
    let mut set = Set::new();
    set.sheet("E-001", "legend", true, |set, x0| {
        set.legend(x0, &[("DUPLEX RECEPTACLE", odd.clone()), ("TELEVISION OUTLET", duplex.clone()), ("CARD READER", card_reader())]);
    });
    set.sheet("E-101", "plan", true, |set, x0| {
        let (lo, hi) = set.room([x0 + 2.0, 2.0], [x0 + 30.0, 20.0]);
        set.draw(&odd, [lo[0] + 0.09, 6.0], 1);
        set.draw(&odd, [hi[0] - 0.09, 8.0], 1);
        set.draw(&duplex, [x0 + 10.0, hi[1] - 0.09], 0);
        set.draw(&card_reader(), [x0 + 14.0, 10.0], 0);
    });
    let found = set.search(&shipped(), None);
    let mut got: Vec<String> = found
        .added
        .iter()
        .map(|dv| {
            let y = dv.symbol.clone().unwrap_or_default();
            format!("{} | {} | {}", dv.kind, y.drawn_as, y.shown_as)
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            "low_voltage_device | CARD READER | ",
            "receptacle | DUPLEX RECEPTACLE | duplex receptacle",
            "receptacle | DUPLEX RECEPTACLE | duplex receptacle",
            "tv_outlet | TELEVISION OUTLET | ",
        ],
        "{:#?}",
        found.added
    );
    let odd_sig = symtable::signature(&odd);
    assert!(found.added.iter().filter(|dv| dv.kind == "receptacle").all(|dv| dv.symbol.as_ref().is_some_and(|y| y.as_drawn == odd_sig)));
    let t = symtable::stored(set.s.doc().unwrap());
    assert!(
        t.get("library:duplex-receptacle").is_some_and(|e| e.off && e.off_reason.contains("another meaning")),
        "{:?}",
        t.get("library:duplex-receptacle")
    );
    // Built into the model, each receptacle says both.
    let mut s2 = crate::Session::new();
    let sp = cadaddy_walk3d::BuildingSpec { units_per_foot: 12.0, ..Default::default() };
    s2.doc_mut().unwrap().building_spec = Some(sp.to_json()).into();
    let recs: Vec<super::super::readers::ReadDevice> = found.added.iter().filter(|dv| dv.kind == "receptacle").cloned().collect();
    super::super::readers::place_devices(&mut s2, &recs, "test").unwrap();
    let sp = cadaddy_walk3d::BuildingSpec::from_json(s2.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    let tags: Vec<&cadaddy_walk3d::tags::ElementTag> = sp.tags.iter().filter(|t| t.id.starts_with("device:")).collect();
    assert_eq!(tags.len(), 2);
    for t in tags {
        assert_eq!(t.fields.get("symbol_shown_as").map(|f| f.value.as_str()), Some("duplex receptacle"));
        assert_eq!(t.fields.get("symbol_as_drawn").map(|f| f.value.as_str()), Some(odd_sig.as_str()));
        assert!(
            t.fields.get("symbol_note").is_some_and(|f| f.value == "drawn on the print as DUPLEX RECEPTACLE, shown as duplex receptacle"),
            "{:?}",
            t.fields
        );
    }
}

/// Test B: a symbol no legend row or library symbol is, drawn four times: one question; its
/// answer classifies all four, and its drawing goes into the shared library; a second set with
/// no legend row for it gets it from there as a derived proposal.
#[test]
fn an_unexplained_repeating_symbol_is_asked_once_and_its_answer_carries_to_the_next_set() {
    let dir = std::env::temp_dir().join(format!("cadaddy-q6484-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut set = Set::new();
    set.sheet("E-101", "plan", true, |set, x0| {
        set.room([x0 + 2.0, 2.0], [x0 + 30.0, 20.0]);
        for (i, k) in [0u8, 1, 2, 3].iter().enumerate() {
            set.draw(&card_reader(), [x0 + 6.0 + 4.0 * i as f64, 10.0], *k);
        }
    });
    let found = set.search(&shipped(), Some(&dir));
    assert!(found.added.is_empty(), "{:#?}", found.added);
    assert_eq!(found.asked, 1);
    let q = set.job.stages.iter().flat_map(|st| &st.questions).find(|q| q.question.starts_with(SYMBOL_QUESTION)).cloned();
    let Some(mut q) = q else { panic!("no symbol question: {:?}", set.job.stages) };
    assert!(q.question.contains("4 times") && q.question.contains("4 on E-101"), "{}", q.question);
    assert!(q.choices.iter().any(|c| c == NOT_A_DEVICE));
    // Asked once: the next build doesn't ask again.
    assert_eq!(set.search(&shipped(), Some(&dir)).asked, 0);
    // Answered: all four are card readers on this set, and the drawing is in the library.
    q.answer = Some(json!({"kind": "card_reader", "name": "card reader"}));
    q.answered_by = Some("agent".into());
    learn_from_answer(&mut set.job, &q);
    assert_eq!(answer_effect(&mut set.job, &q).0, Some(StageName::BimBuild));
    let after = set.search(&shipped(), Some(&dir));
    assert_eq!(after.added.iter().filter(|dv| dv.kind == "card_reader").count(), 4, "{:#?}", after.added);
    assert!(after.added.iter().all(|dv| dv.kind_source == format!("answer {}", q.id) && dv.derived.is_empty()));
    let lib = symtable::library(Some(&dir));
    assert!(lib.iter().any(|(s, learned)| *learned && s.kind == "card_reader" && s.sets == 1), "{lib:?}");
    // The next set, no legend row for it: a derived proposal from the library.
    let mut next = Set::new();
    next.sheet("E-201", "plan", true, |set, x0| {
        let (lo, _) = set.room([x0 + 2.0, 2.0], [x0 + 30.0, 20.0]);
        for (i, k) in [0u8, 3, 1].iter().enumerate() {
            set.draw(&card_reader(), [x0 + 6.0 + 4.0 * i as f64, lo[1] + 0.07], *k);
        }
    });
    let carried = next.search(&lib, None);
    assert_eq!(carried.added.iter().filter(|dv| dv.kind == "card_reader").count(), 3, "{:#?}", carried.added);
    assert!(
        carried.added.iter().all(|dv| dv.kind_source == "symbol library: card reader" && dv.symbol.as_ref().is_some_and(|y| y.status == "derived"))
    );
    assert_eq!(carried.asked, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Test C (the part a unit test reaches): the typical rooms' views are their own reading's, so a
/// symbol drawn in a typical view is never found again by the plan search; the same symbol on the
/// floor plan beside the view is.
#[test]
fn a_typical_views_symbols_are_left_to_the_typical_reading() {
    let mut set = Set::new();
    set.sheet("E-001", "legend", true, |set, x0| set.legend(x0, &[("SPECIAL PURPOSE RECEPTACLE", special())]));
    let mut view = [0.0; 4];
    set.sheet("E-101", "plan", true, |set, x0| {
        let (lo, _) = set.room([x0 + 2.0, 2.0], [x0 + 30.0, 20.0]);
        set.draw(&special(), [lo[0] + 0.06, 6.0], 0);
        set.draw(&special(), [lo[0] + 0.06, 16.0], 0);
        view = [x0 + 1.0, 14.0, x0 + 8.0, 19.0];
    });
    let pages = legend_pages(set.s.doc().unwrap(), &set.job);
    let mut typical = TypicalMarks::default();
    typical.views.insert(sheet_key("E-101"), vec![view]);
    let found = plan_symbols(&mut set.s, &mut set.job, &pages, &typical, &shipped(), None).unwrap();
    assert_eq!(found.added.len(), 1, "{:#?}", found.added);
    assert!(found.added.iter().all(|dv| dv.at[1] < 10.0));
}

/// Hostile sizes: a plan of a great many tiny pieces, a template drawn at no size, a sheet of
/// no width: bounded, no panic.
#[test]
fn hostile_plans_and_tables_do_not_panic() {
    let mut set = Set::new();
    set.sheet("E-101", "plan", true, |set, x0| {
        for i in 0..3_000 {
            let x = x0 + 1.0 + (i % 60) as f64 * 0.3;
            let y = 1.0 + (i / 60) as f64 * 0.3;
            set.line([x, y], [x + 1e-6, y]);
            set.line([x, y], [x + 0.01, y + 0.01]);
        }
    });
    let mut t = symtable::stored(set.s.doc().unwrap());
    t.entries.push(symtable::Entry {
        id: "user:zero".into(),
        kind: "receptacle".into(),
        strokes: vec![Stroke::Round([0.0, 0.0, 0.0]), Stroke::Seg([f64::NAN, 0.0, 1.0, 1.0]), Stroke::Seg([0.0, 0.0, 1e300, 1e300])],
        text_h: f64::INFINITY,
        sheet_w: Some(0.0),
        origin: "user".into(),
        ..symtable::Entry::default()
    });
    symtable::keep(set.s.doc_mut().unwrap(), &t).unwrap();
    let found = set.search(&shipped(), None);
    assert!(found.added.len() <= 3_000);
    // A table entry that names no entry: refused, not a panic.
    assert!(set.s.execute("bim.symbol.set", &json!({"action": "off", "id": "nothing"})).is_err());
    assert!(set.s.execute("bim.symbol.set", &json!({"action": "fly"})).is_err());
    assert!(set.s.execute("bim.symbol.revert", &json!({"change": 999_999})).is_err());
}

/// The hotel's first run (Q6484): an electrical legend speaks for the electrical plans, not a
/// mechanical plan's look-alikes; a sheet printed as several views is searched whole; the
/// second line of a wrapped note (`FIXTURE TO SWITCH LEG "A")`) is no legend row of its own.
#[test]
fn only_the_legends_own_trade_plans_are_searched_whole_and_wrapped_notes_are_no_rows() {
    let mut set = Set::new();
    set.sheet("E-001", "legend", true, |set, x0| {
        set.legend(x0, &[("SPECIAL PURPOSE RECEPTACLE", special()), ("LOWER CASE LETTERING INDICATES SWITCHING (I.E. CONNECT LIGHT", Vec::new())]);
        // The note's second line, with a letter drawn as strokes in its symbol cell.
        let y = 19.7 - 0.3 - 1.4 * TEXT_H;
        set.text([x0 + 1.6, y], "FIXTURE TO SWITCH LEG \"A\")");
        set.draw(&hexagon(), [x0 + 1.2, y + 0.04], 0);
    });
    set.sheet("M-101", "plan", true, |set, x0| {
        let (lo, _) = set.room([x0 + 2.0, 2.0], [x0 + 30.0, 20.0]);
        set.draw(&special(), [lo[0] + 0.06, 6.0], 0);
        set.draw(&hexagon(), [x0 + 10.0, 10.0], 0);
    });
    set.retag("M", &[]);
    set.sheet("E-102", "plan", true, |set, x0| {
        let (lo, hi) = set.room([x0 + 2.0, 2.0], [x0 + 26.0, 20.0]);
        set.draw(&special(), [lo[0] + 0.06, 6.0], 0);
        set.draw(&special(), [hi[0] - 0.06, 6.0], 2);
        set.draw(&hexagon(), [x0 + 10.0, 10.0], 0);
        // A mounting detail beside the plan: its box, a wall and a receptacle in it, its title.
        let (dlo, _) = set.room([x0 + 28.0, 14.0], [x0 + 34.0, 20.0]);
        set.draw(&special(), [dlo[0] + 0.06, 17.0], 0);
        set.s.execute("text", &json!({"at": [x0 + 28.5, 13.4], "text": "RECEPTACLE MOUNTING DETAIL", "height": 0.18})).unwrap();
        set.s.execute("text", &json!({"at": [x0 + 28.5, 13.1], "text": "SCALE: NTS", "height": 0.1})).unwrap();
    });
    let x0 = set.next_x - SHEET_W - 4.0;
    set.retag("E", &[[x0, 0.0, x0 + 18.0, SHEET_H], [x0 + 18.0, 0.0, x0 + SHEET_W, SHEET_H]]);
    let found = set.search(&shipped(), None);
    let got: Vec<String> = found.added.iter().map(|dv| format!("{} {}", dv.sheet, dv.kind)).collect();
    assert_eq!(got, ["E-102 receptacle", "E-102 receptacle"], "{:#?}", found.added);
    // Each records where its print draws it.
    assert!(found.added.iter().all(|dv| dv.symbol.as_ref().is_some_and(|y| y.drawn_at == dv.at)));
    let t = symtable::stored(set.s.doc().unwrap());
    assert!(t.entries.iter().all(|e| !e.name.contains("SWITCH LEG")), "{:?}", t.entries.iter().map(|e| &e.name).collect::<Vec<_>>());
}

/// A small set's electrical legend on a combined MEP sheet speaks for its combined MEP plans.
#[test]
fn a_combined_mep_sheets_legend_and_plans_are_read() {
    let mut set = Set::new();
    set.sheet("MEP0.01", "legend", true, |set, x0| set.legend(x0, &[("SPECIAL PURPOSE RECEPTACLE", special())]));
    set.retag("MEP", &[]);
    set.sheet("MEP1.01", "plan", true, |set, x0| {
        let (lo, hi) = set.room([x0 + 2.0, 2.0], [x0 + 30.0, 20.0]);
        set.draw(&special(), [lo[0] + 0.06, 6.0], 0);
        set.draw(&special(), [hi[0] - 0.06, 9.0], 2);
    });
    set.retag("MEP", &[]);
    let found = set.search(&shipped(), None);
    let got: Vec<String> = found.added.iter().map(|dv| format!("{} {} {}", dv.sheet, dv.kind, dv.kind_source)).collect();
    assert_eq!(
        got,
        ["MEP1.01 receptacle MEP0.01 legend: SPECIAL PURPOSE RECEPTACLE", "MEP1.01 receptacle MEP0.01 legend: SPECIAL PURPOSE RECEPTACLE"],
        "{:#?}",
        found.added
    );
}
