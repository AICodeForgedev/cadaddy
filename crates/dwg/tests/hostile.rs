//! Hostile DWG files found by `cargo xtask fuzz dwg`: each must be refused or read, never abort.

/// An R2004 section whose header claims ~35 PB: acadrust 0.6.4 allocates it outright (the
/// process aborts; an allocation failure can't be caught). acadrust 0.6.5 bounds the size by the
/// section's pages; un-ignore this when Cargo.toml moves to it (it is two weeks old on 2026-11-01).
#[test]
#[ignore = "aborts with acadrust 0.6.4; fixed in 0.6.5 (see the comment)"]
fn an_ac18_section_claiming_petabytes_is_refused() {
    let bytes = include_bytes!("regress/ac18-section-size.dwg");
    let _ = cadaddy_dwg::dwg_to_dxf_noted(bytes, cadaddy_dwg::Limits::DEFAULT);
}
