//! More Visual LISP utilities: string positions, sorting by index, `vl-list*`, and read-only file
//! information (`vl-file-size`, `vl-file-directory-p`, `vl-file-systime`, `vl-directory-files`).
//! The other `vl-` functions are in `builtins.rs`.

use super::LispError;
use super::builtins::{arity, callable, int, list_arg, match_pattern, merge_sort, split_patterns, string};
use super::eval::{Ctx, Flow};
use super::value::Value;

type R = Result<Value, LispError>;

/// Most names `vl-directory-files` returns.
const MAX_DIRECTORY_FILES: usize = 100_000;

/// A position argument: a non-negative integer.
fn position(v: &Value) -> Result<usize, LispError> {
    let i = int(v)?;
    usize::try_from(i).map_err(|_| LispError::bad_arg("fixnump", v))
}

/// `(vl-string-position char-code str [start-pos [from-end-p]])`: where the character is in the
/// string, searching from `start-pos`, or backward from the end down to `start-pos` with
/// `from-end-p`; nil when it isn't there.
pub fn string_position(a: &[Value]) -> R {
    arity(a, 2, 4)?;
    let code = int(&a[0])?;
    let s = string(&a[1])?;
    let start = match a.get(2) {
        None | Some(Value::Nil) => 0,
        Some(v) => position(v)?,
    };
    let from_end = a.get(3).is_some_and(|v| !v.is_nil());
    let Some(c) = u32::try_from(code).ok().and_then(char::from_u32) else { return Ok(Value::Nil) };
    let chars: Vec<char> = s.chars().collect();
    let found = if from_end {
        (start..chars.len()).rev().find(|i| chars.get(*i) == Some(&c))
    } else {
        (start..chars.len()).find(|i| chars.get(*i) == Some(&c))
    };
    Ok(found.and_then(|i| i32::try_from(i).ok()).map(Value::Int).unwrap_or_default())
}

/// `(vl-string-mismatch str1 str2 [pos1 pos2 ignore-case-p])`: how many characters, from `pos1`
/// in the first string and `pos2` in the second, are the same.
pub fn string_mismatch(a: &[Value]) -> R {
    arity(a, 2, 5)?;
    let (s1, s2) = (string(&a[0])?, string(&a[1])?);
    let pos = |i: usize| -> Result<usize, LispError> {
        match a.get(i) {
            None | Some(Value::Nil) => Ok(0),
            Some(v) => position(v),
        }
    };
    let (p1, p2) = (pos(2)?, pos(3)?);
    let fold = a.get(4).is_some_and(|v| !v.is_nil());
    let same = |x: char, y: char| if fold { x.to_lowercase().eq(y.to_lowercase()) } else { x == y };
    let n = s1.chars().skip(p1).zip(s2.chars().skip(p2)).take_while(|(x, y)| same(*x, *y)).count();
    Ok(Value::Int(i32::try_from(n).unwrap_or(i32::MAX)))
}

/// `(vl-string-elt string position)`: the character code at `position` (the first is 0).
pub fn string_elt(a: &[Value]) -> R {
    arity(a, 2, 2)?;
    let s = string(&a[0])?;
    let i = position(&a[1])?;
    match s.chars().nth(i) {
        Some(c) => Ok(Value::Int(i32::try_from(u32::from(c)).unwrap_or(i32::MAX))),
        None => Err(LispError::new("bad argument value: string position out of range")),
    }
}

/// `(vl-list* obj …)`: a list of the objects whose last one is its tail, like `cons` for two.
pub fn list_star(a: &[Value]) -> R {
    arity(a, 1, usize::MAX)?;
    let mut items = a.to_vec();
    let tail = items.pop().unwrap_or_default();
    Ok(if items.is_empty() { tail } else { Value::list_with_tail(items, tail) })
}

/// `(vl-member-if-not predicate list)`: the list from the first element the predicate is nil for.
pub fn member_if_not(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 2, 2)?;
    let f = callable(ctx, &a[0])?;
    list_arg(&a[1])?;
    let mut v = a[1].clone();
    while let Value::Cons(c) = v.clone() {
        if ctx.call_sync(&f, vec![c.car.clone()])?.is_nil() {
            return Ok(Flow::Value(v));
        }
        v = c.cdr.clone();
    }
    Ok(Flow::Value(Value::Nil))
}

