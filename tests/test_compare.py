"""Cross-library comparison (issue #4)."""

from pathlib import Path

import pytest

import liberty_tools as lt

# The two libraries below store the same physical axes in opposite order:
# `ours` puts the load on index_1, `theirs` puts the transition there.
OURS = """
library (ours) {
  time_unit : "1ns";
  capacitive_load_unit (1, pf);
  lu_table_template (delay_template) {
    variable_1 : total_output_net_capacitance;
    variable_2 : input_net_transition;
  }
  cell (u1) {
    area : 1.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
}
"""

THEIRS = """
library (theirs) {
  time_unit : "1ns";
  capacitive_load_unit (1, pf);
  lu_table_template (delay_template) {
    variable_1 : input_net_transition;
    variable_2 : total_output_net_capacitance;
  }
  cell (u1) {
    area : 1.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        cell_rise (delay_template) {
          index_1 ("1, 2");
          index_2 ("0.1, 0.2");
          values ("10, 35, 20, 40");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
}
"""


# The library below is the same in every respect but the unit it declares:
# `ns_library` and `ps_library` store the same numbers in different units.
NS_LIBRARY = """
library (unit_library) {
  time_unit : "1ns";
  capacitive_load_unit (1, pf);
  lu_table_template (delay_template) {
    variable_1 : total_output_net_capacitance;
    variable_2 : input_net_transition;
  }
  cell (u1) {
    area : 1.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
}
"""

PS_LIBRARY = NS_LIBRARY.replace('time_unit : "1ns";', 'time_unit : "1ps";')

# The same table, stored with the load axis in pf and in ff. The numbers of
# the load axis are scaled accordingly, so both libraries describe the same
# physical points.
PF_LIBRARY = NS_LIBRARY

FF_LIBRARY = NS_LIBRARY.replace(
    "capacitive_load_unit (1, pf);",
    "capacitive_load_unit (1, ff);",
).replace(
    'index_1 ("0.1, 0.2");',
    'index_1 ("100, 200");',
)

# A cell whose output pin has two timing groups on the same related pin
# that differ only in their timing sense: they are two different arcs.
SENSE_LIBRARY = """
library (sense_library) {
  time_unit : "1ns";
  capacitive_load_unit (1, pf);
  lu_table_template (delay_template) {
    variable_1 : total_output_net_capacitance;
    variable_2 : input_net_transition;
  }
  cell (u1) {
    area : 1.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        timing_sense : positive_unate;
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
      }
      timing () {
        related_pin : "a";
        timing_sense : negative_unate;
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("11, 21, 31, 41");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
}
"""

# The same library, but the two timing groups are indistinguishable: they
# have the same related pin, the same timing sense and no condition.
DUPLICATE_LIBRARY = """
library (duplicate_library) {
  time_unit : "1ns";
  capacitive_load_unit (1, pf);
  lu_table_template (delay_template) {
    variable_1 : total_output_net_capacitance;
    variable_2 : input_net_transition;
  }
  cell (u1) {
    area : 1.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        timing_sense : positive_unate;
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
      }
      timing () {
        related_pin : "a";
        timing_sense : positive_unate;
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("11, 21, 31, 41");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
}
"""

# The same library, but one of its timing groups declares the default timing
# type explicitly instead of leaving it out.
TYPED_LIBRARY = OURS.replace(
    """      timing () {
        related_pin : "a";""",
    """      timing () {
        related_pin : "a";
        timing_type : combinational;""",
)

# The same library, but one of its timing groups declares the default timing
# sense explicitly instead of leaving it out.
NON_UNATE_LIBRARY = OURS.replace(
    """      timing () {
        related_pin : "a";""",
    """      timing () {
        related_pin : "a";
        timing_sense : non_unate;""",
)

# A malformed library with two `cell (u1)` groups: the cells of a document
# are listed as they appear, so the name of this cell is reported twice.
DUPLICATE_CELL_LIBRARY = """
library (duplicate_cell_library) {
  time_unit : "1ns";
  capacitive_load_unit (1, pf);
  lu_table_template (delay_template) {
    variable_1 : total_output_net_capacitance;
    variable_2 : input_net_transition;
  }
  cell (u1) {
    area : 1.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
  cell (u1) {
    area : 2.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("11, 21, 31, 41");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
}
"""


# Two libraries whose tables name their axes differently: the first names
# the load and the input transition, the second names the transition of the
# related pin and of the constrained pin. No axis of one library has a
# counterpart in the other, so the two grids cannot be aligned by role.
ROLE_LIBRARY_A = """
library (role_a) {
  time_unit : "1ns";
  capacitive_load_unit (1, pf);
  lu_table_template (delay_template) {
    variable_1 : total_output_net_capacitance;
    variable_2 : input_net_transition;
  }
  cell (u1) {
    area : 1.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
}
"""

ROLE_LIBRARY_B = """
library (role_b) {
  time_unit : "1ns";
  capacitive_load_unit (1, pf);
  lu_table_template (delay_template) {
    variable_1 : related_pin_transition;
    variable_2 : constrained_pin_transition;
  }
  cell (u1) {
    area : 1.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
}
"""


