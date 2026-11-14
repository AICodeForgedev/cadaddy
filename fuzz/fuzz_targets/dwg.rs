#![no_main]
//! DWG through the bridge, then its DXF. The bridge runs its reader under `catch_unwind`, so a
//! panic in it is a read error ([`cadaddy_fuzz::run`] keeps it from counting as a crash).

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    cadaddy_fuzz::run(|| {
        if let Ok((dxf, _)) = cadaddy_dwg::dwg_to_dxf_noted(data, cadaddy_dwg::Limits::DEFAULT) {
            let _ = cadaddy_io::read_dxf(&dxf);
        }
    });
});
