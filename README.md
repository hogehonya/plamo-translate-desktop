# PLaMo Translate Desktop

[日本語](README.ja.md)

A desktop translation app that runs [PLaMo 3 Translate](https://www.preferred.jp/ja/blog/tech/plamo-3-translate-weight-release)
locally on your computer. It works offline once a model is downloaded. Written in Rust with [GPUI](https://www.gpui.rs/).

**Built with PLaMo.**

## Features

- Translation between 53 languages. The default target is "Japanese ⇄ English (auto)".
- Paste to translate; text you type is translated after a short pause. The translation streams in as it is generated, with the live speed in the status bar, and can be stopped at any time.
- Japanese output style (polite or plain), a reference context, and a glossary (`source = target`, one term per line).
- Long texts are split into paragraphs and translated in order, with earlier pairs passed as history to keep wording consistent.
  Output that falls into a repetition loop is detected, withdrawn, and translated again sentence by sentence.
- Built-in model manager: choose a model size and quantization (0.8B, 2B, 8B or 31B GGUF).
  - Each file shows the translation quality PFN publishes, with the difference from the unquantized model.
  - Downloads resume after an interruption and are verified with SHA-256.
  - Newly published files are picked up from Hugging Face without an app update.
- Runs on the CPU (AVX2 or AVX-512) or on a GPU through Vulkan. Linux source builds can also use ROCm. The device is selectable, and automatic selection picks the discrete GPU.
- English and Japanese UI. It follows the OS language by default and can be changed under Settings (the gear in the status bar). `PLAMO_LANG=ja` or `en` overrides both.

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

Windows (x86_64) and Linux (x86_64) packages are on the [Releases](https://github.com/tanakh/plamo-translate-desktop/releases) page.
To try a development version, run the **Release** workflow under **Actions** on a branch and download the packages from its artifacts.

- **Windows**: unzip and run `plamo-translate-desktop.exe`. Keep the DLLs next to it: llama.cpp and its CPU and Vulkan backends are loaded from there, and only from there.
- **Linux**: extract and run `plamo-translate-desktop`. The shared libraries next to it are found through the executable's own directory (`RUNPATH=$ORIGIN`).
  It needs a Wayland or X11 session with a Vulkan driver, which the UI uses for rendering and translation can use on the GPU, and a CJK font such as `fonts-noto-cjk`.

On both, translation uses a discrete GPU through Vulkan when one is available, and otherwise the fastest CPU code path the CPU supports (AVX2 or AVX-512).

## Building from source

Requirements: Rust 1.92 or later, CMake, and a C/C++ compiler.

Linux (Ubuntu or Debian):

```sh
sudo apt install cmake libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev
cargo run -p plamo-translate-desktop --release
```

This development build runs on the CPU, with llama.cpp linked statically.
Packages are built like the Release workflow does: with the size-optimized `dist` profile (fat LTO, stripped), Vulkan, and dynamically loaded backends.
The Vulkan backend also needs `libvulkan-dev glslc spirv-headers`.

```sh
cargo build --profile dist -p plamo-translate-desktop --features engine/vulkan,engine/dynamic-backends
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

### Files and logs

Models, the cached model list (`catalog.json`), and a small log (`app.log`) are kept in:

- Windows: `%LOCALAPPDATA%\plamo-translate-desktop\data`
- Linux: `~/.local/share/plamo-translate-desktop` (`$XDG_DATA_HOME`)

The log records start-up, detected devices, model loads, downloads, and deletions, but never translated text.
A log larger than 1 MiB is moved to `app.log.1` at start-up.

Settings are in `settings.json`: `%APPDATA%\plamo-translate-desktop\config` on Windows, and `~/.config/plamo-translate-desktop` (`$XDG_CONFIG_HOME`) on Linux.

## License

The source code of this app is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. The PLaMo models are not part of this repository and are covered by the PLaMo Community License.
