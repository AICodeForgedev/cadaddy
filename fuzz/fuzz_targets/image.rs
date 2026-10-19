#![no_main]
//! Raster image decoding (IMAGEATTACH, underlays).

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = cadaddy_render::images::decode(data);
});