# A table whose grid has four points but whose values hold only two: the
# table is malformed, and none of its numbers can be trusted.
SHORT_VALUES_LIBRARY = OURS.replace(
    'values ("10, 20, 30, 40");',
    'values ("10, 30");',
)

# The same table, with one of its values not a number at all.
NAN_VALUES_LIBRARY = OURS.replace(
    'values ("10, 20, 30, 40");',
    'values ("10, 20, nan, 40");',
)

# The same library, but it declares a current unit and holds a CCS table
# whose values are currents. The current unit of the two libraries differs
# by a factor of a thousand, so the currents they hold are a thousand
# apart even when the numbers are the same.
CURRENT_LIBRARY = """
library (current_library) {
  time_unit : "1ns";
  voltage_unit : "1V";
  current_unit : "1mA";
  capacitive_load_unit (1, pf);
  lu_table_template (delay_template) {
    variable_1 : total_output_net_capacitance;
    variable_2 : input_net_transition;
  }
  lu_table_template (ccst_template) {
    variable_1 : input_net_transition;
    variable_2 : output_voltage;
  }
  cell (u1) {
    area : 1.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
        output_current_rise (ccst_template) {
          index_1 ("1, 2");
          index_2 ("1.0, 2.0");
          values ("10, 20, 30, 40");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
}
"""

AMPERE_LIBRARY = CURRENT_LIBRARY.replace(
    'current_unit : "1mA";',
    'current_unit : "1A";',
)

# The same table, with a load of 1 pF and 2 pF and an input
# transition of 0.1 ns and 0.2 ns: the grid shorthands must find the axes
# whatever their numbers are.
GRID_LIBRARY = """
library (grid_library) {
  time_unit : "1ns";
  capacitive_load_unit (1, pf);
  lu_table_template (delay_template) {
    variable_1 : total_output_net_capacitance;
    variable_2 : input_net_transition;
  }
  cell (u1) {
    area : 1.0;
    pin (z) {
      direction : output;
      timing () {
        related_pin : "a";
        cell_rise (delay_template) {
          index_1 ("1, 2");
          index_2 ("0.1, 0.2");
          values ("10, 20, 30, 40");
        }
      }
    }
    pin (a) {
      direction : input;
    }
  }
}
"""

# The same table with different numbers.
GRID_LIBRARY_B = GRID_LIBRARY.replace(
    'values ("10, 20, 30, 40");',
    'values ("12, 22, 32, 42");',
)


def _write(tmp_path: Path, name: str, text: str) -> Path:
    path = tmp_path / name
    path.write_text(text, encoding="utf-8")
    return path


@pytest.fixture()
def docs(tmp_path: Path) -> tuple[lt.LibertyDocument, lt.LibertyDocument]:
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", THEIRS))
    return ours, theirs


def test_matched_arcs_are_compared_point_by_point(docs):
    ours, theirs = docs

    diff = lt.compare(ours, theirs, cell="u1")

    assert len(diff.entries) == 4
    assert all(entry.matched for entry in diff.entries)
    assert diff.only_in_a == []
    assert diff.only_in_b == []


def test_grids_are_aligned_by_axis_role_not_by_position(docs):
    ours, theirs = docs

    diff = lt.compare(ours, theirs, cell="u1")
    by_point = {(entry.index_1, entry.index_2): entry for entry in diff.entries}

    # Load 0.1 / transition 1 is the same point in both libraries even though
    # it sits at a different grid position in each of them.
    entry = by_point[(0.1, 1.0)]
    assert entry.value_a == 10.0
    assert entry.value_b == 10.0
    assert entry.delta == 0.0

    entry = by_point[(0.2, 2.0)]
    assert entry.value_a == 40.0
    assert entry.value_b == 40.0

    # The values that really differ are reported.
    entry = by_point[(0.2, 1.0)]
    assert entry.value_a == 30.0
    assert entry.value_b == 35.0
    assert entry.delta == -5.0
    assert entry.relative_delta == pytest.approx(-1 / 7)


def test_entries_carry_the_arc_identity(docs):
    ours, theirs = docs

    entry = lt.compare(ours, theirs, cell="u1").entries[0]

    assert entry.cell == "u1"
    assert entry.pin == "z"
    assert entry.related_pin == "a"
    assert entry.table == "cell_rise"
    assert "u1/z" in entry.arc
    assert entry.arc_key.pin == "z"


def test_arcs_missing_from_one_document_are_reported(tmp_path: Path):
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    fewer = OURS.replace(
        """      timing () {
        related_pin : "a";
        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
      }
""",
        "",
    )
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", fewer))

    diff = lt.compare(ours, theirs, cell="u1")

    assert [str(key) for key in diff.only_in_b] == []
    assert len(diff.only_in_a) == 1
    assert diff.only_in_a[0].pin == "z"
    assert diff.only_in_a[0].related_pin == "a"


