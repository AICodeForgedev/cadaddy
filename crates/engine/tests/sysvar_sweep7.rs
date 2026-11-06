//! SETVAR on every value the variable sweep found accepted (battle test 7, sysvar6): AutoCAD-legal
//! ones still go through, the rest are refused with the values the variable takes, and a refused
//! value leaves the variable as it was. MCP `set_variable`, typed SETVAR and LISP all reach `setvar`.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.new_drawing(false);
    s
}

fn get(s: &mut Session, name: &str) -> Value {
    s.execute("getvar", &json!({ "name": name })).unwrap_or_else(|e| panic!("{name}: {e}"))["value"].clone()
}

fn set(s: &mut Session, name: &str, v: &Value) -> Result<(), String> {
    s.execute("setvar", &json!({ "name": name, "value": v })).map(|_| ()).map_err(|e| e.to_string())
}

/// The refusal of `v` for `name` (a panic when it is taken).
fn refused(s: &mut Session, name: &str, v: &Value) -> String {
    match set(s, name, v) {
        Ok(()) => panic!("{name} took {v}"),
        Err(e) => e,
    }
}

/// Every value the sweep's set_variable took (sweep/sysvar6.jsonl, accepted rows): variable,
/// value, whether AutoCAD takes it too.
#[rustfmt::skip]
fn sweep_accepted() -> Vec<(&'static str, Value, bool)> {
    vec![
        ("3DOSMODE", json!(999999), false),
        ("ANGBASE", json!(-1.0), true),
        ("ANGBASE", json!(0.0), true),
        ("ANGBASE", json!(1e+300), false),
        ("ANGBASE", json!(-1e+300), false),
        ("ANNOAUTOSCALE", json!(-1), true),
        ("ANNOMONITOR", json!(-1), true),
        ("CAMERAHEIGHT", json!(-1.0), true),
        ("CAMERAHEIGHT", json!(0.0), true),
        ("CAMERAHEIGHT", json!(1e+300), false),
        ("CAMERAHEIGHT", json!(-1e+300), false),
        ("CELWEIGHT", json!(-1), true),
        ("CENTEREXE", json!(0.0), true),
        ("CETRANSPARENCY", json!(""), false),
        ("CETRANSPARENCY", json!("zz<>|*?"), false),
        ("CETRANSPARENCY", json!("nonexistent_xyz"), false),
        ("CETRANSPARENCY", json!(12345), false),
        ("CHAMFERA", json!(0.0), true),
        ("CHAMFERB", json!(0.0), true),
        ("CHAMFERC", json!(0.0), true),
        ("CHAMFERD", json!(-1.0), true),
        ("CHAMFERD", json!(0.0), true),
        ("CHAMFERD", json!(1e+300), false),
        ("CHAMFERD", json!(-1e+300), false),
        ("CIRCLERAD", json!(0.0), true),
        ("CMLSCALE", json!(-1.0), true),
        ("CMLSCALE", json!(0.0), true),
        ("CMLSCALE", json!(1e+300), false),
        ("CMLSCALE", json!(-1e+300), false),
        ("DCTCUST", json!(""), true),
        ("DCTCUST", json!("zz<>|*?"), false),
        ("DCTCUST", json!("nonexistent_xyz"), true),
        ("DELOBJ", json!(-1), true),
        ("DIMADEC", json!(-1), true),
        ("DIMALT", json!(2), false),
        ("DIMALT", json!(-1), false),
        ("DIMALTRND", json!(0.0), true),
        ("DIMAPOST", json!(""), true),
        ("DIMAPOST", json!("zz<>|*?"), true),
        ("DIMAPOST", json!("nonexistent_xyz"), true),
        ("DIMAPOST", json!(12345), true),
        ("DIMASZ", json!(0.0), true),
        ("DIMBLK", json!(""), true),
        ("DIMBLK", json!("zz<>|*?"), false),
        ("DIMBLK", json!("nonexistent_xyz"), false),
        ("DIMBLK", json!(12345), false),
        ("DIMBLK1", json!(""), true),
        ("DIMBLK1", json!("zz<>|*?"), false),
        ("DIMBLK1", json!("nonexistent_xyz"), false),
        ("DIMBLK1", json!(12345), false),
        ("DIMBLK2", json!(""), true),
        ("DIMBLK2", json!("zz<>|*?"), false),
        ("DIMBLK2", json!("nonexistent_xyz"), false),
        ("DIMBLK2", json!(12345), false),
        ("DIMCEN", json!(-1.0), true),
        ("DIMCEN", json!(0.0), true),
        ("DIMCLRD", json!(12345), false),
        ("DIMCLRE", json!(12345), false),
        ("DIMCLRT", json!(12345), false),
        ("DIMDLE", json!(-1.0), true),
        ("DIMDLE", json!(0.0), true),
        ("DIMDLI", json!(0.0), true),
        ("DIMDSEP", json!(""), false),
        ("DIMDSEP", json!("zz<>|*?"), false),
        ("DIMDSEP", json!("nonexistent_xyz"), false),
        ("DIMDSEP", json!(12345), false),
        ("DIMEXE", json!(0.0), true),
        ("DIMEXO", json!(-1.0), true),
        ("DIMEXO", json!(0.0), true),
        ("DIMFXL", json!(0.0), true),
        ("DIMFXLON", json!(2), false),
        ("DIMFXLON", json!(-1), false),
        ("DIMGAP", json!(-1.0), true),
        ("DIMGAP", json!(0.0), true),
        ("DIMLDRBLK", json!(""), true),
        ("DIMLDRBLK", json!("zz<>|*?"), false),
        ("DIMLDRBLK", json!("nonexistent_xyz"), false),
        ("DIMLDRBLK", json!(12345), false),
        ("DIMLFAC", json!(-1.0), true),
        ("DIMLFAC", json!(0.0), true),
        ("DIMLIM", json!(2), false),
        ("DIMLIM", json!(-1), false),
        ("DIMLWD", json!(-1), true),
        ("DIMLWE", json!(-1), true),
        ("DIMPOST", json!(""), true),
        ("DIMPOST", json!("zz<>|*?"), true),
        ("DIMPOST", json!("nonexistent_xyz"), true),
        ("DIMPOST", json!(12345), true),
        ("DIMRND", json!(0.0), true),
        ("DIMSCALE", json!(0.0), true),
        ("DIMSD1", json!(2), false),
        ("DIMSD1", json!(-1), false),
        ("DIMSD2", json!(2), false),
        ("DIMSD2", json!(-1), false),
        ("DIMSE1", json!(2), false),
        ("DIMSE1", json!(-1), false),
        ("DIMSE2", json!(2), false),
        ("DIMSE2", json!(-1), false),
        ("DIMSOXD", json!(2), false),
        ("DIMSOXD", json!(-1), false),
        ("DIMTFILLCLR", json!(12345), false),
        ("DIMTIH", json!(2), false),
        ("DIMTIH", json!(-1), false),
        ("DIMTIX", json!(2), false),
        ("DIMTIX", json!(-1), false),
        ("DIMTM", json!(-1.0), true),
        ("DIMTM", json!(0.0), true),
        ("DIMTOFL", json!(2), false),
        ("DIMTOFL", json!(-1), false),
        ("DIMTOH", json!(2), false),
        ("DIMTOH", json!(-1), false),
        ("DIMTOL", json!(2), false),
        ("DIMTOL", json!(-1), false),
        ("DIMTP", json!(-1.0), true),
        ("DIMTP", json!(0.0), true),
        ("DIMTSZ", json!(0.0), true),
        ("DIMTVP", json!(-1.0), true),
        ("DIMTVP", json!(0.0), true),
        ("DIMTXTDIRECTION", json!(2), false),
        ("DIMTXTDIRECTION", json!(-1), false),
        ("DIMUPT", json!(2), false),
        ("DIMUPT", json!(-1), false),
        ("DONUTID", json!(0.0), true),
        ("ELEVATION", json!(-1.0), true),
        ("ELEVATION", json!(0.0), true),
        ("FILLETRAD", json!(0.0), true),
        ("FONTALT", json!(""), true),
        ("FONTALT", json!("zz<>|*?"), false),
        ("FONTALT", json!("nonexistent_xyz"), true),
        ("FONTFALLBACK", json!(""), true),
        ("FONTFALLBACK", json!("zz<>|*?"), false),
        ("FONTFALLBACK", json!("nonexistent_xyz"), true),
        ("GRIDUNIT", json!(12345), true),
        ("HPANG", json!(-1.0), true),
        ("HPANG", json!(0.0), true),
        ("HPANG", json!(1e+300), false),
        ("HPANG", json!(-1e+300), false),
        ("HPLAYER", json!(""), false),
        ("HPLAYER", json!("zz<>|*?"), false),
        ("HPLAYER", json!("nonexistent_xyz"), true),
        ("HPLAYER", json!(12345), true),
        ("HPOBJWARNING", json!(999999), true),
        ("HPTRANSPARENCY", json!(""), false),
        ("HPTRANSPARENCY", json!("zz<>|*?"), false),
        ("HPTRANSPARENCY", json!("nonexistent_xyz"), false),
        ("HPTRANSPARENCY", json!(12345), false),
        ("INSBASE", json!(12345), true),
        ("INSNAME", json!(""), true),
        ("INSNAME", json!("zz<>|*?"), false),
        ("INSNAME", json!("nonexistent_xyz"), true),
        ("LIMMAX", json!(12345), true),
        ("LIMMIN", json!(12345), true),
        ("MAXARRAY", json!(999999), true),
        ("MLEADERSCALE", json!(0.0), true),
        ("MTEXTED", json!(""), true),
        ("MTEXTED", json!("zz<>|*?"), false),
        ("MTEXTED", json!("nonexistent_xyz"), true),
        ("MTEXTED", json!(12345), true),
        ("OFFSETDIST", json!(-1.0), true),
        ("OFFSETDIST", json!(0.0), true),
        ("OFFSETDIST", json!(1e+300), false),
        ("OFFSETDIST", json!(-1e+300), false),
        ("OSMODE", json!(999999), false),
        ("PDSIZE", json!(-1.0), true),
        ("PDSIZE", json!(0.0), true),
        ("PICKBOX", json!(0.0), true),
        ("PLINEWID", json!(0.0), true),
        ("POLARADDANG", json!(""), true),
        ("POLARADDANG", json!(12345), true),
        ("POLARANG", json!(1e+300), false),
        ("POLARDIST", json!(0.0), true),
        ("PSVPSCALE", json!(0.0), true),
        ("QNEWTEMPLATE", json!(""), true),
        ("QNEWTEMPLATE", json!("zz<>|*?"), false),
        ("QNEWTEMPLATE", json!("nonexistent_xyz"), true),
        ("QPMODE", json!(-1), true),
        ("QPMODE", json!(999999), false),
        ("QPMODE", json!(-999999), false),
        ("SAVEFILEPATH", json!(""), true),
        ("SAVEFILEPATH", json!("zz<>|*?"), false),
        ("SAVEFILEPATH", json!("nonexistent_xyz"), true),
        ("SAVETIME", json!(0.5), false),
        ("SELECTIONCYCLING", json!(-1), true),
        ("SNAPANG", json!(-1.0), true),
        ("SNAPANG", json!(0.0), true),
        ("SNAPBASE", json!(12345), true),
        ("SNAPUNIT", json!(12345), true),
        ("SPLINESEGS", json!(-1), true),
        ("THICKNESS", json!(-1.0), true),
        ("THICKNESS", json!(0.0), true),
        ("TRACEWID", json!(0.0), true),
        ("TREEDEPTH", json!(-1), true),
        ("TRUSTEDPATHS", json!(""), true),
        ("USERI1", json!(-1), true),
        ("USERI2", json!(-1), true),
        ("USERI3", json!(-1), true),
        ("USERI4", json!(-1), true),
        ("USERI5", json!(-1), true),
        ("USERR1", json!(-1.0), true),
        ("USERR1", json!(0.0), true),
        ("USERR1", json!(1e+300), false),
        ("USERR1", json!(-1e+300), false),
        ("USERR2", json!(-1.0), true),
        ("USERR2", json!(0.0), true),
        ("USERR2", json!(1e+300), false),
        ("USERR2", json!(-1e+300), false),
        ("USERR3", json!(-1.0), true),
        ("USERR3", json!(0.0), true),
        ("USERR3", json!(1e+300), false),
        ("USERR3", json!(-1e+300), false),
        ("USERR4", json!(-1.0), true),
        ("USERR4", json!(0.0), true),
        ("USERR4", json!(1e+300), false),
        ("USERR4", json!(-1e+300), false),
        ("USERR5", json!(-1.0), true),
        ("USERR5", json!(0.0), true),
        ("USERR5", json!(1e+300), false),
        ("USERR5", json!(-1e+300), false),
        ("USERS1", json!(""), true),
        ("USERS1", json!("zz<>|*?"), true),
        ("USERS1", json!("nonexistent_xyz"), true),
        ("USERS1", json!(12345), true),
        ("USERS2", json!(""), true),
        ("USERS2", json!("zz<>|*?"), true),
        ("USERS2", json!("nonexistent_xyz"), true),
        ("USERS2", json!(12345), true),
        ("USERS3", json!(""), true),
        ("USERS3", json!("zz<>|*?"), true),
        ("USERS3", json!("nonexistent_xyz"), true),
        ("USERS3", json!(12345), true),
        ("USERS4", json!(""), true),
        ("USERS4", json!("zz<>|*?"), true),
        ("USERS4", json!("nonexistent_xyz"), true),
        ("USERS4", json!(12345), true),
        ("USERS5", json!(""), true),
        ("USERS5", json!("zz<>|*?"), true),
        ("USERS5", json!("nonexistent_xyz"), true),
        ("USERS5", json!(12345), true),
        ("WSCURRENT", json!("zz<>|*?"), true),
        ("WSCURRENT", json!("nonexistent_xyz"), true),
    ]
}

