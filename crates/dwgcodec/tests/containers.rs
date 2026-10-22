//! Reading and writing DWG containers of files written by another program (acadrust, a black
//! box): CRCs check, every object in the map is read, and writing gives the same bytes back.
mod common;

use cadaddy_dwgcodec::{read, write};

fn versions_read() -> Vec<&'static str> {
    common::VERSIONS.iter().copied().filter(|v| read(&common::dwg(v, 1)).is_ok()).collect()
}

#[test]
fn r2000_round_trips_byte_for_byte() {
    for n in [0, 12, 3000] {
        let bytes = common::dwg("AC1015", n);
        let dwg = read(&bytes).unwrap_or_else(|e| panic!("{n} entities: {e}"));
        assert!(dwg.objects.len() > n, "{n} entities: {} objects", dwg.objects.len());
        let again = write(&dwg).unwrap();
        assert!(
            again == bytes,
            "{n} entities: written {} bytes, read {}; first difference at {:?}",
            again.len(),
            bytes.len(),
            again.iter().zip(&bytes).position(|(a, b)| a != b)
        );
    }
}

#[test]
fn every_version_is_read() {
    assert_eq!(versions_read(), common::VERSIONS);
}

/// R2007 (Q202): our layout of a file read back holds the same header, classes, objects and
/// sections; acadrust reads it to the same handles; and our own layout is stable.
#[test]
fn r2007_round_trips() {
    for n in [0, 12, 400, 3000] {
        let bytes = common::dwg("AC1021", n);
        let d = read(&bytes).unwrap_or_else(|e| panic!("{n}: {e}"));
        let again = write(&d).unwrap();
        let back = read(&again).unwrap_or_else(|e| panic!("{n}: our file: {e}"));
        assert_eq!((&back.header, &back.classes, &back.objects), (&d.header, &d.classes, &d.objects), "{n}");
        for s in &d.sections {
            assert_eq!(back.section(&s.name).map(|x| &x.data), Some(&s.data), "{n}: {}", s.name);
        }
        assert_eq!(common::acadrust_handles(&again), common::acadrust_handles(&bytes), "{n}: acadrust reads ours");
        assert!(write(&back).unwrap() == again, "{n}: our own layout is stable");
    }
}

#[test]
fn r2004_and_later_round_trip_byte_for_byte() {
    for v in ["AC1018", "AC1024", "AC1027", "AC1032"] {
        for n in [0, 12, 3000] {
            let bytes = common::dwg(v, n);
            let dwg = read(&bytes).unwrap_or_else(|e| panic!("{v}, {n} entities: {e}"));
            assert!(dwg.objects.len() > n, "{v}, {n} entities: {} objects", dwg.objects.len());
            let again = write(&dwg).unwrap();
            assert!(
                again == bytes,
                "{v}, {n} entities: written {} bytes, read {}; first difference at {:?}",
                again.len(),
                bytes.len(),
                again.iter().zip(&bytes).position(|(a, b)| a != b)
            );
        }
    }
}

/// Object types with no DXF form, so acadrust's DXF can't show them (spec §20.3: 0x46
/// VX_CONTROL_OBJ, 0x47 VX_TABLE_RECORD).
const NOT_IN_DXF: [u16; 2] = [0x46, 0x47];

/// The parity anchor: every object acadrust reads is in our object map, and ours has only
/// DXF-less objects besides; with them, the counts match.
#[test]
fn the_object_map_matches_acadrust() {
    for v in versions_read() {
        for n in [12, 400] {
            let bytes = common::dwg(v, n);
            let d = read(&bytes).unwrap();
            let ours: std::collections::BTreeSet<u64> = d.objects.iter().map(|o| o.handle).collect();
            assert_eq!(ours.len(), d.objects.len(), "{v}: a handle twice");
            let theirs = common::acadrust_handles(&bytes);
            let missing: Vec<_> = theirs.difference(&ours).collect();
            assert!(missing.is_empty(), "{v}, {n}: acadrust read {missing:x?}, not in our map");
            let extra: Vec<u16> = ours.difference(&theirs).filter_map(|h| d.object(*h)).map(|o| o.type_code).collect();
            assert!(extra.iter().all(|t| NOT_IN_DXF.contains(t)), "{v}, {n}: extra objects of types {extra:?}");
            assert_eq!(ours.len(), theirs.len() + extra.len(), "{v}, {n}");
        }
    }
}

/// A changed object moves everything after it: addresses, the map and the header follow, and
/// another reader still reads the file.
#[test]
fn a_changed_file_is_written_consistently() {
    for v in versions_read() {
        let bytes = common::dwg(v, 40);
        let mut d = read(&bytes).unwrap();
        let before = common::acadrust_handles(&bytes);
        // Pad the first object's record (bits after its data are padding to the CRC, spec §2.14.1).
        let first = d.objects.first_mut().unwrap();
        first.data.extend_from_slice(&[0; 37]);
        let again = write(&d).unwrap();
        assert_ne!(again, bytes);
        let back = read(&again).unwrap_or_else(|e| panic!("{v}: {e}"));
        assert_eq!(back.objects, d.objects, "{v}");
        assert_eq!(common::acadrust_handles(&again), before, "{v}: acadrust reads the rewritten file");
        // A file we laid out ourselves comes back byte for byte too.
        assert!(write(&back).unwrap() == again, "{v}: our own layout is stable");
        // Every section our way: header and classes rewritten unchanged keep their bytes.
        assert_eq!((back.header.clone(), back.classes.clone()), (d.header.clone(), d.classes.clone()), "{v}");
    }
}

