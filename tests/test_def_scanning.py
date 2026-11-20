"""DEF scanning: CPU limit, batch/thread independence and record skipping."""

from __future__ import annotations

import subprocess
import sys
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


def write_design(
    tmp_path: Path, components: int, power_conns: int = 0
) -> tuple[Path, Path]:
    """A DEF with more components than one scan batch (65,536 records)."""
    tmp_path.mkdir(parents=True, exist_ok=True)
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
        lines.append(
            f"  - u{index} INV + PLACED ( {index * 10} {index % 97} ) {orient} ;"
        )
    lines += [
        "END COMPONENTS",
        "PINS 1 ;",
        "  - NETS + NET clk + DIRECTION INPUT ;",
        "END PINS",
    ]
    if power_conns:
        conns = " ".join(f"( t{index} VDD )" for index in range(power_conns))
        lines += [
            "SPECIALNETS 1 ;",
            f"  - VDD ( PIN VDD ) {conns}",
            "    + ROUTED met1 40 ( 0 0 ) ( 10 0 ) ;",
            "END SPECIALNETS",
        ]
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


# Output of the reference build (e98bd32) on the same DEF.
CACHE_ROUND_TRIP_CSV = (
    "clock,root_type,root_name,instance,macro,net,is_clock_cell\n"
    "clk,port,clk,ghost,,clk,false\n"
    "clk,port,clk,u0,INV,clk,true\n"
    "clk,port,clk,u1,CLKBUF,n1,false\n"
)


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
    csv, table, parquet = (
        tmp_path / f"{tag}.{ext}" for ext in ("csv", "txt", "parquet")
    )
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


def test_nets_before_components(tmp_path: Path) -> None:
    # Nets met before COMPONENTS are held until the components are known.
    _lef, design = write_design(tmp_path, components=3)
    text = design.read_text(encoding="utf-8")
    end = "END COMPONENTS\n"
    components = text[text.index("COMPONENTS") : text.index(end) + len(end)]
    nets = text[text.index("NETS 3 ;") : text.index("END NETS") + len("END NETS\n")]
    design.write_text(
        text.replace(components, "").replace(nets, nets + components), encoding="utf-8"
    )

    cells = _native.clock_tree_cells(
        design, ["port:clk"], [("INV", ["A"], ["Y"], False, [])], False
    )

    assert cells == ["u0", "u1", "u2"]


HEADER = 'VERSION 5.8 ;\nDIVIDERCHAR "/" ;\nDESIGN s ;\nUNITS DISTANCE MICRONS 1000 ;\n'
COMP = "COMPONENTS 3 ;\n  - u0 INV + PLACED ( 0 0 ) N ;\n  - u1 INV + PLACED ( 10 0 ) N ;\n  - u2 INV + PLACED ( 20 0 ) N ;\nEND COMPONENTS\n"
NETS = (
    "NETS 3 ;\n  - clk ( PIN clk ) ( u0 A ) ;\n  - n1 ( u0 Y ) ( u1 A ) ;\n"
    "  - n2 ( u1 Y ) ( u2 A ) ;\nEND NETS\n"
)
SPECIAL = (
    "SPECIALNETS 1 ;\n  - VDD ( u0 VDD ) ( u1 VDD ) + USE POWER ;\nEND SPECIALNETS\n"
)
INV_ROLES = [("INV", ["A"], ["Y"], False, [])]


def _trace(design: Path) -> list[str]:
    return _native.clock_tree_cells(design, ["port:clk"], INV_ROLES, False)


def test_specialnets_before_components(tmp_path: Path) -> None:
    design = tmp_path / "d.def"
    design.write_text(HEADER + SPECIAL + NETS + COMP + "END DESIGN\n", encoding="utf-8")

    assert _trace(design) == ["u0", "u1", "u2"]


def test_nets_without_components_section(tmp_path: Path) -> None:
    # Instances known only from nets have no macro, so no role: same as the
    # reference build, which returns no cells.
    design = tmp_path / "d.def"
    design.write_text(HEADER + NETS + "END DESIGN\n", encoding="utf-8")

    assert _trace(design) == []


def test_components_after_nets_is_an_error_with_a_byte_offset(tmp_path: Path) -> None:
    text = HEADER + COMP + NETS + COMP + "END DESIGN\n"
    design = tmp_path / "d.def"
    design.write_text(text, encoding="utf-8")
    second = text.rindex("COMPONENTS 3")

    with pytest.raises(
        Exception, match=rf"second COMPONENTS section near DEF byte {second}:"
    ):
        _trace(design)


