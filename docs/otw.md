# OTW — Open Timing Windows, version 1 (draft specification)

| | |
| --- | --- |
| Format version | `1` |
| Status | **Draft.** Implemented by `open-eda-file-tools`; may change before it is frozen. |
| File extensions | `.otw`, `.otw.gz` |
| Reference implementation | reader `src/otw.rs`, reader/writer `def_tools/otw.py` |

## 1. Introduction

OTW is an open text format for **timing windows and clock/data roles**, per net
or per pin of a design. It carries what static timing analysis hands to
power-integrity, noise and physical tools: for each net or pin, whether it is
clock or data, which clocks reach it, and when and how fast it switches.

There is no open format for this data; every tool has a private "timing window
file". OTW is designed from scratch. It is **not** a dialect of, nor
syntax-compatible with, any vendor format. It carries the same *information*
(section 11), so a converter loses nothing.

### 1.1 Design goals

1. **One declaration per object.** A net or pin is named once and states its
   own role. A consumer never has to reconcile several records for one name.
2. **Net-based and pin-based data are both first-class** and may share a file.
3. **Fast to read.** One record per line; the first byte of a line is its
   record type; no quoting, nesting or back-references. Records form
   independent groups, so a reader parses a file on all cores (section 9).
4. **Fast to write.** A writer needs no global knowledge: no name table, no
   offsets, no up-front counts. Groups can be formatted concurrently and
   appended in any order.
5. **ASCII, and readable.** A person can read a file, `grep` it, and diff two
   of them.
6. **Compact.** Clocks are referenced by index, an instance name is written
   once for all its pins, optional quantities are omitted rather than padded.
7. **Extensible without a version bump.** Any record may carry `key=value`
   attributes; a reader keeps the ones it does not know.

### 1.2 Conformance language

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are to be read as in
RFC 2119. A *writer* is anything that produces an OTW file; a *reader* is
anything that consumes one.

## 2. Data model

An OTW file describes **one design at one analysis corner**. It holds:

- a **header**: design name, units, measurement conventions, free attributes;
- an ordered list of **clocks**, each a waveform with a name. Clocks are
  numbered from 0 in declaration order; that number is the clock's *index*;
- a set of **objects**. An object is a **net**, or a **pin** of an instance (or
  of the design itself — a top-level port). Each object has exactly one
  **role** and optional attributes;
- for each object, zero or more **windows**. A window describes the switching
  of the object caused by one edge of one clock (or by no clock): when the
  rising and falling transitions can arrive, and how fast they are.

Where a source of data holds best/typical/worst values, each corner is a
separate OTW file.

## 3. File structure

### 3.1 Encoding

- A file is a sequence of 7-bit ASCII bytes. A byte above `0x7E`, or a control
  byte other than HTAB, CR and LF, MUST NOT appear; a reader MUST reject a line
  that contains a byte above `0x7F`.
- A file is a sequence of **lines**, each ended by LF. A writer MUST end lines
  with a single LF and MUST end the file with LF. A reader MUST accept CR LF.
- A line is a sequence of **tokens** separated by one or more SP or HTAB. A
  writer MUST separate tokens with exactly one SP, MUST NOT write leading or
  trailing blanks, and MUST start every record in column 0. A reader MUST
  accept any run of SP/HTAB and SHOULD accept leading and trailing blanks.
- A line that is empty, blank, or whose first non-blank byte is `#` is
  **ignorable** and may appear anywhere. A `#` elsewhere on a line is not a
  comment.

### 3.2 Order

A file is, in order: the version line, the header lines, the groups, the `end`
line. Only ignorable lines may precede the version line or follow the `end`
line. The first `n`, `i` or `end` line ends the header.

### 3.3 Groups

The body is a sequence of **groups**:

- a **net group**: one `n` line followed by that net's `w` lines;
- an **instance group**: one `i` line followed by the instance's `p` lines,
  each `p` line followed by that pin's `w` lines.

A group is self-contained. It refers to nothing outside itself except clock
indices from the header. **The order of groups carries no meaning**; a reader
MUST NOT depend on it and a writer MAY emit groups in any order.

