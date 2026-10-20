//! Header variables and classes decoded from files another program wrote (acadrust, black box):
//! the values are the drawing's, every stream is used up exactly, and encoding gives the same bytes.
mod common;

use cadaddy_dwgcodec::bits::Text;
use cadaddy_dwgcodec::header::Value;
use cadaddy_dwgcodec::read;

fn handle(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Handle(h)) => h.value,
        other => panic!("not a handle: {other:?}"),
    }
}

#[test]
fn header_variables_decode_and_encode_back() {
    for v in common::VERSIONS {
        let bytes = common::dwg(v, 12);
        let d = read(&bytes).unwrap();
        let h = d.header_vars().unwrap_or_else(|e| panic!("{v}: {e}"));
        assert_eq!(h.encode(), d.header.body, "{v}: encoding gives the same bytes");
        // Values the drawing has: the standard tables point at objects of the right types
        // (spec §20.3: 0x30 BLOCK_CONTROL, 0x32 LAYER_CONTROL, 0x38 LTYPE_CONTROL, 0x42 APPID_CONTROL).
        for (name, ty) in [("BLOCK_CONTROL", 0x30), ("LAYER_CONTROL", 0x32), ("LINETYPE_CONTROL", 0x38), ("APPID_CONTROL", 0x42)] {
            let o = d.object(handle(h.get(name))).unwrap_or_else(|| panic!("{v}: {name} not an object"));
            assert_eq!(o.type_code, ty, "{v}: {name}");
        }
        // CLAYER is a layer (0x33) and the model space block record (0x31) exists.
        assert_eq!(d.object(handle(h.get("CLAYER"))).map(|o| o.type_code), Some(0x33), "{v}");
        assert_eq!(d.object(handle(h.get("BLOCK_RECORD_MODEL_SPACE"))).map(|o| o.type_code), Some(0x31), "{v}");
        // HANDSEED is past every handle.
        let seed = handle(h.get("HANDSEED"));
        assert!(d.objects.iter().all(|o| o.handle < seed), "{v}: HANDSEED {seed:#x}");
        assert!(matches!(h.get("LTSCALE"), Some(Value::Double(x)) if *x == 1.0), "{v}: {:?}", h.get("LTSCALE"));
        assert!(matches!(h.get("EXTMAX"), Some(Value::Point3(p)) if p[0] >= 100.0), "{v}: {:?}", h.get("EXTMAX"));
    }
}

#[test]
fn a_changed_variable_is_written_and_read_back() {
    for v in common::VERSIONS {
        let bytes = common::dwg(v, 12);
        let mut d = read(&bytes).unwrap();
        let mut h = d.header_vars().unwrap();
        assert!(h.set("LTSCALE", Value::Double(2.5)));
        assert!(h.set(
            "HYPERLINKBASE",
            Value::Text(if d.version.unicode() { Text::Wide("base/".encode_utf16().collect()) } else { Text::Bytes(b"base/".to_vec()) })
        ));
        d.set_header_vars(&h);
        let again = cadaddy_dwgcodec::write(&d).unwrap();
        let back = read(&again).unwrap().header_vars().unwrap();
        assert_eq!(back.get("LTSCALE"), Some(&Value::Double(2.5)), "{v}");
        assert!(matches!(back.get("HYPERLINKBASE"), Some(Value::Text(t)) if t.to_string_lossy() == "base/"), "{v}");
        assert_eq!(back.get("CLAYER"), h.get("CLAYER"), "{v}");
    }
}

#[test]
fn classes_decode_and_encode_back() {
    for v in common::VERSIONS {
        let d = read(&common::dwg(v, 12)).unwrap();
        let c = d.classes().unwrap_or_else(|e| panic!("{v}: {e}"));
        assert_eq!(c.encode(), d.classes.body, "{v}: encoding gives the same bytes");
        assert!(!c.classes.is_empty(), "{v}");
        let names: Vec<String> = c.classes.iter().map(|c| c.dxf_name.to_string_lossy()).collect();
        assert!(names.iter().any(|n| n == "LAYOUT"), "{v}: {names:?}");
        assert!(c.classes.iter().all(|c| c.item_class_id == 0x1F2 || c.item_class_id == 0x1F3), "{v}");
        // Objects of class types name a class that exists.
        for o in d.objects.iter().filter(|o| o.type_code >= 500) {
            assert!(c.get(o.type_code).is_some(), "{v}: object {:#x} of type {}", o.handle, o.type_code);
        }
    }
}
