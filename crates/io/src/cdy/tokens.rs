//! The frozen v1 encoding of model data: a value tree (null, booleans, integers, floats, text,
//! lists and keyed records, the shape every model type serialises to) written as a flat list of
//! tokens with postcard, after a table of the record keys it uses (each key stored once, tokens
//! name it by number). Keys make it self-describing, so a model type that gains, loses or
//! defaults a field still reads; the tree is rebuilt without recursion, under a depth cap.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

/// Deepest nesting read back (model data nests a dozen levels; a hostile file can't overflow the
/// stack).
pub const MAX_DEPTH: usize = 256;
/// Most tokens one chunk holds.
pub const MAX_TOKENS: usize = 50_000_000;
/// Most distinct keys one chunk names.
pub const MAX_KEYS: usize = 1 << 20;

/// One token of the pre-order walk of a value tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Tok {
    Null,
    Bool(bool),
    U(u64),
    I(i64),
    F(f64),
    S(String),
    /// A list of this many values follows.
    List(u32),
    /// A record of this many (key, value) pairs follows: each a `Key` token, then the value.
    Record(u32),
    /// The key at this place in the key table.
    Key(u32),
}

/// [`Tok`] as written: the same variants in the same order (so the same bytes), borrowing text.
#[derive(Serialize)]
enum TokRef<'a> {
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

/// A chunk's encoded value: the key table, then the tokens.
#[derive(Deserialize)]
struct Packed {
    keys: Vec<String>,
    toks: Vec<Tok>,
}

#[derive(Serialize)]
struct PackedRef<'a> {
    keys: Vec<&'a str>,
    toks: Vec<TokRef<'a>>,
}

/// The value encoded by `toks` (their keys in `keys`), or `None` when they don't form one value
/// (damaged data).
pub fn from_tokens(keys: &[String], toks: Vec<Tok>) -> Option<Value> {
    enum Frame {
        List { want: usize, items: Vec<Value> },
        Record { want: usize, map: Map<String, Value>, key: Option<String> },
    }
    let total = toks.len();
    let mut stack: Vec<Frame> = Vec::new();
    let mut done: Option<Value> = None;
    let mut it = toks.into_iter();
    loop {
        // The next complete value: a scalar, or an empty list/record.
        let mut value = match it.next() {
            Some(Tok::Null) => Value::Null,
            Some(Tok::Bool(b)) => Value::Bool(b),
            Some(Tok::U(u)) => Value::Number(u.into()),
            Some(Tok::I(i)) => Value::Number(i.into()),
            Some(Tok::F(f)) => Number::from_f64(f).map_or(Value::Null, Value::Number),
            Some(Tok::S(s)) => Value::String(s),
            Some(Tok::List(n)) => {
                let n = n as usize;
                if n > total || stack.len() >= MAX_DEPTH {
                    return None;
                }
                if n > 0 {
                    stack.push(Frame::List { want: n, items: Vec::with_capacity(n.min(4096)) });
                    continue;
                }
                Value::Array(Vec::new())
            }
            Some(Tok::Record(n)) => {
                let n = n as usize;
                if n > total || stack.len() >= MAX_DEPTH {
                    return None;
                }
                if n > 0 {
                    stack.push(Frame::Record { want: n, map: Map::new(), key: None });
                    continue;
                }
                Value::Object(Map::new())
            }
            Some(Tok::Key(k)) => {
                let k = keys.get(k as usize)?.clone();
                match stack.last_mut() {
                    Some(Frame::Record { key, .. }) if key.is_none() => *key = Some(k),
                    _ => return None,
                }
                continue;
            }
            None => return done.filter(|_| stack.is_empty()),
        };
        // Hand the value to the frame it belongs to, closing frames that fill up.
        loop {
            match stack.last_mut() {
                None => {
                    if done.is_some() {
                        return None;
                    }
                    done = Some(value);
                    break;
                }
                Some(Frame::List { want, items }) => {
                    items.push(value);
                    if items.len() < *want {
                        break;
                    }
                }
                Some(Frame::Record { want, map, key }) => {
                    let k = key.take()?;
                    map.insert(k, value);
                    if map.len() < *want {
                        break;
                    }
                }
            }
            value = match stack.pop() {
                Some(Frame::List { items, .. }) => Value::Array(items),
                Some(Frame::Record { map, .. }) => Value::Object(map),
                None => return None,
            };
        }
    }
}

/// The postcard bytes of a value.
pub fn encode(v: &Value) -> Vec<u8> {
    let mut packed = PackedRef { keys: Vec::new(), toks: Vec::new() };
    keyed_tokens(v, &mut packed);
    postcard::to_allocvec(&packed).unwrap_or_default()
}

/// The value in postcard bytes, or `None` when they are damaged.
pub fn decode(bytes: &[u8]) -> Option<Value> {
    let p: Packed = postcard::from_bytes(bytes).ok()?;
    if p.toks.len() > MAX_TOKENS || p.keys.len() > MAX_KEYS {
        return None;
    }
    from_tokens(&p.keys, p.toks)
}

