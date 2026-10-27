//! The field viewer (Q312) end to end: a drawing's model built and exported as a field
//! package, opened on a tablet and on a phone, a room found by its number, a duct tapped for its
//! type and source, a trade hidden, a measure, a note made offline that survives a reload, and
//! the notes synced back into the drawing once.
//!
//! `CADADDY_FIELD_SHOTS=<dir>` saves the rendered views there (to look at).

use cadaddy_engine::Session;
use cadaddy_ui_egui::fieldview::{FieldViewer, Mode, Tool};
use cadaddy_ui_egui::theme;
use cadaddy_walk3d::field::{FieldCheck, FieldCover, FieldNotes, FieldPackage, FieldPhoto, NoteKind};
use egui_kittest::Harness;
use serde_json::json;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

/// Two guest rooms (214, 215) off a corridor, a supply duct over both, a cold water pipe, a
/// conduit; the duct typed.
fn drawing() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [312.0, 360.0]);
    rect(&mut s, [312.0, 0.0], [624.0, 360.0]);
    rect(&mut s, [0.0, 360.0], [624.0, 432.0]);
    layer(&mut s, "A-ANNO");
    s.execute("text", &json!({"at": [120, 180], "text": "King", "height": 6})).unwrap();
    s.execute("text", &json!({"at": [430, 180], "text": "Double Queen", "height": 6})).unwrap();
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[24, 300], [580, 300], [580, 400]]})).unwrap();
    s.execute("text", &json!({"at": [200, 306], "text": "20x10 BOT @ 7'-6\"", "height": 4})).unwrap();
    layer(&mut s, "P-DOMW-CW");
    s.execute("line", &json!({"points": [[100, 20], [100, 340]]})).unwrap();
    s.execute("text", &json!({"at": [104, 60], "text": "1\" CW CL @ 8'-6\"", "height": 4})).unwrap();
    layer(&mut s, "E-COND");
    s.execute("line", &json!({"points": [[330, 40], [600, 40]]})).unwrap();
    s.execute("text", &json!({"at": [400, 46], "text": "3/4\" C BOT @ 8'-0\"", "height": 4})).unwrap();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "Level 2", "elevation_ft": 12, "height_ft": 11, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "rooms": [
                {"name": "King", "number": "214", "level": "Level 2", "ceiling_ft": 9},
                {"name": "Double Queen", "number": "215", "level": "Level 2", "ceiling_ft": 9}
            ],
            "rcp": {"ceilings": [
                {"level": "Level 2", "room": "214", "outline": [[0, 0], [312, 0], [312, 360], [0, 360]], "height_ft": 9},
                {"level": "Level 2", "room": "215", "outline": [[312, 0], [624, 0], [624, 360], [312, 360]], "height_ft": 9}
            ]}
        }}),
    )
    .unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let duct = s.doc().unwrap().model.iter().find(|e| e.common.layer == "M-DUCT-SUPP" && e.common.xdata.is_empty()).unwrap().handle;
    s.execute(
        "bim.set_spec",
        &json!({"handles": [duct.hex()], "mark": "SD-1", "create": {"category": "duct", "fields": {
            "material": {"value": "galvanized steel", "source": "M-001 general note 4", "confidence": "stated"},
            "insulation": {"value": "1-1/2\" duct wrap", "source": "23 07 13", "confidence": "inferred"}
        }}}),
    )
    .unwrap();
    s
}

fn package(s: &mut Session) -> Vec<u8> {
    let r = s.execute("fieldview.export", &json!({"inline": true, "project": "Sample Hotel"})).unwrap();
    serde_json::to_vec(&r["package"]).unwrap()
}

fn harness(v: FieldViewer, size: egui::Vec2) -> Harness<'static, FieldViewer> {
    let mut h = Harness::builder().with_size(size).wgpu().build_ui_state(
        |ui, v: &mut FieldViewer| {
            theme::apply(ui.ctx(), egui::Theme::Dark, false);
            v.ui(ui);
        },
        v,
    );
    h.run_steps(3);
    h
}

fn shot(h: &mut Harness<'_, FieldViewer>, name: &str) {
    if let Some(dir) = std::env::var_os("CADADDY_FIELD_SHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join(name)).unwrap();
    }
}

