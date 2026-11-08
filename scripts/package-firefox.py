#!/usr/bin/env python3
"""Build the Firefox package without modifying the Chromium extension."""

from pathlib import Path
import json
import zipfile
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "extension"
manifest = json.loads((SOURCE / "manifest.firefox.json").read_text())
output = ROOT / "dist" / f"plamo-translate-{manifest['version']}-firefox-unsigned.zip"
output.parent.mkdir(parents=True, exist_ok=True)
with (
    tempfile.TemporaryDirectory(prefix="plamo-firefox-") as scratch,
    zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive,
):
    subprocess.run(
        [
            "node",
            str(ROOT / "scripts/firefox-worker.mjs"),
            str(SOURCE / "vendor/wllama/wllama.js"),
            scratch,
        ],
        check=True,
    )
    for path in sorted(SOURCE.rglob("*")):
        if path.is_file() and path.name not in (
            "manifest.json",
            "manifest.firefox.json",
        ):
            if path == SOURCE / "vendor/wllama/wllama.js":
                archive.write(Path(scratch) / "wllama.js", "vendor/wllama/wllama.js")
            else:
                archive.write(path, path.relative_to(SOURCE))
    archive.write(
        Path(scratch) / "firefox-worker.js", "vendor/wllama/firefox-worker.js"
    )
    archive.writestr(
        "manifest.json", json.dumps(manifest, ensure_ascii=False, indent=2) + "\n"
    )
print(output)
