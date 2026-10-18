//! Hyperlinks on objects, kept as AutoCAD keeps them: extended data of the application `PE_URL`.
//!
//! ```text
//! 1001 PE_URL
//! 1000 <link>            the URL or file, with `#<named location>` when it has one
//! 1002 {
//! 1000 <description>
//! 1002 {
//! 1071 0
//! 1002 }
//! 1002 }
//! ```
//!
//! A link that is only a named location (`#Layout1`, `#Elevation`) points into the drawing that
//! holds it: a layout or a named view.

use crate::RawTag;

/// The extended data application hyperlinks are kept under.
pub const APP: &str = "PE_URL";
/// Longest link or description kept (longer text is cut).
pub const MAX_LEN: usize = 2048;

/// An object's hyperlink.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hyperlink {
    /// The URL or file name; empty for a link into the drawing itself.
    pub url: String,
    /// A named location: a layout or named view of the target drawing (this one when `url` is
    /// empty), or a web page's fragment.
    pub target: String,
    /// The text shown for the link (the tooltip); empty: the link itself.
    pub description: String,
}

impl Hyperlink {
    /// The link as stored: the URL, then `#` and the named location when it has one.
    pub fn link(&self) -> String {
        if self.target.is_empty() { self.url.clone() } else { format!("{}#{}", self.url, self.target) }
    }

    /// What a tooltip shows: the description, else the link.
    pub fn label(&self) -> String {
        if self.description.is_empty() { self.link() } else { self.description.clone() }
    }

    pub fn is_empty(&self) -> bool {
        self.url.trim().is_empty() && self.target.trim().is_empty()
    }
}

fn cut(s: &str) -> String {
    s.trim().chars().take(MAX_LEN).collect()
}

/// The range of `xdata` that is `PE_URL`'s: its `1001` group up to the next application's.
fn app_range(xdata: &[RawTag]) -> Option<std::ops::Range<usize>> {
    let start = xdata.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(APP))?;
    let len = xdata.get(start + 1..)?.iter().take_while(|t| t.code != 1001).count();
    Some(start..start + 1 + len)
}

/// The hyperlink in an object's extended data, if it has one.
pub fn read(xdata: &[RawTag]) -> Option<Hyperlink> {
    let r = app_range(xdata)?;
    let groups = xdata.get(r.start + 1..r.end)?;
    let link = groups.iter().find(|t| t.code == 1000)?.value.trim().to_string();
    // The description: the first string inside the first brace.
    let description = groups
        .iter()
        .skip_while(|t| !(t.code == 1002 && t.value.trim() == "{"))
        .skip(1)
        .take_while(|t| t.code != 1002)
        .find(|t| t.code == 1000)
        .map(|t| t.value.trim().to_string())
        .unwrap_or_default();
    let (url, target) = match link.split_once('#') {
        Some((u, t)) => (u.trim().to_string(), t.trim().to_string()),
        None => (link, String::new()),
    };
    let h = Hyperlink { url, target, description };
    (!h.is_empty()).then_some(h)
}

/// Give an object's extended data `link` (`None` or an empty link: remove its hyperlink),
/// keeping other applications' data as it is.
pub fn write(xdata: &mut Vec<RawTag>, link: Option<&Hyperlink>) {
    let at = app_range(xdata).map(|r| {
        xdata.drain(r.clone());
        r.start
    });
    let Some(h) = link.filter(|h| !h.is_empty()) else { return };
    let h = Hyperlink { url: cut(&h.url), target: cut(&h.target), description: cut(&h.description) };
    let t = |code: i32, value: &str| RawTag { code, value: value.to_string() };
    let groups = [t(1001, APP), t(1000, &h.link()), t(1002, "{"), t(1000, &h.description), t(1002, "{"), t(1071, "0"), t(1002, "}"), t(1002, "}")];
    let at = at.unwrap_or(xdata.len()).min(xdata.len());
    xdata.splice(at..at, groups);
}

/// Whether a link names a resource by scheme (`https:`, `mailto:`, `file:`…) rather than a
/// path. A one-letter "scheme" is a Windows drive (`C:\`).
fn has_scheme(url: &str) -> bool {
    url.split_once(':').is_some_and(|(s, _)| {
        s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) && s.starts_with(|c: char| c.is_ascii_alphabetic())
    })
}

