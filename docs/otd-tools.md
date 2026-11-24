# OTD in open-eda-file-tools

The [OTD 1 specification](otd.md) defines the interchange format. The
[overview](otd-overview.md) introduces the format and describes how to review
an implementation.

## Implementation status

The current reader and Python convenience writer support object roles, driver
references, clocks, windows and free header attributes. Support for `a` arc
records is pending. The Python writer also still needs the required opening
field guide. Use the specification to evaluate conformance to the full draft.

The Python API preserves `export_scope` as text in `OtdFile.attributes`.
[Export scope](otd.md#42-export-scope).

The reader detects gzip from the file contents. The `def-tools` backend
selector uses the filename or an explicit `otd:` prefix.

## Clock classification and window inspection

```bash
def-tools instances --lef cells.lef --def design.def --lib cells.lib \
    --clock-data design.otd.gz --output instances.csv
```

`def-tools instances` fills `is_clock` from the `c` objects:

- A clock **net** marks the instances that *drive* it, including a divider
  flip-flop whose output carries a generated clock.
- A clock **pin** marks its instance through an output or a clock-through
  input. Endpoint cells qualify through outputs only. The clock input of an
  integrated clock gate qualifies.
- An instance with several nets/pins is a clock cell when any one marks it.

Pin-based files identify each pin directly and can distinguish a gate's clock
pin from its enable. Clock classification uses the object roles and the
design's pin directions and connectivity. The API exposes domains and object
attributes, and the window report exposes individual timing windows.
[DEF tools guide](def.md#clock-sources-for-is_clock).

```python
from def_tools.otd import OtdClock, OtdWindow, read_otd, write_otd

write_otd(
    "design.otd.gz",
    design="top", time_unit=1e-9, cap_unit=1e-12, res_unit=1e3,
    slew_thresholds=(10, 90),
    attributes=[("corner", "ss_0p72v_125c")],
    clocks=[OtdClock("clk", 5.0, 0.0, 2.5, (("source", "clk"),))],
    nets={"clk_leaf_3": "c", "data_bus[3]": "d"},
    pins={("u_core/u_icg", "CK"): "c", ("-", "clk"): "c"},
    net_drivers={"clk_leaf_3": ("u_core/u_icg", "GCK")},
    domains={"clk_leaf_3": [0]},
    object_attributes={"clk_leaf_3": {"load": "0.002"}},
    windows={
        "data_bus[3]": [
            OtdWindow(0, "r", "d", (1.20, 3.75, 1.18, 3.80, 0.05, 0.11, 0.05, 0.12),
                      rise_slack=(0.31, 0.37), fall_slack=(0.29, 0.36)),
            OtdWindow(None, "-", "u", (0.40, 0.90, None, None, 0.05, 0.08, None, None)),
        ],
    },
)
otd = read_otd("design.otd.gz")   # header + per-object data; windows are counted, not loaded
otd.nets                          # {"clk_leaf_3": "c", "data_bus[3]": "d"}
otd.pins                          # {("u_core/u_icg", "CK"): "c", ("-", "clk"): "c"}
otd.net_drivers                   # {"clk_leaf_3": ("u_core/u_icg", "GCK")}
otd.domains                       # {"clk_leaf_3": (0,), "data_bus[3]": (0,)}  (second derived from its window)
otd.domain_names("clk_leaf_3")    # ("clk",)
otd.object_attributes             # {"clk_leaf_3": {"load": "0.002"}}
```

`def-tools clock-data --source design.otd.gz --report windows` streams every
window as CSV, including the resistance and slack ranges. The
`object_attrs` column carries the driver pair when supplied.

## Reader benchmark

Reference reader (`src/otd.rs`), AMD Ryzen 9 5900X (12 cores / 24 threads),
on a synthetic 227 MB file: 1,000,000 nets, 1,000,000 pins in 250,000
instance groups, 2,000,000 windows.

| Operation | 24 threads | 1 thread |
| --- | --- | --- |
| Roles and domains of every object, plain file | 0.27 s (840 MB/s) | 0.84 s |
| Same, gzip (56 MB, single member) | 0.38 s | — |
| Every window to CSV | 0.22 s | 0.92 s |

These time the native reader. Building Python dictionaries for the two million
objects (`def_tools.otd.read_otd`) takes a further 1.6 s. An earlier,
sequential reader of the pre-grouping grammar took 1.33 s on the equivalent
file. The benchmark did not measure write performance.
