//! A chunk's tokens ([`super::tokens`]) read straight into model types: a serde deserializer over
//! the token stream, so a model space chunk is built object by object as its bytes are read, with
//! no token list or value tree of the chunk in between (Q6399: they held about twelve times the
//! chunk's inflated bytes per core, and building them was most of opening's time).
//!
//! The bytes are the same v1 encoding, and every check [`super::tokens::decode`] makes still holds:
//! the key table and token caps, no count larger than the tokens the chunk declares, the nesting
//! depth cap, keys only where a record's key goes and only from the table, and exactly one value
//! using every token declared. Bytes that fail one of them make the chunk damaged, as before. An
//! object that is whole but doesn't read as the model type is reported and stepped over by its
//! token structure, and the objects after it still read.
//!
//! The values reach the model types exactly as they did through a `serde_json::Value`: a
//! non-negative integer as unsigned, a non-finite float as null, a record key read as a number
//! where the type wants one, an enum as its name or as a record of one entry.

use std::fmt;

use serde::Deserialize;
use serde::de::value::BorrowedStrDeserializer;
use serde::de::{self, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, Unexpected, VariantAccess, Visitor};

use super::tokens::{MAX_DEPTH, MAX_KEYS, MAX_TOKENS};

/// A token as read, its text borrowed from the chunk's bytes: the same variants in the same order
/// as [`super::tokens::Tok`], so the same bytes.
#[derive(Clone, Copy, Debug, Deserialize)]
enum Tok<'a> {
    Null,
    Bool(bool),
    U(u64),
    I(i64),
    F(f64),
    S(&'a str),
    List(u32),
    Record(u32),
    Key(u32),
}

impl Tok<'_> {
    fn unexpected(&self) -> Unexpected<'_> {
        match self {
            Tok::Null => Unexpected::Unit,
            Tok::Bool(b) => Unexpected::Bool(*b),
            Tok::U(u) => Unexpected::Unsigned(*u),
            Tok::I(i) => Unexpected::Signed(*i),
            Tok::F(f) => Unexpected::Float(*f),
            Tok::S(s) => Unexpected::Str(s),
            Tok::List(_) => Unexpected::Seq,
            Tok::Record(_) => Unexpected::Map,
            Tok::Key(_) => Unexpected::Other("a record key"),
        }
    }
}

/// Why a value didn't read.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl de::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Error(msg.to_string())
    }
}

/// The token stream of one chunk.
struct Stream<'de> {
    keys: Vec<&'de str>,
    /// The bytes not read yet.
    rest: &'de [u8],
    /// Tokens the chunk declares that are not read yet.
    left: usize,
    /// All the tokens the chunk declares: no list or record may count more.
    total: usize,
    /// Lists and records open.
    depth: usize,
    /// The next token, read ahead, and the bytes after it.
    peeked: Option<(Tok<'de>, &'de [u8])>,
    /// The tokens stopped making sense: the chunk is damaged.
    damaged: bool,
    /// An error was handed out since the current object began.
    failed: bool,
}

impl<'de> Stream<'de> {
    fn damage(&mut self, what: &str) -> Error {
        self.damaged = true;
        self.failed = true;
        Error(format!("damaged data: {what}"))
    }