#[test]
fn a_room_found_tapped_measured_and_noted_offline_syncs_back_once() {
    let mut s = drawing();
    let bytes = package(&mut s);
    let mut v = FieldViewer::new("tab1");
    v.open(&bytes).unwrap();
    let p = v.pkg.as_ref().unwrap();
    assert_eq!(p.rooms.len(), 2);
    assert!(p.trades.iter().any(|t| t.key == "mech") && p.trades.iter().any(|t| t.key == "plumb") && p.trades.iter().any(|t| t.key == "elec"));

    // Room number search, then go there.
    let hits = p.search_rooms("215");
    let room = hits.first().unwrap().0;
    v.select_room(room);
    assert_eq!(v.level_name(), "Level 2");
    assert_eq!(v.room, Some(room));

    // Tap the duct in plan: its type, spec fields and their sources.
    v.tap_plan([40.0, 25.0], 0.5);
    let e = v.selected.and_then(|i| v.pkg.as_ref().unwrap().elements.get(i)).unwrap().clone();
    assert_eq!((e.kind.as_str(), e.mark.as_str()), ("duct", "SD-1"));
    let ty = v.pkg.as_ref().unwrap().type_of(&e).unwrap();
    assert!(ty.fields.iter().any(|f| f.key == "insulation" && f.source == "23 07 13" && f.confidence == "inferred"));

    // Hide mechanical: the duct can't be tapped and the room's view loses it.
    let before = v.visible().len();
    v.hidden.insert("mech".into());
    assert!(v.visible().len() < before);
    v.tap_plan([40.0, 25.0], 0.5);
    assert!(v.selected.is_none_or(|i| v.pkg.as_ref().unwrap().elements.get(i).is_some_and(|e| e.trade != "mech")));
    v.hidden.clear();

    // Measure across the room.
    v.tool = Tool::Measure;
    v.tap_plan([26.0, 2.0], 0.5);
    v.tap_plan([26.0, 28.0], 0.5);
    assert!(v.status.contains("26'-0\""), "{}", v.status);

    // A verified point at the duct, offline.
    v.tool = Tool::Select;
    v.tap_plan([40.0, 25.0], 0.5);
    v.tool = Tool::Note;
    v.tap_plan([40.0, 25.0], 0.5);
    let draft = v.draft.as_mut().unwrap();
    draft.kind = NoteKind::VerifiedPoint;
    draft.text = "duct bottom measured at 7'-4\" AFF".into();
    let id = v.save_draft("2026-05-06T07:30:00Z").unwrap();
    assert_eq!(id, "FN-tab1-0001");
    let note = v.pkg.as_ref().unwrap().notes.last().unwrap().clone();
    assert_eq!((note.room.as_str(), note.element.as_str()), ("215", e.id.as_str()));
    assert!(v.dirty);

    // The device reloads with no connection: everything comes back from its storage.
    let stored = v.to_storage().unwrap();
    let mut again = FieldViewer::new("tab1");
    again.from_storage(&stored).unwrap();
    assert_eq!(again.pkg.as_ref().unwrap().unsynced().len(), 1);
    // A fresh package of the same project keeps the notes not sent yet.
    again.open(&bytes).unwrap();
    assert_eq!(again.pkg.as_ref().unwrap().unsynced().first().map(|n| n.id.as_str()), Some("FN-tab1-0001"));

    // Send Notes, then sync into the drawing — twice is the same as once.
    assert_eq!(again.send_notes(), 1);
    // Handed over, not in the drawing yet: sent, still unsynced until a package brings it back.
    let sent = again.pkg.as_ref().unwrap().unsynced();
    assert_eq!(sent.iter().map(|n| (n.id.as_str(), n.sent, n.synced)).collect::<Vec<_>>(), [("FN-tab1-0001", true, false)]);
    let (name, file) = again.outbox.pop().unwrap();
    assert_eq!(name, "Sample_Hotel-fieldnotes-tab1.json");
    let notes: FieldNotes = serde_json::from_slice(&file).unwrap();
    let r = s.execute("fieldview.sync", &json!({"notes": notes})).unwrap();
    assert_eq!(r["added"], 1);
    let r = s.execute("fieldview.sync", &json!({"notes": notes})).unwrap();
    assert_eq!((r["added"].as_u64(), r["unchanged"].as_u64()), (Some(0), Some(1)));
    let listed = s.execute("fieldview.notes", &json!({"room": "215"})).unwrap();
    assert_eq!(listed["notes"][0]["kind"], "verified_point");
    // The next package brings the note to every device, already sent.
    let mut next = FieldViewer::new("phone2");
    next.open(&package(&mut s)).unwrap();
    assert_eq!(next.pkg.as_ref().unwrap().notes.len(), 1);
    assert!(next.pkg.as_ref().unwrap().unsynced().is_empty());
}

