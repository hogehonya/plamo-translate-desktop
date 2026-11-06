#!/bin/sh
# Copies what the extension needs from a wllama build into the extension: the bundled library,
# the wasm module, the licenses, and where they came from.
#
# usage: scripts/vendor-wllama.sh WLLAMA_DIR OUTPUT_DIR
#   WLLAMA_DIR  a wllama checkout built with build_wasm.sh, build:worker, build:tsup and
#               post_build.sh (see .github/workflows/wllama.yml)
set -eu
src=$1
out=$2

test -f "$src/esm/index.js" || { echo "$src/esm/index.js is missing: build wllama first" >&2; exit 1; }
test -f "$src/esm/wasm/wllama.wasm" || { echo "$src/esm/wasm/wllama.wasm is missing" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out"
cp "$src/esm/index.js" "$out/wllama.js"
cp "$src/esm/wasm/wllama.wasm" "$out/wllama.wasm"
cp "$src/LICENCE" "$out/LICENSE-wllama.txt"
cp "$src/llama.cpp/LICENSE" "$out/LICENSE-llama.cpp.txt"

wllama_commit=$(git -C "$src" rev-parse HEAD)
llama_commit=$(git -C "$src/llama.cpp" rev-parse HEAD)
version=$(node -p "require('./$src/package.json').version")
llama_build=$(sed -n "s/^export const LIBLLAMA_VERSION = '\(.*\)';/\1/p" "$src/src/workers-code/generated.ts")

cat > "$out/README.md" <<EOF
# wllama

[wllama](https://github.com/ngxson/wllama) (MIT) is llama.cpp (MIT) compiled to WebAssembly,
with WebGPU support. The extension's in-browser engine runs PLaMo 3 Translate with it.

Built from source by \`.github/workflows/wllama.yml\`, because the npm releases (3.8.1 so far)
bundle a llama.cpp that predates the PLaMo 3 tokenizer:

- wllama $version, commit $wllama_commit
- llama.cpp $llama_build, commit $llama_commit

\`wllama.js\` is wllama's \`esm/index.js\` and \`wllama.wasm\` its \`esm/wasm/wllama.wasm\`, unmodified.
To update, run the workflow and replace this folder with its artifact.
EOF
ls -l "$out"
