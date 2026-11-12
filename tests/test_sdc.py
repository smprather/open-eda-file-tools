"""Tests for SDC clock extraction (``sdc_tools``)."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from click.testing import CliRunner

from sdc_tools import Clock, clocks
from sdc_tools.cli import main

OPENLANE_SDC = Path.home() / "openlane2-hello-world" / "gcd.sdc"

GCD_STYLE = """\
current_design gcd
create_clock -name clk -period 10.0000 [get_ports {clk}]
set_clock_transition 0.1500 [get_clocks {clk}]
set_clock_uncertainty 0.2500 clk
set_propagated_clock [get_clocks {clk}]
set_input_delay 2.0000 -clock [get_clocks {clk}] -add_delay [get_ports {A[0]}]
set_driving_cell -lib_cell sg13g2_buf_4 -pin {X} [get_ports {clk}]
set_load -pin_load 0.0060 [get_ports {done}]
set_max_fanout 10.0000 [current_design]
"""


def write_sdc(tmp_path: Path, text: str) -> Path:
    path = tmp_path / "design.sdc"
    path.write_text(text, encoding="utf-8")
    return path


def test_create_clock_survives_typical_constraints(tmp_path: Path) -> None:
    path = write_sdc(tmp_path, GCD_STYLE)

    found = clocks(path)

    assert found == [
        Clock(
            name="clk",
            period=10.0,
            waveform=(0.0, 5.0),
            sources=("port:clk",),
        )
    ]


def test_variables_and_foreach_are_evaluated(tmp_path: Path) -> None:
    path = write_sdc(
        tmp_path,
        "set P 8\n"
        "foreach p {clk_a clk_b} {\n"
        "    create_clock -name $p -period $P [get_ports $p]\n"
        "}\n",
    )

    found = clocks(path)

    assert [(c.name, c.period, c.sources) for c in found] == [
        ("clk_a", 8.0, ("port:clk_a",)),
        ("clk_b", 8.0, ("port:clk_b",)),
    ]


def test_generated_clock_derives_its_period(tmp_path: Path) -> None:
    path = write_sdc(
        tmp_path,
        "create_clock -name clk -period 10 [get_ports clk]\n"
        "create_generated_clock -name div2 -source [get_ports clk] "
        "-divide_by 2 [get_pins div/Q]\n",
    )

    found = clocks(path)

    assert found[1] == Clock(
        name="div2",
        period=20.0,
        waveform=(),
        sources=("pin:div/Q",),
        generated_from="clk",
        divide_by=2,
    )


def test_unknown_commands_do_not_abort_the_run(tmp_path: Path) -> None:
    path = write_sdc(
        tmp_path,
        "create_clock -name clk -period 5 [get_ports clk]\n"
        "set_clock_groups -asynchronous -group [get_clocks clk]\n"
        "some_vendor_command -foo bar [get_pins u/A]\n",
    )

    found = clocks(path)

    assert [c.name for c in found] == ["clk"]


def test_malformed_sdc_reports_the_file(tmp_path: Path) -> None:
    path = write_sdc(tmp_path, "if {\n")

    with pytest.raises(ValueError, match="design.sdc"):
        clocks(path)


def test_cli_prints_json(tmp_path: Path) -> None:
    path = write_sdc(tmp_path, GCD_STYLE)

    result = CliRunner().invoke(
        main, ["clocks", "--sdc", str(path), "--format", "json"]
    )

    assert result.exit_code == 0, result.output
    payload = json.loads(result.output)
    assert payload == [
        {
            "name": "clk",
            "period": 10.0,
            "waveform": [0.0, 5.0],
            "sources": ["port:clk"],
            "generated_from": None,
            "divide_by": None,
            "multiply_by": None,
            "edges": [],
            "invert": False,
        }
    ]


@pytest.mark.skipif(not OPENLANE_SDC.exists(), reason="local OpenLane fixture missing")
def test_openlane_hello_world_sdc() -> None:
    found = clocks(OPENLANE_SDC)

    assert [(c.name, c.period, c.sources) for c in found] == [
        ("clk", 10.0, ("port:clk",))
    ]


TRACE_DEF = """\
VERSION 5.8 ;
DESIGN trace ;
UNITS DISTANCE MICRONS 1000 ;
COMPONENTS 3 ;
  - u1 BUFX + PLACED ( 0 0 ) N ;
  - u2 GATEX + PLACED ( 0 0 ) N ;
  - u3 DFFX + PLACED ( 0 0 ) N ;