/// Q5111: Send Notes only marks notes sent. A lost download is sent again with the same id, and
/// a note is synced only once a package exported from the drawing carries it back.
#[test]
fn a_lost_notes_download_is_sent_again_until_a_package_brings_the_note_back() {
    let mut s = drawing();
    let mut v = FieldViewer::new("tab1");
    v.open(&package(&mut s)).unwrap();
    v.tool = Tool::Note;
    v.tap_plan([40.0, 25.0], 0.5);
    v.draft.as_mut().unwrap().text = "sleeve 6\" east of plan".into();
    let id = v.save_draft("2026-05-06T09:00:00Z").unwrap();
    assert_eq!(id, "FN-tab1-0001");
    let ids_in = |file: &[u8]| -> Vec<String> {
        let notes: FieldNotes = serde_json::from_slice(file).unwrap();
        notes.notes.iter().map(|n| n.id.clone()).collect()
    };

    assert_eq!(v.send_notes(), 1);
    assert_eq!(v.outbox.len(), 1);
    // The download fails: the file never reaches the office.
    v.outbox.clear();
    let note = v.pkg.as_ref().unwrap().notes.iter().find(|n| n.id == id).unwrap().clone();
    assert!(note.sent && !note.synced, "{note:?}");

    // Send Notes again: the same note goes out again.
    assert_eq!(v.send_notes(), 1);
    let (_, file) = v.outbox.pop().unwrap();
    assert_eq!(ids_in(&file), vec![id.clone()]);
    let notes: FieldNotes = serde_json::from_slice(&file).unwrap();
    assert!(notes.notes.iter().all(|n| !n.sent && !n.synced));
    // A reload keeps it sent and unsynced.
    let mut v = {
        let mut r = FieldViewer::new("tab1");
        r.from_storage(&v.to_storage().unwrap()).unwrap();
        r
    };
    assert_eq!(v.pkg.as_ref().unwrap().unsynced().iter().map(|n| (n.id.as_str(), n.sent)).collect::<Vec<_>>(), [(id.as_str(), true)]);

    // The office syncs that file twice: added once.
    let r = s.execute("fieldview.sync", &json!({"notes": notes})).unwrap();
    assert_eq!((r["added"].as_u64(), r["updated"].as_u64()), (Some(1), Some(0)));
    let r = s.execute("fieldview.sync", &json!({"notes": notes})).unwrap();
    assert_eq!((r["added"].as_u64(), r["unchanged"].as_u64()), (Some(0), Some(1)));

    // A fresh export after the sync carries it back: the note is synced and nothing is left to send.
    v.open(&package(&mut s)).unwrap();
    let p = v.pkg.as_ref().unwrap();
    assert_eq!(p.notes.iter().filter(|n| n.id == id).count(), 1);
    assert!(p.notes.iter().any(|n| n.id == id && n.synced));
    assert!(p.unsynced().is_empty());
    assert_eq!(v.send_notes(), 0);
    assert!(v.outbox.is_empty());
}

/// A package that does not carry a sent note (exported before the office synced it) leaves the
/// note sent and unsynced on the device.
#[test]
fn a_package_without_the_note_does_not_acknowledge_it() {
    let mut s = drawing();
    let before_sync = package(&mut s);
    let mut v = FieldViewer::new("tab1");
    v.open(&before_sync).unwrap();
    v.tool = Tool::Note;
    v.tap_plan([40.0, 25.0], 0.5);
    v.draft.as_mut().unwrap().text = "box at 18\" AFF".into();
    let id = v.save_draft("2026-05-06T09:10:00Z").unwrap();
    assert_eq!(v.send_notes(), 1);
    v.outbox.clear();
    v.open(&before_sync).unwrap();
    let p = v.pkg.as_ref().unwrap();
    assert_eq!(p.unsynced().iter().map(|n| (n.id.as_str(), n.sent, n.synced)).collect::<Vec<_>>(), [(id.as_str(), true, false)]);
    assert_eq!(v.send_notes(), 1);
}

