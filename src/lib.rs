pub mod def;
pub mod lef;
pub mod otw;
pub mod pex;
pub mod threads;
use pyo3::exceptions::{PyIndexError, PyKeyError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString};
use std::collections::HashSet;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};

use open_eda_file_tools_core::liberty;
use open_eda_file_tools_core::liberty::*;
#[pyclass(from_py_object)]
#[derive(Clone)]
struct Document {
    #[pyo3(get)]
    library_name: String,
    #[pyo3(get)]
    voltage_unit: Option<String>,
    #[pyo3(get)]
    current_unit: Option<String>,
    #[pyo3(get)]
    time_unit: Option<String>,
    /// Second arg of `capacitive_load_unit (1, ff)` -> "ff".
    #[pyo3(get)]
    capacitive_load_unit: Option<String>,
    /// template name -> [variable_1, variable_2, variable_3] from
    /// `lu_table_template` / `power_lut_template` / `output_current_template`.
    templates: Vec<(String, [Option<String>; 3])>,
    /// Ordered library-group simple/complex attributes (technology, delay_model,
    /// nom_*, default_*, thresholds, …) — everything that isn't a sub-group.
    attributes: Vec<(String, String)>,
    /// `operating_conditions` groups: name -> simple/complex attributes.
    operating_conditions: Vec<(String, Vec<(String, String)>)>,
    /// First arg of `capacitive_load_unit (1, ff)` -> the "1" multiplier.
    capacitive_load_multiplier: Option<f64>,
    /// `leakage_power_unit` (e.g. "1pW"), the unit of leakage power values.
    leakage_power_unit: Option<String>,
    /// `normalized_driver_waveform` tables: index_1 = input slew, index_2 =
    /// normalized voltage (0..1), values = time. `name` = driver_waveform_name.
    driver_waveforms: Vec<TimingTableData>,
    cells: Vec<CellData>,
    bus_types: Vec<BusTypeData>,
    /// Path the document was parsed from (None for synthetic documents).
    source_path: Option<String>,
    /// Byte length of the source, used to detect a changed file before writing.
    source_len: usize,
    /// FNV-1a hash of the source bytes.
    source_hash: u64,
    /// Byte span of every parsed cell group: (name, start, end).
    cell_spans: Vec<(String, usize, usize)>,
    /// Pending attribute edits, shared with the `Cell`/`Pin` handles handed out
    /// by `Document::cell` / `Cell::pin` so that mutations survive the copy.
    edits: Arc<Mutex<Vec<AttrEdit>>>,
}

#[pyclass(from_py_object)]
#[derive(Clone)]
struct Cell {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    area: Option<f64>,
    pins: Vec<PinData>,
    buses: Vec<BusData>,
    bundles: Vec<BundleData>,
    attributes: Vec<(String, String)>,
    dynamic_currents: Vec<DynamicCurrentData>,
    ff: Option<FfData>,
    latch: Option<LatchData>,
    leakage_powers: Vec<LeakagePowerData>,
    pg_pins: Vec<PgPinData>,
    /// Unit scales inherited from the library header.
    units: UnitContext,
    /// Edit log of the owning document (None for detached cells).
    edits: Option<Arc<Mutex<Vec<AttrEdit>>>>,
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct Pin {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    direction: Option<String>,
    #[pyo3(get)]
    function: Option<String>,
    #[pyo3(get)]
    three_state: Option<String>,
    #[pyo3(get)]
    power_down_function: Option<String>,
    timing_arcs: Vec<TimingArcData>,
    internal_powers: Vec<InternalPowerData>,
    attributes: Vec<(String, String)>,
    /// Unit scales inherited from the library header.
    units: UnitContext,
    /// Edit log of the owning document (None for detached pins).
    edits: Option<Arc<Mutex<Vec<AttrEdit>>>>,
    /// Name of the owning cell (needed to locate a pin in the source).
    cell_name: Option<String>,
    /// The bus or bundle that owns this pin, `None` for a cell-level pin.
    /// The writer needs it to tell two pins of the same name apart.
    container: Option<(ContainerKind, String)>,
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct InternalPower {
    #[pyo3(get)]
    related_pin: Option<String>,
    #[pyo3(get)]
    related_pg_pin: Option<String>,
    #[pyo3(get)]
    when: Option<String>,
    tables: Vec<TimingTableData>,
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct Bus {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    direction: Option<String>,
    #[pyo3(get)]
    function: Option<String>,
    #[pyo3(get)]
    bus_type: Option<String>,
    pins: Vec<PinData>,
    timing_arcs: Vec<TimingArcData>,
    /// Edit log of the owning document (None for detached buses).
    edits: Option<Arc<Mutex<Vec<AttrEdit>>>>,
    /// Name of the owning cell.
    cell_name: Option<String>,
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct Bundle {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    direction: Option<String>,
    #[pyo3(get)]
    function: Option<String>,
    #[pyo3(get)]
    members: Vec<String>,
    pins: Vec<PinData>,
    timing_arcs: Vec<TimingArcData>,
    /// Edit log of the owning document (None for detached bundles).
    edits: Option<Arc<Mutex<Vec<AttrEdit>>>>,
    /// Name of the owning cell.
    cell_name: Option<String>,
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct BusType {
    #[pyo3(get)]
    name: String,
    attributes: Vec<(String, String)>,
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct TimingArc {
    #[pyo3(get)]
    related_pin: Option<String>,
    #[pyo3(get)]
    timing_type: Option<String>,
    #[pyo3(get)]
    when: Option<String>,
    #[pyo3(get)]
    sdf_cond: Option<String>,
    #[pyo3(get)]
    timing_sense: Option<String>,
    tables: Vec<TimingTableData>,
    ccsn_stages: Vec<CcsnStageData>,
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct CcsnStage {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    is_inverting: Option<String>,
    #[pyo3(get)]
    is_needed: Option<String>,
    #[pyo3(get)]
    is_pass_gate: Option<String>,
    #[pyo3(get)]
    stage_type: Option<String>,
    #[pyo3(get)]
    miller_cap_rise: Option<f64>,
    #[pyo3(get)]
    miller_cap_fall: Option<f64>,
    #[pyo3(get)]
    when: Option<String>,
    #[pyo3(get)]
    mode: Option<String>,
    dc_current: Option<TimingTableData>,
    output_voltage_rise: Vec<TimingTableData>,
    output_voltage_fall: Vec<TimingTableData>,
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct TimingTable {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    index_1: Vec<f64>,
    #[pyo3(get)]
    index_2: Vec<f64>,
    #[pyo3(get)]
    index_3: Vec<f64>,
    /// The table values, **flat** and **row-major** over the axes:
    /// `index_1` is the slowest axis, `index_3` the fastest.
    ///
    /// ```text
    /// 2-D:  values[i * len(index_2) + j]
    /// 3-D:  values[i * len(index_2) * len(index_3) + j * len(index_3) + k]
    /// ```
    #[pyo3(get)]
    values: Vec<f64>,
    /// The lookup-table template named in the group header, e.g. `cell_rise
    /// (delay_template_7x7_x1)` -> `delay_template_7x7_x1`. Resolves axis
    /// variable names via `Document::templates`.
    #[pyo3(get)]
    template: Option<String>,
    /// Axis variable names resolved from the table's template:
    /// [index_1 role, index_2 role, index_3 role]. `None` when the table has
    /// no template or the template does not name that axis.
    axis_vars: [Option<String>; 3],
    /// CCS only: the `reference_time` of a `vector` group.
    #[pyo3(get)]
    reference_time: Option<f64>,
    /// CCS only: nested `vector` sub-tables (one current-vs-time wave each) for
    /// `output_current_*` / `ccsn_*` groups. Empty for ordinary NLDM tables.
    vectors: Vec<TimingTableData>,
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct PgPin {
    #[pyo3(get)]
    name: String,
    attributes: Vec<(String, String)>,
}
impl From<PgPinData> for PgPin {
    fn from(value: PgPinData) -> Self {
        Self {
            name: value.name,
            attributes: value.attributes,
        }
    }
}
#[pymethods]
impl PgPin {
    /// Ordered `(name, value)` simple attributes of the pg_pin group.
    fn attributes(&self) -> Vec<(String, String)> {
        self.attributes.clone()
    }
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct LeakagePower {
    #[pyo3(get)]
    value: Option<f64>,
    #[pyo3(get)]
    when: Option<String>,
    #[pyo3(get)]
    related_pg_pin: Option<String>,
}
impl From<LeakagePowerData> for LeakagePower {
    fn from(value: LeakagePowerData) -> Self {
        Self {
            value: value.value,
            when: value.when,
            related_pg_pin: value.related_pg_pin,
        }
    }
}
#[pymethods]
impl LeakagePower {
    /// The `when` condition as a parsed `BooleanExpression` (None if absent).
    fn when_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.when)
    }
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct Ff {
    #[pyo3(get)]
    variable1: Option<String>,
    #[pyo3(get)]
    variable2: Option<String>,
    #[pyo3(get)]
    next_state: Option<String>,
    #[pyo3(get)]
    clocked_on: Option<String>,
    #[pyo3(get)]
    clocked_on_also: Option<String>,
    #[pyo3(get)]
    clear: Option<String>,
    #[pyo3(get)]
    preset: Option<String>,
    #[pyo3(get)]
    power_down_function: Option<String>,
}
impl From<FfData> for Ff {
    fn from(value: FfData) -> Self {
        Self {
            variable1: value.variable1,
            variable2: value.variable2,
            next_state: value.next_state,
            clocked_on: value.clocked_on,
            clocked_on_also: value.clocked_on_also,
            clear: value.clear,
            preset: value.preset,
            power_down_function: value.power_down_function,
        }
    }
}
#[pymethods]
impl Ff {
    fn next_state_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.next_state)
    }
    fn clocked_on_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.clocked_on)
    }
    fn clocked_on_also_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.clocked_on_also)
    }
    fn clear_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.clear)
    }
    fn preset_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.preset)
    }
    fn power_down_function_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.power_down_function)
    }
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct Latch {
    #[pyo3(get)]
    variable1: Option<String>,
    #[pyo3(get)]
    variable2: Option<String>,
    #[pyo3(get)]
    enable: Option<String>,
    #[pyo3(get)]
    data_in: Option<String>,
    #[pyo3(get)]
    clear: Option<String>,
    #[pyo3(get)]
    preset: Option<String>,
}
impl From<LatchData> for Latch {
    fn from(value: LatchData) -> Self {
        Self {
            variable1: value.variable1,
            variable2: value.variable2,
            enable: value.enable,
            data_in: value.data_in,
            clear: value.clear,
            preset: value.preset,
        }
    }
}
#[pymethods]
impl Latch {
    fn enable_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.enable)
    }
    fn data_in_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.data_in)
    }
    fn clear_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.clear)
    }
    fn preset_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.preset)
    }
}
impl From<CellData> for Cell {
    fn from(value: CellData) -> Self {
        Self {
            name: value.name,
            area: value.area,
            pins: value.pins,
            buses: value.buses,
            bundles: value.bundles,
            attributes: value.attributes,
            dynamic_currents: value.dynamic_currents,
            ff: value.ff,
            latch: value.latch,
            leakage_powers: value.leakage_powers,
            pg_pins: value.pg_pins,
            units: value.units,
            edits: None,
        }
    }
}
impl From<PinData> for Pin {
    fn from(value: PinData) -> Self {
        Self {
            name: value.name,
            direction: value.direction,
            function: value.function,
            three_state: value.three_state,
            power_down_function: value.power_down_function,
            timing_arcs: value.timing_arcs,
            internal_powers: value.internal_powers,
            attributes: value.attributes,
            units: value.units,
            edits: None,
            cell_name: None,
            container: None,
        }
    }
}
impl From<InternalPowerData> for InternalPower {
    fn from(value: InternalPowerData) -> Self {
        Self {
            related_pin: value.related_pin,
            related_pg_pin: value.related_pg_pin,
            when: value.when,
            tables: value.tables,
        }
    }
}
impl From<BusData> for Bus {
    fn from(value: BusData) -> Self {
        Self {
            name: value.name,
            direction: value.direction,
            function: value.function,
            bus_type: value.bus_type,
            pins: value.pins,
            timing_arcs: value.timing_arcs,
            edits: None,
            cell_name: None,
        }
    }
}
impl From<BundleData> for Bundle {
    fn from(value: BundleData) -> Self {
        Self {
            name: value.name,
            direction: value.direction,
            function: value.function,
            members: value.members,
            pins: value.pins,
            timing_arcs: value.timing_arcs,
            edits: None,
            cell_name: None,
        }
    }
}
impl From<BusTypeData> for BusType {
    fn from(value: BusTypeData) -> Self {
        Self {
            name: value.name,
            attributes: value.attributes,
        }
    }
}
impl From<TimingArcData> for TimingArc {
    fn from(value: TimingArcData) -> Self {
        Self {
            related_pin: value.related_pin,
            timing_type: value.timing_type,
            when: value.when,
            sdf_cond: value.sdf_cond,
            timing_sense: value.timing_sense,
            tables: value.tables,
            ccsn_stages: value.ccsn_stages,
        }
    }
}
impl From<CcsnStageData> for CcsnStage {
    fn from(value: CcsnStageData) -> Self {
        Self {
            name: value.name,
            is_inverting: value.is_inverting,
            is_needed: value.is_needed,
            is_pass_gate: value.is_pass_gate,
            stage_type: value.stage_type,
            miller_cap_rise: value.miller_cap_rise,
            miller_cap_fall: value.miller_cap_fall,
            when: value.when,
            mode: value.mode,
            dc_current: value.dc_current,
            output_voltage_rise: value.output_voltage_rise,
            output_voltage_fall: value.output_voltage_fall,
        }
    }
}
impl From<TimingTableData> for TimingTable {
    fn from(value: TimingTableData) -> Self {
        Self {
            name: value.name,
            index_1: value.index_1,
            index_2: value.index_2,
            index_3: value.index_3,
            values: value.values,
            template: value.template,
            axis_vars: value.axis_vars,
            reference_time: value.reference_time,
            vectors: value.vectors,
        }
    }
}
#[pymethods]
impl Document {
    fn cells(&self) -> Vec<String> {
        self.cells.iter().map(|cell| cell.name.clone()).collect()
    }

