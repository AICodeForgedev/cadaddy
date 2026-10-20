//! Check values (spec §2.14, §4.2).
//!
//! The "8-bit CRC" of spec §2.14.1 is a 16-bit CRC (reflected polynomial 0xA001) with a caller's
//! starting value; spec §2.14.2's 32-bit CRC is the common reflected 0xEDB88320 CRC with a seed.
//! The tables are generated from the polynomials (the spec prints the same tables). R2004+ pages
//! carry a two-sum checksum instead (spec §4.2).

const fn crc16_table() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u16;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xA001 } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

const fn crc32_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC16: [u16; 256] = crc16_table();
static CRC32: [u32; 256] = crc32_table();

/// Spec §2.14.1's CRC of `data`, continuing from `seed` (the spec's `crc8`).
pub fn crc8(seed: u16, data: &[u8]) -> u16 {
    let mut dx = seed;
    for &b in data {
        let al = b ^ (dx & 0xFF) as u8;
        dx = (dx >> 8) ^ CRC16[al as usize];
    }
    dx
}

/// Spec §2.14.2's 32-bit CRC of `data` with `seed`.
pub fn crc32(seed: u32, data: &[u8]) -> u32 {
    let mut c = !seed;
    for &b in data {
        c = (c >> 8) ^ CRC32[((c ^ u32::from(b)) & 0xFF) as usize];
    }
    !c
}

/// Spec §4.2's section page checksum of `data`, continuing from `seed`.
pub fn page_checksum(seed: u32, data: &[u8]) -> u32 {
    let mut sum1 = seed & 0xFFFF;
    let mut sum2 = seed >> 16;
    for chunk in data.chunks(0x15B0) {
        for &b in chunk {
            sum1 += u32::from(b);
            sum2 += sum1;
        }
        sum1 %= 0xFFF1;
        sum2 %= 0xFFF1;
    }
    (sum2 << 16) | (sum1 & 0xFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_the_spec() {
        // Spot checks against the tables printed in spec §2.14.
        assert_eq!((CRC16[1], CRC16[2], CRC16[8], CRC16[255]), (0xC0C1, 0xC181, 0xC601, 0x4040));
        assert_eq!((CRC32[1], CRC32[2], CRC32[255]), (0x7707_3096, 0xEE0E_612C, 0x2D02_EF8D));
    }

    #[test]
    fn known_values() {
        // The 0xA001 CRC with seed 0 is CRC-16/ARC; the 32-bit one with seed 0 is CRC-32.
        assert_eq!(crc8(0, b"123456789"), 0xBB3D);
        assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926);
        // Accumulates across calls.
        assert_eq!(crc8(crc8(0xC0C1, b"1234"), b"56789"), crc8(0xC0C1, b"123456789"));
        assert_eq!(crc32(crc32(0, b"1234"), b"56789"), crc32(0, b"123456789"));
        // The page checksum is an Adler-style pair of sums: seed 1 makes it Adler-32.
        assert_eq!(page_checksum(1, b"Wikipedia"), 0x11E6_0398);
        let big: Vec<u8> = (0..20_000u32).map(|i| (i * 7) as u8).collect();
        assert_eq!(page_checksum(page_checksum(0, &big[..9000]), &big[9000..]), page_checksum(0, &big));
    }
}
