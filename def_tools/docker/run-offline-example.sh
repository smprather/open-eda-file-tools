#!/usr/bin/env bash
# Entry point for the AlmaLinux 8 offline-build image. Run with --network none.
set -euo pipefail

: "${PYTHON:=/opt/python/bin/python3.15}"
export PYTHON

echo "python: $("$PYTHON" --version 2>&1)"
echo "cargo:  $(cargo --version)"
echo "rustc:  $(rustc --version)"

"$PYTHON" /src/scripts/check_vendored_crates.py

"$PYTHON" /src/def_tools/tests/gen/design_gen.py \
    -o "$PWD/gen" -n "${INSTANCES:-20000}" >/dev/null

/src/def_tools/scripts/run_example_offline.sh dump_instances.py \
    --lef "$PWD/gen/design.lef" \
    --def "$PWD/gen/design.def.gz" \
    --output "$PWD/instances.parquet"

test -s "$PWD/instances.parquet"
echo "ok: $PWD/instances.parquet ($(stat -c %s "$PWD/instances.parquet") bytes)"