def test_points_missing_from_one_grid_are_reported(tmp_path: Path):
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    # The second library keeps only the first point of its load axis, so its
    # table holds two values for the two points of the remaining grid.
    smaller = THEIRS.replace('index_2 ("0.1, 0.2");', 'index_2 ("0.1");').replace(
        'values ("10, 35, 20, 40");', 'values ("10, 30");'
    )
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", smaller))

    diff = lt.compare(ours, theirs, cell="u1")

    unmatched = [entry for entry in diff.entries if not entry.matched]
    assert len(unmatched) == 2
    # The load axis is the one that is missing points.
    assert {entry.index_1 for entry in unmatched} == {0.2}
    assert all(entry.value_b is None for entry in unmatched)
    assert all(entry.delta is None for entry in unmatched)


def test_interpolation_compares_on_the_first_documents_grid(docs):
    ours, theirs = docs

    diff = lt.compare(ours, theirs, cell="u1", interpolate=True)

    # Every point of our grid gets a value from both documents.
    assert len(diff.entries) == 4
    assert all(entry.matched for entry in diff.entries)
    by_point = {(entry.index_1, entry.index_2): entry for entry in diff.entries}
    assert by_point[(0.1, 1.0)].value_b == 10.0
    assert by_point[(0.2, 1.0)].value_b == 35.0


def test_interpolation_on_a_requested_grid(docs):
    ours, theirs = docs

    diff = lt.compare(
        ours,
        theirs,
        cell="u1",
        interpolate=True,
        grid={"total_output_net_capacitance": [0.1, 0.2], "input_net_transition": [1.5]},
    )

    assert len(diff.entries) == 2
    # Halfway between transition 1 and 2 on each load.
    by_load = {entry.index_1: entry for entry in diff.entries}
    assert by_load[0.1].value_a == pytest.approx(15.0)
    assert by_load[0.1].value_b == pytest.approx(15.0)
    assert by_load[0.2].value_a == pytest.approx(35.0)
    assert by_load[0.2].value_b == pytest.approx(37.5)


def test_an_unknown_grid_axis_is_rejected(docs):
    ours, theirs = docs

    with pytest.raises(ValueError):
        lt.compare(
            ours,
            theirs,
            cell="u1",
            interpolate=True,
            grid={"nonsense": [1.0]},
        )


def test_arcs_are_matched_by_logically_equal_when_conditions(tmp_path: Path):
    a = """
library (a) {
  cell (u1) {
    pin (z) {
      direction : output;
      timing () { related_pin : "a"; when : "A & B"; }
    }
  }
}
"""
    b = """
library (b) {
  cell (u1) {
    pin (z) {
      direction : output;
      timing () { related_pin : "a"; when : "B & A"; }
    }
  }
}
"""
    ours = lt.parse_file(_write(tmp_path, "a.lib", a))
    theirs = lt.parse_file(_write(tmp_path, "b.lib", b))

    diff = lt.compare(ours, theirs, cell="u1")

    assert diff.only_in_a == []
    assert diff.only_in_b == []


def test_comparing_a_library_with_itself_finds_no_differences(tmp_path: Path):
    path = _write(tmp_path, "ours.lib", OURS)
    ours = lt.parse_file(path)
    theirs = lt.parse_file(path)

    diff = lt.compare(ours, theirs, cell="u1")

    assert diff.entries
    assert all(entry.matched for entry in diff.entries)
    assert diff.mismatched == []
    assert diff.only_in_a == []
    assert diff.only_in_b == []
    assert diff.max_abs_delta() == 0.0


def test_the_table_filter_selects_one_table(docs):
    ours, theirs = docs

    diff = lt.compare(ours, theirs, cell="u1", table="cell_rise")
    assert {entry.table for entry in diff.entries} == {"cell_rise"}

    # A table that neither document has yields nothing at all.
    diff = lt.compare(ours, theirs, cell="u1", table="cell_fall")
    assert diff.entries == []
    assert diff.only_in_b_tables == []


def test_tables_with_a_different_number_of_axes_are_reported(tmp_path: Path):
    flat = OURS.replace(
        """        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }""",
        """        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          values ("10, 20");
        }""",
    )
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", flat))

    diff = lt.compare(ours, theirs, cell="u1")

    # The grids cannot be intersected, so the mismatch is reported instead of
    # silently comparing unrelated points.
    assert diff.shape_mismatches
    assert diff.shape_mismatches[0][1] == "cell_rise"
    assert diff.shape_mismatches[0][2:] == (2, 1)
    assert diff.entries == []


def test_interpolation_does_not_invent_values_for_a_different_shape(tmp_path: Path):
    flat = OURS.replace(
        """        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }""",
        """        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          values ("10, 20");
        }""",
    )
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", flat))

    diff = lt.compare(ours, theirs, cell="u1", interpolate=True)

    assert diff.entries == []
    assert diff.shape_mismatches


def test_arcs_with_different_conditions_are_not_compared(tmp_path: Path):
    a = """
library (a) {
  cell (u1) {
    pin (z) {
      direction : output;
      timing () { related_pin : "a"; when : "A"; }
    }
  }
}
"""
    b = """
library (b) {
  cell (u1) {
    pin (z) {
      direction : output;
      timing () { related_pin : "a"; when : "A & B"; }
    }
  }
}
"""
    ours = lt.parse_file(_write(tmp_path, "a.lib", a))
    theirs = lt.parse_file(_write(tmp_path, "b.lib", b))

    diff = lt.compare(ours, theirs, cell="u1")

    # "A" and "A & B" describe different arcs, so there is nothing to
    # compare — the difference is reported instead of a meaningless delta.
    assert diff.entries == []
    assert len(diff.only_in_a) == 1
    assert len(diff.only_in_b) == 1


