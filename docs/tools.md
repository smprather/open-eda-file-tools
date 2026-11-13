# Tools Overview

`open-eda-file-tools` is a single project (`pyproject.toml`, `Cargo.toml`, and
one UTC CalVer version) shipping five EDA file-format libraries and their
command-line front ends:

| Domain | Python Library | CLI Entry Points | Guide |
| --- | --- | --- | --- |
| Liberty (`.lib`, `.lib.gz`) | `liberty_tools`, `liberty_format`, `viewer` | `liberty-tools format`, `liberty-format`, `liberty-view` | [`docs/liberty.md`](liberty.md), [`docs/ai-api.md`](ai-api.md), [`docs/cli.md`](cli.md) |
| Cell LEF (`.lef`, `.lef.gz`) | `lef_tools` | `lef-tools` | [`docs/lef.md`](lef.md) |
| DEF (`.def`, `.def.gz`) | `def_tools` | `def-tools` | [`docs/def.md`](def.md) |
| OTW (`.otw`, `.otw.gz`) — open timing windows, clock/data roles | `def_tools.otw`, `def_tools.clock_data` | `def-tools` | [`docs/otw.md`](otw.md) |
| SDC (`.sdc`) | `sdc_tools` | `sdc-tools` | [`docs/sdc.md`](sdc.md) |
| SPEF / DSPF (`.spef`, `.dspf`, `.gz`) | `pex_tools` | `pex-tools` | [`docs/pex.md`](pex.md) |

## Adding a Tool

1. Create the Python package directory at the repository root (`<name>_tools/`)
   with `__init__.py`, `cli.py`, `version.py`, and `py.typed`, and add it to
   `[tool.maturin] include` in `pyproject.toml`.
2. Register the CLI script under `[project.scripts]` in `pyproject.toml`.
3. If it has a Rust backend, add its module under `src/`, register its PyO3
   classes/functions in `_native` (`src/lib.rs`), and declare their types in
   `liberty_tools/_native.pyi`.
4. Add tests under `tests/` and document the Python and CLI interfaces in
   `docs/ai-api.md` and `README.md`.

## Documentation Synchronization Mandate

All documentation (`docs/ai-api.md`, `docs/cli.md`, `docs/*.md`, `README.md`,
`CLAUDE.md`, and `.pyi` type stubs) **must be kept in sync with the code at all
times**. Every new or changed CLI flag, subcommand, output schema, or Python API
must update its documentation and type stubs in the same commit.
