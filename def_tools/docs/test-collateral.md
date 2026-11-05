# Test collateral

This repository stores **no** design or reference files. Test inputs are
generated on demand by `tests/gen/design_gen.py`. This document records
what the generator is modelled on, so its fidelity can be re-checked
without the original files.

## Why nothing is stored

| File | Size | Why it is not in the repo |
|---|---|---|
| `17-fill.def.gz` | 51 MB | Past GitHub's 50 MB warning threshold, and permanent in history once pushed. Its *shape* is reproducible; its bytes carry no information a generator cannot. |
| `merged.min.lef` | 1.4 MB | Derived from the SkyWater sky130 PDK. Only 41 of its 442 macros are ever placed. |
| `base.sdc` | 16 KB | Design constraints for the same block. |
| `lefdefref.pdf` | 2.5 MB | Cadence LEF/DEF Language Reference — vendor documentation, redistribution rights not established. |
| `des_constraints__ug.pdf` | 1.4 MB | Synopsys Design Constraints user guide — same. |

The two PDFs are specifications, not test inputs; nothing executes them.
Keep them outside the repo and consult them locally.

## Reference design

The generator reproduces the shape of a post-fill DEF from an OpenROAD
sky130hd flow:

- design `swerv_wrapper`, site `unithd` (0.46 × 2.72 µm), 1000 DBU/µm
- die 5,102,215 × 5,112,935 DBU (≈5.1 mm square), 3,163 row segments
- **4,291,871 lines / 576,823,478 bytes** uncompressed, 51 MB gzipped
- 134.4 bytes per line, **223 bytes of DEF per instance**

### Section proportions

| Section | Records | Lines | Share of file |
|---|---:|---:|---:|
| `COMPONENTS` | 2,585,205 | 2,585,205 | 60.2% |
| `SPECIALNETS` | 2 | 1,503,752 | 35.0% |
| `NETS` | 185,715 | 193,818 | 4.5% |
| `PINS` | 1,419 | 5,898 | 0.14% |
| `VIAS` | 4 | 4 | — |

Two properties dominate the file size and are the ones worth reproducing:

1. **92.85% of instances are fill, decap or tap cells.** A single macro
   (`decap_12`) is 45.8% of the design. Logic is 7.10%.
2. **The two PG nets name most instances twice** — once on the rail pin
   and once on the well pin (`VPWR`+`VPB`, `VGND`+`VNB`); tap cells expose
   only the rail. That is 9,670,764 connections wrapped exactly 8 per
   line, which is why two nets occupy a third of the file.

### Other measured statistics

- Orientations alternate by row: `N` 1,296,142 / `FS` 1,285,900, with
  1,583 `S` and 1,580 `FN`.
- Component status: 2,243,858 `PLACED`, 341,347 `FIXED`. 92% of records
  carry `+ SOURCE DIST`.
- Signal-net pin counts (driver included): 2 pins 118,807 nets; 3 pins
  23,434; 7 pins 20,275; 4 pins 10,970; 5 pins 4,820; 6 pins 3,665; a
  thin tail past 8.
- Signal nets are **unrouted** at this stage — only `SPECIALNETS` carry
  geometry (`FOLLOWPIN` rails on met1, `STRIPE` straps on met4/met5).
- 229 distinct macros; the generator models 41, with the tail's instance
  count folded into the logic cells so the mix stays right.

## Generating a design

```bash
# small, for interactive work
python -m tests.gen.design_gen -o /tmp/design -n 20000 --no-gzip

# large, comparable to the reference design
python -m tests.gen.design_gen -o /tmp/big -n 2500000
```

Each run writes `design.lef`, `design.def[.gz]`, `design.sdc` and
`manifest.json`. The manifest is the generator's record of what it wrote
— instance counts, per-macro histogram, die area, and exact expected
bounding boxes for a spread of instances. **Tests assert against the
manifest rather than recomputing expectations**, so a generator bug
surfaces as a mismatch instead of silently agreeing with a parser bug.

Output is a pure function of `(instances, seed)`. The same spec produces
byte-identical files on any machine.

## Fidelity of the generated design

Measured at 20,000 instances against the reference proportions:

| Metric | Generated | Reference |
|---|---:|---:|
| bytes per line | 134.5 | 134.4 |
| bytes per instance | 211 | 223 |
| `COMPONENTS` share | 63.7% | 60.2% |
| `SPECIALNETS` share | 30.9% | 35.0% |
| `NETS` share | 4.3% | 4.5% |
| non-logic instances | 92.9% | 92.85% |
| nets per instance | 0.068 | 0.072 |

`tests/test_generated_design.py` pins these; if the generator drifts, it
fails there rather than quietly changing what the benchmarks measure.

## If a real file is ever needed again

Some regressions genuinely resist synthesis — a vendor-emitted construct
that has not been characterised yet. Keep such a file outside the repo
and point tests at it:

```python
root = os.environ.get("LEFDEF_TOOLS_COLLATERAL")
if not root:
    pytest.skip("set LEFDEF_TOOLS_COLLATERAL to run real-design tests")
```

Record its sha256, size, provenance and *what it covers* here. The record
is the durable artefact; the bytes are replaceable. Reduce the file to a
minimal fixture as soon as the construct is understood, and delete the
entry.
