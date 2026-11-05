#!/usr/bin/env bash
# Run a command with the environment for building and running the ROCm (HIP) backend on Linux,
# including WSL2. Example:
#
#   scripts/rocm.sh cargo run -p plamo-translate-desktop --release --features rocm
#
# GPU_ARCH selects the HIP target (default: the first discrete GPU reported by rocminfo).
# Builds go to target/rocm so they do not invalidate the default (CPU) build.
set -euo pipefail

if ! command -v hipconfig >/dev/null; then
    echo "hipconfig not found: install ROCm and its development packages" >&2
    exit 1
fi

arch="${GPU_ARCH:-}"
if [[ -z "$arch" ]]; then
    # The first gfx target that is not an integrated GPU (gfx103x/gfx115x APUs are listed too).
    arch="$(rocminfo 2>/dev/null | awk '/Name: +gfx/ {print $2}' | grep -v -E '^gfx(1036|1035|1103|115[0-3])$' | head -n1 || true)"
fi
if [[ -z "$arch" ]]; then
    echo "could not detect a GPU target; set GPU_ARCH (for example gfx1201)" >&2
    exit 1
fi

rocm_root="$(hipconfig -R)"
export HIPCXX="$(hipconfig -l)/clang"
export HIP_PATH="$rocm_root"
export CMAKE_HIP_ARCHITECTURES="$arch"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/rocm}"
export LD_LIBRARY_PATH="/opt/rocm/lib:$rocm_root/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

echo "ROCm: $rocm_root, target $arch, CARGO_TARGET_DIR=$CARGO_TARGET_DIR" >&2
exec "$@"
