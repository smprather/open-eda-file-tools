"""Synthesize one big Liberty library by concatenating the *bodies* of several
ASAP7 ccsa/TT libs (all Vth flavors) under a single `library` group.

For each source file the leading comment block, the `library (...) {` opening
line, and the final `}` are dropped; everything in between is streamed out. Cell
names are unique across Vth (`_L`/`_R`/`_SL`/`_SRAM`), so there are no collisions.
Library-level attrs and template groups end up repeated once per file, which the
parser tolerates (last value wins; unknown template refs are harmless).

Optionally replicate the whole set ``--copies N`` times to inflate the file to a
size target; copies 2..N get a ``_cK`` suffix on every cell name to stay unique.

Usage:
    python scripts/build_combo_lib.py OUT.lib.gz LIBNAME [--copies N] IN1.gz IN2.gz ...
Env: GZ_LEVEL (gzip level, default 6).
"""

from __future__ import annotations

import gzip
import os
import re
import sys

_CELL_RE = re.compile(r"^(\s*cell \()([^)]+)(\).*)$")


def stream_body(fh, out, cell_suffix: str = "") -> None:
    """Write this file's `library {...}` interior to ``out`` (no outer braces).

    Tracks body brace depth (depth 0 == directly inside `library`) and drops any
    lone ``}`` at depth 0. That removes the file's own library-close brace *and*
    any stray ``}`` that prematurely closes the library (the ASAP7 SIMPLE-group
    defect), keeping every cell inside the combined library. Brace counting is a
    cheap ``str.count`` — safe here because these libs put no braces inside
    strings or comments (values are numeric).
    """
    started = False
    depth = 0
    for raw in fh:
        line = raw.rstrip("\n")
        if not started:
            if line.lstrip().startswith("library") and "{" in line:
                started = True  # skip the `library (...) {` line itself
            continue
        if line.strip() == "}" and depth == 0:
            continue  # stray / library-close brace at body level -> drop
        if cell_suffix and depth == 0:
            m = _CELL_RE.match(line)
            if m:
                line = f"{m.group(1)}{m.group(2)}{cell_suffix}{m.group(3)}"
        out.write(line)
        out.write("\n")
        depth += line.count("{") - line.count("}")
        if depth < 0:
            depth = 0


def main() -> None:
    args = sys.argv[1:]
    copies = 1
    if "--copies" in args:
        i = args.index("--copies")
        copies = int(args[i + 1])
        del args[i : i + 2]
    out_path, libname, *inputs = args
    level = int(os.environ.get("GZ_LEVEL", "6"))
    with gzip.open(out_path, "wt", encoding="utf-8", compresslevel=level) as out:
        out.write(f"library ({libname}) {{\n")
        for k in range(1, copies + 1):
            suffix = "" if k == 1 else f"_c{k}"
            for j, path in enumerate(inputs, 1):
                with gzip.open(path, "rt", encoding="utf-8") as fh:
                    stream_body(fh, out, suffix)
                print(f"copy {k}/{copies}  [{j}/{len(inputs)}] {path.split('/')[-1]}", flush=True)
        out.write("}\n")
    print(f"DONE -> {out_path}", flush=True)


if __name__ == "__main__":
    main()
