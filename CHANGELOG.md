# Changelog

All notable changes to PLaMo Translate Desktop. Each release's section here also becomes its release notes on GitHub.

## [0.2.1] - 2026-11-06

### Fixed

- Translate this page in the extension's panel seemed to do nothing at times. The browser lets the panel into a tab only after the toolbar button or the right-click menu was used on it, so the button fails on other tabs, such as after switching tabs or going to another site. The explanation now shows right under the button instead of at the bottom of the panel, and offers to allow the extension on all sites only where that helps: pages that extensions cannot enter, such as the browser's own pages, now say so.
- Asking again to translate a page that is translated already now shows its box again (it stayed hidden if it was closed).

## [0.2.0] - 2026-11-06

### Added

- A browser extension for Chrome and Edge (`browser-extension/` in the packages) that translates with the desktop app's model. It talks to `plamo-translate-host`, a small program that ships next to the app; the app does not need to be running. See "Browser extension" in the README.
  - Translate selected text from the context menu, or text typed or pasted into the side panel, into any of the app's target languages and styles. The translation streams in.
  - Translate a whole page in place, keeping links and formatting. Paragraphs in view go first, and the ones you scroll to move ahead. The box on the page can stop, resume, or restore the original. Starting another page stops the previous one, which can be resumed later. Translations can also be shown below the original instead.
  - Choose the model (any downloaded one) and the device in the panel, independently of the app's selection.
  - Release frees the model's memory, including GPU memory. The host also ends by itself after 5 minutes without use, and starts again with the next translation.
  - The extension's texts follow the browser's language, or English or Japanese as set under Display language in the panel's settings.
- Settings → Browser extension in the app: Connect registers the host with Chrome and Edge (on Linux also Chromium and Brave), Disconnect removes the registration, and Open extension folder shows the folder to load into the browser. The section also shows when the browsers use another copy of the app, such as one moved or unpacked elsewhere.

### Known limitations

- The host loads its own copy of the model, so with the app open as well the model is in memory twice.
- Browsers installed as Snap or Flatpak packages on Linux cannot start the host. Firefox is not supported.

## [0.1.0] - 2026-11-05

The first release.

- Local translation between 53 languages with PLaMo 3 Translate through llama.cpp, offline once a model is downloaded. The default target is Japanese ⇄ English (automatic).
- Translation as you type or paste, streamed as it is generated, with the progress, speed, and remaining time in the status bar.
- Japanese output style (polite or plain), a reference context, and a glossary.
- Long texts translated paragraph by paragraph with earlier pairs as history. Output that falls into a repetition loop is detected and translated again in smaller pieces; a passage that keeps looping is cut and marked. Chunk and history sizes are adjustable.
- A model manager for the 0.8B, 2B, 8B, and 31B GGUF files, with PFN's published quality figures, resumable and verified downloads, and the PLaMo Community License shown before the first download.
- CPU (AVX2 or AVX-512) and Vulkan GPU inference, with a selectable device; ROCm for Linux source builds.
- English and Japanese UI, light and dark themes, and synchronized scrolling between the source and the translation.
- Packages for Windows and Linux (x86-64), with third-party notices.

[0.2.1]: https://github.com/tanakh/plamo-translate-desktop/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/tanakh/plamo-translate-desktop/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/tanakh/plamo-translate-desktop/releases/tag/v0.1.0
