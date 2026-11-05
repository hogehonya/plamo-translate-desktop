//! Platform-specific linking and resources.
//!
//! - Windows: embeds the application icon and version information. GPUI loads icon resource
//!   ID 1 for its windows, which is the ID `set_icon` uses.
//! - Linux: sets the runtime library path to the executable's own directory (`$ORIGIN`), where
//!   packages ship the llama.cpp shared libraries. This is the executable's location, never the
//!   current directory, and Windows looks next to the exe for DLLs the same way.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => {
            println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
            return;
        }
        Ok("windows") => {}
        _ => return,
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