    fn peek(&mut self) -> Result<Tok<'de>, Error> {
        if let Some((t, _)) = self.peeked {
            return Ok(t);
        }
        if self.left == 0 {
            return Err(self.damage("more values than the part holds"));
        }
        match postcard::take_from_bytes::<Tok<'de>>(self.rest) {
            Ok((t, rest)) => {
                self.peeked = Some((t, rest));
                Ok(t)
            }
            Err(_) => Err(self.damage("a token doesn't read")),
        }
    }

    fn next(&mut self) -> Result<Tok<'de>, Error> {
        let t = self.peek()?;
        if let Some((_, rest)) = self.peeked.take() {
            self.rest = rest;
        }
        self.left = self.left.saturating_sub(1);
        Ok(t)
    }

    /// The key at `k` in the table.
    fn key(&mut self, k: u32) -> Result<&'de str, Error> {
        match usize::try_from(k).ok().and_then(|k| self.keys.get(k)) {
            Some(s) => Ok(*s),
            None => Err(self.damage("a key not in the table")),
        }
    }

    /// A list or record of `n` entries begins.
    fn open(&mut self, n: u32) -> Result<usize, Error> {
        let n = usize::try_from(n).unwrap_or(usize::MAX);
        if n > self.total {
            return Err(self.damage("a count larger than the part"));
        }
        if n > 0 {
            if self.depth >= MAX_DEPTH {
                return Err(self.damage("nested too deep"));
            }
            self.depth += 1;
        }
        Ok(n)
    }

    /// The list or record of `n` entries [`Self::open`] began has ended.
    fn close(&mut self, n: usize) {
        if n > 0 {
            self.depth = self.depth.saturating_sub(1);
        }
    }

    /// Step over one value, checking its structure as it goes. Without recursion.
    fn skip(&mut self) -> Result<(), Error> {
        // The lists and records open: (a record, slots still to come); a record's entry is two
        // slots, its key then its value.
        let mut frames: Vec<(bool, usize)> = Vec::new();
        loop {
            let key_slot = matches!(frames.last(), Some((true, left)) if left % 2 == 0);
            match (key_slot, self.next()?) {
                (true, Tok::Key(k)) => {
                    self.key(k)?;
                }
                (true, _) | (false, Tok::Key(_)) => return Err(self.damage("a key out of place")),
                (false, Tok::List(n)) => {
                    let n = self.open(n)?;
                    if n > 0 {
                        frames.push((false, n));
                        continue;
                    }
                }
                (false, Tok::Record(n)) => {
                    let n = self.open(n)?;
                    if n > 0 {
                        frames.push((true, n.saturating_mul(2)));
                        continue;
                    }
                }
                _ => {}
            }
            // A value or key is whole: it fills a slot, closing each frame it completes.
            loop {
                let Some((_, left)) = frames.last_mut() else { return Ok(()) };
                *left = left.saturating_sub(1);
                if *left > 0 {
                    break;
                }
                frames.pop();
                self.close(1);
            }
        }
    }

    fn any<V: Visitor<'de>>(&mut self, v: V) -> Result<V::Value, Error> {
        match self.next()? {
            Tok::Null => v.visit_unit(),
            Tok::Bool(b) => v.visit_bool(b),
            Tok::U(u) => v.visit_u64(u),
            // As a `serde_json::Value` number: a non-negative one is unsigned.
            Tok::I(i) => match u64::try_from(i) {
                Ok(u) => v.visit_u64(u),
                Err(_) => v.visit_i64(i),
            },
            Tok::F(f) if f.is_finite() => v.visit_f64(f),
            Tok::F(_) => v.visit_unit(),
            Tok::S(s) => v.visit_borrowed_str(s),
            Tok::List(n) => {
                let n = self.open(n)?;
                let mut seq = Seq { de: &mut *self, left: n };
                let r = v.visit_seq(&mut seq)?;
                if seq.left > 0 {
                    return Err(de::Error::invalid_length(n, &"fewer elements in the list"));
                }
                self.close(n);
                Ok(r)
            }
            Tok::Record(n) => {
                let n = self.open(n)?;
                let mut map = Map { de: &mut *self, left: n };
                let r = v.visit_map(&mut map)?;
                if map.left > 0 {
                    return Err(de::Error::invalid_length(n, &"fewer entries in the record"));
                }
                self.close(n);
                Ok(r)
            }
            Tok::Key(_) => Err(self.damage("a key out of place")),
        }
    }

    fn option<V: Visitor<'de>>(&mut self, v: V) -> Result<V::Value, Error> {
        match self.peek()? {
            Tok::Null => {
                self.next()?;
                v.visit_none()
            }
            Tok::F(f) if !f.is_finite() => {
                self.next()?;
                v.visit_none()
            }
            _ => v.visit_some(self),
        }
    }

    fn enumeration<V: Visitor<'de>>(&mut self, v: V) -> Result<V::Value, Error> {
        match self.next()? {
            Tok::S(s) => v.visit_enum(BorrowedStrDeserializer::<Error>::new(s)),
            Tok::Record(1) => {
                let n = self.open(1)?;
                let variant = match self.next()? {
                    Tok::Key(k) => self.key(k)?,
                    _ => return Err(self.damage("a key out of place")),
                };
                let r = v.visit_enum(Enum { de: &mut *self, variant })?;
                self.close(n);
                Ok(r)
            }
            Tok::Record(_) => Err(de::Error::invalid_value(Unexpected::Map, &"a record with one entry")),
            Tok::Key(_) => Err(self.damage("a key out of place")),
            t => Err(de::Error::invalid_type(t.unexpected(), &"a name or a record with one entry")),
        }
    }

    /// Note an error handed out.
    fn track<T>(&mut self, r: Result<T, Error>) -> Result<T, Error> {
        if r.is_err() {
            self.failed = true;
        }
        r
    }
}

