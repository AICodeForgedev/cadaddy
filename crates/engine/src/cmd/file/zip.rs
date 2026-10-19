//! ZIP archives (the PKWARE APPNOTE format): written with Deflate (or stored, when that doesn't
//! make an entry smaller) and UTF-8 names, and read back (stored and Deflate entries). No ZIP64:
//! an archive holds at most 65 535 entries and 4 GiB.

/// Most bytes one entry inflates to when read.
pub const MAX_ENTRY_BYTES: usize = 1 << 30;
/// Most entries an archive holds (without ZIP64).
const MAX_ENTRIES: usize = 0xFFFF;

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC: [u32; 256] = crc_table();

/// CRC-32 (IEEE) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |c, b| CRC.get(((c ^ u32::from(*b)) & 0xFF) as usize).copied().unwrap_or(0) ^ (c >> 8))
}

fn u16le(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn u32le(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// 1 January 1980, 00:00: entries carry no clock time, so the same files give the same archive.
const DOS_DATE: u16 = 0x21;

/// A ZIP archive of `entries` (path inside the archive with `/` separators, contents).
pub fn write(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    if entries.len() > MAX_ENTRIES {
        return Err(format!("more than {MAX_ENTRIES} files"));
    }
    let too_big = || "the archive would be larger than 4 GiB".to_string();
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let name_bytes = name.as_bytes();
        let name_len = u16::try_from(name_bytes.len()).map_err(|_| format!("{name}: name too long"))?;
        let crc = crc32(data);
        let deflated = miniz_oxide::deflate::compress_to_vec(data, 6);
        let (method, body): (u16, &[u8]) = if deflated.len() < data.len() { (8, &deflated) } else { (0, data) };
        let size = u32::try_from(data.len()).map_err(|_| too_big())?;
        let packed = u32::try_from(body.len()).map_err(|_| too_big())?;
        let offset = u32::try_from(out.len()).map_err(|_| too_big())?;
        // Local file header.
        u32le(&mut out, 0x0403_4b50);
        u16le(&mut out, 20);
        u16le(&mut out, 1 << 11); // UTF-8 names
        u16le(&mut out, method);
        u16le(&mut out, 0);
        u16le(&mut out, DOS_DATE);
        u32le(&mut out, crc);
        u32le(&mut out, packed);
        u32le(&mut out, size);
        u16le(&mut out, name_len);
        u16le(&mut out, 0);
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(body);
        // Its central directory record.
        u32le(&mut central, 0x0201_4b50);
        u16le(&mut central, 20);
        u16le(&mut central, 20);
        u16le(&mut central, 1 << 11);
        u16le(&mut central, method);
        u16le(&mut central, 0);
        u16le(&mut central, DOS_DATE);
        u32le(&mut central, crc);
        u32le(&mut central, packed);
        u32le(&mut central, size);
        u16le(&mut central, name_len);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u32le(&mut central, 0);
        u32le(&mut central, offset);
        central.extend_from_slice(name_bytes);
    }
    let cd_offset = u32::try_from(out.len()).map_err(|_| too_big())?;
    let cd_size = u32::try_from(central.len()).map_err(|_| too_big())?;
    out.extend_from_slice(&central);
    let n = entries.len() as u16;
    u32le(&mut out, 0x0605_4b50);
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, n);
    u16le(&mut out, n);
    u32le(&mut out, cd_size);
    u32le(&mut out, cd_offset);
    u16le(&mut out, 0);
    if out.len() > u32::MAX as usize {
        return Err(too_big());
    }
    Ok(out)
}

fn get_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}
fn get_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// The entries of ZIP archive `b` (stored and Deflate ones; others are an error), checked
/// against their CRC.
pub fn read(b: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let bad = || "not a ZIP archive this reader understands".to_string();
    // The end of central directory record: within the last 64 KiB + 22 bytes.
    let start = b.len().saturating_sub(0xFFFF + 22);
    let eocd = (start..b.len().saturating_sub(21)).rev().find(|i| get_u32(b, *i) == Some(0x0605_4b50)).ok_or_else(bad)?;
    let n = usize::from(get_u16(b, eocd + 10).ok_or_else(bad)?);
    let mut at = get_u32(b, eocd + 16).ok_or_else(bad)? as usize;
    let mut out = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        if get_u32(b, at) != Some(0x0201_4b50) {
            return Err(bad());
        }
        let method = get_u16(b, at + 10).ok_or_else(bad)?;
        let crc = get_u32(b, at + 16).ok_or_else(bad)?;
        let packed = get_u32(b, at + 20).ok_or_else(bad)? as usize;
        let size = get_u32(b, at + 24).ok_or_else(bad)? as usize;
        let name_len = usize::from(get_u16(b, at + 28).ok_or_else(bad)?);
        let extra = usize::from(get_u16(b, at + 30).ok_or_else(bad)?);
        let comment = usize::from(get_u16(b, at + 32).ok_or_else(bad)?);
        let local = get_u32(b, at + 42).ok_or_else(bad)? as usize;
        let name = String::from_utf8_lossy(b.get(at + 46..at + 46 + name_len).ok_or_else(bad)?).into_owned();
        at += 46 + name_len + extra + comment;
        if get_u32(b, local) != Some(0x0403_4b50) {
            return Err(bad());
        }
        let lname = usize::from(get_u16(b, local + 26).ok_or_else(bad)?);
        let lextra = usize::from(get_u16(b, local + 28).ok_or_else(bad)?);
        let data_at = local + 30 + lname + lextra;
        let body = b.get(data_at..data_at.checked_add(packed).ok_or_else(bad)?).ok_or_else(bad)?;
        if size > MAX_ENTRY_BYTES {
            return Err(format!("{name}: larger than {} MB", MAX_ENTRY_BYTES >> 20));
        }
        let data = match method {
            0 => body.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(body, size).map_err(|_| format!("{name}: damaged"))?,
            m => return Err(format!("{name}: compression method {m} isn't supported")),
        };
        if data.len() != size || crc32(&data) != crc {
            return Err(format!("{name}: damaged (CRC)"));
        }
        out.push((name, data));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn archives_round_trip_and_damage_is_found() {
        let entries = vec![
            ("a.txt".to_string(), b"hello hello hello hello hello hello".to_vec()),
            ("dir/é.bin".to_string(), vec![1, 2, 3]),
            ("empty".to_string(), Vec::new()),
        ];
        let z = write(&entries).unwrap();
        assert_eq!(read(&z).unwrap(), entries);
        let mut bad = z.clone();
        if let Some(b) = bad.get_mut(40) {
            *b ^= 0xFF;
        }
        assert!(read(&bad).is_err());
        assert!(read(b"PK").is_err() && read(&[]).is_err());
    }
}
