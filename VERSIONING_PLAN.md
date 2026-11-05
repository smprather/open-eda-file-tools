# Versioning Plan

The repository ships three distributions, each with its own version:

| Distribution | Version lives in | Scheme |
| --- | --- | --- |
| `liberty-tools` | root `Cargo.toml` and `pyproject.toml` | SemVer |
| `lef-tools` | `lef_tools/Cargo.toml` and `lef_tools/pyproject.toml` | `YYYY.M.N` (CalVer, also valid SemVer) |
| `def-tools` | `def_tools/Cargo.toml` and `def_tools/pyproject.toml` | `YYYY.M.N` (CalVer, also valid SemVer) |

The Cargo and Python versions of one distribution must stay in sync: maturin
takes the wheel version from `pyproject.toml`, while Cargo uses `Cargo.toml`.

GitHub release tags are repository-wide; a tag names a point where every
distribution is at a stable version. A tool that did not change can keep its
previous version across consecutive repository releases.

## Cargo Version

Each package version must remain valid SemVer because Cargo enforces SemVer
syntax.

Release commits use stable versions:

```toml
version = "1.0.0"          # root: liberty-tools
version = "2026.11.0"      # lef_tools: lef-tools, def_tools: def-tools
```

After a release, `main` is bumped to the next development version:

```toml
version = "1.0.1-dev"
```

Patch, minor, and major changes follow SemVer:

- Patch: backward-compatible bug fixes.
- Minor: backward-compatible new functionality.
- Major: incompatible CLI, output, or behavior changes.

Default post-release bump is patch `-dev`. Bump minor only when the next cycle
has added backward-compatible functionality. Bump major only for breaking
changes. `lef-tools` and `def-tools` use the same CalVer form for releases and
bump the last component for development builds.

## Release Tags

GitHub releases use month-based tags:

```text
vYYYY.M.N[-suffix]
```

Example:

```text
v2026.10.1
```

`N` starts at `1` for the first release of a month and increments for each
additional release in the same month.

The optional `[-suffix]` marks a tag that is not a release — for example
`v2026.10.1-dev` for a development snapshot. Release tags carry no suffix.

Release tags should point at commits where `Cargo.toml` contains a stable
SemVer version, not a `-dev` version.

Tags released before this scheme (for example `v2026.06.01.1`) use the older
date-based `vYYYY.MM.DD.N` form and are left as they are.

## CLI Version Output

`liberty-format --version` prints the Liberty package version, `lef-tools
--version` the LEF version, and `def-tools --version` the DEF version:

```text
liberty-format 1.0.1-dev
lef-tools, version 2026.11.0
def-tools, version 2026.11.0
```

On release-tagged commits, each should print the stable version for its
distribution.
