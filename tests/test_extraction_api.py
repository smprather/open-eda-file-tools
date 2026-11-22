import gzip
from pathlib import Path

import pytest

import liberty_tools as lt
from liberty_tools import _native


LIBERTY_FIXTURE = """
library (api_lib) {
  type (addr_bus_t) {
    base_type : array;
    data_type : bit;
    bit_width : 2;
    bit_from : 1;
    bit_to : 0;
    downto : true;
  }
  cell (ram_macro) {
    area : 42.5;
    bus (A) {
      bus_type : addr_bus_t;
      direction : input;
      function : "ADDR";
      timing () {
        related_pin : "CLK";
        timing_type : setup_rising;
        when : "CS & WE";
        rise_constraint (constraint_template) {
          index_1 ("0.1,0.2");
          index_2 ("0.3,0.4");
          values (\
            "1,2",\
            "3,4");
        }
      }
      pin (A[0]) {
        direction : input;
        timing () {
          related_pin : "CLK";
          timing_type : hold_rising;
          when : "A & B";
          fall_constraint (constraint_template) {
            index_1 ("0.5,0.6");
            values ("7,8");
          }
        }
      }
      pin (A[1]) {
        direction : input;
      }
    }
    bundle (control) {
      members ("CS WE OE");
      direction : input;
      function : "CS & WE";
      pin (CS) {
        direction : input;
      }
      pin (WE) {
        direction : input;
      }
    }
    pin (Q) {
      direction : output;
      function : "A[0]";
      timing () {
        related_pin : "CLK";
        timing_type : rising_edge;
        when : "A & B";
        cell_rise (delay_template) {
          index_1 ("1,2");
          index_2 ("10,20");
          values (\
            "0.1,0.2",\
            "0.3,0.4");
        }
        output_current_rise (ccst_template) {
          index_1 ("0.1,0.2");
          index_2 ("1.0,2.0");
          index_3 ("0.01,0.02,0.03");
          values (\
            "1,2,3",\
            "4,5,6",\
            "7,8,9",\
            "10,11,12");
        }
      }
      timing () {
        related_pin : "CLK";
        timing_type : falling_edge;
        when : "!A";
        cell_fall (delay_template) {
          index_1 ("1,2");
          values ("5,6");
        }
      }
    }
  }
}
"""


def write_fixture(tmp_path: Path, name: str = "api.lib") -> Path:
    path = tmp_path / name
    path.write_text(LIBERTY_FIXTURE, encoding="utf-8")
    return path


def test_document_cell_pin_bus_bundle_and_type_attributes(tmp_path):
    doc = lt.parse_file(write_fixture(tmp_path))

    assert doc.library_name == "api_lib"
    assert doc.cells() == ["ram_macro"]
    assert doc.bus_types() == ["addr_bus_t"]
    assert doc.bus_type("addr_bus_t").attributes() == {
        "base_type": "array",
        "data_type": "bit",
        "bit_width": "2",
        "bit_from": "1",
        "bit_to": "0",
        "downto": "true",
    }

    cell = doc.cell("ram_macro")
    assert cell.name == "ram_macro"
    assert cell.area == 42.5
    assert cell.pins() == ["Q"]
    assert cell.buses() == ["A"]
    assert cell.bundles() == ["control"]

    pin = cell.pin("Q")
    assert pin.name == "Q"
    assert pin.direction == "output"
    assert pin.function == "A[0]"

    bus = cell.bus("A")
    assert bus.name == "A"
    assert bus.direction == "input"
    assert bus.function == "ADDR"
    assert bus.bus_type == "addr_bus_t"
    assert bus.pins() == ["A[0]", "A[1]"]
    assert bus.pin("A[0]").direction == "input"

    bundle = cell.bundle("control")
    assert bundle.name == "control"
    assert bundle.direction == "input"
    assert bundle.function == "CS & WE"
    assert bundle.members == ["CS", "WE", "OE"]
    assert bundle.pins() == ["CS", "WE"]
    assert bundle.pin("WE").direction == "input"