def test_cache_create_refuses_a_second_components_section_after_nets(
    tmp_path: Path,
) -> None:
    # The cache path rejects the same input, with the same error, as the
    # DEF-backed readers.
    text = HEADER + COMP + NETS + COMP + "END DESIGN\n"
    design = tmp_path / "d.def"
    design.write_text(text, encoding="utf-8")
    second = text.rindex("COMPONENTS 3")

    with pytest.raises(
        Exception, match=rf"second COMPONENTS section near DEF byte {second}:"
    ):
        def_tools.write_def_cache(design, tmp_path / "cache")


def test_cache_round_trip_with_duplicate_and_unknown_instances(tmp_path: Path) -> None:
    components = (
        "COMPONENTS 3 ;\n  - u0 INV + PLACED ( 0 0 ) N ;\n  - u1 INV + PLACED ( 10 0 ) N ;\n"
        "  - u1 CLKBUF + PLACED ( 10 0 ) N ;\nEND COMPONENTS\n"
    )
    nets = (
        "NETS 2 ;\n  - clk ( PIN clk ) ( u0 A ) ( ghost A ) ;\n"
        "  - n1 ( u0 Y ) ( u1 A ) ;\nEND NETS\n"
    )
    design = tmp_path / "d.def"
    design.write_text(HEADER + components + nets + "END DESIGN\n", encoding="utf-8")
    cache = tmp_path / "cache"
    roots = [("clk", "port", "clk")]

    def_tools.write_def_cache(design, cache)
    def_tools.write_clock_instances_csv(design, roots, ["inv"], tmp_path / "def.csv")
    def_tools.write_clock_instances_csv_from_cache(
        cache, roots, ["inv"], tmp_path / "cache.csv"
    )

    from_def = (tmp_path / "def.csv").read_text(encoding="utf-8")
    assert (tmp_path / "cache.csv").read_text(encoding="utf-8") == from_def
    assert from_def == CACHE_ROUND_TRIP_CSV


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

    components = "\n".join(
        f"  - t{index} TIE + PLACED ( 0 0 ) N ;" for index in range(80_000)
    )
    text = design.read_text(encoding="utf-8").replace(
        "COMPONENTS 3 ;", f"COMPONENTS 80003 ;\n{components}"
    )
    design.write_text(text, encoding="utf-8")
    cells, *_ = _native.clock_object_instances(design, ["VDD"], [], ties)
    assert len(cells) == 80_000
    assert cells == sorted(f"t{index}" for index in range(80_000))


def test_malformed_connection_deep_in_a_long_list_is_reported(tmp_path: Path) -> None:
    _lef, design = write_design(tmp_path, components=3, power_conns=80_000)
    text = design.read_text(encoding="utf-8").replace(
        "( t70000 VDD )", "( t70000 VDD extra )"
    )
    design.write_text(text, encoding="utf-8")

    with pytest.raises(RuntimeError, match="net connection"):
        _native.clock_object_instances(design, ["VDD"], [], [])


