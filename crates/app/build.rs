//! Embeds the application icon and version information into Windows executables.
//!
//! GPUI loads icon resource ID 1 for its windows, which is the ID `set_icon` uses.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon/icon.ico")
        .set("ProductName", "PLaMo Translate Desktop")
        .set("FileDescription", "PLaMo Translate Desktop")
        .set("LegalCopyright", "Copyright (c) 2026 Hideyuki Tanaka");
    if let Err(e) = res.compile() {
        panic!("failed to embed Windows resources: {e}");
    }
}
