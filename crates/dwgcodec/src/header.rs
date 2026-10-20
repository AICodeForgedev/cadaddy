//! The header variables (spec §9), decoded into named values and encoded back.
//!
//! Up to R2004 the variables are one bit stream. From R2007 (spec §5.9) strings go to a string
//! stream and handles to a handle stream, as in objects (spec §20.1): the data starts with an RL
//! "size in bits" (the end of the data and string streams); the last bit before that end says
//! whether there is a string stream, and the RS before that bit gives its size in bits; the
//! handle stream starts at the end bit. The spec lists the RL as "R2007 only", but R2010+ files
//! have it too (their streams line up only with it). HANDSEED stays in the data stream.
//!
//! Bits after the last variable of each stream (padding, or values this table does not name)
//! are kept, so encoding what was decoded gives the same bytes.

use crate::bits::{BitReader, BitWriter, HandleRef, Text};
use crate::error::corrupt;
use crate::{Result, Version};

/// Which versions a variable is stored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Since {
    All,
    /// R2000 only ("pre-2004", "R13-R15").
    R2000Only,
    /// R2000 and R2004 ("R13-R18").
    UpToR2004,
    R2004,
    R2007,
    R2010,
    R2013,
}

impl Since {
    fn applies(self, v: Version) -> bool {
        match self {
            Since::All => true,
            Since::R2000Only => v == Version::R2000,
            Since::UpToR2004 => v <= Version::R2004,
            Since::R2004 => v >= Version::R2004,
            Since::R2007 => v >= Version::R2007,
            Since::R2010 => v >= Version::R2010,
            Since::R2013 => v >= Version::R2013,
        }
    }
}

/// How a variable is coded (spec §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    B,
    Bs,
    Bl,
    Bll,
    Bd,
    Rc,
    Rd2,
    Bd3,
    Tv,
    /// A handle in the handle stream (from R2007).
    H,
    /// A handle kept in the data stream (HANDSEED).
    HData,
    /// A handle present only when the previous variable is 3 (CPSNID after CEPSNTYPE).
    HIfPrev3,
    Cmc,
}

use Kind::*;
use Since::*;