def test_timing_arc_attributes_and_boolean_when_commutation(tmp_path):
    doc = lt.parse_file(write_fixture(tmp_path))
    pin = doc.cell("ram_macro").pin("Q")

    arcs = pin.timing_arcs(related_pin="CLK", timing_type="rising_edge", when="B & A")
    assert len(arcs) == 1
    assert arcs[0].related_pin == "CLK"
    assert arcs[0].timing_type == "rising_edge"
    assert arcs[0].when == "A & B"
    assert arcs[0].tables() == ["cell_rise", "output_current_rise"]

    assert pin.timing_arcs(when="A")[0].when == "A & B"
    assert pin.timing_arcs(when="!A")[0].timing_type == "falling_edge"


def test_1d_2d_and_3d_table_extraction(tmp_path):
    doc = lt.parse_file(write_fixture(tmp_path))
    arc = doc.cell("ram_macro").pin("Q").timing_arcs(when="B & A")[0]

    cell_rise = arc.table("cell_rise")
    assert cell_rise.index_1 == [1.0, 2.0]
    assert cell_rise.index_2 == [10.0, 20.0]
    assert cell_rise.index_3 == []
    assert cell_rise.values == [0.1, 0.2, 0.3, 0.4]

    ccst = arc.table("output_current_rise")
    assert ccst.index_1 == [0.1, 0.2]
    assert ccst.index_2 == [1.0, 2.0]
    assert ccst.index_3 == [0.01, 0.02, 0.03]
    assert ccst.values == [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]

    one_d = doc.cell("ram_macro").pin("Q").timing_arcs(when="!A")[0].table("cell_fall")
    assert one_d.index_1 == [1.0, 2.0]
    assert one_d.index_2 == []
    assert one_d.index_3 == []
    assert one_d.values == [5.0, 6.0]


def test_polars_rows_include_3d_axes_and_nested_bus_timing(tmp_path):
    doc = lt.parse_file(write_fixture(tmp_path))

    ccst = doc.to_polars(table="output_current_rise", when="B & A")
    assert ccst.height == 12
    assert {"index_1", "index_2", "index_3", "row", "col", "depth", "value"} <= set(
        ccst.columns
    )
    assert ccst["index_1"].to_list()[:4] == [0.1, 0.1, 0.1, 0.1]
    assert ccst["index_2"].to_list()[:4] == [1.0, 1.0, 1.0, 2.0]
    assert ccst["index_3"].to_list()[:4] == [0.01, 0.02, 0.03, 0.01]
    assert ccst["depth"].to_list()[:4] == [0, 1, 2, 0]
    assert ccst["value"].to_list()[-1] == 12.0

    bus_rows = doc.to_polars(pin="A", table="rise_constraint", when="WE & CS")
    assert bus_rows["pin"].to_list() == ["A", "A", "A", "A"]
    assert bus_rows["value"].to_list() == [1.0, 2.0, 3.0, 4.0]

    nested_pin_rows = doc.to_polars(pin="A[0]", table="fall_constraint", when="B & A")
    assert nested_pin_rows["pin"].to_list() == ["A[0]", "A[0]"]
    assert nested_pin_rows["value"].to_list() == [7.0, 8.0]


def test_gzipped_input_and_parse_time_cell_filter(tmp_path):
    plain = write_fixture(tmp_path)
    gz_path = tmp_path / "api.lib.gz"
    with gzip.open(gz_path, "wt", encoding="utf-8") as stream:
        stream.write(plain.read_text(encoding="utf-8"))

    doc = lt.parse_file(gz_path, cells=["ram_macro"])
    assert doc.cells() == ["ram_macro"]
    assert doc.cell("ram_macro").pin("Q").timing_arcs(when="B & A")


# internal_power is a Liberty misnomer: the table values are switching *energy*
# (joules), not power. Y carries propagating energy (input switches -> output
# switches); input pin A carries non-propagating energy (A switches, no output
# switch) as a 1-D table with no related_pin.
POWER_FIXTURE = """
library (pwr_lib) {
  voltage_unit : "1V";
  current_unit : "1mA";
  time_unit : "1ps";
  cell (buf) {
    pin (Y) {
      direction : output;
      function : "A";
      internal_power () {
        related_pin : "A";
        related_pg_pin : VDD;
        rise_power (pt) {
          index_1 ("1,2");
          index_2 ("10,20");
          values (\
            "0.1,0.2",\
            "0.3,0.4");
        }
        fall_power (pt) {
          index_1 ("1,2");
          index_2 ("10,20");
          values (\
            "0.5,0.6",\
            "0.7,0.8");
        }
      }
    }
    pin (A) {
      direction : input;
      internal_power () {
        when : "!Y";
        related_pg_pin : VDD;
        rise_power (passive) {
          index_1 ("1,2,3");
          values ("0.01,0.02,0.03");
        }
      }
    }
  }
}
"""