def test_axes_named_differently_are_still_aligned(tmp_path: Path):
    renamed = THEIRS.replace(
        "variable_1 : input_net_transition;",
        "variable_1 : input_transition;",
    ).replace(
        "variable_2 : total_output_net_capacitance;",
        "variable_2 : output_capacitance;",
    )
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", renamed))

    diff = lt.compare(ours, theirs, cell="u1")
    by_point = {(entry.index_1, entry.index_2): entry for entry in diff.entries}

    # The slew axis of one library is matched against the slew axis of the
    # other even though the two name it differently.
    entry = by_point[(0.1, 1.0)]
    assert entry.value_a == 10.0
    assert entry.value_b == 10.0
    assert entry.delta == 0.0


def test_summary_and_polars_output(docs):
    ours, theirs = docs

    diff = lt.compare(ours, theirs, cell="u1")
    summary = diff.summary()

    assert summary["entries"] == 4
    assert summary["matched"] == 4
    assert summary["mismatched"] == 1
    assert summary["max_abs_delta"] == 5.0

    frame = diff.to_polars()
    assert frame.height == 4
    assert {"cell", "pin", "table", "value_a", "value_b", "delta"} <= set(frame.columns)


def test_a_time_unit_mismatch_is_reported_instead_of_diffed(tmp_path: Path):
    ns = lt.parse_file(_write(tmp_path, "ns.lib", NS_LIBRARY))
    ps = lt.parse_file(_write(tmp_path, "ps.lib", PS_LIBRARY))

    diff = lt.compare(ns, ps, cell="u1")

    assert diff.unit_mismatches
    mismatch = diff.unit_mismatches[0]
    assert mismatch.dimension == "time"
    assert mismatch.unit_a == "1ns"
    assert mismatch.unit_b == "1ps"
    assert mismatch.cell == "u1"
    assert mismatch.pin == "z"
    assert mismatch.table == "cell_rise"

    # The table was not diffed at all: the two numbers mean different
    # physical quantities, so no delta is reported.
    assert diff.entries == []
    assert diff.max_abs_delta() is None

    assert diff.summary()["unit_mismatches"] == len(diff.unit_mismatches)


def test_a_capacitive_load_unit_mismatch_is_reported(tmp_path: Path):
    pf = lt.parse_file(_write(tmp_path, "pf.lib", PF_LIBRARY))
    ff = lt.parse_file(_write(tmp_path, "ff.lib", FF_LIBRARY))

    diff = lt.compare(pf, ff, cell="u1")

    assert diff.unit_mismatches
    assert {mismatch.dimension for mismatch in diff.unit_mismatches} == {
        "capacitive_load"
    }
    mismatch = diff.unit_mismatches[0]
    assert mismatch.unit_a == "1 pf"
    assert mismatch.unit_b == "1 ff"
    assert mismatch.table == "cell_rise"

    assert diff.entries == []
    assert diff.max_abs_delta() is None


def test_a_capacitive_load_multiplier_mismatch_is_reported(tmp_path: Path):
    one = lt.parse_file(_write(tmp_path, "one.lib", PF_LIBRARY))
    thousand = lt.parse_file(
        _write(
            tmp_path,
            "thousand.lib",
            PF_LIBRARY.replace(
                "capacitive_load_unit (1, pf);",
                "capacitive_load_unit (1000, pf);",
            ),
        )
    )

    diff = lt.compare(one, thousand, cell="u1")

    assert diff.unit_mismatches
    mismatch = diff.unit_mismatches[0]
    assert mismatch.dimension == "capacitive_load"
    # The whole declaration is reported, so the difference is visible.
    assert mismatch.unit_a == "1 pf"
    assert mismatch.unit_b == "1000 pf"
    assert mismatch.unit_a != mismatch.unit_b

    assert diff.entries == []
    assert diff.max_abs_delta() is None


def test_a_current_unit_mismatch_is_reported_instead_of_diffed(tmp_path: Path):
    milli = lt.parse_file(_write(tmp_path, "milli.lib", CURRENT_LIBRARY))
    ampere = lt.parse_file(_write(tmp_path, "ampere.lib", AMPERE_LIBRARY))

    diff = lt.compare(milli, ampere, cell="u1")

    # The two libraries declare the same currents in different units, so
    # the numbers of their current tables mean different quantities.
    assert diff.unit_mismatches
    mismatch = diff.unit_mismatches[0]
    assert mismatch.dimension == "current"
    assert mismatch.unit_a == "1mA"
    assert mismatch.unit_b == "1A"
    assert mismatch.cell == "u1"
    assert mismatch.pin == "z"
    assert mismatch.table == "output_current_rise"

    # The current table is not diffed: no delta is reported for it.
    assert [entry for entry in diff.entries if entry.table == "output_current_rise"] == []
    assert diff.max_abs_delta() == 0.0

    # The other table does not depend on the current unit, so it is still
    # compared.
    assert [entry for entry in diff.entries if entry.table == "cell_rise"]

    assert diff.summary()["unit_mismatches"] == len(diff.unit_mismatches)
    assert diff.has_differences is True


