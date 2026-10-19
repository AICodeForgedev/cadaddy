#![no_main]
//! SVG import.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = cadaddy_io::read(data, "fuzz.svg");
});
