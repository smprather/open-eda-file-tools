from __future__ import annotations

from click.testing import CliRunner

from lef_tools.cli import main

BUF = """\
MACRO BUF
  ORIGIN -0.5 0.25 ;
  SIZE 5.76 BY 3.33 ;
  PIN A
    DIRECTION INPUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0.125 1.35 1.43 1.78 ;
    END
  END A
END BUF
"""


def test_version_flag():
    result = CliRunner().invoke(main, ["--version"])

    assert result.exit_code == 0, result.output
    assert result.output.startswith("lef-tools, version ")


def test_macros_lists_micron_dimensions(tmp_path):
    lef = tmp_path / "cells.lef"
    lef.write_text(BUF, encoding="utf-8")

    result = CliRunner().invoke(main, ["macros", "--lef", str(lef)])

    assert result.exit_code == 0, result.output
    assert "macro\twidth_um\theight_um\torigin_x_um\torigin_y_um" in result.output
    assert "BUF\t5.76\t3.33\t-0.5\t0.25" in result.output


def test_show_prints_pins_and_rects(tmp_path):
    lef = tmp_path / "cells.lef"
    lef.write_text(BUF, encoding="utf-8")

    result = CliRunner().invoke(main, ["show", "--lef", str(lef), "--macro", "BUF"])

    assert result.exit_code == 0, result.output
    assert "pin A direction=input use=signal" in result.output
    assert "li1 0.125 1.35 1.43 1.78" in result.output