    fn bus_types(&self) -> Vec<String> {
        self.bus_types
            .iter()
            .map(|bus_type| bus_type.name.clone())
            .collect()
    }

    fn bus_type(&self, name: &str) -> PyResult<BusType> {
        self.bus_types
            .iter()
            .find(|bus_type| bus_type.name == name)
            .cloned()
            .map(BusType::from)
            .ok_or_else(|| PyKeyError::new_err(format!("unknown bus type {name:?}")))
    }

    fn cell(&self, name: &str) -> PyResult<Cell> {
        self.cells
            .iter()
            .find(|cell| cell.name == name)
            .cloned()
            .map(|data| {
                let mut cell = Cell::from(data);
                cell.edits = Some(self.edits.clone());
                cell.apply_edits();
                cell
            })
            .ok_or_else(|| PyKeyError::new_err(format!("unknown cell {name:?}")))
    }

    #[pyo3(signature = (cell=None, pin=None, related_pin=None, timing_type=None, when=None, table=None))]
    fn timing_tables<'py>(
        &self,
        py: Python<'py>,
        cell: Option<&str>,
        pin: Option<&str>,
        related_pin: Option<&str>,
        timing_type: Option<&str>,
        when: Option<&str>,
        table: Option<&str>,
    ) -> PyResult<Bound<'py, PyList>> {
        let when_filter = WhenFilter::new(when).map_err(PyValueError::new_err)?;
        let rows = PyList::empty(py);
        for cell_data in &self.cells {
            if !matches_opt(cell, &cell_data.name) {
                continue;
            }
            for pin_data in &cell_data.pins {
                if !matches_opt(pin, &pin_data.name) {
                    continue;
                }
                for arc in &pin_data.timing_arcs {
                    if !matches_opt_opt(related_pin, arc.related_pin.as_deref())
                        || !matches_opt_opt(timing_type, arc.timing_type.as_deref())
                        || !when_filter.matches(arc.when.as_deref())
                    {
                        continue;
                    }
                    for timing_table in &arc.tables {
                        if !matches_opt(table, &timing_table.name) {
                            continue;
                        }
                        append_table_rows(
                            py,
                            &rows,
                            &cell_data.name,
                            &pin_data.name,
                            arc,
                            timing_table,
                        )?;
                    }
                }
            }
            for bus_data in &cell_data.buses {
                if matches_opt(pin, &bus_data.name) {
                    append_matching_arc_tables(
                        py,
                        &rows,
                        &cell_data.name,
                        &bus_data.name,
                        &bus_data.timing_arcs,
                        related_pin,
                        timing_type,
                        &when_filter,
                        table,
                    )?;
                }
                for pin_data in &bus_data.pins {
                    if !matches_opt(pin, &pin_data.name) {
                        continue;
                    }
                    append_matching_arc_tables(
                        py,
                        &rows,
                        &cell_data.name,
                        &pin_data.name,
                        &pin_data.timing_arcs,
                        related_pin,
                        timing_type,
                        &when_filter,
                        table,
                    )?;
                }
            }
            for bundle_data in &cell_data.bundles {
                if matches_opt(pin, &bundle_data.name) {
                    append_matching_arc_tables(
                        py,
                        &rows,
                        &cell_data.name,
                        &bundle_data.name,
                        &bundle_data.timing_arcs,
                        related_pin,
                        timing_type,
                        &when_filter,
                        table,
                    )?;
                }
                for pin_data in &bundle_data.pins {
                    if !matches_opt(pin, &pin_data.name) {
                        continue;
                    }
                    append_matching_arc_tables(
                        py,
                        &rows,
                        &cell_data.name,
                        &pin_data.name,
                        &pin_data.timing_arcs,
                        related_pin,
                        timing_type,
                        &when_filter,
                        table,
                    )?;
                }
            }
        }
        Ok(rows)
    }

    /// SI scale (joules) for `internal_power` energy values: voltage * current *
    /// time unit. ASAP7 (1V, 1mA, 1ps) -> 1e-15 = femtojoules.
    fn energy_unit_joules(&self) -> Option<f64> {
        Some(
            unit_scale(self.voltage_unit.as_deref()?)?
                * unit_scale(self.current_unit.as_deref()?)?
                * unit_scale(self.time_unit.as_deref()?)?,
        )
    }

    /// Lookup-table templates: name -> [variable_1, variable_2, variable_3].
    /// Used to label table axes with their physical quantity.
    fn templates<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let map = PyDict::new(py);
        for (name, vars) in &self.templates {
            let list = PyList::empty(py);
            for v in vars {
                list.append(v.as_deref())?;
            }
            map.set_item(name, list)?;
        }
        Ok(map)
    }

    /// Ordered `(name, value)` library-group simple/complex attributes.
    fn attributes(&self) -> Vec<(String, String)> {
        self.attributes.clone()
    }

    /// Value of a library-level simple/complex attribute, or None if absent.
    fn get(&self, key: &str) -> Option<String> {
        attribute_value(&self.attributes, key)
    }

    /// `operating_conditions` groups: (name, attributes).
    fn operating_conditions(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.operating_conditions.clone()
    }

    /// The attributes of one `operating_conditions` group.
    fn operating_condition(&self, name: &str) -> PyResult<Vec<(String, String)>> {
        self.operating_conditions
            .iter()
            .find(|(group, _)| group == name)
            .map(|(_, attributes)| attributes.clone())
            .ok_or_else(|| PyKeyError::new_err(format!("unknown operating condition {name:?}")))
    }

    /// The multiplier of `capacitive_load_unit (1, ff)` — the "1" in the
    /// example. Together with `capacitive_load_unit` it gives the farad
    /// scale of every capacitance value in the library.
    #[getter]
    fn capacitive_load_multiplier(&self) -> Option<f64> {
        self.capacitive_load_multiplier
    }

    /// `leakage_power_unit` (e.g. "1pW") — the unit of the leakage power
    /// values in the library.
    #[getter]
    fn leakage_power_unit(&self) -> Option<String> {
        self.leakage_power_unit.clone()
    }

    /// `default_max_transition` of the library group, in the library time unit.
    #[getter]
    fn default_max_transition(&self) -> Option<f64> {
        self.get("default_max_transition")
            .as_deref()
            .and_then(parse_first_number)
    }

    /// `default_fanout_load` of the library group (dimensionless).
    #[getter]
    fn default_fanout_load(&self) -> Option<f64> {
        self.get("default_fanout_load")
            .as_deref()
            .and_then(parse_first_number)
    }

    /// `nom_voltage` of the library, in the library voltage unit. Read from
    /// the library group when it declares one there, otherwise from the
    /// default operating condition (as `voltage`).
    #[getter]
    fn nom_voltage(&self) -> Option<f64> {
        self.get("nom_voltage")
            .or_else(|| {
                self.default_operating_condition().and_then(|attributes| {
                    attribute_value(attributes, "nom_voltage")
                        .or_else(|| attribute_value(attributes, "voltage"))
                })
            })
            .as_deref()
            .and_then(parse_first_number)
    }

    /// `nom_temperature` of the library. Read from the library group when it
    /// declares one there, otherwise from the default operating condition (as
    /// `temperature`).
    #[getter]
    fn nom_temperature(&self) -> Option<f64> {
        self.get("nom_temperature")
            .or_else(|| {
                self.default_operating_condition().and_then(|attributes| {
                    attribute_value(attributes, "nom_temperature")
                        .or_else(|| attribute_value(attributes, "temperature"))
                })
            })
            .as_deref()
            .and_then(parse_first_number)
    }

    /// The Liberty source text of this document with every pending attribute
    /// edit applied. Unmodified regions are preserved byte for byte.
    ///
    /// Reading, hashing and lexing the source happens with the GIL released:
    /// on a multi-gigabyte library it would otherwise block every other
    /// Python thread.
    #[pyo3(name = "to_string")]
    fn liberty_to_string(&self, py: Python<'_>) -> PyResult<String> {
        py.detach(move || self.render_source_text())
            .map_err(write_error_to_py)
    }

    /// Write the document (with pending edits applied) to `path`. A path ending
    /// in `.gz` is written gzip-compressed.
    fn write(&self, py: Python<'_>, path: &str) -> PyResult<()> {
        let path = path.to_string();
        py.detach(move || self.write_to_path(&path))
            .map_err(write_error_to_py)
    }

    /// `normalized_driver_waveform` tables (index_1 = slew, index_2 = normalized
    /// voltage, values = time). `name` holds the driver_waveform_name.
    fn driver_waveforms(&self) -> Vec<TimingTable> {
        self.driver_waveforms
            .iter()
            .cloned()
            .map(TimingTable::from)
            .collect()
    }

    #[pyo3(signature = (cell=None, pin=None, related_pin=None, related_pg_pin=None, when=None, table=None))]
    fn internal_power_tables<'py>(
        &self,
        py: Python<'py>,
        cell: Option<&str>,
        pin: Option<&str>,
        related_pin: Option<&str>,
        related_pg_pin: Option<&str>,
        when: Option<&str>,
        table: Option<&str>,
    ) -> PyResult<Bound<'py, PyList>> {
        let when_filter = WhenFilter::new(when).map_err(PyValueError::new_err)?;
        let rows = PyList::empty(py);
        for cell_data in &self.cells {
            if !matches_opt(cell, &cell_data.name) {
                continue;
            }
            let mut pin_groups: Vec<&PinData> = cell_data.pins.iter().collect();
            for bus_data in &cell_data.buses {
                pin_groups.extend(bus_data.pins.iter());
            }
            for bundle_data in &cell_data.bundles {
                pin_groups.extend(bundle_data.pins.iter());
            }
            for pin_data in pin_groups {
                if !matches_opt(pin, &pin_data.name) {
                    continue;
                }
                append_matching_power_tables(
                    py,
                    &rows,
                    &cell_data.name,
                    &pin_data.name,
                    &pin_data.internal_powers,
                    related_pin,
                    related_pg_pin,
                    &when_filter,
                    table,
                )?;
            }
        }
        Ok(rows)
    }
}

