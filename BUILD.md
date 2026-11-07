# Building

The **Rust** dependencies are vendored so the native extension builds without
hitting the crates.io registry. Python dependencies are **not** vendored —
install them normally with `uv` or `pip`.

## Vendored Rust crates

`vendor/cargo/` holds every crate the three extensions need (pyo3,
flate2/zlib-rs, rayon, arrow/parquet with zstd, quine-mc_cluskey for boolean
minimization, and their transitive deps). `.cargo/config.toml` redirects
crates.io to that directory, so `cargo` / `maturin` never reach the network for
crates.

Three Rust crates are members of the Cargo workspace declared in the root
`Cargo.toml`: the Liberty backend (`liberty_tools_native`), the LEF backend
(`lef-tools-native` in `lef_tools/`), and the DEF backend
(`def-tools-native` in `def_tools/`, which links the LEF crate). Regenerate
the vendor directory after changing a manifest — crates.io must be reachable,
so move the source replacement aside first:

```bash
mv .cargo/config.toml /tmp/cargo-config.toml
cargo vendor vendor/cargo
mv /tmp/cargo-config.toml .cargo/config.toml
```

Refresh the vendor tree with the source replacement moved aside, never through
`git add` filters: the Python-flavoured `.gitignore` rules (`build/`, `lib/`,
`target/`) match nested directories inside `vendor/cargo/*`, so the negations
at the end of `.gitignore` keep the tree fully trackable. After a refresh,
check that every file cargo expects is present and unchanged:

```bash
python3 scripts/check_vendored_crates.py
```

`cargo` rejects a vendored crate whose files do not match its
`.cargo-checksum.json`, so a partial `git add` breaks every build with a
missing-file error. The `Vendored crates` workflow runs the same check and then
builds `def_tools` offline; keep it green when touching `Cargo.toml`.

## Read-only and air-gapped builds

`cargo` only needs `vendor/` and `Cargo.lock`, so the Rust side builds with no
network. Two constraints remain:

- `uv run` installs the project editable and maturin copies the extension back
  into the source tree, which fails on a read-only checkout. `uv run
  --no-editable` is the minimal fix when the uv cache is warm.
- With no warm uv cache, build with cargo directly and keep every writable path
  under the working directory.

`def_tools/scripts/run_example_offline.sh` does that end to end:

```bash
mkdir -p /tmp/run && cd /tmp/run
/path/to/open-eda-file-tools/def_tools/scripts/run_example_offline.sh \
  dump_instances.py --lef design.lef --def design.def.gz
```

It sets `CARGO_HOME` and `CARGO_TARGET_DIR` under `$PWD`, builds with
`--offline --locked`, assembles an importable `py/def_tools` package, disables
bytecode writes, and runs the example with that package on `PYTHONPATH`.
Nothing outside `$PWD` is written.

Interpreter selection is pinned, never a bare `python3` from `PATH`. The helper
reads `def_tools/.python-version` (`3.14`), prefers the loadout `def-tools`
venv interpreter, then `python3.14` / `python3.15`, then `python3`, checking
each candidate's version. It exports `PYO3_PYTHON` so cargo builds the
non-abi3 extension for exactly the interpreter that imports it. An explicit
`PYTHON=/path/to/python3.14` is a contract: an interpreter older than
`.python-version` is an error, not a fallback.

The same pinning is available once the package is installed: the
`def-dump-instances` console script is generated with an absolute-interpreter
shebang, so `uv tool install` / a loadout install runs it with the owning
interpreter no matter what `python3` resolves to.

Wheels are not published (PyPI or release assets), so an air-gapped machine
needs the vendored checkout and a Rust toolchain; the helper removes the uv and
editable-install requirements but not the compiler.

### zstd-sys OUT_DIR trap

`zstd-sys` copies the vendored zstd headers into its build-script `OUT_DIR`
with `fs::copy`, which preserves the read-only source mode (`444`). If a later
build unit reuses the same `CARGO_TARGET_DIR`, the build script cannot
overwrite those copies and panics:

```text
thread 'main' panicked at vendor/cargo/zstd-sys/build.rs:435:58:
called `Result::unwrap()` on an `Err` value: Os { code: 13, kind: PermissionDenied, ... }
```

Use a fresh target directory, `chmod -R u+w
.cargo-target/release/build/zstd-sys-*/out`, or let `run_example_offline.sh` do
it — the helper repairs the OUT_DIR before and after each build, leaving it
writable for the next build unit.

