//! A DWG read with CADaddy's own entity decoder gives the same drawing as through the DWG bridge,
//! entity by entity and field by field, for every version (Q199).

use cadaddy_doc::{Drawing, Entity};
use serde_json::Value as J;

/// A made-up drawing with an example of every entity the codec decodes.
fn drawing() -> Drawing {
    let mut e = String::new();
    let mut add = |s: &str| e.push_str(s);
    add("0\nLINE\n8\nWALLS\n62\n3\n10\n1\n20\n2\n30\n0\n11\n9\n21\n7\n31\n0\n");
    add("0\nLINE\n8\n0\n6\nDASHED\n370\n35\n48\n2\n10\n1\n20\n2\n30\n3\n11\n4\n21\n5\n31\n6\n39\n2.5\n");
    add("0\nCIRCLE\n8\n0\n420\n16711680\n10\n5\n20\n5\n30\n0\n40\n2.5\n");
    add("0\nARC\n8\n0\n10\n15\n20\n5\n30\n0\n40\n3\n50\n30\n51\n200\n");
    add("0\nPOINT\n8\n0\n10\n7\n20\n8\n30\n0\n");
    add("0\nELLIPSE\n8\n0\n10\n20\n20\n20\n30\n0\n11\n6\n21\n2\n31\n0\n40\n0.4\n41\n0\n42\n3.14159\n");
    add("0\nTEXT\n8\n0\n10\n1\n20\n30\n30\n0\n40\n2\n1\nRoom 101\n50\n15\n");
    add("0\nTEXT\n8\n0\n10\n1\n20\n35\n30\n0\n40\n2\n1\nCentred\n72\n1\n11\n10\n21\n35\n31\n0\n");
    add("0\nMTEXT\n8\n0\n10\n30\n20\n30\n30\n0\n40\n2.5\n41\n20\n71\n1\n72\n1\n1\nTwo\\Plines of text\n");
    add("0\nLWPOLYLINE\n8\nWALLS\n90\n4\n70\n1\n10\n0\n20\n50\n10\n10\n20\n50\n42\n0.5\n10\n10\n20\n60\n10\n0\n20\n60\n");
    add("0\nLWPOLYLINE\n8\n0\n90\n2\n70\n0\n43\n0.5\n10\n0\n20\n70\n10\n20\n20\n70\n");
    add("0\nSOLID\n8\n0\n10\n40\n20\n0\n30\n0\n11\n45\n21\n0\n31\n0\n12\n40\n22\n5\n32\n0\n13\n45\n23\n5\n33\n0\n");
    add("0\n3DFACE\n8\n0\n10\n50\n20\n0\n30\n0\n11\n55\n21\n0\n31\n1\n12\n55\n22\n5\n32\n2\n13\n50\n23\n5\n33\n3\n");
    add("0\nRAY\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n0\n31\n0\n");
    add("0\nXLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n0\n21\n1\n31\n0\n");
    add(
        "0\nSPLINE\n8\n0\n70\n8\n71\n3\n72\n8\n73\n4\n74\n0\n40\n0\n40\n0\n40\n0\n40\n0\n40\n1\n40\n1\n40\n1\n40\n1\n10\n0\n20\n80\n30\n0\n10\n5\n20\n90\n30\n0\n10\n10\n20\n80\n30\n0\n10\n15\n20\n90\n30\n0\n",
    );
    add(
        "0\nPOLYLINE\n8\n0\n66\n1\n10\n0\n20\n0\n30\n0\n70\n8\n0\nVERTEX\n8\n0\n10\n80\n20\n0\n30\n1\n70\n32\n0\nVERTEX\n8\n0\n10\n90\n20\n0\n30\n2\n70\n32\n0\nSEQEND\n8\n0\n",
    );
    add(
        "0\nINSERT\n8\n0\n66\n1\n2\nTAG\n10\n100\n20\n0\n30\n0\n41\n2\n42\n2\n43\n2\n50\n45\n0\nATTRIB\n8\n0\n10\n100\n20\n0\n30\n0\n40\n1\n1\nA-12\n2\nNUMBER\n70\n0\n0\nSEQEND\n8\n0\n",
    );
    add("0\nINSERT\n8\n0\n2\nTAG\n10\n110\n20\n0\n30\n0\n");
    add(
        "0\nHATCH\n8\n0\n10\n0\n20\n0\n30\n0\n210\n0\n220\n0\n230\n1\n2\nSOLID\n70\n1\n71\n0\n91\n1\n92\n2\n72\n0\n73\n1\n93\n4\n10\n0\n20\n100\n10\n10\n20\n100\n10\n10\n20\n110\n10\n0\n20\n110\n97\n0\n75\n0\n76\n1\n98\n0\n",
    );
    add("0\nLEADER\n8\n0\n3\nStandard\n71\n1\n72\n0\n73\n3\n76\n3\n10\n0\n20\n140\n30\n0\n10\n5\n20\n145\n30\n0\n10\n10\n20\n145\n30\n0\n");
    let dxf = format!(
        "0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLTYPE\n0\nLTYPE\n2\nDASHED\n70\n0\n3\n__ __\n72\n65\n73\n2\n40\n0.75\n49\n0.5\n49\n-0.25\n0\nENDTAB\n\
         0\nTABLE\n2\nLAYER\n0\nLAYER\n2\n0\n70\n0\n62\n7\n6\nCONTINUOUS\n0\nLAYER\n2\nWALLS\n70\n0\n62\n1\n6\nCONTINUOUS\n0\nENDTAB\n0\nENDSEC\n\
         0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n8\n0\n2\nTAG\n70\n2\n10\n0\n20\n0\n30\n0\n3\nTAG\n\
         0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n1\n\
         0\nATTDEF\n8\n0\n10\n0\n20\n0\n30\n0\n40\n1\n1\n?\n3\nNumber?\n2\nNUMBER\n70\n0\n0\nENDBLK\n8\n0\n0\nENDSEC\n\
         0\nSECTION\n2\nENTITIES\n{e}0\nENDSEC\n0\nEOF\n"
    );
    cadaddy_io::read(dxf.as_bytes(), "made-up.dxf").unwrap()
}