#[pymethods]
impl Cell {
    fn pins(&self) -> Vec<String> {
        self.pins.iter().map(|pin| pin.name.clone()).collect()
    }

    fn pin(&self, name: &str) -> PyResult<Pin> {
        self.pins
            .iter()
            .find(|pin| pin.name == name)
            .cloned()
            .map(|data| {
                let mut pin = Pin::from(data);
                pin.edits = self.edits.clone();
                pin.cell_name = Some(self.name.clone());
                pin.apply_edits();
                pin
            })
            .ok_or_else(|| PyKeyError::new_err(format!("unknown pin {name:?}")))
    }

    fn buses(&self) -> Vec<String> {
        self.buses.iter().map(|bus| bus.name.clone()).collect()
    }

    fn bus(&self, name: &str) -> PyResult<Bus> {
        self.buses
            .iter()
            .find(|bus| bus.name == name)
            .cloned()
            .map(|data| {
                let mut bus = Bus::from(data);
                bus.edits = self.edits.clone();
                bus.cell_name = Some(self.name.clone());
                bus
            })
            .ok_or_else(|| PyKeyError::new_err(format!("unknown bus {name:?}")))
    }

    fn bundles(&self) -> Vec<String> {
        self.bundles
            .iter()
            .map(|bundle| bundle.name.clone())
            .collect()
    }

    fn bundle(&self, name: &str) -> PyResult<Bundle> {
        self.bundles
            .iter()
            .find(|bundle| bundle.name == name)
            .cloned()
            .map(|data| {
                let mut bundle = Bundle::from(data);
                bundle.edits = self.edits.clone();
                bundle.cell_name = Some(self.name.clone());
                bundle
            })
            .ok_or_else(|| PyKeyError::new_err(format!("unknown bundle {name:?}")))
    }

    /// Ordered `(name, value)` simple/complex attributes of the cell group.
    fn attributes(&self) -> Vec<(String, String)> {
        self.attributes.clone()
    }

