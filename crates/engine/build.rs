//! On Linux, the example binaries find the llama.cpp shared libraries next to themselves
//! (`$ORIGIN`), as packages ship them. See `crates/app/build.rs`.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-examples=-Wl,-rpath,$ORIGIN");
    }
}
