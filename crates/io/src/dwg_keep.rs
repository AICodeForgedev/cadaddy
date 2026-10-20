//! Keeping through DWG what acadrust can't write: other programs' custom entities and objects
//! (and their CLASS records), read from a DXF as their own groups. acadrust writes a custom
//! record to DWG only from the record's own DWG bytes, which a DXF doesn't have, so the bridge
//! used to leave them out.
//!
//! On the way to DWG ([`stash`]) such a record is taken out of the DXF whole, groups as written,
//! and kept in an XRECORD of a `CADADDY_KEEP` dictionary hanging off the named object
//! dictionary — XRECORDs go through DWG intact. So are its CLASS record and the references kept
//! records hold to it (a dictionary's entry, a reactor), which would otherwise point at nothing.
//! On the way back ([`unstash`]) the records are put back where they were (the ENTITIES section,
//! the block they were in, or OBJECTS) with their handles, their classes and the references, and
//! the dictionary goes: the DXF reads as it was written.
//!
//! Other programs reading the DWG see the XRECORDs, not the objects themselves (that needs the
//! objects' DWG encoding, which only their own program has).
//!
//! Which records need it the DWG bridge says (`cadaddy_dwg::custom_records`); the drawing's DWG
//! bridge DXF is binary ([`stash_tags`]), what the bridge reads from a DWG is ASCII ([`unstash`]).

use std::collections::{BTreeMap, HashMap, HashSet};

/// The dictionary key of the kept records in the named object dictionary.
pub const KEY: &str = "CADADDY_KEEP";
/// Records kept, at most (a hostile DXF can't make the bridge build millions of XRECORDs).
const MAX_KEPT: usize = 200_000;

/// A DXF's groups, as (code, value) with the value as written.
type Pairs = Vec<(i32, String)>;

/// The groups of ASCII DXF text, or `None` for binary DXF or text that isn't pairs.
fn parse(dxf: &[u8]) -> Option<Pairs> {
    if dxf.starts_with(b"AutoCAD Binary DXF") {
        return None;
    }
    let text = String::from_utf8_lossy(dxf);
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let mut out = Vec::new();
    while let Some(code) = lines.next() {
        if code.trim().is_empty() && out.last().is_some_and(|(c, v): &(i32, String)| *c == 0 && v == "EOF") {
            break;
        }
        let code: i32 = code.trim().parse().ok()?;
        let value = lines.next()?;
        out.push((code, value.to_string()));
    }
    Some(out)
}

fn write(pairs: &[(i32, String)], crlf: bool) -> Vec<u8> {
    let nl = if crlf { "\r\n" } else { "\n" };
    let mut s = String::with_capacity(pairs.len() * 12);
    for (c, v) in pairs {
        s.push_str(&format!("{c:>3}{nl}{v}{nl}"));
    }
    s.into_bytes()
}

/// Records: each from a `0` group to the next.
fn records(pairs: &[(i32, String)]) -> Vec<std::ops::Range<usize>> {
    let starts: Vec<usize> = pairs.iter().enumerate().filter(|(_, (c, _))| *c == 0).map(|(i, _)| i).collect();
    starts.iter().enumerate().map(|(k, s)| *s..starts.get(k + 1).copied().unwrap_or(pairs.len())).collect()
}

fn handle_of(rec: &[(i32, String)]) -> Option<u64> {
    rec.iter().find(|(c, _)| *c == 5 || *c == 105).and_then(|(_, v)| u64::from_str_radix(v.trim(), 16).ok())
}

fn value(rec: &[(i32, String)], code: i32) -> Option<&str> {
    rec.iter().find(|(c, _)| *c == code).map(|(_, v)| v.trim())
}

