//! Point cloud files: a synthetic LAS round trip (coordinates to the millimetre, colours,
//! intensities), a LAS 1.4 record of format 7 built by hand, compressed LAZ refused, damaged
//! files read safely, and PTS / XYZ text.

use cadaddy_doc::pointcloud::Cloud;
use cadaddy_io::pointcloud_io::{read, read_las, write_las};

fn synthetic(n: usize) -> Cloud {
    let mut c = Cloud::default();
    for i in 0..n {
        let f = i as f32;
        c.points.push([1000.0 + f * 0.25, 2000.0 - f * 0.5, 10.0 + (f * 0.1).sin()]);
        c.colors.push([(i % 256) as u8, ((i * 7) % 256) as u8, ((i * 13) % 256) as u8]);
        c.intensity.push((i * 37 % 65536) as u16);
    }
    c
}

#[test]
fn a_synthetic_las_round_trips() {
    let c = synthetic(1000);
    let bytes = write_las(&c);
    assert_eq!(&bytes[..4], b"LASF");
    assert_eq!(bytes.len(), 227 + 1000 * 26);
    let back = read_las(&bytes).unwrap();
    assert_eq!(back.points.len(), 1000);
    for (a, b) in c.points.iter().zip(&back.points) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 0.0011, "{a:?} vs {b:?}");
        }
    }
    assert_eq!(back.colors, c.colors);
    assert_eq!(back.intensity, c.intensity);
    // By name too.
    assert_eq!(read(&bytes, "scan.las").unwrap().points.len(), 1000);
}

/// A LAS 1.4 file with one point of format 7 (x, y, z, intensity, … RGB at byte 30).
fn las14_format7() -> Vec<u8> {
    let mut h = vec![0u8; 375];
    h[..4].copy_from_slice(b"LASF");
    h[24] = 1;
    h[25] = 4;
    h[94..96].copy_from_slice(&375u16.to_le_bytes());
    h[96..100].copy_from_slice(&375u32.to_le_bytes());
    h[104] = 7;
    h[105..107].copy_from_slice(&36u16.to_le_bytes());
    // Legacy count 0: the 64-bit count at 247 holds it.
    h[247..255].copy_from_slice(&1u64.to_le_bytes());
    for at in [131usize, 139, 147] {
        h[at..at + 8].copy_from_slice(&0.01f64.to_le_bytes());
    }
    h[155..163].copy_from_slice(&500.0f64.to_le_bytes());
    let mut rec = vec![0u8; 36];
    rec[0..4].copy_from_slice(&150i32.to_le_bytes());
    rec[4..8].copy_from_slice(&(-200i32).to_le_bytes());
    rec[8..12].copy_from_slice(&300i32.to_le_bytes());
    rec[12..14].copy_from_slice(&999u16.to_le_bytes());
    rec[30..32].copy_from_slice(&(255u16 << 8).to_le_bytes());
    rec[32..34].copy_from_slice(&(128u16 << 8).to_le_bytes());
    rec[34..36].copy_from_slice(&0u16.to_le_bytes());
    h.extend(rec);
    h
}

#[test]
fn las_1_4_format_7_laz_and_damage() {
    let c = read_las(&las14_format7()).unwrap();
    assert_eq!(c.points, [[501.5, -2.0, 3.0]]);
    assert_eq!(c.intensity, [999]);
    assert_eq!(c.colors, [[255, 128, 0]]);

    let mut laz = las14_format7();
    laz[104] |= 0x80;
    assert!(read_las(&laz).unwrap_err().contains("LAZ"));
    assert!(read(b"anything", "scan.laz").is_err());

    // A file cut short reads the points it holds; garbage is refused.
    let mut short = write_las(&synthetic(10));
    short.truncate(227 + 26 * 4 + 5);
    assert_eq!(read_las(&short).unwrap().points.len(), 4);
    assert!(read_las(b"LASF").is_err());
    assert!(read_las(b"not a las").is_err());
    let mut bad_format = las14_format7();
    bad_format[104] = 42;
    assert!(read_las(&bad_format).is_err());
}

#[test]
fn pts_and_xyz_text() {
    let pts = "3\n1 2 3 100 255 0 0\n4 5 6 -2048 0 255 0\n7 8 9 2047 0 0 255\n";
    let c = read(pts.as_bytes(), "scan.pts").unwrap();
    assert_eq!(c.points.len(), 3);
    assert_eq!(c.colors[1], [0, 255, 0]);
    assert_eq!(c.intensity[1], 0);
    let xyz = "# a comment\n1,2,3\n4,5,6\n\n7\t8\t9\n";
    let c = read(xyz.as_bytes(), "scan.xyz").unwrap();
    assert_eq!(c.points, [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]]);
    assert!(c.colors.is_empty() && c.intensity.is_empty());
    assert!(read(b"nothing here", "scan.xyz").is_err());
    assert!(read(b"x", "scan.e57").is_err());
}