def test_a_point_without_a_value_on_either_side_is_not_a_difference(
    tmp_path: Path,
):
    document = lt.parse_file(_write(tmp_path, "nan.lib", NAN_VALUES_LIBRARY))

    diff = lt.compare(document, document, cell="u1")

    # The `nan` value is no value at all: the point has no data on either
    # side, so the two documents do not differ there.
    assert diff.unmatched == []
    assert diff.has_differences is False
    assert diff.summary()["unmatched"] == 0

    # A point that has a value on one side only is still unmatched.
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    diff = lt.compare(ours, document, cell="u1")
    assert len(diff.unmatched) == 1
    assert diff.unmatched[0].value_b is None
    assert diff.unmatched[0].delta is None
    assert diff.has_differences is True


def test_libraries_with_the_same_units_are_compared_numerically(docs):
    ours, theirs = docs

    diff = lt.compare(ours, theirs, cell="u1")

    assert diff.unit_mismatches == []
    assert diff.summary()["unit_mismatches"] == 0
    assert len(diff.entries) == 4
    assert diff.max_abs_delta() == 5.0


def test_arcs_that_differ_only_in_timing_sense_are_both_compared(tmp_path: Path):
    path = _write(tmp_path, "sense.lib", SENSE_LIBRARY)
    document = lt.parse_file(path)

    diff = lt.compare(document, document, cell="u1")

    # The two timing groups are two different arcs, so both are compared:
    # four points each.
    assert diff.only_in_a == []
    assert diff.only_in_b == []
    assert diff.duplicate_arcs == []
    assert len(diff.entries) == 8
    assert {entry.timing_sense for entry in diff.entries} == {
        "positive_unate",
        "negative_unate",
    }
    assert all(entry.matched for entry in diff.entries)
    assert diff.mismatched == []


def test_an_omitted_timing_type_matches_the_explicit_default(tmp_path: Path):
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    typed = lt.parse_file(_write(tmp_path, "typed.lib", TYPED_LIBRARY))

    diff = lt.compare(ours, typed, cell="u1")

    # `combinational` is the default timing type, so a library that leaves it
    # out describes the same arc as one that spells it out.
    assert diff.only_in_a == []
    assert diff.only_in_b == []
    assert len(diff.entries) == 4
    assert all(entry.matched for entry in diff.entries)
    assert diff.max_abs_delta() == 0.0


def test_an_omitted_timing_sense_matches_the_explicit_default(tmp_path: Path):
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    explicit = lt.parse_file(_write(tmp_path, "explicit.lib", NON_UNATE_LIBRARY))

    diff = lt.compare(ours, explicit, cell="u1")

    # `non_unate` is the default timing sense, so a library that leaves it
    # out describes the same arc as one that spells it out.
    assert diff.only_in_a == []
    assert diff.only_in_b == []
    assert len(diff.entries) == 4
    assert all(entry.matched for entry in diff.entries)
    assert diff.max_abs_delta() == 0.0
    assert {entry.timing_sense for entry in diff.entries} == {"non_unate"}


def test_arcs_that_still_collide_are_reported_instead_of_overwritten(
    tmp_path: Path,
):
    path = _write(tmp_path, "duplicate.lib", DUPLICATE_LIBRARY)
    document = lt.parse_file(path)

    diff = lt.compare(document, document, cell="u1")

    # The two timing groups have the same identity, so neither of them can be
    # told apart from the other: the collision is reported and no value is
    # compared for them.
    assert len(diff.duplicate_arcs) == 1
    assert diff.duplicate_arcs[0].timing_sense == "positive_unate"
    assert diff.duplicate_arcs[0].related_pin == "a"
    assert diff.entries == []
    assert diff.only_in_a == []
    assert diff.only_in_b == []


def test_tables_with_incompatible_axis_roles_are_reported_instead_of_compared(
    tmp_path: Path,
):
    a = lt.parse_file(_write(tmp_path, "a.lib", ROLE_LIBRARY_A))
    b = lt.parse_file(_write(tmp_path, "b.lib", ROLE_LIBRARY_B))

    diff = lt.compare(a, b, cell="u1")

    # The roles of the two libraries do not correspond: the first names a
    # load and an input transition, the second a related-pin and a
    # constrained-pin transition. Comparing the axes by position would
    # compare a load with a transition, so the table is reported instead.
    assert len(diff.axis_role_mismatches) == 1
    mismatch = diff.axis_role_mismatches[0]
    assert mismatch.cell == "u1"
    assert mismatch.pin == "z"
    assert mismatch.table == "cell_rise"
    assert mismatch.roles_a == (
        "total_output_net_capacitance",
        "input_net_transition",
    )
    assert mismatch.roles_b == (
        "related_pin_transition",
        "constrained_pin_transition",
    )

    # The table was not diffed at all: no delta is reported.
    assert diff.entries == []
    assert diff.max_abs_delta() is None

    assert diff.summary()["axis_role_mismatches"] == 1
    assert diff.summary()["has_differences"] is True


