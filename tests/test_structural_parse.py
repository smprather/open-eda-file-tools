"""Tests for the structural (``skip_tables``) Liberty parse."""

from __future__ import annotations

from pathlib import Path

from liberty_tools import parse_file

LIB = """\
library (structural) {
  cell (BUF) {
    pin (A) {
      direction : input ;
    }
    pin (X) {
      direction : output ;
      function : "A" ;
      timing () {
        related_pin : "A" ;
        timing_type : combinational ;
        cell_rise (t) {
          /* } is a comment brace, not a group close */
          values ( "1.0" ) ; // } trailing comment
        }
      }
    }
  }
}
"""


def test_skip_tables_keeps_structure_and_drops_tables(tmp_path: Path) -> None:
    path = tmp_path / "structural.lib"
    path.write_text(LIB, encoding="utf-8")

    full = parse_file(path)
    structural = parse_file(path, skip_tables=True)

    assert full.cells() == structural.cells() == ["BUF"]
    for name in ("A", "X"):
        assert full.cell("BUF").pin(name).direction == structural.cell("BUF").pin(
            name
        ).direction

    full_arc = full.cell("BUF").pin("X").timing_arcs()[0]
    skipped_arc = structural.cell("BUF").pin("X").timing_arcs()[0]
    assert full_arc.related_pin == skipped_arc.related_pin == "A"
    assert full_arc.timing_type == skipped_arc.timing_type == "combinational"
    assert full_arc.tables() == ["cell_rise"]
    assert skipped_arc.tables() == []


def test_skip_tables_handles_comments_and_braces(tmp_path: Path) -> None:
    path = tmp_path / "structural_comments.lib"
    path.write_text(LIB, encoding="utf-8")

    document = parse_file(path, skip_tables=True)

    # If the comment braces ended the skip early, the pin or cell would be lost.
    assert document.cells() == ["BUF"]
    assert document.cell("BUF").pins() == ["A", "X"]


def test_structural_power_keeps_groups_and_omits_samples(tmp_path: Path) -> None:
    source = """library (t) {
      cell (a) {
        dynamic_current () {
          when : A';
          switching_group () {
            input_switching_condition (A);
            pg_current (VDD) {
              vector (power_t) { index_1 ("0, 1"); values ("2, 3"); }
            }
            pg_current (VSS) {}
          }
        }
      }
      cell (b) { area : 1; }
    }
    """
    path = tmp_path / "power.lib"
    path.write_text(source)
    full = parse_file(path).cell("a").dynamic_currents()[0]
    doc = parse_file(path, skip_tables=True)
    group = doc.cell("a").dynamic_currents()[0]
    assert full.when == group.when == "A'"
    assert full.switching_groups()[0].pg_currents()[0].vectors()[0].values == [2, 3]
    currents = group.switching_groups()[0].pg_currents()
    assert [current.pg_pin for current in currents] == ["VDD", "VSS"]
    assert [current.vectors() for current in currents] == [[], []]
    assert doc.cell("b").area == 1
    assert doc.to_string() == source