END COMPONENTS
NETS 4 ;
  - clk ( PIN clk ) ( u1 A ) + USE CLOCK ;
  - n1 ( u1 X ) ( u2 A ) + USE CLOCK ;
  - n2 ( u2 X ) ( u3 CK ) + USE CLOCK ;
  - d ( u3 D ) ;
END NETS
END DESIGN
"""

TRACE_LIB = """\
library (trace) {
  cell (BUFX) {
    pin (A) { direction : input; }
    pin (X) { direction : output; function : "A"; }
  }
  cell (GATEX) {
    pin (A) { direction : input; }
    pin (EN) { direction : input; }
    pin (X) { direction : output; function : "A EN"; }
  }
  cell (DFFX) {
    ff (IQ, IQN) { next_state : "D"; clocked_on : "CK"; }
    pin (CK) { direction : input; clock : true; }
    pin (D) { direction : input; }
    pin (Q) { direction : output; function : "IQ"; }
  }
}
"""


def test_clock_cells_trace_forward_to_sequential_endpoints(tmp_path: Path) -> None:
    from sdc_tools import clock_cells

    design = tmp_path / "trace.def"
    design.write_text(TRACE_DEF, encoding="utf-8")
    liberty = tmp_path / "trace.lib"
    liberty.write_text(TRACE_LIB, encoding="utf-8")
    sdc = write_sdc(
        tmp_path, "create_clock -name clk -period 10 [get_ports clk]\n"
    )

    result = clock_cells(design, [liberty], sdc)

    assert [clock.name for clock in result.clocks] == ["clk"]
    assert result.cells == ("u1", "u2")


def test_clock_cells_cli_writes_lines(tmp_path: Path) -> None:
    design = tmp_path / "trace.def"
    design.write_text(TRACE_DEF, encoding="utf-8")
    liberty = tmp_path / "trace.lib"
    liberty.write_text(TRACE_LIB, encoding="utf-8")
    sdc = write_sdc(
        tmp_path, "create_clock -name clk -period 10 [get_ports clk]\n"
    )

    result = CliRunner().invoke(
        main,
        [
            "clock-cells",
            "--sdc",
            str(sdc),
            "--def",
            str(design),
            "--lib",
            str(liberty),
        ],
    )

    assert result.exit_code == 0, result.output
    assert result.output.splitlines() == ["u1", "u2"]


OPENLANE_DEF = Path.home() / "openlane2-hello-world" / "gcd.def.gz"
OPENLANE_LIB = (
    Path.home()
    / "pdk/IHP-Open-PDK/ihp-sg13g2/libs.ref/sg13g2_stdcell/lib"
    / "sg13g2_stdcell_typ_1p20V_25C.lib"
)


@pytest.mark.skipif(
    not (OPENLANE_SDC.exists() and OPENLANE_DEF.exists() and OPENLANE_LIB.exists()),
    reason="local OpenLane/IHP fixture missing",
)
def test_openlane_clock_cells() -> None:
    from sdc_tools import clock_cells

    result = clock_cells(OPENLANE_DEF, [OPENLANE_LIB], OPENLANE_SDC)

    assert result.cells == (
        "clkbuf_0_clk",
        "clkbuf_2_0__f_clk",
        "clkbuf_2_1__f_clk",
        "clkbuf_2_2__f_clk",
        "clkbuf_2_3__f_clk",
    )
    assert "_419_" not in result.cells  # a flop, not a distribution cell
