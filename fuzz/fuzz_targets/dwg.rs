#![no_main]
//! DWG through the bridge, then its DXF.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Ok((dxf, _)) = cadaddy_dwg::dwg_to_dxf_noted(data, cadaddy_dwg::Limits::DEFAULT) {
        let _ = cadaddy_io::read_dxf(&dxf);
    }
});
