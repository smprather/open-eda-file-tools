"""Gzip is detected consistently at the normal file-reading entry points."""

import gzip
from pathlib import Path

import pytest
from click.testing import CliRunner

import liberty_tools
import pex_tools
from def_tools import instance_bboxes
from def_tools.clock_data import read_clock_cells
from def_tools.instances import read_header
from def_tools.otd import read_otd
from lef_tools import parse_lef
from liberty_tools.cli import main as liberty_main
from sdc_tools import clocks

LIB = "library (test) { cell (BUF) { area : 1; } }\n"
LEF = "VERSION 5.8 ;\nMACRO BUF\n  SIZE 1 BY 1 ;\nEND BUF\nEND LIBRARY\n"
DEF = """\
VERSION 5.8 ;
DESIGN top ;
UNITS DISTANCE MICRONS 1000 ;
DIEAREA ( 0 0 ) ( 10000 10000 ) ;
COMPONENTS 1 ;
- u BUF + PLACED ( 10 20 ) N ;
END COMPONENTS
END DESIGN
"""
SPEF = '*SPEF "IEEE 1481-1999"\n*DESIGN "top"\n*C_UNIT 1 PF\n*D_NET clk 0\n*END\n'


@pytest.mark.parametrize("compressed", [True, False])
def test_format_readers_choose_compression_by_content(tmp_path: Path, compressed: bool) -> None:
    def input_file(name: str, text: str) -> Path:
        data = text.encode("utf-8")
        if compressed:
            # Split inside records to catch readers that silently stop at the
            # end of the first gzip member.
            middle = len(data) // 2
            data = gzip.compress(data[:middle]) + gzip.compress(data[middle:])
        path = tmp_path / (name if compressed else name + ".gz")
        path.write_bytes(data)
        return path

    lib = input_file("cells.lib", LIB)
    assert liberty_tools.parse_file(lib).cell("BUF").area == 1
    assert liberty_tools.LibraryIndex.open(lib).cell("BUF").area == 1

    lef = input_file("cells.lef", LEF)
    assert parse_lef(lef).macro("BUF").width_microns == 1
    design = input_file("design.def", DEF)
    assert read_header(design) == (1000, 1)
    assert len(list(instance_bboxes([lef], design))) == 1

    spef = input_file("design.spef", SPEF)
    assert pex_tools.header(spef).design == "top"
    assert [net.name for net in pex_tools.summaries(spef)] == ["clk"]
    assert [net.name for net in pex_tools.summaries(spef, mode="stream")] == ["clk"]

    otd = input_file("design.otd", "otd 1\nn clk c\nend 1 0 0\n")
    assert read_otd(otd).nets == {"clk": "c"}
    sdc = input_file("design.sdc", "create_clock -name clk -period 10 [get_ports clk]\r\n")
    assert [clock.name for clock in clocks(sdc)] == ["clk"]
    assert read_clock_cells(input_file("clock_cells.txt", '["u"]\n')) == {"u"}


def test_formatter_detects_gzip_under_a_plain_name(tmp_path: Path) -> None:
    path = tmp_path / "cells.lib"
    compressed = gzip.compress(LIB.encode("utf-8"))
    path.write_bytes(compressed)
    runner = CliRunner()
    result = runner.invoke(liberty_main, ["format", str(path)])
    assert result.exit_code == 0, result.output
    assert result.stdout == LIB
    result = runner.invoke(liberty_main, ["format", "--in-place", str(path)])
    assert result.exit_code == 1
    assert "in-place" in result.output
    assert path.read_bytes() == compressed


def test_truncated_gzip_with_a_plain_name_reports_io_error(tmp_path: Path) -> None:
    path = tmp_path / "design.otd"
    path.write_bytes(gzip.compress(b"otd 1\nend 0 0 0\n")[:-4])
    with pytest.raises(OSError, match="design.otd"):
        read_otd(path)