/// The header variables in stored order (spec §9). Variables the spec calls unknown are named
/// UNKNOWN_n.
const VARS: &[(Since, &str, Kind)] = &[
    (R2013, "REQUIREDVERSIONS", Bll),
    (All, "UNKNOWN_1", Bd),
    (All, "UNKNOWN_2", Bd),
    (All, "UNKNOWN_3", Bd),
    (All, "UNKNOWN_4", Bd),
    (All, "UNKNOWN_5", Tv),
    (All, "UNKNOWN_6", Tv),
    (All, "UNKNOWN_7", Tv),
    (All, "UNKNOWN_8", Tv),
    (All, "UNKNOWN_9", Bl),
    (All, "UNKNOWN_10", Bl),
    (R2000Only, "VIEWPORT_ENTITY_HEADER", H),
    (All, "DIMASO", B),
    (All, "DIMSHO", B),
    (All, "PLINEGEN", B),
    (All, "ORTHOMODE", B),
    (All, "REGENMODE", B),
    (All, "FILLMODE", B),
    (All, "QTEXTMODE", B),
    (All, "PSLTSCALE", B),
    (All, "LIMCHECK", B),
    (R2004, "UNKNOWN_11", B),
    (All, "USRTIMER", B),
    (All, "SKPOLY", B),
    (All, "ANGDIR", B),
    (All, "SPLFRAME", B),
    (All, "MIRRTEXT", B),
    (All, "WORLDVIEW", B),
    (All, "TILEMODE", B),
    (All, "PLIMCHECK", B),
    (All, "VISRETAIN", B),
    (All, "DISPSILH", B),
    (All, "PELLIPSE", B),
    (All, "PROXYGRAPHICS", Bs),
    (All, "TREEDEPTH", Bs),
    (All, "LUNITS", Bs),
    (All, "LUPREC", Bs),
    (All, "AUNITS", Bs),
    (All, "AUPREC", Bs),
    (All, "ATTMODE", Bs),
    (All, "PDMODE", Bs),
    (R2004, "UNKNOWN_12", Bl),
    (R2004, "UNKNOWN_13", Bl),
    (R2004, "UNKNOWN_14", Bl),
    (All, "USERI1", Bs),
    (All, "USERI2", Bs),
    (All, "USERI3", Bs),
    (All, "USERI4", Bs),
    (All, "USERI5", Bs),
    (All, "SPLINESEGS", Bs),
    (All, "SURFU", Bs),
    (All, "SURFV", Bs),
    (All, "SURFTYPE", Bs),
    (All, "SURFTAB1", Bs),
    (All, "SURFTAB2", Bs),
    (All, "SPLINETYPE", Bs),
    (All, "SHADEDGE", Bs),
    (All, "SHADEDIF", Bs),
    (All, "UNITMODE", Bs),
    (All, "MAXACTVP", Bs),
    (All, "ISOLINES", Bs),
    (All, "CMLJUST", Bs),
    (All, "TEXTQLTY", Bs),
    (All, "LTSCALE", Bd),
    (All, "TEXTSIZE", Bd),
    (All, "TRACEWID", Bd),
    (All, "SKETCHINC", Bd),
    (All, "FILLETRAD", Bd),
    (All, "THICKNESS", Bd),
    (All, "ANGBASE", Bd),
    (All, "PDSIZE", Bd),
    (All, "PLINEWID", Bd),
    (All, "USERR1", Bd),
    (All, "USERR2", Bd),
    (All, "USERR3", Bd),
    (All, "USERR4", Bd),
    (All, "USERR5", Bd),
    (All, "CHAMFERA", Bd),
    (All, "CHAMFERB", Bd),
    (All, "CHAMFERC", Bd),
    (All, "CHAMFERD", Bd),
    (All, "FACETRES", Bd),
    (All, "CMLSCALE", Bd),
    (All, "CELTSCALE", Bd),
    (UpToR2004, "MENUNAME", Tv),
    (All, "TDCREATE_DAY", Bl),
    (All, "TDCREATE_MS", Bl),
    (All, "TDUPDATE_DAY", Bl),
    (All, "TDUPDATE_MS", Bl),
    (R2004, "UNKNOWN_15", Bl),
    (R2004, "UNKNOWN_16", Bl),
    (R2004, "UNKNOWN_17", Bl),
    (All, "TDINDWG_DAYS", Bl),
    (All, "TDINDWG_MS", Bl),
    (All, "TDUSRTIMER_DAYS", Bl),
    (All, "TDUSRTIMER_MS", Bl),
    (All, "CECOLOR", Cmc),
    (All, "HANDSEED", HData),
    (All, "CLAYER", H),
    (All, "TEXTSTYLE", H),
    (All, "CELTYPE", H),
    (R2007, "CMATERIAL", H),
    (All, "DIMSTYLE", H),
    (All, "CMLSTYLE", H),
    (All, "PSVPSCALE", Bd),
    (All, "PINSBASE", Bd3),
    (All, "PEXTMIN", Bd3),
    (All, "PEXTMAX", Bd3),
    (All, "PLIMMIN", Rd2),
    (All, "PLIMMAX", Rd2),
    (All, "PELEVATION", Bd),
    (All, "PUCSORG", Bd3),
    (All, "PUCSXDIR", Bd3),
    (All, "PUCSYDIR", Bd3),
    (All, "PUCSNAME", H),
    (All, "PUCSORTHOREF", H),
    (All, "PUCSORTHOVIEW", Bs),
    (All, "PUCSBASE", H),
    (All, "PUCSORGTOP", Bd3),
    (All, "PUCSORGBOTTOM", Bd3),
    (All, "PUCSORGLEFT", Bd3),
    (All, "PUCSORGRIGHT", Bd3),
    (All, "PUCSORGFRONT", Bd3),
    (All, "PUCSORGBACK", Bd3),
    (All, "INSBASE", Bd3),
    (All, "EXTMIN", Bd3),
    (All, "EXTMAX", Bd3),
    (All, "LIMMIN", Rd2),
    (All, "LIMMAX", Rd2),
    (All, "ELEVATION", Bd),
    (All, "UCSORG", Bd3),
    (All, "UCSXDIR", Bd3),
    (All, "UCSYDIR", Bd3),
    (All, "UCSNAME", H),
    (All, "UCSORTHOREF", H),
    (All, "UCSORTHOVIEW", Bs),
    (All, "UCSBASE", H),
    (All, "UCSORGTOP", Bd3),
    (All, "UCSORGBOTTOM", Bd3),
    (All, "UCSORGLEFT", Bd3),
    (All, "UCSORGRIGHT", Bd3),
    (All, "UCSORGFRONT", Bd3),
    (All, "UCSORGBACK", Bd3),
    (All, "DIMPOST", Tv),
    (All, "DIMAPOST", Tv),
    (All, "DIMSCALE", Bd),
    (All, "DIMASZ", Bd),
    (All, "DIMEXO", Bd),
    (All, "DIMDLI", Bd),
    (All, "DIMEXE", Bd),
    (All, "DIMRND", Bd),
    (All, "DIMDLE", Bd),
    (All, "DIMTP", Bd),
    (All, "DIMTM", Bd),
    (R2007, "DIMFXL", Bd),
    (R2007, "DIMJOGANG", Bd),
    (R2007, "DIMTFILL", Bs),
    (R2007, "DIMTFILLCLR", Cmc),
    (All, "DIMTOL", B),
    (All, "DIMLIM", B),
    (All, "DIMTIH", B),
    (All, "DIMTOH", B),
    (All, "DIMSE1", B),
    (All, "DIMSE2", B),
    (All, "DIMTAD", Bs),
    (All, "DIMZIN", Bs),
    (All, "DIMAZIN", Bs),
    (R2007, "DIMARCSYM", Bs),
    (All, "DIMTXT", Bd),
    (All, "DIMCEN", Bd),
    (All, "DIMTSZ", Bd),
    (All, "DIMALTF", Bd),
    (All, "DIMLFAC", Bd),
    (All, "DIMTVP", Bd),
    (All, "DIMTFAC", Bd),
    (All, "DIMGAP", Bd),
    (All, "DIMALTRND", Bd),
    (All, "DIMALT", B),
    (All, "DIMALTD", Bs),
    (All, "DIMTOFL", B),
    (All, "DIMSAH", B),
    (All, "DIMTIX", B),
    (All, "DIMSOXD", B),
    (All, "DIMCLRD", Cmc),
    (All, "DIMCLRE", Cmc),
    (All, "DIMCLRT", Cmc),
    (All, "DIMADEC", Bs),
    (All, "DIMDEC", Bs),
    (All, "DIMTDEC", Bs),
    (All, "DIMALTU", Bs),
    (All, "DIMALTTD", Bs),
    (All, "DIMAUNIT", Bs),
    (All, "DIMFRAC", Bs),
    (All, "DIMLUNIT", Bs),
    (All, "DIMDSEP", Bs),
    (All, "DIMTMOVE", Bs),
    (All, "DIMJUST", Bs),
    (All, "DIMSD1", B),
    (All, "DIMSD2", B),
    (All, "DIMTOLJ", Bs),
    (All, "DIMTZIN", Bs),
    (All, "DIMALTZ", Bs),
    (All, "DIMALTTZ", Bs),
    (All, "DIMUPT", B),
    (All, "DIMATFIT", Bs),
    (R2007, "DIMFXLON", B),
    (R2010, "DIMTXTDIRECTION", B),
    (R2010, "DIMALTMZF", Bd),
    (R2010, "DIMALTMZS", Tv),
    (R2010, "DIMMZF", Bd),
    (R2010, "DIMMZS", Tv),
    (All, "DIMTXSTY", H),
    (All, "DIMLDRBLK", H),
    (All, "DIMBLK", H),
    (All, "DIMBLK1", H),
    (All, "DIMBLK2", H),
    (R2007, "DIMLTYPE", H),
    (R2007, "DIMLTEX1", H),
    (R2007, "DIMLTEX2", H),
    (All, "DIMLWD", Bs),
    (All, "DIMLWE", Bs),
    (All, "BLOCK_CONTROL", H),
    (All, "LAYER_CONTROL", H),
    (All, "STYLE_CONTROL", H),
    (All, "LINETYPE_CONTROL", H),
    (All, "VIEW_CONTROL", H),
    (All, "UCS_CONTROL", H),
    (All, "VPORT_CONTROL", H),
    (All, "APPID_CONTROL", H),
    (All, "DIMSTYLE_CONTROL", H),
    (R2000Only, "VIEWPORT_ENTITY_HEADER_CONTROL", H),
    (All, "DICTIONARY_ACAD_GROUP", H),
    (All, "DICTIONARY_ACAD_MLINESTYLE", H),
    (All, "DICTIONARY_NAMED_OBJECTS", H),
    (All, "TSTACKALIGN", Bs),
    (All, "TSTACKSIZE", Bs),
    (All, "HYPERLINKBASE", Tv),
    (All, "STYLESHEET", Tv),
    (All, "DICTIONARY_LAYOUTS", H),
    (All, "DICTIONARY_PLOTSETTINGS", H),
    (All, "DICTIONARY_PLOTSTYLES", H),
    (R2004, "DICTIONARY_MATERIALS", H),
    (R2004, "DICTIONARY_COLORS", H),
    (R2007, "DICTIONARY_VISUALSTYLE", H),
    (R2013, "UNKNOWN_18", H),
    (All, "FLAGS", Bl),
    (All, "INSUNITS", Bs),
    (All, "CEPSNTYPE", Bs),
    (All, "CPSNID", HIfPrev3),
    (All, "FINGERPRINTGUID", Tv),
    (All, "VERSIONGUID", Tv),
    (R2004, "SORTENTS", Rc),
    (R2004, "INDEXCTL", Rc),
    (R2004, "HIDETEXT", Rc),
    (R2004, "XCLIPFRAME", Rc),
    (R2004, "DIMASSOC", Rc),
    (R2004, "HALOGAP", Rc),
    (R2004, "OBSCUREDCOLOR", Bs),
    (R2004, "INTERSECTIONCOLOR", Bs),
    (R2004, "OBSCUREDLTYPE", Rc),
    (R2004, "INTERSECTIONDISPLAY", Rc),
    (R2004, "PROJECTNAME", Tv),
    (All, "BLOCK_RECORD_PAPER_SPACE", H),
    (All, "BLOCK_RECORD_MODEL_SPACE", H),
    (All, "LTYPE_BYLAYER", H),
    (All, "LTYPE_BYBLOCK", H),
    (All, "LTYPE_CONTINUOUS", H),
    (R2007, "CAMERADISPLAY", B),
    (R2007, "UNKNOWN_19", Bl),
    (R2007, "UNKNOWN_20", Bl),
    (R2007, "UNKNOWN_21", Bd),
    (R2007, "STEPSPERSEC", Bd),
    (R2007, "STEPSIZE", Bd),
    (R2007, "3DDWFPREC", Bd),
    (R2007, "LENSLENGTH", Bd),
    (R2007, "CAMERAHEIGHT", Bd),
    (R2007, "SOLIDHIST", Rc),
    (R2007, "SHOWHIST", Rc),
    (R2007, "PSOLWIDTH", Bd),
    (R2007, "PSOLHEIGHT", Bd),
    (R2007, "LOFTANG1", Bd),
    (R2007, "LOFTANG2", Bd),
    (R2007, "LOFTMAG1", Bd),
    (R2007, "LOFTMAG2", Bd),
    (R2007, "LOFTPARAM", Bs),
    (R2007, "LOFTNORMALS", Rc),
    (R2007, "LATITUDE", Bd),
    (R2007, "LONGITUDE", Bd),
    (R2007, "NORTHDIRECTION", Bd),
    (R2007, "TIMEZONE", Bl),
    (R2007, "LIGHTGLYPHDISPLAY", Rc),
    (R2007, "TILEMODELIGHTSYNCH", Rc),
    (R2007, "DWFFRAME", Rc),
    (R2007, "DGNFRAME", Rc),
    (R2007, "UNKNOWN_22", B),
    (R2007, "INTERFERECOLOR", Cmc),
    (R2007, "INTERFEREOBJVS", H),
    (R2007, "INTERFEREVPVS", H),
    (R2007, "DRAGVS", H),
    (R2007, "CSHADOW", Rc),
    (R2007, "UNKNOWN_23", Bd),
];

