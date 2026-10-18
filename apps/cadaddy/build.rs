//! Windows only: embed the app icon and version info into `cadaddy.exe`. Elsewhere a no-op.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/cadaddy.ico");
    println!("cargo:rerun-if-env-changed=CADADDY_REQUIRE_WINRES");
    println!("cargo:rerun-if-env-changed=CADADDY_BUILD_SHA");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    if std::path::Path::new("../../assets/app-icon/cadaddy.ico").exists() {
        res.set_icon("../../assets/app-icon/cadaddy.ico");
    }
    res.set("ProductName", "CADaddy")
        .set("FileDescription", "CADaddy computer-aided design")
        .set("CompanyName", "CADaddy contributors")
        .set("LegalCopyright", "Copyright (c) 2026 AICodeForge. Third-party notices: see NOTICE.")
        .set("OriginalFilename", "cadaddy.exe")
        .set("InternalName", "cadaddy");
    if let Err(e) = res.compile() {
        if std::env::var_os("CADADDY_REQUIRE_WINRES").is_some() {
            eprintln!("embedding Windows resources failed: {e}");
            std::process::exit(1);
        }
        println!("cargo:warning=cadaddy.exe built without icon/version resources: {e}");
    }
}
