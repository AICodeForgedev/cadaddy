//! Bit-coded values (spec §2): a reader and a writer over a byte buffer, most significant bit of
//! each byte first. Raw values (RC, RS, RL, RD, RLL) are little-endian and need not start on a
//! byte boundary. The writer picks the shortest form the spec allows for each value, so a value
//! read and written again round-trips; byte-for-byte copies of whole streams keep the bytes.

use crate::{Error, Result};

/// A handle reference (spec §2.13): a 4-bit code and the handle or offset it carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct HandleRef {
    pub code: u8,
    pub value: u64,
}

impl HandleRef {
    /// The handle this reference points to, given the referencing object's handle (spec §2.13:
    /// codes 6, 8, 0xA and 0xC are offsets from it; other codes carry the handle itself).
    pub fn absolute(self, from: u64) -> Option<u64> {
        match self.code {
            0x6 => from.checked_add(1),
            0x8 => from.checked_sub(1),
            0xA => from.checked_add(self.value),
            0xC => from.checked_sub(self.value),
            _ => Some(self.value),
        }
    }
}

/// A string as stored: code-page bytes (T, up to R2004) or UTF-16 units (TU, R2007 on). Kept as
/// stored so it writes back unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Text {
    #[default]
    Empty,
    Bytes(Vec<u8>),
    Wide(Vec<u16>),
}

impl Text {
    /// The string, decoding code-page bytes as Latin-1 (code pages are mapped by a later layer).
    pub fn to_string_lossy(&self) -> String {
        match self {
            Text::Empty => String::new(),
            Text::Bytes(b) => b.iter().map(|&c| char::from(c)).collect(),
            Text::Wide(w) => String::from_utf16_lossy(w),
        }
    }

    /// Stored characters (bytes or UTF-16 units).
    pub fn len(&self) -> usize {
        match self {
            Text::Empty => 0,
            Text::Bytes(b) => b.len(),
            Text::Wide(w) => w.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Most bytes a modular char or short may run to (a 64-bit value needs ten 7-bit groups).
const MAX_MODULAR_BYTES: usize = 10;
/// Most 15-bit modules in a modular short (sizes: two make a 30-bit size, spec §2.7).
const MAX_MODULAR_SHORTS: usize = 4;

/// Reads bit-coded values from a byte buffer.
#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Position in bits from the start of `data`.
    pos: usize,
    /// End of the readable bits (at most `data.len() * 8`).
    end: usize,
    /// Most characters one string may have.
    max_chars: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, end: data.len().saturating_mul(8), max_chars: crate::Limits::default().max_string_chars }
    }

