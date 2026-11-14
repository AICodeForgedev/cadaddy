#![no_main]
//! CADaddy's own .cdy files.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cadaddy_fuzz::run(|| {
        let _ = cadaddy_io::cdy::read_cdy(data);
    });
});