    /// Value of a cell attribute, or None if absent.
    fn get(&self, key: &str) -> Option<String> {
        attribute_value(&self.attributes, key)
    }

    /// Set (or add) a cell attribute. The change is recorded on the owning
    /// document and applied by `Document.write` / `Document.to_string`.
    fn set_attribute(&mut self, key: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let text = attribute_value_text(value)?;
        self.set_attribute_text(key, &text)
    }

    /// Set the cell `area` (the physical cell area, in library area units).
    fn set_area(&mut self, value: f64) -> PyResult<()> {
        self.set_attribute_text("area", &format!("{value}"))
    }

    /// `cell_leakage_power` in the library's power unit — the one declared by
    /// `leakage_power_unit`, or voltage × current when the library does not
    /// declare one.
    #[getter]
    fn cell_leakage_power(&self) -> Option<f64> {
        self.get("cell_leakage_power")
            .as_deref()
            .and_then(parse_first_number)
    }

    /// `cell_leakage_power` in watts.
    #[getter]
    fn cell_leakage_power_watts(&self) -> Option<f64> {
        let value = self.cell_leakage_power()?;
        Some(value * self.units.power?)
    }

    /// `cell_footprint` (the cell's footprint name), if the library declares one.
    #[getter]
    fn cell_footprint(&self) -> Option<String> {
        self.get("cell_footprint")
    }

    /// `dont_touch` (as written in the file, e.g. "true" / "false").
    #[getter]
    fn dont_touch(&self) -> Option<String> {
        self.get("dont_touch")
    }

    /// `dont_use` (as written in the file, e.g. "true" / "false").
    #[getter]
    fn dont_use(&self) -> Option<String> {
        self.get("dont_use")
    }

    /// CCS power (`dynamic_current`) groups.
    fn dynamic_currents(&self) -> Vec<DynamicCurrent> {
        self.dynamic_currents
            .iter()
            .cloned()
            .map(DynamicCurrent::from)
            .collect()
    }

    /// The flip-flop (`ff` / `ff_bank`) group, if the cell is sequential.
    fn ff(&self) -> Option<Ff> {
        self.ff.clone().map(Ff::from)
    }

    /// The latch (`latch` / `latch_bank`) group, if present.
    fn latch(&self) -> Option<Latch> {
        self.latch.clone().map(Latch::from)
    }

    /// `leakage_power` groups (state-dependent / default static leakage).
    fn leakage_powers(&self) -> Vec<LeakagePower> {
        self.leakage_powers
            .iter()
            .cloned()
            .map(LeakagePower::from)
            .collect()
    }

    /// `pg_pin` groups (power/ground rails).
    fn pg_pins(&self) -> Vec<PgPin> {
        self.pg_pins.iter().cloned().map(PgPin::from).collect()
    }
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct DynamicCurrent {
    #[pyo3(get)]
    related_inputs: Option<String>,
    #[pyo3(get)]
    related_outputs: Option<String>,
    #[pyo3(get)]
    when: Option<String>,
    switching_groups: Vec<SwitchingGroupData>,
}
impl From<DynamicCurrentData> for DynamicCurrent {
    fn from(value: DynamicCurrentData) -> Self {
        Self {
            related_inputs: value.related_inputs,
            related_outputs: value.related_outputs,
            when: value.when,
            switching_groups: value.switching_groups,
        }
    }
}
#[pymethods]
impl DynamicCurrent {
    fn switching_groups(&self) -> Vec<SwitchingGroup> {
        self.switching_groups
            .iter()
            .cloned()
            .map(SwitchingGroup::from)
            .collect()
    }

    /// The `when` condition as a parsed `BooleanExpression` (None if absent).
    fn when_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.when)
    }
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct SwitchingGroup {
    #[pyo3(get)]
    input_switching_condition: Option<String>,
    #[pyo3(get)]
    output_switching_condition: Option<String>,
    pg_currents: Vec<PgCurrentData>,
}
impl From<SwitchingGroupData> for SwitchingGroup {
    fn from(value: SwitchingGroupData) -> Self {
        Self {
            input_switching_condition: value.input_switching_condition,
            output_switching_condition: value.output_switching_condition,
            pg_currents: value.pg_currents,
        }
    }
}
#[pymethods]
impl SwitchingGroup {
    fn pg_currents(&self) -> Vec<PgCurrent> {
        self.pg_currents
            .iter()
            .cloned()
            .map(PgCurrent::from)
            .collect()
    }

    /// `input_switching_condition` as a parsed `BooleanExpression` (None if absent).
    fn input_switching_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.input_switching_condition)
    }

    /// `output_switching_condition` as a parsed `BooleanExpression` (None if absent).
    fn output_switching_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.output_switching_condition)
    }
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct PgCurrent {
    #[pyo3(get)]
    pg_pin: Option<String>,
    vectors: Vec<TimingTableData>,
}
impl From<PgCurrentData> for PgCurrent {
    fn from(value: PgCurrentData) -> Self {
        Self {
            pg_pin: value.pg_pin,
            vectors: value.vectors,
        }
    }
}
#[pymethods]
impl PgCurrent {
    /// One `TimingTable` per (slew, cap) — a current-vs-time wave.
    fn vectors(&self) -> Vec<TimingTable> {
        self.vectors
            .iter()
            .cloned()
            .map(TimingTable::from)
            .collect()
    }
}
#[pymethods]
impl Pin {
    #[pyo3(signature = (related_pin=None, timing_type=None, when=None))]
    fn timing_arcs(
        &self,
        related_pin: Option<&str>,
        timing_type: Option<&str>,
        when: Option<&str>,
    ) -> PyResult<Vec<TimingArc>> {
        filter_timing_arcs(&self.timing_arcs, related_pin, timing_type, when)
    }

    #[pyo3(signature = (related_pin=None, related_pg_pin=None, when=None))]
    fn internal_power(
        &self,
        related_pin: Option<&str>,
        related_pg_pin: Option<&str>,
        when: Option<&str>,
    ) -> PyResult<Vec<InternalPower>> {
        let when_filter = WhenFilter::new(when).map_err(PyValueError::new_err)?;
        Ok(self
            .internal_powers
            .iter()
            .filter(|group| {
                matches_opt_opt(related_pin, group.related_pin.as_deref())
                    && matches_opt_opt(related_pg_pin, group.related_pg_pin.as_deref())
                    && when_filter.matches(group.when.as_deref())
            })
            .cloned()
            .map(InternalPower::from)
            .collect())
    }

    /// Ordered `(name, value)` simple/complex attributes of the pin group.
    fn attributes(&self) -> Vec<(String, String)> {
        self.attributes.clone()
    }

    /// Value of a pin attribute, or None if absent.
    fn get(&self, key: &str) -> Option<String> {
        attribute_value(&self.attributes, key)
    }

    /// Set (or add) a pin attribute. The change is recorded on the owning
    /// document and applied by `Document.write` / `Document.to_string`.
    fn set_attribute(&mut self, key: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let text = attribute_value_text(value)?;
        self.set_attribute_text(key, &text)
    }

    /// `capacitance` in the library capacitive-load unit.
    #[getter]
    fn capacitance(&self) -> Option<f64> {
        self.get("capacitance")
            .as_deref()
            .and_then(parse_first_number)
    }

    /// `capacitance` in farads.
    #[getter]
    fn capacitance_farads(&self) -> Option<f64> {
        let value = self.capacitance()?;
        Some(value * self.units.capacitance?)
    }

    /// `rise_capacitance` in the library capacitive-load unit.
    #[getter]
    fn rise_capacitance(&self) -> Option<f64> {
        self.get("rise_capacitance")
            .as_deref()
            .and_then(parse_first_number)
    }

    /// `rise_capacitance` in farads.
    #[getter]
    fn rise_capacitance_farads(&self) -> Option<f64> {
        let value = self.rise_capacitance()?;
        Some(value * self.units.capacitance?)
    }

    /// `fall_capacitance` in the library capacitive-load unit.
    #[getter]
    fn fall_capacitance(&self) -> Option<f64> {
        self.get("fall_capacitance")
            .as_deref()
            .and_then(parse_first_number)
    }

    /// `fall_capacitance` in farads.
    #[getter]
    fn fall_capacitance_farads(&self) -> Option<f64> {
        let value = self.fall_capacitance()?;
        Some(value * self.units.capacitance?)
    }

    /// `max_transition` in the library time unit.
    #[getter]
    fn max_transition(&self) -> Option<f64> {
        self.get("max_transition")
            .as_deref()
            .and_then(parse_first_number)
    }

    /// `max_transition` in seconds.
    #[getter]
    fn max_transition_seconds(&self) -> Option<f64> {
        let value = self.max_transition()?;
        Some(value * self.units.time?)
    }