#[test]
fn every_value_the_sweep_found_accepted_is_kept_only_when_autocad_takes_it() {
    let rows = sweep_accepted();
    assert_eq!(rows.len(), 240);
    let mut wrong = Vec::new();
    for (name, value, legal) in &rows {
        let mut s = session();
        let before = get(&mut s, name);
        match set(&mut s, name, value) {
            Ok(()) if *legal => {}
            Ok(()) => wrong.push(format!("{name} took {value} (now {})", get(&mut s, name))),
            Err(e) if *legal => wrong.push(format!("{name} refused {value}: {e}")),
            Err(e) => {
                // The refusal says what the variable takes, and nothing changed.
                if !(e.contains("must be") || e.contains(" (")) {
                    wrong.push(format!("{name} {value}: the refusal doesn't say the allowed values: {e}"));
                }
                if get(&mut s, name) != before {
                    wrong.push(format!("{name} {value}: a refused value changed it"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} wrong:\n{}", wrong.len(), wrong.join("\n"));
}

#[test]
fn the_dimension_switches_take_only_zero_and_one() {
    let switches = [
        "DIMALT",
        "DIMTIH",
        "DIMTOH",
        "DIMSE1",
        "DIMSE2",
        "DIMSD1",
        "DIMSD2",
        "DIMTIX",
        "DIMSOXD",
        "DIMTOFL",
        "DIMLIM",
        "DIMTOL",
        "DIMUPT",
        "DIMFXLON",
        "DIMTXTDIRECTION",
    ];
    let mut s = session();
    for var in switches {
        for junk in [json!(2), json!(-1), json!(0.5), json!("yes"), json!("2")] {
            let e = refused(&mut s, var, &junk);
            assert!(e.contains("0 (off) or 1 (on)"), "{var}: {e}");
        }
        for (ok, back) in [(json!(1), 1), (json!(0), 0), (json!(true), 1), (json!("1"), 1)] {
            set(&mut s, var, &ok).unwrap_or_else(|e| panic!("{var} refused {ok}: {e}"));
            let now = get(&mut s, var);
            assert!(now == json!(back) || now == json!(back == 1), "{var}: set {ok}, read {now}");
        }
    }
}

#[test]
fn arrowheads_colours_and_the_decimal_separator_are_checked() {
    let mut s = session();
    let c = s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    s.execute("block", &json!({ "name": "MyArrow", "base": [0, 0], "handles": [c["handle"]], "keep": "delete" })).unwrap();
    for var in ["DIMBLK", "DIMBLK1", "DIMBLK2", "DIMLDRBLK"] {
        for ok in ["_DOT", "_ARCHTICK", "_Open30", "_NONE", "", "."] {
            set(&mut s, var, &json!(ok)).unwrap_or_else(|e| panic!("{var} refused {ok:?}: {e}"));
        }
        set(&mut s, var, &json!("myarrow")).unwrap();
        assert_eq!(get(&mut s, var), json!("MyArrow"), "{var} keeps the block's spelling");
        for junk in [json!("nonexistent_xyz"), json!("zz<>|*?"), json!("_DOT|x"), json!(12345)] {
            let e = refused(&mut s, var, &junk);
            assert!(e.contains("built-in arrowhead"), "{var}: {e}");
        }
        assert_eq!(get(&mut s, var), json!("MyArrow"));
    }
    for var in ["DIMCLRD", "DIMCLRE", "DIMCLRT", "DIMTFILLCLR"] {
        for ok in [json!(0), json!(7), json!(256), json!("3")] {
            set(&mut s, var, &ok).unwrap_or_else(|e| panic!("{var} refused {ok}: {e}"));
        }
        for junk in [json!(12345), json!(-1), json!(257), json!(2.5), json!("notacolour")] {
            let e = refused(&mut s, var, &junk);
            assert!(e.contains("0 ByBlock, 1 to 255 or 256 ByLayer"), "{var}: {e}");
        }
    }
    for ok in [",", ".", "'"] {
        set(&mut s, "DIMDSEP", &json!(ok)).unwrap();
        assert_eq!(get(&mut s, "DIMDSEP"), json!(ok));
    }
    for junk in [json!(""), json!(" "), json!("ab"), json!(12345), json!(5)] {
        let e = refused(&mut s, "DIMDSEP", &junk);
        assert!(e.contains("one character"), "{e}");
    }
    assert_eq!(get(&mut s, "DIMDSEP"), json!("'"));
}

#[test]
fn transparency_layer_angle_and_bitcode_variables_are_checked() {
    let mut s = session();
    for (ok, back) in [(json!("bylayer"), "ByLayer"), (json!("ByBlock"), "ByBlock"), (json!(50), "50"), (json!("90"), "90"), (json!(0), "0")] {
        set(&mut s, "CETRANSPARENCY", &ok).unwrap();
        assert_eq!(get(&mut s, "CETRANSPARENCY"), json!(back));
        set(&mut s, "HPTRANSPARENCY", &ok).unwrap();
        assert_eq!(get(&mut s, "HPTRANSPARENCY"), json!(back));
    }
    set(&mut s, "HPTRANSPARENCY", &json!("Use Current")).unwrap();
    assert_eq!(get(&mut s, "HPTRANSPARENCY"), json!("use current"));
    let e = set(&mut s, "CETRANSPARENCY", &json!("use current")).unwrap_err();
    assert!(e.contains("ByLayer, ByBlock or 0 to 90"), "{e}");
    for junk in [json!(91), json!(-1), json!("50.5"), json!(12345), json!(""), json!("zz<>|*?")] {
        assert!(set(&mut s, "CETRANSPARENCY", &junk).is_err(), "CETRANSPARENCY took {junk}");
        assert!(set(&mut s, "HPTRANSPARENCY", &junk).is_err(), "HPTRANSPARENCY took {junk}");
    }
    for ok in [".", "Hatch Fill", "Use Current"] {
        set(&mut s, "HPLAYER", &json!(ok)).unwrap();
    }
    assert_eq!(get(&mut s, "HPLAYER"), json!("."));
    for junk in ["", "  ", "zz<>|*?", "a/b", "x;y"] {
        let e = refused(&mut s, "HPLAYER", &json!(junk));
        assert!(e.contains("a layer name"), "{e}");
    }
    // Angles: any finite size AutoCAD keeps (wrapped to 0-360), not 1e300.
    for var in ["ANGBASE", "CHAMFERD", "HPANG"] {
        set(&mut s, var, &json!(-90.0)).unwrap();
        assert!((get(&mut s, var).as_f64().unwrap() - 270.0).abs() < 1e-9, "{var}");
        for junk in [json!(1e300), json!(-1e300), json!("1e300"), json!("nan")] {
            assert!(set(&mut s, var, &junk).is_err(), "{var} took {junk}");
        }
        assert!((get(&mut s, var).as_f64().unwrap() - 270.0).abs() < 1e-9, "{var} changed");
    }
    for var in ["CMLSCALE", "OFFSETDIST", "CAMERAHEIGHT", "USERR1"] {
        set(&mut s, var, &json!(-2.5)).unwrap();
        assert!(set(&mut s, var, &json!(1e300)).is_err() && set(&mut s, var, &json!(-1e13)).is_err(), "{var}");
        assert_eq!(get(&mut s, var), json!(-2.5));
    }
    set(&mut s, "OSMODE", &json!(16384 + 4133)).unwrap();
    assert_eq!(get(&mut s, "OSMODE"), json!(16384 + 4133));
    for junk in [json!(32768), json!(999_999), json!(-1)] {
        let e = refused(&mut s, "OSMODE", &junk);
        assert!(e.contains("0 to 32767"), "{e}");
        assert!(set(&mut s, "3DOSMODE", &junk).is_err(), "3DOSMODE took {junk}");
    }
    assert_eq!(get(&mut s, "OSMODE"), json!(16384 + 4133));
    // 3D object snap bits CADaddy doesn't have (16 knot, 128 and up point clouds) are taken, as AutoCAD.
    set(&mut s, "3DOSMODE", &json!(2 + 16 + 128)).unwrap();
    assert_eq!(get(&mut s, "3DOSMODE"), json!(2));
    for (ok, junk) in [(json!(-1), json!(3)), (json!(2), json!(999_999))] {
        set(&mut s, "QPMODE", &ok).unwrap();
        assert!(set(&mut s, "QPMODE", &junk).is_err());
    }
    set(&mut s, "SAVETIME", &json!(20)).unwrap();
    assert!(set(&mut s, "SAVETIME", &json!(0.5)).unwrap_err().contains("whole number of minutes"));
    assert_eq!(get(&mut s, "SAVETIME"), json!(20));
    assert!(set(&mut s, "POLARANG", &json!(1e300)).is_err() && set(&mut s, "POLARANG", &json!(400)).is_err());
    set(&mut s, "POLARANG", &json!(22.5)).unwrap();
    for var in ["MTEXTED", "QNEWTEMPLATE", "SAVEFILEPATH", "DCTCUST", "FONTALT"] {
        let e = refused(&mut s, var, &json!("zz<>|*?"));
        assert!(e.contains("a file or folder name"), "{var}: {e}");
    }
    assert!(set(&mut s, "INSNAME", &json!("zz<>|*?")).is_err());
    set(&mut s, "INSNAME", &json!("Door 36")).unwrap();
}

#[test]
fn typed_setvar_and_lisp_refuse_the_same_values() {
    let mut s = session();
    let _ = s.cmdline("setvar");
    let _ = s.cmdline("DIMALT");
    let _ = s.cmdline("2");
    let _ = s.cmdline("");
    let _ = s.cmdline("(setvar \"DIMTOH\" -1)");
    let _ = s.cmdline("(setvar \"DIMBLK\" \"nonexistent_xyz\")");
    let _ = s.cmdline("(setvar \"CETRANSPARENCY\" \"12345\")");
    assert!(matches!(get(&mut s, "DIMALT"), Value::Bool(false)) || get(&mut s, "DIMALT") == json!(0));
    assert!(matches!(get(&mut s, "DIMTOH"), Value::Bool(true)) || get(&mut s, "DIMTOH") == json!(1));
    assert_eq!(get(&mut s, "DIMBLK"), json!(""));
    assert_eq!(get(&mut s, "CETRANSPARENCY"), json!("ByLayer"));
}
