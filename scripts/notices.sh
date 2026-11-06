#!/bin/sh
# Writes THIRD-PARTY-NOTICES.txt for a package: the licenses of the Rust crates (cargo-about,
# configured by about.toml and about.hbs) followed by the components cargo-about cannot see.
#
# usage: scripts/notices.sh windows|linux|macos OUTPUT
set -eu
platform=$1
output=$2
cd "$(dirname "$0")/.."
# about.toml lists the Windows and Linux targets; the macOS package has its own crate set.
set --
if [ "$platform" = macos ]; then
    set -- --target aarch64-apple-darwin
fi
{
    cargo about generate --locked "$@" about.hbs
    echo "Other components"
    echo "================"
    echo
    cat licenses/llama.cpp.txt
    echo
    cat licenses/lucide.txt
    if [ "$platform" = windows ]; then
        echo
        cat licenses/msvc-runtime.txt
    fi
} > "$output"
