# PLaMo Translate Desktop — Design

Status: describes the code after v0.1.0 (released 2026-11-05), towards v0.2.0. Update it together with the code.

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
  engine/   Backend trait, llama.cpp backend, model catalog and discovery, downloader,
            long-text splitting and loop detection. No GUI dependency.
            examples/translate.rs and examples/download.rs are headless CLIs for testing.
  appdata/  Settings, file locations, logging, device selection: shared by app and host.
  app/      GPUI application (binary `plamo-translate-desktop`).
  host/     Native messaging host for the browser extension (binary `plamo-translate-host`).
extension/  Browser extension (Manifest V3) for Chrome and Edge.
assets/     Icon (scripts/gen_icons.py) and the Linux desktop entry.
licenses/   License texts appended to THIRD-PARTY-NOTICES.txt (scripts/notices.sh).
scripts/    Icon generator, notices generator, ROCm build environment.
docs/       This document.
```

### 4.2 Threads

```
GPUI main thread ──Command──▶ inference worker (one std::thread, owns runtime/model/context)
      ▲  ◀──Event──────────────┘
      └──◀── download threads ─── Progress | Finished | Failed
```

- The inference worker (`engine::worker`) owns all llama.cpp state, so llama.cpp objects never cross threads.
  Commands are `Load`, `Unload`, and `Translate { job, options, document, text }`.
- Events: `RuntimeFailed`, `Devices`, `LoadProgress`, `Loaded`, `LoadFailed`, `Unloaded`,
  and per job `Chunk`, `Token`, `Rewind`, `Retry`, `Cut`, `Finished`, `Failed`.
  A panic of the worker thread is reported as `RuntimeFailed`.
- Jobs supersede each other: a shared `AtomicU64` holds the newest job id, so submitting a translation
  cancels the running one (checked before every generated token) and stale queued jobs are skipped.
  This is what auto-translate needs.
- Each load ends in exactly one `Loaded` or `LoadFailed`. Load events carry no id, so the app counts outstanding loads
  and acts only on the last one.
- The UI receives events through `async-channel` and applies them in batches of one 50 ms frame,
  so a fast GPU does not stall the UI thread.
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
        prompt: &pfml::Prompt<'_>,
        generation: &Generation, // output length limit, DRY on or off
        sink: &mut dyn FnMut(&str),
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TranslateStats>;
}
```

The implementation is `engine::llama::Session` (one llama.cpp context of a loaded `Model`):

- At startup `Runtime::init` loads the ggml backend modules next to the executable (`load_backends_from_path`),
  then lists devices with `list_llama_ggml_backend_devices()`.
- Model parameters: the selected device(s) (`with_devices`) and the number of GPU layers (all by default).
  - A fully offloaded model is read instead of memory-mapped. A mapping keeps the whole file in the process's memory
    as long as the model is loaded, although the weights are in VRAM.
    Measured with ROCm while translating: 0.8B uses 335 MiB instead of 964 MiB, and 2B 374 MiB instead of 2,142 MiB
    (214 MiB of either is the ROCm libraries). Loading takes the same 1–2 s.
  - On the CPU (or with partial offload) the file stays memory-mapped: those pages are the weights in use.
- Context: `n_ctx` is fixed at 32768 (see §8).
- Decoding is greedy. It stops on the `<|plamo:tag|>` token, on any end-of-generation token (`<|plamo:bos|>` is the EOS),
  on cancellation, when the context is full, or when the output runs away (6.1).
- Output is decoded incrementally, so a UTF-8 sequence split across tokens is never shown half-finished.
- Each request starts with a fresh KV cache. Reusing the KV prefix across chunks is a later optimization.

### 6.1 Long text

`engine::document` handles long texts. The worker runs every request through it, and a short text is a single chunk.

- Splitting:
  - The text is split into paragraphs at blank lines.
  - Neighbouring paragraphs are packed into one chunk while they fit.
  - A paragraph larger than the chunk budget is split at the most natural boundary that makes it fit, in this order:
    1. Line breaks after the end of a sentence. Japanese prose usually separates paragraphs with a single newline (and indents with U+3000).
    2. Sentence ends (`。！？．`, or `.!?` before whitespace), keeping closing quotes and brackets with their sentence.
       Other line breaks are ordinary whitespace here, so hard-wrapped prose keeps its sentences whole.
    3. Any line break, for a sentence that is still too long.
    4. After clause punctuation (`、，,;；:：`) or at whitespace.
    5. By character count, the last resort for text without any of the above (minified data, for example),
       so that no chunk exceeds the budget.
  - Pieces are packed on summed token counts; the packed text is measured exactly only near the limit.
  - Leading whitespace and the separators between chunks are copied unchanged into the output.
