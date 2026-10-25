//! `cadaddy-cli run --cmd-file`: commands from a file (Windows PowerShell 5 strips the quotes
//! inside a --cmd JSON argument), and PDFIMPORT leaving the PDF's folder (the job's print set)
//! as it was.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cadaddy-cli")).args(args).output().unwrap()
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cc-cmdfile-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-page PDF with a line and a small dark raster image (one IMAGE object on import).
fn pdf_with_an_image() -> Vec<u8> {
    let content = "0 0 m 300 0 l S q 100 0 0 100 300 300 cm BI /W 2 /H 2 /CS /RGB /BPC 8 /F /AHx ID 000000fefefe103050fefefe> EI Q";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 792 612] /Contents 4 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        pdf.extend(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    pdf
}

/// The print folder with one sheet in it, as a job hands it over.
fn print_folder(root: &Path) -> PathBuf {
    let prints = root.join("Prints");
    std::fs::create_dir_all(&prints).unwrap();
    let pdf = prints.join("Sample Sheet.pdf");
    std::fs::write(&pdf, pdf_with_an_image()).unwrap();
    pdf
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    v.sort();
    v
}

/// What PowerShell 5's `>` and `Out-File` write: UTF-16 little-endian with a byte-order mark.
fn utf16le(text: &str) -> Vec<u8> {
    let mut b = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        b.extend(u.to_le_bytes());
    }
    b
}

fn json_path(p: &Path) -> String {
    serde_json::to_string(&p.to_string_lossy()).unwrap()
}

/// A drawing with no file yet: the images wait in the session's temp folder, and --save takes
/// them next to the saved drawing. The print folder keeps just its PDF.
#[test]
fn unsaved_import_keeps_out_of_the_print_folder_and_save_takes_the_images() {
    let root = dir("unsaved");
    let pdf = print_folder(&root);
    let cmds = root.join("cmds.txt");
    std::fs::write(&cmds, utf16le(&format!("# import one sheet\r\npdfimport {{\"path\":{}}}\r\n", json_path(&pdf)))).unwrap();
    let out_dir = root.join("Work");
    std::fs::create_dir_all(&out_dir).unwrap();
    let drawing = out_dir.join("Plan A.dxf");
    let out = cli(&["run", "--cmd-file", cmds.to_str().unwrap(), "--save", drawing.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("\"imageFiles\":1"), "{stdout}");
    assert_eq!(names(pdf.parent().unwrap()), ["Sample Sheet.pdf"], "nothing lands in the print folder");
    assert_eq!(names(&out_dir.join("Plan A images")), ["Sample Sheet-p1-1.png"]);
    let dxf = std::fs::read_to_string(&drawing).unwrap();
    assert!(dxf.contains("Plan A images") && !dxf.contains("cadaddy-images-"), "the drawing points at its own folder");
    let _ = std::fs::remove_dir_all(&root);
}

/// A saved drawing: the images go straight into "<drawing name> images" next to it.
#[test]
fn saved_drawing_takes_the_images_next_to_it() {
    let root = dir("saved");
    let pdf = print_folder(&root);
    let drawing = root.join("Work").join("Plan B.dxf");
    std::fs::create_dir_all(drawing.parent().unwrap()).unwrap();
    // A UTF-8 file with a byte-order mark (PowerShell 5's `Out-File -Encoding utf8`).
    let text = format!("\u{feff}saveas {{\"path\":{}}}\npdfimport {{\"path\":{}}}\nqsave {{}}\n", json_path(&drawing), json_path(&pdf));
    let cmds = root.join("cmds.jsonl");
    std::fs::write(&cmds, text).unwrap();
    let out = cli(&["run", "--cmd-file", cmds.to_str().unwrap()]);
    assert!(out.status.success(), "{:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    assert_eq!(names(pdf.parent().unwrap()), ["Sample Sheet.pdf"]);
    assert_eq!(names(&root.join("Work").join("Plan B images")), ["Sample Sheet-p1-1.png"]);
    assert!(std::fs::read_to_string(&drawing).unwrap().contains("Plan B images"));
    let _ = std::fs::remove_dir_all(&root);
}

/// A bad line says which file and line it is.
#[test]
fn a_bad_line_names_its_file_and_line() {
    let root = dir("bad");
    let cmds = root.join("cmds.jsonl");
    std::fs::write(&cmds, "line {\"points\":[[0,0],[10,0]]}\n\nline {\"points\":[[0,0],\n").unwrap();
    let out = cli(&["run", "--cmd-file", cmds.to_str().unwrap()]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("cmds.jsonl:3"), "{err}");
    std::fs::write(&cmds, [0xFF, 0xFE, 0x41]).unwrap();
    let out = cli(&["run", "--cmd-file", cmds.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "half a UTF-16 character is an error, not a crash");
    let _ = std::fs::remove_dir_all(&root);
}
