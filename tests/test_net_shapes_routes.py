from __future__ import annotations

from pathlib import Path

import pytest
from click.testing import CliRunner

import def_tools
from def_tools.cli import main
from liberty_tools import _native


def write_def(tmp_path: Path, nets: str, specialnets: str = "") -> Path:
    """Wrap NETS/SPECIALNETS bodies in a minimal routed DEF."""
    sections = ""
    if specialnets:
        sections += "SPECIALNETS 1 ;\n" + specialnets + "\nEND SPECIALNETS\n"
    if nets:
        sections += "NETS 1 ;\n" + nets + "\nEND NETS\n"
    design = tmp_path / "design.def"
    design.write_text(
        "VERSION 5.8 ;\n"
        'DIVIDERCHAR "/" ;\n'
        'BUSBITCHARS "[]" ;\n'
        "DESIGN routes ;\n"
        "UNITS DISTANCE MICRONS 1000 ;\n"
        "COMPONENTS 2 ;\n"
        "  - u1 INV + PLACED ( 0 0 ) N ;\n"
        "  - u2 INV + FIXED ( 10 10 ) N ;\n"
        "END COMPONENTS\n"
        + sections
        + "END DESIGN\n",
        encoding="utf-8",
    )
    return design


def rows(design: Path, net: str, layer: str) -> list[tuple]:
    return [
        (
            shape.shape_type,
            shape.width,
            shape.x1,
            shape.y1,
            shape.x2,
            shape.y2,
            shape.llx,
            shape.lly,
            shape.urx,
            shape.ury,
        )
        for shape in def_tools.net_shapes(design, net, layer)
    ]


ROUTE = """
  - a ( u1 Y ) ( u2 A )
    + ROUTED met2 ( 100 200 ) ( 300 * ) ( * 500 )
    NEW met3 ( 300 500 0 ) ( 300 900 40 )
    NEW met2 ( 0 0 ) VIRTUAL ( 50 * ) ( 50 70 )
    NEW met2 ( 10 10 ) MASK 2 ( 20 * ) ;
"""


def test_star_repeats_previous_coordinate(tmp_path):
    design = write_def(tmp_path, ROUTE)
    assert rows(design, "a", "met2") == [
        ("", 0, 100, 200, 300, 200, 100, 200, 300, 200),
        ("", 0, 300, 200, 300, 500, 300, 200, 300, 500),
        ("", 0, 50, 0, 50, 70, 50, 0, 50, 70),
        ("", 0, 10, 10, 20, 10, 10, 10, 20, 10),
    ]


def test_extension_value_extends_segment_end(tmp_path):
    design = write_def(tmp_path, ROUTE)
    assert rows(design, "a", "met3") == [("", 0, 300, 500, 300, 900, 300, 500, 300, 940)]


def test_star_without_previous_point_is_an_error(tmp_path):
    design = write_def(tmp_path, "  - b ( u1 Y ) + ROUTED met2 ( * 5 ) ( 10 5 ) ;\n")
    with pytest.raises(RuntimeError, match=r"'\*' with no previous point near DEF byte \d+"):
        list(def_tools.net_shapes(design, "b", "met2"))


def test_openroad_route_from_the_failing_design(tmp_path):
    # Shape of the record that failed on cva6 (net _000000_).
    design = write_def(tmp_path, """
  - n0 ( u1 D ) ( u2 X ) + USE SIGNAL
      + ROUTED Metal3 ( 824640 895020 ) ( 830400 * )
      NEW Metal2 ( 824640 895020 ) ( * 897960 )
      NEW Metal1 ( 830400 895020 ) Via1_XY ;
""")
    assert rows(design, "n0", "Metal2") == [("", 0, 824640, 895020, 824640, 897960, 824640, 895020, 824640, 897960)]
    assert rows(design, "n0", "Metal3") == [("", 0, 824640, 895020, 830400, 895020, 824640, 895020, 830400, 895020)]


VIAS = """
  - b ( u1 Y ) ( u2 A )
    + ROUTED met2 TAPER ( 0 0 ) ( 100 * ) via12 ( 100 50 ) ( 200 * )
    NEW met1 TAPERRULE rule1 ( 0 0 ) RECT ( 30 40 -10 -20 )
    NEW met1 STYLE 2 ( 5 5 ) ( 5 15 ) via23 N ;
"""


