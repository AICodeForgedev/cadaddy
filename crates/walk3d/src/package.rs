//! Walkthrough packages (`.cdywalk`): everything needed to walk a building without CADaddy's
//! files around it — the drawing (`.cdy`, its building spec and tours inside), the built model as
//! glTF for other viewers, the spec and tours again as plain JSON, and the baked lightmap — in one
//! file a web page can open. CADaddy (desktop or web) opens a package like a drawing and goes
//! straight into Walk.
//!
//! Layout: the magic `CDYWALK\x01`, then for each file a little-endian u32 name length, the name
//! (UTF-8), a u32 data length and the data (the entries of an `.obj.bundle`), and a zero name
//! length to end it (a package cut short is never taken for a smaller one). The manifest comes
//! first.

/// The first bytes of a package.
pub const MAGIC: &[u8; 8] = b"CDYWALK\x01";
/// The file extension.
pub const EXTENSION: &str = "cdywalk";
pub const MANIFEST: &str = "manifest.json";
pub const DRAWING: &str = "drawing.cdy";
pub const MODEL: &str = "model.glb";
pub const SPEC: &str = "spec.json";
pub const TOURS: &str = "tours.json";
pub const LIGHTMAP_PNG: &str = "lightmap.png";
pub const LIGHTMAP_JSON: &str = "lightmap.json";
/// The package format written.
pub const FORMAT: u32 = 1;
/// Most files read from one package, and the longest name (hostile files).
const MAX_FILES: usize = 1_024;
const MAX_NAME: usize = 1_024;

/// A package's files, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Package {
    pub files: Vec<(String, Vec<u8>)>,
}

/// Whether `bytes` are a package.
pub fn is_package(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

impl Package {
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.files.iter().find(|(n, _)| n == name).map(|(_, d)| d.as_slice())
    }

    /// Add a file, replacing one of the same name.
    pub fn put(&mut self, name: &str, data: Vec<u8>) {
        match self.files.iter_mut().find(|(n, _)| n == name) {
            Some(f) => f.1 = data,
            None => self.files.push((name.to_string(), data)),
        }
    }

    /// The manifest, when there is one that is a JSON object.
    pub fn manifest(&self) -> Option<serde_json::Value> {
        serde_json::from_slice::<serde_json::Value>(self.get(MANIFEST)?).ok().filter(serde_json::Value::is_object)
    }

    /// The package's bytes. Files over 4 GiB can't be stored.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let mut b = Vec::with_capacity(MAGIC.len() + self.files.iter().map(|(n, d)| 8 + n.len() + d.len()).sum::<usize>());
        b.extend_from_slice(MAGIC);
        for (name, data) in &self.files {
            let nl = u32::try_from(name.len()).map_err(|_| format!("{name}: name too long"))?;
            let dl = u32::try_from(data.len()).map_err(|_| format!("{name}: larger than a package holds (4 GiB)"))?;
            b.extend(nl.to_le_bytes());
            b.extend(name.as_bytes());
            b.extend(dl.to_le_bytes());
            b.extend_from_slice(data);
        }
        b.extend(0u32.to_le_bytes());
        Ok(b)
    }

    /// Read a package; damage (a truncated file, a bad name) is an error, never a guess.
    pub fn decode(bytes: &[u8]) -> Result<Package, String> {
        let mut rest = bytes.strip_prefix(MAGIC.as_slice()).ok_or("not a CADaddy walkthrough package")?;
        let mut files = Vec::new();
        let take = |rest: &mut &[u8], n: usize| -> Result<Vec<u8>, String> {
            let (a, b) = rest.split_at_checked(n).ok_or("the walkthrough package is cut short")?;
            *rest = b;
            Ok(a.to_vec())
        };
        let len = |rest: &mut &[u8]| -> Result<usize, String> {
            let (a, b) = rest.split_first_chunk::<4>().ok_or("the walkthrough package is cut short")?;
            *rest = b;
            usize::try_from(u32::from_le_bytes(*a)).map_err(|_| "a file in the package is too large".to_string())
        };
        loop {
            let nl = len(&mut rest)?;
            if nl == 0 {
                return if rest.is_empty() { Ok(Package { files }) } else { Err("the walkthrough package has data after its end".into()) };
            }
            if files.len() >= MAX_FILES {
                return Err(format!("more than {MAX_FILES} files in the walkthrough package"));
            }
            if nl > MAX_NAME {
                return Err("a file in the walkthrough package has a bad name".into());
            }
            let name = String::from_utf8(take(&mut rest, nl)?).map_err(|_| "a file in the walkthrough package has a bad name")?;
            // Names stay inside the package: no absolute paths, drives or `..` (zip-slip).
            if !cadaddy_geom::filename::is_safe_relative(&name) {
                return Err("a file in the walkthrough package has a name that leaves the package".into());
            }
            let dl = len(&mut rest)?;
            let data = take(&mut rest, dl)?;
            files.push((name, data));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_package_round_trips_and_damage_is_refused() {
        let mut p = Package::default();
        p.put(MANIFEST, br#"{"format":1}"#.to_vec());
        p.put(DRAWING, vec![1, 2, 3]);
        p.put(MODEL, Vec::new());
        p.put(DRAWING, vec![4, 5]);
        let bytes = p.encode().unwrap();
        assert!(is_package(&bytes));
        let back = Package::decode(&bytes).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.get(DRAWING), Some([4u8, 5].as_slice()));
        assert_eq!(back.get(MODEL), Some([].as_slice()));
        assert_eq!(back.manifest().unwrap()["format"], 1);
        // Cut short anywhere: an error.
        for n in MAGIC.len() + 1..bytes.len() {
            assert!(Package::decode(&bytes[..n]).is_err(), "cut at {n}");
        }
        assert!(Package::decode(b"not a package").is_err());
        let mut huge = MAGIC.to_vec();
        huge.extend(u32::MAX.to_le_bytes());
        assert!(Package::decode(&huge).is_err());
        let mut bad_name = MAGIC.to_vec();
        bad_name.extend(2u32.to_le_bytes());
        bad_name.extend([0xff, 0xfe]);
        bad_name.extend(0u32.to_le_bytes());
        bad_name.extend(0u32.to_le_bytes());
        assert!(Package::decode(&bad_name).is_err());
        assert!(Package::decode(MAGIC).is_err(), "no end");
        assert_eq!(Package::decode(&Package::default().encode().unwrap()).unwrap(), Package::default());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(Package::decode(&trailing).is_err());
    }

    #[test]
    fn names_that_leave_the_package_are_refused() {
        for bad in ["../evil.sh", "/etc/x", "C:\\Windows\\x.dll", "a/../../x"] {
            let mut p = Package::default();
            p.put(bad, b"x".to_vec());
            let e = Package::decode(&p.encode().unwrap()).unwrap_err();
            assert!(e.contains("leaves the package"), "{bad}: {e}");
        }
    }
}
