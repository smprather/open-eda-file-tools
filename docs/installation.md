# Installation

`open-eda-file-tools` is a single Python project that installs all five
libraries (`liberty_tools`, `lef_tools`, `def_tools`, `sdc_tools`, `pex_tools`)
and all command-line tools (`liberty-tools`, `liberty-format`, `liberty-view`,
`lef-tools`, `def-tools`, `sdc-tools`, `pex-tools`).

## Prerequisites

- **Python 3.14+**
- **Rust toolchain** (`rustc` and `cargo`, edition 2024) and a C compiler (for `zstd-sys`)
- **Tcl >= 8.6** (`tclsh8.6` or `tclsh` on `PATH`, or `TCLSH=/path/to/tclsh`) for `sdc-tools`
- **[`uv`](https://docs.astral.sh/uv/)** for development and tool installs

## Install from GitHub

With `pip`:

```bash
python -m pip install "git+https://github.com/smprather/open-eda-file-tools.git"
```

With `uv`:

```bash
uv add "open-eda-file-tools @ git+https://github.com/smprather/open-eda-file-tools.git"
```

To install all CLI tools globally with `uv`:

```bash
uv tool install "open-eda-file-tools @ git+https://github.com/smprather/open-eda-file-tools.git"
```

## Develop Locally

```bash
git clone https://github.com/smprather/open-eda-file-tools.git
cd open-eda-file-tools
uv sync --dev
uv run maturin develop --release
```

Re-run `uv run maturin develop --release` after editing any Rust file under `src/`.

## Verify

```bash
uv run pytest -q
uv run liberty-tools --version
uv run lef-tools --version
uv run def-tools --version
uv run sdc-tools --version
uv run pex-tools --version
```

All packages include `py.typed` markers and `.pyi` type stubs for static
checkers and AI coding agents.