/// A colour value (CMC, spec §2.11).
#[derive(Clone, Debug, PartialEq)]
pub struct Color {
    pub index: i16,
    /// R2004+: the colour as 0xFLRRGGBB (flag byte, then RGB).
    pub rgb: u32,
    /// R2004+: bit 1, a colour name follows; bit 2, a book name follows.
    pub flags: u8,
    pub name: Text,
    pub book: Text,
}

/// A header variable's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Double(f64),
    Point2([f64; 2]),
    Point3([f64; 3]),
    Text(Text),
    Handle(HandleRef),
    Color(Color),
    /// A conditional variable that is not stored (CPSNID unless CEPSNTYPE is 3).
    Absent,
}

/// The decoded header variables.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderVars {
    pub version: Version,
    /// The variables in stored order.
    pub vars: Vec<(&'static str, Value)>,
    /// Bits after the last variable in each stream (data, strings, handles), kept as stored.
    rest: [Bits; 3],
    /// R2007+: whether there was a string stream.
    strings: bool,
}

/// A run of bits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Bits {
    bytes: Vec<u8>,
    len: usize,
}

fn take_rest(r: &mut BitReader, end: usize) -> Result<Bits> {
    let mut w = BitWriter::new();
    let n = end.saturating_sub(r.bit_pos());
    for _ in 0..n {
        w.b(r.b()?);
    }
    Ok(Bits { len: w.bit_len(), bytes: w.into_bytes() })
}