- Budget: a chunk gets at most 1,024 tokens and history at most 4,096 (`DocumentOptions`).
  Settings → Chunk size and History change these (256–4,096 and none–8,192 tokens; other values can be set in `settings.json`).
  They are part of the translation cache key.
  They are further limited to 1/6 and 1/3 of `n_ctx` minus the fixed parts (context and glossary, plus a margin of 64 tokens),
  which leaves the output room for about three times the chunk.
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
    1. A chunk whose output runs away (or is incomplete) is withdrawn (`Output::Truncate`, `Event::Rewind`) and translated again in pieces of up to 256 tokens,
       split at the same boundaries as chunks.
    2. A piece that runs away again is translated sentence by sentence.
       (Retrying a chunk sentence by sentence straight away needed dozens of prompts, each with the whole history: minutes on the CPU.)
    3. A sentence that runs away again is retried with llama.cpp's DRY sampler (multiplier 0.8, base 1.75, allowed length 2), which penalizes the output repeating itself.
       DRY is not used otherwise, because it also discourages legitimate repetition.
       A level that would not split the text further is skipped: the same input with the same history only repeats the same output.
    4. If even that loops, the output is cut after the cycle's first occurrence.
       When the prompt contains the same cycle (text copied verbatim, such as repeated lines of code), as many repeats as the prompt has are kept:
       the model tends to copy such text and not stop, and only the repeats beyond the source's are the loop.
  - Output that ran away never enters the history, so one loop cannot seed the next.
  - A cut is marked in the translation (`DocumentOptions::cut_marker`, in the UI language), so it shows when the text is copied.
    The engine emits the marker itself, keeping the output offsets it reports in step with the text.
  - The app logs retries and cuts to `app.log` (without text, with the source character position of a cut).
    The status bar reports cuts; clicking it selects the next cut passage in both panes.
- The worker reports `Chunk { index, count, source }`. The status bar shows the chunk (`(2/5)`), the tokens generated,
  the speed, and the remaining time. The estimate counts progress in source bytes, with the chunk in progress weighted by its output
  against the output per source byte of the finished chunks; it appears once a chunk is done.

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
- Cached entries are dropped unless they look like what discovery produces (repository and file name patterns, hex digests),
  because the file name becomes a path that is written and deleted.
- Download (`engine::download`): `GET https://huggingface.co/<repo>/resolve/<revision>/<file>` into `<file>.<digest>.part`
  (named after the expected SHA-256, so a partial file of an older revision is never resumed). It uses `ureq` on its own thread,
  with a 30 s read timeout so a stalled connection cannot block cancellation.
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
- The Models dialog opens from the model button in the status bar. Each row shows its state and actions:
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
- The model marked "Recommended" comes from `engine::models::recommend`: the largest Q4_K_M up to 8B whose file is
  at most 2/3 of the free memory of the selected device (system memory for the CPU).

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
- Changing the device reloads the model. The status bar shows the model and the device, for example `2B · Q4_K_M · Vulkan0`.

