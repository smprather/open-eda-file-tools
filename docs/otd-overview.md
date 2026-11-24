# Open Timing Data overview and review guide

Open Timing Data (OTD) lets an EDA tool export switching windows and clock
roles for another tool to consume. A static timing analyzer can supply
arrivals and slews to a power-integrity tool. A physical-design tool can
identify clock nets and pins from the same exchange file. The format can be
implemented independently by any producer or consumer.

OTD 1 is a community review draft. The [OTD 1 specification](otd.md) defines
the file format and conformance requirements. This guide explains how to
read an example and what to check when reviewing an implementation.

## What a file carries

| Part | What to look for |
| --- | --- |
| Header | Design, units, clock waveforms and available measurement conventions for one analysis corner. Optional metadata describes the producer, its export settings and the scope of the exported data. |
| Net groups | One record per declared net. A net window describes the net's sole known driver. A writer can include the driver's instance and pin even in a net-only export. |
| Instance groups | Named pins of an instance, including sink windows where available. Pin-only exports can carry driver timing on the output pin. |
| Windows | Rise and fall arrival bounds and slew bounds, associated with a launch clock edge when known. A `-` leaves the individual field unspecified. |
| Arc observations | Optional input pin, input and output transitions, and evaluated propagation delay for a case contributing to a window. |
| Trailer | Counts of net, pin and window records. A writer can stream groups and emit the totals at the end. |

A file may contain net groups, instance groups or both. The spec covers the
exchange data produced by timing analysis. The original netlist, constraints,
parasitics and full timing paths remain separate inputs or reports.

## Read a window

This excerpt omits the required opening field guide. The file declares a
nanosecond as its unit of time and `clk` as its first clock, at index `0`.
A complete file with the field guide is available in the specification.
[Complete example](otd.md#appendix-a--complete-example-informative).

```text
otd 1
design top
time_unit 1e-9
corner slow
writer_options digits=4
export_scope views=nets,pins selection=windowed window_sites=net_driver,pin_sink
clock clk 2 0 1 source=clk
n data d driver_instance=u_buf driver_pin=Z
w 0 r d 0.100 0.150 - 0.169 0.010 0.025 - -
a r A r 0.012 tags=min_delay output_arrival=0.112
i u_sink
p A d
w 0 r d 0.110 0.170 - 0.184 0.014 0.029 - -
end 1 1 2
```

The `n` group names `u_buf/Z` as the driver of `data`. Its `w` record is for
data launched by the rising edge of `clk`. At the driver, the signal's rising
arrival is between 100 and 150 ps and its rising slew is between 10 and 25 ps.
For the falling signal, only the 169 ps late arrival is supplied. The `r`
after the clock index identifies the launch edge. The values further along
the line describe both signal polarities. [Window fields](otd.md#6-windows).

The `a` record gives an observed 12 ps delay from a rising transition on
`u_buf/A` to a rising transition on `u_buf/Z`. Its observed output arrival is
112 ps. An IR-drop consumer can use that delay as an averaging interval width
and step the interval across the 100–150 ps rising arrival window on a local
`VDD - VSS` trace with the same time origin. The later window on `u_sink/A`
describes a sink at a different point on the signal path.
[Arc observations](otd.md#61-cell-arc-observations) ·
[Supply averaging](otd.md#62-sliding-supply-averaging-informative).

In this example, `selection=windowed` means every emitted net and pin has a
window. Other design objects may be absent. The header leaves the slew and
arc policies unspecified. The window and arc records supply the actual values.
[Export scope](otd.md#42-export-scope).

## Interoperability checks

Use a small, known design and record the expected values before testing on
large designs. The cases below exercise distinctions that can survive syntax
validation and still change a downstream result.

| Case | What to check |
| --- | --- |
| A net with a driver and a sink | Net timing measures the driver. The sink's pin record carries timing at the load. A net with several drivers can carry a role but has no net window. [Objects](otd.md#5-objects) |
| A clock with both launch edges and separate clock and data arrivals | Clock indices follow declaration order. The window's launch edge and the signal's rise/fall fields have distinct meanings. An object with any clock arrival has role `c`. [Roles](otd.md#54-roles) · [Windows](otd.md#6-windows) |
| One-sided timing and unavailable slew | Each `-` survives exchange independently of other values. A real zero stays zero. Check units and any stated slew thresholds against the producer's measurement convention. [Header](otd.md#4-header) · [Windows](otd.md#6-windows) |
| Several arcs into one output | The input pin and transition identify the contributing arc. Delay extrema compare evaluated delays. An `output_arrival`, when present, belongs to that same observation and fits the parent window. [Arc observations](otd.md#61-cell-arc-observations) |
| A windowed export with deliberately omitted slew | Each emitted object has a `w` record and all four slew fields are `-` when `slew=omitted`. An absent object gives no information about its role or timing. [Export scope](otd.md#42-export-scope) |
| Truncation, reordered groups and gzip | A missing or inconsistent trailer is rejected. Reordering complete body groups preserves meaning. A reader sees all concatenated gzip members as one stream. [Groups](otd.md#33-groups) · [Trailer](otd.md#7-the-end-line) · [Compression](otd.md#91-compression) |

## Community review

Spec feedback is welcome from tool authors and users. Cite the relevant
section and draft revision. For a disputed interpretation, include a small
OTD example and what a reader should return or reject. For a proposed field,
describe the use case and what a reader can infer when the field is absent.
A producer-to-consumer interoperability report is especially useful if it
identifies both implementations, the spec revision and the cases exercised.
[Spec feedback](https://github.com/smprather/open-eda-file-tools/issues).

When citing a particular draft in a review or an interoperability report,
link to `docs/otd.md` at its commit. The version line identifies the on-disk
format. During review, multiple draft revisions may use `otd 1`.

The [implementation notes](otd-tools.md) describe `open-eda-file-tools`
commands, supported features and measured reader performance.