    /// `min_pulse_width_high` in the library time unit (the pin-attribute
    /// form of the minimum clock pulse width).
    #[getter]
    fn min_pulse_width_high(&self) -> Option<f64> {
        self.get("min_pulse_width_high")
            .or_else(|| self.get("min_capulse_width_high"))
            .as_deref()
            .and_then(parse_first_number)
    }

    /// `min_pulse_width_high` in seconds.
    #[getter]
    fn min_pulse_width_high_seconds(&self) -> Option<f64> {
        let value = self.min_pulse_width_high()?;
        Some(value * self.units.time?)
    }

    /// `min_pulse_width_low` in the library time unit (the pin-attribute form
    /// of the minimum clock pulse width).
    #[getter]
    fn min_pulse_width_low(&self) -> Option<f64> {
        self.get("min_pulse_width_low")
            .or_else(|| self.get("min_capulse_width_low"))
            .as_deref()
            .and_then(parse_first_number)
    }

    /// `min_pulse_width_low` in seconds.
    #[getter]
    fn min_pulse_width_low_seconds(&self) -> Option<f64> {
        let value = self.min_pulse_width_low()?;
        Some(value * self.units.time?)
    }

    /// The pin `function` as a parsed `BooleanExpression` (None if absent).
    fn function_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.function)
    }

    /// The `three_state` condition as a parsed `BooleanExpression` (None if absent).
    fn three_state_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.three_state)
    }

    /// The `power_down_function` as a parsed `BooleanExpression` (None if absent).
    fn power_down_function_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.power_down_function)
    }
}
#[pymethods]
impl InternalPower {
    fn tables(&self) -> Vec<String> {
        self.tables.iter().map(|table| table.name.clone()).collect()
    }

    fn table(&self, name: &str) -> PyResult<TimingTable> {
        self.tables
            .iter()
            .find(|table| table.name == name)
            .cloned()
            .map(TimingTable::from)
            .ok_or_else(|| PyKeyError::new_err(format!("unknown power table {name:?}")))
    }

    /// The `when` condition as a parsed `BooleanExpression` (None if absent).
    fn when_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.when)
    }
}
#[pymethods]
impl Bus {
    fn pins(&self) -> Vec<String> {
        self.pins.iter().map(|pin| pin.name.clone()).collect()
    }

    fn pin(&self, name: &str) -> PyResult<Pin> {
        self.pins
            .iter()
            .find(|pin| pin.name == name)
            .cloned()
            .map(|data| {
                let mut pin = Pin::from(data);
                pin.edits = self.edits.clone();
                pin.cell_name = self.cell_name.clone();
                pin.container = Some((ContainerKind::Bus, self.name.clone()));
                pin.apply_edits();
                pin
            })
            .ok_or_else(|| PyKeyError::new_err(format!("unknown bus pin {name:?}")))
    }

    #[pyo3(signature = (related_pin=None, timing_type=None, when=None))]
    fn timing_arcs(
        &self,
        related_pin: Option<&str>,
        timing_type: Option<&str>,
        when: Option<&str>,
    ) -> PyResult<Vec<TimingArc>> {
        filter_timing_arcs(&self.timing_arcs, related_pin, timing_type, when)
    }

    /// The bus `function` as a parsed `BooleanExpression` (None if absent).
    fn function_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.function)
    }
}
#[pymethods]
impl Bundle {
    fn pins(&self) -> Vec<String> {
        self.pins.iter().map(|pin| pin.name.clone()).collect()
    }

    fn pin(&self, name: &str) -> PyResult<Pin> {
        self.pins
            .iter()
            .find(|pin| pin.name == name)
            .cloned()
            .map(|data| {
                let mut pin = Pin::from(data);
                pin.edits = self.edits.clone();
                pin.cell_name = self.cell_name.clone();
                pin.container = Some((ContainerKind::Bundle, self.name.clone()));
                pin.apply_edits();
                pin
            })
            .ok_or_else(|| PyKeyError::new_err(format!("unknown bundle pin {name:?}")))
    }

    #[pyo3(signature = (related_pin=None, timing_type=None, when=None))]
    fn timing_arcs(
        &self,
        related_pin: Option<&str>,
        timing_type: Option<&str>,
        when: Option<&str>,
    ) -> PyResult<Vec<TimingArc>> {
        filter_timing_arcs(&self.timing_arcs, related_pin, timing_type, when)
    }

    /// The bundle `function` as a parsed `BooleanExpression` (None if absent).
    fn function_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.function)
    }
}
#[pymethods]
impl BusType {
    fn attributes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let attrs = PyDict::new(py);
        for (key, value) in &self.attributes {
            attrs.set_item(key, value)?;
        }
        Ok(attrs)
    }

    fn get(&self, key: &str) -> Option<String> {
        self.attributes
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }
}
#[pymethods]
impl TimingArc {
    fn tables(&self) -> Vec<String> {
        self.tables.iter().map(|table| table.name.clone()).collect()
    }

    fn table(&self, name: &str) -> PyResult<TimingTable> {
        self.tables
            .iter()
            .find(|table| table.name == name)
            .cloned()
            .map(TimingTable::from)
            .ok_or_else(|| PyKeyError::new_err(format!("unknown timing table {name:?}")))
    }

    fn ccsn_stages(&self) -> Vec<CcsnStage> {
        self.ccsn_stages
            .iter()
            .cloned()
            .map(CcsnStage::from)
            .collect()
    }

    /// The `when` condition as a parsed `BooleanExpression` (None if absent).
    fn when_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.when)
    }

    /// The `sdf_cond` condition as a parsed `BooleanExpression` (None if absent).
    fn sdf_cond_expr(&self) -> PyResult<Option<BooleanExpression>> {
        expr_of(&self.sdf_cond)
    }
}
#[pymethods]
impl CcsnStage {
    fn dc_current(&self) -> Option<TimingTable> {
        self.dc_current.clone().map(TimingTable::from)
    }

    fn output_voltage_rise(&self) -> Vec<TimingTable> {
        self.output_voltage_rise
            .iter()
            .cloned()
            .map(TimingTable::from)
            .collect()
    }

    fn output_voltage_fall(&self) -> Vec<TimingTable> {
        self.output_voltage_fall
            .iter()
            .cloned()
            .map(TimingTable::from)
            .collect()
    }
}
#[pymethods]
impl TimingTable {
    /// CCS `vector` sub-tables (current-vs-time waves). Empty for NLDM tables.
    fn vectors(&self) -> Vec<TimingTable> {
        self.vectors
            .iter()
            .cloned()
            .map(TimingTable::from)
            .collect()
    }

    /// Number of axes the table has (1, 2 or 3).
    fn axis_count(&self) -> usize {
        if self.index_1.is_empty() {
            0
        } else if self.index_2.is_empty() {
            1
        } else if self.index_3.is_empty() {
            2
        } else {
            3
        }
    }

    /// The physical variable of each axis, in index order
    /// (`[index_1 role, index_2 role, index_3 role]`), truncated to the axes
    /// the table actually has. Entries are None when the table has no
    /// template or the template does not name that axis.
    fn axis_roles(&self) -> Vec<Option<String>> {
        self.axis_vars[..self.axis_count()].to_vec()
    }

    /// The variable of one axis: 1 = `index_1`, 2 = `index_2`, 3 = `index_3`.
    fn axis_role(&self, axis: usize) -> Option<String> {
        match axis {
            1 => self.axis_vars[0].clone(),
            2 => self.axis_vars[1].clone(),
            3 => self.axis_vars[2].clone(),
            _ => None,
        }
    }

    /// The variable of the `index_1` axis (e.g. "input_net_transition").
    #[getter]
    fn index_1_variable(&self) -> Option<String> {
        self.axis_vars[0].clone()
    }

    /// The variable of the `index_2` axis (e.g. "total_output_net_capacitance").
    #[getter]
    fn index_2_variable(&self) -> Option<String> {
        self.axis_vars[1].clone()
    }

    /// The variable of the `index_3` axis.
    #[getter]
    fn index_3_variable(&self) -> Option<String> {
        self.axis_vars[2].clone()
    }