/// Where each record is: its section, and inside BLOCKS the block it belongs to.
fn containers(pairs: &[(i32, String)], recs: &[std::ops::Range<usize>]) -> Vec<String> {
    let mut section = String::new();
    let mut block: Option<String> = None;
    let mut out = Vec::with_capacity(recs.len());
    for r in recs {
        let rec = &pairs[r.clone()];
        let kind = rec.first().map(|(_, v)| v.trim()).unwrap_or_default();
        match kind {
            "SECTION" => section = value(rec, 2).unwrap_or_default().to_string(),
            "ENDSEC" => section.clear(),
            "BLOCK" if section == "BLOCKS" => block = value(rec, 2).map(str::to_string),
            "ENDBLK" => block = None,
            _ => {}
        }
        out.push(match (&block, kind) {
            // A section's own header and end are in no container.
            (_, "SECTION" | "ENDSEC") => String::new(),
            (Some(b), k) if !matches!(k, "BLOCK" | "ENDBLK") => format!("BLOCK:{b}"),
            _ => section.clone(),
        });
    }
    out
}

/// One XRECORD of the kept dictionary: what it holds, where it goes, and the groups.
struct Kept {
    kind: &'static str,
    /// For `record` and `class`: the container ([`containers`]); for `entry` and `reactor`: the
    /// record (its handle, hex) the groups go back into.
    place: String,
    index: usize,
    pairs: Pairs,
}

/// The DXF with the records whose handles are in `take` kept in a `CADADDY_KEEP` dictionary
/// instead, and how many were; `None` when there's nothing to keep or the DXF isn't ASCII.
#[cfg(test)]
pub fn stash(dxf: &[u8], take: &HashSet<u64>) -> Option<(Vec<u8>, usize)> {
    let crlf = dxf.windows(2).any(|w| w == b"\r\n");
    let (out, n) = stash_pairs(&parse(dxf)?, take)?;
    Some((write(&out, crlf), n))
}

/// [`stash`] on a DXF's tags (the DWG bridge's binary DXF).
pub fn stash_tags(tags: &[cadaddy_dxf::Tag], take: &HashSet<u64>) -> Option<(Vec<cadaddy_dxf::Tag>, usize)> {
    if take.is_empty() {
        return None;
    }
    let pairs: Pairs = tags.iter().map(|t| (t.code, t.str())).collect();
    let (out, n) = stash_pairs(&pairs, take)?;
    Some((out.into_iter().map(|(c, v)| tag(c, v)).collect(), n))
}

/// A group from its text, typed by its code (the text is what [`cadaddy_dxf::Tag::str`] gave,
/// which writes reals in their shortest exact form).
fn tag(code: i32, v: String) -> cadaddy_dxf::Tag {
    use cadaddy_dxf::{Kind, Tag, Value};
    match cadaddy_dxf::kind_of(code) {
        Kind::Str => Tag::s(code, v),
        Kind::Real => Tag::new(code, Value::Real(v.trim().parse::<f64>().ok().filter(|f| f.is_finite()).unwrap_or(0.0))),
        Kind::Int16 | Kind::Int32 | Kind::Int64 => Tag::new(code, Value::Int(v.trim().parse().unwrap_or(0))),
        Kind::Bool => Tag::new(code, Value::Bool(v.trim() != "0")),
        Kind::Hex => Tag::new(code, Value::Hex(v)),
    }
}