### 3.4 Grammar

ABNF (RFC 5234). `SEP` is the token separator, `EOL` the line ending.

```abnf
otw-file     = *ignorable version *(ignorable / header-line)
               *(ignorable / group) end-line *ignorable

version      = %s"otw" SEP %s"1" EOL

header-line  = ( design / unit / thresholds / clock / free ) EOL
design       = %s"design" SEP name
unit         = ( %s"time_unit" / %s"cap_unit" / %s"res_unit" ) SEP real
thresholds   = %s"slew_thresholds" SEP real SEP real
clock        = %s"clock" SEP name SEP real SEP real SEP real attrs
free         = key [ SEP text ]

group        = net-group / inst-group
net-group    = net-line *( ignorable / window-line )
inst-group   = inst-line *( ignorable / pin-line / window-line )
net-line     = %s"n" SEP name SEP role attrs EOL
inst-line    = %s"i" SEP name EOL             ; name "-" = the design itself
pin-line     = %s"p" SEP name SEP role attrs EOL
window-line  = %s"w" SEP cause SEP role 8( SEP value ) attrs EOL
cause        = ( index SEP ( %s"r" / %s"f" ) ) / ( "-" SEP "-" )

end-line     = %s"end" SEP count SEP count SEP count EOL
                                              ; nets, pins, windows

attrs        = *( SEP key "=" avalue )
role         = %s"c" / %s"d" / %s"u" / %s"z"
value        = decimal / "-"
range        = decimal [ ":" decimal ]
decimal      = [ "-" ] 1*DIGIT [ "." 1*DIGIT ]
real         = [ "-" ] 1*DIGIT [ "." *DIGIT ] [ ( "e" / "E" ) [ "+" / "-" ] 1*DIGIT ]
index        = 1*DIGIT
count        = 1*DIGIT

name         = 1*NCHAR
key          = 1*KCHAR
avalue       = 1*NCHAR
text         = *( NCHAR / SP / HTAB )
NCHAR        = %x21-7E                         ; visible ASCII
KCHAR        = %x21-3C / %x3E-7E               ; NCHAR except "="

SEP          = 1*( SP / HTAB )
EOL          = [ CR ] LF
ignorable    = *( SP / HTAB ) [ "#" *( NCHAR / SP / HTAB ) ] EOL
```

Keywords and roles are lower case and case-sensitive. In an instance group a
`w` line belongs to the nearest `p` line above it; a `w` line directly after
the `i` line is an error.

### 3.5 Numbers

OTW has two number forms, on purpose:

- **`decimal`** — fixed point, no exponent, no `inf`/`nan`. Used for every
  number in the body that occurs in bulk: the eight window values and every
  `range`. A reader can validate one with a byte scan and convert it without a
  general floating-point parser; a writer can emit one from a scaled integer.
  A writer SHOULD choose `time_unit` so that values need few digits, and
  SHOULD NOT write more fractional digits than the data is good for.
- **`real`** — exponent allowed. Used in the header and in the `freq`
  attribute, where there are few numbers and wide ranges.

## 4. Header

| Line | Meaning |
| --- | --- |
| `otw 1` | Format version. MUST be the first non-ignorable line. A reader MUST reject a version it does not implement. |
| `design <name>` | Design name. |
| `time_unit <real>` | Seconds per unit of every time, slew and slack in the file (`1e-9` = ns). MUST be positive. |
| `cap_unit <real>` | Farads per unit of capacitance (`load`). MUST be positive. |
| `res_unit <real>` | Ohms per unit of resistance (`rise_res`, `fall_res`, `drive_rise`, `drive_fall`). MUST be positive. |
| `slew_thresholds <low> <high>` | The percentages of the signal swing a slew is measured between. MUST satisfy `0 <= low < high <= 100`. |
| `clock <name> <period> <rise_edge> <fall_edge> [attrs]` | A clock waveform, in `time_unit`. Clock names MUST be unique. |
| `<key> <text>` | Any other key is a **free attribute**, kept verbatim. |

