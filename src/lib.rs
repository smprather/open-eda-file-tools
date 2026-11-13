pub mod def;
pub mod lef;
pub mod otw;
pub mod pex;

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use pyo3::exceptions::{PyIndexError, PyKeyError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

/// Unit scales resolved from the library header and inherited by every cell
/// and pin, so that scalar accessors can report values with their scale
/// attached (and SI-converted variants where useful).
#[derive(Clone, Copy, Default)]
struct UnitContext {
    /// Seconds per library time unit (`time_unit : "1ns"` -> 1e-9).
    time: Option<f64>,
    /// Farads per library capacitive-load unit
    /// (`capacitive_load_unit (1, ff)` -> 1e-15).
    capacitance: Option<f64>,
    /// Watts per library power unit (voltage * current).
    power: Option<f64>,
}

impl UnitContext {
    fn from_library(
        voltage: Option<&str>,
        current: Option<&str>,
        time: Option<&str>,
        capacitance_multiplier: Option<f64>,
        capacitance_unit: Option<&str>,
        leakage_power: Option<&str>,
    ) -> Self {
        let voltage = voltage.and_then(unit_scale);
        let current = current.and_then(unit_scale);
        let time = time.and_then(unit_scale);
        let capacitance = capacitance_unit
            .and_then(unit_scale)
            .map(|scale| scale * capacitance_multiplier.unwrap_or(1.0));
        // Leakage power uses its own unit when the library declares one
        // (`leakage_power_unit : "1pW"`); otherwise it is voltage * current.
        let power = leakage_power
            .and_then(unit_scale)
            .or_else(|| match (voltage, current) {
                (Some(v), Some(c)) => Some(v * c),
                _ => None,
            });
        Self {
            time,
            capacitance,
            power,
        }
    }
}

/// The kind of group that owns a pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ContainerKind {
    Bus,
    Bundle,
}

impl ContainerKind {
    /// The Liberty keyword of the group this kind stands for.
    fn keyword(&self) -> &'static str {
        match self {
            ContainerKind::Bus => "bus",
            ContainerKind::Bundle => "bundle",
        }
    }
}

/// What makes an edited attribute unique: the cell, the pin, the container
/// that owns the pin and the attribute's key.
type EditKey = (
    String,
    Option<String>,
    Option<(ContainerKind, String)>,
    String,
);

/// A pending attribute edit recorded by `Cell::set_attribute` /
/// `Pin::set_attribute`. The writer replays these against the original
/// source text.
#[derive(Clone, Debug)]
struct AttrEdit {
    cell: String,
    /// `None` for a cell-level attribute, `Some(pin)` for a pin attribute.
    pin: Option<String>,
    /// The bus or bundle that owns the pin, `None` for a cell-level pin.
    container: Option<(ContainerKind, String)>,
    key: String,
    value: String,
}

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
    /// Unit scales resolved from the library header.
    units: UnitContext,
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

/// An `internal_power` group. Despite the Liberty name, the table values are
/// switching **energy** (joules in SI), not power; scale = voltage * current *
/// time unit (see `Document::energy_unit_joules`).
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

#[derive(Clone)]
struct CellData {
    name: String,
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
    units: UnitContext,
}

/// A `pg_pin` group (power/ground rail): name + its simple attributes
/// (pg_type, voltage_name, …).
#[derive(Clone)]
struct PgPinData {
    name: String,
    attributes: Vec<(String, String)>,
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

/// A `leakage_power` group: a state-dependent (or default) static leakage value,
/// optionally per power rail.
#[derive(Clone, Default)]
struct LeakagePowerData {
    value: Option<f64>,
    when: Option<String>,
    related_pg_pin: Option<String>,
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

/// A flip-flop (`ff` / `ff_bank`) group. The two header args are the
/// state-variable names (e.g. `IQ, IQN`); the rest are boolean expressions.
#[derive(Clone, Default)]
struct FfData {
    variable1: Option<String>,
    variable2: Option<String>,
    next_state: Option<String>,
    clocked_on: Option<String>,
    clocked_on_also: Option<String>,
    clear: Option<String>,
    preset: Option<String>,
    power_down_function: Option<String>,
}

/// A latch (`latch` / `latch_bank`) group.
#[derive(Clone, Default)]
struct LatchData {
    variable1: Option<String>,
    variable2: Option<String>,
    enable: Option<String>,
    data_in: Option<String>,
    clear: Option<String>,
    preset: Option<String>,
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

/// CCS power (`dynamic_current`): per-condition power/ground current waveforms.
#[derive(Clone)]
struct DynamicCurrentData {
    related_inputs: Option<String>,
    related_outputs: Option<String>,
    when: Option<String>,
    switching_groups: Vec<SwitchingGroupData>,
}

#[derive(Clone)]
struct SwitchingGroupData {
    input_switching_condition: Option<String>,
    output_switching_condition: Option<String>,
    pg_currents: Vec<PgCurrentData>,
}

#[derive(Clone)]
struct PgCurrentData {
    pg_pin: Option<String>,
    /// One `vector` per (slew, cap): a current-vs-time wave (reuses the timing
    /// table's index_1/2 = slew/cap, index_3 = time, values = current).
    vectors: Vec<TimingTableData>,
}

#[derive(Clone)]
struct PinData {
    name: String,
    direction: Option<String>,
    function: Option<String>,
    three_state: Option<String>,
    power_down_function: Option<String>,
    timing_arcs: Vec<TimingArcData>,
    internal_powers: Vec<InternalPowerData>,
    attributes: Vec<(String, String)>,
    units: UnitContext,
}

#[derive(Clone)]
struct InternalPowerData {
    related_pin: Option<String>,
    related_pg_pin: Option<String>,
    when: Option<String>,
    tables: Vec<TimingTableData>,
}

#[derive(Clone)]
struct BusData {
    name: String,
    direction: Option<String>,
    function: Option<String>,
    bus_type: Option<String>,
    pins: Vec<PinData>,
    timing_arcs: Vec<TimingArcData>,
}

#[derive(Clone)]
struct BundleData {
    name: String,
    direction: Option<String>,
    function: Option<String>,
    members: Vec<String>,
    pins: Vec<PinData>,
    timing_arcs: Vec<TimingArcData>,
}

#[derive(Clone)]
struct BusTypeData {
    name: String,
    attributes: Vec<(String, String)>,
}

#[derive(Clone)]
struct TimingArcData {
    related_pin: Option<String>,
    timing_type: Option<String>,
    when: Option<String>,
    sdf_cond: Option<String>,
    timing_sense: Option<String>,
    tables: Vec<TimingTableData>,
    ccsn_stages: Vec<CcsnStageData>,
}

#[derive(Clone)]
struct CcsnStageData {
    name: String,
    is_inverting: Option<String>,
    is_needed: Option<String>,
    is_pass_gate: Option<String>,
    stage_type: Option<String>,
    miller_cap_rise: Option<f64>,
    miller_cap_fall: Option<f64>,
    when: Option<String>,
    mode: Option<String>,
    dc_current: Option<TimingTableData>,
    output_voltage_rise: Vec<TimingTableData>,
    output_voltage_fall: Vec<TimingTableData>,
}

#[derive(Clone)]
struct TimingTableData {
    name: String,
    index_1: Vec<f64>,
    index_2: Vec<f64>,
    index_3: Vec<f64>,
    values: Vec<f64>,
    template: Option<String>,
    /// Axis variable names resolved from the table's template:
    /// [index_1 role, index_2 role, index_3 role].
    axis_vars: [Option<String>; 3],
    reference_time: Option<f64>,
    vectors: Vec<TimingTableData>,
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
        let when_filter = WhenFilter::new(when)?;
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
            .map_err(WriteError::into_py)
    }

