from __future__ import annotations

import gzip
import pathlib

import pytest

from lef_tools import parse_lef


def write_lef(tmp_path, name, text):
    p = tmp_path / name
    p.write_text(text, encoding="utf-8")
    return p


REP = """\
VERSION 5.8 ;
MACRO BUF
  ORIGIN 0 0 ;
  SIZE 5.76 BY 3.33 ;
  PIN A
    DIRECTION INPUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0.125 1.35 1.43 1.78 ;
    END
  END A
  PIN X
    DIRECTION OUTPUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 1.96 0.35 2.13 0.96 ;
      RECT 1.96 0.96 5.145 1.13 ;
    END
  END X
  PIN VGND
    DIRECTION INOUT ;
    USE GROUND ;
    PORT
      LAYER met1 ;
      RECT 0 -0.245 5.76 0.245 ;
    END
  END VGND
  PIN VPWR
    DIRECTION INOUT ;
    USE POWER ;
    PORT
      LAYER met1 ;
      RECT 0 3.085 5.76 3.575 ;
    END
  END VPWR
END BUF
END LIBRARY
"""


def test_macro_name_size_negative_origin(tmp_path):
    path = write_lef(
        tmp_path,
        "pad.lef",
        """\
VERSION 5.8 ;
MACRO PAD
  ORIGIN -0.5 -1.25 ;
  SIZE 12.5 BY 7.25 ;
END PAD
MACRO BLK
  SIZE 2.0 BY 1.0 ;
END BLK
END LIBRARY
""",
    )
    lib = parse_lef(path)

    macro = lib.macro("PAD")
    assert macro.name == "PAD"
    assert macro.width_microns == pytest.approx(12.5)
    assert macro.height_microns == pytest.approx(7.25)
    assert macro.origin_x_microns == pytest.approx(-0.5)
    assert macro.origin_y_microns == pytest.approx(-1.25)
    assert isinstance(macro.width_microns, float)
    assert isinstance(macro.height_microns, float)
    assert isinstance(macro.origin_x_microns, float)
    assert isinstance(macro.origin_y_microns, float)

    blk = lib.macro("BLK")
    assert blk.origin_x_microns == pytest.approx(0.0)
    assert blk.origin_y_microns == pytest.approx(0.0)


def test_signal_and_power_ground_pin_metadata(tmp_path):
    path = write_lef(tmp_path, "cells.lef", REP)
    lib = parse_lef(path)
    macro = lib.macro("BUF")

    assert macro.pin_names() == ["A", "X", "VGND", "VPWR"]

    expected = {
        "A": ("input", "signal"),
        "X": ("output", "signal"),
        "VGND": ("inout", "ground"),
        "VPWR": ("inout", "power"),
    }
    for name, (direction, use) in expected.items():
        pin = macro.pin(name)
        assert pin.direction == direction
        assert pin.use == use


def test_multiple_rectangles_on_one_output_pin(tmp_path):
    path = write_lef(tmp_path, "cells.lef", REP)
    lib = parse_lef(path)
    macro = lib.macro("BUF")
    pin = macro.pin("X")

    ports = pin.ports()
    assert len(ports) == 1
    rects = ports[0].rects()
    assert len(rects) == 2

    r0 = rects[0]
    assert r0.layer == "li1"
    assert r0.x0_microns == pytest.approx(1.96)
    assert r0.y0_microns == pytest.approx(0.35)
    assert r0.x1_microns == pytest.approx(2.13)
    assert r0.y1_microns == pytest.approx(0.96)

    r1 = rects[1]
    assert r1.layer == "li1"
    assert r1.x0_microns == pytest.approx(1.96)
    assert r1.y0_microns == pytest.approx(0.96)
    assert r1.x1_microns == pytest.approx(5.145)
    assert r1.y1_microns == pytest.approx(1.13)


