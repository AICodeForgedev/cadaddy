//! One name rule for every symbol table: layers, text/dimension/multileader/table/multiline
//! styles, linetypes, blocks, groups, views, UCSs and layouts (create and rename alike).
//!
//! A name may not be empty or blank, may not hold any of `< > / \ " : ; ? * | , = \``
//! or a control character (newline, tab…), and has at most 255 characters. DXF and DWG
//! cannot keep anything else, so a name accepted here must survive a save and reopen.

use super::bad;
use crate::Result;

/// Longest symbol table name.
pub(crate) const MAX_NAME_CHARS: usize = 255;

/// The characters no symbol table name may hold.
pub(crate) const BAD_CHARS: &[char] = &['<', '>', '/', '\\', '"', ':', ';', '?', '*', '|', ',', '=', '`'];

/// Why `name` cannot name a symbol table entry (None when it can). The text names the bad
/// character so the caller can show it as is.
pub(crate) fn name_problem(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        return Some("it is empty".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Some(format!("it is longer than {MAX_NAME_CHARS} characters"));
    }
    if let Some(c) = name.chars().find(|c| c.is_control()) {
        return Some(format!("it holds the control character U+{:04X}", c as u32));
    }
    name.chars().find(|c| BAD_CHARS.contains(c)).map(|c| format!("it holds `{c}` (names can't hold any of < > / \\ \" : ; ? * | , = `)"))
}

/// Whether `name` is a legal symbol table name.
pub(crate) fn is_valid(name: &str) -> bool {
    name_problem(name).is_none()
}

/// Refuse a bad name for command `cmd`: "invalid <kind> name `x`: it holds `|` …".
pub(crate) fn check(cmd: &str, kind: &str, name: &str) -> Result<()> {
    match name_problem(name) {
        None => Ok(()),
        Some(why) => Err(bad(cmd, format!("invalid {kind} name `{}`: {why}", name.escape_debug()))),
    }
}

#[cfg(test)]
mod tests;