    /// Write the document (with pending edits applied) to `path`. A path ending
    /// in `.gz` is written gzip-compressed.
    fn write(&self, py: Python<'_>, path: &str) -> PyResult<()> {
        let path = path.to_string();
        py.detach(move || self.write_to_path(&path))
            .map_err(WriteError::into_py)
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
        let when_filter = WhenFilter::new(when)?;
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
        let when_filter = WhenFilter::new(when)?;
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
    py.detach(move || parse_file_document(&path, filter, skip_tables))
        .map_err(ParseLoadError::into_py)
}

fn parse_file_document(
    path: &str,
    filter: Option<HashSet<String>>,
    skip_tables: bool,
) -> Result<Document, ParseLoadError> {
    let reader = open_input(path).map_err(ParseLoadError::Io)?;
    let mut parser = Parser::new(reader, filter, skip_tables).map_err(ParseLoadError::Parse)?;
    let mut document = parser.parse_document().map_err(ParseLoadError::Parse)?;
    document.source_path = Some(path.to_string());
    Ok(document)
}

/// Encode the lexer's token stream as strings (`W<value>` for words, `S<char>`
/// for symbols). Comments and whitespace are not tokens, so two files with
/// equal token streams parse identically — the basis for `liberty-format`'s
/// functional-transparency guarantee.
fn lex_all(reader: Box<dyn Read>) -> Result<Vec<String>, ParseError> {
    let mut lexer = Lexer::new(reader)?;
    let mut out = Vec::new();
    while let Some(tok) = lexer.next_token()? {
        match tok.kind {
            TokenKind::Word(v) => out.push(format!("W{v}")),
            TokenKind::Symbol(b) => out.push(format!("S{}", b as char)),
        }
    }
    Ok(out)
}

/// Does an axis variable name answer to the given keyword? Accepts the exact
/// name, its synonyms and the usual shorthands (`load`, `transition`, …).
///
/// Exposed so that the comparison helper and the table lookup agree on what
/// an axis is called.
#[pyfunction]
fn axis_role_matches(role: &str, keyword: &str) -> bool {
    role_matches_role(role, keyword)
}

#[pyfunction]
fn tokenize(path: &str) -> PyResult<Vec<String>> {
    let reader = open_input(path).map_err(|err| PyValueError::new_err(err.to_string()))?;
    lex_all(reader).map_err(ParseError::into_py)
}

#[pyfunction]
fn tokenize_str(text: &str) -> PyResult<Vec<String>> {
    let reader: Box<dyn Read> = Box::new(io::Cursor::new(text.as_bytes().to_vec()));
    lex_all(reader).map_err(ParseError::into_py)
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

fn table_position(table: &TimingTableData, idx: usize) -> (usize, usize, usize) {
    if !table.index_3.is_empty() {
        let n3 = table.index_3.len();
        let n2 = table.index_2.len().max(1);
        let plane = n2 * n3;
        (idx / plane, (idx % plane) / n3, idx % n3)
    } else if !table.index_2.is_empty() {
        let n2 = table.index_2.len();
        (idx / n2, idx % n2, 0)
    } else if !table.index_1.is_empty() {
        (idx, 0, 0)
    } else {
        (0, idx, 0)
    }
}

fn filter_timing_arcs(
    arcs: &[TimingArcData],
    related_pin: Option<&str>,
    timing_type: Option<&str>,
    when: Option<&str>,
) -> PyResult<Vec<TimingArc>> {
    let when_filter = WhenFilter::new(when)?;
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

fn matches_opt(filter: Option<&str>, actual: &str) -> bool {
    filter.map_or(true, |value| value == actual)
}

fn matches_opt_opt(filter: Option<&str>, actual: Option<&str>) -> bool {
    filter.map_or(true, |value| actual == Some(value))
}

struct WhenFilter {
    query: Option<BoolExpr>,
}

impl WhenFilter {
    fn new(query: Option<&str>) -> PyResult<Self> {
        let query = query
            .map(parse_bool_expr)
            .transpose()
            .map_err(|err| PyValueError::new_err(format!("invalid when expression: {err}")))?;
        Ok(Self { query })
    }

    fn matches(&self, actual: Option<&str>) -> bool {
        let Some(query) = &self.query else {
            return true;
        };
        let Some(actual) = actual else {
            return false;
        };
        parse_bool_expr(actual)
            .map(|actual| bool_implies(&actual, query))
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug)]
enum BoolExpr {
    Var(String),
    Const(bool),
    Not(Box<BoolExpr>),
    And(Box<BoolExpr>, Box<BoolExpr>),
    Or(Box<BoolExpr>, Box<BoolExpr>),
    Xor(Box<BoolExpr>, Box<BoolExpr>),
}

impl BoolExpr {
    fn eval(&self, true_vars: &HashSet<String>) -> bool {
        match self {
            BoolExpr::Var(name) => true_vars.contains(name),
            BoolExpr::Const(value) => *value,
            BoolExpr::Not(expr) => !expr.eval(true_vars),
            BoolExpr::And(left, right) => left.eval(true_vars) && right.eval(true_vars),
            BoolExpr::Or(left, right) => left.eval(true_vars) || right.eval(true_vars),
            BoolExpr::Xor(left, right) => left.eval(true_vars) ^ right.eval(true_vars),
        }
    }

    fn collect_vars(&self, vars: &mut Vec<String>) {
        match self {
            BoolExpr::Var(name) => {
                if !vars.iter().any(|item| item == name) {
                    vars.push(name.clone());
                }
            }
            BoolExpr::Const(_) => {}
            BoolExpr::Not(expr) => expr.collect_vars(vars),
            BoolExpr::And(left, right) | BoolExpr::Or(left, right) | BoolExpr::Xor(left, right) => {
                left.collect_vars(vars);
                right.collect_vars(vars);
            }
        }
    }
}

fn bool_implies(actual: &BoolExpr, query: &BoolExpr) -> bool {
    let mut vars = Vec::new();
    actual.collect_vars(&mut vars);
    query.collect_vars(&mut vars);
    let assignment_count = 1usize.checked_shl(vars.len() as u32).unwrap_or(0);
    if assignment_count == 0 {
        return false;
    }

    for mask in 0..assignment_count {
        let mut true_vars = HashSet::new();
        for (idx, name) in vars.iter().enumerate() {
            if (mask & (1usize << idx)) != 0 {
                true_vars.insert(name.clone());
            }
        }
        if actual.eval(&true_vars) && !query.eval(&true_vars) {
            return false;
        }
    }
    true
}

fn parse_bool_expr(input: &str) -> Result<BoolExpr, String> {
    let mut parser = BoolParser::new(input);
    let expr = parser.parse_or()?;
    parser.skip_ws();
    if parser.is_eof() {
        Ok(expr)
    } else {
        Err(format!("unexpected token at byte {}", parser.pos))
    }
}

struct BoolParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> BoolParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn parse_or(&mut self) -> Result<BoolExpr, String> {
        let mut expr = self.parse_xor()?;
        loop {
            self.skip_ws();
            if self.consume('|') || self.consume('+') {
                let right = self.parse_xor()?;
                expr = BoolExpr::Or(Box::new(expr), Box::new(right));
            } else {
                return Ok(expr);
            }
        }
    }

    fn parse_xor(&mut self) -> Result<BoolExpr, String> {
        let mut expr = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.consume('^') {
                let right = self.parse_and()?;
                expr = BoolExpr::Xor(Box::new(expr), Box::new(right));
            } else {
                return Ok(expr);
            }
        }
    }

    fn parse_and(&mut self) -> Result<BoolExpr, String> {
        let mut expr = self.parse_not()?;
        loop {
            self.skip_ws();
            if self.consume('&') || self.consume('*') {
                let right = self.parse_not()?;
                expr = BoolExpr::And(Box::new(expr), Box::new(right));
            } else if self.at_primary_start() {
                // Liberty Table 7-4: a space is AND, so `A B` == `A & B`. The
                // suffix `'` is excluded below, so `A'B` parses as Not(A) & B.
                let right = self.parse_not()?;
                expr = BoolExpr::And(Box::new(expr), Box::new(right));
            } else {
                return Ok(expr);
            }
        }
    }

    /// True if the next non-whitespace char can begin a primary/`not` term,
    /// i.e. an implicit (juxtaposition) AND should be inserted here. False for
    /// the binary/suffix operators and `)` (which terminate an AND chain) and
    /// for end-of-input.
    fn at_primary_start(&self) -> bool {
        let mut probe = self.pos;
        while let Some(ch) = self.input[probe..].chars().next() {
            if ch.is_whitespace() {
                probe += ch.len_utf8();
            } else {
                return !matches!(ch, '|' | '+' | '^' | '&' | '*' | '\'' | ')');
            }
        }
        false
    }

    fn parse_not(&mut self) -> Result<BoolExpr, String> {
        self.skip_ws();
        if self.consume('!') || self.consume('~') {
            Ok(BoolExpr::Not(Box::new(self.parse_not()?)))
        } else {
            let mut expr = self.parse_primary()?;
            loop {
                self.skip_ws();
                if self.consume('\'') {
                    expr = BoolExpr::Not(Box::new(expr));
                } else {
                    return Ok(expr);
                }
            }
        }
    }

    fn parse_primary(&mut self) -> Result<BoolExpr, String> {
        self.skip_ws();
        if self.consume('(') {
            let expr = self.parse_or()?;
            self.skip_ws();
            if !self.consume(')') {
                return Err(format!("expected ')' at byte {}", self.pos));
            }
            return Ok(expr);
        }
        let ident = self.parse_ident()?;
        match ident.as_str() {
            "1" | "true" | "TRUE" => Ok(BoolExpr::Const(true)),
            "0" | "false" | "FALSE" => Ok(BoolExpr::Const(false)),
            _ => Ok(BoolExpr::Var(ident)),
        }
    }

    fn parse_ident(&mut self) -> Result<String, String> {
        self.skip_ws();
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_whitespace()
                || matches!(
                    ch,
                    '!' | '~' | '&' | '*' | '|' | '+' | '^' | '\'' | '(' | ')'
                )
            {
                break;
            }
            self.pos += ch.len_utf8();
        }
        if self.pos == start {
            Err(format!("expected identifier at byte {}", self.pos))
        } else {
            Ok(self.input[start..self.pos].to_string())
        }
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(ch) if ch.is_whitespace()) {
            self.pos += self.peek().unwrap().len_utf8();
        }
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }
}

// ---- canonical / minimized boolean forms -----------------------------------
//
// Enumeration is over the *mentioned* variables (support discovery needs the
// full truth table), so the 2^n cost is bounded by this cap rather than the
// support size. Real Liberty `function`/`when` expressions use a handful of
// variables; 12 is a generous ceiling and keeps both the truth-table and the
// Quine-McCluskey minimizer fast.
const BOOL_VAR_CAP: usize = 12;

/// Render one product term: literals sorted alphabetically by variable name,
/// each as `VAR` or `!VAR`, joined by `*`.
fn render_term(mut lits: Vec<(String, bool)>) -> String {
    lits.sort_by(|a, b| a.0.cmp(&b.0));
    lits.into_iter()
        .map(|(name, neg)| if neg { format!("!{name}") } else { name })
        .collect::<Vec<_>>()
        .join("*")
}

/// Canonical sum-of-minterms over the expression's *support* (vacuous variables
/// eliminated, so `A & (B | !B)` canonicalizes to `A`). Returns the sorted
/// support, the sorted product-term strings, and their `+`-joined canonical
/// string. Tautology -> "1", contradiction -> "0". `mentioned` must be the
/// sorted, cap-checked list of variables the expression names.
fn canonical_minterm_parts(
    expr: &BoolExpr,
    mentioned: &[String],
) -> (Vec<String>, Vec<String>, String) {
    let n = mentioned.len();
    let rows = 1usize << n;

    // Full truth table over mentioned vars; mentioned[0] is the MSB.
    let mut outputs = vec![false; rows];
    for (mask, out) in outputs.iter_mut().enumerate() {
        let mut true_vars = HashSet::new();
        for (i, name) in mentioned.iter().enumerate() {
            if (mask >> (n - 1 - i)) & 1 == 1 {
                true_vars.insert(name.clone());
            }
        }
        *out = expr.eval(&true_vars);
    }

    // Support: a var is kept iff flipping only it changes the output somewhere.
    let mut in_support = vec![false; n];
    for (j, sup) in in_support.iter_mut().enumerate() {
        let jbit = n - 1 - j;
        for mask in 0..rows {
            if (mask >> jbit) & 1 == 0 && outputs[mask] != outputs[mask | (1 << jbit)] {
                *sup = true;
                break;
            }
        }
    }
    let support: Vec<String> = mentioned
        .iter()
        .zip(&in_support)
        .filter(|&(_, &keep)| keep)
        .map(|(name, _)| name.clone())
        .collect();

    if !outputs.iter().any(|&b| b) {
        return (support, Vec::new(), "0".to_string());
    }
    if outputs.iter().all(|&b| b) {
        return (support, Vec::new(), "1".to_string());
    }

    // One product term per ON row of the support-only truth table.
    let k = support.len();
    let support_bits: Vec<usize> = support
        .iter()
        .map(|name| n - 1 - mentioned.iter().position(|m| m == name).unwrap())
        .collect();
    let mut parts: Vec<String> = Vec::new();
    for srow in 0..(1usize << k) {
        let mut mask = 0usize;
        for (s, &bitpos) in support_bits.iter().enumerate() {
            if (srow >> (k - 1 - s)) & 1 == 1 {
                mask |= 1 << bitpos;
            }
        }
        if !outputs[mask] {
            continue;
        }
        let lits: Vec<(String, bool)> = support
            .iter()
            .enumerate()
            .map(|(s, name)| (name.clone(), (srow >> (k - 1 - s)) & 1 == 0))
            .collect();
        parts.push(render_term(lits));
    }
    parts.sort();
    let canonical = parts.join(" + ");
    (support, parts, canonical)
}

/// Convert our `BoolExpr` into the minimizer crate's `Bool`, mapping variable
/// names to indices by their position in `mentioned` (XOR is expanded since the
/// crate has no XOR node).
fn boolexpr_to_qmc(expr: &BoolExpr, mentioned: &[String]) -> quine_mc_cluskey::Bool {
    use quine_mc_cluskey::Bool;
    match expr {
        BoolExpr::Const(true) => Bool::True,
        BoolExpr::Const(false) => Bool::False,
        BoolExpr::Var(name) => Bool::Term(mentioned.iter().position(|m| m == name).unwrap() as u8),
        BoolExpr::Not(e) => Bool::Not(Box::new(boolexpr_to_qmc(e, mentioned))),
        BoolExpr::And(l, r) => Bool::And(vec![
            boolexpr_to_qmc(l, mentioned),
            boolexpr_to_qmc(r, mentioned),
        ]),
        BoolExpr::Or(l, r) => Bool::Or(vec![
            boolexpr_to_qmc(l, mentioned),
            boolexpr_to_qmc(r, mentioned),
        ]),
        BoolExpr::Xor(l, r) => Bool::Or(vec![
            Bool::And(vec![
                boolexpr_to_qmc(l, mentioned),
                Bool::Not(Box::new(boolexpr_to_qmc(r, mentioned))),
            ]),
            Bool::And(vec![
                Bool::Not(Box::new(boolexpr_to_qmc(l, mentioned))),
                boolexpr_to_qmc(r, mentioned),
            ]),
        ]),
    }
}

fn collect_qmc_literals(
    b: &quine_mc_cluskey::Bool,
    mentioned: &[String],
    out: &mut Vec<(String, bool)>,
) {
    use quine_mc_cluskey::Bool;
    match b {
        Bool::Term(i) => out.push((mentioned[*i as usize].clone(), false)),
        Bool::Not(inner) => {
            if let Bool::Term(i) = inner.as_ref() {
                out.push((mentioned[*i as usize].clone(), true));
            }
        }
        Bool::And(items) => items
            .iter()
            .for_each(|it| collect_qmc_literals(it, mentioned, out)),
        _ => {}
    }
}

fn render_qmc_sop(b: &quine_mc_cluskey::Bool, mentioned: &[String]) -> String {
    use quine_mc_cluskey::Bool;
    match b {
        Bool::True => "1".to_string(),
        Bool::False => "0".to_string(),
        Bool::Or(terms) => {
            let mut parts: Vec<String> = terms
                .iter()
                .map(|t| {
                    let mut lits = Vec::new();
                    collect_qmc_literals(t, mentioned, &mut lits);
                    render_term(lits)
                })
                .collect();
            parts.sort();
            parts.join(" + ")
        }
        other => {
            let mut lits = Vec::new();
            collect_qmc_literals(other, mentioned, &mut lits);
            render_term(lits)
        }
    }
}

/// Minimized sum-of-products (Quine-McCluskey). `simplify()` can return several
/// equally-minimal covers; render each and pick the lexicographically smallest
/// so the result is deterministic. Constants short-circuit via `canonical`.
fn minimized_sop(expr: &BoolExpr, mentioned: &[String], canonical: &str) -> String {
    if canonical == "0" || canonical == "1" {
        return canonical.to_string();
    }
    let mut rendered: Vec<String> = boolexpr_to_qmc(expr, mentioned)
        .simplify()
        .iter()
        .map(|sol| render_qmc_sop(sol, mentioned))
        .collect();
    rendered.sort();
    rendered
        .into_iter()
        .next()
        .unwrap_or_else(|| canonical.to_string())
}

/// A parsed Liberty boolean expression. `__str__` is a minimized sum-of-products
/// (compact, human-readable); equality/hashing use the canonical sum-of-minterms
/// over the support, so `==` is true exactly when two expressions are logically
/// equivalent.
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

/// Parse an optional raw boolean field into a `BooleanExpression`, propagating a
/// parse error as `PyValueError`. Used by the `*_expr` accessors.
fn expr_of(field: &Option<String>) -> PyResult<Option<BooleanExpression>> {
    field.as_deref().map(BooleanExpression::build).transpose()
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

fn open_input(path: &str) -> io::Result<Box<dyn Read>> {
    let file = File::open(path)?;
    let reader = BufReader::with_capacity(128 * 1024, file);
    if path.ends_with(".gz") {
        Ok(Box::new(GzDecoder::new(reader)))
    } else {
        Ok(Box::new(reader))
    }
}

// ---- byte-offset cell indexing (lazy-parse backend) ------------------------
//
// Scan the whole (decompressed) file once, recording the byte range of every
// top-level `cell (...) {...}` group, so the viewer can list cells and parse one
// at a time instead of parsing the entire file up front. The scanner is lexer-
// faithful about strings/comments but does *not* build any AST.

fn read_to_memory(path: &str) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    open_input(path)?.read_to_end(&mut buf)?;
    Ok(buf)
}

fn is_index_symbol(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'{' | b'}' | b':' | b';' | b',')
}

