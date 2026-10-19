#![no_main]
//! DXF, ASCII and binary.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = cadaddy_io::read_dxf(data);
});