    /// Caps strings at `max_chars` characters (longer ones are an error).
    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars;
        self
    }

    /// Bit position from the start of the buffer.
    pub fn bit_pos(&self) -> usize {
        self.pos
    }

    /// Moves to a bit position (at most the end).
    pub fn set_bit_pos(&mut self, pos: usize) -> Result<()> {
        if pos > self.end {
            return Err(Error::Truncated("a bit stream (seek past its end)"));
        }
        self.pos = pos;
        Ok(())
    }

    /// Stops reading at bit `end` (an object's data ends before its handles).
    pub fn set_bit_end(&mut self, end: usize) -> Result<()> {
        if end > self.data.len().saturating_mul(8) || end < self.pos {
            return Err(crate::error::corrupt("a stream end outside its data"));
        }
        self.end = end;
        Ok(())
    }

    pub fn bit_end(&self) -> usize {
        self.end
    }

    /// Bits left to read.
    pub fn remaining_bits(&self) -> usize {
        self.end.saturating_sub(self.pos)
    }

    /// Byte position (rounded down).
    pub fn byte_pos(&self) -> usize {
        self.pos / 8
    }

    /// Skips to the next byte boundary.
    pub fn align(&mut self) {
        self.pos = self.pos.div_ceil(8).saturating_mul(8).min(self.end);
    }

    /// B: one bit.
    pub fn b(&mut self) -> Result<bool> {
        if self.pos >= self.end {
            return Err(Error::Truncated("a bit stream"));
        }
        let byte = self.data.get(self.pos / 8).copied().ok_or(Error::Truncated("a bit stream"))?;
        let bit = byte >> (7 - (self.pos % 8)) & 1;
        self.pos += 1;
        Ok(bit == 1)
    }

    /// `n` bits (at most 64), first read most significant.
    pub fn bits(&mut self, n: u32) -> Result<u64> {
        if n > 64 {
            return Err(crate::error::corrupt("more than 64 bits asked for at once"));
        }
        if self.remaining_bits() < n as usize {
            return Err(Error::Truncated("a bit stream"));
        }
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 1) | u64::from(self.b()?);
        }
        Ok(v)
    }

    /// BB: a 2-bit code.
    pub fn bb(&mut self) -> Result<u8> {
        Ok(self.bits(2)? as u8)
    }

    /// 3B: one to three bits, stopping at the first 0 (spec §2.1).
    pub fn b3(&mut self) -> Result<u8> {
        let mut v = 0u8;
        for _ in 0..3 {
            let bit = self.b()?;
            v = (v << 1) | u8::from(bit);
            if !bit {
                break;
            }
        }
        Ok(v)
    }

    /// RC: a raw byte.
    pub fn rc(&mut self) -> Result<u8> {
        if self.pos.is_multiple_of(8) && self.remaining_bits() >= 8 {
            let b = self.data.get(self.pos / 8).copied().ok_or(Error::Truncated("a bit stream"))?;
            self.pos += 8;
            return Ok(b);
        }
        Ok(self.bits(8)? as u8)
    }

    /// `n` raw bytes.
    pub fn bytes(&mut self, n: usize) -> Result<Vec<u8>> {
        if self.remaining_bits() / 8 < n {
            return Err(Error::Truncated("a bit stream"));
        }
        if self.pos.is_multiple_of(8) {
            let start = self.pos / 8;
            let out = self.data.get(start..start + n).ok_or(Error::Truncated("a bit stream"))?.to_vec();
            self.pos += n * 8;
            return Ok(out);
        }
        (0..n).map(|_| self.rc()).collect()
    }

    fn le<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0u8; N];
        for b in &mut a {
            *b = self.rc()?;
        }
        Ok(a)
    }

    /// RS: a raw little-endian short.
    pub fn rs(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.le()?))
    }

    /// RL: a raw little-endian long.
    pub fn rl(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.le()?))
    }

    /// RLL: a raw little-endian 64-bit value.
    pub fn rll(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.le()?))
    }

    /// RD: a raw little-endian double.
    pub fn rd(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.le()?))
    }

    /// BS: bit short (spec §2.2).
    pub fn bs(&mut self) -> Result<i16> {
        Ok(match self.bb()? {
            0 => self.rs()? as i16,
            1 => i16::from(self.rc()?),
            2 => 0,
            _ => 256,
        })
    }

    /// BL: bit long (spec §2.3).
    pub fn bl(&mut self) -> Result<i32> {
        match self.bb()? {
            0 => Ok(self.rl()? as i32),
            1 => Ok(i32::from(self.rc()?)),
            2 => Ok(0),
            _ => Err(crate::error::corrupt("a bit long with the unused code 11")),
        }
    }

    /// BLL: bit long long (spec §2.4): a byte count, then that many bytes, least first. The spec
    /// says the count is a 3B (one to three bits); in files from other writers it is a plain
    /// 3-bit number (R2013 header REQUIREDVERSIONS: only then do the values after it read as
    /// their documented defaults).
    pub fn bll(&mut self) -> Result<u64> {
        let n = self.bits(3)?;
        let mut v = 0u64;
        for i in 0..n {
            v |= u64::from(self.rc()?) << (8 * i as u32);
        }
        Ok(v)
    }

    /// BD: bit double (spec §2.5).
    pub fn bd(&mut self) -> Result<f64> {
        match self.bb()? {
            0 => self.rd(),
            1 => Ok(1.0),
            2 => Ok(0.0),
            _ => Err(crate::error::corrupt("a bit double with the unused code 11")),
        }
    }

    /// DD: bit double with default (spec §2.9).
    pub fn dd(&mut self, default: f64) -> Result<f64> {
        let mut b = default.to_le_bytes();
        match self.bb()? {
            0 => {}
            1 => {
                for x in b.iter_mut().take(4) {
                    *x = self.rc()?;
                }
            }
            2 => {
                b[4] = self.rc()?;
                b[5] = self.rc()?;
                for x in b.iter_mut().take(4) {
                    *x = self.rc()?;
                }
            }
            _ => return self.rd(),
        }
        Ok(f64::from_le_bytes(b))
    }

    /// BT: bit thickness (spec §2.10).
    pub fn bt(&mut self, r2000_plus: bool) -> Result<f64> {
        if r2000_plus && self.b()? { Ok(0.0) } else { self.bd() }
    }

    /// BE: bit extrusion (spec §2.8).
    pub fn be(&mut self, r2000_plus: bool) -> Result<[f64; 3]> {
        if r2000_plus && self.b()? {
            return Ok([0.0, 0.0, 1.0]);
        }
        Ok([self.bd()?, self.bd()?, self.bd()?])
    }

    /// 2RD, 3RD, 2BD, 3BD.
    pub fn rd2(&mut self) -> Result<[f64; 2]> {
        Ok([self.rd()?, self.rd()?])
    }
    pub fn rd3(&mut self) -> Result<[f64; 3]> {
        Ok([self.rd()?, self.rd()?, self.rd()?])
    }
    pub fn bd2(&mut self) -> Result<[f64; 2]> {
        Ok([self.bd()?, self.bd()?])
    }
    pub fn bd3(&mut self) -> Result<[f64; 3]> {
        Ok([self.bd()?, self.bd()?, self.bd()?])
    }

    /// MC: modular char, unsigned (spec §2.6; object map handle offsets).
    pub fn mc_unsigned(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for i in 0..MAX_MODULAR_BYTES {
            let b = self.rc()?;
            v |= u64::from(b & 0x7F).checked_shl(7 * i as u32).unwrap_or(0);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(crate::error::corrupt("a modular char longer than ten bytes"))
    }

    /// MC: modular char, signed: bit 0x40 of the last byte is the sign (spec §2.6).
    pub fn mc(&mut self) -> Result<i64> {
        let mut v = 0u64;
        for i in 0..MAX_MODULAR_BYTES {
            let b = self.rc()?;
            let shift = 7 * i as u32;
            if b & 0x80 == 0 {
                let neg = b & 0x40 != 0;
                v |= u64::from(b & 0x3F).checked_shl(shift).unwrap_or(0);
                let v = i64::try_from(v).map_err(|_| crate::error::corrupt("a modular char out of range"))?;
                return Ok(if neg { -v } else { v });
            }
            v |= u64::from(b & 0x7F).checked_shl(shift).unwrap_or(0);
        }
        Err(crate::error::corrupt("a modular char longer than ten bytes"))
    }

    /// MS: modular short, unsigned (spec §2.7; object sizes).
    pub fn ms(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for i in 0..MAX_MODULAR_SHORTS {
            let s = self.rs()?;
            v |= u64::from(s & 0x7FFF) << (15 * i as u32);
            if s & 0x8000 == 0 {
                return Ok(v);
            }
        }
        Err(crate::error::corrupt("a modular short longer than four shorts"))
    }

    /// H: handle reference (spec §2.13): code, byte count, then the bytes most significant first.
    pub fn h(&mut self) -> Result<HandleRef> {
        let code = self.bits(4)? as u8;
        let n = self.bits(4)? as u8;
        if n > 8 {
            return Err(crate::error::corrupt(format!("a handle of {n} bytes")));
        }
        let mut value = 0u64;
        for _ in 0..n {
            value = (value << 8) | u64::from(self.rc()?);
        }
        Ok(HandleRef { code, value })
    }

    /// T: bit-short length, then that many code-page bytes.
    pub fn t(&mut self) -> Result<Text> {
        let n = self.text_len()?;
        if n == 0 {
            return Ok(Text::Empty);
        }
        Ok(Text::Bytes(self.bytes(n)?))
    }

    /// TU: bit-short length, then that many UTF-16 units.
    pub fn tu(&mut self) -> Result<Text> {
        let n = self.text_len()?;
        if n == 0 {
            return Ok(Text::Empty);
        }
        if self.remaining_bits() / 16 < n {
            return Err(Error::Truncated("a string"));
        }
        Ok(Text::Wide((0..n).map(|_| self.rs()).collect::<Result<_>>()?))
    }

    /// TV: T up to R2004, TU from R2007 (spec §2).
    pub fn tv(&mut self, unicode: bool) -> Result<Text> {
        if unicode { self.tu() } else { self.t() }
    }

    fn text_len(&mut self) -> Result<usize> {
        // The length is unsigned: a short with its top bit set is a long string, not a negative one.
        let n = usize::from(self.bs()? as u16);
        if n > self.max_chars {
            return Err(Error::TooLarge("a string"));
        }
        Ok(n)
    }

    /// SN: a 16-byte sentinel; `expected` must match.
    pub fn sentinel(&mut self, expected: &[u8; 16], what: &'static str) -> Result<()> {
        let got = self.bytes(16)?;
        if got.as_slice() != expected {
            return Err(Error::Corrupt(format!("{what}: sentinel missing")));
        }
        Ok(())
    }

    /// OT: object type (spec §2.12): a bit short up to R2007, a 2-bit code and one or two bytes from
    /// R2010.
    pub fn ot(&mut self, r2010_plus: bool) -> Result<u16> {
        if !r2010_plus {
            return Ok(self.bs()? as u16);
        }
        Ok(match self.bb()? {
            0 => u16::from(self.rc()?),
            1 => u16::from(self.rc()?) + 0x1F0,
            _ => self.rs()?,
        })
    }
}

