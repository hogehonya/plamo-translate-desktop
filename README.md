# PLaMo Translate Desktop

[日本語](README.ja.md)

A desktop translation app that runs [PLaMo 3 Translate](https://www.preferred.jp/ja/blog/tech/plamo-3-translate-weight-release)
locally on your computer. It works offline once a model is downloaded. Written in Rust with [GPUI](https://www.gpui.rs/).

**Built with PLaMo.**

## Features

- Translation between 53 languages. The default target is "Japanese ⇄ English (auto)".
- Paste to translate; text you type is translated after a short pause (the Auto switch in the toolbar turns this off). The translation streams in as it is generated, with the progress, speed, and remaining time in the status bar, and can be stopped at any time.
- Japanese output style (polite or plain), a reference context, and a glossary (`source = target`, one term per line).
- Long texts are split into paragraphs and translated in order, with earlier pairs passed as history to keep wording consistent.
  Output that falls into a repetition loop is detected, withdrawn, and translated again in smaller pieces. A passage that keeps looping is cut and marked in the translation.
- Built-in model manager: choose a model size and quantization (0.8B, 2B, 8B or 31B GGUF).
  - Each file shows the translation quality PFN publishes, with the difference from the unquantized model.
  - Downloads resume after an interruption and are verified with SHA-256.
  - Newly published files are picked up from Hugging Face without an app update.
- Runs on the CPU (AVX2 or AVX-512) or on a GPU through Vulkan. Linux source builds can also use ROCm. The device is selectable, and automatic selection picks the discrete GPU.
- English and Japanese UI. It follows the OS language by default and can be changed under Settings (the gear in the status bar). `PLAMO_LANG=ja` or `en` overrides both.
- Light and dark themes. The default follows the OS; Settings can fix either one.
- The source and the translation scroll together, and moving the caret in one selects the corresponding paragraph in the other (can be turned off in Settings).

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

Windows (x86_64) and Linux (x86_64) packages are on the [Releases](https://github.com/tanakh/plamo-translate-desktop/releases) page,
with their SHA-256 checksums in `SHA256SUMS.txt`. For the latest changes, build from source (below).

- **Windows** (10 or later): unzip and run `plamo-translate-desktop.exe`. Keep the DLLs next to it:
  llama.cpp and its CPU and Vulkan backends are loaded from there, and only from there.
  The Visual C++ runtime is included, so nothing else needs to be installed.
- **Linux**: extract and run `plamo-translate-desktop`. The shared libraries next to it are found through the executable's own directory (`RUNPATH=$ORIGIN`).
  - It is built on Ubuntu 24.04 and needs glibc 2.39 or later (Ubuntu 24.04, Debian 13, Fedora 40, or newer),
    plus `libgomp1`, `libxkbcommon-x11-0`, and `libxcb1`, which desktop installations usually have.
  - It needs a Wayland or X11 session with a Vulkan driver, which the UI uses for rendering and translation can use on the GPU, and a CJK font such as `fonts-noto-cjk`.
  - To add it to the application menu, run this in the extracted folder:

    ```sh
    mkdir -p ~/.local/share/applications ~/.local/share/icons/hicolor/256x256/apps
    sed "s|^Exec=.*|Exec=\"$PWD/plamo-translate-desktop\"|" plamo-translate-desktop.desktop \
      > ~/.local/share/applications/plamo-translate-desktop.desktop
    cp plamo-translate-desktop.png ~/.local/share/icons/hicolor/256x256/apps/
    ```

On both, translation uses a discrete GPU through Vulkan when one is available, and otherwise the fastest CPU code path the CPU supports (AVX2 or AVX-512).

## Browser extension

The packages include an extension for Chrome and Edge (`browser-extension/`) that translates text in the browser with the model selected in the app.

1. In the app, open Settings (the gear in the status bar) and choose **Connect** under Browser extension.
   This registers the bundled `plamo-translate-host` with Chrome and Edge (on Linux also Chromium and Brave).
2. Open `chrome://extensions` (or `edge://extensions`), turn on Developer mode, choose **Load unpacked**,
   and select the `browser-extension` folder (**Open extension folder** in the same Settings section).
3. Select text on a page, right-click, and choose **Translate with PLaMo**: the side panel shows the translation as it streams in.
   The toolbar button opens the panel to type or paste text.
4. To translate a whole page, right-click it and choose **Translate this page with PLaMo** (or **Translate page** in the panel).
   Paragraphs are translated in place, visible ones first, keeping links and formatting; a box at the bottom right shows the progress
   and can stop or restore the page. **Pages** in the panel's settings shows translations below the original instead.

The host uses the model and device selected in the app unless you choose others under **Model and device** in the panel
(any downloaded model; downloads stay in the app). It loads its own copy of the model, so with the app open as well the model is in memory twice.
It ends, freeing the memory, after 5 minutes without use or when you choose **Release** in the panel; the next translation starts it again.
To update the app in place, release it and close the app first (the host keeps its files open). If you move the app's folder, choose Connect again.
Browsers installed as Snap or Flatpak packages on Linux cannot start the host.

## Building from source

Requirements: Rust 1.92 or later, CMake, and a C/C++ compiler.

Linux (Ubuntu or Debian):

```sh
sudo apt install cmake libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev
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
- `crates/appdata`: settings, file locations, and logging, shared by the app and the browser host.
- `crates/app`: the GPUI application.
- `crates/host`: `plamo-translate-host`, the native messaging host for the browser extension.
- `extension/`: the browser extension (Manifest V3).
- `docs/design.md`: the design document.

Two command-line examples run the engine without the GUI:

```sh
cargo run -p engine --release --example translate -- --model path/to/model.gguf --to en "こんにちは"
cargo run -p engine --release --example download -- --list
```

A GGUF path passed as the first argument (or in `PLAMO_MODEL`) overrides the model selected in the app.

### Files and logs

Models (in `models/`), the cached model list (`catalog.json`), and a small log (`app.log`) are kept in:

- Windows: `%LOCALAPPDATA%\plamo-translate-desktop\data`
- Linux: `~/.local/share/plamo-translate-desktop` (`$XDG_DATA_HOME`)

The log records start-up, detected devices, model loads, downloads, deletions, settings changes, and where long texts were retried or cut, but never translated text.
A log larger than 1 MiB is moved to `app.log.1` at start-up.

Settings are in `settings.json`: `%APPDATA%\plamo-translate-desktop\config` on Windows, and `~/.config/plamo-translate-desktop` (`$XDG_CONFIG_HOME`) on Linux.

## License

The source code of this app is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. The PLaMo models are not part of this repository and are covered by the PLaMo Community License.

The packages include third-party software (Rust crates, llama.cpp, Lucide icons, and on Windows the Visual C++ runtime);
their licenses are in `THIRD-PARTY-NOTICES.txt` in each package, generated by `scripts/notices.sh`.
