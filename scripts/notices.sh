#!/bin/sh
# Writes THIRD-PARTY-NOTICES.txt for a package: the licenses of the Rust crates (cargo-about,
# configured by about.toml and about.hbs) followed by the components cargo-about cannot see.
#
# usage: scripts/notices.sh windows|linux OUTPUT
set -eu
platform=$1
output=$2
cd "$(dirname "$0")/.."
{
    cargo about generate --locked about.hbs
    echo "Other components"
    echo "================"
    echo
    cat licenses/llama.cpp.txt
    echo
    cat licenses/wllama.txt
    echo
    cat licenses/lucide.txt
    if [ "$platform" = windows ]; then
        echo
        cat licenses/msvc-runtime.txt
    fi
} > "$output"
