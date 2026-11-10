# Versioning Plan

`open-eda-file-tools` is a single project with one `pyproject.toml`, one
`Cargo.toml`, and one version. It ships several libraries (`liberty_tools`,
`lef_tools`, `def_tools`) and command-line tools (`liberty-view`,
`liberty-format`, `lef-tools`, `def-tools`) from the same source tree, the same
release, and the same version number.

| File | Role |
| --- | --- |
| `pyproject.toml` | Project version (`[project] version`), dependencies, and CLI entry points |
| `Cargo.toml` | Native extension crate version (`[package] version`) |
| `uv.lock`, `Cargo.lock` | Locked Python and Rust dependency graphs |
## Scheme

Releases use the same CalVer as `watchglass`: the release date, `YYYY.M.D`,
without zero padding, with a same-day ordinal when the project ships more than
once in a day. Every version is valid PEP 440, so it can be uploaded to PyPI.

```text
2026.11.5        # release on 5 November 2026
2026.11.5.1      # second release that day
```

There are no per-tool versions and no development versions:

- `pyproject.toml` and `Cargo.toml` at the repository root carry the project
  version, and `uv.lock` and `Cargo.lock` record it.
- There is no `-dev` or `.dev0` suffix and no post-release bump: `main` keeps
  the last released version until the next release commit sets the new date.
  The release date is not known in advance, so a development version would
  have to guess it.

## Cargo and the Fourth Component

Cargo accepts SemVer, which has exactly three numeric components. A fourth
fails with `unexpected character '.' after patch version number`, so the
Cargo version cannot be `2026.11.5.1`.

The project therefore uses the full version on the Python side and the
three-component date on the Cargo side for a same-day follow-up:

```toml
# Cargo.toml (workspace) and Cargo.lock
version = "2026.11.5"

# pyproject.toml, uv.lock and the wheel
version = "2026.11.5.1"
```

The three components must match across the two. maturin takes the wheel
version from `pyproject.toml` and accepts the difference: a wheel built from
`2026.11.5.1` in `pyproject.toml` with `2026.11.5` in `Cargo.toml` reports
`Version: 2026.11.5.1` in its metadata. Only the same-day ordinal is ever
missing from Cargo.

## Release Tags

One tag covers the whole project:

```text
vYYYY.M.D[.N]
```

Examples:

```text
v2026.11.5
v2026.11.5.1    # second release on 2026-11-05
```

`D` is the day of release without zero padding. `N` starts at `1` for the
second release on that day and increments for each additional one. Release
tags carry no suffix and point at commits where the project is at the released
version.

Tags from the previous month-sequence scheme (`v2026.11.1` … `v2026.11.5`) and
the older date-based `vYYYY.MM.DD.N` form (`v2026.06.01.1`) are historical and
are left as they are.

## CLI Version Output

Every tool prints the project version:

```text
liberty-format 2026.11.9
lef-tools, version 2026.11.9
def-tools, version 2026.11.9
```
