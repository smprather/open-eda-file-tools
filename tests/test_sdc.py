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