def test_a_table_that_exists_only_in_one_library_is_a_difference(tmp_path: Path):
    with_fall = OURS.replace(
        """        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
""",
        """        cell_rise (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("10, 20, 30, 40");
        }
        cell_fall (delay_template) {
          index_1 ("0.1, 0.2");
          index_2 ("1, 2");
          values ("11, 21, 31, 41");
        }
""",
    )
    ours = lt.parse_file(_write(tmp_path, "ours.lib", with_fall))
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", OURS))

    diff = lt.compare(ours, theirs, cell="u1")

    assert [name for _key, name in diff.only_in_a_tables] == ["cell_fall"]
    assert diff.only_in_b_tables == []

    # The table both libraries have is identical, so the count of mismatched
    # points reads as "no difference" ...
    assert diff.summary()["mismatched"] == 0
    # ... but a whole table is missing, which is a difference.
    assert diff.summary()["only_in_a_tables"] == 1
    assert diff.summary()["has_differences"] is True


def test_has_differences_is_false_for_two_equal_libraries(tmp_path: Path):
    path = _write(tmp_path, "ours.lib", OURS)
    ours = lt.parse_file(path)
    theirs = lt.parse_file(path)

    diff = lt.compare(ours, theirs, cell="u1")

    assert diff.entries
    assert diff.mismatched == []
    assert diff.has_differences is False
    assert diff.summary()["has_differences"] is False


def test_a_cell_named_twice_is_not_an_identity_collision(docs):
    ours, theirs = docs

    diff = lt.compare(ours, theirs, cells=["u1"], cell="u1")

    # Naming the same cell twice reads its arcs twice, which is not a
    # collision: the comparison is the one of a single naming.
    assert diff.duplicate_arcs == []
    assert len(diff.entries) == 4
    assert diff.entries == lt.compare(ours, theirs, cells=["u1"]).entries


def test_a_library_with_two_cell_groups_of_the_same_name_is_still_compared(
    tmp_path: Path,
):
    path = _write(tmp_path, "duplicate_cell.lib", DUPLICATE_CELL_LIBRARY)
    document = lt.parse_file(path)

    diff = lt.compare(document, document)

    # The library lists the cell twice, but it is one cell: its arcs are
    # compared, and the repetition is not reported as a collision.
    assert document.cells() == ["u1", "u1"]
    assert diff.duplicate_arcs == []
    assert len(diff.entries) == 4
    assert all(entry.matched for entry in diff.entries)


def test_duplicate_cell_names_are_reported(tmp_path: Path):
    path = _write(tmp_path, "duplicate_cell.lib", DUPLICATE_CELL_LIBRARY)
    document = lt.parse_file(path)

    diff = lt.compare(document, document)

    # The document holds two groups named `u1`: the comparison uses the
    # first one, and the repetition is reported instead of being swallowed.
    assert diff.duplicate_cells == ["u1"]
    assert diff.summary()["duplicate_cells"] == 1
    assert diff.has_differences is True
    # The behaviour does not change: the first group is the one compared.
    assert len(diff.entries) == 4
    assert all(entry.matched for entry in diff.entries)
    assert sorted(entry.value_a for entry in diff.entries) == [
        10.0,
        20.0,
        30.0,
        40.0,
    ]


def test_duplicate_cell_names_are_reported_for_either_document(tmp_path: Path):
    unique = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    duplicate = lt.parse_file(
        _write(tmp_path, "duplicate_cell.lib", DUPLICATE_CELL_LIBRARY)
    )

    diff = lt.compare(unique, duplicate)

    assert diff.duplicate_cells == ["u1"]

    # The union is pinned on both sides: the name is reported whichever
    # document holds the repeated group.
    assert lt.compare(duplicate, unique).duplicate_cells == ["u1"]


def test_documents_with_unique_cell_names_report_no_duplicate_cells(
    tmp_path: Path,
):
    path = _write(tmp_path, "ours.lib", OURS)
    document = lt.parse_file(path)

    diff = lt.compare(document, document)

    assert diff.duplicate_cells == []
    assert diff.summary()["duplicate_cells"] == 0
    assert diff.has_differences is False


def test_a_table_with_too_few_values_is_reported_as_malformed(tmp_path: Path):
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", SHORT_VALUES_LIBRARY))

    diff = lt.compare(ours, theirs, cell="u1")

    # The grid of the table has four points but its values hold two: the
    # table is reported instead of diffed.
    assert len(diff.malformed_tables) == 1
    malformed = diff.malformed_tables[0]
    assert malformed.cell == "u1"
    assert malformed.pin == "z"
    assert malformed.table == "cell_rise"
    assert malformed.expected_values == 4
    assert malformed.actual_values == 2

    # The malformed table is not compared numerically: no delta is reported
    # for it, so no wrong number can be read out of the comparison.
    assert diff.entries
    assert all(
        entry.delta is None
        for entry in diff.entries
        if entry.table == "cell_rise"
    )
    assert diff.max_abs_delta() is None

    assert diff.summary()["malformed_tables"] == 1
    assert diff.has_differences is True