def test_multiple_layers_within_one_port(tmp_path):
    path = write_lef(
        tmp_path,
        "layers.lef",
        """\
VERSION 5.8 ;
MACRO M
  SIZE 10 BY 10 ;
  PIN P
    DIRECTION INOUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0 0 1 1 ;
      LAYER met1 ;
      RECT 2 2 3 3 ;
      RECT 4 4 5 5 ;
    END
  END P
END M
END LIBRARY
""",
    )
    lib = parse_lef(path)
    macro = lib.macro("M")
    port = macro.pin("P").ports()[0]
    rects = port.rects()
    assert len(rects) == 3
    assert [r.layer for r in rects] == ["li1", "met1", "met1"]
    assert (rects[0].x0_microns, rects[0].y0_microns, rects[0].x1_microns, rects[0].y1_microns) == (
        pytest.approx(0.0),
        pytest.approx(0.0),
        pytest.approx(1.0),
        pytest.approx(1.0),
    )
    assert (rects[1].x0_microns, rects[1].y0_microns, rects[1].x1_microns, rects[1].y1_microns) == (
        pytest.approx(2.0),
        pytest.approx(2.0),
        pytest.approx(3.0),
        pytest.approx(3.0),
    )
    assert (rects[2].x0_microns, rects[2].y0_microns, rects[2].x1_microns, rects[2].y1_microns) == (
        pytest.approx(4.0),
        pytest.approx(4.0),
        pytest.approx(5.0),
        pytest.approx(5.0),
    )


def test_multiple_ports_within_one_pin(tmp_path):
    path = write_lef(
        tmp_path,
        "ports.lef",
        """\
VERSION 5.8 ;
MACRO M
  SIZE 10 BY 10 ;
  PIN P
    DIRECTION INOUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0 0 1 1 ;
    END
    PORT
      LAYER met1 ;
      RECT 5 5 6 6 ;
    END
  END P
END M
END LIBRARY
""",
    )
    lib = parse_lef(path)
    ports = lib.macro("M").pin("P").ports()
    assert len(ports) == 2

    r0 = ports[0].rects()
    assert len(r0) == 1
    assert r0[0].layer == "li1"
    assert r0[0].x0_microns == pytest.approx(0.0)
    assert r0[0].y0_microns == pytest.approx(0.0)
    assert r0[0].x1_microns == pytest.approx(1.0)
    assert r0[0].y1_microns == pytest.approx(1.0)

    r1 = ports[1].rects()
    assert len(r1) == 1
    assert r1[0].layer == "met1"
    assert r1[0].x0_microns == pytest.approx(5.0)
    assert r1[0].y0_microns == pytest.approx(5.0)
    assert r1[0].x1_microns == pytest.approx(6.0)
    assert r1[0].y1_microns == pytest.approx(6.0)


def test_negative_power_rail_coordinates_preserved(tmp_path):
    path = write_lef(tmp_path, "cells.lef", REP)
    lib = parse_lef(path)
    macro = lib.macro("BUF")

    vgnd = macro.pin("VGND").ports()[0].rects()[0]
    assert vgnd.layer == "met1"
    assert vgnd.y0_microns < 0
    assert vgnd.y0_microns == pytest.approx(-0.245)
    assert vgnd.x0_microns == pytest.approx(0.0)
    assert vgnd.x1_microns == pytest.approx(5.76)
    assert vgnd.y1_microns == pytest.approx(0.245)

    vpwr = macro.pin("VPWR").ports()[0].rects()[0]
    assert vpwr.y1_microns == pytest.approx(3.575)
    assert vpwr.y1_microns > macro.height_microns


def test_reversed_rectangle_coordinates_normalized(tmp_path):
    path = write_lef(
        tmp_path,
        "rev.lef",
        """\
VERSION 5.8 ;
MACRO M
  SIZE 10 BY 10 ;
  PIN P
    DIRECTION INOUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 3.5 4.5 1.5 0.5 ;
    END
  END P
END M
END LIBRARY
""",
    )
    lib = parse_lef(path)
    rect = lib.macro("M").pin("P").ports()[0].rects()[0]
    assert rect.x0_microns == pytest.approx(1.5)
    assert rect.y0_microns == pytest.approx(0.5)
    assert rect.x1_microns == pytest.approx(3.5)
    assert rect.y1_microns == pytest.approx(4.5)