def test_internal_power_energy_unit_and_propagating_extraction(tmp_path):
    path = tmp_path / "pwr.lib"
    path.write_text(POWER_FIXTURE, encoding="utf-8")
    doc = lt.parse_file(path)

    assert (doc.voltage_unit, doc.current_unit, doc.time_unit) == ("1V", "1mA", "1ps")
    assert doc.energy_unit_joules() == 1e-15  # 1V * 1mA * 1ps = femtojoule

    group = doc.cell("buf").pin("Y").internal_power(related_pg_pin="VDD")[0]
    assert group.related_pin == "A"
    assert group.related_pg_pin == "VDD"
    assert group.tables() == ["rise_power", "fall_power"]
    assert group.table("rise_power").values == [0.1, 0.2, 0.3, 0.4]

    rise = doc.to_polars(kind="power", pin="Y", table="rise_power")
    assert rise.height == 4
    assert {"related_pg_pin", "index_1", "index_2", "value"} <= set(rise.columns)
    assert rise["related_pin"].to_list() == ["A", "A", "A", "A"]
    assert rise["value"].to_list() == [0.1, 0.2, 0.3, 0.4]


def test_internal_power_non_propagating_input_pin(tmp_path):
    path = tmp_path / "pwr.lib"
    path.write_text(POWER_FIXTURE, encoding="utf-8")
    doc = lt.parse_file(path)

    nonprop = doc.to_polars(kind="power", pin="A")
    assert nonprop.height == 3  # 1-D passive table
    assert nonprop["related_pin"].to_list() == [None, None, None]  # not propagating
    assert nonprop["when"].to_list() == ["!Y", "!Y", "!Y"]
    assert nonprop["index_1"].to_list() == [1.0, 2.0, 3.0]
    assert nonprop["index_2"].to_list() == [None, None, None]
    assert nonprop["value"].to_list() == [0.01, 0.02, 0.03]


CCS_FIXTURE = """
library (ccs_lib) {
  cell (buf) {
    pin (Y) {
      direction : output;
      function : "A";
      timing () {
        related_pin : "A";
        output_current_rise (ccs_template) {
          vector (ccs_template) {
            reference_time : 1.5;
            index_1 ("5");
            index_2 ("1.44");
            index_3 ("10, 20, 30");
            values ("0.1, 0.2, 0.05");
          }
          vector (ccs_template) {
            reference_time : 1.5;
            index_1 ("5");
            index_2 ("2.88");
            index_3 ("11, 21, 31");
            values ("0.3, 0.4, 0.1");
          }
        }
      }
    }
  }
}
"""


def test_ccs_vector_extraction(tmp_path):
    path = tmp_path / "ccs.lib"
    path.write_text(CCS_FIXTURE, encoding="utf-8")
    doc = lt.parse_file(path)
    tbl = doc.cell("buf").pin("Y").timing_arcs()[0].table("output_current_rise")

    vectors = tbl.vectors()
    assert len(vectors) == 2
    v0 = vectors[0]
    assert v0.reference_time == 1.5
    assert v0.index_1 == [5.0]  # input slew
    assert v0.index_2 == [1.44]  # output cap
    assert v0.index_3 == [10.0, 20.0, 30.0]  # time
    assert v0.values == [0.1, 0.2, 0.05]  # current wave
    assert vectors[1].index_2 == [2.88]