/// Skip whitespace, `#`/`//`/`* */` comments, and `\`+newline continuations.
fn skip_trivia(d: &[u8], mut i: usize) -> usize {
    let n = d.len();
    loop {
        while i < n && d[i].is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < n && d[i] == b'\\' && (d[i + 1] == b'\n' || d[i + 1] == b'\r') {
            i += 2;
        } else if i < n && d[i] == b'#' {
            while i < n && d[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < n && d[i] == b'/' && d[i + 1] == b'/' {
            while i < n && d[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < n && d[i] == b'/' && d[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(d[i] == b'*' && d[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
        } else {
            break;
        }
    }
    i
}

/// Consume a quoted string starting at `d[i] == quote`; return index after the
/// closing quote, honoring backslash escapes.
fn skip_quoted(d: &[u8], mut i: usize) -> usize {
    let n = d.len();
    let quote = d[i];
    i += 1;
    while i < n {
        if d[i] == b'\\' {
            i += 2;
            continue;
        }
        if d[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    i
}

/// Read one token (bare word or quoted string) at `i`; return (value, next).
fn read_token(d: &[u8], i: usize) -> (String, usize) {
    let n = d.len();
    if i < n && (d[i] == b'"' || d[i] == b'\'') {
        let end = skip_quoted(d, i);
        let inner = &d[i + 1..end.saturating_sub(1).max(i + 1)];
        return (String::from_utf8_lossy(inner).into_owned(), end);
    }
    let start = i;
    let mut j = i;
    while j < n && !d[j].is_ascii_whitespace() && !is_index_symbol(d[j]) {
        j += 1;
    }
    (String::from_utf8_lossy(&d[start..j]).into_owned(), j)
}

/// At `d[i] == '('`, read the comma-separated args (quotes stripped); return
/// (args, index after the matching ')').
fn read_paren_args(d: &[u8], mut i: usize) -> (Vec<String>, usize) {
    let n = d.len();
    i += 1; // past '('
    let mut args = Vec::new();
    let mut cur = String::new();
    while i < n {
        i = skip_trivia(d, i);
        if i >= n {
            break;
        }
        match d[i] {
            b')' => {
                i += 1;
                break;
            }
            b',' => {
                args.push(cur.trim().to_string());
                cur.clear();
                i += 1;
            }
            b'"' | b'\'' => {
                let end = skip_quoted(d, i);
                cur.push_str(&String::from_utf8_lossy(
                    &d[i + 1..end.saturating_sub(1).max(i + 1)],
                ));
                i = end;
            }
            _ => {
                cur.push(d[i] as char);
                i += 1;
            }
        }
    }
    if !cur.trim().is_empty() || !args.is_empty() {
        args.push(cur.trim().to_string());
    }
    (args, i)
}

/// At `d[i] == '{'`, return the index just past the matching `}` (string/comment
/// aware).
fn match_braces(d: &[u8], mut i: usize) -> usize {
    let n = d.len();
    let mut depth = 0usize;
    while i < n {
        match d[i] {
            b'"' | b'\'' => i = skip_quoted(d, i),
            b'#' => {
                while i < n && d[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && d[i + 1] == b'/' => {
                while i < n && d[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && d[i + 1] == b'*' => {
                i += 2;
                while i + 1 < n && !(d[i] == b'*' && d[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(n);
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => i += 1,
        }
    }
    i
}

fn skip_to_semicolon(d: &[u8], mut i: usize) -> usize {
    let n = d.len();
    while i < n {
        match d[i] {
            b'"' | b'\'' => i = skip_quoted(d, i),
            b';' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// The top-level structure of a Liberty file, found by a byte-level scan.
struct LibraryScan {
    /// Name of the library group.
    name: String,
    /// Byte range of the library body before the first cell group.
    preamble: (usize, usize),
    /// Byte ranges of the top-level items that follow the first cell group
    /// (units, templates, operating conditions, …), in source order.
    late_items: Vec<(usize, usize)>,
    /// (cell name, start, end) of every cell group.
    cells: Vec<(String, usize, usize)>,
}

/// Index every top-level `cell` group. Lenient: a stray `}` that drops
/// depth early does not stop cell discovery.
///
/// The scan also records the top-level items that follow the first cell:
/// a library may declare its units or its lookup-table templates *after*
/// the cells that use them, and `parse_file` resolves them there too.
fn index_cells(d: &[u8]) -> LibraryScan {
    let n = d.len();
    let mut i = skip_trivia(d, 0);
    let (_kw, j) = read_token(d, i);
    i = skip_trivia(d, j);
    let mut lib_name = String::new();
    if i < n && d[i] == b'(' {
        let (args, k) = read_paren_args(d, i);
        lib_name = args.into_iter().next().unwrap_or_default();
        i = skip_trivia(d, k);
    }
    if i < n && d[i] == b'{' {
        i += 1;
    }
    let body_start = i;
    let mut cells = Vec::new();
    // Byte ranges of the top-level items that are not cells.
    let mut items: Vec<(usize, usize)> = Vec::new();
    let mut first_cell: Option<usize> = None;
    while i < n {
        i = skip_trivia(d, i);
        if i >= n {
            break;
        }
        if d[i] == b'}' {
            i += 1; // library close or stray — keep scanning
            continue;
        }
        let tok_start = i;
        let (name, j) = read_token(d, i);
        if name.is_empty() {
            i += 1;
            continue;
        }
        i = skip_trivia(d, j);
        if i < n && d[i] == b'(' {
            let (args, k) = read_paren_args(d, i);
            i = skip_trivia(d, k);
            if i < n && d[i] == b'{' {
                let end = match_braces(d, i);
                let is_cell = name == "cell";
                if is_cell {
                    if first_cell.is_none() {
                        first_cell = Some(tok_start);
                    }
                    cells.push((args.into_iter().next().unwrap_or_default(), tok_start, end));
                }
                i = end;
                let s = skip_trivia(d, i);
                if s < n && d[s] == b';' {
                    i = s + 1;
                }
                if !is_cell {
                    items.push((tok_start, i));
                }
            } else if i < n && d[i] == b';' {
                i += 1;
                items.push((tok_start, i));
            }
        } else if i < n && d[i] == b':' {
            i = skip_to_semicolon(d, i + 1);
            items.push((tok_start, i));
        } else {
            i += 1;
        }
    }
    let header_end = first_cell.unwrap_or(i);
    // The items before the first cell are part of the preamble already; only
    // the ones after it have to be collected separately.
    let late_items = match first_cell {
        Some(start) => items
            .into_iter()
            .filter(|(item_start, _)| *item_start >= start)
            .collect(),
        None => Vec::new(),
    };
    LibraryScan {
        name: lib_name,
        preamble: (body_start, header_end),
        late_items,
        cells,
    }
}

fn parse_slice_as_library(name: &str, body: &[u8]) -> Result<Document, ParseError> {
    let mut buf = Vec::with_capacity(body.len() + 32);
    buf.extend_from_slice(format!("library ({name}) {{\n").as_bytes());
    buf.extend_from_slice(body);
    buf.extend_from_slice(b"\n}\n");
    let reader: Box<dyn Read> = Box::new(io::Cursor::new(buf));
    Parser::new(reader, None, false)?.parse_document()
}

/// Cell-offset index over an in-memory Liberty file: list cells cheaply, parse
/// one cell at a time. Built for multi-GB files where a full parse is too slow.
#[pyclass]
struct LibraryIndex {
    data: Vec<u8>,
    #[pyo3(get)]
    library_name: String,
    order: Vec<String>,
    ranges: HashMap<String, (usize, usize)>,
    header: Document,
}

#[pymethods]
impl LibraryIndex {
    #[staticmethod]
    fn open(py: Python<'_>, path: &str) -> PyResult<LibraryIndex> {
        let path = path.to_string();
        py.detach(move || open_library_index(&path))
            .map_err(ParseLoadError::into_py)
    }

    fn cell_names(&self) -> Vec<String> {
        self.order.clone()
    }

    fn num_cells(&self) -> usize {
        self.order.len()
    }

    fn cell(&self, py: Python<'_>, name: &str) -> PyResult<Cell> {
        let name = name.to_string();
        py.detach(move || self.parse_cell_by_name(&name))
            .map_err(CellLookupError::into_py)
    }

    /// Raw Liberty source text of one cell, sliced from the in-memory buffer.
    /// O(cell size) — independent of the total file size.
    fn cell_source(&self, py: Python<'_>, name: &str) -> PyResult<String> {
        let name = name.to_string();
        py.detach(move || self.cell_source_by_name(&name))
            .map_err(CellSourceError::into_py)
    }

    // -- library header pass-throughs (parsed once at open) --
    #[getter]
    fn voltage_unit(&self) -> Option<String> {
        self.header.voltage_unit.clone()
    }
    #[getter]
    fn current_unit(&self) -> Option<String> {
        self.header.current_unit.clone()
    }
    #[getter]
    fn time_unit(&self) -> Option<String> {
        self.header.time_unit.clone()
    }
    #[getter]
    fn capacitive_load_unit(&self) -> Option<String> {
        self.header.capacitive_load_unit.clone()
    }
    fn energy_unit_joules(&self) -> Option<f64> {
        self.header.energy_unit_joules()
    }
    fn templates<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.header.templates(py)
    }
    fn attributes(&self) -> Vec<(String, String)> {
        self.header.attributes()
    }
    fn driver_waveforms(&self) -> Vec<TimingTable> {
        self.header.driver_waveforms()
    }
}

impl LibraryIndex {
    fn parse_cell_by_name(&self, name: &str) -> Result<Cell, CellLookupError> {
        let &(start, end) = self
            .ranges
            .get(name)
            .ok_or_else(|| CellLookupError::Unknown(name.to_string()))?;
        let mut doc = parse_slice_as_library(&self.library_name, &self.data[start..end])
            .map_err(CellLookupError::Parse)?;
        // The cell slice is parsed on its own, so the library header's units
        // and templates have to be applied to it here.
        if let Some(cell) = doc.cells.first_mut() {
            apply_units(std::slice::from_mut(cell), self.header.units);
            resolve_axis_vars(std::slice::from_mut(cell), &self.header.templates);
        }
        doc.cells
            .into_iter()
            .next()
            .map(Cell::from)
            .ok_or_else(|| CellLookupError::Empty(name.to_string()))
    }

    fn cell_source_by_name(&self, name: &str) -> Result<String, CellSourceError> {
        let &(start, end) = self
            .ranges
            .get(name)
            .ok_or_else(|| CellSourceError::Unknown(name.to_string()))?;
        Ok(String::from_utf8_lossy(&self.data[start..end]).into_owned())
    }
}

fn open_library_index(path: &str) -> Result<LibraryIndex, ParseLoadError> {
    let data = read_to_memory(path).map_err(ParseLoadError::Io)?;
    let scan = index_cells(&data);
    // The library header is the preamble *and* the top-level items that
    // follow the first cell: a library may declare its units or its
    // lookup-table templates after the cells that use them, and the full
    // parser resolves them there too.
    let mut header_bytes = Vec::new();
    header_bytes.extend_from_slice(&data[scan.preamble.0..scan.preamble.1]);
    for (start, end) in &scan.late_items {
        if !header_bytes.ends_with(b"\n") {
            header_bytes.push(b'\n');
        }
        header_bytes.extend_from_slice(&data[*start..*end]);
    }
    let header =
        parse_slice_as_library(&scan.name, &header_bytes).map_err(ParseLoadError::Parse)?;
    let mut order = Vec::with_capacity(scan.cells.len());
    let mut ranges = HashMap::with_capacity(scan.cells.len());
    for (name, start, end) in scan.cells {
        if !ranges.contains_key(&name) {
            order.push(name.clone());
        }
        ranges.insert(name, (start, end));
    }
    Ok(LibraryIndex {
        data,
        library_name: scan.name,
        order,
        ranges,
        header,
    })
}

#[derive(Debug, Clone)]
enum TokenKind {
    Word(String),
    Symbol(u8),
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    line: usize,
    column: usize,
    /// Byte offset of the first byte of the token in the (decompressed) source.
    start: usize,
    /// Byte offset just past the last byte of the token.
    end: usize,
}

struct Lexer {
    reader: Box<dyn Read>,
    peeked: VecDeque<u8>,
    line: usize,
    column: usize,
    eof: bool,
    /// Byte offset of the next byte to consume (source position).
    offset: usize,
    /// FNV-1a hash over every byte consumed so far. Used to detect that the
    /// source file changed underneath a parsed document before writing it back.
    hash: u64,
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

impl Lexer {
    fn new(reader: Box<dyn Read>) -> Result<Self, ParseError> {
        Ok(Self {
            reader,
            peeked: VecDeque::new(),
            line: 1,
            column: 1,
            eof: false,
            offset: 0,
            hash: FNV_OFFSET,
        })
    }

    fn next_token(&mut self) -> Result<Option<Token>, ParseError> {
        self.skip_ws_and_comments()?;
        if self.peek()?.is_none() {
            return Ok(None);
        }

        let line = self.line;
        let column = self.column;
        let start = self.offset;
        let byte = self.bump()?.unwrap();
        if is_symbol(byte) {
            return Ok(Some(Token {
                kind: TokenKind::Symbol(byte),
                line,
                column,
                start,
                end: self.offset,
            }));
        }
        if byte == b'"' || byte == b'\'' {
            let value = self.read_quoted(byte, line, column)?;
            return Ok(Some(Token {
                kind: TokenKind::Word(value),
                line,
                column,
                start,
                end: self.offset,
            }));
        }

        let mut value = Vec::new();
        value.push(byte);
        while let Some(next) = self.peek()? {
            if next.is_ascii_whitespace() || is_symbol(next) {
                break;
            }
            if next == b'/' && matches!(self.peek_next()?, Some(b'*' | b'/')) {
                break;
            }
            if next == b'\\' {
                self.bump()?;
                match self.peek()? {
                    Some(b'\n') => {
                        self.bump()?;
                    }
                    Some(b'\r') => {
                        self.bump()?;
                        if self.peek()? == Some(b'\n') {
                            self.bump()?;
                        }
                    }
                    Some(escaped) => {
                        value.push(b'\\');
                        value.push(escaped);
                        self.bump()?;
                    }
                    None => value.push(b'\\'),
                }
                continue;
            }
            value.push(self.bump()?.unwrap());
        }

        Ok(Some(Token {
            kind: TokenKind::Word(String::from_utf8_lossy(&value).into_owned()),
            line,
            column,
            start,
            end: self.offset,
        }))
    }

    fn read_quoted(
        &mut self,
        quote: u8,
        start_line: usize,
        start_column: usize,
    ) -> Result<String, ParseError> {
        let mut value = Vec::new();
        let mut escaped = false;
        while let Some(byte) = self.bump()? {
            if escaped {
                match byte {
                    b'\n' => {}
                    b'\r' => {
                        if self.peek()? == Some(b'\n') {
                            self.bump()?;
                        }
                    }
                    _ => value.push(byte),
                }
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                continue;
            }
            if byte == quote {
                return Ok(String::from_utf8_lossy(&value).into_owned());
            }
            value.push(byte);
        }
        Err(ParseError::new(
            start_line,
            start_column,
            "unterminated quoted string",
        ))
    }

    fn skip_ws_and_comments(&mut self) -> Result<(), ParseError> {
        loop {
            loop {
                if matches!(self.peek()?, Some(byte) if byte.is_ascii_whitespace()) {
                    self.bump()?;
                } else if self.peek()? == Some(b'\\')
                    && matches!(self.peek_next()?, Some(b'\n' | b'\r'))
                {
                    self.bump()?;
                    if self.peek()? == Some(b'\r') {
                        self.bump()?;
                        if self.peek()? == Some(b'\n') {
                            self.bump()?;
                        }
                    } else {
                        self.bump()?;
                    }
                } else {
                    break;
                }
            }
            match (self.peek()?, self.peek_next()?) {
                (Some(b'#'), _) => self.skip_line(),
                (Some(b'/'), Some(b'/')) => {
                    self.bump()?;
                    self.bump()?;
                    self.skip_line();
                }
                (Some(b'/'), Some(b'*')) => {
                    let line = self.line;
                    let column = self.column;
                    self.bump()?;
                    self.bump()?;
                    self.skip_block_comment(line, column)?;
                }
                _ => return Ok(()),
            }
        }
    }

    fn skip_line(&mut self) {
        while let Ok(Some(byte)) = self.bump() {
            if byte == b'\n' {
                break;
            }
        }
    }

    fn skip_block_comment(&mut self, line: usize, column: usize) -> Result<(), ParseError> {
        let mut prev = 0;
        while let Some(byte) = self.bump()? {
            if prev == b'*' && byte == b'/' {
                return Ok(());
            }
            prev = byte;
        }
        Err(ParseError::new(line, column, "unterminated block comment"))
    }

    /// Skip bytes until the brace nesting level in `depth` reaches zero.
    ///
    /// Strings and comments are honored so a brace inside `"..."` or a comment
    /// does not change the depth. This is the fast path for table bodies: it
    /// hashes and positions every byte but allocates no tokens or values.
    fn skip_braces(&mut self, mut depth: usize) -> Result<(), ParseError> {
        while depth > 0 {
            let Some(byte) = self.bump()? else {
                return Err(ParseError::new(
                    self.line,
                    self.column,
                    "unexpected end of file while skipping group",
                ));
            };
            match byte {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                b'"' | b'\'' => self.skip_quoted_bytes(byte)?,
                b'#' => self.skip_line(),
                b'/' => match self.peek()? {
                    Some(b'/') => {
                        self.bump()?;
                        self.skip_line();
                    }
                    Some(b'*') => {
                        let line = self.line;
                        let column = self.column;
                        self.bump()?;
                        self.skip_block_comment(line, column)?;
                    }
                    _ => {}
                },
                b'\\' => {
                    if matches!(self.peek()?, Some(b'\n' | b'\r')) {
                        let first = self.bump()?.unwrap_or(b'\n');
                        if first == b'\r' && self.peek()? == Some(b'\n') {
                            self.bump()?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn skip_quoted_bytes(&mut self, quote: u8) -> Result<(), ParseError> {
        let line = self.line;
        let column = self.column;
        let mut escaped = false;
        while let Some(byte) = self.bump()? {
            if escaped {
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                continue;
            }
            if byte == quote {
                return Ok(());
            }
        }
        Err(ParseError::new(line, column, "unterminated quoted string"))
    }

    fn bump(&mut self) -> Result<Option<u8>, ParseError> {
        self.fill_peek(1)?;
        let Some(byte) = self.peeked.pop_front() else {
            return Ok(None);
        };
        if byte == b'\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        self.offset += 1;
        self.hash ^= byte as u64;
        self.hash = self.hash.wrapping_mul(FNV_PRIME);
        Ok(Some(byte))
    }

    fn peek(&mut self) -> Result<Option<u8>, ParseError> {
        self.fill_peek(1)?;
        Ok(self.peeked.front().copied())
    }

    fn peek_next(&mut self) -> Result<Option<u8>, ParseError> {
        self.fill_peek(2)?;
        Ok(self.peeked.get(1).copied())
    }

    fn fill_peek(&mut self, len: usize) -> Result<(), ParseError> {
        while !self.eof && self.peeked.len() < len {
            let mut byte = [0_u8; 1];
            match self.reader.read(&mut byte)? {
                0 => self.eof = true,
                _ => self.peeked.push_back(byte[0]),
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct LibraryUnits {
    voltage: Option<String>,
    current: Option<String>,
    time: Option<String>,
    capacitive_load: Option<String>,
    capacitive_load_multiplier: Option<f64>,
    leakage_power: Option<String>,
    templates: Vec<(String, [Option<String>; 3])>,
    attributes: Vec<(String, String)>,
    operating_conditions: Vec<(String, Vec<(String, String)>)>,
    driver_waveforms: Vec<TimingTableData>,
}

struct Parser {
    lexer: Lexer,
    current: Option<Token>,
    cell_filter: Option<HashSet<String>>,
    /// Skip floating-point table bodies (`cell_rise`, power, CCSN) without
    /// materializing them. Structural metadata (pins, arcs, scalars) is kept.
    skip_tables: bool,
    /// End offset of the most recently consumed token. `advance()` overwrites
    /// it, so it is only meaningful right after a token has been consumed.
    previous_end: usize,
    /// Byte span (start, end) of every parsed cell group, in source order.
    cell_spans: Vec<(String, usize, usize)>,
}

impl Parser {
    fn new(
        reader: Box<dyn Read>,
        cell_filter: Option<HashSet<String>>,
        skip_tables: bool,
    ) -> Result<Self, ParseError> {
        let mut parser = Self {
            lexer: Lexer::new(reader)?,
            current: None,
            cell_filter,
            skip_tables,
            previous_end: 0,
            cell_spans: Vec::new(),
        };
        parser.advance()?;
        Ok(parser)
    }

    fn parse_document(&mut self) -> Result<Document, ParseError> {
        let mut library_name = String::new();
        let mut cells = Vec::new();
        let mut bus_types = Vec::new();
        let mut units = LibraryUnits::default();

        while self.current.is_some() {
            // A '}' here has no matching open group at top level: the file has
            // unbalanced braces (e.g. the ASAP7 SIMPLE group ships a spurious
            // '}' that closes `library` early). Report it instead of failing
            // later with a cryptic "expected identifier" at the next token.
            if let Some(Token {
                kind: TokenKind::Symbol(b'}'),
                line,
                column,
                ..
            }) = self.current
            {
                return Err(ParseError::new(
                    line,
                    column,
                    "unexpected '}': unbalanced braces (a group has an extra \
                     closing brace, or an opening brace is missing earlier)",
                ));
            }
            let name_pos = self.current_position();
            let name = self.take_word()?;
            let args = if self.consume_symbol(b'(')? {
                self.read_args(&name)?
            } else {
                Vec::new()
            };
            self.expect_symbol(b'{')?;
            if name == "library" {
                library_name = args.first().cloned().unwrap_or_default();
                self.parse_library_body(&mut cells, &mut bus_types, &mut units)?;
            } else {
                // The only valid top-level group is `library`. Anything else
                // here (commonly a `cell` left orphaned by a premature library
                // close) signals unbalanced braces.
                let (line, column) = name_pos;
                return Err(ParseError::new(
                    line,
                    column,
                    &format!(
                        "unexpected top-level {name:?} group: expected a single \
                         'library' group (unbalanced braces may have closed it early)"
                    ),
                ));
            }
            self.consume_symbol(b';')?;
        }

        if library_name.is_empty() {
            return Err(self.error_here("missing top-level library group"));
        }

        // The library header is only fully known once the body has been read
        // (units and templates may be declared after the cells that use them),
        // so resolve them now.
        let unit_context = UnitContext::from_library(
            units.voltage.as_deref(),
            units.current.as_deref(),
            units.time.as_deref(),
            units.capacitive_load_multiplier,
            units.capacitive_load.as_deref(),
            units.leakage_power.as_deref(),
        );
        apply_units(&mut cells, unit_context);
        resolve_axis_vars(&mut cells, &units.templates);
        resolve_axis_vars_in(&mut units.driver_waveforms, &units.templates);

        Ok(Document {
            library_name,
            voltage_unit: units.voltage,
            current_unit: units.current,
            time_unit: units.time,
            capacitive_load_unit: units.capacitive_load,
            templates: units.templates,
            attributes: units.attributes,
            operating_conditions: units.operating_conditions,
            capacitive_load_multiplier: units.capacitive_load_multiplier,
            leakage_power_unit: units.leakage_power,
            driver_waveforms: units.driver_waveforms,
            cells,
            bus_types,
            source_path: None,
            source_len: self.lexer.offset,
            source_hash: self.lexer.hash,
            cell_spans: std::mem::take(&mut self.cell_spans),
            edits: Arc::new(Mutex::new(Vec::new())),
            units: unit_context,
        })
    }

    fn parse_library_body(
        &mut self,
        cells: &mut Vec<CellData>,
        bus_types: &mut Vec<BusTypeData>,
        units: &mut LibraryUnits,
    ) -> Result<(), ParseError> {
        while !self.consume_symbol(b'}')? {
            let item_start = self.current_offset();
            let name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&name)?;
                if self.consume_symbol(b'{')? {
                    match name.as_str() {
                        "cell" => {
                            let cell_name = args.first().cloned().unwrap_or_default();
                            if self
                                .cell_filter
                                .as_ref()
                                .map_or(true, |filter| filter.contains(&cell_name))
                            {
                                cells.push(self.parse_cell_body(cell_name.clone())?);
                                // `previous_end` is the end of the cell's closing
                                // brace: the group text is [item_start, end).
                                self.cell_spans
                                    .push((cell_name, item_start, self.previous_end));
                            } else {
                                self.skip_group_body()?;
                            }
                        }
                        "type" => {
                            bus_types.push(
                                self.parse_bus_type_body(
                                    args.first().cloned().unwrap_or_default(),
                                )?,
                            );
                        }
                        "operating_conditions" => {
                            let oc_name = args.first().cloned().unwrap_or_default();
                            let attrs = self.parse_operating_conditions_body()?;
                            units.operating_conditions.push((oc_name, attrs));
                        }
                        "lu_table_template" | "power_lut_template" | "output_current_template" => {
                            let tmpl_name = args.first().cloned().unwrap_or_default();
                            let vars = self.parse_template_body()?;
                            units.templates.push((tmpl_name, vars));
                        }
                        "normalized_driver_waveform" => {
                            let tmpl = args.first().cloned();
                            if self.skip_tables {
                                self.skip_group_body_fast()?;
                            } else {
                                units
                                    .driver_waveforms
                                    .push(self.parse_timing_table_body(String::new(), tmpl)?);
                            }
                        }
                        _ => {
                            self.skip_group_body()?;
                        }
                    }
                    self.consume_symbol(b';')?;
                } else {
                    // Complex attribute, e.g. `capacitive_load_unit (1, ff)`.
                    if name == "capacitive_load_unit" {
                        units.capacitive_load = args.get(1).cloned();
                        units.capacitive_load_multiplier =
                            args.first().and_then(|value| parse_number(value));
                    }
                    units.attributes.push((name, args.join(", ")));
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match name.as_str() {
                    "voltage_unit" => units.voltage = Some(value.clone()),
                    "current_unit" => units.current = Some(value.clone()),
                    "time_unit" => units.time = Some(value.clone()),
                    "leakage_power_unit" => units.leakage_power = Some(value.clone()),
                    _ => {}
                }
                units.attributes.push((name, value));
            } else {
                return Err(self.error_here("expected '(' or ':' after library item name"));
            }
        }
        Ok(())
    }

    fn parse_cell_body(&mut self, name: String) -> Result<CellData, ParseError> {
        let mut area = None;
        let mut pins = Vec::new();
        let mut buses = Vec::new();
        let mut bundles = Vec::new();
        let mut attributes = Vec::new();
        let mut dynamic_currents = Vec::new();
        let mut ff = None;
        let mut latch = None;
        let mut leakage_powers = Vec::new();
        let mut pg_pins = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    match item_name.as_str() {
                        "pin" => {
                            pins.push(
                                self.parse_pin_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        "bus" => {
                            buses.push(
                                self.parse_bus_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        "bundle" => {
                            bundles.push(
                                self.parse_bundle_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        "dynamic_current" => {
                            dynamic_currents.push(self.parse_dynamic_current_body()?);
                        }
                        // `ff_bank`/`latch_bank` reuse the same parser; the extra
                        // bit-width arg is ignored. `statetable` is a tabular
                        // state-transition group, not a boolean expression, so
                        // it stays skipped.
                        "ff" | "ff_bank" => {
                            ff = Some(self.parse_ff_body(&args)?);
                        }
                        "latch" | "latch_bank" => {
                            latch = Some(self.parse_latch_body(&args)?);
                        }
                        "leakage_power" => {
                            leakage_powers.push(self.parse_leakage_power_body()?);
                        }
                        "pg_pin" => {
                            pg_pins.push(
                                self.parse_pg_pin_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        _ => {
                            self.skip_group_body()?;
                        }
                    }
                    self.consume_symbol(b';')?;
                } else {
                    attributes.push((item_name, args.join(", ")));
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                if item_name == "area" {
                    area = parse_number(&value);
                }
                attributes.push((item_name, value));
            } else {
                return Err(self.error_here("expected '(' or ':' after cell item name"));
            }
        }
        Ok(CellData {
            name,
            area,
            pins,
            buses,
            bundles,
            attributes,
            dynamic_currents,
            ff,
            latch,
            leakage_powers,
            pg_pins,
            units: UnitContext::default(),
        })
    }

    /// Parse a `pg_pin` body: capture its simple `name : value` attributes;
    /// nested groups are skipped.
    fn parse_pg_pin_body(&mut self, name: String) -> Result<PgPinData, ParseError> {
        let mut attributes = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                attributes.push((item_name, value));
            } else {
                return Err(self.error_here("expected '(' or ':' in pg_pin"));
            }
        }
        Ok(PgPinData { name, attributes })
    }

    /// Parse a `leakage_power` body: a `value` plus optional `when` /
    /// `related_pg_pin`. Any nested groups are skipped.
    fn parse_leakage_power_body(&mut self) -> Result<LeakagePowerData, ParseError> {
        let mut leak = LeakagePowerData::default();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "value" => leak.value = parse_number(&value),
                    "when" => leak.when = Some(value),
                    "related_pg_pin" => leak.related_pg_pin = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' in leakage_power"));
            }
        }
        Ok(leak)
    }

    /// Parse an `ff` / `ff_bank` body. `args` are the header arguments; the
    /// first two are the state-variable names.
    fn parse_ff_body(&mut self, args: &[String]) -> Result<FfData, ParseError> {
        let mut ff = FfData {
            variable1: args.first().cloned(),
            variable2: args.get(1).cloned(),
            ..FfData::default()
        };
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "next_state" => ff.next_state = Some(value),
                    "clocked_on" => ff.clocked_on = Some(value),
                    "clocked_on_also" => ff.clocked_on_also = Some(value),
                    "clear" => ff.clear = Some(value),
                    "preset" => ff.preset = Some(value),
                    "power_down_function" => ff.power_down_function = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' in ff group"));
            }
        }
        Ok(ff)
    }

    /// Parse a `latch` / `latch_bank` body.
    fn parse_latch_body(&mut self, args: &[String]) -> Result<LatchData, ParseError> {
        let mut latch = LatchData {
            variable1: args.first().cloned(),
            variable2: args.get(1).cloned(),
            ..LatchData::default()
        };
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "enable" => latch.enable = Some(value),
                    "data_in" => latch.data_in = Some(value),
                    "clear" => latch.clear = Some(value),
                    "preset" => latch.preset = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' in latch group"));
            }
        }
        Ok(latch)
    }

    fn parse_dynamic_current_body(&mut self) -> Result<DynamicCurrentData, ParseError> {
        let mut related_inputs = None;
        let mut related_outputs = None;
        let mut when = None;
        let mut switching_groups = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    if item_name == "switching_group" {
                        switching_groups.push(self.parse_switching_group_body()?);
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    // Complex attr (e.g. typical_capacitances) — ignored.
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "related_inputs" => related_inputs = Some(value),
                    "related_outputs" => related_outputs = Some(value),
                    "when" => when = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' in dynamic_current"));
            }
        }
        Ok(DynamicCurrentData {
            related_inputs,
            related_outputs,
            when,
            switching_groups,
        })
    }

    fn parse_switching_group_body(&mut self) -> Result<SwitchingGroupData, ParseError> {
        let mut input_switching_condition = None;
        let mut output_switching_condition = None;
        let mut pg_currents = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    if item_name == "pg_current" {
                        pg_currents.push(self.parse_pg_current_body(args.first().cloned())?);
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    // `input_switching_condition (rise)` etc. are complex attrs.
                    match item_name.as_str() {
                        "input_switching_condition" => {
                            input_switching_condition = args.first().cloned();
                        }
                        "output_switching_condition" => {
                            output_switching_condition = args.first().cloned();
                        }
                        _ => {}
                    }
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let _ = self.read_simple_attribute_value()?;
            } else {
                return Err(self.error_here("expected '(' or ':' in switching_group"));
            }
        }
        Ok(SwitchingGroupData {
            input_switching_condition,
            output_switching_condition,
            pg_currents,
        })
    }

    fn parse_pg_current_body(
        &mut self,
        pg_pin: Option<String>,
    ) -> Result<PgCurrentData, ParseError> {
        let mut vectors = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    if item_name == "vector" {
                        let tmpl = args.first().cloned();
                        vectors.push(self.parse_timing_table_body("vector".to_string(), tmpl)?);
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let _ = self.read_simple_attribute_value()?;
            } else {
                return Err(self.error_here("expected '(' or ':' in pg_current"));
            }
        }
        Ok(PgCurrentData { pg_pin, vectors })
    }

    fn parse_pin_body(&mut self, name: String) -> Result<PinData, ParseError> {
        let mut direction = None;
        let mut function = None;
        let mut three_state = None;
        let mut power_down_function = None;
        let mut timing_arcs = Vec::new();
        let mut internal_powers = Vec::new();
        let mut attributes = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    match item_name.as_str() {
                        "timing" => timing_arcs.push(self.parse_timing_body()?),
                        "internal_power" => internal_powers.push(self.parse_internal_power_body()?),
                        _ => self.skip_group_body()?,
                    }
                    self.consume_symbol(b';')?;
                } else {
                    // Complex attribute, e.g. `rise_capacitance_range (0.55, 0.74)`.
                    attributes.push((item_name, args.join(", ")));
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "direction" => direction = Some(value.clone()),
                    "function" => function = Some(value.clone()),
                    "three_state" => three_state = Some(value.clone()),
                    "power_down_function" => power_down_function = Some(value.clone()),
                    _ => {}
                }
                attributes.push((item_name, value));
            } else {
                return Err(self.error_here("expected '(' or ':' after pin item name"));
            }
        }
        Ok(PinData {
            name,
            direction,
            function,
            three_state,
            power_down_function,
            timing_arcs,
            internal_powers,
            attributes,
            units: UnitContext::default(),
        })
    }

    fn parse_internal_power_body(&mut self) -> Result<InternalPowerData, ParseError> {
        let mut related_pin = None;
        let mut related_pg_pin = None;
        let mut when = None;
        let mut tables = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    if is_power_table_group(&item_name) {
                        if self.skip_tables {
                            self.skip_group_body_fast()?;
                        } else {
                            let template = args.first().cloned();
                            tables.push(self.parse_timing_table_body(item_name, template)?);
                        }
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "related_pin" => related_pin = Some(value),
                    "related_pg_pin" => related_pg_pin = Some(value),
                    "when" => when = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after internal_power item name"));
            }
        }
        Ok(InternalPowerData {
            related_pin,
            related_pg_pin,
            when,
            tables,
        })
    }

    fn parse_bus_body(&mut self, name: String) -> Result<BusData, ParseError> {
        let mut direction = None;
        let mut function = None;
        let mut bus_type = None;
        let mut pins = Vec::new();
        let mut timing_arcs = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    match item_name.as_str() {
                        "pin" => {
                            pins.push(
                                self.parse_pin_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        "timing" => {
                            timing_arcs.push(self.parse_timing_body()?);
                        }
                        _ => {
                            self.skip_group_body()?;
                        }
                    }
                    self.consume_symbol(b';')?;
                } else {
                    if item_name == "members" {
                        pins.extend(members_from_args(&args).into_iter().map(|name| PinData {
                            name,
                            direction: direction.clone(),
                            function: None,
                            three_state: None,
                            power_down_function: None,
                            timing_arcs: Vec::new(),
                            internal_powers: Vec::new(),
                            attributes: Vec::new(),
                            units: UnitContext::default(),
                        }));
                    }
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "direction" => direction = Some(value),
                    "function" => function = Some(value),
                    "bus_type" => bus_type = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after bus item name"));
            }
        }
        Ok(BusData {
            name,
            direction,
            function,
            bus_type,
            pins,
            timing_arcs,
        })
    }

    fn parse_bundle_body(&mut self, name: String) -> Result<BundleData, ParseError> {
        let mut direction = None;
        let mut function = None;
        let mut members = Vec::new();
        let mut pins = Vec::new();
        let mut timing_arcs = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    match item_name.as_str() {
                        "pin" => {
                            pins.push(
                                self.parse_pin_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        "timing" => {
                            timing_arcs.push(self.parse_timing_body()?);
                        }
                        _ => {
                            self.skip_group_body()?;
                        }
                    }
                    self.consume_symbol(b';')?;
                } else {
                    if item_name == "members" {
                        members = members_from_args(&args);
                    }
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "direction" => direction = Some(value),
                    "function" => function = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after bundle item name"));
            }
        }
        Ok(BundleData {
            name,
            direction,
            function,
            members,
            pins,
            timing_arcs,
        })
    }

    fn parse_bus_type_body(&mut self, name: String) -> Result<BusTypeData, ParseError> {
        let mut attributes = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b':')? {
                attributes.push((item_name, self.read_simple_attribute_value()?));
            } else if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                } else {
                    self.consume_symbol(b';')?;
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after type item name"));
            }
        }
        Ok(BusTypeData { name, attributes })
    }

    /// Parse an `operating_conditions` body: its simple/complex attributes
    /// (`nom_voltage`, `nom_temperature`, `process`, …). Nested groups are
    /// skipped.
    fn parse_operating_conditions_body(&mut self) -> Result<Vec<(String, String)>, ParseError> {
        let mut attributes = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                attributes.push((item_name, args.join(", ")));
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                attributes.push((item_name, value));
            } else {
                return Err(self.error_here("expected '(' or ':' in operating_conditions"));
            }
        }
        Ok(attributes)
    }

    /// Read `variable_1/2/3` from a lookup-table template group, skipping the
    /// index rows. Returns the three axis variable names (any may be None).
    fn parse_template_body(&mut self) -> Result<[Option<String>; 3], ParseError> {
        let mut vars: [Option<String>; 3] = [None, None, None];
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "variable_1" => vars[0] = Some(value),
                    "variable_2" => vars[1] = Some(value),
                    "variable_3" => vars[2] = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' in template body"));
            }
        }
        Ok(vars)
    }

    fn parse_timing_body(&mut self) -> Result<TimingArcData, ParseError> {
        let mut related_pin = None;
        let mut timing_type = None;
        let mut when = None;
        let mut sdf_cond = None;
        let mut timing_sense = None;
        let mut tables = Vec::new();
        let mut ccsn_stages = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    if is_ccsn_stage_group(&item_name) {
                        ccsn_stages.push(self.parse_ccsn_stage_body(item_name)?);
                    } else if is_timing_table_group(&item_name) {
                        if self.skip_tables {
                            self.skip_group_body_fast()?;
                        } else {
                            let template = args.first().cloned();
                            tables.push(self.parse_timing_table_body(item_name, template)?);
                        }
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "related_pin" => related_pin = Some(value),
                    "timing_type" => timing_type = Some(value),
                    "when" => when = Some(value),
                    "sdf_cond" => sdf_cond = Some(value),
                    "timing_sense" => timing_sense = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after timing item name"));
            }
        }
        Ok(TimingArcData {
            related_pin,
            timing_type,
            when,
            sdf_cond,
            timing_sense,
            tables,
            ccsn_stages,
        })
    }

    fn parse_ccsn_stage_body(&mut self, name: String) -> Result<CcsnStageData, ParseError> {
        let mut is_inverting = None;
        let mut is_needed = None;
        let mut is_pass_gate = None;
        let mut stage_type = None;
        let mut miller_cap_rise = None;
        let mut miller_cap_fall = None;
        let mut when = None;
        let mut mode = None;
        let mut dc_current = None;
        let mut output_voltage_rise = Vec::new();
        let mut output_voltage_fall = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    match item_name.as_str() {
                        "dc_current" => {
                            if self.skip_tables {
                                self.skip_group_body_fast()?;
                            } else {
                                let template = args.first().cloned();
                                dc_current =
                                    Some(self.parse_timing_table_body(item_name, template)?);
                            }
                        }
                        "output_voltage_rise" => {
                            if self.skip_tables {
                                self.skip_group_body_fast()?;
                            } else {
                                let table = self.parse_timing_table_body(item_name, None)?;
                                output_voltage_rise = table.vectors;
                            }
                        }
                        "output_voltage_fall" => {
                            if self.skip_tables {
                                self.skip_group_body_fast()?;
                            } else {
                                let table = self.parse_timing_table_body(item_name, None)?;
                                output_voltage_fall = table.vectors;
                            }
                        }
                        "propagated_noise_high" | "propagated_noise_low" => {
                            // TODO: deferred 4D propagated-noise parsing.
                            self.skip_group_body()?;
                        }
                        _ => self.skip_group_body()?,
                    }
                    self.consume_symbol(b';')?;
                } else {
                    self.consume_symbol(b';')?;
                    if item_name == "mode" {
                        mode = Some(args.join(", "));
                    }
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "is_inverting" => is_inverting = Some(value),
                    "is_needed" => is_needed = Some(value),
                    "is_pass_gate" => is_pass_gate = Some(value),
                    "stage_type" => stage_type = Some(value),
                    "miller_cap_rise" => miller_cap_rise = parse_number(&value),
                    "miller_cap_fall" => miller_cap_fall = parse_number(&value),
                    "when" => when = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after ccsn stage item name"));
            }
        }
        Ok(CcsnStageData {
            name,
            is_inverting,
            is_needed,
            is_pass_gate,
            stage_type,
            miller_cap_rise,
            miller_cap_fall,
            when,
            mode,
            dc_current,
            output_voltage_rise,
            output_voltage_fall,
        })
    }

    fn parse_timing_table_body(
        &mut self,
        mut name: String,
        template: Option<String>,
    ) -> Result<TimingTableData, ParseError> {
        let mut index_1 = Vec::new();
        let mut index_2 = Vec::new();
        let mut index_3 = Vec::new();
        let mut values = Vec::new();
        let mut reference_time = None;
        let mut vectors = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    // A nested group: CCS `output_current_*` / `ccsn_*` tables
                    // contain `vector` sub-groups, each a current-vs-time wave.
                    if item_name == "vector" {
                        let vtemplate = args.first().cloned();
                        vectors.push(self.parse_timing_table_body(item_name, vtemplate)?);
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    self.consume_symbol(b';')?;
                    match item_name.as_str() {
                        "index_1" => index_1 = parse_float_list(args.first()),
                        "index_2" => index_2 = parse_float_list(args.first()),
                        "index_3" => index_3 = parse_float_list(args.first()),
                        "values" => values = parse_float_args(&args),
                        _ => {}
                    }
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                if item_name == "reference_time" {
                    reference_time = parse_number(&value);
                } else if item_name == "driver_waveform_name" {
                    // normalized_driver_waveform groups name themselves here, not
                    // in the group header (which holds the template name).
                    name = value;
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after table item name"));
            }
        }
        Ok(TimingTableData {
            name,
            index_1,
            index_2,
            index_3,
            values,
            template,
            axis_vars: [None, None, None],
            reference_time,
            vectors,
        })
    }

    fn skip_group_body(&mut self) -> Result<(), ParseError> {
        let mut depth = 1usize;
        while let Some(token) = self.current.clone() {
            self.advance()?;
            match token.kind {
                TokenKind::Symbol(b'{') => depth += 1,
                TokenKind::Symbol(b'}') => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err(self.error_here("unexpected end of file while skipping group"))
    }

    /// Skip a group at the byte level instead of tokenizing its body.
    ///
    /// The parser has already consumed the opening `{`, so `current` is the
    /// first body token. It accounts for that token, then lets the lexer skip
    /// the remaining bytes while handling strings, comments and escapes.
    fn skip_group_body_fast(&mut self) -> Result<(), ParseError> {
        let Some(token) = self.current.take() else {
            return Err(self.error_here("unexpected end of file while skipping group"));
        };
        let mut depth = 1usize;
        match token.kind {
            TokenKind::Symbol(b'{') => depth += 1,
            TokenKind::Symbol(b'}') => depth -= 1,
            _ => {}
        }
        if depth > 0 {
            self.lexer.skip_braces(depth)?;
        }
        self.current = self.lexer.next_token()?;
        Ok(())
    }

    fn read_args(&mut self, name: &str) -> Result<Vec<String>, ParseError> {
        let mut args = Vec::new();
        let allow_spaces = matches!(
            name,
            "input_switching_condition" | "output_switching_condition"
        );
        while !self.consume_symbol(b')')? {
            args.push(self.read_value()?);
            if self.consume_symbol(b')')? {
                break;
            }
            if !allow_spaces {
                self.expect_symbol(b',')?;
            } else {
                self.consume_symbol(b',')?;
            }
        }
        Ok(args)
    }

    fn read_value(&mut self) -> Result<String, ParseError> {
        let base = self.take_word()?;
        let index = if self.consume_symbol(b'[')? {
            let sel = self.take_word()?;
            self.expect_symbol(b']')?;
            Some(sel)
        } else {
            None
        };
        Ok(indexed_value(&base, index.as_deref()))
    }

    fn read_simple_attribute_value(&mut self) -> Result<String, ParseError> {
        let mut parts = Vec::new();
        while !self.consume_symbol(b';')? {
            if self.current.is_none() {
                return Err(self.error_here("unexpected end of file in attribute value"));
            }
            parts.push(self.token_to_string()?);
            self.advance()?;
        }
        Ok(parts.join(" "))
    }

    fn take_word(&mut self) -> Result<String, ParseError> {
        match self.current.clone() {
            Some(Token {
                kind: TokenKind::Word(value),
                ..
            }) => {
                self.advance()?;
                Ok(value)
            }
            Some(token) => Err(ParseError::new(
                token.line,
                token.column,
                "expected identifier or value",
            )),
            None => Err(self.error_here("unexpected end of file")),
        }
    }

    fn token_to_string(&self) -> Result<String, ParseError> {
        match &self.current {
            Some(Token {
                kind: TokenKind::Word(value),
                ..
            }) => Ok(value.clone()),
            Some(Token {
                kind: TokenKind::Symbol(byte),
                ..
            }) => Ok((*byte as char).to_string()),
            None => Err(self.error_here("unexpected end of file")),
        }
    }

    fn consume_symbol(&mut self, symbol: u8) -> Result<bool, ParseError> {
        if matches!(
            self.current,
            Some(Token {
                kind: TokenKind::Symbol(current),
                ..
            }) if current == symbol
        ) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect_symbol(&mut self, symbol: u8) -> Result<(), ParseError> {
        if self.consume_symbol(symbol)? {
            Ok(())
        } else {
            Err(self.error_here(&format!("expected '{}'", symbol as char)))
        }
    }

    fn advance(&mut self) -> Result<(), ParseError> {
        if let Some(token) = &self.current {
            self.previous_end = token.end;
        }
        self.current = self.lexer.next_token()?;
        Ok(())
    }

    /// Byte offset of the first byte of the current token.
    fn current_offset(&self) -> usize {
        match &self.current {
            Some(token) => token.start,
            None => self.lexer.offset,
        }
    }

    fn current_position(&self) -> (usize, usize) {
        match &self.current {
            Some(token) => (token.line, token.column),
            None => (self.lexer.line, self.lexer.column),
        }
    }

    fn error_here(&self, message: &str) -> ParseError {
        let (line, column) = self.current_position();
        ParseError::new(line, column, message)
    }
}

fn is_symbol(byte: u8) -> bool {
    matches!(
        byte,
        b',' | b'{' | b'}' | b'(' | b')' | b'[' | b']' | b';' | b':'
    )
}

/// Reassemble a value that carries an index suffix. The lexer splits
/// `A[0]` into the words `A` and `0` and the symbols `[` and `]`; the value
/// is put back together the same way everywhere.
fn indexed_value(base: &str, index: Option<&str>) -> String {
    match index {
        Some(index) => format!("{base}[{index}]"),
        None => base.to_string(),
    }
}

fn is_timing_table_group(name: &str) -> bool {
    matches!(
        name,
        "cell_rise"
            | "cell_fall"
            | "rise_transition"
            | "fall_transition"
            | "rise_constraint"
            | "fall_constraint"
            | "cell_degradation"
            | "retaining_rise"
            | "retaining_fall"
            | "retain_rise_slew"
            | "retain_fall_slew"
            | "output_current_rise"
            | "output_current_fall"
            | "ccst_rise"
            | "ccst_fall"
            | "ccsp_rise"
            | "ccsp_fall"
            | "receiver_capacitance1_rise"
            | "receiver_capacitance1_fall"
            | "receiver_capacitance2_rise"
            | "receiver_capacitance2_fall"
    )
}

fn is_ccsn_stage_group(name: &str) -> bool {
    matches!(
        name,
        "ccsn_first_stage"
            | "ccsn_last_stage"
            | "ccsn_rise_first_stage"
            | "ccsn_rise_last_stage"
            | "ccsn_fall_first_stage"
            | "ccsn_fall_last_stage"
    )
}

fn is_power_table_group(name: &str) -> bool {
    matches!(name, "rise_power" | "fall_power" | "power")
}

/// Parse a Liberty unit string (e.g. "1mA", "1ps", "1V") into its SI scale.
/// Returns magnitude * SI-prefix factor; "1mA" -> 1e-3, "1ps" -> 1e-12.
fn unit_scale(value: &str) -> Option<f64> {
    let value = value.trim();
    let split = value.find(|ch: char| ch.is_ascii_alphabetic())?;
    let (number, unit) = value.split_at(split);
    let magnitude: f64 = number.trim().parse().unwrap_or(1.0);
    // A multi-char unit (e.g. "mA", "ps") carries an SI prefix; "V"/"A"/"s" do not.
    let prefix_factor = if unit.len() >= 2 {
        match unit.chars().next()? {
            'f' => 1e-15,
            'p' => 1e-12,
            'n' => 1e-9,
            'u' => 1e-6,
            'm' => 1e-3,
            'k' => 1e3,
            'M' => 1e6,
            'G' => 1e9,
            _ => 1.0,
        }
    } else {
        1.0
    };
    Some(magnitude * prefix_factor)
}

fn parse_number(value: &str) -> Option<f64> {
    value.parse::<f64>().ok()
}

/// Position of a coordinate on an axis, allowing for floating point noise.
/// Returns None when the coordinate is not on the axis.
fn find_axis_position(values: &[f64], target: f64) -> Option<usize> {
    values.iter().position(|value| {
        let tolerance = 1e-12 + 1e-9 * value.abs().max(target.abs());
        (value - target).abs() <= tolerance
    })
}

/// Flat position of a grid point in the flat `values` list: `index_3` varies
/// fastest, then `index_2`, then `index_1`.
fn flat_index(index_1: usize, index_2: usize, index_3: usize, len_2: usize, len_3: usize) -> usize {
    let len_2 = len_2.max(1);
    let len_3 = len_3.max(1);
    index_1 * len_2 * len_3 + index_2 * len_3 + index_3
}

/// The axis-variable names a keyword belongs to. Two names in the same group
/// mean the same physical quantity, so a load axis named
/// `output_capacitance` matches one named `total_output_net_capacitance`.
fn role_synonyms(name: &str) -> &'static [&'static str] {
    match name {
        "load"
        | "cap"
        | "capacitance"
        | "output_cap"
        | "output_capacitance"
        | "total_output_net_capacitance"
        | "output_net_capacitance" => &[
            "total_output_net_capacitance",
            "output_net_capacitance",
            "output_capacitance",
            "capacitance",
        ],
        "transition" | "slew" | "input_slew" | "input_transition" | "input_net_transition" => {
            &["input_net_transition", "input_transition"]
        }
        "time" => &["time"],
        "voltage" | "vin" | "input_voltage" => &["input_voltage", "voltage"],
        "current" | "iout" | "output_current" => &["output_current", "current"],
        _ => &[],
    }
}

/// Does an axis role answer to the given keyword? Accepts the exact template
/// variable name, its synonyms and the usual shorthands (`load`, `transition`,
/// `slew`, …).
fn role_matches_role(role: &str, keyword: &str) -> bool {
    if role == keyword {
        return true;
    }
    let group = role_synonyms(keyword);
    !group.is_empty() && group.contains(&role)
}

/// Value of an attribute list entry, or None if the key is absent.
fn attribute_value(attributes: &[(String, String)], key: &str) -> Option<String> {
    attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

/// Parse the first number of an attribute value. Complex attributes are stored
/// as `"1.0, 2.0"`; the leading value is the one that carries the meaning for
/// the scalar accessors.
fn parse_first_number(value: &str) -> Option<f64> {
    value
        .split(',')
        .next()
        .and_then(|item| item.trim().parse::<f64>().ok())
}

/// FNV-1a hash of a byte slice (must match the lexer's running hash).
fn fnv1a(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ---- writer ---------------------------------------------------------------
//
// The writer is *surgical*: it keeps the original source text and rewrites only
// the attributes that were changed through `Cell::set_attribute` /
// `Pin::set_attribute`. Everything else — comments, spacing, unknown groups —
// survives byte for byte.

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
        let text = self.render_source_text()?;
        let bytes = text.as_bytes();
        if path.ends_with(".gz") {
            let file = File::create(path).map_err(|err| WriteError::Io(err.to_string()))?;
            let mut encoder = GzEncoder::new(file, flate2::Compression::default());
            encoder
                .write_all(bytes)
                .map_err(|err| WriteError::Io(err.to_string()))?;
            encoder
                .finish()
                .map_err(|err| WriteError::Io(err.to_string()))?;
        } else {
            std::fs::write(path, bytes).map_err(|err| WriteError::Io(err.to_string()))?;
        }
        Ok(())
    }
}

/// Read the original source text of a document, verifying that the file has
/// not changed since it was parsed.
///
/// Pure Rust: it takes no Python token, so the caller can run it with the GIL
/// released.
fn read_source_text(
    source_path: Option<&str>,
    source_len: usize,
    source_hash: u64,
) -> Result<String, WriteError> {
    let path = source_path.ok_or(WriteError::NoSource)?;
    let data = read_to_memory(path).map_err(|err| WriteError::Io(err.to_string()))?;
    if data.len() != source_len || fnv1a(&data) != source_hash {
        return Err(WriteError::Changed(format!(
            "source file {path:?} changed since it was parsed; re-parse it before writing"
        )));
    }
    String::from_utf8(data).map_err(|err| WriteError::Io(err.to_string()))
}

/// Apply the pending attribute edits to the source text of a document.
///
/// Only the cells that were edited are re-written; every other byte of the
/// source is kept as it is.
///
/// Pure Rust: it takes no Python token, so the caller can run it with the GIL
/// released.
fn apply_edits_to_source(
    source: String,
    cell_spans: &[(String, usize, usize)],
    edits: &[AttrEdit],
) -> Result<String, WriteError> {
    if edits.is_empty() {
        return Ok(source);
    }
    // Only the last edit of an attribute counts: applying two edits to the
    // same span would make the second one land in the middle of the
    // first one's text. The surviving edits keep the order they were made
    // in, so the written file is deterministic.
    let mut chosen: Vec<&AttrEdit> = Vec::new();
    {
        // Walk the log backwards and keep the first occurrence of every
        // key; that is the last edit of the attribute. Reversing afterwards
        // puts the survivors back in the order the edits were made in.
        let mut seen: HashSet<EditKey> = HashSet::new();
        for edit in edits.iter().rev() {
            let key = (
                edit.cell.clone(),
                edit.pin.clone(),
                edit.container.clone(),
                edit.key.clone(),
            );
            if seen.insert(key) {
                chosen.push(edit);
            }
        }
        chosen.reverse();
    }
    let mut by_cell: HashMap<&str, Vec<&AttrEdit>> = HashMap::new();
    for edit in chosen {
        by_cell.entry(edit.cell.as_str()).or_default().push(edit);
    }
    // Patch only the cell that `Document::cell` resolves to — the first
    // cell of that name in the source. A second cell with the same name
    // is a different cell and must not be rewritten.
    let mut spans: Vec<&(String, usize, usize)> = Vec::new();
    for cell_name in by_cell.keys() {
        let span = cell_spans
            .iter()
            .find(|(name, _, _)| name.as_str() == *cell_name)
            .ok_or_else(|| {
                WriteError::Invalid(format!(
                    "cell {cell_name:?} is not in the source text; its edits cannot be \
                     written back"
                ))
            })?;
        spans.push(span);
    }
    // Patch from the last cell backwards so earlier offsets stay valid.
    spans.sort_by_key(|(_, start, _)| std::cmp::Reverse(*start));
    let mut text = source;
    for (name, start, end) in spans {
        let cell_text = text.get(*start..*end).ok_or_else(|| {
            WriteError::Invalid(format!("cell {name:?} is outside the source text"))
        })?;
        let patched = patch_cell_text(cell_text, name, &by_cell[name.as_str()])?;
        text.replace_range(*start..*end, &patched);
    }
    Ok(text)
}

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

/// Replace an attribute's value in an ordered attribute list, or append it
/// when the key is not present yet.
fn set_attribute_in(attributes: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(entry) = attributes.iter_mut().find(|(name, _)| name == key) {
        entry.1 = value.to_string();
    } else {
        attributes.push((key.to_string(), value.to_string()));
    }
}

/// Convert a Python attribute value (string, bool or number) to Liberty text.
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

/// A group header found in a cell's source text.
struct SourceGroup {
    keyword: String,
    arg: Option<String>,
    /// Byte offset of the group's header (its keyword).
    start: usize,
    /// Byte offset of the group's closing brace.
    close: usize,
}

/// The first argument of a group header, reassembled the way the parser does
/// it: `A[0]` is lexed as the words `A` and `0` and the symbols `[` and `]`.
fn group_argument(tokens: &[Token], first: usize) -> Option<String> {
    let base = match tokens.get(first).map(|token| &token.kind) {
        Some(TokenKind::Word(value)) => value.clone(),
        _ => return None,
    };
    let index = match tokens.get(first + 1).map(|token| &token.kind) {
        Some(TokenKind::Symbol(b'[')) => match tokens.get(first + 2).map(|token| &token.kind) {
            Some(TokenKind::Word(index)) => Some(index.clone()),
            _ => None,
        },
        _ => None,
    };
    Some(indexed_value(&base, index.as_deref()))
}

/// A simple or complex attribute found in a cell's source text.
struct SourceAttribute {
    /// Index into the group stack.
    group: usize,
    key: String,
    /// Byte range of the whole attribute (name through value).
    start: usize,
    end: usize,
    complex: bool,
}

/// Scan one cell group's source text and return its groups and attributes.
fn scan_source_groups(text: &str) -> Result<(Vec<SourceGroup>, Vec<SourceAttribute>), ParseError> {
    let mut lexer = Lexer::new(Box::new(io::Cursor::new(text.as_bytes().to_vec())))?;
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token()? {
        tokens.push(token);
    }
    if tokens.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut groups: Vec<SourceGroup> = Vec::new();
    let mut attributes: Vec<SourceAttribute> = Vec::new();
    // Indices of the currently open groups, outermost first.
    let mut open: Vec<usize> = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        match &tokens[index].kind {
            TokenKind::Symbol(b'}') => {
                if let Some(group) = open.pop() {
                    groups[group].close = tokens[index].start;
                }
                index += 1;
            }
            TokenKind::Word(word) => {
                let is_paren = matches!(
                    tokens.get(index + 1).map(|token| &token.kind),
                    Some(TokenKind::Symbol(b'('))
                );
                if is_paren {
                    // Find the matching ')'.
                    let mut depth = 0i32;
                    let mut cursor = index + 1;
                    let mut close = tokens.len().saturating_sub(1);
                    while cursor < tokens.len() {
                        match tokens[cursor].kind {
                            TokenKind::Symbol(b'(') => depth += 1,
                            TokenKind::Symbol(b')') => {
                                depth -= 1;
                                if depth == 0 {
                                    close = cursor;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        cursor += 1;
                    }
                    let opens_group = matches!(
                        tokens.get(close + 1).map(|token| &token.kind),
                        Some(TokenKind::Symbol(b'{'))
                    );
                    if opens_group {
                        let arg = group_argument(&tokens, index + 2);
                        groups.push(SourceGroup {
                            keyword: word.clone(),
                            arg,
                            start: tokens[index].start,
                            close: 0,
                        });
                        open.push(groups.len() - 1);
                        index = close + 2;
                        continue;
                    }
                    attributes.push(SourceAttribute {
                        group: open.last().copied().unwrap_or(0),
                        key: word.clone(),
                        start: tokens[index].start,
                        end: tokens[close].end,
                        complex: true,
                    });
                    index = close + 1;
                    continue;
                }
                let is_colon = matches!(
                    tokens.get(index + 1).map(|token| &token.kind),
                    Some(TokenKind::Symbol(b':'))
                );
                if is_colon {
                    // Simple attribute: the value runs to the ';'.
                    let mut cursor = index + 2;
                    let mut end = tokens[index].end;
                    while cursor < tokens.len() {
                        if matches!(tokens[cursor].kind, TokenKind::Symbol(b';')) {
                            break;
                        }
                        end = tokens[cursor].end;
                        cursor += 1;
                    }
                    attributes.push(SourceAttribute {
                        group: open.last().copied().unwrap_or(0),
                        key: word.clone(),
                        start: tokens[index].start,
                        end,
                        complex: false,
                    });
                    index = cursor;
                    continue;
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    Ok((groups, attributes))
}

/// Render an attribute value so that reading the file back yields exactly
/// this value.
///
/// A value is written unquoted only when it cannot confuse the lexer: it must
/// be non-empty and consist of letters, digits and `_`, `+`, `.`, `-` only.
/// Every other value is written as a quoted string, where `\` becomes `\\` and
/// `"` becomes `\"` — the inverse of what the lexer's `read_quoted` does.
/// Quoting by a safe pattern rather than by a blacklist of dangerous
/// characters keeps the rule closed: a value with a `:`, a bracket or any
/// other punctuation in it still parses back to itself.
fn render_attribute_value(value: &str) -> String {
    let is_plain = !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '+' | '.' | '-'));
    if is_plain {
        return value.to_string();
    }
    let mut rendered = String::with_capacity(value.len() + 2);
    rendered.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => rendered.push_str("\\\\"),
            '"' => rendered.push_str("\\\""),
            _ => rendered.push(ch),
        }
    }
    rendered.push('"');
    rendered
}

/// Render the arguments of a complex attribute, keeping it complex: a value of
/// "0.1, 0.2" becomes `(0.1, 0.2)`, not a single quoted string. Each part is
/// quoted with the same rule as a simple value, so that the arguments parse
/// back to the same parts.
fn render_complex_attribute_value(value: &str) -> String {
    let parts: Vec<String> = value
        .split(',')
        .map(|part| render_attribute_value(part.trim()))
        .collect();
    parts.join(", ")
}

/// The group an edit's pin refers to.
///
/// A pin edit that names its container (the bus or bundle the pin was taken
/// from) is patched in the pin group inside that container. A pin edit
/// without a container is patched in the only pin group of that name in the
/// cell: when the name matches more than one group the write fails instead
/// of patching an arbitrary one.
fn resolve_pin_group(
    groups: &[SourceGroup],
    cell_name: &str,
    pin: &str,
    container: Option<&(ContainerKind, String)>,
) -> Result<usize, String> {
    let candidates: Vec<usize> = groups
        .iter()
        .enumerate()
        .filter(|(_, group)| group.keyword == "pin" && group.arg.as_deref() == Some(pin))
        .map(|(index, _)| index)
        .collect();
    let Some((kind, container_name)) = container else {
        return match candidates.len() {
            1 => Ok(candidates[0]),
            0 => Err(format!("cell {cell_name:?} has no pin {pin:?} to edit")),
            _ => Err(format!(
                "cell {cell_name:?} has {} pins named {pin:?} ({}); the edit does not say \
                 which one to patch",
                candidates.len(),
                describe_pin_locations(groups, &candidates)
            )),
        };
    };
    let container_group = groups
        .iter()
        .position(|group| {
            group.keyword == kind.keyword() && group.arg.as_deref() == Some(container_name.as_str())
        })
        .ok_or_else(|| {
            format!(
                "cell {cell_name:?} has no {} {container_name:?} to edit",
                kind.keyword()
            )
        })?;
    let inside: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&index| {
            groups[index].start > groups[container_group].start
                && groups[index].close <= groups[container_group].close
        })
        .collect();
    match inside.len() {
        1 => Ok(inside[0]),
        0 => Err(format!(
            "cell {cell_name:?} has no pin {pin:?} in {} {container_name:?} to edit",
            kind.keyword()
        )),
        _ => Err(format!(
            "cell {cell_name:?} has {} pins named {pin:?} in {} {container_name:?}; the edit \
             does not say which one to patch",
            inside.len(),
            kind.keyword()
        )),
    }
}

/// Where the candidate pin groups live, for the ambiguity message: the
/// innermost group that contains them (`bus (A)`, `bundle (B)`, …).
fn describe_pin_locations(groups: &[SourceGroup], candidates: &[usize]) -> String {
    candidates
        .iter()
        .map(|&index| {
            groups
                .iter()
                .filter(|group| {
                    group.start < groups[index].start && group.close >= groups[index].close
                })
                .max_by_key(|group| group.start)
                .and_then(|group| {
                    group
                        .arg
                        .as_deref()
                        .map(|arg| format!("{} ({arg})", group.keyword))
                })
                .unwrap_or_else(|| "the cell".to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Apply attribute edits to one cell group's source text.
///
/// Every edit is turned into a `(start, end, text)` range against the
/// *original* text — an insertion is an empty range — and all of them are
/// applied in a single back-to-front pass, so no edit can shift another.
fn patch_cell_text(
    cell_text: &str,
    cell_name: &str,
    edits: &[&AttrEdit],
) -> Result<String, WriteError> {
    let (groups, attributes) = scan_source_groups(cell_text).map_err(WriteError::Parse)?;

    let cell_group = groups
        .iter()
        .position(|group| group.keyword == "cell")
        .ok_or_else(|| {
            WriteError::Invalid(format!(
                "cell {cell_name:?} has no cell group in the source"
            ))
        })?;

    // (start, end, replacement, order) against the original text.
    let mut patches: Vec<(usize, usize, String, usize)> = Vec::new();

    for (order, edit) in edits.iter().enumerate() {
        let target_group = match &edit.pin {
            None => cell_group,
            Some(pin) => resolve_pin_group(&groups, cell_name, pin, edit.container.as_ref())
                .map_err(WriteError::Invalid)?,
        };
        let existing = attributes
            .iter()
            .find(|attribute| attribute.group == target_group && attribute.key == edit.key);
        match existing {
            Some(attribute) => {
                let rendered = if attribute.complex {
                    format!(
                        "{} ({})",
                        edit.key,
                        render_complex_attribute_value(&edit.value)
                    )
                } else {
                    format!("{} : {}", edit.key, render_attribute_value(&edit.value))
                };
                patches.push((attribute.start, attribute.end, rendered, order));
            }
            None => {
                // Add a new attribute to the group.
                let group = &groups[target_group];
                let line_start = cell_text[..group.close]
                    .rfind('\n')
                    .map(|position| position + 1)
                    .unwrap_or(0);
                let before_brace = &cell_text[line_start..group.close];
                let brace_alone_on_line = before_brace.trim().is_empty();
                let closing_indent: String = before_brace
                    .chars()
                    .take_while(|ch| ch.is_whitespace())
                    .collect();
                let rendered = format!("{} : {};", edit.key, render_attribute_value(&edit.value));
                let newline = if cell_text.contains("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                };
                if brace_alone_on_line {
                    // The group ends on a line of its own: add the attribute
                    // as a new line above it, one level deeper.
                    let indent = attributes
                        .iter()
                        .find(|attribute| attribute.group == target_group)
                        .and_then(|attribute| {
                            let start = cell_text[..attribute.start]
                                .rfind('\n')
                                .map(|position| position + 1)
                                .unwrap_or(0);
                            let line = &cell_text[start..attribute.start];
                            let whitespace: String =
                                line.chars().take_while(|ch| ch.is_whitespace()).collect();
                            if whitespace.len() == line.len() && !whitespace.is_empty() {
                                Some(whitespace)
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| format!("{closing_indent}  "));
                    patches.push((
                        line_start,
                        line_start,
                        format!("{indent}{rendered}{newline}"),
                        order,
                    ));
                } else {
                    // The group is written on one line: keep it that way and
                    // add the attribute in front of the closing brace.
                    let separator = if cell_text[..group.close]
                        .chars()
                        .next_back()
                        .map(|ch| ch.is_whitespace())
                        .unwrap_or(false)
                    {
                        ""
                    } else {
                        " "
                    };
                    patches.push((
                        group.close,
                        group.close,
                        format!("{separator}{rendered} "),
                        order,
                    ));
                }
            }
        }
    }

    // Apply the edits back to front so the offsets stay valid. Edits at the
    // same offset are applied in reverse order, so that they end up in the
    // order they were made.
    patches
        .sort_by_key(|(start, _, _, order)| (std::cmp::Reverse(*start), std::cmp::Reverse(*order)));
    let mut text = cell_text.to_string();
    for (start, end, rendered, _) in patches {
        text.replace_range(start..end, &rendered);
    }
    Ok(text)
}

/// Attach the library's unit scales to every cell and pin, so that scalar
/// accessors can report values with their scale attached.
fn apply_units(cells: &mut [CellData], ctx: UnitContext) {
    for cell in cells {
        cell.units = ctx;
        for pin in &mut cell.pins {
            pin.units = ctx;
        }
        for bus in &mut cell.buses {
            for pin in &mut bus.pins {
                pin.units = ctx;
            }
        }
        for bundle in &mut cell.bundles {
            for pin in &mut bundle.pins {
                pin.units = ctx;
            }
        }
    }
}

/// Look up a table template by name and return its axis variable names.
fn template_vars(
    templates: &[(String, [Option<String>; 3])],
    name: &Option<String>,
) -> Option<[Option<String>; 3]> {
    let name = name.as_ref()?;
    templates
        .iter()
        .find(|(template, _)| template == name)
        .map(|(_, vars)| vars.clone())
}

/// Resolve the axis variable names of every table in a list (recursively, so
/// CCS `vector` sub-tables are covered too).
fn resolve_axis_vars_in(
    tables: &mut [TimingTableData],
    templates: &[(String, [Option<String>; 3])],
) {
    for table in tables.iter_mut() {
        table.axis_vars = template_vars(templates, &table.template).unwrap_or([None, None, None]);
        resolve_axis_vars_in(&mut table.vectors, templates);
    }
}

/// Resolve the axis variable names of every table reachable from the cells.
fn resolve_axis_vars(cells: &mut [CellData], templates: &[(String, [Option<String>; 3])]) {
    fn resolve_arcs(arcs: &mut [TimingArcData], templates: &[(String, [Option<String>; 3])]) {
        for arc in arcs.iter_mut() {
            resolve_axis_vars_in(&mut arc.tables, templates);
            for stage in arc.ccsn_stages.iter_mut() {
                if let Some(table) = stage.dc_current.as_mut() {
                    table.axis_vars =
                        template_vars(templates, &table.template).unwrap_or([None, None, None]);
                    resolve_axis_vars_in(&mut table.vectors, templates);
                }
                resolve_axis_vars_in(&mut stage.output_voltage_rise, templates);
                resolve_axis_vars_in(&mut stage.output_voltage_fall, templates);
            }
        }
    }
    fn resolve_pins(pins: &mut [PinData], templates: &[(String, [Option<String>; 3])]) {
        for pin in pins.iter_mut() {
            resolve_arcs(&mut pin.timing_arcs, templates);
            for group in pin.internal_powers.iter_mut() {
                resolve_axis_vars_in(&mut group.tables, templates);
            }
        }
    }
    for cell in cells.iter_mut() {
        resolve_pins(&mut cell.pins, templates);
        for bus in cell.buses.iter_mut() {
            resolve_arcs(&mut bus.timing_arcs, templates);
            resolve_pins(&mut bus.pins, templates);
        }
        for bundle in cell.bundles.iter_mut() {
            resolve_arcs(&mut bundle.timing_arcs, templates);
            resolve_pins(&mut bundle.pins, templates);
        }
        for dynamic in cell.dynamic_currents.iter_mut() {
            for group in dynamic.switching_groups.iter_mut() {
                for current in group.pg_currents.iter_mut() {
                    resolve_axis_vars_in(&mut current.vectors, templates);
                }
            }
        }
    }
}

fn parse_float_list(value: Option<&String>) -> Vec<f64> {
    value
        .map(|items| {
            items
                .split(',')
                .filter_map(|item| item.trim().parse::<f64>().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_float_args(values: &[String]) -> Vec<f64> {
    values
        .iter()
        .flat_map(|items| items.split(','))
        .filter_map(|item| item.trim().parse::<f64>().ok())
        .collect()
}

fn members_from_args(args: &[String]) -> Vec<String> {
    args.iter()
        .flat_map(|arg| arg.split_whitespace())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[derive(Debug)]
struct ParseError {
    line: usize,
    column: usize,
    message: String,
}

impl ParseError {
    fn new(line: usize, column: usize, message: &str) -> Self {
        Self {
            line,
            column,
            message: message.to_string(),
        }
    }

    fn into_py(self) -> PyErr {
        PyValueError::new_err(format!(
            "Liberty parse error at line {}, column {}: {}",
            self.line, self.column, self.message
        ))
    }
}

enum ParseLoadError {
    Io(io::Error),
    Parse(ParseError),
}

impl ParseLoadError {
    fn into_py(self) -> PyErr {
        match self {
            Self::Io(err) => PyValueError::new_err(err.to_string()),
            Self::Parse(err) => err.into_py(),
        }
    }
}

enum CellLookupError {
    Unknown(String),
    Parse(ParseError),
    Empty(String),
}

impl CellLookupError {
    fn into_py(self) -> PyErr {
        match self {
            Self::Unknown(name) => PyKeyError::new_err(format!("unknown cell {name:?}")),
            Self::Parse(err) => err.into_py(),
            Self::Empty(name) => PyValueError::new_err(format!("failed to parse cell {name:?}")),
        }
    }
}

enum CellSourceError {
    Unknown(String),
}

impl CellSourceError {
    fn into_py(self) -> PyErr {
        match self {
            Self::Unknown(name) => PyKeyError::new_err(format!("unknown cell {name:?}")),
        }
    }
}

/// Failure of the writer path.
///
/// The writer runs with the GIL released, so it cannot build a `PyErr`
/// itself: it reports the failure with this plain enum and the caller
/// converts it to a `PyErr` outside the detached closure.
enum WriteError {
    /// The document was not parsed from a file.
    NoSource,
    /// The source file could not be read or written.
    Io(String),
    /// The source file changed since the document was parsed.
    Changed(String),
    /// The document's edit log is poisoned.
    Poisoned,
    /// An edit does not apply to the source text.
    Invalid(String),
    /// The source text could not be lexed.
    Parse(ParseError),
}

impl WriteError {
    fn into_py(self) -> PyErr {
        match self {
            Self::NoSource => PyValueError::new_err(
                "this document has no source file; only documents parsed from a \
                 file can be written back",
            ),
            Self::Io(message) => PyValueError::new_err(message),
            Self::Changed(message) => PyValueError::new_err(message),
            Self::Poisoned => PyValueError::new_err("document edit log is poisoned"),
            Self::Invalid(message) => PyValueError::new_err(message),
            Self::Parse(err) => err.into_py(),
        }
    }
}

impl From<io::Error> for ParseError {
    fn from(value: io::Error) -> Self {
        Self::new(0, 0, &value.to_string())
    }
}