#[test]
fn arbitrary_bytes_never_panic() {
    for v in common::VERSIONS {
        fuzz(v);
    }
}

fn fuzz(v: &str) {
    let good = common::dwg(v, 30);
    let mut seed = 0x9E37_79B9u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for i in 0..1500 {
        let mut b = good.clone();
        match i % 3 {
            // Flip a few bytes anywhere.
            0 => {
                for _ in 0..1 + next() % 8 {
                    let at = next() as usize % b.len();
                    b[at] ^= next() as u8 | 1;
                }
            }
            // Cut it short.
            1 => b.truncate(next() as usize % b.len()),
            // Random bytes behind a DWG id.
            _ => {
                b = (0..next() % 4000).map(|_| next() as u8).collect();
                b.splice(0..0, v.bytes());
            }
        }
        let _ = read(&b);
    }
}

/// The sequence an R2004-layout file header is XORed with (spec §4.1).
fn header_mask(n: usize) -> Vec<u8> {
    let mut seed: u32 = 1;
    (0..n)
        .map(|_| {
            seed = seed.wrapping_mul(0x343FD).wrapping_add(0x269EC3);
            (seed >> 16) as u8
        })
        .collect()
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// A file whose page map records its own page larger than what was written, the file ending
/// inside it (some writers do: the page map's page is last, and the file's closing copy of the
/// header data follows its data straight away). Read as AutoCAD reads it, objects and all.
#[test]
fn a_page_map_page_running_past_the_end_is_read() {
    use cadaddy_dwgcodec::{crc, lz77};
    for v in ["AC1018", "AC1024", "AC1032"] {
        let bytes = common::dwg(v, 40);
        let expected = read(&bytes).unwrap();
        let mask = header_mask(0x6C);
        let mut data: Vec<u8> = bytes[0x80..0x80 + 0x6C].iter().zip(&mask).map(|(a, b)| a ^ b).collect();
        let map_at = 0x100 + u64::from_le_bytes(data[0x54..0x5C].try_into().unwrap()) as usize;
        let map_id = le32(&data, 0x50);
        let (size, csize) = (le32(&bytes, map_at + 4) as usize, le32(&bytes, map_at + 8) as usize);
        let mut map = lz77::decompress(&bytes[map_at + 20..map_at + 20 + csize], size).unwrap();
        // Enlarge the page map's own entry (pages are laid out in map order; it must be last).
        let mut p = 0;
        let mut last = None;
        while p + 8 <= map.len() {
            let id = le32(&map, p) as i32;
            if id as u32 == map_id {
                last = Some(p);
            }
            p += if id < 0 { 24 } else { 8 };
        }
        let entry = last.expect("the page map lists itself");
        assert_eq!(entry + 8, map.len(), "{v}: the page map's page is the last page");
        let declared = le32(&map, entry + 4) + 0x5C0;
        map[entry + 4..entry + 8].copy_from_slice(&declared.to_le_bytes());
        let c = lz77::compress(&map).unwrap();
        let mut page = Vec::new();
        for x in [0x4163_0E3Bu32, map.len() as u32, c.len() as u32, 2, 0] {
            page.extend_from_slice(&x.to_le_bytes());
        }
        let sum = crc::page_checksum(crc::page_checksum(0, &page), &c);
        page[16..20].copy_from_slice(&sum.to_le_bytes());
        page.extend_from_slice(&c);
        // The file: everything before the page map, its unpadded page, 0x14 bytes, the copy.
        let mut out = bytes[..map_at].to_vec();
        out.extend_from_slice(&page);
        out.extend_from_slice(&[0; 0x14]);
        let copy_at = out.len() as u64;
        data[0x34..0x3C].copy_from_slice(&copy_at.to_le_bytes());
        data[0x68..0x6C].copy_from_slice(&[0; 4]);
        let crc = crc::crc32(0, &data);
        data[0x68..0x6C].copy_from_slice(&crc.to_le_bytes());
        let enc: Vec<u8> = data.iter().zip(&mask).map(|(a, b)| a ^ b).collect();
        out[0x80..0x80 + 0x6C].copy_from_slice(&enc);
        out.extend_from_slice(&enc);
        assert!(map_at + declared as usize > out.len(), "{v}: the file ends inside the page map's page");

        let d = read(&out).unwrap_or_else(|e| panic!("{v}: {e}"));
        assert_eq!(d.objects, expected.objects, "{v}");
        // Written again, it is a whole file that reads the same.
        let again = write(&d).unwrap();
        assert_eq!(read(&again).unwrap_or_else(|e| panic!("{v}: rewritten: {e}")).objects, expected.objects, "{v}");
        assert_eq!(common::acadrust_handles(&again), common::acadrust_handles(&bytes), "{v}: acadrust reads the rewritten file");
    }
}
