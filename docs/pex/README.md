# PEX (SPEF / DSPF) documentation notes

Reference material for `pex-tools`, the parasitic-exchange parser. Every file
here is redistributable; sources that are not open are kept outside the
repository (see `local/`, which is gitignored).

## Included

| Source | License | Contents |
| --- | --- | --- |
| [OpenTimer Parser-SPEF](https://github.com/OpenTimer/Parser-SPEF) | MIT | `opentimer-parser-spef/README.md`: a fast header-only SPEF parser's data model and the format's section layout. `LICENSE` is the upstream license. |

## Referenced, not included

| Source | License | Why it is local-only |
| --- | --- | --- |
| IEEE 1481-1998/2001/2009, *Standard for Delay and Power Calculation System* | proprietary | The authoritative SPEF standard; sold by IEEE, not redistributable. |
| [OpenSTA `SpefParse.yy` / `SpefLex.ll`](https://github.com/The-OpenROAD-Project/OpenSTA) | GPL-3.0-or-later | A complete SPEF/DSPF grammar and lexer. Kept in `local/` for reference only; do not copy code into this repository. |
| [Wikipedia: Standard Parasitic Exchange Format](https://en.wikipedia.org/wiki/Standard_Parasitic_Exchange_Format) | CC BY-SA 4.0 | Share-alike text; kept in `local/` rather than mixed into the repository. |
| Cadence DSPF manuals | proprietary | DSPF is described in Cadence documentation; kept as links/notes only. |

## Format coverage

`pex-tools` reads the SPEF/DSPF structures this project needs:

- header (`*SPEF` / `*DSPF`, design, vendor, units, divider/delimiter),
- `*NAME_MAP`, `*PORTS`, power/ground net lists,
- `*D_NET` / `*R_NET` blocks with `*CONN`, `*CAP`, `*RES`, `*INDUC`, `*END`,
- DSPF connection attributes `*C x y`, `*L`, `*S`, `*D cell`,
- `$` extension comments (including `layer=`, `x=`, `y=` pairs),
- `//` and `/* ... */` comments used by OpenROAD-family writers.

Physical (`*D_PNET`/`*R_PNET`) and reduced-RSPF sections are skipped
deliberately in the first version.