/// The whole items at the start of a damaged chunk's list (`bytes` read up to where they stop
/// making sense): the key table must read whole; then tokens are read one by one until one doesn't,
/// and each top-level item whose tokens are all there and name known keys is kept. `None` when
/// not even the key table or the list's start reads. Objects past the damage are lost.
pub fn salvage_list(bytes: &[u8]) -> Option<Vec<Value>> {
    let (keys, rest): (Vec<String>, &[u8]) = postcard::take_from_bytes(bytes).ok()?;
    if keys.len() > MAX_KEYS {
        return None;
    }
    let (count, mut rest): (u64, &[u8]) = postcard::take_from_bytes(rest).ok()?;
    let count = usize::try_from(count).ok()?.min(MAX_TOKENS);
    let mut toks = Vec::new();
    while toks.len() < count {
        let Ok((t, r)) = postcard::take_from_bytes::<Tok>(rest) else { break };
        if matches!(t, Tok::Key(k) if k as usize >= keys.len()) {
            break;
        }
        toks.push(t);
        rest = r;
    }
    let Some(Tok::List(_)) = toks.first() else { return None };
    // Each top-level item: from its first token to where its tree is complete.
    let mut items = Vec::new();
    let mut at = 1;
    'items: while at < toks.len() {
        let mut need: u64 = 1;
        let mut end = at;
        while need > 0 {
            let Some(t) = toks.get(end) else { break 'items };
            need -= 1;
            match t {
                Tok::List(n) => need = need.saturating_add(u64::from(*n)),
                Tok::Record(n) => need = need.saturating_add(2 * u64::from(*n)),
                _ => {}
            }
            if need > (toks.len() - end) as u64 {
                break 'items;
            }
            end += 1;
        }
        let item = toks.get(at..end).map(<[Tok]>::to_vec).unwrap_or_default();
        match from_tokens(&keys, item) {
            Some(v) => items.push(v),
            None => break,
        }
        at = end;
    }
    // The last item read may run into the damage (garbage can still form tokens): left out too,
    // so what is kept is exact.
    items.pop();
    Some(items)
}

/// The tokens of `v`: a pre-order walk, each record's entries in key order (equal values give
/// equal bytes) with the key before its value; keys numbered in the order first met. A stack of
/// work items instead of recursion.
fn keyed_tokens<'a>(v: &'a Value, out: &mut PackedRef<'a>) {
    enum Work<'a> {
        Value(&'a Value),
        Key(&'a str),
    }
    let mut ids: HashMap<&'a str, u32> = HashMap::new();
    let mut stack = vec![Work::Value(v)];
    while let Some(w) = stack.pop() {
        let v = match w {
            Work::Key(k) => {
                let next = u32::try_from(out.keys.len()).unwrap_or(u32::MAX);
                let id = *ids.entry(k).or_insert_with(|| {
                    out.keys.push(k);
                    next
                });
                out.toks.push(TokRef::Key(id));
                continue;
            }
            Work::Value(v) => v,
        };
        out.toks.push(match v {
            Value::Null => TokRef::Null,
            Value::Bool(b) => TokRef::Bool(*b),
            Value::Number(n) => {
                if let Some(u) = n.as_u64() {
                    TokRef::U(u)
                } else if let Some(i) = n.as_i64() {
                    TokRef::I(i)
                } else {
                    TokRef::F(n.as_f64().unwrap_or(0.0))
                }
            }
            Value::String(s) => TokRef::S(s),
            Value::Array(a) => {
                stack.extend(a.iter().rev().map(Work::Value));
                TokRef::List(u32::try_from(a.len()).unwrap_or(u32::MAX))
            }
            Value::Object(m) => {
                let mut entries: Vec<(&String, &Value)> = m.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                for (k, v) in entries.into_iter().rev() {
                    stack.push(Work::Value(v));
                    stack.push(Work::Key(k));
                }
                TokRef::Record(u32::try_from(m.len()).unwrap_or(u32::MAX))
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_roundtrip_exactly() {
        for v in [
            json!(null),
            json!({"b": [1, -2, 3.25, "x", {"z": null, "a": true}], "a": {}, "c": [], "d": u64::MAX, "e": i64::MIN}),
            json!([[[[[1]]]]]),
            json!(0.1 + 0.2),
        ] {
            assert_eq!(decode(&encode(&v)).as_ref(), Some(&v), "{v}");
        }
        // Floats keep every bit.
        let f = 1.0f64 / 3.0;
        assert_eq!(decode(&encode(&json!(f))).and_then(|v| v.as_f64()).map(f64::to_bits), Some(f.to_bits()));
    }

    #[test]
    fn equal_values_give_equal_bytes() {
        let a = json!({"x": 1, "y": [2, 3], "a": "q"});
        let b = json!({"a": "q", "y": [2, 3], "x": 1});
        assert_eq!(encode(&a), encode(&b));
    }

    #[test]
    fn damaged_and_hostile_tokens_are_refused() {
        let k = ["k".to_string()];
        assert!(from_tokens(&k, vec![Tok::List(3), Tok::U(1)]).is_none(), "cut short");
        assert!(from_tokens(&k, vec![Tok::U(1), Tok::U(2)]).is_none(), "two values");
        assert!(from_tokens(&k, vec![Tok::Record(1), Tok::U(1)]).is_none(), "value without its key");
        assert!(from_tokens(&k, vec![Tok::Key(0)]).is_none());
        assert!(from_tokens(&k, vec![Tok::Record(1), Tok::Key(9), Tok::U(1)]).is_none(), "a key not in the table");
        assert!(from_tokens(&k, vec![Tok::Record(1), Tok::Key(0), Tok::U(1)]).is_some());
        assert!(from_tokens(&k, vec![Tok::List(u32::MAX)]).is_none(), "a count beyond the tokens");
        let deep: Vec<Tok> = std::iter::repeat_n(Tok::List(1), MAX_DEPTH + 5).chain([Tok::Null]).collect();
        assert!(from_tokens(&k, deep).is_none(), "too deep");
        assert!(decode(&[0xFF; 32]).is_none());
        assert!(decode(&[]).is_none());
        // Deep but allowed: rebuilt without recursion.
        let ok: Vec<Tok> = std::iter::repeat_n(Tok::List(1), MAX_DEPTH - 1).chain([Tok::Null]).collect();
        assert!(from_tokens(&k, ok).is_some());
    }
}