/// Writes bit-coded values.
#[derive(Clone, Debug, Default)]
pub struct BitWriter {
    data: Vec<u8>,
    /// Length in bits.
    len: usize,
}

impl BitWriter {
    pub fn new() -> Self {
        BitWriter::default()
    }

    /// Length in bits.
    pub fn bit_len(&self) -> usize {
        self.len
    }

    /// The bytes written (the last one padded with zero bits).
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Pads with zero bits to a byte boundary.
    pub fn align(&mut self) {
        self.len = self.data.len() * 8;
    }

    pub fn b(&mut self, bit: bool) {
        if self.len.is_multiple_of(8) {
            self.data.push(0);
        }
        if bit && let Some(last) = self.data.last_mut() {
            *last |= 1 << (7 - (self.len % 8));
        }
        self.len += 1;
    }

    /// The low `n` bits of `v` (at most 64), most significant first.
    pub fn bits(&mut self, v: u64, n: u32) {
        for i in (0..n.min(64)).rev() {
            self.b(v >> i & 1 == 1);
        }
    }

    pub fn bb(&mut self, v: u8) {
        self.bits(u64::from(v), 2);
    }

    /// 3B for 0, 2, 6 or 7 (other values are written as the nearest smaller form).
    pub fn b3(&mut self, v: u8) {
        match v {
            0 | 1 => self.b(false),
            2..=5 => {
                self.b(true);
                self.b(false);
            }
            6 => self.bits(0b110, 3),
            _ => self.bits(0b111, 3),
        }
    }