impl<'de> Deserializer<'de> for &mut Stream<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
        let r = self.any(v);
        self.track(r)
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
        let r = self.option(v);
        self.track(r)
    }

    fn deserialize_enum<V: Visitor<'de>>(self, _name: &'static str, _variants: &'static [&'static str], v: V) -> Result<V::Value, Error> {
        let r = self.enumeration(v);
        self.track(r)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, v: V) -> Result<V::Value, Error> {
        let r = v.visit_newtype_struct(&mut *self);
        self.track(r)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
        let r = self.skip().and_then(|()| v.visit_unit());
        self.track(r)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier
    }
}

/// A list's items.
struct Seq<'a, 'de> {
    de: &'a mut Stream<'de>,
    left: usize,
}

impl<'de> SeqAccess<'de> for Seq<'_, 'de> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Error> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        seed.deserialize(&mut *self.de).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.left)
    }
}

/// A record's entries.
struct Map<'a, 'de> {
    de: &'a mut Stream<'de>,
    left: usize,
}

impl<'de> MapAccess<'de> for Map<'_, 'de> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Error> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        let key = match self.de.next()? {
            Tok::Key(k) => self.de.key(k)?,
            _ => return Err(self.de.damage("a key out of place")),
        };
        let r = seed.deserialize(Key(key)).map(Some);
        self.de.track(r)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        seed.deserialize(&mut *self.de)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.left)
    }
}

/// An enum written as a record of one entry: the variant's name, then its value.
struct Enum<'a, 'de> {
    de: &'a mut Stream<'de>,
    variant: &'de str,
}

impl<'de> EnumAccess<'de> for Enum<'_, 'de> {
    type Error = Error;
    type Variant = Self;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self), Error> {
        let v = seed.deserialize(BorrowedStrDeserializer::<Error>::new(self.variant))?;
        Ok((v, self))
    }
}

impl<'de> VariantAccess<'de> for Enum<'_, 'de> {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Error> {
        <()>::deserialize(&mut *self.de)
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        seed.deserialize(&mut *self.de)
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, v: V) -> Result<V::Value, Error> {
        self.de.deserialize_seq(v)
    }

    fn struct_variant<V: Visitor<'de>>(self, _fields: &'static [&'static str], v: V) -> Result<V::Value, Error> {
        self.de.deserialize_map(v)
    }
}

/// A record key, read as `serde_json` reads an object's key: text, or a number or boolean parsed
/// from it where the type wants one (a map keyed by numbers).
struct Key<'de>(&'de str);

impl Key<'_> {
    fn parsed<T: std::str::FromStr>(&self, what: &dyn de::Expected) -> Result<T, Error> {
        self.0.parse().map_err(|_| de::Error::invalid_value(Unexpected::Str(self.0), what))
    }
}