## 9. UI

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ PLaMo Translate                                                     _  □  × │
├──────────────────────────────────────────────────────────────────────────────┤
│ [Detect ▾]  ⇄  [Japanese ⇄ English ▾]  [Style ▾]   Context & glossary  (o) Auto [Translate] │
├───────────────────────────────────────┬──────────────────────────────────────┤
│ Source                                │ Translation (streaming)              │
│ 1,234 chars                  [Clear]  │                              [Copy]  │
├───────────────────────────────────────┴──────────────────────────────────────┤
│ Translating… (3/12) · 4,210 tokens · 250.3 tok/s · about 2 min left   [2B · Q4_K_M · Vulkan1] ⚙ Built with PLaMo │
└──────────────────────────────────────────────────────────────────────────────┘
```

- Auto-translate is on by default; the Auto switch in the toolbar turns it off.
  - A paste translates immediately. Typing is debounced by 500 ms. With Auto off, only Translate (or `Ctrl+Enter`) translates.
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
  - The glossary is a text area (see §6.2).
- The Models & Backend dialog lists the catalog with status and Download, Cancel, Use, and Delete actions,
  a progress bar showing rate and ETA, and the backend and device settings.
- While a translation runs, the Translate button becomes Stop. Stopping keeps the partial output; it is marked partial,
  so it is never cached and the same request translates again.
- Streamed text is applied as edits at the end of the output pane. Replacing the whole value every frame laid out the entire translation
  again and stalled the UI on long texts.
- The status bar ends a translation with the time taken, the tokens, and the speed. When passages were cut, clicking the message selects the next one in both panes.
- Settings (the gear): display language, theme, chunk size, history, and synchronized scrolling.
- Streaming keeps the output pane's scroll position, so a reader can scroll through a long translation while it grows.
  A new translation starts at the top.
- Japanese IME input goes through gpui-kit's textarea.
- Synchronized scrolling (Settings, on by default; `crates/app/src/align.rs`):
  - Chunk starts are known pairs of source and output positions. Within a chunk, paragraphs (blank-line separated, or lines in text without blank lines)
    are paired when both sides have the same number; otherwise the chunk is one segment. Positions inside a segment map proportionally.
    The chunk still being translated pairs the paragraphs translated so far, in order; its untranslated paragraphs have no counterpart.
  - Scrolling a pane scrolls the other so the counterpart of its first visible line is at the top.
    Moving the caret selects the counterpart paragraph in the other pane, level with the caret; each pane keeps its selection visible while the other has focus.
  - A pane that was scrolled to follow ignores its own changes for 150 ms, so the two do not chase each other.
  - It applies while the source is the text that was translated; cached results keep their chunks for it.
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

The app saves `settings.json` in the platform config directory (`crates/app/src/settings.rs`):
the selected model, the license acceptance, the device, the glossary, the UI language, the theme,
the chunk and history sizes, and synchronized scrolling.
The language pair, style, context, auto-translate flag, and window geometry are not saved.
A file that does not parse is kept as `settings.json.invalid` instead of being overwritten.

## 11. Build, test, and distribution

- Development and day-to-day testing happen in WSL2 (Linux build, WSLg).
  WSLg has only a software Vulkan driver (lavapipe), so Vulkan inference cannot be tested there.
  ROCm, however, supports WSL2. The `rocm` feature (built with `scripts/rocm.sh`) runs the model on the RX 9070 XT from WSL2.
  For 2B Q4_K_M, generation runs at about 120–150 tok/s, against about 20 tok/s on the CPU.
  ggml also lists the integrated GPU, but only the discrete GPU's architecture is compiled; automatic selection skips integrated GPUs anyway.
- Windows binaries are built natively on GitHub Actions `windows-2025` (MSVC, Windows SDK, and CMake are preinstalled;
  the Vulkan SDK is installed by the workflow), not cross-compiled. Cross-compiling from Linux fails:
  - The `gpui-pre-windows` build script compiles its HLSL shaders with `fxc.exe` only when the build host is Windows.
  - Release builds then `include!` the generated shader bytes, so a Linux-hosted release build cannot compile.
- CI runs on every push and pull request: rustfmt and clippy, and the tests, as two parallel Linux jobs with the latest stable Rust.
  It builds no packages, so the slow parts (the Windows build, llama.cpp's Vulkan shaders, fat LTO) are not on every push.
- Build configurations:
  - Windows: features `engine/vulkan` and `engine/dynamic-backends`. llama.cpp is built as DLLs
    (`llama.dll`, `ggml*.dll`), and the backend modules sit next to the exe.
    - The exe and the DLLs import the Visual C++ runtime and OpenMP (`vcruntime140*.dll`, `msvcp140.dll`, `vcomp140.dll`).
      The package ships them from the toolset that built it, so the Redistributable need not be installed.
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
    - Built on Ubuntu 24.04, so it needs glibc 2.39 or later. It also needs `libgomp1`, `libxkbcommon-x11-0`, `libxcb1`,
      a Vulkan driver (the UI renders with Vulkan), and a CJK font (for example `fonts-noto-cjk`; a minimal WSL image has none).
  - Packages use the `dist` profile (fat LTO, one codegen unit, stripped).
    The Linux app binary is 34.5 MB this way, against 73.7 MB for a release build with llama.cpp linked statically.
    Lints and tests run on Linux only, so Windows code is compiled only by the Release workflow.
  - The `translate` CLI is built and used for a smoke test (`--list-devices` with the package's own libraries), but it is not shipped.
- The Release workflow builds the packages, with the latest stable Rust:
  - Pushing a tag `vX.Y.Z` publishes a GitHub release with generated notes and `SHA256SUMS.txt`. The tag must match the workspace version.
  - A manual run (**Run workflow**, from any branch) builds the same packages as workflow artifacts without publishing,
    versioned `X.Y.Z-dev.<commit>`, plus the Windows `translate.exe` for testing the engine on Windows GPUs.
  - Windows: a portable zip with the exe, the DLLs and backend modules, the Visual C++ runtime, the READMEs, the licenses,
    and `THIRD-PARTY-NOTICES.txt`.
  - Linux: a tar.gz with the same contents (no Visual C++ runtime), a desktop entry, and its icon.
  - `THIRD-PARTY-NOTICES.txt` is generated by `scripts/notices.sh`: cargo-about (`about.toml`, `about.hbs`) for the Rust crates,
    then `licenses/` for llama.cpp/ggml, the Lucide icons, and (Windows) the Visual C++ runtime.
  - Build caches are keyed on the rustc version. A tag can restore only its own caches and those of the default branch,
    so a manual run on `main` before tagging makes the release build warm.
- Windows-specific behaviour (DirectX rendering, IME, Vulkan performance on real hardware) is checked by running Release artifacts on the Windows host.
- Icon: a white block "p" on a blue rounded tile, after the PLaMo logo. `scripts/gen_icons.py` produces `assets/icon/`
  (SVG, a 256 px PNG, and a multi-size `.ico`) using only the Python standard library.
  On Windows, `crates/app/build.rs` embeds the icon as resource ID 1, which GPUI loads for its windows.
  On Linux, the window `app_id` is `plamo-translate-desktop`, matching `assets/plamo-translate-desktop.desktop`.
- Attribution: "Built with PLaMo" appears in the status bar, the Models dialog, and the README (PLaMo Community License, Article 4).
- Linux build dependencies: CMake and `libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev`,
  plus `libvulkan-dev glslc spirv-headers` for the Vulkan backend.

## 12. Browser extension

The extension (`extension/`) translates in the browser with the desktop app's model, through Chrome's native messaging.

- Components:
  - `plamo-translate-host` ships next to the app and shares its DLLs/shared libraries, models, and settings, so the package grows by about 1 MB.
    It has no GPUI dependency. It is a console program on Windows too, since Chrome starts hosts through `cmd.exe` with the window hidden.
  - The extension: context menu items for selected text ("Translate with PLaMo", shown in the side panel) and for the whole page,
    the side panel, and `page.js` for page translation.
  - The background service worker (`background.js`) holds the one native port and shares it: panel translations start at once
    (interrupting a page batch, whose unfinished paragraphs go back to the queue), and page paragraphs go to the host in batches
    of up to 10 paragraphs or 2,000 characters while nothing else runs. Chrome keeps the worker alive while the port is open.
    The port closes after 5 minutes without work, or on Release in the panel.
- Registration (Settings → Browser extension → Connect, `crates/app/src/browser.rs`) writes a host manifest named
  `com.github.tanakh.plamo_translate` that points to the host and allows only the extension's ID:
  - Windows: the manifest in the data directory, referenced from `HKCU\Software\{Google\Chrome,Microsoft\Edge}\NativeMessagingHosts`.
  - Linux: a copy in `NativeMessagingHosts/` of each browser that has a configuration directory (Chrome, Chromium, Edge, Brave), or Chrome's if none has.
  - The settings show whether the registration points to this copy of the app, so a moved app can be connected again.
- The extension's ID (`bcagbgackmnblhlofmpcmjfkfhhogpmh`) follows from the public key in its manifest, so the unpacked extension always has it.
  A Chrome Web Store release would get another ID, which must be added to `EXTENSION_IDS`.
- Protocol (`crates/host/src/protocol.rs`): length-prefixed JSON both ways.
  - Requests: `translate { id, text, target (code or auto), source, style }`, `translate_segments { id, segments, … }`, `cancel`, `status`, `options`,
    and `configure { model, device }` (the extension's own choices; absent fields follow the app).
  - Responses: `status` (starting, loading, ready, no_model, error), `options` (downloaded models, devices, and the current choices),
    `started`, `progress`, `text`, `segment { index, text }`, `rewind` (a UTF-16 length, as JavaScript counts),
    `done { tokens, cut, stopped }`, and `error` with a code the extension localizes.
  - The host reads the app's settings before each request and reloads when the model or device changed.
    It uses the app's glossary, chunk and history sizes, and cut marker.
  - The panel's model and device choices are kept by the extension and sent in `configure` when it connects;
    the host loads nothing before the first request, so the app's model is not loaded only to be replaced.
    Writing them into the app's settings would have been undone by the app's next save.
  - A new request supersedes the running one, as in the app; the superseded one ends with `done { stopped }`.
- Closing the port ends the host and frees the model's memory and the app's files (which Windows keeps locked while the host runs).
  The next request starts it again.
- Page translation (`page.js`, injected on request with `activeTab`, or from the panel with the optional all-sites permission):
  - The page is cut into runs: consecutive text and inline elements within one block. Code, line breaks, and text marked `translate="no"`
    or `notranslate` stay as they are inside a run; `pre`, form controls, scripts, and similar are skipped, as are Japanese runs for a Japanese target.
  - A run is sent as simple HTML whose elements carry an index instead of their attributes (`see the <a i="2">guide</a>`).
    PLaMo 3 Translate keeps such tags in place and reorders them with the sentence (checked with 0.8B and 2B, HTML and Markdown).
  - The translation is parsed inertly (`DOMParser`) and rebuilt from text nodes and clones of the run's own elements, looked up by index,
    so links keep their targets and nothing the model writes can add attributes, elements, or scripts. Unknown or repeated indices keep only their text.
  - The runs in view go first; when scrolling stops, the pending runs in view move to the front of the queue (`prioritize`).
    Starting a page translation stops those of other tabs (their batch is cancelled at once); they can resume from their page.
    New content (infinite scrolling) is picked up by a `MutationObserver`. Stop keeps what is translated, and Resume sends the rest.
  - Replacing (the default) or showing the translation below the original (without the copies' ids); the original nodes are kept to restore the page.
  - The host's `translate_segments` translates the runs of a batch in order with the earlier ones as history (`engine::document::translate_segments`).
- Limits:
  - The host loads its own copy of the model, so with the app open the model is in memory twice. Relaying to a running app would avoid that.
  - Browsers installed as Snap or Flatpak packages on Linux cannot start programs outside their sandbox.
  - Firefox would need its own manifest format (`allowed_extensions`) and an extension build for it.
- Translating without the desktop app (WASM llama.cpp or WebGPU in the extension) is possible later; it would be much slower and limited to the small models.

## 13. Status and later work

Done for v0.1.0: feasibility (M0), the two-pane MVP (M1), model management (M2), device selection (M3),
translation options and long texts (M4), and distribution (M5: Release workflow, icon, third-party notices, desktop entry).
Since then (v0.2.0): the browser extension (§12).

Possible later work:

- Reuse the KV cache prefix between chunks (each request starts from an empty cache today).
- Partial GPU offload for models larger than the GPU memory.
- An AppImage or distribution packages for Linux; signing the Windows executable.
- A global hotkey that translates the clipboard.
- Remote inference backends (§1).

## 14. Risks

| Risk | Status |
|---|---|
| The `plamo3` implementation in the vendored llama.cpp differs from the reference | Resolved: CLI output matches the model card examples |
| `llama-cpp-2` releases lag behind llama.cpp | Open: pinned to a git revision; move back to crates.io once a release vendors llama.cpp ≥ 0.8.0 |
| `dynamic-backends` with Vulkan does not build on Windows MSVC | Resolved: the Release workflow builds it |
| IME or text input gaps in gpui-kit's textarea | Open: patch or contribute upstream if reported |
| gpui-pre snapshots break the API often | Open: exact versions are pinned and updated deliberately |
| Long llama.cpp + Vulkan shader builds in CI | Resolved: packages are built only in the Release workflow, with cached target directories |
| Small models loop or skip text in long outputs | Mitigated: small chunks, loop detection, retries, and cut markers (§6.1) |
