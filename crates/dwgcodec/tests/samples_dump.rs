//! Writes the sample DWGs to `DWGCODEC_DUMP` (a folder) for looking at; does nothing otherwise.
mod common;

#[test]
fn dump_samples() {
    let Ok(dir) = std::env::var("DWGCODEC_DUMP") else { return };
    for v in common::VERSIONS {
        std::fs::write(format!("{dir}/{v}.dwg"), common::dwg(v, std::env::var("DWGCODEC_N").ok().and_then(|n| n.parse().ok()).unwrap_or(12)))
            .unwrap();
    }
}
