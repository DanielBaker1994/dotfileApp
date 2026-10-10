//! Embed the bundle Info.plist into `__TEXT,__info_plist` (Swift builds do
//! this with `-Xlinker -sectcreate`). TCC and `Bundle.main` read it before
//! AppKit starts; see PLAN-rust-port.md item 7.

fn main() {
    println!("cargo:rerun-if-changed=../../Info.plist");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }
    let plist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Info.plist");
    if !plist.exists() {
        return;
    }
    let plist = std::fs::canonicalize(&plist).unwrap_or(plist);
    println!(
        "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}
