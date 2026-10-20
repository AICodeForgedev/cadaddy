//! The classes section's entries (spec §10, §5.8): the custom classes objects of type 500 and up
//! refer to.
//!
//! R2000: entries until the data ends. R2004: a maximum class number, two zero bytes and a bit
//! first, and four more fields per entry. From R2007 the data starts with an RL size in bits
//! and the strings are in a string stream located from that end bit as in the header
//! ([`crate::header`]).

use crate::bits::{BitReader, BitWriter, Text};
use crate::error::corrupt;
use crate::{Result, Version};

/// Most classes read.
const MAX_CLASSES: usize = 1 << 16;

/// One class definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Class {
    /// The object type number objects of this class have (500 and up).
    pub number: i16,
    /// R2000: the version field; R2004+: the proxy flags.
    pub proxy_flags: i16,
    pub app_name: Text,
    pub cpp_name: Text,
    pub dxf_name: Text,
    pub was_zombie: bool,
    /// 0x1F2 for classes of entities, 0x1F3 for classes of objects.
    pub item_class_id: i16,
    /// R2004+: the instance count, DWG version, maintenance version and two unknown longs.
    pub instances: i32,
    pub dwg_version: i32,
    pub maintenance: i32,
    pub unknown: [i32; 2],
}

/// The decoded classes section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classes {
    pub version: Version,
    /// R2004+: the largest class number.
    pub max_number: i32,
    pub classes: Vec<Class>,
    /// R2004: the two bytes and the bit before the entries.
    lead: (u8, u8, bool),
    /// Bits after the last entry in the data and string streams.
    rest: (Vec<bool>, Vec<bool>),
}

fn rest_bits(r: &mut BitReader, end: usize) -> Result<Vec<bool>> {
    (r.bit_pos()..end).map(|_| r.b()).collect()
}

impl Classes {
    /// Decodes the classes section's data (the framed section's body).
    pub fn decode(body: &[u8], version: Version) -> Result<Classes> {
        let unicode = version.unicode();
        let mut data = BitReader::new(body);
        let (mut strings, data_end, str_end) = if unicode {
            let end_bit = data.rl()? as usize;
            if end_bit > body.len() * 8 || end_bit < 49 {
                return Err(corrupt("the classes' size in bits"));
            }
            let (start, end) = crate::bits::string_stream(body, end_bit)?.unwrap_or((end_bit - 1, end_bit - 1));
            let mut s = BitReader::new(body);
            s.set_bit_pos(start)?;
            s.set_bit_end(end)?;
            (Some(s), start, end)
        } else {
            (None, body.len() * 8, 0)
        };
        data.set_bit_end(data_end)?;
        let r2004 = version >= Version::R2004;
        let (max_number, lead) = if unicode {
            (data.bl()?, (0, 0, data.b()?))
        } else if r2004 {
            (i32::from(data.bs()?), (data.rc()?, data.rc()?, data.b()?))
        } else {
            (0, (0, 0, false))
        };
        let mut classes = Vec::new();
        // An entry needs well over a byte; fewer bits left are padding.
        while data.remaining_bits() >= 16 {
            let start = data.bit_pos();
            let mut text = |data: &mut BitReader| match strings.as_mut() {
                Some(s) => s.tv(unicode),
                None => data.tv(unicode),
            };
            let entry = (|| -> Result<Class> {
                let number = data.bs()?;
                let proxy_flags = data.bs()?;
                let app_name = text(&mut data)?;
                let cpp_name = text(&mut data)?;
                let dxf_name = text(&mut data)?;
                let was_zombie = data.b()?;
                let item_class_id = data.bs()?;
                let (mut instances, mut dwg_version, mut maintenance, mut unknown) = (0, 0, 0, [0, 0]);
                if r2004 {
                    instances = data.bl()?;
                    dwg_version = data.bl()?;
                    maintenance = data.bl()?;
                    unknown = [data.bl()?, data.bl()?];
                }
                Ok(Class {
                    number,
                    proxy_flags,
                    app_name,
                    cpp_name,
                    dxf_name,
                    was_zombie,
                    item_class_id,
                    instances,
                    dwg_version,
                    maintenance,
                    unknown,
                })
            })();
            match entry {
                Ok(c) if (500..=i16::MAX).contains(&c.number) => classes.push(c),
                // Not an entry: the padding after the last one.
                _ => {
                    data.set_bit_pos(start)?;
                    break;
                }
            }
            if classes.len() > MAX_CLASSES {
                return Err(crate::Error::TooLarge("the classes section"));
            }
        }
        let rest_data = rest_bits(&mut data, data_end)?;
        let rest_strings = match strings.as_mut() {
            Some(s) => rest_bits(s, str_end)?,
            None => Vec::new(),
        };
        Ok(Classes { version, max_number, classes, lead, rest: (rest_data, rest_strings) })
    }

    /// Encodes the classes as the section's data.
    pub fn encode(&self) -> Vec<u8> {
        let unicode = self.version.unicode();
        let r2004 = self.version >= Version::R2004;
        let mut data = BitWriter::new();
        let mut strings = BitWriter::new();
        if unicode {
            data.rl(0);
            data.bl(self.max_number);
            data.b(self.lead.2);
        } else if r2004 {
            data.bs(self.max_number as i16);
            data.rc(self.lead.0);
            data.rc(self.lead.1);
            data.b(self.lead.2);
        }
        for c in &self.classes {
            data.bs(c.number);
            data.bs(c.proxy_flags);
            for t in [&c.app_name, &c.cpp_name, &c.dxf_name] {
                if unicode { &mut strings } else { &mut data }.tv(t, unicode);
            }
            data.b(c.was_zombie);
            data.bs(c.item_class_id);
            if r2004 {
                for v in [c.instances, c.dwg_version, c.maintenance, c.unknown[0], c.unknown[1]] {
                    data.bl(v);
                }
            }
        }
        self.rest.0.iter().for_each(|&b| data.b(b));
        if !unicode {
            return data.into_bytes();
        }
        self.rest.1.iter().for_each(|&b| strings.b(b));
        crate::bits::put_string_stream(&mut data, strings.as_bytes(), strings.bit_len());
        let end_bit = data.bit_len() as u32;
        let mut bytes = data.into_bytes();
        if let Some(slot) = bytes.get_mut(..4) {
            slot.copy_from_slice(&end_bit.to_le_bytes());
        }
        bytes
    }

    /// The class with type number `number`.
    pub fn get(&self, number: u16) -> Option<&Class> {
        self.classes.iter().find(|c| c.number as u16 == number)
    }
}
