#!/usr/bin/env bash
# Build the native extension offline and run `def-tools instances` from a
# writable working directory without writing to the (possibly read-only)
# repository checkout.
#
# Usage:  run_example_offline.sh [instances args...]
#         PYTHON=/usr/bin/python3.14 run_example_offline.sh --lef ... --def ...
#
# Everything writable stays under $PWD: .cargo-home, .cargo-target and py/.
set -euo pipefail

usage() {
    cat <<'EOF'
Build open-eda-file-tools offline in the current (writable) directory and run
the DEF instance dumper (`def-tools instances`).

Usage: run_example_offline.sh [args...]

  args...      Forwarded to `def_tools.instances` (for example
               `--lef design.lef --def design.def.gz --output instances.parquet`).

Environment:
  PYTHON       Interpreter to build for and run with. Default: the loadout
               venv python, else python<version> from .python-version, else
               python3 -- each checked against >= 3.14. Setting it is a
               contract: a too-old interpreter is an error, not a fallback.

State is written to $PWD/.cargo-home, $PWD/.cargo-target and $PWD/py.
Run this from a writable directory outside the repository checkout.
EOF
}

case "${1:-}" in
    -h | --help)
        usage
        exit 0
        ;;
esac

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
REPO_DIR=$(dirname -- "$SCRIPT_DIR")
RUN=$PWD

REQUIRED_VERSION=$(cat -- "$REPO_DIR/.python-version" 2>/dev/null || printf '3.14')
REQUIRED_MINOR=${REQUIRED_VERSION#*.}
REQUIRED_MAJOR=${REQUIRED_VERSION%%.*}
case "$REQUIRED_MAJOR" in '' | *[!0-9]*) REQUIRED_MAJOR=3 ;; esac
case "$REQUIRED_MINOR" in '' | *[!0-9]*) REQUIRED_MINOR=14 ;; esac

version_ok() {
    "$1" -c "import sys; sys.exit(0 if sys.version_info[:2] >= ($REQUIRED_MAJOR, $REQUIRED_MINOR) else 1)" \
        >/dev/null 2>&1
}

resolve_python() {
    for candidate in \
        "${XDG_DATA_HOME:-$HOME/.local/share}/loadout/share/uv/tools/open-eda-file-tools/bin/python" \
        "${XDG_DATA_HOME:-$HOME/.local/share}/loadout/share/uv/tools/def-tools/bin/python" \
        "python$REQUIRED_MAJOR.$REQUIRED_MINOR" \
        "python$REQUIRED_MAJOR.$((REQUIRED_MINOR + 1))" \
        python3; do
        resolved=$(command -v -- "$candidate" 2>/dev/null || true)
        if [ -n "$resolved" ] && version_ok "$resolved"; then
            printf '%s\n' "$resolved"
            return 0
        fi
    done
    return 1
}

if [ -n "${PYTHON:-}" ]; then
    PYTHON_PATH=$(command -v -- "$PYTHON" 2>/dev/null || true)
    if [ -z "$PYTHON_PATH" ]; then
        echo "error: PYTHON=$PYTHON not found" >&2
        exit 1
    fi
    if ! version_ok "$PYTHON_PATH"; then
        echo "error: PYTHON=$PYTHON is $("$PYTHON_PATH" -c 'import sys; print(sys.version.split()[0])' 2>/dev/null); $REQUIRED_VERSION or newer is required" >&2
        exit 1
    fi
    PYTHON=$PYTHON_PATH
elif ! PYTHON=$(resolve_python); then
    cat >&2 <<EOF
error: no Python >= $REQUIRED_VERSION found
  install one, or point PYTHON at it:
    PYTHON=/path/to/python$REQUIRED_VERSION $0 --lef ... --def ...
EOF
    exit 1
fi
export PYO3_PYTHON=$PYTHON

if [ "$#" -gt 0 ] && { [ "$1" = "dump_instances.py" ] || [ "$1" = "instances" ]; }; then
    shift
fi

case "$RUN/" in
    "$REPO_DIR"/*)
        echo "error: run from a writable directory outside $REPO_DIR" >&2
        exit 1
        ;;
esac
if [ ! -w "$RUN" ]; then
    echo "error: $RUN is not writable" >&2
    exit 1
fi

for tool in cargo "$PYTHON"; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not found on PATH" >&2
        exit 1
    fi
done

MARKER=$RUN/py/.open-eda-file-tools-run
if [ -e "$RUN/py" ] && [ ! -e "$MARKER" ] && [ ! -e "$RUN/py/.def-tools-run" ]; then
    echo "error: $RUN/py exists and was not created by run_example_offline.sh" >&2
    echo "       remove it or run from a different directory" >&2
    exit 1
fi

export CARGO_HOME=$RUN/.cargo-home
export CARGO_TARGET_DIR=$RUN/.cargo-target
export CARGO_NET_OFFLINE=true

repair_zstd_out_dir() {
    if [ -d "$CARGO_TARGET_DIR" ]; then
        find "$CARGO_TARGET_DIR" -type d -path '*/zstd-sys-*/out' -prune \
            -exec chmod -R u+w {} + 2>/dev/null || true
    fi
}

repair_zstd_out_dir

echo "building open-eda-file-tools (offline)..." >&2
(cd "$REPO_DIR" && cargo build --release --locked --offline --lib)
repair_zstd_out_dir

NATIVE_LIB=
for ext in so dylib dll; do
    NATIVE_LIB=$(find "$CARGO_TARGET_DIR/release" -maxdepth 1 \
        -name "lib_native.$ext" -print -quit)
    if [ -n "$NATIVE_LIB" ]; then
        break
    fi
done
if [ -z "$NATIVE_LIB" ]; then
    echo "error: native library not found under $CARGO_TARGET_DIR/release" >&2
    exit 1
fi

SUFFIX=$("$PYTHON" -c 'import sysconfig; print(sysconfig.get_config_var("EXT_SUFFIX"))')
if [ -z "$SUFFIX" ]; then
    echo "error: could not determine EXT_SUFFIX for $PYTHON" >&2
    exit 1
fi

rm -rf "$RUN/py"
mkdir -p "$RUN/py/liberty_tools" "$RUN/py/lef_tools" "$RUN/py/def_tools" "$RUN/py/sdc_tools"
: > "$MARKER"
for pkg in liberty_tools lef_tools def_tools sdc_tools; do
    cp "$REPO_DIR/$pkg"/*.py "$RUN/py/$pkg"/
    for extra in __init__.pyi _native.pyi py.typed; do
        if [ -f "$REPO_DIR/$pkg/$extra" ]; then
            cp "$REPO_DIR/$pkg/$extra" "$RUN/py/$pkg/$extra"
        fi
    done
done
cp "$NATIVE_LIB" "$RUN/py/liberty_tools/_native$SUFFIX"

export PYTHONDONTWRITEBYTECODE=1
export PYTHONPATH="$RUN/py${PYTHONPATH:+:$PYTHONPATH}"

exec "$PYTHON" -m def_tools.instances "$@"