    /// The value at a grid point.
    ///
    /// Give the position on each axis either by index (`index_1=`, `index_2=`,
    /// `index_3=`) or by the axis's physical variable (`load=`,
    /// `transition=`, or the template's variable name). Returns None when the
    /// point is not on the table's grid.
    #[pyo3(signature = (index_1=None, index_2=None, index_3=None, **roles))]
    fn lookup(
        &self,
        index_1: Option<f64>,
        index_2: Option<f64>,
        index_3: Option<f64>,
        roles: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Option<f64>> {
        let mut coords: [Option<f64>; 3] = [index_1, index_2, index_3];
        if let Some(roles) = roles {
            for (key, value) in roles.iter() {
                let keyword: String = key.extract()?;
                let coordinate: f64 = value.extract()?;
                let axis = self
                    .axis_roles()
                    .iter()
                    .position(|role| {
                        role.as_deref()
                            .map(|role| role_matches_role(role, &keyword))
                            .unwrap_or(false)
                    })
                    .ok_or_else(|| {
                        PyValueError::new_err(format!(
                            "table has no axis named {keyword:?}; its axes are {:?}",
                            self.axis_roles()
                        ))
                    })?;
                if coords[axis].is_some() {
                    return Err(PyValueError::new_err(format!(
                        "axis {} was given twice",
                        axis + 1
                    )));
                }
                coords[axis] = Some(coordinate);
            }
        }
        let axes = self.axis_count();
        if axes == 0 {
            // The table has no grid (a CCS wave container, for instance), so
            // no point can be on it.
            return Ok(None);
        }
        for axis in 0..axes {
            if coords[axis].is_none() {
                return Err(PyValueError::new_err(format!(
                    "axis {} ({}) needs a coordinate",
                    axis + 1,
                    self.axis_vars[axis]
                        .clone()
                        .unwrap_or_else(|| "unnamed".to_string())
                )));
            }
        }
        for axis in axes..3 {
            if coords[axis].is_some() {
                return Err(PyValueError::new_err(format!(
                    "axis {} was given but the table only has {axes} axes",
                    axis + 1
                )));
            }
        }
        let index_1 = match find_axis_position(&self.index_1, coords[0].unwrap()) {
            Some(position) => position,
            None => return Ok(None),
        };
        let index_2 = if axes >= 2 {
            match find_axis_position(&self.index_2, coords[1].unwrap()) {
                Some(position) => position,
                None => return Ok(None),
            }
        } else {
            0
        };
        let index_3 = if axes >= 3 {
            match find_axis_position(&self.index_3, coords[2].unwrap()) {
                Some(position) => position,
                None => return Ok(None),
            }
        } else {
            0
        };
        let flat = flat_index(
            index_1,
            index_2,
            index_3,
            self.index_2.len(),
            self.index_3.len(),
        );
        Ok(self.values.get(flat).copied())
    }

    /// The value at a grid position: `row` indexes `index_1`, `col` indexes
    /// `index_2`, `depth` indexes `index_3`.
    #[pyo3(signature = (row, col, depth=None))]
    fn value_at(&self, row: i64, col: i64, depth: Option<i64>) -> PyResult<f64> {
        // A negative position is out of range like any other one, so it is
        // reported as an IndexError and not as a conversion overflow.
        if row < 0 {
            return Err(PyIndexError::new_err(format!(
                "row {row} is out of range (the table has {} rows)",
                self.index_1.len()
            )));
        }
        if col < 0 {
            return Err(PyIndexError::new_err(format!(
                "col {col} is out of range (the table has {} columns)",
                self.index_2.len()
            )));
        }
        let depth = depth.unwrap_or(0);
        if depth < 0 {
            return Err(PyIndexError::new_err(format!(
                "depth {depth} is out of range (the table has {} levels)",
                self.index_3.len()
            )));
        }
        let (row, col, depth) = (row as usize, col as usize, depth as usize);
        if !self.index_1.is_empty() && row >= self.index_1.len() {
            return Err(PyIndexError::new_err(format!(
                "row {row} is out of range (the table has {} rows)",
                self.index_1.len()
            )));
        }
        if self.index_2.is_empty() {
            if col != 0 {
                return Err(PyIndexError::new_err(
                    "the table has one axis; col must be 0",
                ));
            }
        } else if col >= self.index_2.len() {
            return Err(PyIndexError::new_err(format!(
                "col {col} is out of range (the table has {} columns)",
                self.index_2.len()
            )));
        }
        if self.index_3.is_empty() {
            if depth != 0 {
                return Err(PyIndexError::new_err(
                    "the table has no third axis; depth must be 0",
                ));
            }
        } else if depth >= self.index_3.len() {
            return Err(PyIndexError::new_err(format!(
                "depth {depth} is out of range (the table has {} levels)",
                self.index_3.len()
            )));
        }
        let flat = flat_index(row, col, depth, self.index_2.len(), self.index_3.len());
        self.values.get(flat).copied().ok_or_else(|| {
            PyIndexError::new_err(format!(
                "position ({row}, {col}, {depth}) is outside the table's {} values",
                self.values.len()
            ))
        })
    }

    fn rows<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let rows = PyList::empty(py);
        append_table_rows(
            py,
            &rows,
            "",
            "",
            &TimingArcData {
                related_pin: None,
                timing_type: None,
                when: None,
                sdf_cond: None,
                timing_sense: None,
                tables: Vec::new(),
                ccsn_stages: Vec::new(),
            },
            &TimingTableData {
                name: self.name.clone(),
                index_1: self.index_1.clone(),
                index_2: self.index_2.clone(),
                index_3: self.index_3.clone(),
                values: self.values.clone(),
                template: self.template.clone(),
                axis_vars: self.axis_vars.clone(),
                reference_time: self.reference_time,
                vectors: Vec::new(),
            },
        )?;
        Ok(rows)
    }
}
#[pyfunction]
#[pyo3(signature = (path, cells=None, skip_tables=false))]
fn parse_file(
    py: Python<'_>,
    path: &str,
    cells: Option<Vec<String>>,
    skip_tables: bool,
) -> PyResult<Document> {
    let path = path.to_string();
    let filter = cells.map(|items| items.into_iter().collect::<HashSet<_>>());
    py.detach(move || liberty::parse_file_document(&path, filter, skip_tables))
        .map(Document::from)
        .map_err(parse_load_error_to_py)
}
#[pyfunction]
fn axis_role_matches(role: &str, keyword: &str) -> bool {
    role_matches_role(role, keyword)
}
#[pyfunction]
fn tokenize(path: &str) -> PyResult<Vec<String>> {
    let reader = open_input(path).map_err(|err| PyValueError::new_err(err.to_string()))?;
    lex_all(reader).map_err(parse_error_to_py)
}
#[pyfunction]
fn tokenize_str(text: &str) -> PyResult<Vec<String>> {
    let reader: Box<dyn Read> = Box::new(io::Cursor::new(text.as_bytes().to_vec()));
    lex_all(reader).map_err(parse_error_to_py)
}
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Document>()?;
    m.add_class::<Cell>()?;
    m.add_class::<Pin>()?;
    m.add_class::<Bus>()?;
    m.add_class::<Bundle>()?;
    m.add_class::<BusType>()?;
    m.add_class::<TimingArc>()?;
    m.add_class::<CcsnStage>()?;
    m.add_class::<TimingTable>()?;
    m.add_class::<InternalPower>()?;
    m.add_class::<DynamicCurrent>()?;
    m.add_class::<SwitchingGroup>()?;
    m.add_class::<PgCurrent>()?;
    m.add_class::<LibraryIndex>()?;
    m.add_class::<BooleanExpression>()?;
    m.add_class::<Ff>()?;
    m.add_class::<Latch>()?;
    m.add_class::<LeakagePower>()?;
    m.add_class::<PgPin>()?;
    m.add_function(wrap_pyfunction!(parse_file, m)?)?;
    m.add_function(wrap_pyfunction!(tokenize, m)?)?;
    m.add_function(wrap_pyfunction!(tokenize_str, m)?)?;
    m.add_function(wrap_pyfunction!(axis_role_matches, m)?)?;
    lef::register(m)?;
    def::register(m)?;
    threads::register(m)?;
    pex::register(m)?;
    otw::register(m)?;
    Ok(())
}
fn append_table_rows<'py>(
    py: Python<'py>,
    rows: &Bound<'py, PyList>,
    cell: &str,
    pin: &str,
    arc: &TimingArcData,
    table: &TimingTableData,
) -> PyResult<()> {
    for (idx, value) in table.values.iter().enumerate() {
        let row = PyDict::new(py);
        row.set_item("cell", cell)?;
        row.set_item("pin", pin)?;
        row.set_item("related_pin", arc.related_pin.as_deref())?;
        row.set_item("timing_type", arc.timing_type.as_deref())?;
        row.set_item("when", arc.when.as_deref())?;
        row.set_item("table", &table.name)?;
        let (i, j, k) = table_position(table, idx);
        row.set_item("index_1", table.index_1.get(i).copied())?;
        row.set_item("index_2", table.index_2.get(j).copied())?;
        row.set_item("index_3", table.index_3.get(k).copied())?;
        row.set_item("row", i)?;
        row.set_item("col", j)?;
        row.set_item("depth", k)?;
        row.set_item("value", value)?;
        rows.append(row)?;
    }
    Ok(())
}
fn append_matching_arc_tables<'py>(
    py: Python<'py>,
    rows: &Bound<'py, PyList>,
    cell: &str,
    pin: &str,
    arcs: &[TimingArcData],
    related_pin: Option<&str>,
    timing_type: Option<&str>,
    when_filter: &WhenFilter,
    table: Option<&str>,
) -> PyResult<()> {
    for arc in arcs {
        if !matches_opt_opt(related_pin, arc.related_pin.as_deref())
            || !matches_opt_opt(timing_type, arc.timing_type.as_deref())
            || !when_filter.matches(arc.when.as_deref())
        {
            continue;
        }
        for timing_table in &arc.tables {
            if matches_opt(table, &timing_table.name) {
                append_table_rows(py, rows, cell, pin, arc, timing_table)?;
            }
        }
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn append_matching_power_tables<'py>(
    py: Python<'py>,
    rows: &Bound<'py, PyList>,
    cell: &str,
    pin: &str,
    groups: &[InternalPowerData],
    related_pin: Option<&str>,
    related_pg_pin: Option<&str>,
    when_filter: &WhenFilter,
    table: Option<&str>,
) -> PyResult<()> {
    for group in groups {
        if !matches_opt_opt(related_pin, group.related_pin.as_deref())
            || !matches_opt_opt(related_pg_pin, group.related_pg_pin.as_deref())
            || !when_filter.matches(group.when.as_deref())
        {
            continue;
        }
        for power_table in &group.tables {
            if !matches_opt(table, &power_table.name) {
                continue;
            }
            for (idx, value) in power_table.values.iter().enumerate() {
                let row = PyDict::new(py);
                row.set_item("cell", cell)?;
                row.set_item("pin", pin)?;
                row.set_item("related_pin", group.related_pin.as_deref())?;
                row.set_item("related_pg_pin", group.related_pg_pin.as_deref())?;
                row.set_item("when", group.when.as_deref())?;
                row.set_item("table", &power_table.name)?;
                let (i, j, k) = table_position(power_table, idx);
                row.set_item("index_1", power_table.index_1.get(i).copied())?;
                row.set_item("index_2", power_table.index_2.get(j).copied())?;
                row.set_item("index_3", power_table.index_3.get(k).copied())?;
                row.set_item("row", i)?;
                row.set_item("col", j)?;
                row.set_item("depth", k)?;
                row.set_item("value", value)?;
                rows.append(row)?;
            }
        }
    }
    Ok(())
}
#[pyclass(from_py_object)]
#[derive(Clone)]
struct BooleanExpression {
    source: String,
    expr: BoolExpr,
    support: Vec<String>,
    minterm_parts: Vec<String>,
    canonical: String,
    minimized: String,
}
impl BooleanExpression {
    /// Parse `source` and precompute the canonical + minimized forms. Shared by
    /// the Python constructor and every `*_expr` accessor.
    fn build(source: &str) -> PyResult<Self> {
        let expr = parse_bool_expr(source).map_err(|e| {
            PyValueError::new_err(format!("invalid boolean expression {source:?}: {e}"))
        })?;
        let mut mentioned = Vec::new();
        expr.collect_vars(&mut mentioned);
        mentioned.sort();
        if mentioned.len() > BOOL_VAR_CAP {
            return Err(PyValueError::new_err(format!(
                "boolean expression mentions {} variables (cap {BOOL_VAR_CAP}); \
                 refusing to enumerate 2^{} rows",
                mentioned.len(),
                mentioned.len()
            )));
        }
        let (support, minterm_parts, canonical) = canonical_minterm_parts(&expr, &mentioned);
        let minimized = minimized_sop(&expr, &mentioned, &canonical);
        Ok(Self {
            source: source.to_string(),
            expr,
            support,
            minterm_parts,
            canonical,
            minimized,
        })
    }
}
#[pymethods]
impl BooleanExpression {
    #[new]
    fn new(source: &str) -> PyResult<Self> {
        Self::build(source)
    }

    fn __str__(&self) -> String {
        self.minimized.clone()
    }

    fn __repr__(&self) -> String {
        format!(
            "BooleanExpression('{}')",
            self.source.replace('\\', "\\\\").replace('\'', "\\'")
        )
    }

    fn __eq__(&self, other: &BooleanExpression) -> bool {
        self.canonical == other.canonical
    }

    fn __hash__(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.canonical.hash(&mut hasher);
        hasher.finish()
    }

    /// The original (verbatim) expression text.
    #[getter]
    fn source(&self) -> String {
        self.source.clone()
    }

    /// Alias of `source`.
    #[getter]
    fn raw(&self) -> String {
        self.source.clone()
    }

    /// Support variables (those the function actually depends on), sorted.
    #[getter]
    fn variables(&self) -> Vec<String> {
        self.support.clone()
    }

    /// Canonical product terms (sum-of-minterms over the support).
    fn minterms(&self) -> Vec<String> {
        self.minterm_parts.clone()
    }

    /// The canonical sum-of-minterms string that backs equality/hashing.
    fn canonical(&self) -> String {
        self.canonical.clone()
    }

    /// The minimized sum-of-products string (same as `str(self)`).
    fn minimized(&self) -> String {
        self.minimized.clone()
    }

    /// Evaluate under a `{var: bool}` mapping and/or keyword args. Variables not
    /// given default to false; unknown variables are ignored.
    #[pyo3(signature = (mapping=None, **kwargs))]
    fn eval(
        &self,
        mapping: Option<&Bound<'_, PyDict>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<bool> {
        let mut true_vars = HashSet::new();
        for dict in [mapping, kwargs].into_iter().flatten() {
            for (key, value) in dict.iter() {
                if value.extract::<bool>()? {
                    true_vars.insert(key.extract::<String>()?);
                }
            }
        }
        Ok(self.expr.eval(&true_vars))
    }

    /// True if `self` logically implies `other` (every ON-set assignment of
    /// `self` is in the ON-set of `other`).
    fn implies(&self, other: &BooleanExpression) -> bool {
        bool_implies(&self.expr, &other.expr)
    }
}
#[pyclass]
struct LibraryIndex {
    inner: liberty::LibraryIndex,
}

#[pymethods]
impl LibraryIndex {
    #[staticmethod]
    fn open(py: Python<'_>, path: &str) -> PyResult<LibraryIndex> {
        let path = path.to_string();
        let inner = py
            .detach(move || liberty::open_library_index(&path))
            .map_err(parse_load_error_to_py)?;
        Ok(LibraryIndex { inner })
    }

    #[getter]
    fn library_name(&self) -> String {
        self.inner.library_name.clone()
    }

    fn cell_names(&self) -> Vec<String> {
        self.inner.order.clone()
    }

    fn num_cells(&self) -> usize {
        self.inner.order.len()
    }

    fn cell(&self, py: Python<'_>, name: &str) -> PyResult<Cell> {
        let name = name.to_string();
        py.detach(move || self.inner.parse_cell_by_name(&name))
            .map(Cell::from)
            .map_err(cell_lookup_error_to_py)
    }

    /// Raw Liberty source text of one cell, sliced from the in-memory buffer.
    /// O(cell size) — independent of the total file size.
    fn cell_source(&self, py: Python<'_>, name: &str) -> PyResult<String> {
        let name = name.to_string();
        py.detach(move || self.inner.cell_source_by_name(&name))
            .map_err(cell_source_error_to_py)
    }

    // -- library header pass-throughs (parsed once at open) --
    #[getter]
    fn voltage_unit(&self) -> Option<String> {
        self.inner.header.voltage_unit.clone()
    }
    #[getter]
    fn current_unit(&self) -> Option<String> {
        self.inner.header.current_unit.clone()
    }
    #[getter]
    fn time_unit(&self) -> Option<String> {
        self.inner.header.time_unit.clone()
    }
    #[getter]
    fn capacitive_load_unit(&self) -> Option<String> {
        self.inner.header.capacitive_load_unit.clone()
    }
    fn energy_unit_joules(&self) -> Option<f64> {
        self.inner.header.energy_unit_joules()
    }
    fn templates<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let map = PyDict::new(py);
        for (name, vars) in &self.inner.header.templates {
            let list = PyList::empty(py);
            for v in vars {
                list.append(v.as_deref())?;
            }
            map.set_item(name, list)?;
        }
        Ok(map)
    }
    fn attributes(&self) -> Vec<(String, String)> {
        self.inner.header.attributes.clone()
    }
    fn driver_waveforms(&self) -> Vec<TimingTable> {
        self.inner
            .header
            .driver_waveforms
            .iter()
            .cloned()
            .map(TimingTable::from)
            .collect()
    }
}

