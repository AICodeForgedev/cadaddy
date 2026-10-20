//! PDF review markups read back from an annotated page: every kind, its place, comment, author
//! and colour.

use cadaddy_doc::markup::{Markup, MarkupKind};
use cadaddy_io::pdf_markups::{annotated_pdf, read};

pub fn sample() -> Vec<Markup> {
    let m = |kind, points: Vec<[f64; 2]>, rect: [f64; 4], text: &str| Markup {
        kind,
        points,
        strokes: Vec::new(),
        rect,
        text: text.into(),
        author: "Reviewer".into(),
        subject: String::new(),
        color: Some([255, 0, 0]),
        date: String::new(),
    };
    let mut ink = m(MarkupKind::Ink, Vec::new(), [500.0, 500.0, 560.0, 540.0], "");
    ink.strokes = vec![vec![[500.0, 500.0], [520.0, 540.0], [560.0, 510.0]]];
    let mut hl = m(MarkupKind::Highlight, Vec::new(), [100.0, 700.0, 200.0, 712.0], "check this");
    hl.strokes = vec![vec![[100.0, 700.0], [200.0, 700.0], [200.0, 712.0], [100.0, 712.0]]];
    vec![
        m(MarkupKind::Cloud, vec![[100.0, 100.0], [300.0, 100.0], [300.0, 200.0], [100.0, 200.0]], [100.0, 100.0, 300.0, 200.0], "Move this wall 2'"),
        m(MarkupKind::Rectangle, Vec::new(), [400.0, 100.0, 450.0, 150.0], ""),
        m(MarkupKind::Arrow, vec![[320.0, 150.0], [380.0, 250.0]], [320.0, 150.0, 380.0, 250.0], ""),
        m(MarkupKind::Callout, vec![[200.0, 150.0], [250.0, 300.0], [260.0, 300.0]], [260.0, 290.0, 400.0, 320.0], "Add outlet here"),
        m(MarkupKind::Note, Vec::new(), [600.0, 600.0, 700.0, 640.0], "General note: ✓ verify"),
        m(MarkupKind::Stamp, Vec::new(), [650.0, 50.0, 750.0, 90.0], "APPROVED"),
        ink,
        hl,
    ]
}

#[test]
fn every_kind_reads_back() {
    let pdf = annotated_pdf(1224.0, 792.0, &sample()).unwrap();
    let page = read(&pdf, 1).unwrap();
    assert_eq!((page.width, page.height), (1224.0, 792.0));
    let kinds: Vec<MarkupKind> = page.markups.iter().map(|m| m.kind).collect();
    assert_eq!(
        kinds,
        [
            MarkupKind::Cloud,
            MarkupKind::Rectangle,
            MarkupKind::Arrow,
            MarkupKind::Callout,
            MarkupKind::Note,
            MarkupKind::Stamp,
            MarkupKind::Ink,
            MarkupKind::Highlight
        ]
    );
    let cloud = &page.markups[0];
    assert_eq!(cloud.text, "Move this wall 2'");
    assert_eq!(cloud.author, "Reviewer");
    assert_eq!(cloud.color, Some([255, 0, 0]));
    assert_eq!(cloud.points.len(), 4);
    assert_eq!(page.markups[2].points.first(), Some(&[320.0, 150.0]), "arrowhead first");
    assert_eq!(page.markups[3].points.len(), 3, "the callout's leader");
    assert_eq!(page.markups[4].text, "General note: ✓ verify", "UTF-16 text");
    assert_eq!(page.markups[5].text, "APPROVED");
    assert_eq!(page.markups[6].strokes[0].len(), 3);
    assert_eq!(page.markups[7].strokes.len(), 1);
    assert!(read(&pdf, 2).is_err());
    assert!(read(b"not a pdf", 1).is_err());
}

#[test]
fn an_icon_sized_note_is_a_sticky_note() {
    let note = |rect: [f64; 4]| Markup {
        kind: MarkupKind::Note,
        points: Vec::new(),
        strokes: Vec::new(),
        rect,
        text: "Verify the clearance in front of the panel.".into(),
        author: "Reviewer".into(),
        subject: String::new(),
        color: None,
        date: String::new(),
    };
    let pdf = annotated_pdf(612.0, 792.0, &[note([600.0, 600.0, 620.0, 620.0])]).unwrap();
    assert!(pdf.windows(13).any(|w| w == b"/Subtype/Text") || pdf.windows(14).any(|w| w == b"/Subtype /Text"), "a Text annotation");
    let page = read(&pdf, 1).unwrap();
    let m = &page.markups[0];
    assert_eq!((m.kind, m.rect, m.text.as_str()), (MarkupKind::Note, [600.0, 600.0, 620.0, 620.0], "Verify the clearance in front of the panel."));
    assert_eq!(m.points, [[600.0, 620.0]], "anchored at the icon's top left");
}
