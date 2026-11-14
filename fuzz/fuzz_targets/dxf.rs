#![no_main]
//! DXF, ASCII and binary.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cadaddy_fuzz::run(|| {
        let _ = cadaddy_io::read_dxf(data);
    });
});