impl From<liberty::Document> for Document {
    fn from(value: liberty::Document) -> Self {
        Self {
            library_name: value.library_name,
            voltage_unit: value.voltage_unit,
            current_unit: value.current_unit,
            time_unit: value.time_unit,
            capacitive_load_unit: value.capacitive_load_unit,
            templates: value.templates,
            attributes: value.attributes,
            operating_conditions: value.operating_conditions,
            capacitive_load_multiplier: value.capacitive_load_multiplier,
            leakage_power_unit: value.leakage_power_unit,
            driver_waveforms: value.driver_waveforms,
            cells: value.cells,
            bus_types: value.bus_types,
            source_path: value.source_path,
            source_len: value.source_len,
            source_hash: value.source_hash,
            cell_spans: value.cell_spans,
            edits: value.edits,
        }
    }
}

impl Document {
    /// The operating condition used for the `nom_*` accessors: the group named
    /// by the library attribute `default_operating_conditions`, else the group
    /// named "default", else the first group.
    fn default_operating_condition(&self) -> Option<&Vec<(String, String)>> {
        let named = self.get("default_operating_conditions");
        self.operating_conditions
            .iter()
            .find(|(name, _)| Some(name.as_str()) == named.as_deref())
            .or_else(|| {
                self.operating_conditions
                    .iter()
                    .find(|(name, _)| name == "default")
            })
            .or_else(|| self.operating_conditions.first())
            .map(|(_, attributes)| attributes)
    }

