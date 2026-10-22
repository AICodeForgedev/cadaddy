//! The files that tell the desktop which drawings CADaddy opens: the Linux `.desktop` entry and
//! MIME types, the macOS `Info.plist`, and the package scripts that install them. Opening a
//! `.dwg`, `.dxf` or `.cdy` from the file manager depends on all of them agreeing.

use std::path::PathBuf;

const APP_ID: &str = "dev.aicodeforge.cadaddy";
/// Each drawing format: extension and the MIME type CADaddy registers for it.
const FORMATS: [(&str, &str); 3] = [("cdy", "application/x-cadaddy-drawing"), ("dxf", "image/vnd.dxf"), ("dwg", "image/vnd.dwg")];

fn packaging(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packaging").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The value of `key=` in the `[Desktop Entry]` group.
fn desktop_key<'a>(entry: &'a str, key: &str) -> Option<&'a str> {
    entry.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
}

/// The `type` attribute of every `<mime-type>` in a shared-mime-info file, with its element.
fn mime_types(xml: &str) -> Vec<(&str, &str)> {
    xml.split("<mime-type type=\"")
        .skip(1)
        .filter_map(|chunk| {
            let (name, rest) = chunk.split_once('"')?;
            Some((name, rest.split("</mime-type>").next()?))
        })
        .collect()
}

/// A shared-mime-info string value's bytes (`\r`, `\n`, `\xNN` escapes).
fn magic_bytes(value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.extend(c.to_string().bytes());
            continue;
        }
        match chars.next() {
            Some('r') => out.push(b'\r'),
            Some('n') => out.push(b'\n'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                out.push(u8::from_str_radix(&hex, 16).expect("a \\x escape"));
            }
            other => panic!("unexpected escape {other:?} in {value}"),
        }
    }
    out
}

#[test]
fn the_desktop_entry_opens_files_and_lists_every_drawing_type() {
    let entry = packaging(&format!("linux/{APP_ID}.desktop"));
    assert!(entry.starts_with("[Desktop Entry]"));
    // The file manager passes the files as paths (`%F`), several at once.
    let exec = desktop_key(&entry, "Exec").expect("Exec");
    assert!(exec.split_whitespace().any(|a| a == "%F" || a == "%U"), "Exec passes files: {exec}");
    assert_eq!(desktop_key(&entry, "Icon"), Some(APP_ID));
    assert_eq!(desktop_key(&entry, "StartupWMClass"), Some(APP_ID), "the window is matched to its launcher");
    let listed: Vec<&str> = desktop_key(&entry, "MimeType").expect("MimeType").split(';').filter(|s| !s.is_empty()).collect();
    for (ext, mime) in FORMATS {
        assert!(listed.contains(&mime), ".{ext} ({mime}) is in MimeType: {listed:?}");
    }
    // Every type the package defines is one the entry opens.
    let xml = packaging(&format!("linux/{APP_ID}.mime.xml"));
    for (name, _) in mime_types(&xml) {
        assert!(listed.contains(&name), "{name} is defined but not opened");
    }
}

#[test]
fn the_mime_types_match_the_files_cadaddy_reads() {
    let xml = packaging(&format!("linux/{APP_ID}.mime.xml"));
    let types = mime_types(&xml);
    for (ext, mime) in FORMATS {
        let body = types.iter().find(|(n, _)| *n == mime).map(|(_, b)| *b).unwrap_or_else(|| panic!("{mime} is defined"));
        assert!(body.contains(&format!("<glob pattern=\"*.{ext}\"/>")), "{mime} globs *.{ext}");
        assert!(body.contains("<comment>"), "{mime} has a name for the file manager");
    }
    let magic = |mime: &str| -> Vec<u8> {
        let body = types.iter().find(|(n, _)| *n == mime).map(|(_, b)| *b).unwrap_or_default();
        let value = body.split("value=\"").nth(1).and_then(|v| v.split('"').next()).unwrap_or_else(|| panic!("{mime} has magic"));
        magic_bytes(value)
    };
    // A renamed .cdy is still recognised by its first bytes, the ones the reader checks.
    let cdy = magic("application/x-cadaddy-drawing");
    assert!(cadaddy_io::cdy::is_cdy(&cdy), "{cdy:?}");
    // Binary DXF's sentinel, as the DXF reader expects it; DWG's version string prefix.
    let dxf = magic("image/vnd.dxf");
    assert!(b"AutoCAD Binary DXF\r\n\x1a\0".starts_with(&dxf) && dxf.len() > 18, "{dxf:?}");
    assert_eq!(magic("image/vnd.dwg"), b"AC10");
}

