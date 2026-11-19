"""DEF scanning: CPU limit, batch/thread independence and record skipping."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from click.testing import CliRunner

import def_tools
from def_tools.cli import main
from liberty_tools import _native

LEF = """\
VERSION 5.8 ;
UNITS DATABASE MICRONS 1000 ;
MACRO INV
  SIZE 1.2 BY 2.4 ;
END INV
MACRO TIE
  SIZE 0.5 BY 2.4 ;
END TIE
"""


@pytest.fixture(autouse=True)
def _reset_thread_limit():
    yield
    _native.set_threads(None)


def write_design(tmp_path: Path, components: int, power_conns: int = 0) -> tuple[Path, Path]:
    """A DEF with more components than one scan batch (65,536 records)."""
    lef = tmp_path / "cells.lef"
    lef.write_text(LEF, encoding="utf-8")
    lines = [
        "VERSION 5.8 ;",
        'DIVIDERCHAR "/" ;',
        "DESIGN scan ;",
        "UNITS DISTANCE MICRONS 1000 ;",
        "VIAS 1 ;",
        "  - NETS + RECT met1 ( 0 0 ) ( 1 1 ) ;",
        "END VIAS",
        f"COMPONENTS {components} ;",
    ]
    for index in range(components):
        orient = ("N", "FS", "E", "FW")[index % 4]
        lines.append(f"  - u{index} INV + PLACED ( {index * 10} {index % 97} ) {orient} ;")
    lines += [
        "END COMPONENTS",
        "PINS 1 ;",
        "  - NETS + NET clk + DIRECTION INPUT ;",
        "END PINS",
    ]
    if power_conns:
        conns = " ".join(f"( t{index} VDD )" for index in range(power_conns))
        lines += ["SPECIALNETS 1 ;", f"  - VDD ( PIN VDD ) {conns}", "    + ROUTED met1 40 ( 0 0 ) ( 10 0 ) ;", "END SPECIALNETS"]
    lines += [
        "NETS 3 ;",
        "  - clk ( PIN clk ) ( u0 A ) + USE CLOCK ;",
        "  - n1 ( u0 Y ) ( u1 A )",
        "    + ROUTED met1 ( 0 0 ) ( 100 0 ) NEW met2 ( 100 0 ) ( 100 50 ) ;",
        "  - n2 ( u1 Y ) ( u2 A ) ;",
        "END NETS",
        "END DESIGN",
    ]
    design = tmp_path / "design.def"
    design.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return lef, design


def test_set_threads_limits_and_resets() -> None:
    default = _native.get_threads()
    assert default >= 1

    def_tools.set_threads(2)
    assert def_tools.get_threads() == 2

    def_tools.set_threads(None)
    assert def_tools.get_threads() == default


def test_set_threads_rejects_absurd_values() -> None:
    with pytest.raises(ValueError, match="threads"):
        _native.set_threads(100_000)


def outputs(tmp_path: Path, lef: Path, design: Path, tag: str) -> dict[str, Any]:
    csv, table, parquet = (tmp_path / f"{tag}.{ext}" for ext in ("csv", "txt", "parquet"))
    cache = tmp_path / f"{tag}-cache"
    count = def_tools.write_instance_bbox_csv([lef], design, csv)
    def_tools.write_instance_table([lef], design, table)
    def_tools.write_instance_parquet([lef], design, parquet, 30_000)
    def_tools.write_def_cache(design, cache)
    return {
        "count": count,
        "csv": csv.read_bytes(),
        "table": table.read_bytes(),
        "parquet": parquet.read_bytes(),
        "cache": {path.name: path.read_bytes() for path in sorted(cache.iterdir())},
        "iter": [row.as_tuple() for row in def_tools.instance_bboxes([lef], design)],
        "trace": _native.clock_tree_cells(
            design, ["port:clk"], [("INV", ["A"], ["Y"], False, [])], False
        ),
        "shapes": [repr(shape) for shape in def_tools.net_shapes(design, "n1", "met2")],
    }


def test_outputs_do_not_depend_on_the_thread_limit(tmp_path: Path) -> None:
    lef, design = write_design(tmp_path, components=70_000)

    unlimited = outputs(tmp_path, lef, design, "all")
    _native.set_threads(1)
    single = outputs(tmp_path, lef, design, "one")
    _native.set_threads(3)
    three = outputs(tmp_path, lef, design, "three")

    assert unlimited["count"] == 70_000
    assert unlimited["trace"] == ["u0", "u1", "u2"]
    assert len(unlimited["shapes"]) == 1
    assert single == unlimited
    assert three == unlimited


def test_records_named_like_section_keywords_are_skipped(tmp_path: Path) -> None:
    # A via and a pin named NETS used to be read as the start of the NETS
    # section by the net scanners, which tokenized every record.
    lef, design = write_design(tmp_path, components=3)

    shapes = list(def_tools.net_shapes(design, "n1", "met1"))
    cells = _native.clock_tree_cells(
        design, ["port:clk"], [("INV", ["A"], ["Y"], False, [])], False
    )

    assert [(shape.x1, shape.x2) for shape in shapes] == [(0, 100)]
    assert cells == ["u0", "u1", "u2"]
    assert def_tools.write_instance_bbox_csv([lef], design, tmp_path / "i.csv") == 3


def test_long_connection_lists_parse_in_pieces(tmp_path: Path) -> None:
    # 80,000 connections is a ~1.4 MB list: split into pieces parsed in
    # parallel, and the record straddles the 1 MiB read buffer.
    _lef, design = write_design(tmp_path, components=3, power_conns=80_000)
    ties = [("TIE", [], ["VDD"], False, [])]

    cells, unmatched_nets, _pins, _unclassified = _native.clock_object_instances(
        design, ["VDD"], [], ties
    )
    # Instances absent from COMPONENTS are not classified.
    assert cells == [] and unmatched_nets == []

    components = "\n".join(f"  - t{index} TIE + PLACED ( 0 0 ) N ;" for index in range(80_000))
    text = design.read_text(encoding="utf-8").replace(
        "COMPONENTS 3 ;", f"COMPONENTS 80003 ;\n{components}"
    )
    design.write_text(text, encoding="utf-8")
    cells, *_ = _native.clock_object_instances(design, ["VDD"], [], ties)
    assert len(cells) == 80_000
    assert cells == sorted(f"t{index}" for index in range(80_000))


def test_malformed_connection_deep_in_a_long_list_is_reported(tmp_path: Path) -> None:
    _lef, design = write_design(tmp_path, components=3, power_conns=80_000)
    text = design.read_text(encoding="utf-8").replace("( t70000 VDD )", "( t70000 VDD extra )")
    design.write_text(text, encoding="utf-8")

    with pytest.raises(RuntimeError, match="net connection"):
        _native.clock_object_instances(design, ["VDD"], [], [])


def test_cli_threads_option_sets_the_limit_first(tmp_path: Path) -> None:
    lef, design = write_design(tmp_path, components=10)
    output = tmp_path / "out.csv"

    result = CliRunner().invoke(
        main,
        ["instances", "--lef", str(lef), "--def", str(design), "--threads", "2", "--output", str(output)],
    )

    assert result.exit_code == 0, result.output
    assert _native.get_threads() == 2
    assert len(output.read_text(encoding="utf-8").splitlines()) == 11


@pytest.mark.parametrize(
    "args",
    [
        ["cache", "create", "--output", "cache"],
        ["net-shapes", "--net", "n1", "--layer", "met1"],
    ],
)
def test_threads_option_on_other_commands(
    tmp_path: Path, args: list[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    _lef, design = write_design(tmp_path, components=10)
    monkeypatch.chdir(tmp_path)

    result = CliRunner().invoke(main, [*args, "--def", str(design), "--threads", "1"])

    assert result.exit_code == 0, result.output
    assert _native.get_threads() == 1


def test_threads_must_be_positive(tmp_path: Path) -> None:
    lef, design = write_design(tmp_path, components=1)

    result = CliRunner().invoke(
        main, ["instances", "--lef", str(lef), "--def", str(design), "--threads", "0"]
    )

    assert result.exit_code == 2


def test_synthesized_connections_are_kept(tmp_path):
    lef, design = write_design(tmp_path, components=3)
    text = design.read_text(encoding="utf-8").replace("( u1 A )", "( u1 A + SYNTHESIZED )")
    design.write_text(text, encoding="utf-8")
    cells = _native.clock_tree_cells(design, ["port:clk"], [("INV", ["A"], ["Y"], False, [])], False)
    assert cells == ["u0", "u1", "u2"]
    _native.write_def_cache(design, tmp_path / "cache")  # must not raise