A writer MUST write `time_unit` if the file contains any window, `res_unit`
if it writes any resistance and `cap_unit` if it writes any load. A header key
other than `clock` MUST NOT appear more than once. The keys `n`, `i`, `p`, `w`
and `end` are not available as free attribute keys.

Registered `clock` attributes:

| Attribute | Meaning |
| --- | --- |
| `source=<port>` or `source=<instance/pin>` | Where the clock enters or is generated. A clock without `source` is virtual. |
| `master=<index>` | The clock a generated clock is derived from. |

Conventional free attribute keys (a reader keeps them; none is required):
`corner` (name of the analysis view or operating condition), `process`,
`voltage`, `temperature`, `default_input_slew`, `divider`, `bus_delimiter`,
`date`, `program`.

## 5. Objects

### 5.1 Nets

```text
n <net> <role> [attrs]
```

Declares a net. A net MUST NOT be declared more than once in a file.

### 5.2 Instances and pins

```text
i <instance>
p <pin> <role> [attrs]
```

`i` opens an instance group; each following `p` line declares one pin of that
instance. The instance name `-` denotes the design itself, so its pins are the
top-level ports.

- An instance MUST NOT have more than one group in a file.
- A pin MUST NOT be declared more than once in its group.
- A writer MUST NOT write an `i` line with no `p` line; a reader MAY accept one.

### 5.3 Names

A name is a token of visible ASCII with no blanks. Names are written as they
appear in the design's DEF. DEF backslash escapes are optional: a consumer
MUST compare names with every backslash removed, so `n data[3] d` matches the
DEF net `data\[3\]`. Because the instance and the pin are separate tokens, no
hierarchy divider is needed to split a pin name.

### 5.4 Roles

| Role | Meaning |
| --- | --- |
| `c` | Clock network: a clock propagates on this object. |
| `d` | Data. |
| `u` | Unconstrained: not timed. |
| `z` | Constant: never switches. |

**Precedence rule.** An object on which *any* clock arrival exists MUST be
declared `c`, even when data arrivals exist on it too (a gated or multiplexed
clock carries both). A writer converting from a source that tags each record
separately MUST apply this rule.

### 5.5 Registered object attributes

| Attribute | Value | Meaning |
| --- | --- | --- |
| `clocks` | `index` list, comma-separated | **Clock domains**: the clocks that reach this object. Each index MUST name a declared clock. When absent, the object's domains are the clocks of its windows. |
| `freq` | `real`, `>= 0` | Switching (toggle) frequency, in hertz. |
| `load` | `range` | External capacitive load, in `cap_unit`. |
| `value` | `0` or `1` | Logic value of a constant (`z`) object. |
| `drive_rise`, `drive_fall` | `range` | Strength of whatever drives a top-level input port for the rising / falling transition, as a resistance in `res_unit`. |

A `range` is `min:max`, or a single `decimal` standing for both. Where an
analysis yields an early (hold) and a late (setup) figure, they are the `min`
and the `max`.

## 6. Windows

```text
w <clock> <edge> <role> <rise_early> <rise_late> <fall_early> <fall_late>
  <rise_slew_min> <rise_slew_max> <fall_slew_min> <fall_slew_max> [attrs]
```

(one line). A `w` line is one window of the object declared above it.

| Field | Value | Meaning |
| --- | --- | --- |
| `clock` | `index`, or `-` | The launching clock. `-`: no clock launches this arrival (an asynchronous or unconstrained source). |
| `edge` | `r`, `f`, or `-` | The launching clock edge. MUST be `-` exactly when `clock` is `-`. |
| `role` | role | Whether *this* arrival is a clock (`c`) or not (`d`, `u`, `z`). |
| `rise_early`, `rise_late` | `value` | Earliest and latest arrival of the rising transition, in `time_unit`. |
| `fall_early`, `fall_late` | `value` | Earliest and latest arrival of the falling transition. |
| `rise_slew_min`, `rise_slew_max` | `value` | Fastest and slowest rising transition time. |
| `fall_slew_min`, `fall_slew_max` | `value` | Fastest and slowest falling transition time. |

