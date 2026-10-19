//! One PDF from several (a sheet set's sheets, published together), with a bookmark per part:
//! parts that name a group (a subset) are bookmarked under it, in order.

use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};

use crate::{IoError, Result};

/// Most parts merged, and most pages.
const MAX_PARTS: usize = 5_000;
const MAX_PAGES: usize = 20_000;

/// A bookmark title as a PDF text string (UTF-16BE with its byte order mark).
fn text(s: &str) -> Object {
    let mut b = vec![0xFE, 0xFF];
    for u in s.chars().take(500).collect::<String>().encode_utf16() {
        b.extend_from_slice(&u.to_be_bytes());
    }
    Object::String(b, StringFormat::Hexadecimal)
}

/// The page attributes a page inherits from its page tree (PDF 32000 §7.7.3.4).
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Merge `parts` (PDF bytes, bookmark title, group: "" for none) into one PDF.
pub fn merge(parts: &[(Vec<u8>, String, String)]) -> Result<Vec<u8>> {
    if parts.is_empty() {
        return Err(IoError::Format("nothing to merge".into()));
    }
    if parts.len() > MAX_PARTS {
        return Err(IoError::Format(format!("at most {MAX_PARTS} PDFs can be merged")));
    }
    let mut out = Document::with_version("1.7");
    let pages_id = out.new_object_id();
    let mut kids: Vec<Object> = Vec::new();
    // (title, group, first page) per part.
    let mut marks: Vec<(String, String, ObjectId)> = Vec::new();
    for (i, (bytes, title, group)) in parts.iter().enumerate() {
        let mut d = Document::load_mem(bytes).map_err(|e| IoError::Format(format!("part {}: {e}", i + 1)))?;
        d.renumber_objects_with(out.max_id + 1);
        let pages: Vec<ObjectId> = d.get_pages().into_values().collect();
        if kids.len() + pages.len() > MAX_PAGES {
            return Err(IoError::Format(format!("more than {MAX_PAGES} pages")));
        }
        // Each page carries what it inherited, then hangs from the new page tree.
        for p in &pages {
            let mut inherited: Vec<(Vec<u8>, Object)> = Vec::new();
            let mut parent = d.get_dictionary(*p).ok().and_then(|pd| pd.get(b"Parent").ok()).and_then(|o| o.as_reference().ok());
            let mut guard = 0;
            while let Some(pid) = parent {
                guard += 1;
                if guard > 64 {
                    break;
                }
                let Ok(node) = d.get_dictionary(pid) else { break };
                for k in INHERITED {
                    if let Ok(v) = node.get(k)
                        && !inherited.iter().any(|(n, _)| n == k)
                    {
                        inherited.push((k.to_vec(), v.clone()));
                    }
                }
                parent = node.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
            }
            if let Ok(Object::Dictionary(pd)) = d.objects.get_mut(p).ok_or(()) {
                for (k, v) in inherited {
                    if pd.get(&k).is_err() {
                        pd.set(k, v);
                    }
                }
                pd.set("Parent", Object::Reference(pages_id));
            }
            kids.push(Object::Reference(*p));
        }
        if let Some(first) = pages.first() {
            marks.push((title.clone(), group.clone(), *first));
        }
        // Everything but the part's own catalog, page tree nodes and outline.
        let root = d.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok());
        for (id, obj) in std::mem::take(&mut d.objects) {
            let kind = obj.as_dict().ok().and_then(|dd| dd.get(b"Type").ok()).and_then(|t| t.as_name().ok()).map(<[u8]>::to_vec);
            if Some(id) == root || matches!(kind.as_deref(), Some(b"Catalog" | b"Pages" | b"Outlines")) {
                continue;
            }
            out.objects.insert(id, obj);
        }
        out.max_id = out.max_id.max(d.max_id);
    }
    let count = kids.len() as i64;
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Object::Array(kids));
    pages.set("Count", Object::Integer(count));
    out.objects.insert(pages_id, Object::Dictionary(pages));
    // The outline: groups (subsets) in the order they first appear, each part under its group.
    let outlines_id = out.new_object_id();
    let mut top: Vec<(ObjectId, Dictionary)> = Vec::new();
    let mut children: Vec<Vec<(ObjectId, Dictionary)>> = Vec::new();
    for (title, group, page) in &marks {
        let mut item = Dictionary::new();
        item.set("Title", text(title));
        item.set("Dest", Object::Array(vec![Object::Reference(*page), Object::Name(b"Fit".to_vec())]));
        let id = out.new_object_id();
        if group.is_empty() {
            top.push((id, item));
            children.push(Vec::new());
            continue;
        }
        let g = top.iter().position(|(_, d)| d.get(b"Title").ok() == Some(&text(group)) && d.get(b"Dest").is_err());
        let g = match g {
            Some(g) => g,
            None => {
                let mut gd = Dictionary::new();
                gd.set("Title", text(group));
                let gid = out.new_object_id();
                top.push((gid, gd));
                children.push(Vec::new());
                top.len() - 1
            }
        };
        if let Some(c) = children.get_mut(g) {
            c.push((id, item));
        }
    }
    let link = |items: &mut Vec<(ObjectId, Dictionary)>, parent: ObjectId| {
        let ids: Vec<ObjectId> = items.iter().map(|(id, _)| *id).collect();
        for (i, (_, d)) in items.iter_mut().enumerate() {
            d.set("Parent", Object::Reference(parent));
            if let Some(prev) = i.checked_sub(1).and_then(|j| ids.get(j)) {
                d.set("Prev", Object::Reference(*prev));
            }
            if let Some(next) = ids.get(i + 1) {
                d.set("Next", Object::Reference(*next));
            }
        }
        ids
    };
    let top_ids = link(&mut top, outlines_id);
    let mut total = top.len() as i64;
    for ((gid, gd), kids) in top.iter_mut().zip(children.iter_mut()) {
        if kids.is_empty() {
            continue;
        }
        let ids = link(kids, *gid);
        if let (Some(f), Some(l)) = (ids.first(), ids.last()) {
            gd.set("First", Object::Reference(*f));
            gd.set("Last", Object::Reference(*l));
            gd.set("Count", Object::Integer(ids.len() as i64));
        }
        total += ids.len() as i64;
        for (id, d) in kids.drain(..) {
            out.objects.insert(id, Object::Dictionary(d));
        }
    }
    let mut outlines = Dictionary::new();
    outlines.set("Type", Object::Name(b"Outlines".to_vec()));
    if let (Some(f), Some(l)) = (top_ids.first(), top_ids.last()) {
        outlines.set("First", Object::Reference(*f));
        outlines.set("Last", Object::Reference(*l));
    }
    outlines.set("Count", Object::Integer(total));
    for (id, d) in top {
        out.objects.insert(id, Object::Dictionary(d));
    }
    out.objects.insert(outlines_id, Object::Dictionary(outlines));
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    catalog.set("Outlines", Object::Reference(outlines_id));
    catalog.set("PageMode", Object::Name(b"UseOutlines".to_vec()));
    let catalog_id = out.add_object(catalog);
    out.trailer.set("Root", Object::Reference(catalog_id));
    let mut bytes = Vec::new();
    out.save_to(&mut bytes).map_err(|e| IoError::Format(format!("writing the PDF: {e}")))?;
    Ok(bytes)
}

