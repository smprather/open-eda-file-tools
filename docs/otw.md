# OTW — Open Timing Windows (draft 1)

OTW is an open, line-oriented text format for **timing windows and clock/data
roles**, per net or per pin. It exists because there is no open format for this
data: every STA and power-integrity tool has a private "timing window file"
dialect. OTW is designed from scratch and is **not** a dialect of, nor
syntax-compatible with, any vendor format. It aims to carry the same
*information* (see [Coverage](#coverage)), so a converter loses nothing.

Status: **draft**. The grammar below is what `def_tools` reads and writes
today (`otw 1`); it is expected to evolve before it is frozen.

Files are UTF-8 text, `.otw`, or gzip-compressed `.otw.gz`.

## Design goals

- **One declaration per object.** A net or pin is named once and states its own
  role. Formats that repeat an object once per launching clock edge leave the
  reader to arbitrate conflicting clock/data flags; OTW has no such conflict.
- **Net-based and pin-based data are both first-class** and may share a file.
- **Cheap to read.** One record per line, whitespace-separated tokens, a
  one-character record type, no quoting and no nesting. A reader that only
  needs roles never parses a number on a `w` line.
- **Compact.** Clocks are declared once and referenced by index; an object's
  name is never repeated for its windows; optional quantities are omitted, not
  written as placeholders.
- **Unambiguous pins.** Instance and pin are separate tokens, so no hierarchy
  divider has to be guessed.
- **Extensible without a version bump.** Any record may end with `key=value`
  attributes; a reader keeps the ones it does not know.

## Example

```text
# net-based and pin-based objects in one file
otw 1
design top
time_unit 1e-9
cap_unit 1e-12
res_unit 1e3
slew_thresholds 10 90
corner ss_0p72v_125c
program my_sta 2026.11
clock clk 5 0 2.5 source=clk
clock clk_div2 10 0 5 master=0 source=u_div/Q
n clk_leaf_3 c clocks=0 freq=2e8
w 0 r c 0.582 0.598 0.313 0.344 0.045 0.064 0.063 0.064 rise_res=0.59:0.61
n data_bus[3] d
w 0 r d 1.20 3.75 1.18 3.80 0.05 0.11 0.05 0.12 rise_slack=0.31:0.37 fall_slack=0.29:0.36
w - - u 0.40 0.90 - - 0.05 0.08 - -
n tie_lo z
p u_core/u_icg CK c clocks=0,1
p u_core/u_icg E d
p - clk c
```

## Grammar

Blank lines and lines whose first non-blank character is `#` are ignored.
Tokens are separated by spaces or tabs. `clock`, `n`, `p` and `w` lines may end
with any number of `key=value` attribute tokens (no spaces around `=`); after
the positional fields nothing else is allowed.

### Line 1: version

```text
otw <version>
```

Must be the first non-comment line. This draft is version `1`; a reader must
reject a version it does not know.

### Header (before the first object)

| Line | Meaning |
| --- | --- |
| `design <name>` | Design name. |
| `time_unit <seconds>` | Unit of every time, slew and slack, as a positive float (`1e-9` = ns). |
| `cap_unit <farads>` | Unit of capacitances (`load=`). |
| `res_unit <ohms>` | Unit of resistances (`rise_res=`, `fall_res=`). |
| `slew_thresholds <low%> <high%>` | Swing percentages a slew is measured between (`10 90`). |
| `clock <name> <period> <rise_edge> <fall_edge> [key=value ...]` | A clock waveform. Clocks are numbered from 0 in declaration order. Conventional attributes: `source=<port or inst/pin>`, `master=<clock index>` for a generated clock. |
| `<key> <rest of line>` | Any other key is a free attribute, kept verbatim. Conventional keys: `corner` (analysis view / operating condition), `process`, `voltage`, `temperature`, `default_input_slew`, `divider`, `bus_delimiter`, `date`, `program`. |

A file describes **one** analysis corner. Where another format packs
best/typical/worst values into one file, write one OTW file per corner.
Header lines are not allowed after the first object.

### Objects

```text
n <net> <role> [key=value ...]
p <instance> <pin> <role> [key=value ...]
```

- `n` declares a net; `p` declares a pin of an instance. Instance `-` means a
  top-level port (`p - clk c`).
- An object may be declared **at most once**; a duplicate is an error.
- Names are whitespace-free tokens written as in the design's DEF. DEF
  backslash escapes are optional: readers compare names with backslashes
  removed, so `n data[3] c` matches DEF net `data\[3\]`.

Object attributes:

| Attribute | Meaning |
| --- | --- |
| `clocks=<i>[,<j>...]` | **Clock domains**: indices of the clocks that reach this object. When absent, the domains are the clocks of the object's windows. |
| `freq=<hertz>` | Switching (toggle) frequency of the object. |
| `load=<value>` | Capacitive load, in `cap_unit`. |
| anything else | Kept verbatim. |

### Roles

| Role | Meaning |
| --- | --- |
| `c` | Clock network: a clock propagates here. |
| `d` | Data. |
| `u` | Unconstrained / not timed. |
| `z` | Never switches (constant). |

**Precedence rule.** An object on which *any* clock arrival exists is `c`, even
when data arrivals exist too (a gated or muxed clock net carries both). A
writer converting from a source that tags each record separately must apply
this rule; a reader enforces it (below).

### Windows

```text
w <clock> <edge> <role> <rise_early> <rise_late> <fall_early> <fall_late> <rise_slew_min> <rise_slew_max> <fall_slew_min> <fall_slew_max> [key=value ...]
```

A `w` line is one timing window of the object declared above it, caused by one
edge of one clock. Windows are optional; an object may have none.

- `<clock>` — index of a declared `clock`, or `-` for an arrival that no clock
  launches (asynchronous or unconstrained input); `<edge>` is then `-` too.
- `<edge>` — `r` or `f`: the launching clock edge.
- `<role>` — whether *this* window is a clock (`c`) or data (`d`, `u`, `z`)
  arrival. A `c` window on an object not declared `c` is an error.
- Eight values in `time_unit`: the earliest/latest arrival of the rising and
  falling transition, then the min/max slew (transition time) of each. `-`
  means the transition does not occur.

Window attributes; each value is a `min:max` range, or one number for both:

| Attribute | Meaning |
| --- | --- |
| `rise_res=`, `fall_res=` | Resistance of the source driving the rising / falling transition, in `res_unit`. |
| `rise_slack=`, `fall_slack=` | Slack of the rising / falling transition, in `time_unit`. Omit when no slack was computed. |
| anything else | Kept verbatim. |

## Errors a reader must raise

Not starting with `otw <known version>`; a non-positive unit; thresholds not
`0 <= low < high <= 100`; an unknown role; a duplicate object; a trailing token
that is not `key=value`; `clocks=` naming an undeclared clock; a negative
`freq`; a `w` line before any object, with an undeclared clock index, a bad
edge, fewer than eight values, or a malformed range attribute; a clock window
on a non-clock object; a header line after the first object. Errors name the
file and line.

## Coverage

What timing-window files in the field carry, and where it lives in OTW:

| Information | OTW |
| --- | --- |
| Design name, producing program, date | `design`, `program`, `date` |
| Time / capacitance / resistance scale | `time_unit`, `cap_unit`, `res_unit` |
| Slew measurement thresholds | `slew_thresholds` |
| Process, voltage, temperature, operating condition / view | `process`, `voltage`, `temperature`, `corner` (one corner per file) |
| Default input slew | `default_input_slew` |
| Hierarchy divider, bus delimiters | Not needed for pins (instance and pin are separate tokens); `divider`, `bus_delimiter` for net names |
| Clock waveforms: period, rise and fall edge | `clock` |
| Clock source, generated-clock master | `clock ... source= master=` |
| Per net **or** per pin records | `n` / `p` |
| Launching clock and edge of a window; no launching clock | `w <clock> <edge>`; `w - -` |
| Min/max rise and fall arrival window | `rise_early rise_late fall_early fall_late` |
| Min/max rise and fall transition time | `rise_slew_min rise_slew_max fall_slew_min fall_slew_max` |
| Min/max rise and fall source resistance | `rise_res=`, `fall_res=` |
| Min/max rise and fall slack | `rise_slack=`, `fall_slack=` |
| Absent transition / value | `-`, or the attribute is omitted |
| Clock vs data flag per record | window `<role>` |
| Clock vs data per object (and constant / unconstrained) | object `<role>`, with the precedence rule |
| Clock domain per object | `clocks=`, or derived from the windows |
| Switching frequency, external load | `freq=`, `load=` |
| Anything tool-specific (statistical sigma, derates, ...) | free `key=value` attributes and header keys |

## How `def-tools` uses OTW

`def-tools instances --clock-data design.otw.gz --lib cells.lib` fills
`is_clock` from the `c` objects:

- A clock **net** marks the instances that *drive* it, except flip-flops,
  latches and memories — a divider flop driving a generated clock is a clock
  source, not a clock-tree cell.
- A clock **pin** marks its instance when the instance is not such an endpoint
  and the pin is an output or a clock-through input (for an integrated clock
  gate: the clock pin, never the enable).
- An instance with several nets/pins is a clock cell when any one marks it.

Pin-based files are recommended when available: they need no connectivity
inference and they distinguish a gate's clock pin from its enable. Windows,
domains and the other attributes are read and exposed but do not affect
`is_clock`. See [def.md](def.md) for the options, exit codes and the plug-in
backend seam.

## Python

```python
from def_tools.otw import OtwClock, OtwWindow, read_otw, write_otw

write_otw(
    "design.otw.gz",
    design="top", time_unit=1e-9, cap_unit=1e-12, res_unit=1e3,
    slew_thresholds=(10, 90),
    attributes=[("corner", "ss_0p72v_125c")],
    clocks=[OtwClock("clk", 5.0, 0.0, 2.5, (("source", "clk"),))],
    nets={"clk_leaf_3": "c", "data_bus[3]": "d"},
    pins={("u_core/u_icg", "CK"): "c", ("-", "clk"): "c"},
    domains={"clk_leaf_3": [0]},
    object_attributes={"clk_leaf_3": {"freq": "2e8"}},
    windows={
        "data_bus[3]": [
            OtwWindow(0, "r", "d", (1.20, 3.75, 1.18, 3.80, 0.05, 0.11, 0.05, 0.12),
                      rise_slack=(0.31, 0.37), fall_slack=(0.29, 0.36)),
            OtwWindow(None, "-", "u", (0.40, 0.90, None, None, 0.05, 0.08, None, None)),
        ],
    },
)
otw = read_otw("design.otw.gz")   # header + per-object data; windows are counted, not loaded
otw.nets                          # {"clk_leaf_3": "c", "data_bus[3]": "d"}
otw.pins                          # {("u_core/u_icg", "CK"): "c", ("-", "clk"): "c"}
otw.domains                       # {"clk_leaf_3": (0,), "data_bus[3]": (0,)}  (second derived from its window)
otw.domain_names("clk_leaf_3")    # ("clk",)
otw.object_attributes             # {"clk_leaf_3": {"freq": "2e8"}}
```

`def-tools clock-data --source design.otw.gz --report windows` streams every
window as CSV, including the resistance and slack ranges.