### AlmaLinux 8 offline image

`def_tools/docker/almalinux8.Dockerfile` proves the offline build on glibc
2.28. `rustc`/`cargo` are assumed to be present on the offline system; the
image installs the AlmaLinux Rust module and gcc for that baseline, plus a
checksummed python-build-standalone CPython 3.15, then copies the checkout
read-only. The compile and example run need no network and use only
`vendor/cargo`:

```bash
docker build -f def_tools/docker/almalinux8.Dockerfile -t def-tools-almalinux8 .
docker run --rm --network none def-tools-almalinux8
```

The container checks the vendored crate tree, generates a synthetic design,
builds the extension offline, and runs `dump_instances.py`; success ends with
`ok: /run/work/instances.parquet (...)`. The image is a test harness, not a
distribution: it pins CPython 3.15 for the non-abi3 extension. `.dockerignore`
must never filter `vendor/cargo` — the crates contain checksummed `.pdf` and
`.parquet` files, and the entry point's checksum check fails the build if any
are missing.

## Frontend

The viewer loads `viewer/static/plotly.min.js` locally (not from a CDN), so it
works without internet. Refresh it with:

```bash
curl -o viewer/static/plotly.min.js https://cdn.plot.ly/plotly-2.35.2.min.js
```

The viewer frontend is plain JS/CSS/HTML. There is no bundler. Syntax-check
edited static files directly:

```bash
node --check viewer/static/app.js
node --check viewer/static/symbol.js
node --check logic2svg/src/layout.js
node --check logic2svg/src/render.js
```

`viewer/static/symbol.js` is a hand-synced copy of `logic2svg/src/*`. For
logic-symbol edits, update both copies and run:

```bash
uv run pytest -q tests/test_symbol_render.py
```

## Prerequisites

- **Rust toolchain** (`rustc` + `cargo`); the Liberty crate uses edition 2021,
  the LEF and DEF crates edition 2024.
- **CPython 3.14** with `venv`.

## Build

Liberty (repository root):

```bash
# Python deps (online — uv or pip):
uv sync --dev
#   or:  python3.14 -m venv .venv && . .venv/bin/activate
#        pip install fastapi polars 'uvicorn[standard]' click maturin pytest ruff ty

# Native extension (Rust crates come from vendor/cargo via .cargo/config.toml):
uv run maturin develop --release
#   build a wheel instead:  uv run maturin build --release
```

LEF (its own project and virtualenv):

```bash
cd lef_tools
uv sync --dev                      # installs deps and builds the extension
uv run maturin develop --release   # after any Rust change
uv run maturin build --release     # wheel build
```

DEF (its own project and virtualenv; links `lef_tools` at the Rust level):

```bash
cd def_tools
uv sync --dev
uv run maturin develop --release
uv run maturin build --release
```

`.cargo/config.toml` points cargo at the vendored crates unconditionally, so the
crate side is already offline. Set `CARGO_NET_OFFLINE=true` to make any stray
crate fetch fail loudly.

## Run / test

```bash
# Liberty
uv run liberty-view dev.lib          # browser viewer (Plotly served locally)
uv run liberty-format dev.lib        # transparency-checked formatter
uv run pytest -q                     # Python tests
uv run ruff check . && cargo fmt --all --check

# LEF
cd lef_tools
uv run lef-tools --help
uv run pytest -q
uv run ruff check . && uv run ty check

# DEF
cd ../def_tools
uv run def-tools --help
uv run pytest -q                     # Python/CLI tests; perf tests skip without --runperf
uv run pytest --runperf              # scale tests (minutes, generates a ~225 MB design)
uv run ruff check . && uv run ty check
cargo test -p lef-tools-native -p def-tools-native  # Rust unit tests, from the repo root
```

## Documentation

The documentation site is built with Great Docs and Quarto. `great-docs.yml`
is the source of truth; narrative pages live in `user_guide/`, and the
generated `great-docs/` directory is gitignored.

Install [Quarto](https://quarto.org/docs/get-started/) and run:

```bash
PYTHONPATH=$PWD uvx --from great-docs==0.17.0 --with uvicorn great-docs build
PYTHONPATH=$PWD uvx --from great-docs==0.17.0 --with uvicorn great-docs preview
```

The build uses griffe static analysis (`dynamic: false`), so it does not need
the native extension or a `uv sync`. `.github/workflows/docs.yml` deploys the
site to GitHub Pages on `main`.