A `value` of `-` means the quantity does not exist: the transition does not
occur. Arrival times are relative to the time origin of the clock waveforms.

- A window with role `c` MUST NOT appear on an object whose role is not `c`.
- An object MAY have several windows, including several for the same clock and
  edge.
- An object MAY have no windows; its role and attributes still stand.

Registered window attributes, each a `range`:

| Attribute | Meaning |
| --- | --- |
| `rise_res`, `fall_res` | Resistance of the source driving the rising / falling transition, in `res_unit`. |
| `rise_slack`, `fall_slack` | Slack of the rising / falling transition, in `time_unit`. Omitted when no slack was computed. |

## 7. The `end` line

```text
end <nets> <pins> <windows>
```

The last record. Its three counts are the number of `n` lines, `p` lines and
`w` lines in the file.

- A writer MUST write it. Because it comes last, a writer can stream the body
  and count as it goes.
- A reader MUST reject a file with no `end` line (the file is truncated), with
  counts that do not match, or with any record after it.

## 8. Attributes and extension

Any `clock`, `n`, `p` or `w` line may end with `key=value` tokens. Nothing
else may follow the positional fields.

- A reader MUST validate the registered attributes (sections 4 to 6) and MUST
  preserve, without interpreting, any attribute it does not know.
- A key MUST NOT appear twice on one line.
- Unregistered keys are free for tools to use. A tool-specific key SHOULD carry
  a prefix naming the tool (`acme_sigma=3`), so later versions of this
  specification can register plain keys without collision.
- A change that an existing reader would misread — a new record type, a new
  positional field, a new role — requires a new format version.

## 9. Compression and parallelism

### 9.1 Compression

A `.otw.gz` file is the OTW text compressed with gzip (RFC 1952). It MAY
consist of **several gzip members concatenated**; a reader MUST read all of
them as one stream. A writer can therefore compress runs of groups
independently, on separate threads, and append the members in any order (the
header first, the `end` line last).

### 9.2 Why the format is fast

These are properties of the format, not of one implementation:

| Property | Consequence |
| --- | --- |
| The first byte of a line is its record type | A reader that wants only roles dispatches on one byte and never converts a number on a `w` line. |
| Groups are independent and unordered | A reader cuts the input at any line that starts with `n`, `i` or `e` and parses the pieces concurrently. A writer formats groups concurrently. |
| No name table, offsets or forward references | A writer streams; a reader needs no second pass. |
| Counts are in a trailer, not the header | A streaming writer knows them only at the end, and that is where they go. Truncation is still detected. |
| An instance name is written once for all its pins | Pin-based files are smaller, and a consumer does one instance lookup per group. |
| Body numbers are fixed-point decimals | Validation is a byte scan; conversion needs no general float parser; a writer needs no shortest-representation float printer. |
| 7-bit ASCII | No multi-byte decoding or validation. |
| One declaration per object | No reconciliation pass over repeated records. |

### 9.3 Measured

Reference reader (`src/otw.rs`), AMD Ryzen 9 5900X (12 cores / 24 threads),
on a synthetic 227 MB file: 1,000,000 nets, 1,000,000 pins in 250,000
instance groups, 2,000,000 windows.

| Operation | 24 threads | 1 thread |
| --- | --- | --- |
| Roles and domains of every object, plain file | 0.27 s (840 MB/s) | 0.84 s |
| Same, gzip (56 MB, single member) | 0.38 s | — |
| Every window to CSV | 0.22 s | 0.92 s |

These time the native reader. Building Python dictionaries for the two million
objects (`def_tools.otw.read_otw`) takes a further 1.6 s. An earlier,
sequential reader of the pre-grouping grammar took 1.33 s on the equivalent
file. Writers have not been benchmarked; `def_tools.otw.write_otw` is a
convenience writer, not a fast one.

## 10. Errors

A reader MUST reject a file, naming the file and, where one applies, the line,
when:

