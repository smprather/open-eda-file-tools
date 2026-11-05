"""Tests for the synthetic design generator itself.

The generator is test infrastructure, so it needs its own tests: if it
drifts, every test built on it drifts silently in the same direction.
"""

from __future__ import annotations

import gzip
import itertools
import json
import re
from collections import Counter

import pytest

from tests.gen.design_gen import (
    CELLS,
    SITE_H,
    SITE_W,
    DesignSpec,
    _iter_placements,
    generate,
)


def test_generation_is_deterministic(tmp_path):
    spec = DesignSpec(instances=1500, seed=3, gzip=False)
    a = generate(spec, tmp_path / "a")
    b = generate(spec, tmp_path / "b")

    assert a == b
    for name in ("design.def", "design.lef", "design.sdc"):
        assert (tmp_path / "a" / name).read_bytes() == (tmp_path / "b" / name).read_bytes()


def test_different_seeds_produce_different_designs(tmp_path):
    one = generate(DesignSpec(instances=1500, seed=1, gzip=False), tmp_path / "one")
    two = generate(DesignSpec(instances=1500, seed=2, gzip=False), tmp_path / "two")

    assert one["per_macro"] != two["per_macro"]
    assert one["counts"]["components"] == two["counts"]["components"] == 1500


def test_placement_stream_is_repeatable():
    """Sections are emitted by re-walking this stream, so it must not drift."""
    spec = DesignSpec(instances=800, seed=11)
    first = [(p.name, p.x, p.y, p.orient) for p in _iter_placements(spec)]
    second = [(p.name, p.x, p.y, p.orient) for p in _iter_placements(spec)]

    assert first == second
    assert len(first) == 800


def test_instance_count_is_exact(tmp_path):
    for n in (1, 2, 37, 500, 3000):
        manifest = generate(DesignSpec(instances=n, seed=0, gzip=False), tmp_path / f"n{n}")
        assert manifest["counts"]["components"] == n
        assert sum(manifest["per_macro"].values()) == n


def test_placements_are_on_site_grid():
    spec = DesignSpec(instances=5000, seed=4)
    for p in _iter_placements(spec):
        assert p.x % SITE_W == 0, f"{p.name} off the site grid"
        assert p.y % SITE_H == 0, f"{p.name} off the row pitch"


def test_orientation_alternates_by_row():
    """Rows flip N/FS so abutting cells share power rails, as in a real design."""
    for p in _iter_placements(DesignSpec(instances=5000, seed=4)):
        row = (p.y - 4 * SITE_H) // SITE_H
        assert p.orient == ("N" if row % 2 == 0 else "FS")


def test_cells_do_not_overlap():
    spec = DesignSpec(instances=5000, seed=4)
    by_row: dict[int, list[tuple[int, int, str]]] = {}
    for p in _iter_placements(spec):
        by_row.setdefault(p.y, []).append((p.x, p.x + p.cell.width, p.name))
    for row, spans in by_row.items():
        spans.sort()
        for (_, end, left), (start, _, right) in itertools.pairwise(spans):
            assert end <= start, f"{left} overlaps {right} in row at y={row}"


def test_cell_mix_matches_the_reference_design():
    """Fill/decap/tap dominance is what makes a real post-fill DEF enormous.

    Reference design: 92.85% non-logic, 7.10% logic.  Benchmarks built on
    a design with the wrong mix measure the wrong thing, so the bounds are
    tight around the measured values.
    """
    spec = DesignSpec(instances=50_000, seed=5)
    kinds = Counter(p.cell.kind for p in _iter_placements(spec))

    non_logic = (kinds["fill"] + kinds["tap"] + kinds["diode"]) / spec.instances
    logic = kinds["logic"] / spec.instances

    assert non_logic == pytest.approx(0.9285, abs=0.02)
    assert logic == pytest.approx(0.0710, abs=0.02)


def test_manifest_records_what_was_written(tmp_path):
    manifest = generate(DesignSpec(instances=2000, seed=6, gzip=False), tmp_path)
    text = (tmp_path / "design.def").read_text(encoding="utf-8")

    assert text.count("\n    - ") >= manifest["counts"]["components"]
    assert re.search(rf"COMPONENTS {manifest['counts']['components']} ;", text)
    assert re.search(rf"NETS {manifest['counts']['nets']} ;", text)
    assert re.search(rf"PINS {manifest['counts']['pins']} ;", text)
    assert json.loads((tmp_path / "manifest.json").read_text()) == manifest


def test_section_counts_match_declared_headers(tmp_path):
    """A DEF whose section headers lie is not a useful test input."""
    generate(DesignSpec(instances=2500, seed=8, gzip=False), tmp_path)
    lines = (tmp_path / "design.def").read_text(encoding="utf-8").splitlines()

    section = None
    declared: dict[str, int] = {}
    actual: Counter[str] = Counter()
    for line in lines:
        header = re.match(r"^(COMPONENTS|NETS|SPECIALNETS|PINS|VIAS) (\d+) ;$", line)
        if header:
            section, declared[header.group(1)] = header.group(1), int(header.group(2))
            continue
        if line.startswith("END ") and section and line == f"END {section}":
            section = None
            continue
        if section and line.startswith("    - "):
            actual[section] += 1

    assert declared
    assert dict(actual) == declared


def test_gzip_and_plain_designs_have_identical_content(tmp_path):
    plain = generate(DesignSpec(instances=1200, seed=9, gzip=False), tmp_path / "plain")
    gzed = generate(DesignSpec(instances=1200, seed=9, gzip=True), tmp_path / "gz")

    a = (tmp_path / "plain" / "design.def").read_text(encoding="utf-8")
    with gzip.open(tmp_path / "gz" / "design.def.gz", "rt", encoding="utf-8") as fh:
        b = fh.read()
    assert a == b
    assert plain["counts"] == gzed["counts"]


def test_every_macro_placed_is_defined_in_the_lef(tmp_path):
    manifest = generate(DesignSpec(instances=8000, seed=10, gzip=False), tmp_path)
    lef = (tmp_path / "design.lef").read_text(encoding="utf-8")
    defined = set(re.findall(r"^MACRO (\S+)$", lef, re.MULTILINE))

    assert set(manifest["per_macro"]) <= defined
    assert defined == {c.name for c in CELLS}


@pytest.mark.parametrize("instances", [1, 5, 100])
def test_tiny_designs_are_still_valid(tmp_path, instances):
    """Row-tail filling and net closing must not misbehave at the edges."""
    manifest = generate(
        DesignSpec(instances=instances, seed=0, gzip=False), tmp_path / str(instances)
    )
    text = (tmp_path / str(instances) / "design.def").read_text(encoding="utf-8")

    assert manifest["counts"]["components"] == instances
    assert text.startswith("VERSION 5.8 ;")
    assert text.rstrip().endswith("END DESIGN")
