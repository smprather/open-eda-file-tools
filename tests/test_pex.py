"""Tests for the SPEF/DSPF parser and ``pex-tools`` CLI."""

from __future__ import annotations

from pathlib import Path

import pytest
from click.testing import CliRunner

import pex_tools
from pex_tools.cli import main

SPEF = """\
*SPEF "ieee 1481-1999"
*DESIGN "tiny"
*DATE "today"
*VENDOR "test"
*PROGRAM "test"
*VERSION "1"
*DIVIDER /
*DELIMITER :
*BUS_DELIMITER []
*T_UNIT 1 NS
*C_UNIT 1 PF
*R_UNIT 1 OHM
*L_UNIT 1 HENRY

*NAME_MAP
*1 clk
*2 u1:A
*3 u2:B

*PORTS
*P *1 I

*D_NET *1 0.5
*CONN
*P *1 I
*I *2 I *D buf *C 10 20
*CAP
1 *1 0.1 $ layer=M1
2 *1 *2 0.4 $ layer=M2 x=1 y=2
*RES
1 *1 *2 3.5 $ layer=M1
*END

*D_NET clk2 0.2
*CONN
*P clk2 I
*CAP
1 clk2 0.2
*END
"""

DSPF = """\
*DSPF "1.0"
*DESIGN "dspf_tiny"
*DELIMITER :
*T_UNIT 1 PS
*C_UNIT 1 FF
*R_UNIT 1 OHM
*D_NET net1 0.3
*CONN
*P in I
*I u1:A I *D buf1 *C 1.5 2.5
*CAP
1 in 0.1 $ layer=M1
2 in u1:A 0.2 $ layer=M2 x=3 y=4
*RES
1 in u1:A 7 $ layer=M1
*END
"""

OPENLANE_SPEF = (
    Path.home()
    / "openlane2-hello-world/runs/RUN_2026-11-11_20-15-37"
    / "final/spef/nom/gcd.nom.spef"
)


def write(tmp_path: Path, text: str, name: str) -> Path:
    path = tmp_path / name
    path.write_text(text, encoding="utf-8")
    return path


def test_spef_summaries_and_entries(tmp_path: Path) -> None:
    path = write(tmp_path, SPEF, "tiny.spef")

    info = pex_tools.header(path)
    assert info.standard == "ieee 1481-1999"
    assert info.design == "tiny"
    assert (info.t_unit, info.c_unit, info.r_unit, info.l_unit) == (
        "1 NS",
        "1 PF",
        "1 OHM",
        "1 HENRY",
    )
    assert info.ports == [("clk", "I")]

    nets = pex_tools.summaries(path)
    assert [(net.name, net.total_cap) for net in nets] == [
        ("clk", 0.5),
        ("clk2", 0.2),
    ]
    assert nets[0].ground_cap == pytest.approx(0.1)
    assert nets[0].coupling_cap == pytest.approx(0.4)
    assert nets[0].cap_count == 2
    assert nets[0].res_count == 1
    assert nets[0].res_total == pytest.approx(3.5)

    parsed = pex_tools.parse(path)
    net = parsed.nets[0]
    assert net.name == "clk"
    assert [(cap.node1, cap.node2, cap.value) for cap in net.caps] == [
        ("clk", None, 0.1),
        ("clk", "u1:A", 0.4),
    ]
    assert net.caps[0].ext == "layer=M1"
    assert net.caps[1].ext == "layer=M2 x=1 y=2"
    assert net.res[0].ext == "layer=M1"
    assert net.conns[1].name == "u1:A"
    assert (net.conns[1].x, net.conns[1].y) == (10.0, 20.0)


def test_dspf_connection_attributes(tmp_path: Path) -> None:
    path = write(tmp_path, DSPF, "tiny.dspf")

    info = pex_tools.header(path)
    assert info.standard == "1.0"
    assert info.design == "dspf_tiny"

    parsed = pex_tools.parse(path)
    net = parsed.nets[0]
    assert net.name == "net1"
    assert net.total_cap == pytest.approx(0.3)
    assert net.ground_cap == pytest.approx(0.1)
    assert net.coupling_cap == pytest.approx(0.2)
    assert net.conns[1].attrs == ["*D", "buf1", "*C", "1.5", "2.5"]
    assert (net.conns[1].x, net.conns[1].y) == (1.5, 2.5)
    assert net.caps[1].ext == "layer=M2 x=3 y=4"
    assert net.res[0].value == pytest.approx(7.0)


def test_streaming_matches_parallel(tmp_path: Path) -> None:
    path = write(tmp_path, SPEF, "tiny.spef")

    parallel = pex_tools.summaries(path, mode="auto")
    streamed = pex_tools.summaries(path, mode="stream")

    assert [(n.name, n.total_cap, n.ground_cap, n.coupling_cap) for n in parallel] == [
        (n.name, n.total_cap, n.ground_cap, n.coupling_cap) for n in streamed
    ]


def test_histogram_counts_every_net(tmp_path: Path) -> None:
    path = write(tmp_path, SPEF, "tiny.spef")

    hist = pex_tools.histogram(path, bins=4)

    assert len(hist.counts) == 4
    assert sum(hist.counts) == 2
    assert hist.min == pytest.approx(0.2)
    assert hist.max == pytest.approx(0.5)


def test_cli_summary_histogram_and_nets(tmp_path: Path) -> None:
    path = write(tmp_path, SPEF, "tiny.spef")
    runner = CliRunner()

    summary = runner.invoke(main, ["summary", "--spef", str(path)])
    assert summary.exit_code == 0, summary.output
    assert "design: tiny" in summary.output
    assert "nets: 2" in summary.output

    hist = runner.invoke(main, ["histogram", "--spef", str(path), "--bins", "2"])
    assert hist.exit_code == 0, hist.output
    assert hist.output.splitlines()[0] == "bin_low\tbin_high\tcount"

    nets = runner.invoke(main, ["nets", "--spef", str(path), "--top", "1"])
    assert nets.exit_code == 0, nets.output
    assert nets.output.splitlines()[1].startswith("clk\t0.5")


@pytest.mark.skipif(not OPENLANE_SPEF.exists(), reason="local OpenLane SPEF missing")
def test_openlane_gcd_spef() -> None:
    nets = pex_tools.summaries(OPENLANE_SPEF)

    assert len(nets) == 359
    assert nets[0].name == "A[0]"
    assert sum(net.total_cap for net in nets) == pytest.approx(1.0196748797)
    assert sum(net.res_count for net in nets) == 1235
