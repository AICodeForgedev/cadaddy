//! WBLOCK at the command line (AutoCAD's -WBLOCK prompts), and `.dwg` added to a bare file name.

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::{IoHooks, set_io};
use serde_json::json;

fn install() {
    set_io(IoHooks {
        read: |b, n| cadcraft_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadcraft_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

#[test]
fn typed_wblock_writes_new_objects_a_block_and_the_whole_drawing() {
    install();
    let dir = std::env::temp_dir().join(format!("cadaddy-wblock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new();
    s.execute("circle", &json!({"center": [10, 10], "radius": 2})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [5, 0]]})).unwrap();

    // New objects: base point, then a selection (ALL), Enter.
    let part = dir.join("part");
    for i in ["wblock", &part.to_string_lossy(), "", "10,10", "all", ""] {
        s.cmdline(i).unwrap();
    }
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let d = cadcraft_io::read(&std::fs::read(dir.join("part.dwg")).unwrap(), "part.dwg").unwrap();
    assert_eq!(d.model.iter().count(), 2);

    // The whole drawing.
    let whole = dir.join("whole.dxf");
    for i in ["wblock", &whole.to_string_lossy(), "*"] {
        s.cmdline(i).unwrap();
    }
    assert!(whole.exists() && s.running.is_none());
    std::fs::remove_dir_all(&dir).ok();
}
