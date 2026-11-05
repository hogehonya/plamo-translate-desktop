#!/usr/bin/env bash
# Prints the section of CHANGELOG.md for version $1 (without its heading), for release notes.
# Prints nothing if there is no such section.
set -euo pipefail
version=${1:?usage: changelog.sh VERSION}
changelog=$(dirname "$0")/../CHANGELOG.md
awk -v heading="## [$version]" '
  /^## \[/ { if (found) exit; found = index($0, heading) == 1; next }
  /^\[[^]]*\]: / { next }
  found
' "$changelog" | sed -e '/./,$!d' | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