def test_interpolation_does_not_invent_values_for_a_malformed_table(
    tmp_path: Path,
):
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", SHORT_VALUES_LIBRARY))

    diff = lt.compare(ours, theirs, cell="u1", interpolate=True)

    # The missing values are missing data, not zeroes: the points where the
    # second document has no value get no value at all.
    assert len(diff.entries) == 4
    assert all(entry.value_a is not None for entry in diff.entries)
    assert all(entry.value_b is None for entry in diff.entries)
    assert all(entry.delta is None for entry in diff.entries)
    assert diff.max_abs_delta() is None


def test_a_nan_value_is_not_treated_as_data(tmp_path: Path):
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", NAN_VALUES_LIBRARY))

    diff = lt.compare(ours, theirs, cell="u1")

    # The third value of the table is `nan`: it is not a number, so the
    # point has no data instead of a delta of `nan`.
    by_point = {(entry.index_1, entry.index_2): entry for entry in diff.entries}
    entry = by_point[(0.2, 1.0)]
    assert entry.value_b is None
    assert entry.matched is False
    assert entry.delta is None

    # The points that do have a value are still compared.
    assert len(diff.matched) == 3
    assert diff.mismatched == []
    assert diff.max_abs_delta() == 0.0


def test_the_tolerance_decides_whether_two_axis_values_are_the_same_point(
    tmp_path: Path,
):
    # The load axis of the second library is off by 1e-11: inside the
    # default tolerance, outside a tighter one.
    slightly_off = OURS.replace(
        'index_1 ("0.1, 0.2");', 'index_1 ("0.10000000001, 0.2");'
    )
    ours = lt.parse_file(_write(tmp_path, "ours.lib", OURS))
    theirs = lt.parse_file(_write(tmp_path, "theirs.lib", slightly_off))

    # The default tolerance is relative, so the two grids describe the same
    # points and the tables are compared as they are.
    diff = lt.compare(ours, theirs, cell="u1")

    assert len(diff.entries) == 4
    assert all(entry.matched for entry in diff.entries)
    assert diff.unmatched == []
    assert diff.max_abs_delta() == 0.0

    # A tolerance below the difference separates the two grids: the load of
    # 0.1 is not a point of the other library any more.
    diff = lt.compare(ours, theirs, cell="u1", tolerance=1e-12)

    by_point = {(entry.index_1, entry.index_2): entry for entry in diff.entries}
    assert by_point[(0.1, 1.0)].value_b is None
    assert by_point[(0.1, 1.0)].delta is None
    assert by_point[(0.1, 1.0)].matched is False
    # The load of 0.2 is the same point in both libraries.
    assert by_point[(0.2, 1.0)].value_b == 30.0
    assert by_point[(0.2, 1.0)].matched is True
    # The point the second library has on its own is reported as well.
    assert diff.unmatched


def test_to_polars_of_an_empty_comparison_has_the_documented_columns(
    docs,
):
    ours, theirs = docs

    diff = lt.compare(ours, theirs, cell="u1", table="cell_fall")

    assert diff.entries == []
    frame = diff.to_polars()
    assert len(frame) == 0
    assert frame.columns == [
        "cell",
        "pin",
        "related_pin",
        "timing_type",
        "timing_sense",
        "when",
        "table",
        "axis_1",
        "axis_2",
        "axis_3",
        "index_1",
        "index_2",
        "index_3",
        "value_a",
        "value_b",
        "delta",
        "relative_delta",
    ]


def test_the_compare_module_is_not_shadowed_by_the_function():
    import liberty_tools.comparison as comparison

    # The module is importable under its own name and holds the classes ...
    assert comparison.Comparison is lt.Comparison
    # ... while the package attribute stays the function of the same name.
    assert callable(lt.compare)
    assert lt.compare is comparison.compare


def test_the_grid_accepts_the_lookup_shorthands(tmp_path: Path):
    a = lt.parse_file(_write(tmp_path, "a.lib", GRID_LIBRARY))
    b = lt.parse_file(_write(tmp_path, "b.lib", GRID_LIBRARY_B))

    diff = lt.compare(
        a,
        b,
        cell="u1",
        interpolate=True,
        grid={"load": [1.0], "transition": [0.1]},
    )

    # The shorthands `lookup()` accepts resolve to the axes they belong to:
    # a load of 1 pF and an input transition of 0.1 ns are the first point
    # of the grid.
    assert len(diff.entries) == 1
    entry = diff.entries[0]
    assert entry.index_1 == 1.0
    assert entry.index_2 == 0.1
    assert entry.value_a == 10.0
    assert entry.value_b == 12.0
    assert entry.delta == -2.0



