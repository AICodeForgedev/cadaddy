//! Q6577: a print set's sheet stored to go out of memory (`cdy::encode_stored`) keeps only its
//! bytes. The compressor's buffer is sized for the worst case (half the input), and a stored chunk
//! is held as long as its sheet is out: a four-storey job's parked sheets held more spare buffer
//! than their objects took in memory.

use std::sync::Arc;

use cadaddy_doc::{Entity, EntityKind, Handle, Line};
use cadaddy_geom::Vec3;

#[test]
fn a_stored_sheet_chunk_holds_only_its_bytes() {
    let items: Vec<Arc<Entity>> = (0..1000u32)
        .map(|i| {
            let x = f64::from(i);
            Arc::new(Entity::new(Handle(0x100 + u64::from(i)), EntityKind::Line(Line { a: Vec3::new(x, 0.0, 0.0), b: Vec3::new(x, 1.0, 0.0) })))
        })
        .collect();
    let c = cadaddy_io::cdy::encode_stored(&items, 16).unwrap();
    assert!(c.deflated && (c.stored.len() as u64) * 4 < c.raw_len, "{} bytes stored of {}", c.stored.len(), c.raw_len);
    assert_eq!(c.stored.capacity(), c.stored.len(), "the stored chunk keeps the compressor's spare buffer");
    let (back, notes) = cadaddy_io::cdy::decode_stored(&c).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(back.len(), items.len());
}
