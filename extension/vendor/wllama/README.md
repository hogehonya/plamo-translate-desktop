# wllama

[wllama](https://github.com/ngxson/wllama) (MIT) is llama.cpp (MIT) compiled to WebAssembly,
with WebGPU support. The extension's in-browser engine runs PLaMo 3 Translate with it.

Built from source by `.github/workflows/wllama.yml`, because the npm releases (3.8.1 so far)
bundle a llama.cpp that predates the PLaMo 3 tokenizer:

- wllama 3.12.0, commit c10c21114984e37fe6ba4edbba446c28c5b7a954
- llama.cpp commit d38bea34227bbf9069d62fdc239c98ab6e05435a

`wllama.js` is wllama's `esm/index.js` and `wllama.wasm` its `esm/wasm/wllama.wasm`, unmodified.
To update, run the workflow and replace this folder with its artifact.