fn put_bits(w: &mut BitWriter, b: &Bits) {
    let mut r = BitReader::new(&b.bytes);
    for _ in 0..b.len {
        w.b(r.b().unwrap_or(false));
    }
}

impl HeaderVars {
    /// A variable's value.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.vars.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    /// Sets a variable that is stored in this version (false when it is not).
    pub fn set(&mut self, name: &str, value: Value) -> bool {
        match self.vars.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => {
                slot.1 = value;
                true
            }
            None => false,
        }
    }

    /// Decodes the header variables section's data (the framed section's body).
    pub fn decode(body: &[u8], version: Version) -> Result<HeaderVars> {
        let unicode = version.unicode();
        let mut data = BitReader::new(body);
        // From R2007: the streams' end bit, the string stream before it, the handles after it.
        let (mut strings, mut handles, data_end, has_strings, str_end) = if unicode {
            let end_bit = data.rl()? as usize;
            if end_bit > body.len() * 8 || end_bit < 32 + 17 {
                return Err(corrupt("the header's size in bits"));
            }
            let (start, str_end, present) = match crate::bits::string_stream(body, end_bit)? {
                Some((a, b)) => (a, b, true),
                None => (end_bit - 1, end_bit - 1, false),
            };
            let mut s = BitReader::new(body);
            s.set_bit_pos(start)?;
            s.set_bit_end(str_end)?;
            let mut h = BitReader::new(body);
            h.set_bit_pos(end_bit)?;
            (Some(s), Some(h), start, present, str_end)
        } else {
            (None, None, body.len() * 8, false, 0)
        };
        data.set_bit_end(data_end)?;
        let mut vars = Vec::with_capacity(VARS.len());
        for &(since, name, kind) in VARS {
            if !since.applies(version) {
                continue;
            }
            let value = (|| -> Result<Value> {
                let text = |r: &mut BitReader| r.tv(unicode);
                Ok(match kind {
                    B => Value::Bool(data.b()?),
                    Bs => Value::Int(i64::from(data.bs()?)),
                    Bl => Value::Int(i64::from(data.bl()?)),
                    Bll => Value::Int(data.bll()? as i64),
                    Bd => Value::Double(data.bd()?),
                    Rc => Value::Int(i64::from(data.rc()?)),
                    Rd2 => Value::Point2(data.rd2()?),
                    Bd3 => Value::Point3(data.bd3()?),
                    Tv => Value::Text(text(strings.as_mut().unwrap_or(&mut data))?),
                    H => Value::Handle(handles.as_mut().unwrap_or(&mut data).h()?),
                    HData => Value::Handle(data.h()?),
                    HIfPrev3 => {
                        if vars.last().is_some_and(|(_, v)| *v == Value::Int(3)) {
                            Value::Handle(handles.as_mut().unwrap_or(&mut data).h()?)
                        } else {
                            Value::Absent
                        }
                    }
                    Cmc => Value::Color(read_color(&mut data, strings.as_mut(), version)?),
                })
            })()
            .map_err(|e| corrupt(format!("header variable {name}: {e}")))?;
            vars.push((name, value));
        }
        let rest_data = take_rest(&mut data, data_end)?;
        let rest_strings = match strings.as_mut() {
            Some(s) => take_rest(s, str_end)?,
            None => Bits::default(),
        };
        let rest_handles = match handles.as_mut() {
            Some(h) => take_rest(h, body.len() * 8)?,
            None => Bits::default(),
        };
        Ok(HeaderVars { version, vars, rest: [rest_data, rest_strings, rest_handles], strings: has_strings })
    }

    /// Encodes the variables as the header variables section's data.
    pub fn encode(&self) -> Vec<u8> {
        let v = self.version;
        let unicode = v.unicode();
        let mut data = BitWriter::new();
        let mut strings = BitWriter::new();
        let mut handles = BitWriter::new();
        if unicode {
            // The end bit, filled in below.
            data.rl(0);
        }
        for (name, value) in &self.vars {
            let kind = VARS.iter().find(|x| x.1 == *name).map(|x| x.2);
            match (kind, value) {
                (_, Value::Absent) => {}
                (Some(HData), Value::Handle(h)) => data.h(*h),
                (_, Value::Handle(h)) => if unicode { &mut handles } else { &mut data }.h(*h),
                (_, Value::Text(t)) => if unicode { &mut strings } else { &mut data }.tv(t, unicode),
                (_, Value::Color(c)) => write_color(&mut data, &mut strings, c, v),
                (Some(B), Value::Bool(b)) => data.b(*b),
                (Some(Bs), Value::Int(i)) => data.bs(*i as i16),
                (Some(Bl), Value::Int(i)) => data.bl(*i as i32),
                (Some(Bll), Value::Int(i)) => data.bll(*i as u64),
                (Some(Rc), Value::Int(i)) => data.rc(*i as u8),
                (_, Value::Double(d)) => data.bd(*d),
                (_, Value::Point2(p)) => data.rd2(*p),
                (_, Value::Point3(p)) => data.bd3(*p),
                (_, Value::Bool(b)) => data.b(*b),
                (_, Value::Int(i)) => data.bl(*i as i32),
            }
        }
        put_bits(&mut data, &self.rest[0]);
        if !unicode {
            return data.into_bytes();
        }
        // Data, then the string stream, its size and the flag, then the handles.
        put_bits(&mut strings, &self.rest[1]);
        let mut all = data;
        if self.strings {
            crate::bits::put_string_stream(&mut all, strings.as_bytes(), strings.bit_len());
        } else {
            all.b(false);
        }
        let end_bit = all.bit_len() as u32;
        put_bits(&mut all, &Bits { len: handles.bit_len(), bytes: handles.as_bytes().to_vec() });
        put_bits(&mut all, &self.rest[2]);
        let mut bytes = all.into_bytes();
        if let Some(slot) = bytes.get_mut(..4) {
            slot.copy_from_slice(&end_bit.to_le_bytes());
        }
        bytes
    }
}

