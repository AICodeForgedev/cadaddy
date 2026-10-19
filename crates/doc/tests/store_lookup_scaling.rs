//! Entity lookups by handle stay quick in large drawings (perf1): a store of hundreds of
//! thousands of objects finds each one without walking its chunks, also after draw-order changes
//! put chunks and objects out of order.

use cadaddy_doc::*;
use cadaddy_geom::Vec3;

fn line(i: usize) -> EntityKind {
    let x = i as f64;
    EntityKind::Line(Line { a: Vec3::new(x, 0.0, 0.0), b: Vec3::new(x, 1.0, 0.0) })
}

#[test]
fn lookups_scale_to_hundreds_of_thousands_of_objects() {
    let n = 300_000;
    let mut d = Drawing::new_metric();
    let hs: Vec<Handle> = (0..n).map(|i| d.add(&Space::Model, Common::default(), line(i)).unwrap()).collect();
    let t = std::time::Instant::now();
    for (i, h) in hs.iter().enumerate() {
        let e = d.model.get(*h).expect("found");
        assert!(matches!(&e.kind, EntityKind::Line(l) if l.a.x == i as f64));
    }
    let took = t.elapsed();
    assert!(took.as_secs_f64() < 10.0, "{n} lookups took {took:?}");
    let t = std::time::Instant::now();
    for h in &hs {
        assert!(d.model.modify(*h, |e| e.kind.transform(&cadaddy_geom::Mat3::translate(cadaddy_geom::Vec2::new(1.0, 0.0)))));
    }
    assert!(t.elapsed().as_secs_f64() < 10.0, "{n} edits took {:?}", t.elapsed());
}

#[test]
fn lookups_still_find_everything_out_of_order() {
    let mut d = Drawing::new_metric();
    let hs: Vec<Handle> = (0..5000).map(|i| d.add(&Space::Model, Common::default(), line(i)).unwrap()).collect();
    // Send some to the back (a new chunk in front, with a higher id), move others next to
    // earlier objects, and remove some: chunk ids and handles are no longer in order.
    for h in hs.iter().step_by(7) {
        assert!(d.model.send_to_back(*h));
    }
    for (i, h) in hs.iter().enumerate().skip(3).step_by(11) {
        assert!(d.model.move_next_to(*h, hs[i % 3], i % 2 == 0));
    }
    let gone: Vec<Handle> = hs.iter().copied().skip(5).step_by(13).collect();
    for h in &gone {
        assert!(d.model.remove(*h).is_some());
    }
    for (i, h) in hs.iter().enumerate() {
        let e = d.model.get(*h);
        if gone.contains(h) {
            assert!(e.is_none());
        } else {
            assert!(e.is_some_and(|e| e.handle == *h && matches!(&e.kind, EntityKind::Line(l) if l.a.x == i as f64)), "{i}");
            assert!(d.model.modify(*h, |_| {}));
        }
    }
    assert_eq!(d.model.len(), hs.len() - gone.len());
    assert!(d.model.get(Handle(0xFFFF_FFFF)).is_none());
}