def test_segments_after_a_via_are_skipped_and_counted(tmp_path):
    design = write_def(tmp_path, VIAS)
    assert rows(design, "b", "met2") == [("", 0, 0, 0, 100, 0, 0, 0, 100, 0)]


def test_rect_patch_rows(tmp_path):
    design = write_def(tmp_path, VIAS)
    assert rows(design, "b", "met1") == [
        ("RECT", 0, -10, -20, 30, 40, -10, -20, 30, 40),
        ("", 0, 5, 5, 5, 15, 5, 5, 5, 15),
    ]


def test_rect_without_previous_point_is_an_error(tmp_path):
    design = write_def(tmp_path, "  - c ( u1 Y ) + ROUTED met1 RECT ( 0 0 1 1 ) ;\n")
    with pytest.raises(RuntimeError, match="RECT with no previous point"):
        list(def_tools.net_shapes(design, "c", "met1"))


SPECIAL = """
  - VDD ( * VDD )
    + ROUTED met1 100 + SHAPE STRIPE + STYLE 1 ( 0 0 ) ( 1000 0 ) via12_a DO 2 BY 1 STEP 50 0 ( 1000 300 )
    NEW met2 200 ( 0 0 ) ( 0 500 )
    + RECT met3 ( 20 30 ) ( 10 10 )
    + POLYGON met3 ( 0 0 ) ( 10 0 ) ( 10 10 )
    + VIA via23_b ( 5 5 ) ( 6 6 )
    + USE POWER ;
"""


def test_special_net_keywords(tmp_path):
    design = write_def(tmp_path, "", specialnets=SPECIAL)
    assert rows(design, "VDD", "met1") == [("STRIPE", 100, 0, 0, 1000, 0, 0, -50, 1000, 50)]
    assert rows(design, "VDD", "met2") == [("", 200, 0, 0, 0, 500, -100, 0, 100, 500)]
    assert rows(design, "VDD", "met3") == [("RECT", 0, 10, 10, 20, 30, 10, 10, 20, 30)]
    assert next(def_tools.net_shapes(design, "VDD", "met3")).route_status == ""


def test_net_level_keywords_end_the_path(tmp_path):
    design = write_def(tmp_path, """
  - c ( u1 Y ) ( u2 A )
    + ROUTED met1 ( 0 0 ) ( 10 0 )
    + VPIN vp LAYER met1 ( 0 0 ) ( 9 9 ) PLACED ( 1 1 ) N
    + SUBNET s1 ( u1 A ) ROUTED met1 ( 0 0 ) ( 0 99 ) ;
""")
    assert rows(design, "c", "met1") == [("", 0, 0, 0, 10, 0, 0, 0, 10, 0)]


def test_writers_return_skip_counts(tmp_path):
    design = write_def(tmp_path, VIAS)
    counts = _native.write_net_shapes_csv(design, "b", "met2", tmp_path / "s.csv")
    assert counts == {"after_via": 2, "polygon": 0, "subnet": 0}


def test_iterator_exposes_skip_counts(tmp_path):
    design = write_def(tmp_path, "", specialnets=SPECIAL)
    shapes = def_tools.net_shapes(design, "VDD", "met1")
    list(shapes)
    assert shapes.skipped == {"after_via": 1, "polygon": 0, "subnet": 0}  # the polygon is on met3
    shapes = def_tools.net_shapes(design, "VDD", "met3")
    list(shapes)
    assert shapes.skipped == {"after_via": 1, "polygon": 1, "subnet": 0}


def test_cli_warns_about_skipped_pieces(tmp_path):
    design = write_def(tmp_path, VIAS)
    result = CliRunner().invoke(main, ["net-shapes", "--def", str(design), "--net", "b", "--layer", "met2"])
    assert result.exit_code == 0
    assert "warning: net-shapes skipped route pieces: after_via=2 polygon=0 subnet=0" in result.stderr


def test_cache_and_direct_queries_agree_on_new_constructs(tmp_path):
    design = write_def(tmp_path, ROUTE + VIAS, specialnets=SPECIAL)
    _native.write_def_cache(design, tmp_path / "cache")
    for net, layer in (("a", "met2"), ("b", "met1"), ("VDD", "met3")):
        direct, cached = tmp_path / "d.csv", tmp_path / "c.csv"
        assert _native.write_net_shapes_csv(design, net, layer, direct) == \
            _native.write_net_shapes_csv_from_cache(tmp_path / "cache", net, layer, cached)
        assert direct.read_bytes() == cached.read_bytes()



