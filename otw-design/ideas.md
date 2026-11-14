# Candidate STA results for OTW

Brainstormed 2026-11-14. The motivating consumer is dynamic IR analysis, which
has to decide where in time to place each switching event. Arrival bounds are
the least informative thing STA knows about a net.

Nothing here is designed, and none of it has been checked against how an IR
tool actually schedules events.

## Source of each candidate

The **Source** column says who can produce the data:

- **STA**: a timing engine can compute it from the netlist, libraries,
  parasitics and constraints.
- **Activity**: it depends on how often or under what logic conditions
  something toggles. That comes from logic simulation or from a power tool's
  activity propagation, not from STA.
- **Mixed**: part of it is STA, part is activity; the split is in the notes.

The first draft of this list presented several activity items as STA results.
They are kept, marked, so the decision to drop them or carry them as optional
externally supplied data is explicit.

## When does it switch

| Candidate | Source | Notes |
| --- | --- | --- |
| Arrival histogram | STA | Distribution of arrival times inside the window instead of only its bounds. STA can count paths or startpoints per bin. A path count is not a probability; weighting bins by activity is a separate, activity-sourced step. |
| Arrival–slew correlation | STA | Early arrivals tend to come with fast slews. Independent min/max values lose the pairing. Could be a slew per histogram bin. |
| Statistical arrival | STA | Mean and sigma from a statistical or variation-aware run. The parametric cousin of the histogram. |
| Multicycle and phase | STA | Which cycle of the hyperperiod a window belongs to; a multicycle path does not launch every cycle. Comes from clock waveforms and timing exceptions. |
| Switching probability per window | Activity | How often the object toggles on this clock edge. v1 has one `freq` per object. |
| Rise/fall probability split | Activity | Windows already carry the edge; only the probability is new, and that is activity. |
| Glitch count | Activity | Transitions per cycle before the net settles. STA sees multiple arrival times but cannot say how many transitions occur. |

## What switches together

| Candidate | Source | Notes |
| --- | --- | --- |
| Clock exclusivity | STA | Clocks declared logically or physically exclusive, and objects held constant by case analysis. Windows of exclusive clocks cannot coincide. |
| Startpoint sets | STA | The launching flops or ports in an object's fan-in. Structural. That objects sharing a startpoint "switch together or not at all" is a functional claim STA does not make. |
| Clock-gating domain | Mixed | Which gate each clock pin sits under is STA (clock network structure). The gate's enable probability is activity. |
| Functional exclusivity | Activity | Mux branches, one-hot state. Needs logic analysis or simulation. |

## How much current

| Candidate | Source | Notes |
| --- | --- | --- |
| Net capacitance | STA | Effective capacitance, and total, wire and pin capacitance. v1 carries slew but not the load the driver sees. |
| Driving arc | STA | The input pin and `when` condition of the arc behind each window, which selects the Liberty current and internal-energy table. |
| Clock-pin events on sequential cells | STA | Arrival at the clock pin is already a pin window in v1. The internal energy is a library lookup for the consumer, not file data. Probably nothing to add. |

## Back toward timing

| Candidate | Source | Notes |
| --- | --- | --- |
| Voltage sensitivity | STA | Delay change per millivolt of supply droop at an instance, so IR can weight droop where slack is thin. Needs libraries at more than one voltage, or a derate model. |
| Endpoint cone size | STA | How many endpoints depend on a net. |

## Scenario

| Candidate | Source | Notes |
| --- | --- | --- |
| Mode | STA | Functional, scan shift or scan capture, in the header. One per file, same rule as corner. |

## Consequences for the format

- **Vector data needs a new record type**, for example an `h` line under a
  `w` line. That is a version bump under the spec's own rule.
- **Sets must be referenced by index**, declared in the header like clocks,
  so record groups stay independent.
- **Instance-level data has no home in v1.** `i` only groups pins. Voltage
  sensitivity would need attributes on the `i` line.
- **Activity data, if carried, should be marked as externally supplied**, so
  a reader can tell what the STA tool computed from what was fed to it.