def test_cli_threads_option_sets_the_limit_first(tmp_path: Path) -> None:
    lef, design = write_design(tmp_path, components=10)
    output = tmp_path / "out.csv"

    result = CliRunner().invoke(
        main,
        [
            "instances",
            "--lef",
            str(lef),
            "--def",
            str(design),
            "--threads",
            "2",
            "--output",
            str(output),
        ],
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
    text = design.read_text(encoding="utf-8").replace(
        "( u1 A )", "( u1 A + SYNTHESIZED )"
    )
    design.write_text(text, encoding="utf-8")
    cells = _native.clock_tree_cells(
        design, ["port:clk"], [("INV", ["A"], ["Y"], False, [])], False
    )
    assert cells == ["u0", "u1", "u2"]
    _native.write_def_cache(design, tmp_path / "cache")  # must not raise


def test_synthesized_in_split_connection_list(tmp_path):
    """A >1 MiB list splits for parallel parsing; `+ SYNTHESIZED` must not end it."""
    count = 160_000
    _lef, plain = write_design(tmp_path / "plain", components=3, power_conns=count)
    _lef, marked = write_design(tmp_path / "marked", components=3, power_conns=count)
    text = marked.read_text(encoding="utf-8")
    for index in (count // 2, count - 5):
        text = text.replace(f"( t{index} VDD )", f"( t{index} VDD + SYNTHESIZED )")
    assert text.count("SYNTHESIZED") == 2
    marked.write_text(text, encoding="utf-8")
    assert len(text.split("+ ROUTED")[0]) > 1 << 20
    cells = [
        _native.clock_tree_cells(
            path, ["port:clk"], [("INV", ["A"], ["Y"], False, [])], False
        )
        for path in (plain, marked)
    ]
    assert cells[0] == cells[1]
    _native.write_def_cache(plain, tmp_path / "plain_cache")
    _native.write_def_cache(marked, tmp_path / "marked_cache")
    names = sorted(p.name for p in (tmp_path / "plain_cache").iterdir())
    assert "connectivity.bin" in names
    for name in names:
        if name in ("nets.dat", "nets.idx"):  # raw records keep the marker text
            continue
        assert (tmp_path / "plain_cache" / name).read_bytes() == (
            tmp_path / "marked_cache" / name
        ).read_bytes(), name


FANOUT_ROLES = [("INV", ["A"], ["Y"], False, []), ("CLKBUF", ["A"], ["Y"], False, [])]


def write_fanout_design(path: Path, nets: int) -> Path:
    """One clock buffer `u0` driving `nets` nets, each read by an inverter."""
    lines = [
        "VERSION 5.8 ;",
        'DIVIDERCHAR "/" ;',
        "DESIGN s ;",
        "UNITS DISTANCE MICRONS 1000 ;",
        f"COMPONENTS {nets + 1} ;",
        "  - u0 CLKBUF + PLACED ( 0 0 ) N ;",
    ]
    lines += [f"  - h{i} INV + PLACED ( 0 0 ) N ;" for i in range(nets)]
    lines += ["END COMPONENTS", f"NETS {nets + 1} ;", "  - clk ( PIN clk ) ( u0 A ) ;"]
    lines += [f"  - n{i} ( u0 Y ) ( h{i} A ) ;" for i in range(nets)]
    lines += ["END NETS", "END DESIGN", ""]
    path.write_text("\n".join(lines), encoding="utf-8")
    return path


SMALL_FANOUT_DEF = """\
VERSION 5.8 ;
DIVIDERCHAR "/" ;
DESIGN s ;
UNITS DISTANCE MICRONS 1000 ;
COMPONENTS 7 ;
  - u0 CLKBUF + PLACED ( 0 0 ) N ;
  - u1 CLKBUF + PLACED ( 0 0 ) N ;
  - h0 INV + PLACED ( 0 0 ) N ;
  - h1 INV + PLACED ( 0 0 ) N ;
  - h2 INV + PLACED ( 0 0 ) N ;
  - h3 INV + PLACED ( 0 0 ) N ;
  - hk INV + PLACED ( 0 0 ) N ;
END COMPONENTS
NETS 5 ;
  - clk ( PIN clk ) ( u0 A ) ;
  - n0 ( u0 Y ) ( u1 A ) ( h0 A ) ( hk A ) ;
  - n1 ( u0 Y ) ( hk A ) ( h1 A ) ;
  - n2 ( u1 Y ) ( h2 A ) ( h3 A ) ;
  - n3 ( u1 Y ) ( u0 A ) ( hk A ) ;
END NETS
END DESIGN
"""

# Literals produced by the reference build (commit e98bd32, before the
# per-instance expansion change) on SMALL_FANOUT_DEF.
SMALL_FANOUT_CSV = (
    "clock,root_type,root_name,instance,macro,net,is_clock_cell\n"
    "clk,port,clk,h0,INV,n0,false\n"
    "clk,port,clk,h1,INV,n1,false\n"
    "clk,port,clk,h2,INV,n2,false\n"
    "clk,port,clk,h3,INV,n2,false\n"
    "clk,port,clk,hk,INV,n0,false\n"
    "clk,port,clk,u0,CLKBUF,clk,true\n"
    "clk,port,clk,u1,CLKBUF,n0,true\n"
)
SMALL_FANOUT_CELLS = ["h0", "h1", "h2", "h3", "hk", "u0", "u1"]


def test_fanout_small_output_matches_reference(tmp_path: Path) -> None:
    """hk is reached by n0 and n1 (first net n0); u1 is a second clock cell."""
    design = tmp_path / "small.def"
    design.write_text(SMALL_FANOUT_DEF, encoding="utf-8")
    out = tmp_path / "out.csv"
    def_tools.write_clock_instances_csv(design, [("clk", "port", "clk")], ["clk"], out)
    assert out.read_text(encoding="utf-8") == SMALL_FANOUT_CSV
    cells = _native.clock_tree_cells(design, ["port:clk"], FANOUT_ROLES, False)
    assert cells == SMALL_FANOUT_CELLS


FANOUT_CHILD = """
import resource, sys
resource.setrlimit(resource.RLIMIT_AS, (4 << 30, 4 << 30))
import def_tools
from liberty_tools import _native
_native.set_threads(1)
design, out = sys.argv[1], sys.argv[2]
roles = [("INV", ["A"], ["Y"], False, []), ("CLKBUF", ["A"], ["Y"], False, [])]
def_tools.write_clock_instances_csv(design, [("clk", "port", "clk")], ["clk"], out)
cells = _native.clock_tree_cells(design, ["port:clk"], roles, False)
assert len(cells) == int(sys.argv[3]) + 1, len(cells)
"""


@pytest.mark.skipif(
    not sys.platform.startswith("linux"), reason="RLIMIT_AS is only enforced on Linux"
)
def test_fanout_clock_cell_traversal_memory_is_bounded(tmp_path: Path) -> None:
    """A clock cell on N nets must not queue its nets once per net (O(N^2) memory)."""
    count = 140_000
    design = write_fanout_design(tmp_path / "fan.def", count)
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            FANOUT_CHILD,
            str(design),
            str(tmp_path / "o.csv"),
            str(count),
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert result.returncode == 0, result.stderr[-2000:]


# Item 7: roots whose instance is not a plain, unique component. Expected
# values are the output of the reference build (e98bd32) on the same DEFs.
ONLY_IN_NETS_DEF = (
    HEADER
    + "COMPONENTS 2 ;\n  - u0 INV + PLACED ( 0 0 ) N ;\n  - u1 INV + PLACED ( 10 0 ) N ;\n"
    + "END COMPONENTS\nNETS 2 ;\n  - n1 ( g Y ) ( u0 A ) ;\n  - n2 ( u0 Y ) ( u1 A ) ;\n"
    + "END NETS\nEND DESIGN\n"
)
DUPLICATE_DEF = (
    HEADER
    + "COMPONENTS 4 ;\n  - u1 INV + PLACED ( 0 0 ) N ;\n  - u1 BUF + PLACED ( 0 0 ) N ;\n"
    + "  - u2 INV + PLACED ( 10 0 ) N ;\n  - u3 INV + PLACED ( 20 0 ) N ;\nEND COMPONENTS\n"
    + "NETS 3 ;\n  - n1 ( u1 Y ) ( u2 A ) ;\n  - n2 ( u1 Z ) ( u3 A ) ;\n"
    + "  - n0 ( u1 I ) ( u1 A ) ;\nEND NETS\nEND DESIGN\n"
)
BUF_ROLE = ("BUF", ["I"], ["Z"], False, [])


def test_pin_root_on_an_instance_that_only_appears_in_nets(tmp_path: Path) -> None:
    design = tmp_path / "d.def"
    design.write_text(ONLY_IN_NETS_DEF, encoding="utf-8")

    cells = _native.clock_tree_cells(design, ["pin:g/Y"], INV_ROLES, False)
    pruned = _native.clock_tree_cells(design, ["pin:g/Y"], INV_ROLES, True)

    assert cells == ["u0", "u1"]
    assert pruned == []


def test_cell_root_on_a_duplicated_component_uses_the_last_definition(
    tmp_path: Path,
) -> None:
    # u1 is defined as INV, then as BUF; nets resolve to the BUF definition, so
    # only its I -> Z arc (not INV's A -> Y) carries the clock.
    design = tmp_path / "d.def"
    design.write_text(DUPLICATE_DEF, encoding="utf-8")

    def trace(roles: list[Any]) -> list[str]:
        return _native.clock_tree_cells(design, ["cell:u1"], roles, False)

    assert trace(INV_ROLES + [BUF_ROLE]) == ["u3"]
    assert trace(INV_ROLES) == []
    assert trace([BUF_ROLE]) == []
