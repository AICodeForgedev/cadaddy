//! AREA reports square inches and square feet in architectural units (not a feet-inches length).

use cadaddy_engine::Session;
use serde_json::json;

#[test]
fn architectural_area_reads_as_square_inches_and_feet() {
    let mut s = Session::new();
    s.execute("setvar", &json!({"name": "LUNITS", "value": 4})).unwrap();
    let r = s.execute("area", &json!({"points": [[0, 0], [120, 0], [120, 96], [0, 96]]})).unwrap();
    let m = r["message"].as_str().unwrap();
    assert_eq!(m, "Area = 11520.0000 square in. (80.0000 square ft.), Perimeter = 36'-0\"");
    s.execute("setvar", &json!({"name": "LUNITS", "value": 2})).unwrap();
    let r = s.execute("area", &json!({"points": [[0, 0], [2, 0], [2, 3]]})).unwrap();
    assert!(r["message"].as_str().unwrap().starts_with("Area = 3.0000,"), "{r}");
}
