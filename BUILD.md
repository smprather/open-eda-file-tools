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

## Offline builds

### What is vendored and what the environment supplies

`vendor/cargo/` plus `Cargo.lock` contain every Rust **package dependency**.
No compiler or interpreter is vendored: an offline workstation provides those
(Engineering Loadout does; see below).

| Needed | Supplied by | Verify |
| --- | --- | --- |
| Crate dependencies | `vendor/cargo/` + `Cargo.lock` | `python3 scripts/check_vendored_crates.py` |
| `rustc`, `cargo` | offline system; Loadout `rust` 1.96.0 | `cargo --version` |
| Python 3.14+ | offline system; Loadout `portable-python` 3.15.0 | `python3.15 --version` |
| C compiler | offline system; gcc 8.5 (EL8) or clang 23 | `cc --version` |
| Python packages | **not vendored** | examples need none; pytest does |

`.cargo/config.toml` redirects crates.io to `vendor/cargo`, so
`cargo build --offline --locked` never contacts a registry.

### Engineering Loadout (the offline package manager)

On the reference machine Loadout lives in `~/engineering-loadout` and installs
under `$XDG_DATA_HOME/loadout` (default `~/.local/share/loadout`). It supplies
the toolchain this repository needs:

```bash
./loadout info rust              # 1.96.0, payload/.../runtime/rust.tar.bz2
./loadout info portable-python   # 3.15.0, BOLT-optimized CPython
./loadout install rust portable-python
export PATH="${XDG_DATA_HOME:-$HOME/.local/share}/loadout/bin:$PATH"
export PYTHON="${XDG_DATA_HOME:-$HOME/.local/share}/loadout/bin/python3.15"
```

Loadout installs from its release payload; neither the install nor the build
needs network. `clang` 23 is also available (`./loadout install clang`) for
systems without gcc: set `CC=clang` and `AR=llvm-ar`. gcc/glibc headers remain
system prerequisites because `zstd-sys` compiles vendored C sources.

### Reproduce the compile

Run these from the repository root. Each step prints the expected evidence.

```bash
python3 scripts/check_vendored_crates.py
# ok: 119 crates, 5536 files present and unchanged

cd def_tools
CARGO_NET_OFFLINE=true cargo build --release --locked --offline --lib
# ...
# Finished `release` profile [optimized] target(s) in ...
```

Run cargo from `def_tools/` (or another directory inside the repository) so it
discovers `.cargo/config.toml`; a build started outside the repository with
`--manifest-path` misses the vendored source replacement.

The output is `target/release/libdef_tools_native.so`, a non-abi3 extension.
The extension and the interpreter that loads it must match; the helper exports
`PYO3_PYTHON` for exactly that reason.

### Run the example

Everything writable stays in the current directory:

```bash
mkdir -p /tmp/run && cd /tmp/run
PYTHON="${XDG_DATA_HOME:-$HOME/.local/share}/loadout/bin/python3.15" \
  /path/to/open-eda-file-tools/def_tools/scripts/run_example_offline.sh \
  dump_instances.py --lef design.lef --def design.def.gz --output instances.parquet
```

Expected tail:

```text
instances: <N>
format: parquet
output: /tmp/run/instances.parquet (… bytes)
```

The helper sets `CARGO_HOME`/`CARGO_TARGET_DIR` under `$PWD`, builds with
`--offline --locked`, assembles `py/def_tools`, disables bytecode writes, and
pins the interpreter (`PYTHON` override, checked against
`def_tools/.python-version`). Nothing outside `$PWD` is written and the
checkout may be read-only. Once installed, `def-dump-instances` uses the same
pin through an absolute-interpreter shebang.

Wheels are not published, so an air-gapped machine compiles from the vendored
checkout.

### Modify or optimize offline

- Rust backend: `def_tools/rust/src/lib.rs`. Python API/CLI:
  `def_tools/src/def_tools/` (`__init__.py`, `cli.py`, `dump_instances.py`,
  `io_config.py`, `sdc.py`).
- Rebuild incrementally with the same command. The helper is safe to re-run:
  it reuses `$PWD/.cargo-target`.
- **Do not add crates offline.** A new dependency needs a networked
  `cargo vendor vendor/cargo` refresh (move `.cargo/config.toml` aside first),
  then commit `vendor/` and re-run `scripts/check_vendored_crates.py`.
- Keep `PYO3_PYTHON` in sync with the interpreter you run; the helper does.
- Tests need dev-only Python packages (`pytest`), which are not vendored in
  this repository. The examples and the native build need none.
- The example report (`wall`, `peak RSS`, instances/s) is the quickest way to
  measure a runtime change; every documented command already uses the release
  profile.

### Verify the result (self-evidence)

```bash
file target/release/libdef_tools_native.so
# ELF 64-bit ... shared object, x86-64
ldd target/release/libdef_tools_native.so
# linux-vdso, libgcc_s, libm, libc, ld-linux -- no libpython or Loadout paths
PYTHONPATH=/tmp/run/py python3.15 -c 'import def_tools; print(def_tools.__version__)'
```

The Docker run below exits 0 only when the vendored tree is complete, the
extension builds with no network, and the example writes its Parquet output.

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

### Docker reproduction (AlmaLinux 8, no network)

`def_tools/docker/almalinux8.Dockerfile` models an air-gapped AlmaLinux 8
(glibc 2.28) workstation: gcc comes from the image, while rustc/cargo and
Python come from the Engineering Loadout install mounted at `/opt/loadout`.

```bash
docker build -f def_tools/docker/almalinux8.Dockerfile -t def-tools-almalinux8 .
docker run --rm --network none \
  -v "${XDG_DATA_HOME:-$HOME/.local/share}/loadout:/opt/loadout:ro" \
  def-tools-almalinux8
```

The entry point verifies the vendored tree, generates a synthetic design,
builds offline, and runs `dump_instances.py`; success ends with
`ok: /run/work/instances.parquet (...)`. Pass `-e INSTANCES=2585205` to run the
full-size design (2.58M instances). `.dockerignore` must never filter
`vendor/cargo` — the crates contain checksummed `.pdf` and `.parquet` files,
and the entry point's checksum check fails the build if any are missing.

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