macro_rules! parsed_key {
    ($($f:ident $t:ty => $visit:ident),* $(,)?) => {
        $(fn $f<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
            let n: $t = self.parsed(&v)?;
            v.$visit(n)
        })*
    };
}

impl<'de> Deserializer<'de> for Key<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
        v.visit_borrowed_str(self.0)
    }

    parsed_key! {
        deserialize_bool bool => visit_bool,
        deserialize_i8 i8 => visit_i8,
        deserialize_i16 i16 => visit_i16,
        deserialize_i32 i32 => visit_i32,
        deserialize_i64 i64 => visit_i64,
        deserialize_i128 i128 => visit_i128,
        deserialize_u8 u8 => visit_u8,
        deserialize_u16 u16 => visit_u16,
        deserialize_u32 u32 => visit_u32,
        deserialize_u64 u64 => visit_u64,
        deserialize_u128 u128 => visit_u128,
        deserialize_f32 f32 => visit_f32,
        deserialize_f64 f64 => visit_f64,
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
        v.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, v: V) -> Result<V::Value, Error> {
        v.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(self, _name: &'static str, _variants: &'static [&'static str], v: V) -> Result<V::Value, Error> {
        v.visit_enum(BorrowedStrDeserializer::<Error>::new(self.0))
    }

    serde::forward_to_deserialize_any! {
        char str string bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

/// Read a chunk whose value is a list, item by item: `each(index, item)` gets each item built as
/// `T`, or why it couldn't be (its tokens are stepped over and the next item read). `None` when
/// the bytes don't form one list, checked as [`super::tokens::decode`] checks them (damaged): what
/// `each` was handed before then must not be kept.
pub fn each_in_list<'de, T: Deserialize<'de>>(bytes: &'de [u8], mut each: impl FnMut(usize, Result<T, String>)) -> Option<()> {
    let (keys, rest): (Vec<&'de str>, &'de [u8]) = postcard::take_from_bytes(bytes).ok()?;
    if keys.len() > MAX_KEYS {
        return None;
    }
    let (count, rest): (u64, &'de [u8]) = postcard::take_from_bytes(rest).ok()?;
    let total = usize::try_from(count).ok().filter(|n| *n <= MAX_TOKENS)?;
    let mut s = Stream { keys, rest, left: total, total, depth: 0, peeked: None, damaged: false, failed: false };
    let n = match s.next().ok()? {
        Tok::List(n) => s.open(n).ok()?,
        _ => return None,
    };
    let depth = s.depth;
    for i in 0..n {
        // Nothing is read ahead between items (only an option peeks, and it then reads what it
        // peeked), so the position here is where the item starts.
        let mark = (s.rest, s.left);
        s.peeked = None;
        s.failed = false;
        let r = T::deserialize(&mut s);
        if s.damaged {
            return None;
        }
        match r {
            Ok(t) if !s.failed => each(i, Ok(t)),
            r => {
                // A type that caught an error and went on may have left the stream anywhere:
                // its object is not kept either.
                let why = r.err().map_or_else(|| "its data didn't read as one object".to_string(), |e| e.to_string());
                (s.rest, s.left) = mark;
                s.peeked = None;
                s.depth = depth;
                if s.skip().is_err() {
                    return None;
                }
                each(i, Err(why));
            }
        }
    }
    s.close(n);
    // Exactly one value: tokens declared past it mean the part is damaged.
    (s.left == 0 && s.peeked.is_none()).then_some(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::super::tokens::{self, Tok as T};
    use super::*;

    /// The items of a list encoded in v1 bytes, read by the stream.
    fn streamed<X: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<Vec<Result<X, String>>> {
        let mut out = Vec::new();
        each_in_list::<X>(bytes, |i, r| {
            assert_eq!(i, out.len());
            out.push(r);
        })?;
        Some(out)
    }

    /// The items as the value-tree reader read them.
    fn via_value<X: serde::de::DeserializeOwned>(bytes: &[u8]) -> Option<Vec<Result<X, String>>> {
        let Value::Array(list) = tokens::decode(bytes)? else { return None };
        Some(list.into_iter().map(|v| serde_json::from_value::<X>(v).map_err(|e| e.to_string())).collect())
    }

    /// Postcard bytes of a hand-made token list (to make damaged and hostile ones).
    fn raw(keys: &[&str], toks: &[T]) -> Vec<u8> {
        #[derive(serde::Serialize)]
        struct P<'a> {
            keys: &'a [&'a str],
            toks: &'a [T],
        }
        postcard::to_allocvec(&P { keys, toks }).unwrap()
    }

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    enum Shape {
        Dot {
            at: (f64, f64),
        },
        Named {
            name: String,
            #[serde(default)]
            tags: Vec<String>,
        },
    }

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "camelCase")]
    enum Mode {
        Off,
        Scale(f64),
        Pair(u8, i32),
        Box { w: f32, h: f32 },
    }

    #[derive(Debug, PartialEq, Deserialize)]
    struct Handle(u64);

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Thing {
        handle: Handle,
        #[serde(default)]
        label: Option<String>,
        shape: Shape,
        modes: Vec<Mode>,
        by_id: BTreeMap<u32, i64>,
        flags: BTreeMap<String, bool>,
        any: Value,
        signed: i64,
        small: u8,
        ch: char,
        maybe: Option<f64>,
        nothing: (),
    }

    fn thing(i: u64) -> Value {
        json!({
            "handle": i,
            "label": if i.is_multiple_of(2) { json!(format!("T{i}")) } else { Value::Null },
            "shape": if i.is_multiple_of(3) { json!({"type": "dot", "at": [1.5, -2.0]}) } else { json!({"type": "named", "name": "n", "tags": ["a", "b"]}) },
            "modes": ["off", {"scale": 0.25}, {"pair": [7, -3]}, {"box": {"w": 1.0, "h": 2.5}}],
            "byId": {"3": -1, "12": 5},
            "flags": {"x": true, "y": false},
            "any": {"z": [1, -2, 3.5, "s", null, {"q": {}}], "e": []},
            "signed": 42,
            "small": 200,
            "ch": "c",
            "maybe": null,
            "nothing": null,
            "unknownField": {"deep": [[1], [2, {"k": "v"}]]},
        })
    }

    #[test]
    fn objects_read_as_through_a_value_tree() {
        let list: Vec<Value> = (0..12).map(thing).collect();
        let bytes = tokens::encode(&Value::Array(list));
        let a = streamed::<Thing>(&bytes).unwrap();
        let b = via_value::<Thing>(&bytes).unwrap();
        assert_eq!(a.len(), 12);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.as_ref().ok(), y.as_ref().ok());
            assert!(x.is_ok(), "{x:?}");
        }
        // And the dynamic value is the same tree.
        let first = a.into_iter().next().unwrap().unwrap();
        assert_eq!(first.any, json!({"z": [1, -2, 3.5, "s", null, {"q": {}}], "e": []}));
        assert_eq!(first.by_id.get(&12), Some(&5));
    }

    #[test]
    fn an_object_that_does_not_read_is_reported_and_the_next_still_reads() {
        let mut bad = thing(1);
        bad["small"] = json!(300); // out of range for u8
        let mut bad2 = thing(2);
        bad2["modes"] = json!([{"pair": [1, 2, 3]}]); // a tuple too long
        let mut bad3 = thing(3);
        bad3["shape"] = json!({"type": "nope"});
        let list = vec![thing(0), bad, thing(4), bad2, bad3, thing(6)];
        let bytes = tokens::encode(&Value::Array(list));
        let a = streamed::<Thing>(&bytes).unwrap();
        let b = via_value::<Thing>(&bytes).unwrap();
        let oks = |v: &[Result<Thing, String>]| v.iter().map(Result::is_ok).collect::<Vec<_>>();
        assert_eq!(oks(&a), vec![true, false, true, false, false, true]);
        assert_eq!(oks(&a), oks(&b));
        assert_eq!(a.get(5).and_then(|r| r.as_ref().ok()).map(|t| t.handle.0), Some(6));
        let why = a.get(1).and_then(|r| r.as_ref().err()).unwrap();
        assert!(why.contains("300"), "{why}");
    }

    #[test]
    fn non_finite_and_negative_numbers_read_as_a_value_tree_reads_them() {
        #[derive(Debug, PartialEq, Deserialize)]
        struct N {
            a: Option<f64>,
            b: u32,
            c: Value,
        }
        // An I token holding a non-negative number reads as unsigned; a NaN float as null.
        let bytes = raw(&["a", "b", "c"], &[T::List(1), T::Record(3), T::Key(0), T::F(f64::NAN), T::Key(1), T::I(7), T::Key(2), T::I(-4)]);
        let a = streamed::<N>(&bytes).unwrap();
        let b = via_value::<N>(&bytes).unwrap();
        assert_eq!(a.first().and_then(|r| r.as_ref().ok()), Some(&N { a: None, b: 7, c: json!(-4) }));
        assert_eq!(a.first().and_then(|r| r.as_ref().ok()), b.first().and_then(|r| r.as_ref().ok()));
        // A NaN where a number must be is an object that doesn't read, in both.
        let bytes = raw(&["a", "b", "c"], &[T::List(1), T::Record(3), T::Key(0), T::Null, T::Key(1), T::F(f64::INFINITY), T::Key(2), T::Null]);
        assert!(streamed::<N>(&bytes).unwrap().first().unwrap().is_err());
        assert!(via_value::<N>(&bytes).unwrap().first().unwrap().is_err());
    }

    #[test]
    fn damaged_and_hostile_parts_are_refused_as_before() {
        let k = &["k"];
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("not a list", raw(k, &[T::U(1)])),
            ("cut short", raw(k, &[T::List(3), T::U(1)])),
            ("two values", raw(k, &[T::List(1), T::U(1), T::U(2)])),
            ("value without its key", raw(k, &[T::List(1), T::Record(1), T::U(1)])),
            ("a key not in the table", raw(k, &[T::List(1), T::Record(1), T::Key(9), T::U(1)])),
            ("a key as a value", raw(k, &[T::List(1), T::Key(0)])),
            ("a count beyond the tokens", raw(k, &[T::List(1), T::List(u32::MAX)])),
            ("a top count beyond the tokens", raw(k, &[T::List(u32::MAX)])),
            ("garbage", vec![0xFF; 32]),
            ("nothing", Vec::new()),
        ];
        for (what, bytes) in cases {
            assert!(streamed::<Value>(&bytes).is_none(), "{what}");
            assert!(via_value::<Value>(&bytes).is_none(), "{what} (value tree)");
        }
        // A damaged item stepped over (the type refuses it, its structure is broken) is damage
        // too: an item whose tokens are cut short ends the part.
        assert!(streamed::<u8>(&raw(k, &[T::List(2), T::U(1), T::Record(1), T::Key(0)])).is_none());
        // Too deep, also inside an object the type steps over.
        let deep: Vec<T> = [T::List(1)].into_iter().chain(std::iter::repeat_n(T::List(1), MAX_DEPTH + 5)).chain([T::Null]).collect();
        assert!(streamed::<Value>(&raw(k, &deep)).is_none());
        assert!(streamed::<u8>(&raw(k, &deep)).is_none());
        // Deep but allowed.
        let ok: Vec<T> = std::iter::repeat_n(T::List(1), MAX_DEPTH - 1).chain([T::Null]).collect();
        let ok = raw(k, &ok);
        assert_eq!(streamed::<Value>(&ok).map(|v| v.len()), via_value::<Value>(&ok).map(|v| v.len()));
        assert!(streamed::<Value>(&ok).is_some());
        // Every cut of a real chunk either reads or is refused; it never panics.
        let bytes = tokens::encode(&Value::Array((0..3).map(thing).collect()));
        for cut in 0..bytes.len() {
            let _ = streamed::<Thing>(&bytes[..cut]);
        }
    }
}
