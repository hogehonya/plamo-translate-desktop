# PLaMo Translate Desktop — Design

Status: agreed, 2026-11-05. Changes are made by editing this file.

## 1. Goal

A desktop translation app that runs [PLaMo 3 Translate](https://www.preferred.jp/ja/blog/tech/plamo-3-translate-weight-release)
entirely on the local machine. It is written in Rust with GPUI.

- Primary platform: Windows (x86_64, MSVC). Secondary: Linux (x86_64).
- Typical use: paste text, get the translation as you type, copy it out.
- The user picks the model (size and quantization) and the inference backend (CPU or a GPU) in the GUI.
  Models are downloaded on demand with visible progress.

Non-goals for now:

- Remote inference servers (OpenAI-compatible llama-server or vLLM). The backend trait leaves room for one,
  but it will not be built until it is needed.
- macOS, mobile, and the non-GGUF weight formats (BF16, FP8, W4A16, NVFP4, MLX). Those formats target
  vLLM, Transformers, or Apple silicon.

## 2. Model facts this design relies on

Source: the Hugging Face model cards `pfnet/plamo-3-translate-{0.8b,2b,8b,31b}` and their `-GGUF` variants.

- Architecture `Plamo3ForCausalLM`. Sliding-window attention (window 2048) with full attention on every
  8th layer, YaRN RoPE for full layers, and plain RoPE for local layers. llama.cpp supports it (`src/models/plamo3.cpp`).
- Maximum length during training: 32,768 tokens for input and output combined.
- GGUF files (`pfnet/plamo-3-translate-<size>-GGUF/plamo-3-translate-<size>-<quant>.gguf`):

  | Size | Q4_K_M | Q4_0 | Q3_K_M | Q2_K |
  |------|--------|------|--------|------|
  | 0.8B | 0.57 GB | 0.55 GB | 0.49 GB | 0.41 GB |
  | 2B   | 1.81 GB | 1.70 GB | 1.50 GB | 1.23 GB |
  | 8B   | 5.28 GB | —       | 4.31 GB | 3.45 GB |
  | 31B  | 19.53 GB | —      | 15.86 GB | 12.61 GB |

  Q4_K_M is the recommended default.
- Prompt format: PFML3 (see §5). Decoding is greedy. Generation stops at `<|plamo:tag|>` or `<|plamo:bos|>`.
- 53 target languages, identified by lowercase codes (`ja`, `en`, `zh-hans`, `zh-hant`, …). The official chat
  template also accepts `ja(easy)` (easy Japanese), which the model card does not document.
  We treat it as experimental.
- License: PLaMo Community License. Personal and non-commercial use is free. Commercial use is free up to
  ¥1B annual revenue, with prior registration. The app shows the license before the first download and records the user's acceptance.

## 3. Technology choices

| Concern | Choice | Notes |
|---|---|---|
| UI | GPUI via `gpui-kit` 0.9 | Ships a pinned Zed snapshot (`gpui-pre` 0.3.11) and components such as textarea, select, progress, notification, settings, and resizable panes. The crates.io `gpui` 0.2.2 has not been updated since 2025-10. |
| Inference | llama.cpp in-process via `llama-cpp-2` | The only engine that runs PLaMo 3 today and covers CPU, Vulkan, ROCm, and CUDA on Windows and Linux. Pinned to a llama-cpp-rs git revision for now: crates.io 0.1.161 vendors llama.cpp 0.6.0, which lacks the `plamo3` tokenizer. |
| GPU selection | `llama-cpp-2` feature `dynamic-backends` | ggml backends are separate DLL/.so modules loaded at runtime, so one build serves CPU and Vulkan users. |
| HTTP (downloads) | `ureq` on dedicated threads | Avoids running tokio alongside the GPUI executor. |
| Settings / paths | `serde_json`, `directories` | |
| i18n | `rust-i18n` (already used by gpui-kit), `sys-locale` | English and Japanese. The OS locale picks the default. |

Rust-native engines were considered and rejected for now. Candle and mistral.rs have no PLaMo 3
implementation and support only CUDA and Metal GPUs, so the target AMD GPU would fall back to CPU.
burn and cubecl reach Vulkan and ROCm, but we would have to write the model and the GGUF quantized kernels ourselves.
The backend trait (§6) keeps the door open.

## 4. Architecture

### 4.1 Workspace layout

```
crates/
  pfml/     PFML3 prompt builder, language table, validation. Pure Rust, no I/O.
  engine/   Backend trait, llama.cpp backend, model catalog, downloader, text chunker.
            No GUI dependency. examples/translate.rs is a headless CLI for testing.
  app/      GPUI application (binary `plamo-translate-desktop`).
docs/       This document.
```

### 4.2 Threads

```
GPUI main thread ──Command──▶ inference worker (one std::thread, owns backend/model/context)
      ▲  ◀──Event──────────────┘   Loaded | LoadFailed | Token | Done | Error
      └──◀── download threads ─── Progress | Finished | Failed
```

- The inference worker owns all llama.cpp state, so llama.cpp objects never cross threads.
  Commands are `LoadModel`, `Unload`, `Translate { job_id, request }`, and `Cancel { job_id }`.
- A new `Translate` command cancels the running job. This is what auto-translate needs.
  Cancellation is an `AtomicBool` checked after every generated token.
- The UI receives events through `async-channel` in a `cx.spawn` task and updates its entities.
  Translated text streams into the output pane token by token.
- Download threads throttle progress events to about 10 Hz.

## 5. Prompt construction (crate `pfml`)

PFML3 elements are delimited by special tokens:
`<|plamo:tag|>` starts an element, `<|plamo:key|>`/`<|plamo:val|>` add attributes, and `<|plamo:msg|>` starts the body.
The glossary uses `<|plamo:index|>` (source term) and `<|plamo:item|>` (target term). The order mirrors the
official `chat_template.jinja`:

```
<bos>
<|plamo:tag|>dataset<|plamo:msg|>translation
[<|plamo:tag|>context<|plamo:msg|>{context}]
[for each history pair:
   <|plamo:tag|>input[<|plamo:key|>lang<|plamo:val|>{src}]<|plamo:msg|>{source}
   <|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>{tgt}<|plamo:msg|>{translation}]
[<|plamo:tag|>glossary(<|plamo:index|>{term}<|plamo:item|>{translation})*]
<|plamo:tag|>input[<|plamo:key|>lang<|plamo:val|>{src}]<|plamo:msg|>{text}
<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>{tgt}[<|plamo:key|>form<|plamo:val|>{plain|polite}]<|plamo:msg|>
```

- `source_lang` is optional. When it is omitted, the model detects the language.
- `form` is allowed only when the target is `ja`.
- The builder returns a list of segments, `Special(&'static str)` or `Text(String)`, not one string.
  The llama.cpp backend tokenizes special segments with `parse_special = true` and user text with `parse_special = false`.
  Text that contains `<|plamo:…|>` therefore can never inject control tokens.
  Because llama.cpp splits on special tokens before running BPE, per-segment tokenization gives the same
  tokens as tokenizing the whole prompt.
- A string renderer (for debugging and any future HTTP backend) rejects text that contains `<|plamo:`,
  as the official template does.
- Validation mirrors the template: language codes, non-empty text, and glossary entries.

## 6. Inference engine (crate `engine`)

```rust
pub trait TranslationBackend {
    fn translate(
        &mut self,
        request: &TranslateRequest,
        sink: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<TranslateStats>;
}
```

The first implementation is `LlamaCppBackend`:

- At startup it loads the ggml backend modules next to the executable (`load_backends_from_path`),
  then lists devices with `list_llama_ggml_backend_devices()`.
- Model parameters: the selected device(s) (`with_devices`) and the number of GPU layers (all by default).
  - A fully offloaded model is read instead of memory-mapped. A mapping keeps the whole file in the process's memory
    as long as the model is loaded, although the weights are in VRAM.
    Measured with ROCm while translating: 0.8B uses 335 MiB instead of 964 MiB, and 2B 374 MiB instead of 2,142 MiB
    (214 MiB of either is the ROCm libraries). Loading takes the same 1–2 s.
  - On the CPU (or with partial offload) the file stays memory-mapped: those pages are the weights in use.
- Context: `n_ctx` defaults to 16384 and can be configured up to 32768.
- Decoding is greedy. It stops on the `<|plamo:tag|>` token, on any end-of-generation token (`<|plamo:bos|>` is the EOS),
  on cancellation, when the context is full, or when the output runs away (6.1).
- Output is decoded incrementally, so a UTF-8 sequence split across tokens is never shown half-finished.
- Each request starts with a fresh KV cache. Reusing the KV prefix across chunks is a later optimization.

### 6.1 Long text

`engine::document` handles long texts. The worker runs every request through it, and a short text is a single chunk.

- Splitting:
  - The text is split into paragraphs at blank lines.
  - Neighbouring paragraphs are packed into one chunk while they fit.
  - A paragraph larger than the chunk budget is split at sentence ends (`。！？．`, or `.!?` before whitespace), keeping closing quotes and brackets with their sentence.
    Line breaks inside a paragraph are ordinary whitespace here, so hard-wrapped prose keeps its sentences whole.
    Only a sentence that is still too long is split at its line breaks.
  - Leading whitespace and the separators between chunks are copied unchanged into the output.
- Budget: a chunk gets at most 1,024 tokens and history at most 4,096 (`DocumentOptions`).
  They are further limited to 1/6 and 1/3 of `n_ctx` minus the fixed parts (context and glossary, plus a margin of 64 tokens),
  so the output always has room for three times the chunk.
- Why chunks are small, although the model was trained on 32k tokens:
  - PLaMo 3 attends to only the last 2,048 tokens in 7 of every 8 layers. Small models decoding greedily lose their place in long outputs.
  - Measured with 0.8B Q4_K_M on the first 250 KB of an English book (Project Gutenberg #5000, into Japanese, ROCm):

    | Chunk / history (tokens) | Chunks | Looped | Output paragraphs (source: 1,158) | Time |
    |---|---|---|---|---|
    | about 5,400 / 10,900 (the earlier 1/5 and 1/2 of 32k) | 11 | 5 | 607 | 533 s |
    | 1,024 / 4,096 | 57 | 0 | 1,159 | 202 s |

    Large chunks also dropped about half of the paragraphs silently.
  - PFN's published scores are for sentence- and paragraph-level inputs, and the model card's examples cap the output at 512 tokens.
  - A typical paragraph still fits in one chunk: in that book, 4 of 6,942 paragraphs exceed about 700 words.
- History: chunks are translated in order. The most recent source/translation pairs that fit the history budget go into each prompt,
  and the oldest drop first, as the model card recommends.
- Runaway output (`engine::repetition`):
  - Greedy decoding can fall into a cycle and repeat a few tokens until the context is full.
    Whether it does depends on near-ties between tokens, so it differs between backends, GPUs, and thread counts.
  - The detector tracks, for each period up to 128 tokens, how long the output has repeated with that period (constant time per token and period).
    A loop is a span of at least 64 tokens that repeats its cycle at least three times.
    The threshold rises to twice the longest such span in the prompt, so input that repeats legitimately (rulers, refrains) can be translated.
  - The output of a chunk is also limited to 3 × its tokens + 128.
  - A chunk of at least 200 tokens whose translation ends at under a quarter of its tokens counts as incomplete:
    large chunks often ended early and silently skipped the rest. It is retried like a loop.
  - Recovery:
    1. A chunk whose output runs away (or is incomplete) is withdrawn (`Output::Truncate`, `Event::Rewind`) and translated again sentence by sentence.
    2. A sentence that runs away again is retried with llama.cpp's DRY sampler (multiplier 0.8, base 1.75, allowed length 2), which penalizes the output repeating itself.
       DRY is not used otherwise, because it also discourages legitimate repetition.
    3. If even that loops, the output is cut after the cycle's first occurrence.
  - Output that ran away never enters the history, so one loop cannot seed the next.
  - The app logs retries and cuts to `app.log` (without text). The status bar reports cuts.
- The worker reports `Chunk { index, count }`, and the status bar shows progress such as `(2/5)`.

### 6.2 Context and glossary

- The toolbar button "Context & glossary" opens two text areas below the panes.
- Context is free text for the whole request. It is not persisted, because it is usually per document.
- The glossary is written one term per line as `source = target`. A tab, `=>`, or `→` also works, and `#` starts a comment.
  It is saved in settings, and the button shows the number of terms.
- Editing either text area re-translates after the usual debounce.

## 7. Model management

- The catalog (`engine::models`) starts with a built-in list (`BUILTIN`) of the 14 GGUF files published at build time.
  Each entry has its repo, pinned commit, file name, size, and SHA-256, and the download URL uses the pinned commit.
- Catalog discovery (`engine::discovery`) keeps the app current without a new release:
  - At start-up it searches Hugging Face for `pfnet/<family>-<size>-GGUF` repositories of supported families (today `plamo-3-translate`).
  - It pins each repository to its current commit and reads each `<family>-<size>-<quant>.gguf` with its size and SHA-256 from the Git LFS metadata.
  - New sizes or quantizations are added, and newer revisions of known files replace the old ones.
  - The result is cached in `catalog.json` next to the models, so offline starts know it. "Check again" in the dialog repeats the search.
  - A new model generation with a different prompt format still needs an app update, so unknown families are ignored.
- Quality data (`engine::models::QUALITY`) holds the scores PFN publishes for every file and for the BF16 reference:
  PFTB, SaaS, WMT24++ COMET-22, and FLORES+ chrF++ (Ja↔ and En↔14 languages).
  - The dialog groups files by size. Each size has a header with a description and its BF16 scores.
  - Each file shows Ja↔En (PFTB) and multilingual (FLORES+ Ja↔) scores with the difference from BF16.
    Drops beyond 0.003 / 0.3 are amber, and beyond 0.008 / 0.8 red.
  - Files without published figures (discovered later) say so.
- Download (`engine::download`): `GET https://huggingface.co/<repo>/resolve/<revision>/<file>` into `<file>.part`. It uses `ureq` on its own thread.
  - Hugging Face redirects to a CDN that honours `Range`, so an interrupted or cancelled download resumes from the partial file.
  - The existing part is hashed first, so the digest always covers the whole file.
  - The file is renamed into place only after the SHA-256 matches, so a final file of the right size counts as downloaded.
  - Progress reports bytes and two rates, throttled to 10 Hz.
    The recent rate (exponentially smoothed) is what the UI shows.
    The average since the transfer started is used for the ETA once 3 s have passed, so the estimate does not jump around.
- Storage:
  - Windows: `%LOCALAPPDATA%\plamo-translate-desktop\data\models`
  - Linux: `$XDG_DATA_HOME/plamo-translate-desktop/models` (usually `~/.local/share/...`)
  - Both come from `directories::ProjectDirs::data_local_dir()`.
  - A GGUF path given on the command line or in `PLAMO_MODEL` overrides the selection (for development).
- The Models dialog opens from the model button in the title bar. Each row shows its state and actions:
  - Not downloaded: Download.
  - Downloading: a progress bar and Cancel.
  - Paused: Resume and Delete.
  - Downloaded: Use (Unload for the active model) and Delete.
  - Deleting asks for confirmation.
  - Deleting the active model unloads it first. The file is removed after the worker reports `Unloaded`, because Windows cannot delete a memory-mapped file.
  - Downloads stay disabled until the user accepts the PLaMo Community License. The acceptance is saved in settings.
    The license is shown as a modal at first start, with "I agree" and "Later".
    Until it is accepted, the Models dialog shows a one-line notice with "Review license".
    Afterwards it shows only "Built with PLaMo · PLaMo Community License".
- Startup picks the first available of: the saved selection if downloaded, 2B Q4_K_M if downloaded, any downloaded model.
  With no model, the Models dialog opens and the first finished download becomes active automatically.
- The model marked "Recommended" is 2B Q4_K_M until M3 provides device memory.
  `engine::models::recommend` then picks the largest Q4_K_M up to 8B whose file is at most 2/3 of the available memory.

## 8. Backend selection

- After llama.cpp starts, the worker reports the device list from ggml, for example:
  - `CPU`
  - `Vulkan0 — AMD Radeon RX 9070 XT (16 GB)`
  - `Vulkan1 — AMD Radeon(TM) Graphics`
- The first model load waits for this list.
- The device setting is saved as one of three values:
  - `auto` (the default) picks the discrete GPU with the most free memory, or the CPU if there is none. Integrated GPUs are never picked automatically.
  - `cpu`.
  - A device name such as `Vulkan0`. If that device disappears, the app falls back to `auto`.
- The model is fully offloaded to the selected GPU. Partial offload (GPU layers) is a possible later setting.
- The context length is fixed at 32768, the maximum the model was trained with, and is not a setting.
  - The KV cache for sliding-window layers is limited to the window (`swa_full = false`). This is safe because every request starts from an empty cache.
  - At 32k this needs 0.05 / 0.3 / 0.4 / 1.6 GiB of KV cache for 0.8B / 2B / 8B / 31B. With a full SWA cache it would be 0.26 / 1.5 / 2.0 / 8.2 GiB.
  - Translation speed does not depend on the context length (measured: 144–148 tok/s for 2B at 4k, 16k, and 32k).
- CPU threads are set automatically. Prompt processing uses the physical cores, and generation uses at most 8,
  because it is memory-bound.
- Memory check: the app estimates the file size × 1.2 + 384 MiB.
  If that exceeds the free memory of the selected device, the dialog shows a warning; the model still loads.
- The "Recommended" model is the largest Q4_K_M up to 8B that fits the free memory of the selected device
  (system memory for the CPU).
- Changing the device reloads the model. The title bar shows the model and the device, for example `2B · Q4_K_M · Vulkan0`.

## 9. UI

```
┌───────────────────────────────────────────────────────────────────┐
│ [Auto detect ▾] ⇄ [Japanese ▾]  Style [Polite ▾]  ● 2B Q4_K_M · Vulkan0 ▾ │
├────────────────────────────────┬──────────────────────────────────┤
│ Source                         │ Translation (streaming)          │
│                                │                                  │
│ 1,234 chars          [Clear]   │                          [Copy]  │
├────────────────────────────────┴──────────────────────────────────┤
│ ▸ Context / Glossary                                              │
├───────────────────────────────────────────────────────────────────┤
│ Ready · 42.1 tok/s                         [Translate  Ctrl+Enter] │
└───────────────────────────────────────────────────────────────────┘
```

- Auto-translate is on by default.
  - A paste (a large input change) translates immediately. Typing is debounced by about 600 ms.
  - Editing the input cancels the running job and starts a new one.
  - Results are cached by (input, settings), so swapping languages back and forth costs nothing.
- The source pane has focus at startup. `Ctrl+Shift+C` copies the translation, and `Ctrl+Enter` translates immediately.
- Languages:
  - The default source is "Detect language".
  - The default target is "Japanese ⇄ English (auto)": text with kana goes to English, and everything else goes to Japanese.
    Kanji alone does not count, because it may be Chinese.
  - Swapping exchanges explicit languages and moves the translation into the source pane.
- The title bar is drawn by the app (gpui-kit `TitleBar`) on every platform.
  Wayland compositors such as WSLg and GNOME do not draw server-side decorations.
  - The title bar holds only the title. On Linux, a press on the title bar arms a window move, and any pointer motion before release
    hands the pointer to the compositor, so buttons there can lose clicks.
  - The Models button (model and device, or "Choose a model…" in primary style when none is loaded) and the Settings gear sit at the right end of the status bar.
- Translation options:
  - Style (plain or polite) is shown only for a Japanese target.
  - Context is a separate textarea.
  - The glossary is a table that can be imported and exported as TSV.
- The Models & Backend dialog lists the catalog with status and Download, Cancel, Use, and Delete actions,
  a progress bar showing rate and ETA, and the backend and device settings.
- While a translation runs, the Translate button becomes Stop. Stopping keeps the partial output, and that partial output is never cached.
- Streaming keeps the output pane's scroll position, so a reader can scroll through a long translation while it grows.
  A new translation starts at the top.
- Japanese IME input must work on both platforms. This is checked in M0.
- Theme: Settings → Theme (System, Light, or Dark). System follows the OS, including changes while the app runs.
  The light and dark themes come from gpui-kit.
- UI strings live in `crates/app/locales/app.yml`. The language is the first that applies of:
  1. `PLAMO_LANG` (a development override).
  2. Settings → Display language (System, English, or 日本語).
  3. The OS locale.
- The change applies immediately. Components that baked strings in when they were created (select items, placeholders) are rebuilt,
  and everything else is translated at render time.

Future idea: a global hotkey that translates the clipboard (with the `global-hotkey` crate).

## 10. Settings

The app saves `settings.json` in the platform config directory (M2 stores the selected model and the license acceptance). The full set is UI language, theme, the selected model,
the backend (device, GPU layers, `n_ctx`, threads), the last language pair, the translation options, glossaries,
the auto-translate flag, the window geometry, and the license acceptance record.

## 11. Build, test, and distribution

- Development and day-to-day testing happen in WSL2 (Linux build, WSLg).
  WSLg has only a software Vulkan driver (lavapipe), so Vulkan inference cannot be tested there.
  ROCm, however, supports WSL2. The `rocm` feature (built with `scripts/rocm.sh`) runs the model on the RX 9070 XT from WSL2.
  For 2B Q4_K_M, generation runs at about 120–150 tok/s, against about 20 tok/s on the CPU.
  ggml also lists the integrated GPU, but only the discrete GPU's architecture is compiled; automatic selection skips integrated GPUs anyway.
- Local Windows builds and test runs use `/mnt/e/` (a ReFS dev drive), which is faster than `C:`.
- Windows binaries are built natively on GitHub Actions `windows-2025` (MSVC, Windows SDK, and CMake are preinstalled;
  the Vulkan SDK is installed by the workflow), not cross-compiled. Cross-compiling from Linux fails:
  - The `gpui-pre-windows` build script compiles its HLSL shaders with `fxc.exe` only when the build host is Windows.
  - Release builds then `include!` the generated shader bytes, so a Linux-hosted release build cannot compile.
- CI runs on every push and pull request: rustfmt and clippy, and the tests, as two parallel Linux jobs with the latest stable Rust.
  It builds no packages, so the slow parts (the Windows build, llama.cpp's Vulkan shaders, fat LTO) are not on every push.
- Build configurations:
  - Windows: features `engine/vulkan` and `engine/dynamic-backends`. llama.cpp is built as DLLs
    (`llama.dll`, `ggml*.dll`), and the backend modules sit next to the exe.
    - A machine without a working Vulkan driver simply does not list a Vulkan device.
    - Five CPU variants are shipped: `x64`, `haswell` (AVX2), `skylakex` and `icelake` (AVX-512), and `alderlake` (AVX-VNNI).
      ggml loads the best one the CPU supports.
    - Measured on a Ryzen 9 9950X with 2B Q4_K_M: generation is memory-bound (about 19.5 tok/s for every variant from AVX2 up).
      AVX-512 processes prompts about 25% faster than AVX2, and `x64` is roughly 25× slower at prompts.
    - A `zen4` variant (AVX-512 BF16, which MSVC cannot build) would add nothing for quantized models.
  - Modules are loaded only from the executable's folder.
    - The build output directory is searched only when the executable runs from that cargo target directory (`cargo run`).
    - If nothing loads, start-up fails instead of letting `llama_backend_init` search ggml's default paths, which include the current directory.
  - Linux: the same features as Windows.
    - The executables get `RUNPATH=$ORIGIN` from build scripts, so they find `libllama.so.0`, `libggml.so.0`, and `libggml-base.so.0` next to themselves.
      This is the executable's directory, never the current directory.
    - llama-cpp-sys-2 copies the versioned `.so` symlinks into the target directory without their targets, so packaging copies the files they point to (`cp -L`).
    - The package ships the Vulkan backend and the same five CPU variants as Windows.
      On native Linux with a Vulkan driver, the discrete GPU is used.
      WSL2 has no GPU Vulkan driver, so there the app uses the CPU, or the GPU through ROCm (`scripts/rocm.sh`).
  - Packages use the `dist` profile (fat LTO, one codegen unit, stripped).
    The Linux app binary is 34.5 MB this way, against 73.7 MB for a release build with llama.cpp linked statically.
    Lints and tests run on Linux only, so Windows code is compiled only by the Release workflow.
  - The `translate` CLI is built and used for a smoke test (`--list-devices` with the package's own libraries), but it is not shipped.
- The Release workflow builds the packages, with the latest stable Rust:
  - Pushing a tag `vX.Y.Z` publishes a GitHub release with generated notes. The tag must match the workspace version.
  - A manual run (**Run workflow**, from any branch) builds the same packages as workflow artifacts without publishing,
    versioned `X.Y.Z-dev.<commit>`.
  - Windows: a portable zip with the exe, the DLLs and backend modules, the READMEs, and the licenses.
  - Linux: a tar.gz with the same contents and the icon (an AppImage comes later).
  - Build caches are keyed on the rustc version. A tag can restore only its own caches and those of the default branch,
    so a manual run on `main` before tagging makes the release build warm.
- Windows-specific behaviour (DirectX rendering, IME, Vulkan performance on real hardware) is checked by running Release artifacts on the Windows host.
  Once the app is feature-complete, the repo is also built and run locally on Windows.
- Icon: a white block "p" on a blue rounded tile, after the PLaMo logo. `scripts/gen_icons.py` produces `assets/icon/`
  (SVG, PNGs from 16 to 512 px, and a multi-size `.ico`) using only the Python standard library.
  On Windows, `crates/app/build.rs` embeds the icon as resource ID 1, which GPUI loads for its windows.
  On Linux, the window `app_id` is `plamo-translate-desktop`, to match a desktop entry (M5).
- Attribution: "Built with PLaMo" appears in the status bar, the Models dialog, and the README (PLaMo Community License, Article 4).
- Linux build dependencies: `libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libx11-xcb-dev libssl-dev libzstd-dev libvulkan1`, plus a Vulkan driver and CMake.
  CJK text needs a CJK font at runtime (for example `fonts-noto-cjk`). A minimal WSL image ships none.

## 12. Milestones

- **M0 — feasibility**
  - `pfml` with unit tests.
  - Headless CLI that translates with `llama-cpp-2` (Linux CPU; Windows Vulkan via CI artifact).
  - `dynamic-backends` lists the AMD GPU on Windows.
  - A gpui-kit window with a working textarea and IME on both platforms.
  - CI for Windows and Linux.
- **M1 — MVP**: two-pane UI, language selection, streaming translation, cancellation, and auto-translate. The model path is fixed.
- **M2 — model management**: catalog, download (progress, resume, verify, delete), and first-run onboarding.
- **M3 — backend selection**: device list, GPU layers, `n_ctx`, threads, and the memory estimate.
- **M4 — translation options**: style, context, glossary, and long text with history.
- **M5 — distribution**:
  - Release workflow (done).
  - Icon (done).
  - Third-party license notices for Lucide icons (ISC/MIT), llama.cpp (MIT), GPUI (Apache-2.0), and the other Rust crates.
  - Linux desktop entry.
  - Settings polish.
  - The local Windows build check.

## 13. Risks

| Risk | Mitigation |
|---|---|
| The `plamo3` implementation in the vendored llama.cpp differs from the reference | Compare CLI output with the model card examples in M0. Done: the outputs match on CPU |
| `llama-cpp-2` releases lag behind llama.cpp | Pin a git revision when needed, and move back to crates.io once a release catches up |
| `dynamic-backends` with Vulkan does not build on Windows MSVC | Fall back to a statically linked Vulkan build, or ship two exe variants |
| IME or text input gaps in gpui-kit's textarea | Check in M0; patch or contribute upstream if needed |
| gpui-pre snapshots break the API often | Pin exact versions and update deliberately |
| Long llama.cpp + Vulkan shader builds in CI | Build packages only in the Release workflow; cache the target dir (`Swatinem/rust-cache` on Linux, `actions/cache` on Windows) |