1. the first non-ignorable line is not `otw <version>`, or the version is not
   implemented;
2. a line contains a non-ASCII byte;
3. a unit is not positive, or `slew_thresholds` is not `0 <= low < high <= 100`;
4. a clock name is declared twice;
5. a role is not one of `c`, `d`, `u`, `z`;
6. a net is declared twice, an instance has two groups, or a pin is declared
   twice in its group;
7. a `p` line is outside an instance group, or a `w` line has no `n` or `p`
   line above it in its group;
8. a token after the positional fields is not `key=value`;
9. a registered attribute is malformed: `clocks` names an undeclared clock,
   `freq` is negative, `value` is not `0`/`1`, a `range` is not decimals;
10. a `w` line names an undeclared clock, has an edge that does not fit its
    clock, has fewer than eight values, or a value that is neither a `decimal`
    nor `-`;
11. a window with role `c` is on an object not declared `c`;
12. a header line appears after the first group;
13. there is no `end` line, its counts do not match the file, or a record
    follows it.

A reader MUST NOT repair or guess: a file that fails any check yields no data.

## 11. Coverage

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
| Virtual clocks (a waveform with no source) | `clock` with no `source=` |
| Per net **or** per pin records | `n` / `i` + `p` |
| Launching clock and edge of a window; no launching clock | `w <clock> <edge>`; `w - -` |
| Min/max rise and fall arrival window | `rise_early rise_late fall_early fall_late` |
| Min/max rise and fall transition time | `rise_slew_min rise_slew_max fall_slew_min fall_slew_max` |
| Min/max rise and fall source resistance | `rise_res=`, `fall_res=` |
| Min/max rise and fall slack | `rise_slack=`, `fall_slack=` |
| Absent transition / value | `-`, or the attribute is omitted |
| Clock vs data flag per record | window `<role>` |
| Clock vs data per object (and constant / unconstrained) | object `<role>`, with the precedence rule |
| Constant nets and their logic value | role `z` with `value=0` / `value=1` |
| Input-port driver strength, per edge | `drive_rise=`, `drive_fall=` on a pin of instance `-` |
| External load on a port or net | `load=` |
| Clock domain per object | `clocks=`, or derived from the windows |
| Switching frequency | `freq=` |
| Anything tool-specific (statistical sigma, derates, ...) | prefixed `key=value` attributes and free header keys |

## Appendix A — Complete example (informative)

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
n tie_lo z value=0
i u_core/u_icg
p CK c clocks=0,1
p E d
w 0 r d 0.90 1.40 0.90 1.45 0.04 0.07 0.04 0.07
i -
p clk c
p scan_in d drive_rise=1.833 drive_fall=2.084
p scan_out d load=0.0
end 3 5 4
```

## Appendix B — A binary form (informative; not part of OTW 1)

A binary encoding was considered and is **not** specified in version 1.

- *What it would buy.* The text reader already runs at about 0.8 GB/s, so
  record framing is not the bottleneck. What text cannot avoid is
  decimal-to-binary conversion for consumers of every window value, and, for
  compressed files, single-stream gzip inflation.
- *What it would cost.* A second format to specify, test and keep equivalent,
  with none of goal 5 (readable, `grep`-able, diff-able).
- *Recommendation.* If a binary form is added, it should not be a new
  container. It should be a **columnar profile**: an Apache Parquet file with
  one row per window (an object without windows contributes one row), the
  header in the file's key-value metadata, and these columns —
  `kind`, `instance`, `name`, `role`, `clocks`, `freq`, `load_min`,
  `load_max`, `value`, `clock`, `edge`, `window_role`, the eight window values,
  and the four ranges as `_min` / `_max` pairs. Dictionary encoding makes the
  repeated object columns nearly free, row groups give parallel and partial
  reads, and every analysis environment reads it without an OTW library.
  `def-tools clock-data --report windows` already emits this table as CSV.

Text remains the interchange form; a columnar profile would be a derived,
lossless cache of it.

## Appendix C — Use in `open-eda-file-tools` (informative)

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
