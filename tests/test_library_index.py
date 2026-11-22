"""LibraryIndex must resolve the library items that are declared after the
first cell, exactly like a full ``parse_file`` does."""

from pathlib import Path

import pytest

import liberty_tools as lt

# The units and the lookup-table template are declared *after* the cell that
# uses them — legal Liberty, and the full parser resolves them.
LATE_ITEMS = """
library (late_lib) {
  cell (u1) {
    area : 1.0;
    pin (a) {
      direction : input;
      capacitance : 0.5;
    }
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        cell_rise (t) {
          index_1 ("1, 2");
          index_2 ("3, 4");
          values ("0.1, 0.2", "0.3, 0.4");
        }
      }
    }
  }
  capacitive_load_unit (1, pf);
  lu_table_template (t) {
    variable_1 : input_net_transition;
    variable_2 : total_output_net_capacitance;
  }
}
"""


def _write(tmp_path: Path) -> Path:
    path = tmp_path / "late.lib"
    path.write_text(LATE_ITEMS, encoding="utf-8")
    return path


def test_index_resolves_units_and_templates_declared_after_the_cell(tmp_path: Path):
    path = _write(tmp_path)

    cell = lt.LibraryIndex.open(path).cell("u1")

    # The library declares picofarads after the cell that uses them.
    assert cell.pin("a").capacitance_farads == pytest.approx(0.5e-12)
    # ... and the lookup-table template after the cell that uses it.
    table = cell.pin("z").timing_arcs()[0].table("cell_rise")
    assert table.axis_roles() == [
        "input_net_transition",
        "total_output_net_capacitance",
    ]


def test_index_agrees_with_a_full_parse_on_late_items(tmp_path: Path):
    path = _write(tmp_path)

    index = lt.LibraryIndex.open(path)
    doc = lt.parse_file(path)

    assert index.capacitive_load_unit == doc.capacitive_load_unit
    assert index.templates() == doc.templates()
    assert (
        index.cell("u1").pin("a").capacitance_farads
        == doc.cell("u1").pin("a").capacitance_farads
    )
    assert index.cell("u1").pin("z").timing_arcs()[0].table(
        "cell_rise"
    ).axis_roles() == doc.cell("u1").pin("z").timing_arcs()[0].table(
        "cell_rise"
    ).axis_roles()


def test_index_templates_keep_declaration_order(tmp_path: Path):
    names = ["zeta", "mid", "alpha", "tpl_9", "beta", "omega", "gamma"]
    body = "".join(
        f"  lu_table_template ({n}) {{\n"
        "    variable_1 : input_net_transition;\n"
        "  }\n"
        for n in names
    )
    path = tmp_path / "order.lib"
    path.write_text(
        f"library (order_lib) {{\n{body}  cell (u1) {{ area : 1.0; }}\n}}\n",
        encoding="utf-8",
    )

    index = lt.LibraryIndex.open(path)
    doc = lt.parse_file(path)

    assert list(index.templates()) == names
    assert list(index.templates()) == list(doc.templates())


@pytest.mark.parametrize("compressed", [False, True])
def test_postfix_apostrophe_does_not_hide_following_cell(tmp_path, compressed):
    import gzip

    keep = "cell (keep) { area : 1; }"
    source = f"library (t) {{ cell (omit) {{ pin (Z) {{ function : A'; }} }} {keep} }}"
    path = tmp_path / ("postfix.lib.gz" if compressed else "postfix.lib")
    data = source.encode()
    path.write_bytes(gzip.compress(data) if compressed else data)

    assert lt.parse_file(path).cells() == ["omit", "keep"]
    assert lt.LibraryIndex.open(path).cell_source("keep") == keep
    doc = lt.parse_file(path, cells=["keep"])
    assert doc.cell("keep").area == 1
    assert doc.to_string() == source


@pytest.mark.parametrize("body", [
    'test : "quoted } /*";',
    "test : A\\}; // }",
    "test : A'/* } */;",
    "test : A\\\r\n';",
    "extra () { empty () {} test : 'literal }'; }",
])
def test_selected_and_indexed_cells_agree_after_skipped_groups(tmp_path, body):
    path = tmp_path / "escape.lib"
    want = "cell (keep) { area : 4; }"
    source = f"library (t) {{ cell (omit) {{ {body}\n }} {want} }}"
    path.write_bytes(source.encode())
    assert lt.parse_file(path).cells() == ["omit", "keep"]
    assert lt.LibraryIndex.open(path).cell_source("keep") == want
    selected = lt.parse_file(path, cells=["keep"])
    assert selected.cell("keep").area == 4
    assert selected.to_string() == source


@pytest.mark.parametrize(("body", "error"), [
    ('   attr : "open', "unterminated quoted string"),
    ("   /* open", "unterminated block comment"),
    ("   attr : x;", "unexpected end of file while skipping group"),
])
def test_fast_cell_skip_reports_lexical_errors(tmp_path, body, error):
    path = tmp_path / "malformed.lib"
    path.write_text(f"library (t) {{\n  cell (omit) {{\n{body}")
    with pytest.raises(ValueError, match=error):
        lt.parse_file(path, cells=["keep"])


@pytest.mark.parametrize("newline", ["\n", "\r\n"])
def test_index_normalizes_continuations_in_unquoted_cell_names(tmp_path, newline):
    path = tmp_path / "continued-name.lib"
    path.write_bytes(f"library (t) {{ cell (in\\{newline}verter) {{ area : 1; }} }}".encode())
    doc = lt.parse_file(path)
    index = lt.LibraryIndex.open(path)
    assert index.cell_names() == doc.cells() == ["inverter"]
    assert index.cell("inverter").area == 1