fn read_color(data: &mut BitReader, strings: Option<&mut BitReader>, v: Version) -> Result<Color> {
    let index = data.bs()?;
    if v < Version::R2004 {
        return Ok(Color { index, rgb: 0, flags: 0, name: Text::Empty, book: Text::Empty });
    }
    let rgb = data.bl()? as u32;
    let flags = data.rc()?;
    let unicode = v.unicode();
    let mut name = Text::Empty;
    let mut book = Text::Empty;
    match strings {
        Some(s) => {
            if flags & 1 != 0 {
                name = s.tv(unicode)?;
            }
            if flags & 2 != 0 {
                book = s.tv(unicode)?;
            }
        }
        None => {
            if flags & 1 != 0 {
                name = data.tv(unicode)?;
            }
            if flags & 2 != 0 {
                book = data.tv(unicode)?;
            }
        }
    }
    Ok(Color { index, rgb, flags, name, book })
}

fn write_color(data: &mut BitWriter, strings: &mut BitWriter, c: &Color, v: Version) {
    data.bs(c.index);
    if v < Version::R2004 {
        return;
    }
    data.bl(c.rgb as i32);
    data.rc(c.flags);
    let unicode = v.unicode();
    let out = if unicode { strings } else { data };
    if c.flags & 1 != 0 {
        out.tv(&c.name, unicode);
    }
    if c.flags & 2 != 0 {
        out.tv(&c.book, unicode);
    }
}