#[test]
fn every_linux_package_installs_the_entry_and_the_types() {
    let desktop = format!("{APP_ID}.desktop");
    let mime = format!("{APP_ID}.mime.xml");
    // The staged tree (.deb, .rpm, .tar.gz and the AppImage's usr/).
    let script = packaging("linux/package.sh");
    assert!(script.contains("\"$HERE/$APP_ID.desktop\" \"$STAGE/usr/share/applications/$APP_ID.desktop\""));
    assert!(script.contains("\"$HERE/$APP_ID.mime.xml\" \"$STAGE/usr/share/mime/packages/$APP_ID.xml\""));
    assert!(script.contains("APP_ID=dev.aicodeforge.cadaddy"));
    // The AppImage also carries the entry at its root, for AppImage integrators.
    assert!(script.contains("cp \"$HERE/$APP_ID.desktop\" \"$APPDIR/$APP_ID.desktop\""));
    // deb/rpm take the whole staged share/ tree, then refresh the caches that make the types work.
    let nfpm = packaging("linux/nfpm.yaml");
    assert!(nfpm.contains("src: ${STAGE}/usr/share/") && nfpm.contains("dst: /usr/share/"));
    let postinst = packaging("linux/postinst.sh");
    for tool in ["update-desktop-database", "update-mime-database /usr/share/mime"] {
        assert!(postinst.contains(tool), "postinst runs {tool}");
    }
    assert!(nfpm.contains("postinstall: packaging/linux/postinst.sh") && nfpm.contains("postremove: packaging/linux/postinst.sh"));
    // Flatpak.
    let flatpak = packaging(&format!("linux/flatpak/{APP_ID}.yml"));
    assert!(flatpak.contains(&format!("packaging/linux/{desktop} /app/share/applications/{desktop}")));
    assert!(flatpak.contains(&format!("packaging/linux/{mime} /app/share/mime/packages/{APP_ID}.xml")));
}

#[test]
fn macos_declares_and_opens_every_drawing_type() {
    let plist = packaging("macos/Info.plist.in");
    let section = |key: &str| -> String {
        let start = plist.find(&format!("<key>{key}</key>")).unwrap_or_else(|| panic!("{key}"));
        let rest = plist.get(start..).unwrap_or_default();
        // The array that follows the key, to its matching end.
        let mut depth = 0usize;
        let mut end = 0;
        for (i, _) in rest.match_indices("array>") {
            let open = rest.get(..i).is_some_and(|s| s.ends_with('<'));
            if open {
                depth += 1;
            } else {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
        }
        rest.get(..end).unwrap_or_default().to_string()
    };
    let declared = format!("{}{}", section("UTExportedTypeDeclarations"), section("UTImportedTypeDeclarations"));
    let documents = section("CFBundleDocumentTypes");
    for (ext, mime) in FORMATS {
        // The declaration naming the extension (each starts with its identifier).
        let decl = declared
            .split("<key>UTTypeIdentifier</key>")
            .skip(1)
            .find(|d| d.contains(&format!("<string>{ext}</string>")))
            .unwrap_or_else(|| panic!(".{ext} is declared"));
        assert!(decl.contains(&format!("<string>{mime}</string>")), ".{ext} has the Linux MIME type");
        let uti = decl.split("<string>").nth(1).and_then(|r| r.split('<').next()).expect("an identifier");
        assert!(documents.contains(&format!("<string>{uti}</string>")), "CADaddy opens {uti} (.{ext})");
    }
    // The .cdy is CADaddy's own; the exchange formats are opened as an alternative.
    assert!(documents.contains("<string>Owner</string>") && documents.contains("<string>Alternate</string>"));
}