/// Two JSON values equal but for float rounding; the path of the first difference.
fn diff(a: &J, b: &J, path: &str) -> Option<String> {
    match (a, b) {
        (J::Number(x), J::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            ((x - y).abs() > 1e-9 * x.abs().max(1.0)).then(|| format!("{path}: {x} vs {y}"))
        }
        (J::Array(x), J::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: {} items vs {}", x.len(), y.len()));
            }
            x.iter().zip(y).enumerate().find_map(|(i, (p, q))| diff(p, q, &format!("{path}[{i}]")))
        }
        (J::Object(x), J::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            keys.into_iter().find_map(|k| diff(x.get(k).unwrap_or(&J::Null), y.get(k).unwrap_or(&J::Null), &format!("{path}.{k}")))
        }
        _ => (a != b).then(|| format!("{path}: {a} vs {b}")),
    }
}

fn entities(d: &Drawing) -> Vec<(String, &Entity)> {
    let mut v: Vec<(String, &Entity)> = d.model.iter().map(|e| ("model".to_string(), e.as_ref())).collect();
    for (name, b) in &d.blocks {
        v.extend(b.entities.iter().map(|e| (format!("block {name}"), e.as_ref())));
    }
    v
}

#[test]
fn native_entities_match_the_bridge_field_by_field() {
    let mut report = Vec::new();
    for v in ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"] {
        let mut d = drawing();
        d.header.set_str("ACADVER", v);
        let bytes = cadaddy_io::write(&d, "made-up.dwg").unwrap();
        let bridge = cadaddy_io::read(&bytes, "made-up.dwg").unwrap();
        let (native, rep) = cadaddy_io::dwg_native::read_with_native_entities(&bytes).unwrap();
        assert!(rep.failed.is_empty(), "{v}: undecoded {:?}", rep.failed);
        let a = entities(&bridge);
        let b = entities(&native);
        // Every entity record came from the codec (a POLYLINE's vertices and an INSERT's
        // attributes come with their owner, so there are fewer records than entities).
        eprintln!("{v}: {} entities, {} records from the codec", a.len(), rep.replaced);
        assert!(rep.replaced >= a.len() - 4, "{v}: only {} of {} entities decoded natively", rep.replaced, a.len());
        assert_eq!(a.len(), b.len(), "{v}: entity count");
        for ((wa, ea), (wb, eb)) in a.iter().zip(&b) {
            let ja = serde_json::to_value(ea).unwrap();
            let jb = serde_json::to_value(eb).unwrap();
            if wa != wb {
                report.push(format!("{v}: {wa} vs {wb}"));
            } else if let Some(d) = diff(&ja, &jb, "") {
                report.push(format!("{v} {wa} {:?}: {d}", ea.handle));
            }
        }
    }
    assert!(report.is_empty(), "{} differences:\n{}", report.len(), report.join("\n"));
}
