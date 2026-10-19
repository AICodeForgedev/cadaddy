#![no_main]
//! PDF import.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = cadaddy_io::pdf_read::read(data, &cadaddy_io::pdf_read::PdfOptions::default());
});