    pub fn rc(&mut self, v: u8) {
        if self.len.is_multiple_of(8) {
            self.data.push(v);
            self.len += 8;
        } else {
            self.bits(u64::from(v), 8);
        }
    }

    pub fn bytes(&mut self, v: &[u8]) {
        if self.len.is_multiple_of(8) {
            self.data.extend_from_slice(v);
            self.len += v.len() * 8;
        } else {
            for &b in v {
                self.rc(b);
            }
        }
    }

    pub fn rs(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn rl(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn rll(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn rd(&mut self, v: f64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn bs(&mut self, v: i16) {
        match v {
            0 => self.bb(2),
            256 => self.bb(3),
            1..=255 => {
                self.bb(1);
                self.rc(v as u8);
            }
            _ => {
                self.bb(0);
                self.rs(v as u16);
            }
        }
    }

    pub fn bl(&mut self, v: i32) {
        match v {
            0 => self.bb(2),
            1..=255 => {
                self.bb(1);
                self.rc(v as u8);
            }
            _ => {
                self.bb(0);
                self.rl(v as u32);
            }
        }
    }

    /// BLL with the fewest bytes (at most seven: the count is three bits).
    pub fn bll(&mut self, v: u64) {
        let n = (8 - v.leading_zeros() / 8).min(7);
        self.bits(u64::from(n), 3);
        self.bytes(&v.to_le_bytes()[..n as usize]);
    }

    pub fn bd(&mut self, v: f64) {
        match v.to_bits() {
            0 => self.bb(2),
            b if b == 1.0f64.to_bits() => self.bb(1),
            _ => {
                self.bb(0);
                self.rd(v);
            }
        }
    }

    pub fn dd(&mut self, v: f64, default: f64) {
        let (b, d) = (v.to_le_bytes(), default.to_le_bytes());
        if b == d {
            self.bb(0);
        } else if b[4..] == d[4..] {
            self.bb(1);
            self.bytes(&b[..4]);
        } else if b[6..] == d[6..] {
            self.bb(2);
            self.bytes(&b[4..6]);
            self.bytes(&b[..4]);
        } else {
            self.bb(3);
            self.rd(v);
        }
    }

    pub fn bt(&mut self, v: f64, r2000_plus: bool) {
        if r2000_plus {
            let zero = v.to_bits() == 0;
            self.b(zero);
            if zero {
                return;
            }
        }
        self.bd(v);
    }

    pub fn be(&mut self, v: [f64; 3], r2000_plus: bool) {
        if r2000_plus {
            let default = v.map(f64::to_bits) == [0.0f64, 0.0, 1.0].map(f64::to_bits);
            self.b(default);
            if default {
                return;
            }
        }
        for c in v {
            self.bd(c);
        }
    }

    pub fn bd2(&mut self, v: [f64; 2]) {
        v.into_iter().for_each(|c| self.bd(c));
    }
    pub fn bd3(&mut self, v: [f64; 3]) {
        v.into_iter().for_each(|c| self.bd(c));
    }
    pub fn rd2(&mut self, v: [f64; 2]) {
        v.into_iter().for_each(|c| self.rd(c));
    }
    pub fn rd3(&mut self, v: [f64; 3]) {
        v.into_iter().for_each(|c| self.rd(c));
    }

    pub fn mc_unsigned(&mut self, mut v: u64) {
        loop {
            let low = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                self.rc(low);
                return;
            }
            self.rc(low | 0x80);
        }
    }

    pub fn mc(&mut self, v: i64) {
        let neg = v < 0;
        let mut m = v.unsigned_abs();
        loop {
            if m < 0x40 {
                self.rc(m as u8 | if neg { 0x40 } else { 0 });
                return;
            }
            self.rc((m & 0x7F) as u8 | 0x80);
            m >>= 7;
        }
    }

    pub fn ms(&mut self, mut v: u64) {
        loop {
            let low = (v & 0x7FFF) as u16;
            v >>= 15;
            if v == 0 {
                self.rs(low);
                return;
            }
            self.rs(low | 0x8000);
        }
    }

    pub fn h(&mut self, h: HandleRef) {
        let n = (8 - h.value.leading_zeros() / 8) as u8;
        self.bits(u64::from(h.code & 0xF), 4);
        self.bits(u64::from(n), 4);
        for i in (0..n).rev() {
            self.rc((h.value >> (8 * u32::from(i))) as u8);
        }
    }

    pub fn t(&mut self, s: &Text) {
        match s {
            Text::Bytes(b) => {
                self.bs(b.len().min(u16::MAX as usize) as u16 as i16);
                self.bytes(&b[..b.len().min(u16::MAX as usize)]);
            }
            Text::Wide(w) => {
                // A UTF-16 string in a code-page slot: write its Latin-1 bytes.
                let b: Vec<u8> = String::from_utf16_lossy(w).chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect();
                self.t(&Text::Bytes(b));
            }
            Text::Empty => self.bs(0),
        }
    }

    pub fn tu(&mut self, s: &Text) {
        match s {
            Text::Wide(w) => {
                let w = &w[..w.len().min(u16::MAX as usize)];
                self.bs(w.len() as u16 as i16);
                for &u in w {
                    self.rs(u);
                }
            }
            Text::Bytes(b) => self.tu(&Text::Wide(b.iter().map(|&c| u16::from(c)).collect())),
            Text::Empty => self.bs(0),
        }
    }

    pub fn tv(&mut self, s: &Text, unicode: bool) {
        if unicode { self.tu(s) } else { self.t(s) }
    }

    pub fn ot(&mut self, v: u16, r2010_plus: bool) {
        if !r2010_plus {
            self.bs(v as i16);
        } else if v < 0x100 {
            self.bb(0);
            self.rc(v as u8);
        } else if (0x1F0..0x1F0 + 0x100).contains(&v) {
            self.bb(1);
            self.rc((v - 0x1F0) as u8);
        } else {
            self.bb(2);
            self.rs(v);
        }
    }
}

/// Where an R2007+ string stream lies (spec §20.1): `end_bit` is the end of the data and string
/// streams. The bit before it says whether there is a string stream; the RS before that bit is
/// its size in bits, and when that size has its top bit set, the RS before it gives the high
/// bits (size = low 15 bits | high << 15). Returns (start, end) of the string stream, or `None`
/// when there is none (the data stream then ends at `end_bit - 1`).
pub fn string_stream(data: &[u8], end_bit: usize) -> Result<Option<(usize, usize)>> {
    if end_bit < 1 || end_bit > data.len().saturating_mul(8) {
        return Err(crate::error::corrupt("a stream end outside its data"));
    }
    let mut r = BitReader::new(data);
    r.set_bit_pos(end_bit - 1)?;
    if !r.b()? {
        return Ok(None);
    }
    let mut end = end_bit.checked_sub(17).ok_or(Error::Truncated("a string stream size"))?;
    r.set_bit_pos(end)?;
    let low = usize::from(r.rs()?);
    let mut size = low;
    if low & 0x8000 != 0 {
        end = end.checked_sub(16).ok_or(Error::Truncated("a string stream size"))?;
        r.set_bit_pos(end)?;
        let high = usize::from(r.rs()?);
        size = (low & 0x7FFF) | (high << 15);
    }
    let start = end.checked_sub(size).ok_or_else(|| crate::error::corrupt("a string stream larger than its object"))?;
    Ok(Some((start, end)))
}

/// Appends a string stream (`bits` of `strings`) and its size and flag to `w`, as
/// [`string_stream`] reads them.
pub fn put_string_stream(w: &mut BitWriter, strings: &[u8], bits: usize) {
    let present = bits > 0;
    let mut r = BitReader::new(strings);
    for _ in 0..bits {
        w.b(r.b().unwrap_or(false));
    }
    if present {
        if bits >= 0x8000 {
            w.rs((bits >> 15) as u16);
            w.rs((bits & 0x7FFF) as u16 | 0x8000);
        } else {
            w.rs(bits as u16);
        }
    }
    w.b(present);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bits written as a string of 0s and 1s, padded with zeros to whole bytes.
    fn from_bits(s: &str) -> Vec<u8> {
        let mut w = BitWriter::new();
        s.chars().filter(|c| !c.is_whitespace()).for_each(|c| w.b(c == '1'));
        w.into_bytes()
    }

    #[test]
    fn the_spec_examples_read() {
        // Spec §2.2: five bit shorts.
        let d = from_bits("0000000001000000011011010000111110");
        let mut r = BitReader::new(&d);
        assert_eq!([r.bs(), r.bs(), r.bs(), r.bs(), r.bs()].map(Result::unwrap), [257, 0, 256, 15, 0]);
        // Spec §2.3: four bit longs.
        let d = from_bits("000000000100000001000000000000000010010000111110");
        let mut r = BitReader::new(&d);
        assert_eq!([r.bl(), r.bl(), r.bl(), r.bl()].map(Result::unwrap), [257, 0, 15, 0]);
        // Spec §2.6: modular chars 10000010 00100100 = 4610; 11101001 10010111 11100110 00110101
        // = 112823273; 10000101 01001011 = -1413 (spec §2.6's negative example).
        let mut r = BitReader::new(&[0b1000_0010, 0b0010_0100]);
        assert_eq!(r.mc().unwrap(), 4610);
        let mut r = BitReader::new(&[0b1110_1001, 0b1001_0111, 0b1110_0110, 0b0011_0101]);
        assert_eq!(r.mc().unwrap(), 112_823_273);
        let mut r = BitReader::new(&[0b1000_0101, 0b0100_1011]);
        assert_eq!(r.mc().unwrap(), -1413);
        // Spec §2.7: modular short 00110001 11110100 10001101 00000000 = 4650033.
        let mut r = BitReader::new(&[0b0011_0001, 0b1111_0100, 0b1000_1101, 0b0000_0000]);
        assert_eq!(r.ms().unwrap(), 4_650_033);
        // Spec §2.13: 5.2.05.E7, a hard pointer to handle 0x5E7.
        let mut r = BitReader::new(&[0b0101_0010, 0b0000_0101, 0b1110_0111]);
        assert_eq!(r.h().unwrap(), HandleRef { code: 5, value: 0x5E7 });
        // Spec §2.14.1: a bit short of 3 then padding.
        let mut r = BitReader::new(&[0b0100_0000, 0b1110_0000]);
        assert_eq!(r.bs().unwrap(), 3);
    }

    #[test]
    fn every_kind_round_trips_off_byte_boundaries() {
        let mut w = BitWriter::new();
        w.b(true);
        w.bb(2);
        for v in [0i16, 1, 255, 256, 257, -1, i16::MIN, i16::MAX] {
            w.bs(v);
        }
        for v in [0i32, 7, 255, 256, -1, i32::MAX, i32::MIN] {
            w.bl(v);
        }
        for v in [0u64, 1, 0xFF, 0x1_0000, 0xFFFF_FFFF_FFFF, u64::MAX >> 8] {
            w.bll(v);
        }
        for v in [0.0, 1.0, -0.0, 2.5, f64::MAX, -1e-300] {
            w.bd(v);
        }
        for (v, d) in [(1.0, 1.0), (1.000_000_1, 1.0), (3.0, 2.0), (-7.25, 412_148_564_080.0)] {
            w.dd(v, d);
        }
        w.bt(0.0, true);
        w.bt(4.5, true);
        w.be([0.0, 0.0, 1.0], true);
        w.be([0.0, 1.0, 0.0], true);
        for v in [0i64, 63, 64, -64, -1413, 4610, i64::MAX / 2, -(i64::MAX / 2)] {
            w.mc(v);
        }
        for v in [0u64, 127, 128, 0xFFFF_FFFF] {
            w.mc_unsigned(v);
        }
        for v in [0u64, 0x7FFF, 0x8000, 4_650_033] {
            w.ms(v);
        }
        for h in [HandleRef { code: 5, value: 0x5E7 }, HandleRef { code: 6, value: 0 }, HandleRef { code: 3, value: u64::MAX }] {
            w.h(h);
        }
        w.t(&Text::Bytes(b"LAYER 0".to_vec()));
        w.tu(&Text::Wide("Ünïcode ✓".encode_utf16().collect()));
        w.t(&Text::Empty);
        for v in [0u16, 0x1F3, 0x1F0 + 0xFF, 0x1F0 + 0x100, 0x7FF] {
            w.ot(v, true);
        }
        w.ot(500, false);
        w.rd3([1.0, -2.0, 3.5]);
        let bytes = w.into_bytes();

        let mut r = BitReader::new(&bytes);
        assert!(r.b().unwrap());
        assert_eq!(r.bb().unwrap(), 2);
        for v in [0i16, 1, 255, 256, 257, -1, i16::MIN, i16::MAX] {
            assert_eq!(r.bs().unwrap(), v);
        }
        for v in [0i32, 7, 255, 256, -1, i32::MAX, i32::MIN] {
            assert_eq!(r.bl().unwrap(), v);
        }
        for v in [0u64, 1, 0xFF, 0x1_0000, 0xFFFF_FFFF_FFFF, u64::MAX >> 8] {
            assert_eq!(r.bll().unwrap(), v);
        }
        for v in [0.0f64, 1.0, -0.0, 2.5, f64::MAX, -1e-300] {
            assert_eq!(r.bd().unwrap().to_bits(), v.to_bits());
        }
        for (v, d) in [(1.0f64, 1.0), (1.000_000_1, 1.0), (3.0, 2.0), (-7.25, 412_148_564_080.0)] {
            assert_eq!(r.dd(d).unwrap().to_bits(), v.to_bits());
        }
        assert_eq!(r.bt(true).unwrap(), 0.0);
        assert_eq!(r.bt(true).unwrap(), 4.5);
        assert_eq!(r.be(true).unwrap(), [0.0, 0.0, 1.0]);
        assert_eq!(r.be(true).unwrap(), [0.0, 1.0, 0.0]);
        for v in [0i64, 63, 64, -64, -1413, 4610, i64::MAX / 2, -(i64::MAX / 2)] {
            assert_eq!(r.mc().unwrap(), v);
        }
        for v in [0u64, 127, 128, 0xFFFF_FFFF] {
            assert_eq!(r.mc_unsigned().unwrap(), v);
        }
        for v in [0u64, 0x7FFF, 0x8000, 4_650_033] {
            assert_eq!(r.ms().unwrap(), v);
        }
        for h in [HandleRef { code: 5, value: 0x5E7 }, HandleRef { code: 6, value: 0 }, HandleRef { code: 3, value: u64::MAX }] {
            assert_eq!(r.h().unwrap(), h);
        }
        assert_eq!(r.t().unwrap().to_string_lossy(), "LAYER 0");
        assert_eq!(r.tu().unwrap().to_string_lossy(), "Ünïcode ✓");
        assert_eq!(r.t().unwrap(), Text::Empty);
        for v in [0u16, 0x1F3, 0x1F0 + 0xFF, 0x1F0 + 0x100, 0x7FF] {
            assert_eq!(r.ot(true).unwrap(), v);
        }
        assert_eq!(r.ot(false).unwrap(), 500);
        assert_eq!(r.rd3().unwrap(), [1.0, -2.0, 3.5]);
    }

    #[test]
    fn compact_forms_are_chosen() {
        let len = |f: &dyn Fn(&mut BitWriter)| {
            let mut w = BitWriter::new();
            f(&mut w);
            w.bit_len()
        };
        assert_eq!(len(&|w| w.bs(0)), 2);
        assert_eq!(len(&|w| w.bs(15)), 10);
        assert_eq!(len(&|w| w.bs(-1)), 18, "negatives use the short form");
        assert_eq!(len(&|w| w.bd(1.0)), 2);
        assert_eq!(len(&|w| w.dd(5.0, 5.0)), 2);
        assert_eq!(len(&|w| w.h(HandleRef { code: 5, value: 0 })), 8);
    }

    #[test]
    fn hostile_input_is_an_error_not_a_panic() {
        assert!(BitReader::new(&[]).b().is_err());
        assert!(BitReader::new(&[0xFF]).bl().is_err(), "code 11 unused, then truncated");
        assert!(BitReader::new(&[0xC0]).bd().is_err());
        assert!(BitReader::new(&[0xFF; 12]).mc().is_err(), "endless modular char");
        assert!(BitReader::new(&[0xFF; 12]).ms().is_err());
        assert!(BitReader::new(&[0x5F, 0, 0]).h().is_err(), "15-byte handle");
        // A string claiming 65535 characters in a short buffer.
        let mut w = BitWriter::new();
        w.bs(-1);
        assert!(BitReader::new(w.as_bytes()).t().is_err());
        assert!(BitReader::new(w.as_bytes()).with_max_chars(10).tu().is_err());
        let mut r = BitReader::new(&[1, 2, 3]);
        assert!(r.set_bit_pos(25).is_err() && r.set_bit_end(25).is_err());
        assert!(r.bits(65).is_err());
        // Arbitrary bytes through every reader: errors or values, never a panic.
        let mut seed = 0x1234_5678u32;
        for _ in 0..2000 {
            let data: Vec<u8> = (0..(seed % 40) as usize)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (seed >> 16) as u8
                })
                .collect();
            let mut r = BitReader::new(&data);
            while r.remaining_bits() > 0 {
                let k = r.bit_pos() % 13;
                let ok = match k {
                    0 => r.bs().is_ok(),
                    1 => r.bl().is_ok(),
                    2 => r.bll().is_ok(),
                    3 => r.bd().is_ok(),
                    4 => r.dd(1.0).is_ok(),
                    5 => r.mc().is_ok(),
                    6 => r.ms().is_ok(),
                    7 => r.h().is_ok(),
                    8 => r.t().is_ok(),
                    9 => r.tu().is_ok(),
                    10 => r.be(true).is_ok(),
                    11 => r.ot(true).is_ok(),
                    _ => r.b3().is_ok(),
                };
                if !ok {
                    break;
                }
            }
        }
    }
}
