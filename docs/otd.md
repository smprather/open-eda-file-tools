# Open Timing Data (OTD) 1

| | |
| --- | --- |
| Status | **Community review draft.** The format may change before version 1 is frozen. |
| Format version | `1` (`otd 1` on disk) |
| File extensions | `.otd`, `.otd.gz` |

The specification defines the interchange format for independently developed
EDA tools. Start with the [overview and review guide](otd-overview.md) for an
annotated example and topics for community review.

[Data model](#2-data-model) · [Grammar](#34-grammar) · [Header](#4-header) ·
[Objects](#5-objects) · [Windows and arcs](#6-windows) ·
[Reader errors](#10-errors) · [Complete example](#appendix-a--complete-example-informative)

## 1. Introduction

OTD is an open text format for exchanging **timing windows and clock/data
roles**, per net or per pin of a design. Static timing analysis can supply
this information to power-integrity, noise and physical-design tools. A file
can identify which clocks reach an object, when and how fast it switches, and
its slack, source resistance or contributing cell-arc delays when available.

OTD defines the representation of those results. Timing analysis algorithms,
the input constraints and netlist, and complete timing path reports are
outside its scope.

### 1.1 Design goals

1. **One declaration per object.** A net or pin is named once and states its
   own role. A consumer never has to reconcile several records for one name.
2. **Net-based and pin-based data are both first-class** and may share a file.
3. **Fast to read.** Writers begin each record line with its type. The
   grammar has no quoted fields, and a reader can process independent body
   groups in parallel (section 9).
4. **Fast to write.** A writer declares clocks in the header. The body needs
   no global name table, offsets or up-front counts. Groups can be formatted
   concurrently and appended in any order.
5. **ASCII, and readable.** A person can read a file, `grep` it, and diff two
   of them.
6. **Compact.** Clocks are referenced by index, an instance name is written
   once for all its pins, optional quantities are omitted rather than padded.
7. **Extensible without a version bump.** The `clock`, `n`, `p`, `w` and `a`
   records may carry `key=value` attributes. A reader keeps unknown attributes.

### 1.2 Conformance language

The uppercase keywords MUST, MUST NOT, SHOULD, SHOULD NOT and MAY express
requirements or permissions as defined in BCP 14. A *writer* produces an OTD
file and a *reader* consumes one. Conformance is defined by the requirements
in this document. Sections and appendices marked informative explain the
format or show examples. [RFC 2119](https://www.rfc-editor.org/info/rfc2119)
and [RFC 8174](https://www.rfc-editor.org/info/rfc8174).

## 2. Data model

An OTD file describes **one design at one analysis corner**. It holds:

- a **header**: design name, units, measurement conventions, free attributes;
- an ordered list of **clocks**, each a waveform with a name. Clocks are
  numbered from 0 in declaration order; that number is the clock's *index*;
- a set of **objects**. An object is a **net**, or a **pin** of an instance (or
  of the design itself — a top-level port). Each object has exactly one
  **role** and optional attributes;
- for each object, zero or more **windows**. A window describes the switching
  of the object caused by one edge of one clock (or by no clock): when the
  rising and falling transitions can arrive, and how fast they are.
- for each window, zero or more **cell-arc observations**, which identify the
  contributing input pin and describe its delay to the output pin.

Where a source of data holds best/typical/worst values, each corner is a
separate OTD file.

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

Writers MUST begin the file's uncompressed text with the following field
guide. Readers handle it as ordinary comments. Extra comments may follow it.

```text
# OTD 1 field guide. Brackets enclose optional fields.
# otd version
# design name
# time_unit seconds_per_unit
# cap_unit farads_per_unit
# res_unit ohms_per_unit
# slew_thresholds low_pct high_pct
# clock name period rise_edge fall_edge [key=value ...]
# n net role [key=value ...]
# i instance (- for top-level ports)
# p pin role [key=value ...]
# w launch_clock launch_edge role rise_early rise_late fall_early fall_late rise_slew_min rise_slew_max fall_slew_min fall_slew_max [key=value ...]
# a output_edge input_pin input_edge delay [tags=list] [output_arrival=time] [key=value ...]
# end net_count pin_count window_count
# Roles: c clock, d data, u unconstrained, z constant. Edges: r rise, f fall.
# On w, - marks an absent launch clock, launch edge, or transition value.
# Clock indices start at 0. Arrivals, slews, and delays use time_unit.
# Common object attrs: clocks (indices), load, value, drive_rise, drive_fall.
# Window attrs include rise_res, fall_res, rise_slack, and fall_slack.
# A range uses min:max or one value for both bounds.
# Arc tags min_delay and max_delay describe the extrema for the output edge.
```

After comments and blank lines are skipped, the file starts with the
version line, then the header lines, the groups, and the `end` line. The
first `n`, `i` or `end` line ends the header.

### 3.3 Groups

The body is a sequence of **groups**:

- a **net group**: one `n` line followed by that net's `w` lines;
- an **instance group**: one `i` line followed by the instance's `p` lines,
  each `p` line followed by that pin's `w` lines.

Any `w` line may be followed by its `a` lines before the next window or object.

A group can be decoded using the header and its own records. Driver names
identify objects in the design and can be used even if those objects have no
separate pin records. A writer MAY emit groups in any order. A reader MUST
accept any group order.

### 3.4 Grammar

The grammar uses ABNF, including `%s` for case-sensitive literals. `SEP` is
the token separator and `EOL` is the line ending.
[RFC 5234](https://www.rfc-editor.org/info/rfc5234) and
[RFC 7405](https://www.rfc-editor.org/info/rfc7405).

```abnf
otd-file     = *ignorable version *(ignorable / header-line)
               *(ignorable / group) end-line *ignorable

version      = %s"otd" SEP %s"1" EOL

header-line  = ( design / unit / thresholds / clock / free ) EOL
design       = %s"design" SEP name
unit         = ( %s"time_unit" / %s"cap_unit" / %s"res_unit" ) SEP real
thresholds   = %s"slew_thresholds" SEP real SEP real
clock        = %s"clock" SEP name SEP real SEP real SEP real attrs
free         = key [ SEP text ]

group        = net-group / inst-group
net-group    = net-line *( ignorable / window-group )
inst-group   = inst-line *( ignorable / pin-group )
pin-group    = pin-line *( ignorable / window-group )
window-group = window-line *( ignorable / arc-line )
net-line     = %s"n" SEP name SEP role attrs EOL
inst-line    = %s"i" SEP name EOL             ; name "-" = the design itself
pin-line     = %s"p" SEP name SEP role attrs EOL
window-line  = %s"w" SEP cause SEP role 8( SEP value ) attrs EOL
arc-line     = %s"a" SEP transition SEP name SEP transition SEP decimal attrs EOL
cause        = ( index SEP transition ) / ( "-" SEP "-" )
transition   = %s"r" / %s"f"

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
`w` line belongs to the nearest `p` line above it. An `a` line belongs to the
nearest `w` line of the same object above it.

### 3.5 Numbers

OTD has two number forms, on purpose:

- **`decimal`** — fixed point, no exponent, no `inf`/`nan`. Used for every
  number in the body that occurs in bulk: the eight window values, arc delays
  and arrivals, and every `range`. A reader can validate one with a byte scan
  and convert it without a general floating-point parser. A writer can emit
  one from a scaled integer.
  A writer SHOULD choose `time_unit` so that values need few digits, and
  SHOULD NOT write more fractional digits than the data is good for.
- **`real`** — exponent allowed. Used for numeric header fields.

## 4. Header

| Line | Meaning |
| --- | --- |
| `otd 1` | Format version. MUST be the first non-ignorable line. A reader MUST reject a version it does not implement. |
| `design <name>` | Design name. |
| `time_unit <real>` | Seconds per unit of every time, slew and slack in the file (`1e-9` = ns). MUST be positive. |
| `cap_unit <real>` | Farads per unit of capacitance (`load`). MUST be positive. |
| `res_unit <real>` | Ohms per unit of resistance (`rise_res`, `fall_res`, `drive_rise`, `drive_fall`). MUST be positive. |
| `slew_thresholds <low> <high>` | The percentages of the signal swing a slew is measured between. MUST satisfy `0 <= low < high <= 100`. |
| `clock <name> <period> <rise_edge> <fall_edge> [attrs]` | A clock waveform, in `time_unit`. Clock names MUST be unique. |
| `export_scope <key=value> ...` | Optional description of the export's object selection and data coverage (§4.2). |
| `<key> <text>` | Any other key is a **free attribute**, kept verbatim. |

A writer MUST write `time_unit` if the file contains any window, `res_unit`
if it writes any resistance and `cap_unit` if it writes any load. A header key
other than `clock` MUST NOT appear more than once. The keys `n`, `i`, `p`, `w`,
`a` and `end` are reserved for records.

Registered `clock` attributes:

| Attribute | Meaning |
| --- | --- |
| `source=<port>` or `source=<instance/pin>` | Where the clock enters or is generated. A clock without `source` is virtual. |
| `master=<index>` | The clock a generated clock is derived from. |

Conventional free attribute keys (a reader keeps them; none is required):
`corner` (name of the analysis view or operating condition), `process`,
`voltage`, `temperature`, `default_input_slew`, `divider`, `bus_delimiter`,
`date`, `program`, [`writer_options`](#41-writer-options).

### 4.1 Writer options

`writer_options` is an optional free attribute for effective export settings
that help explain the timing data. For example, a writer that rounds window
and arc values to four fractional digits could emit:

```text
writer_options digits=4
```

Record the effective value of each setting you include, even when it came
from a default. Use the existing header fields for settings they already
describe, such as `corner` for the selected analysis view. Keep file paths
and raw command invocations in run logs. Compression and timestamp options
can be left out.

The value is one line of producer-defined text in the file's ASCII character
set. Readers preserve it as a free attribute. The defined header fields and
timing records govern how readers interpret the timing data.

### 4.2 Export scope

`export_scope` is an optional header attribute that describes the coverage
and deliberate omissions of the exported file. Its keys have the same
meaning across producers. Use `writer_options` for effective settings
specific to the producer (§4.1).

```text
export_scope views=nets,pins selection=windowed window_sites=net_driver,pin_sink slew=omitted arcs=extrema
```

Use the free header syntax in §3.4. The value MUST contain one or more
`key=value` tokens separated by `SEP`. Each key MUST occur at most once. A
list is comma-separated and MUST contain at least one value with no
duplicates. Key order and list order carry no meaning.

| Key | Values and meaning |
| --- | --- |
| `views` | A list drawn from `nets`, `pins`. Only the listed views are enabled. `nets` covers `n` groups. `pins` covers `i` groups and their `p` records, including top-level ports. An enabled view may have zero records. |
| `selection` | `windowed` means every emitted net and pin has at least one emitted `w` record. The writer omits objects without emitted windows and instance groups left with no pins. |
| `window_sites` | A list drawn from `net_driver`, `pin_driver`, `pin_sink`, or the single value `none`. Windows occur only at the listed sites. `net_driver` means a net window at the net's sole driver. `pin_driver` and `pin_sink` mean windows on pins or top-level ports acting as drivers and sinks, respectively. `none` means zero `w` records. When `views` is supplied, each listed site other than `none` MUST belong to an enabled view. |
| `slew` | `omitted` means all four positional slew fields of every window are `-`. |
| `arcs` | `none` means zero `a` records. `extrema` selects only numerical delay extrema. For each output edge of a window with `a` records, include `min_delay` and `max_delay` tags, which may occur on the same observation (§6.1). A window or edge can have zero observations. |

Each key is optional. An absent key or unrecognized value leaves that
policy unspecified. A reader can use a listed view or site to determine
eligibility. The body records determine which objects and measurements are
present. An absent object is unreported, and a `-` remains unspecified for
its individual window field.

A reader MAY preserve `export_scope` as opaque header text. A reader using
the metadata MUST ignore unrecognized keys. If the reader detects malformed
syntax or a conflict between claims or with the body, it MUST disregard the
line when deciding scope. A tool that changes the export MUST update or
remove scope claims it can no longer make.

## 5. Objects

### 5.1 Nets

```text
n <net> <role> [attrs]
```

Declares a net. A net MUST NOT be declared more than once in a file.

A writer MUST emit net windows only for a net with one known driver. The
arrival and slew values describe switching at that driver pin or top-level
port. Use pin windows for arrival and slew at sinks. Nets with an unknown
driver or multiple drivers may still carry a role and attributes. For
example, a multi-driver clock mesh can be declared `c` with zero windows.

A writer MAY identify the net's sole driver with the `driver_instance` and
`driver_pin` attributes. The pair is optional even when the net has windows.
Omitting it leaves the driver's identity unspecified.
Both attributes MUST appear together, once each, as names (§5.3). An instance
of `-` identifies a driving top-level port. The names identify a pin in the
design, so they work in a net-only file. The writer is responsible for
matching the design's connectivity.

### 5.2 Instances and pins

```text
i <instance>
p <pin> <role> [attrs]
```

`i` opens an instance group; each following `p` line declares one pin of that
instance. The instance name `-` denotes the design itself, so its pins are the
top-level ports.

An instance group may list any subset of its pins. Each declared driver pin,
sink pin, or port may carry its own windows. A pin-only file can report
driver timing on the driver pin. A mixed file may report a driver's timing
on its pin, on its driven net, or on both. When both describe the same
switching event, consumers should treat them as two views of that event.

- An instance MUST NOT have more than one group in a file.
- A pin MUST NOT be declared more than once in its group.
- A writer MUST NOT write an `i` line with no `p` line; a reader MAY accept one.

### 5.3 Names

A name is a token of visible ASCII with no blanks. Use the design's instance,
pin and net names. DEF backslash escapes are optional. A consumer MUST
compare names with every backslash removed, so `n data[3] d` matches the DEF
net `data\[3\]`. The instance and the pin are separate tokens, so splitting a
pin name requires no hierarchy divider.

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
| `load` | `range` | External capacitive load, in `cap_unit`. |
| `value` | `0` or `1` | Logic value of a constant (`z`) object. |
| `drive_rise`, `drive_fall` | `range` | Strength of whatever drives a top-level input port for the rising / falling transition, as a resistance in `res_unit`. |
| `driver_instance`, `driver_pin` | two `name` values | Optional instance and pin of the sole driver, on `n` records only (§5.1). |

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

In the eight arrival and slew fields, `-` means that the writer supplies no
value for that field. For example, `rise_early=-` with `rise_late=0.150`
reports a late rising bound only. Two `-` arrival bounds for one polarity
leave its arrival time unspecified. Writers may use `-` for an unavailable
or deliberately omitted measurement, or an absent transition.

Arrival times are relative to the time origin of the clock waveforms.
Windows describe driven transitions between logic levels. Writers MUST omit
transitions into or out of high impedance. The `z` role denotes a constant
logic value.

- A window with role `c` MUST NOT appear on an object whose role is not `c`.
- An object MAY have several windows, including several for the same clock and
  edge.
- An object MAY have no windows; its role and attributes still stand.

Registered window attributes, each a `range`:

| Attribute | Meaning |
| --- | --- |
| `rise_res`, `fall_res` | Resistance of the source driving the rising / falling transition, in `res_unit`. |
| `rise_slack`, `fall_slack` | Slack of the rising / falling transition, in `time_unit`. Omitted when no slack was computed. |

### 6.1 Cell-arc observations

```text
a <output_edge> <input_pin> <input_edge> <delay> [attrs]
```

A window MAY carry zero or more `a` records. Each describes an evaluated cell
propagation arc leading to a transition in the parent window. The arc ends at
the net's sole driving pin for a net window, or at the declared output pin for
a pin window. The input pin is named within the same instance. A sequential
propagation arc, such as clock-to-Q, can be reported the same way.

| Field | Value | Meaning |
| --- | --- | --- |
| `output_edge` | `r` or `f` | Signal transition at the output. The parent window MUST have an arrival bound for this transition. |
| `input_pin` | `name` | Instance-local input that contributes to the output transition. |
| `input_edge` | `r` or `f` | Signal transition at that input. |
| `delay` | `decimal` | Increment from the input pin's arrival to the output pin's arrival for this arc, in `time_unit`. |

Registered arc attributes:

| Attribute | Value | Meaning |
| --- | --- | --- |
| `tags` | `min_delay`, `max_delay`, or both, comma-separated | The arc realizes the numerical minimum or maximum evaluated delay among the arcs contributing to this window for this output edge. Both tags may appear on one observation when the extrema coincide. |
| `output_arrival` | `decimal` | Output arrival of this same observation, in `time_unit` and relative to the window's time origin. If supplied, it MUST lie within the available arrival bounds for its output edge. |

A writer MAY report selected observations, including the minimum and maximum
for each output edge, the arcs that set the window's arrival bounds, or other
contributing cases. An observation can carry a delay and input pin even when
the exact output arrival of that case is unavailable.

```text
n data d
w - - d 0.100 0.150 - - 0.010 0.025 - -
a r A r 0.012 tags=min_delay
a r B f 0.040 tags=max_delay output_arrival=0.130
```

### 6.2 Sliding supply averaging (informative)

When both arrival bounds are available for an output edge and its arc delay
`d` is positive, step the end time `t` of the averaging interval over the
early-to-late arrival window. At each step, average the driver's local
`VDD - VSS` over `[t - d, t]`. The lowest average across the sampled steps
and selected widths gives the largest measured compression for that sweep.
Use the same time origin for the supply trace and the OTD window.

The sweep evaluates possible placements of an interval. An observation with
`output_arrival` also identifies a particular STA case. Minimum-delay observations
supply the narrowest intervals. A wider interval can pick up a droop missed by
the narrower sweep.

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

Any `clock`, `n`, `p`, `w` or `a` line may end with `key=value` tokens. Nothing
else may follow the positional fields.

- A reader MUST validate the registered attributes (sections 4 to 6) and MUST
  preserve, without interpreting, any attribute it does not know.
- A key MUST NOT appear twice on one line.
- Unregistered keys are free for tools to use. A tool-specific key SHOULD carry
  a prefix naming the tool (`acme_sigma=3`), so later versions of this
  specification can register plain keys without collision.
- Once a version is frozen, a change that an existing reader would misread,
  such as a new record type, positional field or role, requires a new version.

## 9. Compression and parallelism

### 9.1 Compression

A `.otd.gz` file is the OTD text compressed with gzip. It MAY
consist of **several gzip members concatenated**; a reader MUST read all of
them as one stream. A writer can therefore compress runs of groups
independently, on separate threads, and append the members in any order (the
header first, the `end` line last).
[RFC 1952](https://www.rfc-editor.org/info/rfc1952).

### 9.2 Streaming and parallelism (informative)

Several format choices support streaming and parallel processing.

| Property | Consequence |
| --- | --- |
| Writers start each record with its type | A reader can classify a record before decoding its fields. |
| Groups are independent and unordered | Readers and writers can process complete groups concurrently. |
| No name table, offsets or references between body groups | A writer can stream groups and a reader can process them independently once it has the header. |
| Counts are in a trailer, not the header | A streaming writer knows them only at the end, and that is where they go. Truncation is still detected. |
| An instance name is written once for all its pins | Pin-based files are smaller, and a consumer does one instance lookup per group. |
| Body numbers are fixed-point decimals | Validation is a byte scan; conversion needs no general float parser; a writer needs no shortest-representation float printer. |
| 7-bit ASCII | No multi-byte decoding or validation. |
| One declaration per object | No reconciliation pass over repeated records. |

## 10. Errors

A reader MUST reject a file, naming the file and, where one applies, the line,
when:

1. the first non-ignorable line is not `otd <version>`, or the version is not
   implemented;
2. a line contains a non-ASCII byte;
3. a unit is not positive, or `slew_thresholds` is not `0 <= low < high <= 100`;
4. a clock name is declared twice;
5. a role is not one of `c`, `d`, `u`, `z`;
6. a net is declared twice, an instance has two groups, or a pin is declared
   twice in its group;
7. a `p` line is outside an instance group, a `w` line has no `n` or `p` line
   above it in its group, or an `a` line has no parent window on its object;
8. a token after the positional fields is not `key=value`;
9. a registered attribute is malformed: `clocks` names an undeclared clock,
   `value` is not `0`/`1`, a `range` is not decimals,
   or a net driver pair violates §5.1;
10. a `w` line names an undeclared clock, has an edge that does not fit its
    clock, has fewer than eight values, or a value that is neither a `decimal`
    nor `-`;
11. a window with role `c` is on an object not declared `c`;
12. an arc observation has an invalid field or attribute, or its output
    transition or arrival conflicts with the parent window (§6.1);
13. a header line appears after the first group;
14. there is no `end` line, its counts do not match the file, or a record
    follows it.

A reader MUST NOT repair or guess: a file that fails any check yields no data.

## 11. Field lookup (informative)

Timing data and its corresponding OTD fields.

| Information | OTD |
| --- | --- |
| Design name, producing program, date | `design`, `program`, `date` |
| Effective export settings | Optional `writer_options` header attribute (§4.1) |
| Object selection, window placement, slew and arc coverage | Optional `export_scope` header attribute (§4.2) |
| Time / capacitance / resistance scale | `time_unit`, `cap_unit`, `res_unit` |
| Slew measurement thresholds | `slew_thresholds` |
| Process, voltage, temperature, operating condition / view | `process`, `voltage`, `temperature`, `corner` (one corner per file) |
| Default input slew | `default_input_slew` |
| Hierarchy divider, bus delimiters | Not needed for pins (instance and pin are separate tokens); `divider`, `bus_delimiter` for net names |
| Clock waveforms: period, rise and fall edge | `clock` |
| Clock source, generated-clock master | `clock ... source= master=` |
| Virtual clocks (a waveform with no source) | `clock` with no `source=` |
| Per net **or** per pin records | `n` / `i` + `p` |
| Driving pin of a net | `driver_instance=` and `driver_pin=` on `n` |
| Launching clock and edge of a window; no launching clock | `w <clock> <edge>`; `w - -` |
| Min/max rise and fall arrival window | `rise_early rise_late fall_early fall_late` |
| Min/max rise and fall transition time | `rise_slew_min rise_slew_max fall_slew_min fall_slew_max` |
| Input pin, cell-arc delay extrema, and observed output arrival | `a` records within a window |
| Min/max rise and fall source resistance | `rise_res=`, `fall_res=` |
| Min/max rise and fall slack | `rise_slack=`, `fall_slack=` |
| Unavailable window field | `-` in that field. Optional attributes may be omitted. |
| Clock vs data flag per record | window `<role>` |
| Clock vs data per object (and constant / unconstrained) | object `<role>`, with the precedence rule |
| Constant nets and their logic value | role `z` with `value=0` / `value=1` |
| Input-port driver strength, per edge | `drive_rise=`, `drive_fall=` on a pin of instance `-` |
| External load on a port or net | `load=` |
| Clock domain per object | `clocks=`, or derived from the windows |
| Anything tool-specific (statistical sigma, derates, ...) | prefixed `key=value` attributes and free header keys |

## Appendix A — Complete example (informative)

```text
# OTD 1 field guide. Brackets enclose optional fields.
# otd version
# design name
# time_unit seconds_per_unit
# cap_unit farads_per_unit
# res_unit ohms_per_unit
# slew_thresholds low_pct high_pct
# clock name period rise_edge fall_edge [key=value ...]
# n net role [key=value ...]
# i instance (- for top-level ports)
# p pin role [key=value ...]
# w launch_clock launch_edge role rise_early rise_late fall_early fall_late rise_slew_min rise_slew_max fall_slew_min fall_slew_max [key=value ...]
# a output_edge input_pin input_edge delay [tags=list] [output_arrival=time] [key=value ...]
# end net_count pin_count window_count
# Roles: c clock, d data, u unconstrained, z constant. Edges: r rise, f fall.
# On w, - marks an absent launch clock, launch edge, or transition value.
# Clock indices start at 0. Arrivals, slews, and delays use time_unit.
# Common object attrs: clocks (indices), load, value, drive_rise, drive_fall.
# Window attrs include rise_res, fall_res, rise_slack, and fall_slack.
# A range uses min:max or one value for both bounds.
# Arc tags min_delay and max_delay describe the extrema for the output edge.
# net-based and pin-based objects in one file
otd 1
design top
time_unit 1e-9
cap_unit 1e-12
res_unit 1e3
slew_thresholds 10 90
corner ss_0p72v_125c
program my_sta 2026.11
writer_options digits=4
export_scope views=nets,pins window_sites=net_driver,pin_sink arcs=none
clock clk 5 0 2.5 source=clk
clock clk_div2 10 0 5 master=0 source=u_div/Q
n clk_leaf_3 c clocks=0 driver_instance=u_core/u_icg driver_pin=GCK
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

## Appendix B — A binary form (informative; not part of OTD 1)

OTD 1 specifies an ASCII interchange file. An implementation can derive
its own binary cache or columnar view for analysis. A future interoperable
binary representation would need to define how it preserves the header,
objects, windows, arc observations and extension attributes.