/// `(vl-sort-i list comparison)`: the indices of the list's elements in sorted order (a stable
/// sort; duplicates are kept, unlike `vl-sort` of numbers).
pub fn sort_i(ctx: &mut Ctx<'_>, a: Vec<Value>) -> Result<Flow, LispError> {
    arity(&a, 2, 2)?;
    let f = callable(ctx, &a[1])?;
    let items = list_arg(&a[0])?;
    let mut idx: Vec<Value> = (0..items.len()).filter_map(|i| i32::try_from(i).ok()).map(Value::Int).collect();
    let at = |v: &Value| match v {
        Value::Int(i) => usize::try_from(*i).ok().and_then(|i| items.get(i)).cloned().unwrap_or_default(),
        _ => Value::Nil,
    };
    let mut err = None;
    merge_sort(&mut idx, &mut |x, y| {
        if err.is_some() {
            return false;
        }
        match ctx.call_sync(&f, vec![at(x), at(y)]) {
            Ok(v) => !v.is_nil(),
            Err(e) => {
                err = Some(e);
                false
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    Ok(Flow::Value(Value::list(idx)))
}

/// `(vl-file-size filename)`: the file's size in bytes, 0 for a folder, nil when it doesn't exist.
pub fn file_size(a: &[Value]) -> R {
    arity(a, 1, 1)?;
    let p = string(&a[0])?;
    Ok(match metadata(&p) {
        Some((true, _, _)) => Value::Int(0),
        // An integer when it fits AutoLISP's 32 bits, else a real.
        Some((false, len, _)) => i32::try_from(len).map(Value::Int).unwrap_or(Value::Real(len as f64)),
        None => Value::Nil,
    })
}

/// `(vl-file-directory-p filename)`: T when it names a folder.
pub fn file_directory_p(a: &[Value]) -> R {
    arity(a, 1, 1)?;
    let p = string(&a[0])?;
    Ok(Value::bool(metadata(&p).is_some_and(|(dir, _, _)| dir)))
}

/// `(vl-file-systime filename)`: when the file was last changed, as `(year month day-of-week day
/// hours minutes seconds milliseconds)` (UTC; Sunday is 0); nil for a folder or a missing file.
pub fn file_systime(a: &[Value]) -> R {
    arity(a, 1, 1)?;
    let p = string(&a[0])?;
    let Some((false, _, Some(ms))) = metadata(&p) else { return Ok(Value::Nil) };
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    // 1970-01-01 was a Thursday (4).
    let wday = (days + 4).rem_euclid(7);
    let parts = [y, m, wday, d, rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000];
    Ok(Value::list(parts.into_iter().map(|x| Value::Int(i32::try_from(x).unwrap_or(0)))))
}

/// The calendar date of a day count since 1970-01-01 (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `(vl-directory-files [directory pattern directories])`: the names in a folder (the current one
/// by default) matching a `wcmatch` pattern (`*.*` and nil match all), sorted; `directories` -1
/// lists folders only, 1 files only, 0 or nil both. Nil when the folder can't be read.
pub fn directory_files(a: &[Value]) -> R {
    arity(a, 0, 3)?;
    let dir = match a.first() {
        None | Some(Value::Nil) => ".".to_string(),
        Some(v) => string(v)?.to_string(),
    };
    let pattern = match a.get(1) {
        None | Some(Value::Nil) => "*".to_string(),
        Some(v) => string(v)?.to_string(),
    };
    let which = match a.get(2) {
        None | Some(Value::Nil) => 0,
        Some(v) => int(v)?,
    };
    let pats = split_patterns(if pattern == "*.*" { "*" } else { &pattern });
    let Some(entries) = list_dir(&dir) else { return Ok(Value::Nil) };
    let mut names: Vec<String> = entries
        .into_iter()
        .filter(|(_, is_dir)| match which {
            -1 => *is_dir,
            1 => !*is_dir,
            _ => true,
        })
        .map(|(n, _)| n)
        .filter(|n| {
            let text: Vec<char> = n.chars().collect();
            pats.iter().any(|p| match_pattern(&text, p))
        })
        .collect();
    names.sort();
    Ok(Value::list(names.iter().map(|n| Value::str(n))))
}

/// Whether `path` is a folder, its size, and when it was changed (ms since 1970).
#[cfg(not(target_arch = "wasm32"))]
fn metadata(path: &str) -> Option<(bool, u64, Option<i64>)> {
    let m = std::fs::metadata(path).ok()?;
    let ms = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).and_then(|d| i64::try_from(d.as_millis()).ok());
    Some((m.is_dir(), m.len(), ms))
}
#[cfg(target_arch = "wasm32")]
fn metadata(_path: &str) -> Option<(bool, u64, Option<i64>)> {
    None
}

/// A folder's entries (name, is a folder), "." and ".." left out, at most
/// [`MAX_DIRECTORY_FILES`].
#[cfg(not(target_arch = "wasm32"))]
fn list_dir(dir: &str) -> Option<Vec<(String, bool)>> {
    let rd = std::fs::read_dir(dir).ok()?;
    Some(
        rd.filter_map(|e| e.ok())
            .take(MAX_DIRECTORY_FILES)
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.file_type().is_ok_and(|t| t.is_dir())))
            .collect(),
    )
}
#[cfg(target_arch = "wasm32")]
fn list_dir(_dir: &str) -> Option<Vec<(String, bool)>> {
    None
}
