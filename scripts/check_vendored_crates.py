#!/usr/bin/env python3
"""Verify that vendor/cargo is complete and intact.

`cargo` validates every vendored crate against its `.cargo-checksum.json` and
aborts on the first missing or altered file, so a tree that was `git add`ed
while an ignore rule was active fails every from-source build. Run this after a
`cargo vendor` refresh (and in CI) to catch that class of breakage.
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

VENDOR_DIR = Path(__file__).resolve().parent.parent / "vendor" / "cargo"


def main() -> int:
    manifests = sorted(VENDOR_DIR.glob("*/.cargo-checksum.json"))
    if not manifests:
        print(f"no vendored crates found under {VENDOR_DIR}", file=sys.stderr)
        return 1

    missing = 0
    mismatched = 0
    checked = 0
    for manifest in manifests:
        crate_dir = manifest.parent
        files: dict[str, str] = json.loads(manifest.read_text())["files"]
        for relative, expected in files.items():
            path = crate_dir / relative
            if not path.is_file():
                print(f"{crate_dir.name}: missing {relative}")
                missing += 1
                continue
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            if digest != expected:
                print(f"{crate_dir.name}: checksum mismatch {relative}")
                mismatched += 1
        checked += len(files)

    if missing or mismatched:
        print(f"FAIL: {missing} missing, {mismatched} mismatched files")
        return 1

    print(f"ok: {len(manifests)} crates, {checked} files present and unchanged")
    return 0


if __name__ == "__main__":
    sys.exit(main())
