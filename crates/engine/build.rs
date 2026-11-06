//! Platform-specific linking.
//!
//! - Linux: the example binaries find the llama.cpp shared libraries next to themselves
//!   (`$ORIGIN`), as packages ship them. See `crates/app/build.rs`.
//! - macOS: links clang's runtime library for llama.cpp. ggml's Metal backend checks the OS
//!   version with `@available`. When the deployment target (`MACOSX_DEPLOYMENT_TARGET`) is older
//!   than the version checked, clang compiles the check into a call to
//!   `__isPlatformVersionAtLeast` in its runtime library (`libclang_rt.osx.a`), which rustc does
//!   not link. A library's `rustc-link-lib` reaches every executable that links this crate: the
//!   app, the browser host, and the examples.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => println!("cargo:rustc-link-arg-examples=-Wl,-rpath,$ORIGIN"),
        Ok("macos") => link_clang_runtime(),
        _ => {}
    }
}

fn link_clang_runtime() {
    let resource_dir = Command::new("clang")
        .arg("--print-resource-dir")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()));
    let dir = resource_dir.map(|dir| dir.join("lib/darwin"));
    match dir.filter(|dir| dir.join("libclang_rt.osx.a").is_file()) {
        Some(dir) => {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=static:-bundle=clang_rt.osx");
        }
        None => println!("cargo:warning=libclang_rt.osx.a not found; linking may fail"),
    }
}
