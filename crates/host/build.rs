//! On Linux, sets the runtime library path to the executable's own directory (`$ORIGIN`), where
//! packages ship the llama.cpp shared libraries, as for the app.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
}