def test_templates_units_and_table_template_name(tmp_path):
    fixture = """
library (tmpl_lib) {
  time_unit : "1ps";
  capacitive_load_unit (1, ff);
  lu_table_template (delay_t) {
    variable_1 : input_net_transition;
    variable_2 : total_output_net_capacitance;
    index_1 ("1, 2");
    index_2 ("3, 4");
  }
  cell (buf) {
    pin (Y) {
      direction : output;
      timing () {
        related_pin : "A";
        cell_rise (delay_t) {
          index_1 ("1, 2");
          index_2 ("3, 4");
          values ("0.1, 0.2", "0.3, 0.4");
        }
      }
    }
  }
}
"""
    path = tmp_path / "tmpl.lib"
    path.write_text(fixture, encoding="utf-8")
    doc = lt.parse_file(path)

    assert doc.capacitive_load_unit == "ff"
    assert doc.templates()["delay_t"] == [
        "input_net_transition",
        "total_output_net_capacitance",
        None,
    ]
    tbl = doc.cell("buf").pin("Y").timing_arcs()[0].table("cell_rise")
    assert tbl.template == "delay_t"


def test_library_attributes_and_driver_waveforms(tmp_path):
    fixture = """
library (lib) {
  time_unit : "1ps";
  delay_model : table_lookup;
  voltage_map (VDD, 0.7);
  normalized_driver_waveform (wave_t) {
    driver_waveform_name : "PreDriver:rise";
    index_1 ("5, 10");
    index_2 ("0, 0.5, 1");
    values ( \
      "0, 1, 2", \
      "0, 2, 4" );
  }
  normalized_driver_waveform (wave_t) {
    index_1 ("5, 10");
    index_2 ("0, 0.5, 1");
    values ("0, 3, 6", "0, 4, 8");
  }
  cell (buf) { area : 1; }
}
"""
    path = tmp_path / "lib_attrs.lib"
    path.write_text(fixture, encoding="utf-8")
    doc = lt.parse_file(path)

    attrs = dict(doc.attributes())
    assert attrs["delay_model"] == "table_lookup"
    assert attrs["time_unit"] == "1ps"
    assert attrs["voltage_map"] == "VDD, 0.7"  # complex attr joined

    waves = doc.driver_waveforms()
    assert [w.name for w in waves] == ["PreDriver:rise", ""]  # 2nd is unnamed
    w0 = waves[0]
    assert list(w0.index_1) == [5.0, 10.0]
    assert list(w0.index_2) == [0.0, 0.5, 1.0]
    # values are row-major over slew x normalized-voltage
    assert list(w0.values) == [0.0, 1.0, 2.0, 0.0, 2.0, 4.0]


def test_ccs_power_dynamic_current_extraction(tmp_path):
    fixture = """
library (ccsp_lib) {
  output_current_template (ccsp_t) {
    variable_1 : input_net_transition;
    variable_2 : total_output_net_capacitance;
    variable_3 : time;
  }
  cell (buf) {
    pg_pin (VDD) { pg_type : primary_power; }
    pg_pin (VSS) { pg_type : primary_ground; }
    dynamic_current () {
      related_inputs : "A";
      related_outputs : "Y";
      switching_group () {
        input_switching_condition (rise);
        output_switching_condition (rise);
        pg_current (VDD) {
          vector (ccsp_t) {
            reference_time : 1.0;
            index_1 ("5"); index_2 ("1.5");
            index_3 ("10, 20, 30");
            values ("0.1, 0.2, 0.05");
          }
        }
        pg_current (VSS) {
          vector (ccsp_t) {
            reference_time : 1.0;
            index_1 ("5"); index_2 ("1.5");
            index_3 ("10, 20, 30");
            values ("-0.1, -0.2, -0.05");
          }
        }
      }
    }
  }
}
"""
    path = tmp_path / "ccsp.lib"
    path.write_text(fixture, encoding="utf-8")
    doc = lt.parse_file(path)

    dcs = doc.cell("buf").dynamic_currents()
    assert len(dcs) == 1
    dc = dcs[0]
    assert (dc.related_inputs, dc.related_outputs) == ("A", "Y")
    sgs = dc.switching_groups()
    assert len(sgs) == 1
    sg = sgs[0]
    assert (sg.input_switching_condition, sg.output_switching_condition) == ("rise", "rise")
    pgs = sg.pg_currents()
    assert [p.pg_pin for p in pgs] == ["VDD", "VSS"]
    vdd_vec = pgs[0].vectors()[0]
    assert list(vdd_vec.index_3) == [10.0, 20.0, 30.0]
    assert list(vdd_vec.values) == [0.1, 0.2, 0.05]  # VDD source: positive
    assert list(pgs[1].vectors()[0].values) == [-0.1, -0.2, -0.05]  # VSS: negated