/// The bookmarks of PDF `bytes` as (title, depth, page number from 1), in order (for tests and
/// checks).
pub fn bookmarks(bytes: &[u8]) -> Result<Vec<(String, usize, usize)>> {
    let d = Document::load_mem(bytes).map_err(|e| IoError::Format(e.to_string()))?;
    let pages: Vec<ObjectId> = d.get_pages().into_values().collect();
    let title = |o: &Dictionary| -> String {
        match o.get(b"Title") {
            Ok(Object::String(b, _)) if b.starts_with(&[0xFE, 0xFF]) => {
                let u: Vec<u16> = b
                    .get(2..)
                    .unwrap_or_default()
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0)]))
                    .collect();
                String::from_utf16_lossy(&u)
            }
            Ok(Object::String(b, _)) => String::from_utf8_lossy(b).into_owned(),
            _ => String::new(),
        }
    };
    let mut out = Vec::new();
    let root = d.trailer.get(b"Root").and_then(Object::as_reference).map_err(|e| IoError::Format(e.to_string()))?;
    let first = d
        .get_dictionary(root)
        .ok()
        .and_then(|c| c.get(b"Outlines").ok())
        .and_then(|o| o.as_reference().ok())
        .and_then(|o| d.get_dictionary(o).ok())
        .and_then(|o| o.get(b"First").ok())
        .and_then(|o| o.as_reference().ok());
    let mut stack: Vec<(ObjectId, usize)> = first.map(|f| vec![(f, 0)]).unwrap_or_default();
    let mut seen = 0usize;
    while let Some((id, depth)) = stack.pop() {
        seen += 1;
        if seen > MAX_PAGES * 2 {
            break;
        }
        let Ok(item) = d.get_dictionary(id) else { continue };
        let page = item
            .get(b"Dest")
            .ok()
            .and_then(|o| o.as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| o.as_reference().ok())
            .and_then(|p| pages.iter().position(|q| *q == p))
            .map_or(0, |i| i + 1);
        out.push((title(item), depth, page));
        if let Ok(next) = item.get(b"Next").and_then(Object::as_reference) {
            stack.push((next, depth));
        }
        if let Ok(child) = item.get(b"First").and_then(Object::as_reference) {
            stack.push((child, depth + 1));
        }
    }
    Ok(out)
}