def test_unsupported_geometry_reported_without_dropping_supported_rects(tmp_path):
    path = write_lef(
        tmp_path,
        "unsup.lef",
        """\
VERSION 5.8 ;
MACRO M
  SIZE 10 BY 10 ;
  PIN POLY
    DIRECTION INOUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0 0 1 1 ;
      POLYGON 0 0 1 0 1 1 0 1 ;
    END
  END POLY
  PIN PTH
    DIRECTION INOUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0 0 1 1 ;
      PATH 0 0 1 1 ;
    END
  END PTH
  PIN V
    DIRECTION INOUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0 0 1 1 ;
      VIA 1.5 2.5 M1M2_VIA ;
    END
  END V
  PIN ONLY_UNSUP
    DIRECTION INOUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      POLYGON 0 0 1 0 1 1 0 1 ;
    END
  END ONLY_UNSUP
  PIN NO_PORT
    DIRECTION INOUT ;
    USE SIGNAL ;
  END NO_PORT
  PIN CLEAN
    DIRECTION INOUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 7 7 8 8 ;
    END
  END CLEAN
END M
END LIBRARY
""",
    )
    lib = parse_lef(path)
    macro = lib.macro("M")

    for name in ("POLY", "PTH", "V"):
        port = macro.pin(name).ports()[0]
        assert port.has_unsupported_geometry is True
        rects = port.rects()
        assert len(rects) == 1
        assert rects[0].x0_microns == pytest.approx(0.0)
        assert rects[0].y0_microns == pytest.approx(0.0)
        assert rects[0].x1_microns == pytest.approx(1.0)
        assert rects[0].y1_microns == pytest.approx(1.0)

    only_unsup = macro.pin("ONLY_UNSUP").ports()
    assert len(only_unsup) == 1
    assert only_unsup[0].has_unsupported_geometry is True
    assert only_unsup[0].rects() == []

    assert macro.pin("NO_PORT").ports() == []

    clean_port = macro.pin("CLEAN").ports()[0]
    assert clean_port.has_unsupported_geometry is False
    assert len(clean_port.rects()) == 1


def test_gzipped_input(tmp_path):
    gz_path = tmp_path / "cells.lef.gz"
    with gzip.open(gz_path, "wt", encoding="utf-8") as f:
        f.write(REP)
    lib = parse_lef(gz_path)
    macro = lib.macro("BUF")
    assert macro.name == "BUF"
    assert macro.width_microns == pytest.approx(5.76)
    assert macro.height_microns == pytest.approx(3.33)


def test_multiple_files_in_input_order(tmp_path):
    a = write_lef(
        tmp_path,
        "a.lef",
        """\
VERSION 5.8 ;
MACRO AAA
  SIZE 1 BY 1 ;
END AAA
END LIBRARY
""",
    )
    b = write_lef(
        tmp_path,
        "b.lef",
        """\
VERSION 5.8 ;
MACRO BBB
  SIZE 1 BY 1 ;
END BBB
END LIBRARY
""",
    )

    assert parse_lef([a, b]).macro_names() == ["AAA", "BBB"]
    assert parse_lef([b, a]).macro_names() == ["BBB", "AAA"]

    assert parse_lef(a).macro_names() == ["AAA"]
    assert parse_lef(pathlib.Path(a)).macro_names() == ["AAA"]
    assert parse_lef(str(a)).macro_names() == ["AAA"]


def test_last_duplicate_macro_wins_and_reported_once(tmp_path):
    f1 = write_lef(
        tmp_path,
        "f1.lef",
        """\
VERSION 5.8 ;
MACRO DUP
  SIZE 1 BY 1 ;
END DUP
END LIBRARY
""",
    )
    f2 = write_lef(
        tmp_path,
        "f2.lef",
        """\
VERSION 5.8 ;
MACRO DUP
  SIZE 2 BY 2 ;
END DUP
END LIBRARY
""",
    )
    f3 = write_lef(
        tmp_path,
        "f3.lef",
        """\
VERSION 5.8 ;
MACRO DUP
  SIZE 3 BY 3 ;
END DUP
END LIBRARY
""",
    )
    lib = parse_lef([f1, f2, f3])

    assert lib.macro("DUP").width_microns == pytest.approx(3.0)
    assert lib.duplicate_macro_names() == ["DUP"]
    assert lib.macro_names().count("DUP") == 1


def test_macro_filtering(tmp_path):
    path = write_lef(
        tmp_path,
        "filter.lef",
        """\
VERSION 5.8 ;
MACRO KEEP
  SIZE 1 BY 1 ;
END KEEP
MACRO SKIP
  SIZE 2 BY 2 ;
  PIN S
    DIRECTION INPUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0 0 1 1 ;
    END
  END S
END SKIP
END LIBRARY
""",
    )

    assert parse_lef(path, macros=["KEEP"]).macro_names() == ["KEEP"]
    assert parse_lef(path, macros=["NOPE"]).macro_names() == []