    /// The source text with the pending edits applied.
    ///
    /// Pure Rust — no Python token is involved — so the caller can run it
    /// with the GIL released.
    fn render_source_text(&self) -> Result<String, WriteError> {
        let source = read_source_text(
            self.source_path.as_deref(),
            self.source_len,
            self.source_hash,
        )?;
        let edits = self.edits.lock().map_err(|_| WriteError::Poisoned)?;
        apply_edits_to_source(source, &self.cell_spans, &edits)
    }

    /// Write the rendered document to `path` (gzip-compressed when the path
    /// ends in `.gz`).
    ///
    /// Pure Rust — no Python token is involved — so the caller can run it
    /// with the GIL released.
    fn write_to_path(&self, path: &str) -> Result<(), WriteError> {
        write_source_text(path, &self.render_source_text()?)
    }
}

/// Read the original source text of a document, verifying that the file has
/// not changed since it was parsed.
///
/// Pure Rust: it takes no Python token, so the caller can run it with the GIL
/// released.
/// Apply the pending attribute edits to the source text of a document.
///
/// Only the cells that were edited are re-written; every other byte of the
/// source is kept as it is.
///
/// Pure Rust: it takes no Python token, so the caller can run it with the GIL
/// released.
impl Cell {
    /// Apply the edits already recorded for this cell to a fresh copy, so that
    /// `doc.cell(name)` sees the changes made through a previous handle.
    fn apply_edits(&mut self) {
        let Some(edits) = &self.edits else {
            return;
        };
        let Ok(edits) = edits.lock() else {
            return;
        };
        for edit in edits.iter() {
            if edit.cell == self.name && edit.pin.is_none() {
                set_attribute_in(&mut self.attributes, &edit.key, &edit.value);
                if edit.key == "area" {
                    self.area = parse_first_number(&edit.value);
                }
            }
        }
    }

    /// Apply an attribute change to the in-memory model and record it on the
    /// owning document's edit log.
    fn set_attribute_text(&mut self, key: &str, text: &str) -> PyResult<()> {
        let Some(edits) = &self.edits else {
            // Without a document there is nowhere to record the change, so the
            // edit would be silently lost when the file is written. Refuse.
            return Err(PyRuntimeError::new_err(format!(
                "cell {:?} is not attached to a document; attribute edits cannot be \
                 written back",
                self.name
            )));
        };
        set_attribute_in(&mut self.attributes, key, text);
        if key == "area" {
            self.area = parse_first_number(text);
        }
        let mut edits = edits
            .lock()
            .map_err(|_| PyRuntimeError::new_err("document edit log is poisoned"))?;
        edits.push(AttrEdit {
            cell: self.name.clone(),
            pin: None,
            container: None,
            key: key.to_string(),
            value: text.to_string(),
        });
        Ok(())
    }
}
impl Pin {
    /// Mirror an attribute change into the typed field the getters read, so
    /// that `direction`, `function`, `three_state` and `power_down_function`
    /// (and the `*_expr` accessors built on them) see the edit right away —
    /// the way `Cell` mirrors `area`.
    fn mirror_typed_field(&mut self, key: &str, value: &str) {
        let value = Some(value.to_string());
        match key {
            "direction" => self.direction = value,
            "function" => self.function = value,
            "three_state" => self.three_state = value,
            "power_down_function" => self.power_down_function = value,
            _ => {}
        }
    }

    /// Apply the edits already recorded for this pin to a fresh copy.
    fn apply_edits(&mut self) {
        // The edit log is cloned (an `Arc` bump, not a copy) so that the lock
        // guard does not borrow the pin and the typed fields can be updated
        // below.
        let Some(edits) = self.edits.clone() else {
            return;
        };
        let Ok(edits) = edits.lock() else {
            return;
        };
        for edit in edits.iter() {
            if self.cell_name.as_deref() == Some(edit.cell.as_str())
                && edit.pin.as_deref() == Some(self.name.as_str())
                && edit.container == self.container
            {
                set_attribute_in(&mut self.attributes, &edit.key, &edit.value);
                self.mirror_typed_field(&edit.key, &edit.value);
            }
        }
    }

    /// Apply an attribute change to the in-memory model and record it on the
    /// owning document's edit log.
    fn set_attribute_text(&mut self, key: &str, text: &str) -> PyResult<()> {
        let Some(edits) = self.edits.clone() else {
            // Without a document there is nowhere to record the change, so the
            // edit would be silently lost when the file is written. Refuse.
            return Err(PyRuntimeError::new_err(format!(
                "pin {:?} is not attached to a document; attribute edits cannot be \
                 written back",
                self.name
            )));
        };
        let Some(cell) = self.cell_name.clone() else {
            return Err(PyRuntimeError::new_err(format!(
                "pin {:?} is not attached to a document; attribute edits cannot be \
                 written back",
                self.name
            )));
        };
        set_attribute_in(&mut self.attributes, key, text);
        self.mirror_typed_field(key, text);
        let mut edits = edits
            .lock()
            .map_err(|_| PyRuntimeError::new_err("document edit log is poisoned"))?;
        edits.push(AttrEdit {
            cell,
            pin: Some(self.name.clone()),
            container: self.container.clone(),
            key: key.to_string(),
            value: text.to_string(),
        });
        Ok(())
    }
}
fn attribute_value_text(value: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(text) = value.cast::<PyString>() {
        return Ok(text.to_str()?.to_string());
    }
    // Liberty spells booleans "true"/"false"; a Python bool would otherwise
    // come out as 1/0.
    if let Ok(flag) = value.extract::<bool>() {
        return Ok(if flag { "true" } else { "false" }.to_string());
    }
    if let Ok(number) = value.extract::<f64>() {
        if number.is_finite() && number.fract() == 0.0 && number.abs() < 1e15 {
            return Ok(format!("{}", number as i64));
        }
        return Ok(format!("{number}"));
    }
    Err(PyValueError::new_err(
        "attribute value must be a string, a bool or a number",
    ))
}

fn expr_of(field: &Option<String>) -> PyResult<Option<BooleanExpression>> {
    field.as_deref().map(BooleanExpression::build).transpose()
}

fn filter_timing_arcs(
    arcs: &[TimingArcData],
    related_pin: Option<&str>,
    timing_type: Option<&str>,
    when: Option<&str>,
) -> PyResult<Vec<TimingArc>> {
    let when_filter = WhenFilter::new(when).map_err(PyValueError::new_err)?;
    Ok(arcs
        .iter()
        .filter(|arc| {
            matches_opt_opt(related_pin, arc.related_pin.as_deref())
                && matches_opt_opt(timing_type, arc.timing_type.as_deref())
                && when_filter.matches(arc.when.as_deref())
        })
        .cloned()
        .map(TimingArc::from)
        .collect())
}

fn parse_error_to_py(err: ParseError) -> PyErr {
    PyValueError::new_err(err.to_string())
}

fn parse_load_error_to_py(err: ParseLoadError) -> PyErr {
    PyValueError::new_err(err.to_string())
}

fn cell_lookup_error_to_py(err: CellLookupError) -> PyErr {
    match err {
        CellLookupError::Unknown(_) => PyKeyError::new_err(err.to_string()),
        _ => PyValueError::new_err(err.to_string()),
    }
}

fn cell_source_error_to_py(err: CellSourceError) -> PyErr {
    PyKeyError::new_err(err.to_string())
}

fn write_error_to_py(err: WriteError) -> PyErr {
    match err {
        WriteError::NoSource => PyValueError::new_err(
            "this document has no source file; only documents parsed from a \
             file can be written back",
        ),
        WriteError::Io(message) => PyValueError::new_err(message),
        WriteError::Changed(message) => PyValueError::new_err(message),
        WriteError::Poisoned => PyValueError::new_err("document edit log is poisoned"),
        WriteError::Invalid(message) => PyValueError::new_err(message),
        WriteError::Parse(err) => parse_error_to_py(err),
    }
}
