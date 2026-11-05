"""Parser coverage for DEF constructs that only appear in real designs.

The hand-written fixtures elsewhere in this suite are minimal by design.
This module drives the parser with a generated design that reproduces the
reference file's shape, covering constructs the minimal fixtures omit:
``+ SOURCE DIST`` on components, PG nets that name most instances twice
across wrapped continuation lines, ``SHAPE FOLLOWPIN``/``STRIPE``
routing, hierarchical instance names with bus bits, and ROW/TRACKS/
GCELLGRID/VIAS preambles.
"""

from __future__ import annotations

import re
from collections import Counter

import pytest

import def_tools as lt
from def_tools.sdc import clock_roots_from_sdc


@pytest.fixture(scope="module")
def parsed(small_design):
    out, manifest = small_design
    boxes = list(lt.instance_bboxes([out / manifest["lef"]], out / manifest["def"]))
    return out, manifest, boxes


def test_all_instances_are_parsed(parsed):
    _, manifest, boxes = parsed
    assert len(boxes) == manifest["counts"]["components"]


def test_per_macro_counts_match_the_manifest(parsed):
    _, manifest, boxes = parsed
    assert dict(Counter(b.macro_name for b in boxes)) == manifest["per_macro"]


def test_spot_check_bboxes_match_exactly(parsed):
    """Ground truth comes from the generator, not from re-deriving here."""
    _, manifest, boxes = parsed
    by_name = {b.instance: b for b in boxes}
    for check in manifest["spot_checks"]:
        box = by_name[check["name"]]
        assert [box.llx, box.lly, box.urx, box.ury] == check["bbox"]
        assert box.macro_name == check["macro"]
        assert box.orient == check["orient"]
        assert box.status == check["status"]


def test_source_dist_components_are_parsed(parsed):
    """92% of the reference design's component records carry '+ SOURCE DIST'.

    The parser reaches placement by skipping unrecognised '+' keywords; if
    that ever tightens up, nearly every real DEF stops parsing.
    """
    out, manifest, boxes = parsed
    text = (out / manifest["def"]).read_text(encoding="utf-8")
    source_dist = {
        m.group(1) for m in re.finditer(r"^    - (\S+) \S+ \+ SOURCE DIST ", text, re.MULTILINE)
    }

    assert source_dist, "generated design should contain '+ SOURCE DIST' records"
    assert source_dist <= {b.instance for b in boxes}


def test_fixed_and_placed_statuses_both_appear(parsed):
    _, _, boxes = parsed
    statuses = {b.status for b in boxes}
    assert {"FIXED", "PLACED"} <= statuses


def test_hierarchical_and_bus_bit_instance_names_survive(parsed):
    """Logic instances are named 'core/u_dec_3/reg[7]/U42'."""
    _, _, boxes = parsed
    hierarchical = [b for b in boxes if "/" in b.instance and "[" in b.instance]

    assert hierarchical
    for box in hierarchical[:50]:
        assert re.fullmatch(r"core/u_dec_\d+/reg\[\d+\]/U\d+", box.instance)


def test_followpin_rails_are_read_from_specialnets(parsed):
    out, manifest, _ = parsed
    shapes = list(lt.net_shapes(out / manifest["def"], "VPWR", "met1"))

    assert len(shapes) == manifest["layout"]["rows"]
    for shape in shapes:
        assert shape.net == "VPWR"
        assert shape.llx < shape.urx, "FOLLOWPIN rails run horizontally"


def test_stripe_straps_are_read_from_specialnets(parsed):
    out, manifest, _ = parsed
    shapes = list(lt.net_shapes(out / manifest["def"], "VGND", "met4"))

    assert shapes
    for shape in shapes:
        assert shape.lly < shape.ury, "STRIPE straps run vertically"


def test_both_pg_nets_are_distinguished(parsed):
    out, manifest, _ = parsed
    vpwr = list(lt.net_shapes(out / manifest["def"], "VPWR", "met1"))
    vgnd = list(lt.net_shapes(out / manifest["def"], "VGND", "met1"))

    assert {s.net for s in vpwr} == {"VPWR"}
    assert {s.net for s in vgnd} == {"VGND"}
    assert {(s.llx, s.lly) for s in vpwr} != {(s.llx, s.lly) for s in vgnd}


def test_gzipped_and_plain_designs_parse_identically(small_design, gzipped_design):
    plain_dir, plain = small_design
    gz_dir, gzed = gzipped_design

    a = [
        b.as_tuple()
        for b in lt.instance_bboxes([plain_dir / plain["lef"]], plain_dir / plain["def"])
    ]
    b = [b.as_tuple() for b in lt.instance_bboxes([gz_dir / gzed["lef"]], gz_dir / gzed["def"])]

    assert a == b


def test_clock_roots_are_read_from_the_generated_sdc(parsed):
    out, manifest, _ = parsed
    roots = clock_roots_from_sdc(out / manifest["sdc"])

    assert (("core_clk", "port", manifest["clock_net"])) in [(r[0], r[1], r[2]) for r in roots]
