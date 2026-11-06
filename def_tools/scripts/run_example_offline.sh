#!/usr/bin/env bash
# Run a def_tools example from a writable working directory with no network and
# without writing to the (possibly read-only) package checkout.
#
# Usage:  run_example_offline.sh [example.py] [example args...]
#         PYTHON=/usr/bin/python3.14 run_example_offline.sh dump_instances.py ...
#
# Everything writable stays under $PWD: .cargo-home, .cargo-target and py/.
# The example is run with PYTHONPATH pointing at the assembled py/ package.
set -euo pipefail

usage() {
    cat <<'EOF'
Run a def_tools example from the current (writable) directory, offline.

Usage: run_example_offline.sh [example.py] [args...]

  example.py   Name under def_tools/examples/ (default: dump_instances.py).
  args...      Forwarded to the example; if the first argument starts with "-",
               it is forwarded to the default example.

Environment:
  PYTHON       Interpreter to build for and run with. Default: the loadout
               def-tools venv python, else python<version> from .python-version,
               else python3 -- each checked against .python-version. Setting it
               is a contract: a too-old interpreter is an error, not a fallback.

State is written to $PWD/.cargo-home, $PWD/.cargo-target and $PWD/py.
Run this from a writable directory outside the package checkout.
EOF
}

case "${1:-}" in
    -h | --help)
        usage
        exit 0
        ;;
esac

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
PKG_DIR=$(dirname -- "$SCRIPT_DIR")
REPO_DIR=$(dirname -- "$PKG_DIR")
RUN=$PWD

# The extension is a non-abi3 cdylib: the interpreter that builds it must be the
# one that runs it. Never default to a bare `python3` from PATH -- resolve an
# interpreter that satisfies the project's own .python-version, and pin cargo to
# it with PYO3_PYTHON so the built ABI cannot drift from the runner.
REQUIRED_VERSION=$(cat -- "$PKG_DIR/.python-version" 2>/dev/null || printf '3.14')
REQUIRED_MINOR=${REQUIRED_VERSION#*.}
REQUIRED_MAJOR=${REQUIRED_VERSION%%.*}
case "$REQUIRED_MAJOR" in '' | *[!0-9]*) REQUIRED_MAJOR=3 ;; esac
case "$REQUIRED_MINOR" in '' | *[!0-9]*) REQUIRED_MINOR=14 ;; esac

version_ok() {
    "$1" -c "import sys; sys.exit(0 if sys.version_info[:2] >= ($REQUIRED_MAJOR, $REQUIRED_MINOR) else 1)" \
        >/dev/null 2>&1
}

resolve_python() {
    # A loadout (engineering-loadout) def-tools venv pins its own interpreter;
    # prefer that copy, then the versioned names, and only then `python3`.
    for candidate in \
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

# An explicit PYTHON is a contract, not a hint: fail instead of silently falling
# back to a different interpreter than the caller asked for.
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
error: no Python >= $REQUIRED_VERSION found (project .python-version: $REQUIRED_VERSION)
  install one, or point PYTHON at it:
    PYTHON=/path/to/python$REQUIRED_VERSION $0 ${1:-dump_instances.py} ...
EOF
    exit 1
fi
# Cargo/pyo3 must build the extension for exactly the interpreter that will
# import it; do not let pyo3 rediscover one from PATH.
export PYO3_PYTHON=$PYTHON

EXAMPLE=dump_instances.py
if [ "$#" -gt 0 ] && [ "${1#-}" = "$1" ]; then
    EXAMPLE=$1
    shift
fi

case "$EXAMPLE" in
    */*) EXAMPLE_PATH=$EXAMPLE ;;
    *) EXAMPLE_PATH=$PKG_DIR/examples/$EXAMPLE ;;
esac
if [ ! -f "$EXAMPLE_PATH" ]; then
    echo "error: example not found: $EXAMPLE_PATH" >&2
    exit 1
fi

# Writes must not land in the package checkout (which may be read-only).
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

export CARGO_HOME=$RUN/.cargo-home
export CARGO_TARGET_DIR=$RUN/.cargo-target
export CARGO_NET_OFFLINE=true

# zstd-sys's build script copies the vendored zstd headers into OUT_DIR with
# fs::copy, which preserves the read-only source mode (444). A later build-script
# run that reuses the same OUT_DIR then cannot overwrite them and panics. Repair
# existing copies before every build and leave new copies writable afterwards,
# so a subsequent build unit (uv build, another target) can reuse the target dir.
repair_zstd_out_dir() {
    if [ -d "$CARGO_TARGET_DIR" ]; then
        find "$CARGO_TARGET_DIR" -type d -path '*/zstd-sys-*/out' -prune \
            -exec chmod -R u+w {} + 2>/dev/null || true
    fi
}

repair_zstd_out_dir

echo "building def_tools (offline)..." >&2
# Run from the package directory so cargo discovers the repository's
# .cargo/config.toml (the vendored source replacement) while walking up.
(cd "$PKG_DIR" && cargo build --release --locked --offline --lib)
repair_zstd_out_dir

NATIVE_LIB=
for ext in so dylib dll; do
    NATIVE_LIB=$(find "$CARGO_TARGET_DIR/release" -maxdepth 1 \
        -name "libdef_tools_native.$ext" -print -quit)
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

PACKAGE_DIR=$RUN/py/def_tools
rm -rf "$RUN/py"
mkdir -p "$PACKAGE_DIR"
cp "$PKG_DIR"/src/def_tools/*.py "$PACKAGE_DIR"/
for extra in _def.pyi py.typed; do
    if [ -f "$PKG_DIR/src/def_tools/$extra" ]; then
        cp "$PKG_DIR/src/def_tools/$extra" "$PACKAGE_DIR"/
    fi
done
cp "$NATIVE_LIB" "$PACKAGE_DIR/_def$SUFFIX"

export PYTHONDONTWRITEBYTECODE=1
export PYTHONPATH="$RUN/py${PYTHONPATH:+:$PYTHONPATH}"

exec "$PYTHON" "$EXAMPLE_PATH" "$@"