fn stash_pairs(pairs: &[(i32, String)], take: &HashSet<u64>) -> Option<(Pairs, usize)> {
    if take.is_empty() {
        return None;
    }
    let recs = records(pairs);
    let places = containers(pairs, &recs);
    let mut kept: Vec<Kept> = Vec::new();
    let mut gone: Vec<bool> = vec![false; recs.len()];
    let mut taken_types: HashSet<String> = HashSet::new();
    let mut taken: HashSet<u64> = HashSet::new();
    let mut index_in: BTreeMap<String, usize> = BTreeMap::new();
    for (i, r) in recs.iter().enumerate() {
        let rec = &pairs[r.clone()];
        let at = index_in.entry(places[i].clone()).or_default();
        let here = *at;
        *at += 1;
        if kept.len() >= MAX_KEPT {
            continue;
        }
        if let Some(h) = handle_of(rec).filter(|h| take.contains(h)) {
            taken.insert(h);
            taken_types.insert(rec.first().map(|(_, v)| v.trim().to_string()).unwrap_or_default());
            kept.push(Kept { kind: "record", place: places[i].clone(), index: here, pairs: rec.to_vec() });
            gone[i] = true;
        }
    }
    if kept.is_empty() {
        return None;
    }
    let count = kept.len();
    // Their CLASS records.
    let mut class_index = 0;
    for (i, r) in recs.iter().enumerate() {
        let rec = &pairs[r.clone()];
        if rec.first().is_some_and(|(_, v)| v.trim() == "CLASS") {
            if value(rec, 1).is_some_and(|t| taken_types.contains(t)) {
                kept.push(Kept { kind: "class", place: "CLASSES".into(), index: class_index, pairs: rec.to_vec() });
                gone[i] = true;
            }
            class_index += 1;
        }
    }
    // What the rest hold of them: dictionary entries and reactors, taken out and kept.
    let points_at_taken = |v: &str| u64::from_str_radix(v.trim(), 16).is_ok_and(|h| taken.contains(&h));
    let mut out: Pairs = Vec::with_capacity(pairs.len());
    for (i, r) in recs.iter().enumerate() {
        if gone[i] {
            continue;
        }
        let rec = &pairs[r.clone()];
        let owner = handle_of(rec).map(|h| format!("{h:X}")).unwrap_or_default();
        let is_dict = rec.first().is_some_and(|(_, v)| v.trim().starts_with("DICTIONARY"));
        let mut in_reactors = false;
        let mut k = 0;
        while k < rec.len() {
            let (c, v) = &rec[k];
            if *c == 102 {
                in_reactors = v.trim() == "{ACAD_REACTORS";
            }
            if is_dict && *c == 3 && rec.get(k + 1).is_some_and(|(c2, v2)| matches!(c2, 350 | 360) && points_at_taken(v2)) && !owner.is_empty() {
                kept.push(Kept { kind: "entry", place: owner.clone(), index: 0, pairs: vec![rec[k].clone(), rec[k + 1].clone()] });
                k += 2;
                continue;
            }
            if in_reactors && *c == 330 && points_at_taken(v) && !owner.is_empty() {
                kept.push(Kept { kind: "reactor", place: owner.clone(), index: 0, pairs: vec![(330, v.clone())] });
                k += 1;
                continue;
            }
            out.push((*c, v.clone()));
            k += 1;
        }
    }
    // The kept dictionary and its XRECORDs, under new handles above every handle in the file.
    let seed = out
        .windows(2)
        .find(|w| w[0].0 == 9 && w[0].1.trim() == "$HANDSEED")
        .and_then(|w| u64::from_str_radix(w[1].1.trim(), 16).ok())
        .unwrap_or(0)
        .max(
            out.iter()
                .filter(|(c, _)| *c == 5 || *c == 105)
                .filter_map(|(_, v)| u64::from_str_radix(v.trim(), 16).ok())
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
    let dict_handle = seed;
    let mut next = seed.saturating_add(1);
    let objects_start = {
        let recs = records(&out);
        let places = containers(&out, &recs);
        recs.iter().zip(&places).find(|(_, p)| *p == "OBJECTS").map(|(r, _)| r.start)
    }?;
    // The named object dictionary: the first record of OBJECTS.
    let nod_handle = handle_of(&out[objects_start..records(&out[objects_start..]).first().map_or(objects_start, |r| objects_start + r.end)])?;
    let mut dict: Pairs = vec![
        (0, "DICTIONARY".into()),
        (5, format!("{dict_handle:X}")),
        (330, format!("{nod_handle:X}")),
        (100, "AcDbDictionary".into()),
        (281, "1".into()),
    ];
    let mut xrecords: Pairs = Vec::new();
    for (n, k) in kept.iter().enumerate() {
        let h = next;
        next = next.saturating_add(1);
        dict.push((3, format!("K{n:06}")));
        dict.push((350, format!("{h:X}")));
        xrecords.extend([
            (0, "XRECORD".to_string()),
            (5, format!("{h:X}")),
            (330, format!("{dict_handle:X}")),
            (100, "AcDbXrecord".into()),
            (280, "1".into()),
            (2, k.kind.to_string()),
            (3, k.place.clone()),
            (91, k.index.to_string()),
        ]);
        for (c, v) in &k.pairs {
            xrecords.push((92, c.to_string()));
            xrecords.push((1, v.clone()));
        }
    }
    // Into the named object dictionary's entries, and the new records at the end of OBJECTS.
    let nod_end = objects_start + records(&out[objects_start..]).first().map_or(0, |r| r.end);
    out.splice(nod_end..nod_end, [(3, KEY.to_string()), (350, format!("{dict_handle:X}"))]);
    let objects_end = {
        let recs = records(&out);
        let places = containers(&out, &recs);
        recs.iter().zip(&places).rev().find(|(_, p)| *p == "OBJECTS").map(|(r, _)| r.end)
    }?;
    out.splice(objects_end..objects_end, dict.into_iter().chain(xrecords));
    // The handle seed above what was added.
    if let Some(w) = out.iter().position(|(c, v)| *c == 9 && v.trim() == "$HANDSEED")
        && let Some(slot) = out.get_mut(w + 1)
    {
        slot.1 = format!("{next:X}");
    }
    Some((out, count))
}

/// The DXF with the records a [`stash`] kept put back, or `None` when it has no kept dictionary.
pub fn unstash(dxf: &[u8]) -> Option<Vec<u8>> {
    let pairs = parse(dxf)?;
    let crlf = dxf.windows(2).any(|w| w == b"\r\n");
    Some(write(&unstash_pairs(&pairs)?, crlf))
}

/// [`unstash`] on a DXF's tags (the DWG read natively).
pub fn unstash_tags(tags: &[cadaddy_dxf::Tag]) -> Option<Vec<cadaddy_dxf::Tag>> {
    if !tags.iter().any(|t| t.code == 3 && t.str().trim() == KEY) {
        return None;
    }
    let pairs: Pairs = tags.iter().map(|t| (t.code, t.str())).collect();
    Some(unstash_pairs(&pairs)?.into_iter().map(|(c, v)| tag(c, v)).collect())
}

fn unstash_pairs(pairs: &Pairs) -> Option<Pairs> {
    let pairs = pairs.clone();
    let recs = records(&pairs);
    let places = containers(&pairs, &recs);
    let objects: Vec<usize> = (0..recs.len()).filter(|i| places[*i] == "OBJECTS").collect();
    let nod = *objects.first()?;
    let nod_rec = &pairs[recs[nod].clone()];
    let at = nod_rec.windows(2).position(|w| w[0].0 == 3 && w[0].1.trim() == KEY && matches!(w[1].0, 350 | 360))?;
    let dict_handle = u64::from_str_radix(nod_rec[at + 1].1.trim(), 16).ok()?;
    let dict_i = objects.iter().copied().find(|i| handle_of(&pairs[recs[*i].clone()]) == Some(dict_handle))?;
    let entries: HashSet<u64> = pairs[recs[dict_i].clone()]
        .iter()
        .filter(|(c, _)| matches!(c, 350 | 360))
        .filter_map(|(_, v)| u64::from_str_radix(v.trim(), 16).ok())
        .collect();
    // Decode the XRECORDs.
    let mut kept: Vec<Kept> = Vec::new();
    let mut drop: HashSet<usize> = HashSet::from([dict_i]);
    for i in objects.iter().copied() {
        let rec = &pairs[recs[i].clone()];
        if rec.first().is_none_or(|(_, v)| v.trim() != "XRECORD") || !handle_of(rec).is_some_and(|h| entries.contains(&h)) {
            continue;
        }
        drop.insert(i);
        let kind = match value(rec, 2) {
            Some("record") => "record",
            Some("class") => "class",
            Some("entry") => "entry",
            Some("reactor") => "reactor",
            _ => continue,
        };
        let place = rec.iter().find(|(c, _)| *c == 3).map(|(_, v)| v.clone()).unwrap_or_default();
        let index = value(rec, 91).and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
        let mut groups: Pairs = Vec::new();
        let mut code: Option<i32> = None;
        for (c, v) in rec.iter().skip_while(|(c, _)| *c != 91).skip(1) {
            match (c, code) {
                (92, _) => code = v.trim().parse().ok(),
                (1, Some(g)) => {
                    groups.push((g, v.clone()));
                    code = None;
                }
                _ => {}
            }
        }
        kept.push(Kept { kind, place, index, pairs: groups });
    }
    // Everything but the kept dictionary, its XRECORDs and the named object dictionary's entry.
    let mut base: Vec<Pairs> = Vec::new();
    for (i, r) in recs.iter().enumerate() {
        if drop.contains(&i) {
            continue;
        }
        let mut rec = pairs[r.clone()].to_vec();
        if i == nod {
            rec.drain(at..at + 2);
        }
        base.push(rec);
    }
    // Back into the records they were taken from: dictionary entries and reactors.
    let by_handle: HashMap<u64, usize> = base.iter().enumerate().filter_map(|(i, r)| handle_of(r).map(|h| (h, i))).collect();
    for k in kept.iter().filter(|k| matches!(k.kind, "entry" | "reactor")) {
        let Ok(owner) = u64::from_str_radix(k.place.trim(), 16) else { continue };
        let Some(rec) = by_handle.get(&owner).and_then(|&i| base.get_mut(i)) else { continue };
        if k.kind == "entry" {
            rec.extend(k.pairs.iter().cloned());
        } else if let Some(open) = rec.iter().position(|(c, v)| *c == 102 && v.trim() == "{ACAD_REACTORS") {
            let close = rec.iter().skip(open).position(|(c, v)| *c == 102 && v.trim() == "}").map_or(rec.len(), |p| p + open);
            rec.splice(close..close, k.pairs.iter().cloned());
        } else {
            // Reactors come right after the handle.
            let after = rec.iter().position(|(c, _)| *c == 5 || *c == 105).map_or(1, |p| p + 1);
            let mut groups = vec![(102, "{ACAD_REACTORS".to_string())];
            groups.extend(k.pairs.iter().cloned());
            groups.push((102, "}".to_string()));
            rec.splice(after..after, groups);
        }
    }
    // The records and classes into their places, in the order they had: the n-th kept record
    // of a container becomes its n-th record (after the last when there are fewer).
    let flat: Pairs = base.into_iter().flatten().collect();
    let ranges = records(&flat);
    let places = containers(&flat, &ranges);
    let mut placed: Vec<&Kept> = kept.iter().filter(|k| matches!(k.kind, "record" | "class")).collect();
    placed.sort_by_key(|k| k.index);
    let container_of = |k: &Kept| if k.kind == "class" { "CLASSES".to_string() } else { k.place.clone() };
    let mut pending: HashMap<String, std::collections::VecDeque<&Kept>> = HashMap::new();
    for k in placed {
        pending.entry(container_of(k)).or_default().push_back(k);
    }
    // How many records each container has, to know its last.
    let mut left: HashMap<&str, usize> = HashMap::new();
    for p in &places {
        *left.entry(p.as_str()).or_default() += 1;
    }
    let mut recs_out: Vec<Pairs> = Vec::with_capacity(ranges.len() + kept.len());
    let mut places_out: Vec<String> = Vec::with_capacity(ranges.len() + kept.len());
    let mut count: HashMap<String, usize> = HashMap::new();
    for (r, place) in ranges.iter().zip(&places) {
        if let Some(queue) = pending.get_mut(place) {
            // Kept records whose place comes before this one.
            let n = count.entry(place.clone()).or_default();
            while queue.front().is_some_and(|k| k.index <= *n) {
                if let Some(k) = queue.pop_front() {
                    recs_out.push(k.pairs.clone());
                    places_out.push(place.clone());
                    *n += 1;
                }
            }
        }
        recs_out.push(flat.get(r.clone()).unwrap_or_default().to_vec());
        places_out.push(place.clone());
        *count.entry(place.clone()).or_default() += 1;
        if let Some(l) = left.get_mut(place.as_str()) {
            *l -= 1;
            // After a container's last record, the rest of its kept ones.
            if *l == 0
                && !place.is_empty()
                && let Some(queue) = pending.get_mut(place)
            {
                while let Some(k) = queue.pop_front() {
                    recs_out.push(k.pairs.clone());
                    places_out.push(place.clone());
                }
            }
        }
    }
    // Containers with no records left: right after their opening (the section header, or the
    // block's BLOCK record); a missing CLASSES section is made before TABLES.
    let mut rest: Vec<(String, &Kept)> = pending.into_iter().flat_map(|(c, q)| q.into_iter().map(move |k| (c.clone(), k))).collect();
    rest.sort_by_key(|(_, k)| k.index);
    for (container, k) in rest {
        let at = match container.strip_prefix("BLOCK:") {
            Some(name) => match recs_out.iter().position(|r| r.first().is_some_and(|(_, v)| v.trim() == "BLOCK") && value(r, 2) == Some(name)) {
                Some(p) => p + 1,
                None => continue,
            },
            None => {
                match recs_out.iter().position(|r| r.first().is_some_and(|(_, v)| v.trim() == "SECTION") && value(r, 2) == Some(container.as_str())) {
                    Some(p) => p + 1 + places_out.iter().skip(p + 1).take_while(|c| **c == container).count(),
                    None if container == "CLASSES" => {
                        let before = recs_out
                            .iter()
                            .position(|r| r.first().is_some_and(|(_, v)| v.trim() == "SECTION") && value(r, 2) == Some("TABLES"))
                            .unwrap_or(0);
                        recs_out
                            .splice(before..before, [vec![(0, "SECTION".to_string()), (2, "CLASSES".to_string())], vec![(0, "ENDSEC".to_string())]]);
                        places_out.splice(before..before, ["CLASSES".to_string(), String::new()]);
                        before + 1
                    }
                    None => continue,
                }
            }
        };
        recs_out.insert(at, k.pairs.clone());
        places_out.insert(at, container);
    }
    Some(recs_out.into_iter().flatten().collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up DXF: a custom class's entity in model space and in a block, a custom object in
    /// the named object dictionary that reacts to a line, and an XRECORD that stays as it is.
    pub const DXF: &str = "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n9\n$HANDSEED\n5\n400\n0\nENDSEC\n\
0\nSECTION\n2\nCLASSES\n0\nCLASS\n1\nACME_WIDGET\n2\nAcmeWidget\n3\nAcmeApp\n90\n0\n280\n0\n281\n0\n0\nCLASS\n1\nACME_GADGET\n2\nAcmeGadget\n3\nAcmeApp\n90\n0\n280\n0\n281\n1\n0\nENDSEC\n\
0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n5\n20\n2\nPART\n0\nACME_GADGET\n5\n120\n8\n0\n100\nAcmeGadget\n1\nin a block\n0\nENDBLK\n5\n21\n0\nENDSEC\n\
0\nSECTION\n2\nENTITIES\n0\nLINE\n5\n100\n102\n{ACAD_REACTORS\n330\n300\n102\n}\n8\n0\n10\n0\n20\n0\n11\n10\n21\n5\n0\nACME_GADGET\n5\n110\n8\n0\n100\nAcmeGadget\n1\ngadget label\n0\nCIRCLE\n5\n111\n8\n0\n10\n1\n20\n1\n40\n2\n0\nENDSEC\n\
0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n330\n0\n100\nAcDbDictionary\n3\nACME_SETTINGS\n350\n201\n3\nACME_WIDGETS\n350\n300\n0\n\
XRECORD\n5\n201\n330\nC\n100\nAcDbXrecord\n1\nsettings payload\n0\n\
ACME_WIDGET\n5\n300\n102\n{ACAD_REACTORS\n330\n100\n102\n}\n330\nC\n100\nAcmeWidget\n1\nspinner\n90\n7\n0\nENDSEC\n0\nEOF\n";

    fn take() -> HashSet<u64> {
        HashSet::from([0x110, 0x120, 0x300])
    }

    /// The groups without the handle seed (the kept dictionary raises it, and it stays raised).
    fn canon(dxf: &[u8]) -> Pairs {
        let mut p: Pairs = parse(dxf).unwrap().into_iter().map(|(c, v)| (c, v.trim().to_string())).collect();
        if let Some(i) = p.iter().position(|(c, v)| *c == 9 && v == "$HANDSEED") {
            p.drain(i..i + 2);
        }
        p
    }

    #[test]
    fn kept_records_leave_the_dxf_and_come_back_where_they_were() {
        let (stashed, n) = stash(DXF.as_bytes(), &take()).unwrap();
        assert_eq!(n, 3);
        let text = String::from_utf8(stashed.clone()).unwrap();
        // Gone from the DXF acadrust reads: the records, their classes and the references.
        let types: Vec<String> = parse(&stashed).unwrap().into_iter().filter(|(c, _)| *c == 0).map(|(_, v)| v).collect();
        assert!(!types.iter().any(|t| t.starts_with("ACME_") || t == "CLASS"), "{types:?}");
        assert!(!text.contains("ACME_WIDGETS\n350\n300"), "the dictionary entry to it");
        assert!(text.contains(KEY) && text.contains("AcDbXrecord"));
        assert!(text.contains("settings payload"), "the XRECORD it doesn't need to keep stays");
        // Back as they were: the same groups in the same order.
        let back = unstash(&stashed).unwrap();
        let (a, b) = (canon(&back), canon(DXF.as_bytes()));
        let first = a.iter().zip(&b).position(|(x, y)| x != y);
        assert_eq!(
            a,
            b,
            "first difference at {first:?}: {:?} vs {:?}",
            first.map(|i| &a[i.saturating_sub(3)..(i + 3).min(a.len())]),
            first.map(|i| &b[i.saturating_sub(3)..(i + 3).min(b.len())])
        );
    }

    #[test]
    fn nothing_to_keep_changes_nothing() {
        assert!(stash(DXF.as_bytes(), &HashSet::new()).is_none());
        assert!(stash(DXF.as_bytes(), &HashSet::from([0xABCDEF])).is_none());
        assert!(unstash(DXF.as_bytes()).is_none());
        assert!(stash(b"AutoCAD Binary DXF\r\n\x1a\0", &take()).is_none());
    }

    #[test]
    fn the_new_handles_are_above_the_seed() {
        let (stashed, _) = stash(DXF.as_bytes(), &take()).unwrap();
        let p = parse(&stashed).unwrap();
        let seed = p.windows(2).find(|w| w[0].1 == "$HANDSEED").map(|w| u64::from_str_radix(w[1].1.trim(), 16).unwrap()).unwrap();
        // Handles past the header (where $HANDSEED itself is a group 5).
        let body = p.iter().position(|(c, v)| *c == 0 && v == "ENDSEC").unwrap();
        let max = p[body..].iter().filter(|(c, _)| *c == 5).map(|(_, v)| u64::from_str_radix(v.trim(), 16).unwrap()).max().unwrap();
        assert!(seed > max && max >= 0x400, "seed {seed:X} max {max:X}");
    }
}