#[test]
fn tablet_and_phone_views_render() {
    let mut s = drawing();
    let bytes = package(&mut s);
    let mut v = FieldViewer::new("tab1");
    v.open(&bytes).unwrap();
    let room = v.pkg.as_ref().unwrap().search_rooms("214").first().unwrap().0;
    v.select_room(room);
    v.tap_plan([15.0, 25.0], 0.5);
    assert!(v.selected.is_some());
    v.measure = vec![[2.0, 2.0, 12.0], [24.0, 2.0, 12.0]];

    // Tablet, plan.
    let mut h = harness(v, egui::vec2(1180.0, 820.0));
    shot(&mut h, "field-tablet-plan.png");
    // Tablet, 3D of the room.
    h.state_mut().mode = Mode::Model;
    h.state_mut().measure.clear();
    h.run_steps(2);
    assert!(h.state().drawn_3d > 3, "{}", h.state().drawn_3d);
    shot(&mut h, "field-tablet-3d.png");
    // A tap in 3D at the centre picks something (the camera looks at the room).
    h.state_mut().tool = Tool::Select;
    // Walk, standing in the room.
    h.state_mut().mode = Mode::Walk;
    h.state_mut().heading = 90.0;
    h.run_steps(2);
    shot(&mut h, "field-tablet-walk.png");
    let v = h.into_state();

    // Phone: the details in a bottom sheet.
    let mut h = harness(v, egui::vec2(390.0, 844.0));
    h.state_mut().mode = Mode::Model;
    h.run_steps(2);
    shot(&mut h, "field-phone-3d.png");
    h.state_mut().mode = Mode::Plan;
    h.state_mut().plan = None;
    h.run_steps(2);
    shot(&mut h, "field-phone-plan.png");
    // Nothing loaded yet: the open prompt.
    let mut h = harness(FieldViewer::new("tab1"), egui::vec2(390.0, 844.0));
    shot(&mut h, "field-phone-empty.png");
    h.state_mut().open(b"{\"schema\":\"cadaddy-field/99\"}").unwrap_err();
}

/// Q5119: tapping a wall shows the face on the tapped side, with its pre-cover checklist and the
/// photos taken behind it (the package carries them from the drawing's cover record).
#[test]
fn a_wall_tap_lists_the_tapped_faces_checklist_and_photos() {
    let mut s = drawing();
    let mut pkg: FieldPackage = serde_json::from_slice(&package(&mut s)).unwrap();
    // The wall between room 214 and the corridor, west to east along y = 30'.
    let face = |f: &str, rooms: &[&str], photos: Vec<FieldPhoto>| FieldCover {
        area: format!("W2-001/{f}"),
        wall: "W2-001".into(),
        face: f.into(),
        level: "Level 2".into(),
        a: [0.0, 30.0],
        b: [26.0, 30.0],
        rooms: rooms.iter().map(|r| r.to_string()).collect(),
        fire_rating: "1 HR".into(),
        ready: false,
        checklist: vec![
            FieldCheck { id: "c1".into(), category: "boxes".into(), description: "Boxes installed and secured".into(), done: true },
            FieldCheck { id: "c2".into(), category: "firestop".into(), description: "Firestop at rated penetrations".into(), done: false },
        ],
        photos,
    };
    let photo = FieldPhoto {
        id: "FN-ph1-0002".into(),
        uri: "IMG_0002.jpg".into(),
        caption: "boxes before drywall".into(),
        elevation_ft: 3.0,
        along_ft: 12.0,
        ..FieldPhoto::default()
    };
    pkg.cover = vec![face("A", &[], Vec::new()), face("B", &["214"], vec![photo])];
    let mut v = FieldViewer::new("tab1");
    v.open(pkg.to_json().unwrap().as_bytes()).unwrap();
    let room = v.pkg.as_ref().unwrap().search_rooms("214").first().unwrap().0;
    v.select_room(room);
    // South of the wall (inside 214): face B.
    v.tap_plan([12.0, 29.7], 0.5);
    let c = v.cover.and_then(|i| v.pkg.as_ref().unwrap().cover.get(i)).unwrap();
    assert_eq!((c.area.as_str(), c.photos.len()), ("W2-001/B", 1));
    // North: face A, no photos. Far from the wall: no face.
    v.tap_plan([12.0, 30.3], 0.5);
    assert_eq!(v.cover.map(|i| v.pkg.as_ref().unwrap().cover[i].area.clone()).as_deref(), Some("W2-001/A"));
    v.tap_plan([12.0, 15.0], 0.5);
    assert!(v.cover.is_none());
    v.tap_plan([12.0, 29.7], 0.5);
    // The details panel lists the checklist and the photo.
    let mut h = harness(v, egui::vec2(1180.0, 820.0));
    h.get_by_label("Firestop at rated penetrations");
    h.get_by_label("FN-ph1-0002: boxes before drywall (IMG_0002.jpg, 3'-0\")");
    h.get_by_label("Not ready to cover");
    shot(&mut h, "field-tablet-wall.png");
}
