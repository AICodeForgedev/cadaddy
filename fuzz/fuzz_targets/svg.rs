#![no_main]
//! SVG import.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cadaddy_fuzz::run(|| {
        let _ = cadaddy_io::read(data, "fuzz.svg");
    });
});