def test_aligned_3d_grids_compare_in_first_library_order(tmp_path, monkeypatch):
    import liberty_tools.comparison as comparison

    def document(name, roles, axes, values):
        template = " ".join(f"variable_{i} : {role};" for i, role in enumerate(roles, 1))
        index = " ".join(f'index_{i} ("{axis}");' for i, axis in enumerate(axes, 1))
        text = f"""library (test) {{
          lu_table_template (t) {{ {template} }}
          cell (u) {{ pin (z) {{ timing () {{
            cell_rise (t) {{ {index} values ("{values}"); }}
          }} }} }}
        }}"""
        return lt.parse_file(_write(tmp_path, name, text))

    a = document(
        "a.lib", ["input_net_transition", "total_output_net_capacitance", "voltage"],
        ["1, 2", "3, 4", "5, 6"], "1, 2, 3, nan, 5, 6, 7, 8",
    )
    b = document(
        "b.lib", ["total_output_net_capacitance", "voltage", "input_net_transition"],
        ["3, 4", "5, 6", "1, 2"], "1, 5, 2, 6, 3, 7, 4, inf",
    )
    search = comparison._match_value
    searches = []

    def count_search(*args):
        searches.append(args)
        return search(*args)

    monkeypatch.setattr(comparison, "_match_value", count_search)
    result = lt.compare(a, b)
    assert [(e.index_1, e.index_2, e.index_3, e.value_a, e.value_b) for e in result] == [
        (1, 3, 5, 1, 1), (1, 3, 6, 2, 2), (1, 4, 5, 3, 3), (1, 4, 6, None, 4),
        (2, 3, 5, 5, 5), (2, 3, 6, 6, 6), (2, 4, 5, 7, 7), (2, 4, 6, 8, None),
    ]
    assert searches == []


@pytest.mark.parametrize(
    ("axis", "tolerance", "expected"),
    [
        ("1, 2", 0.5, [(1, 1), (2, 1)]),
        ("2, 1", 0.5, [(1, 1), (2, 1)]),
        ("1, 1", 0.0, [(1, 1), (2, 1)]),
        ("inf, 2", 1e-9, [(1, 2), (2, 1)]),
        ("nan, 2", 1e-9, [(1, None), (2, 2), (None, 1)]),
        ("1, 2", -1, [(1, None), (2, None), (None, 1), (None, 2)]),
    ],
)
def test_equal_grids_preserve_first_coordinate_match(tmp_path, axis, tolerance, expected):
    text = f"""library (test) {{
      cell (u) {{ pin (z) {{ timing () {{
        cell_rise (t) {{ index_1 ("{axis}"); values ("1, 2"); }}
      }} }} }}
    }}"""
    doc = lt.parse_file(_write(tmp_path, "grid.lib", text))
    assert [(e.value_a, e.value_b) for e in lt.compare(doc, doc, tolerance=tolerance)] == expected


def test_condition_matching_builds_each_distinct_key_once(tmp_path, monkeypatch):
    from collections import Counter

    import liberty_tools.comparison as comparison

    def document(name, arcs):
        body = []
        for when, value in arcs:
            condition = "" if when is None else f'when : "{when}";'
            body.append(f"""timing () {{ related_pin : D; {condition}
              cell_rise (t) {{ index_1 ("1"); values ("{value}"); }}
            }}""")
        text = f'library (test) {{ cell (u) {{ pin (z) {{ {" ".join(body)} }} }} }}'
        return lt.parse_file(_write(tmp_path, name, text))

    a = document("a.lib", [(None, 1), ("A |", 2), ("A & B", 3), ("B A", 4), ("C", 5)])
    b = document("b.lib", [("A & B", 30), ("D", 60), ("B & A", 40), ("A |", 20), (None, 10)])
    parse = comparison._condition_expr
    calls = Counter()

    def counted(when):
        calls[when] += 1
        return parse(when)

    monkeypatch.setattr(comparison, "_condition_expr", counted)
    result = lt.compare(a, b)
    assert [(e.when, e.value_a, e.value_b) for e in result] == [
        ("A & B", 3, 30), ("B A", 4, 40), (None, 1, 10), ("A |", 2, 20),
    ]
    assert [key.when for key in result.only_in_a] == ["C"]
    assert [key.when for key in result.only_in_b] == ["D"]
    assert calls == Counter([None, "A |", "A & B", "B A", "C", "D", "B & A"])


def test_summary_reads_entries_once_and_reflects_later_mutations(docs):
    class CountedEntries(list):
        passes = 0

        def __iter__(self):
            self.passes += 1
            return super().__iter__()

    result = lt.compare(*docs)
    result.entries = entries = CountedEntries(result.entries)
    summary = result.summary()
    assert summary["matched"] == 4
    assert summary["mismatched"] == 1
    assert summary["unmatched"] == 0
    assert summary["has_differences"] is True
    assert summary["max_abs_delta"] == 5
    assert entries.passes == 1

    entries[0].value_a = None
    entries[2].value_a = None
    entries[2].value_b = None
    updated = result.summary()
    assert updated["matched"] == 2
    assert updated["mismatched"] == 0
    assert updated["unmatched"] == 1
    assert updated["has_differences"] is True
    assert updated["max_abs_delta"] == 0
    assert entries.passes == 2

    entries.clear()
    result.duplicate_cells.append("duplicate")
    diagnostics_only = result.summary()
    assert diagnostics_only["duplicate_cells"] == 1
    assert diagnostics_only["has_differences"] is True
    assert diagnostics_only["max_abs_delta"] is None


def test_targeted_comparison_reports_duplicate_unselected_cells(tmp_path):
    doc = lt.parse_file(_write(tmp_path, "duplicate.lib", DUPLICATE_CELL_LIBRARY))
    result = lt.compare(doc, doc, cell="unselected")
    assert result.entries == []
    assert result.duplicate_cells == ["u1"]
    assert result.summary()["has_differences"] is True