fn is_absolute_path(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with('/') || p.starts_with('\\') || (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
}

/// A path as a `file:` URL.
fn file_url(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = p.replace(' ', "%20").replace('#', "%23");
    if p.starts_with('/') { format!("file://{p}") } else { format!("file:///{p}") }
}

/// What following a link opens: a URL a browser or the system opens, with a relative link
/// resolved against HYPERLINKBASE (`base`), else the drawing's folder (`folder`). `None` for a
/// link into the drawing itself (only a named location).
pub fn resolve(h: &Hyperlink, base: &str, folder: Option<&str>) -> Option<String> {
    let url = h.url.trim();
    if url.is_empty() {
        return None;
    }
    let fragment = if h.target.is_empty() { String::new() } else { format!("#{}", h.target) };
    if has_scheme(url) {
        return Some(format!("{url}{fragment}"));
    }
    if url.starts_with("www.") {
        return Some(format!("https://{url}{fragment}"));
    }
    if is_absolute_path(url) {
        return Some(format!("{}{fragment}", file_url(url)));
    }
    let base = base.trim();
    let joined = if !base.is_empty() {
        let sep = if base.ends_with('/') || base.ends_with('\\') {
            ""
        } else if has_scheme(base) {
            "/"
        } else {
            std::path::MAIN_SEPARATOR_STR
        };
        format!("{base}{sep}{url}")
    } else {
        match folder.filter(|f| !f.is_empty()) {
            Some(f) => std::path::Path::new(f).join(url).to_string_lossy().into_owned(),
            None => url.to_string(),
        }
    };
    if has_scheme(&joined) {
        Some(format!("{joined}{fragment}"))
    } else if is_absolute_path(&joined) {
        Some(format!("{}{fragment}", file_url(&joined)))
    } else {
        Some(format!("{joined}{fragment}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[(i32, &str)]) -> Vec<RawTag> {
        list.iter().map(|(c, v)| RawTag { code: *c, value: v.to_string() }).collect()
    }

    #[test]
    fn reads_and_writes_autocads_layout() {
        // As another program's file has it (with other applications' data around it).
        let x = tags(&[
            (1001, "ACME"),
            (1000, "a"),
            (1001, "PE_URL"),
            (1000, "https://example.com/spec"),
            (1002, "{"),
            (1000, "Specification"),
            (1002, "{"),
            (1071, "0"),
            (1002, "}"),
            (1002, "}"),
            (1001, "OTHER"),
            (1070, "1"),
        ]);
        let h = read(&x).unwrap();
        assert_eq!(h, Hyperlink { url: "https://example.com/spec".into(), target: String::new(), description: "Specification".into() });
        // Replaced in place, the other applications' data untouched.
        let mut y = x.clone();
        write(&mut y, Some(&Hyperlink { url: "plans/a.dwg".into(), target: "Layout1".into(), description: "".into() }));
        assert_eq!(y.first(), x.first());
        assert_eq!(y.last(), x.last());
        assert_eq!(y.iter().filter(|t| t.code == 1001).count(), 3);
        let h = read(&y).unwrap();
        assert_eq!((h.url.as_str(), h.target.as_str(), h.label().as_str()), ("plans/a.dwg", "Layout1", "plans/a.dwg#Layout1"));
        // Removed: the rest stays.
        write(&mut y, None);
        assert_eq!(read(&y), None);
        assert_eq!(y, tags(&[(1001, "ACME"), (1000, "a"), (1001, "OTHER"), (1070, "1")]));
        // Added to an object without one; an empty link adds nothing.
        let mut z = Vec::new();
        write(&mut z, Some(&Hyperlink { target: "Elevation".into(), ..Default::default() }));
        assert_eq!(read(&z).map(|h| h.link()), Some("#Elevation".to_string()));
        let mut e = Vec::new();
        write(&mut e, Some(&Hyperlink::default()));
        assert!(e.is_empty());
        // Hostile: huge text is cut, broken data reads as no link.
        let mut big = Vec::new();
        write(&mut big, Some(&Hyperlink { url: "x".repeat(100_000), ..Default::default() }));
        assert_eq!(read(&big).map(|h| h.url.len()), Some(MAX_LEN));
        assert_eq!(read(&tags(&[(1001, "PE_URL")])), None);
        assert_eq!(read(&tags(&[(1001, "PE_URL"), (1002, "{")])), None);
    }

    #[test]
    fn links_resolve_against_the_base_or_the_drawing_folder() {
        let l = |url: &str, target: &str| Hyperlink { url: url.into(), target: target.into(), description: String::new() };
        assert_eq!(resolve(&l("https://example.com/a", ""), "", None).as_deref(), Some("https://example.com/a"));
        assert_eq!(resolve(&l("https://example.com/a", "part"), "", None).as_deref(), Some("https://example.com/a#part"));
        assert_eq!(resolve(&l("www.example.com", ""), "", None).as_deref(), Some("https://www.example.com"));
        assert_eq!(resolve(&l("mailto:info@example.com", ""), "", None).as_deref(), Some("mailto:info@example.com"));
        assert_eq!(resolve(&l("/srv/plans/a b.pdf", ""), "", None).as_deref(), Some("file:///srv/plans/a%20b.pdf"));
        assert_eq!(resolve(&l("C:\\plans\\a.dwg", "Layout1"), "", None).as_deref(), Some("file:///C:/plans/a.dwg#Layout1"));
        assert_eq!(
            resolve(&l("docs/spec.pdf", ""), "https://example.com/project", None).as_deref(),
            Some("https://example.com/project/docs/spec.pdf")
        );
        assert_eq!(
            resolve(&l("docs/spec.pdf", ""), "https://example.com/project/", None).as_deref(),
            Some("https://example.com/project/docs/spec.pdf")
        );
        assert_eq!(resolve(&l("spec.pdf", ""), "", Some("/srv/plans")).as_deref(), Some("file:///srv/plans/spec.pdf"));
        assert_eq!(resolve(&l("", "Layout1"), "https://example.com", Some("/srv")), None, "a link into the drawing");
    }
}