def test_unbalanced_braces_report_clear_error(tmp_path):
    # A spurious '}' closes the library early, orphaning a trailing cell at the
    # top level (mirrors the ASAP7 SIMPLE-group defect).
    path = tmp_path / "broken.lib"
    path.write_text(
        "library (l) {\n  cell (a) { area : 1; }\n}\n  cell (b) { area : 2; }\n}\n",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="unbalanced braces"):
        lt.parse_file(path)


def test_frame_export_uses_native_columns(tmp_path, monkeypatch):
    liberty = lt.parse_file(write_fixture(tmp_path))
    pl = pytest.importorskip("polars")
    from polars.testing import assert_frame_equal

    def fail(**_filters):
        pytest.fail("column-wise export requested the list of dictionaries")

    power_path = tmp_path / "power.lib"
    power_path.write_text(POWER_FIXTURE)
    power = lt.parse_file(power_path)
    for doc, kind, method, name in (
        (liberty, "timing", "timing_tables", "ram_macro"),
        (power, "power", "internal_power_tables", "buf"),
    ):
        rows = getattr(doc, method)(cell=name)
        assert rows
        expected = pl.DataFrame(rows, infer_schema_length=None)
        with monkeypatch.context() as patch:
            patch.setattr(type(doc), method, lambda self, **filters: fail(**filters))
            assert_frame_equal(doc.to_polars(kind, cell=name), expected)
            assert doc.to_polars(kind, cell="absent").shape == (0, 0)



def test_ipc_preserves_late_optional_columns_and_duplicate_rows(tmp_path):
    import polars as pl
    from polars.testing import assert_frame_equal

    samples = ", ".join(str(n) for n in range(4096))
    source = f"""library (m) {{
        cell (C) {{ area : 1; pin (Y) {{
            timing () {{ cell_rise (a) {{ index_1 ("{samples}"); values ("{samples}"); }} }}
        }} }}
        cell (C) {{ area : 2; pin (Y) {{
            timing () {{ when : A'; cell_rise (scalar) {{ values ("-1"); }} }}
        }} }}
    }}"""
    path = tmp_path / "late-columns.lib"
    path.write_text(source)
    doc = lt.parse_file(path)
    assert doc.cells() == ["C", "C"]
    assert doc.cell("C").area == 1
    rows = doc.timing_tables(cell="C")
    assert len(rows) == 4097 and rows[-1]["when"] == "A'"
    expected = pl.DataFrame(rows, infer_schema_length=None)
    actual = doc.to_polars(cell="C")
    assert actual.schema["index_3"] == pl.Null
    assert actual.schema["row"] == pl.Int64
    assert_frame_equal(actual, expected)

    table = doc.cell("C").pin("Y").timing_arcs()[0].table("cell_rise")
    raw_doc = _native.parse_file(str(path))
    raw_table = raw_doc.cell("C").pin("Y").timing_arcs()[0].table("cell_rise")
    assert_frame_equal(table.to_polars(), pl.DataFrame(raw_table.rows()))
    with pytest.raises(TypeError, match=r"Document\.timing_tables\(\) got an unexpected keyword"):
        doc.to_polars(related_pg_pin="VDD")
    with pytest.raises(TypeError, match=r"Document\.internal_power_tables\(\) got an unexpected keyword"):
        doc.to_polars("power", timing_type="combinational")
    with pytest.raises(ValueError, match="kind"):
        doc.to_polars("noise")  # ty: ignore[invalid-argument-type]
    with pytest.raises(ValueError, match="invalid when expression"):
        doc.to_polars(when="(")


def test_child_handles_keep_shared_cell_data_alive(tmp_path):
    path = write_fixture(tmp_path)
    index = lt.LibraryIndex.open(path)
    cell = index.cell("ram_macro")
    pin = cell.bus("A").pin("A[0]")
    arc = pin.timing_arcs()[0]
    table = arc.table("fall_constraint")
    del index, cell, pin, arc

    values = table.values
    values.clear()
    assert table.values == [7.0, 8.0]
    assert table.index_1 == [0.5, 0.6]