def test_keyerror_for_unknown_names(tmp_path):
    path = write_lef(
        tmp_path,
        "ke.lef",
        """\
VERSION 5.8 ;
MACRO BUF
  SIZE 1 BY 1 ;
  PIN A
    DIRECTION INPUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0 0 1 1 ;
    END
  END A
END BUF
END LIBRARY
""",
    )
    lib = parse_lef(path)
    macro = lib.macro("BUF")

    with pytest.raises(KeyError):
        lib.macro("NOSUCH")
    with pytest.raises(KeyError):
        macro.pin("NOSUCH")


def test_error_context_missing_size(tmp_path):
    path = write_lef(
        tmp_path,
        "bad.lef",
        """\
VERSION 5.8 ;
MACRO BUF
  ORIGIN 0 0 ;
END BUF
END LIBRARY
""",
    )
    with pytest.raises(ValueError) as excinfo:
        parse_lef(path)
    msg = str(excinfo.value)
    assert str(path) in msg
    assert "MACRO BUF" in msg
    assert "missing SIZE" in msg


def test_error_context_rect_before_layer(tmp_path):
    path = write_lef(
        tmp_path,
        "bad2.lef",
        """\
VERSION 5.8 ;
MACRO BUF
  SIZE 1 BY 1 ;
  PIN A
    DIRECTION INPUT ;
    USE SIGNAL ;
    PORT
      RECT 0 0 1 1 ;
    END
  END A
END BUF
END LIBRARY
""",
    )
    with pytest.raises(ValueError) as excinfo:
        parse_lef(path)
    msg = str(excinfo.value)
    assert str(path) in msg
    assert "MACRO BUF PIN A: RECT appears before LAYER" in msg


def test_error_context_malformed_rect_coordinate(tmp_path):
    path = write_lef(
        tmp_path,
        "bad3.lef",
        """\
VERSION 5.8 ;
MACRO BUF
  SIZE 1 BY 1 ;
  PIN A
    DIRECTION INPUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0.1 x 1.0 2.0 ;
    END
  END A
END BUF
END LIBRARY
""",
    )
    with pytest.raises(ValueError) as excinfo:
        parse_lef(path)
    msg = str(excinfo.value)
    assert str(path) in msg
    assert "MACRO BUF PIN A: malformed RECT coordinate 'x'" in msg


# The LEF lexer only merges a leading sign into a word when a digit or "." follows,
# because DEF needs a bare "-" as its record marker. So "-inf" arrives as the token "-"
# and that is what the error echoes; it is still rejected with full context.
@pytest.mark.parametrize(("literal", "reported"), [("inf", "inf"), ("nan", "nan"), ("-inf", "-")])
def test_error_context_non_finite_rect_coordinate(tmp_path, literal, reported):
    path = write_lef(
        tmp_path,
        "nonfinite.lef",
        f"""\
VERSION 5.8 ;
MACRO BUF
  SIZE 1 BY 1 ;
  PIN A
    DIRECTION INPUT ;
    USE SIGNAL ;
    PORT
      LAYER li1 ;
      RECT 0.1 {literal} 1.0 2.0 ;
    END
  END A
END BUF
END LIBRARY
""",
    )
    with pytest.raises(ValueError) as excinfo:
        parse_lef(path)
    msg = str(excinfo.value)
    assert str(path) in msg
    assert f"MACRO BUF PIN A: malformed RECT coordinate '{reported}'" in msg


def test_empty_path_list_raises_valueerror():
    with pytest.raises(ValueError):
        parse_lef([])


def test_missing_file_raises_oserror(tmp_path):
    with pytest.raises(FileNotFoundError) as excinfo:
        parse_lef(tmp_path / "does_not_exist.lef")
    assert "does_not_exist.lef" in str(excinfo.value)


def test_property_definitions_are_not_macros(tmp_path):
    path = tmp_path / "props.lef"
    path.write_text(
        "VERSION 5.8 ;\n"
        "PROPERTYDEFINITIONS\n"
        "  MACRO CatenaDesignType STRING ;\n"
        "  PIN Foo STRING ;\n"
        "END PROPERTYDEFINITIONS\n"
        "MACRO BUF\n"
        "  SIZE 1 BY 1 ;\n"
        "END BUF\n",
        encoding="utf-8",
    )

    library = parse_lef(path)

    assert library.macro_names() == ["BUF"]
