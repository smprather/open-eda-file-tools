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
