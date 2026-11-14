# OTW design workspace

Working area for the next version of OTW (Open Timing Windows). Nothing here
is normative and nothing here is shipped.

| What | Where |
| --- | --- |
| Normative v1 spec | `docs/otw.md` |
| v1 reader (Rust) | `src/otw.rs` |
| v1 reader/writer (Python) | `def_tools/otw.py` |
| Candidate additions for the next version | `otw-design/ideas.md` |

## Goal

1. Decide which additional STA results OTW should carry (`ideas.md`).
2. Design them into the format and update the spec, reader and writer.
3. Write an OTW writer inside an open-source STA tool and submit it upstream
   as a pull request.

## Status (2026-11-14)

- v1 draft is specified and implemented. It has not been validated against a
  real design.
- The candidate list is saved, unranked and undesigned. No grammar exists for
  any of it.
- Paused while other work is done.

## Open decisions

- **Which candidates go in.** See the source column in `ideas.md`: several
  are activity data, not STA results, and may not belong in this file at all.
- **Clock domains on instances.** How a domain reaches the instance table
  (`clock_domain` column) and which domain wins when an instance sees several.
- **Vector data.** Attributes are scalar, so a histogram needs a new record
  type, which is a format version bump.
- **Group references.** Anything that names a set of objects must be declared
  in the header and referenced by index, or the record groups stop being
  independent and the parallel reader breaks.
- **Binary profile.** `docs/otw.md` Appendix B recommends a Parquet columnar
  profile and does not specify it.
- **Target STA tool.** Not chosen. OpenSTA (used by OpenROAD) is the obvious
  candidate; its licence and contribution terms have not been checked.

## Rules that carry over from v1

- Designed from scratch. Do not shape it after a vendor file.
- One analysis corner per file.
- ASCII, line-oriented and readable; fast to read and to write.
- Record groups are independent and unordered.
