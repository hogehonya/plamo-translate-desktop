# PLaMo Translate Desktop

A desktop translation app that runs [PLaMo 3 Translate](https://www.preferred.jp/ja/blog/tech/plamo-3-translate-weight-release)
locally on your computer. It works offline once a model is downloaded. Written in Rust with [GPUI](https://www.gpui.rs/).

**Built with PLaMo.**

## Features

- Translation between 53 languages. The default target is "Japanese ⇄ English (auto)".
- Paste to translate; text you type is translated after a short pause. The translation streams in as it is generated.
- Japanese output style (polite or plain), a reference context, and a glossary (`source = target`, one term per line).
- Long texts are split into paragraphs and translated in order, with earlier pairs passed as history to keep wording consistent.
- Built-in model manager: choose a model size and quantization (0.8B, 2B, 8B or 31B GGUF). The download resumes after an interruption and is verified with SHA-256.
- Runs on the CPU or on a GPU through Vulkan (Windows builds). The device and the context length are selectable.
- English and Japanese UI. It follows the OS language by default and can be changed under Settings (the gear in the title bar). `PLAMO_LANG=ja` or `en` overrides both.

Shortcuts: `Ctrl+Enter` translates now. `Ctrl+Shift+C` copies the translation.

## Models and their license

The app does not ship model weights. When you pick a model in the app, it downloads it from
[Preferred Networks on Hugging Face](https://huggingface.co/pfnet). The models are licensed under the
[PLaMo Community License](https://plamo.preferredai.jp/info/plamo-community-license-en)
([Japanese](https://plamo.preferredai.jp/info/plamo-community-license-ja)), and the app asks you to accept it before the first download.

- **Personal and non-commercial use** is free.
- **Commercial use**, including use inside an organization, requires
  [registration](https://forms.gle/mTL8tBLrMYXKNZD56) if your (or your affiliates') annual revenue is ¥1 billion or less.
  Above that, a separate license from Preferred Networks is required.
- If you publish translations produced by the model, indicate that they are PLaMo output (Article 5).

PLaMo is a trademark of Preferred Networks, Inc.

## Download

Windows (x86_64) and Linux (x86_64) builds are produced by GitHub Actions. Open a workflow run under **Actions** and download its artifacts.

- **Windows**: unzip and run `plamo-translate-desktop.exe`. Keep the DLLs next to it, because the CPU and Vulkan backends are loaded from there.
- **Linux**: the build runs on the CPU only. It needs a Wayland or X11 session with a Vulkan driver (used for rendering) and a CJK font such as `fonts-noto-cjk`.

## Building from source

Requirements: Rust 1.92 or later, CMake, and a C/C++ compiler.

Linux (Ubuntu or Debian):

```sh
sudo apt install cmake libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev
cargo run -p plamo-translate-desktop --release
```

On Linux (including WSL2) with an AMD GPU, the ROCm (HIP) backend runs the model on the GPU.
It needs ROCm and its development packages, for example `amdrocm-core-dev10.1-gfx1201 amdrocm-blas-dev10.1` for an RX 9070 XT.
`scripts/rocm.sh` sets up the environment and builds into `target/rocm`:

```sh
scripts/rocm.sh cargo run -p plamo-translate-desktop --release --features rocm
```

Windows needs Visual Studio 2022 with "Desktop development with C++" (MSVC and the Windows SDK) and CMake.
The GPU build also needs the [Vulkan SDK](https://vulkan.lunarg.com/sdk/home#windows):

```powershell
cargo build -p plamo-translate-desktop --release --features engine/vulkan,engine/dynamic-backends
```

With `dynamic-backends`, copy the `ggml-*.dll` modules from `target/release/build/llama-cpp-sys-2-*/out/backends` next to the executable,
or run it with `cargo run`, which finds them in the build directory.

### Development

- `crates/pfml`: PFML3 prompt builder (the PLaMo prompt format).
- `crates/engine`: llama.cpp backend, model catalog, downloads, long-text splitting. No GUI dependency.
- `crates/app`: the GPUI application.
- `docs/design.md`: the design document.

Two command-line examples run the engine without the GUI:

```sh
cargo run -p engine --release --example translate -- --model path/to/model.gguf --to en "こんにちは"
cargo run -p engine --release --example download -- --list
```

A GGUF path passed as the first argument (or in `PLAMO_MODEL`) overrides the model selected in the app.

## License

The source code of this app is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. The PLaMo models are not part of this repository and are covered by the PLaMo Community License.
