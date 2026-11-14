#![no_main]
//! DWG through CADaddy's own codec (`CADADDY_DWG=native`), the reader the bridge is checked against.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cadaddy_fuzz::run(|| {
        let _ = cadaddy_io::dwg_read::read(data);
    });
});
