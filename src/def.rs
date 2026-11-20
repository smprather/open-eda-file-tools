//! Python surface of the DEF tool: pyclasses and pyfunctions over the
//! plain-Rust machinery in `open_eda_file_tools_core::def`.

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, BooleanArray, Float64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use open_eda_file_tools_core::def::{
    self as core_def, ColumnKind, ColumnSpec, DefScanner, InstanceBBox as CoreInstanceBBox,
    MacroPinRole, NetShape as CoreNetShape, SkippedRoutes, parse_columns, parse_lef_files,
};

#[pyclass(frozen, skip_from_py_object)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceBBox {
    inner: CoreInstanceBBox,
}

#[pyclass(frozen, skip_from_py_object)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetShape {
    inner: CoreNetShape,
}

#[pymethods]
impl InstanceBBox {
    #[getter]
    fn instance(&self) -> &str {
        &self.inner.instance
    }

    #[getter]
    fn macro_name(&self) -> &str {
        &self.inner.macro_name
    }

    #[getter]
    fn status(&self) -> &str {
        &self.inner.status
    }

    #[getter]
    fn orient(&self) -> &str {
        &self.inner.orient
    }

    #[getter]
    fn llx(&self) -> i64 {
        self.inner.llx
    }

    #[getter]
    fn lly(&self) -> i64 {
        self.inner.lly
    }

    #[getter]
    fn urx(&self) -> i64 {
        self.inner.urx
    }

    #[getter]
    fn ury(&self) -> i64 {
        self.inner.ury
    }

    fn __repr__(&self) -> String {
        format!(
            "InstanceBBox(instance={:?}, macro_name={:?}, status={:?}, orient={:?}, llx={}, lly={}, urx={}, ury={})",
            self.inner.instance,
            self.inner.macro_name,
            self.inner.status,
            self.inner.orient,
            self.inner.llx,
            self.inner.lly,
            self.inner.urx,
            self.inner.ury
        )
    }

    pub fn as_tuple(&self) -> (String, String, String, String, i64, i64, i64, i64) {
        (
            self.inner.instance.clone(),
            self.inner.macro_name.clone(),
            self.inner.status.clone(),
            self.inner.orient.clone(),
            self.inner.llx,
            self.inner.lly,
            self.inner.urx,
            self.inner.ury,
        )
    }
}

#[pymethods]
impl NetShape {
    #[getter]
    fn net(&self) -> &str {
        &self.inner.net
    }

    #[getter]
    fn section(&self) -> &str {
        &self.inner.section
    }

    #[getter]
    fn route_status(&self) -> &str {
        &self.inner.route_status
    }

    #[getter]
    fn shape_type(&self) -> &str {
        &self.inner.shape_type
    }

    #[getter]
    fn layer(&self) -> &str {
        &self.inner.layer
    }

    #[getter]
    fn width(&self) -> i64 {
        self.inner.width
    }

    #[getter]
    fn x1(&self) -> i64 {
        self.inner.x1
    }

    #[getter]
    fn y1(&self) -> i64 {
        self.inner.y1
    }

    #[getter]
    fn x2(&self) -> i64 {
        self.inner.x2
    }

    #[getter]
    fn y2(&self) -> i64 {
        self.inner.y2
    }

    #[getter]
    fn llx(&self) -> i64 {
        self.inner.llx
    }

    #[getter]
    fn lly(&self) -> i64 {
        self.inner.lly
    }

    #[getter]
    fn urx(&self) -> i64 {
        self.inner.urx
    }

    #[getter]
    fn ury(&self) -> i64 {
        self.inner.ury
    }

    fn __repr__(&self) -> String {
        format!(
            "NetShape(net={:?}, section={:?}, route_status={:?}, shape_type={:?}, layer={:?}, width={}, x1={}, y1={}, x2={}, y2={}, llx={}, lly={}, urx={}, ury={})",
            self.inner.net,
            self.inner.section,
            self.inner.route_status,
            self.inner.shape_type,
            self.inner.layer,
            self.inner.width,
            self.inner.x1,
            self.inner.y1,
            self.inner.x2,
            self.inner.y2,
            self.inner.llx,
            self.inner.lly,
            self.inner.urx,
            self.inner.ury
        )
    }
}

#[pyclass(unsendable)]
struct InstanceBBoxIter {
    inner: core_def::InstanceBBoxIter,
}

#[pyclass(unsendable)]
struct NetShapeIter {
    inner: core_def::NetShapeIter,
}

#[pymethods]
impl InstanceBBoxIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self) -> PyResult<Option<InstanceBBox>> {
        match self.inner.next() {
            Some(Ok(inner)) => Ok(Some(InstanceBBox { inner })),
            Some(Err(err)) => Err(to_py_err(err)),
            None => Ok(None),
        }
    }
}

#[pymethods]
impl NetShapeIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self) -> PyResult<Option<NetShape>> {
        match self.inner.next() {
            Some(Ok(inner)) => Ok(Some(NetShape { inner })),
            Some(Err(err)) => Err(to_py_err(err)),
            None => Ok(None),
        }
    }

    /// Route pieces skipped so far: `after_via`, `polygon`, `subnet`.
    #[getter]
    fn skipped<'py>(&self, py: Python<'py>) -> Bound<'py, PyDict> {
        skipped_dict(py, self.inner.skipped())
    }
}

#[pyfunction]
fn instance_bboxes(lefs: Vec<PathBuf>, def_path: PathBuf) -> PyResult<InstanceBBoxIter> {
    Ok(InstanceBBoxIter {
        inner: core_def::scan_instance_bboxes(&def_path, &lefs).map_err(to_py_err)?,
    })
}

#[pyfunction]
fn net_shapes(def_path: PathBuf, net: String, layer: String) -> PyResult<NetShapeIter> {
    Ok(NetShapeIter {
        inner: core_def::scan_net_shapes(&def_path, &net, &layer).map_err(to_py_err)?,
    })
}

#[pyfunction]
#[pyo3(signature = (lefs, def_path, output, columns = None, sequential = None, clock = None))]
fn write_instance_table(
    py: Python<'_>,
    lefs: Vec<PathBuf>,
    def_path: PathBuf,
    output: Option<PathBuf>,
    columns: Option<Vec<(String, String)>>,
    sequential: Option<Vec<String>>,
    clock: Option<Vec<String>>,
) -> PyResult<usize> {
    let sequential = sequential.map(|values| values.into_iter().collect::<HashSet<_>>());
    let clock = clock.map(|values| values.into_iter().collect::<HashSet<_>>());
    let columns = parse_columns(columns, sequential.is_some(), clock.is_some())
        .map_err(PyRuntimeError::new_err)?;
    py.detach(|| {
        open_eda_file_tools_core::def::write_instance_table_impl(
            &lefs,
            &def_path,
            output.as_deref(),
            &columns,
            sequential.as_ref(),
            clock.as_ref(),
        )
        .map_err(to_py_err)
    })
}

#[pyfunction]
#[pyo3(signature = (lefs, def_path, output, columns = None, sequential = None, clock = None))]
fn write_instance_bbox_csv(
    py: Python<'_>,
    lefs: Vec<PathBuf>,
    def_path: PathBuf,
    output: Option<PathBuf>,
    columns: Option<Vec<(String, String)>>,
    sequential: Option<Vec<String>>,
    clock: Option<Vec<String>>,
) -> PyResult<usize> {
    let sequential = sequential.map(|values| values.into_iter().collect::<HashSet<_>>());
    let clock = clock.map(|values| values.into_iter().collect::<HashSet<_>>());
    let columns = parse_columns(columns, sequential.is_some(), clock.is_some())
        .map_err(PyRuntimeError::new_err)?;
    py.detach(|| {
        open_eda_file_tools_core::def::write_instance_bbox_csv_impl(
            &lefs,
            &def_path,
            output.as_deref(),
            &columns,
            sequential.as_ref(),
            clock.as_ref(),
        )
        .map_err(to_py_err)
    })
}

#[pyfunction]
#[pyo3(signature = (lefs, def_path, output, batch_size, columns = None, sequential = None, clock = None))]
fn write_instance_parquet(
    py: Python<'_>,
    lefs: Vec<PathBuf>,
    def_path: PathBuf,
    output: PathBuf,
    batch_size: usize,
    columns: Option<Vec<(String, String)>>,
    sequential: Option<Vec<String>>,
    clock: Option<Vec<String>>,
) -> PyResult<(usize, usize)> {
    let sequential = sequential.map(|values| values.into_iter().collect::<HashSet<_>>());
    let clock = clock.map(|values| values.into_iter().collect::<HashSet<_>>());
    let columns = parse_columns(columns, sequential.is_some(), clock.is_some())
        .map_err(PyRuntimeError::new_err)?;
    py.detach(|| {
        write_instance_parquet_impl(
            &lefs,
            &def_path,
            &output,
            batch_size,
            &columns,
            sequential.as_ref(),
            clock.as_ref(),
        )
        .map_err(to_py_err)
    })
}

#[pyfunction]
fn write_net_shapes_csv(
    py: Python<'_>,
    def_path: PathBuf,
    net: String,
    layer: String,
    output: Option<PathBuf>,
) -> PyResult<Bound<'_, PyDict>> {
    let skipped = py.detach(|| {
        open_eda_file_tools_core::def::write_net_shapes_csv_impl(
            &def_path,
            &net,
            &layer,
            output.as_deref(),
        )
        .map_err(to_py_err)
    })?;
    Ok(skipped_dict(py, skipped))
}

#[pyfunction]
fn write_def_cache(py: Python<'_>, def_path: PathBuf, cache_dir: PathBuf) -> PyResult<()> {
    py.detach(|| {
        open_eda_file_tools_core::def::write_def_cache_impl(&def_path, &cache_dir)
            .map_err(to_py_err)
    })
}

#[pyfunction]
fn write_net_shapes_csv_from_cache(
    py: Python<'_>,
    cache_dir: PathBuf,
    net: String,
    layer: String,
    output: Option<PathBuf>,
) -> PyResult<Bound<'_, PyDict>> {
    let skipped = py.detach(|| {
        open_eda_file_tools_core::def::write_net_shapes_csv_from_cache_impl(
            &cache_dir,
            &net,
            &layer,
            output.as_deref(),
        )
        .map_err(to_py_err)
    })?;
    Ok(skipped_dict(py, skipped))
}

#[pyfunction]
fn write_clock_instances_csv(
    py: Python<'_>,
    def_path: PathBuf,
    roots: Vec<(String, String, String)>,
    clock_cell_patterns: Vec<String>,
    output: Option<PathBuf>,
) -> PyResult<()> {
    py.detach(|| {
        open_eda_file_tools_core::def::write_clock_instances_csv_impl(
            &def_path,
            &roots,
            &clock_cell_patterns,
            output.as_deref(),
        )
        .map_err(to_py_err)
    })
}

#[pyfunction]
fn write_clock_instances_csv_from_cache(
    py: Python<'_>,
    cache_dir: PathBuf,
    roots: Vec<(String, String, String)>,
    clock_cell_patterns: Vec<String>,
    output: Option<PathBuf>,
) -> PyResult<()> {
    py.detach(|| {
        open_eda_file_tools_core::def::write_clock_instances_csv_from_cache_impl(
            &cache_dir,
            &roots,
            &clock_cell_patterns,
            output.as_deref(),
        )
        .map_err(to_py_err)
    })
}

// Retain the Python tuple format at the binding boundary.
type MacroRole = (String, Vec<String>, Vec<String>, bool, Vec<String>);
type ClockObjectInstances = (Vec<String>, Vec<String>, Vec<(String, String)>, Vec<String>);

fn macro_pin_roles(macros: Vec<MacroRole>) -> Vec<MacroPinRole> {
    macros
        .into_iter()
        .map(
            |(macro_name, inputs, outputs, endpoint, clock_pins)| MacroPinRole {
                macro_name,
                inputs,
                outputs,
                endpoint,
                clock_pins,
            },
        )
        .collect()
}

#[pyfunction]
#[pyo3(signature = (def_path, roots, macros, prune = true))]
fn clock_tree_cells(
    py: Python<'_>,
    def_path: PathBuf,
    roots: Vec<String>,
    macros: Vec<MacroRole>,
    prune: bool,
) -> PyResult<Vec<String>> {
    py.detach(|| {
        let macros = macro_pin_roles(macros);
        core_def::clock_tree_cells_impl(&def_path, &roots, &macros, prune).map_err(to_py_err)
    })
}

/// Forward trace from symbolic clock roots through DEF connectivity.
///
/// A non-endpoint cell with a clock-through input on a clock net is a
/// candidate; its output nets are followed. Endpoints end the trace and are
/// never reported.
///
/// With `prune`, a candidate is kept only when a clock pin is reachable from
/// it: a generated clock on a divider flop's `Q` also fans out into ordinary
/// datapath logic, and that cone ends at data pins, not at clock pins.
#[pyfunction]
#[pyo3(signature = (def_path, nets, pins, macros))]
fn clock_object_instances(
    py: Python<'_>,
    def_path: PathBuf,
    nets: Vec<String>,
    pins: Vec<(String, String)>,
    macros: Vec<MacroRole>,
) -> PyResult<ClockObjectInstances> {
    py.detach(|| {
        let macros = macro_pin_roles(macros);
        let resolution = core_def::clock_object_instances(&def_path, &nets, &pins, &macros)
            .map_err(to_py_err)?;
        Ok((
            resolution.instances,
            resolution.unmatched_nets,
            resolution.unmatched_pins,
            resolution.unclassified,
        ))
    })
}

fn write_instance_parquet_impl(
    lefs: &[PathBuf],
    def_path: &Path,
    output: &Path,
    batch_size: usize,
    columns: &[ColumnSpec],
    sequential: Option<&HashSet<String>>,
    clock: Option<&HashSet<String>>,
) -> Result<(usize, usize), String> {
    let mut scanner = DefScanner::from_path(def_path).map_err(|e| e.to_string())?;
    let (macros, duplicates) = parse_lef_files(lefs, scanner.dbu_per_micron)?;
    if !duplicates.is_empty() {
        eprintln!(
            "warning: duplicate LEF macros, last definition used: {}",
            duplicates.join(", ")
        );
    }
    let dbu = scanner.dbu_per_micron as f64;
    let fields: Vec<Field> = columns
        .iter()
        .map(|column| {
            let data_type = match column.kind {
                ColumnKind::Instance
                | ColumnKind::BaseCell
                | ColumnKind::Status
                | ColumnKind::Orient => DataType::Utf8,
                ColumnKind::IsSeq | ColumnKind::IsClock => DataType::Boolean,
                _ => DataType::Float64,
            };
            Field::new(&column.name, data_type, false)
        })
        .collect();
    let schema = Arc::new(Schema::new(fields));
    let file = File::create(output).map_err(|e| format!("{}: {e}", output.display()))?;
    let chunk = batch_size.max(1);
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(Default::default()))
        .set_max_row_group_row_count(Some(chunk))
        .build();
    let mut writer = ArrowWriter::try_new(file, Arc::clone(&schema), Some(properties))
        .map_err(|e| e.to_string())?;

    let mut count = 0usize;
    open_eda_file_tools_core::def::for_each_batch(
        || scanner.next_batch(chunk),
        |batch, overlapped| {
            let bboxes = open_eda_file_tools_core::threads::install_for(overlapped, || {
                open_eda_file_tools_core::def::parse_components(&batch, &macros)
            })?;
            if bboxes.is_empty() {
                return Ok(());
            }
            let mut cache: HashMap<ColumnKind, ArrayRef> = HashMap::new();
            let mut arrays: Vec<ArrayRef> = Vec::with_capacity(columns.len());
            for column in columns {
                if let Some(array) = cache.get(&column.kind) {
                    arrays.push(Arc::clone(array));
                    continue;
                }
                let array: ArrayRef = match column.kind {
                    ColumnKind::Instance => Arc::new(StringArray::from(
                        bboxes.iter().map(|item| item.instance).collect::<Vec<_>>(),
                    )),
                    ColumnKind::BaseCell => Arc::new(StringArray::from(
                        bboxes
                            .iter()
                            .map(|item| item.macro_name)
                            .collect::<Vec<_>>(),
                    )),
                    ColumnKind::Status => Arc::new(StringArray::from(
                        bboxes.iter().map(|item| item.status).collect::<Vec<_>>(),
                    )),
                    ColumnKind::Orient => Arc::new(StringArray::from(
                        bboxes.iter().map(|item| item.orient).collect::<Vec<_>>(),
                    )),
                    ColumnKind::IsSeq => Arc::new(BooleanArray::from(
                        bboxes
                            .iter()
                            .map(|item| sequential.is_some_and(|set| set.contains(item.macro_name)))
                            .collect::<Vec<_>>(),
                    )),
                    ColumnKind::IsClock => Arc::new(BooleanArray::from(
                        bboxes
                            .iter()
                            .map(|item| clock.is_some_and(|set| set.contains(item.instance)))
                            .collect::<Vec<_>>(),
                    )),
                    ColumnKind::Llx => Arc::new(Float64Array::from(
                        bboxes
                            .iter()
                            .map(|item| item.llx as f64 / dbu)
                            .collect::<Vec<_>>(),
                    )),
                    ColumnKind::Lly => Arc::new(Float64Array::from(
                        bboxes
                            .iter()
                            .map(|item| item.lly as f64 / dbu)
                            .collect::<Vec<_>>(),
                    )),
                    ColumnKind::Urx => Arc::new(Float64Array::from(
                        bboxes
                            .iter()
                            .map(|item| item.urx as f64 / dbu)
                            .collect::<Vec<_>>(),
                    )),
                    ColumnKind::Ury => Arc::new(Float64Array::from(
                        bboxes
                            .iter()
                            .map(|item| item.ury as f64 / dbu)
                            .collect::<Vec<_>>(),
                    )),
                };
                cache.insert(column.kind, Arc::clone(&array));
                arrays.push(array);
            }
            let record =
                RecordBatch::try_new(Arc::clone(&schema), arrays).map_err(|e| e.to_string())?;
            writer.write(&record).map_err(|e| e.to_string())?;
            count += bboxes.len();
            Ok(())
        },
    )?;
    let metadata = writer.close().map_err(|e| e.to_string())?;
    Ok((count, metadata.num_row_groups()))
}

fn skipped_dict(py: Python<'_>, skipped: SkippedRoutes) -> Bound<'_, PyDict> {
    let map = PyDict::new(py);
    map.set_item("after_via", skipped.after_via).unwrap();
    map.set_item("polygon", skipped.polygon).unwrap();
    map.set_item("subnet", skipped.subnet).unwrap();
    map
}

fn to_py_err<E: ToString>(err: E) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<InstanceBBox>()?;
    m.add_class::<InstanceBBoxIter>()?;
    m.add_class::<NetShape>()?;
    m.add_class::<NetShapeIter>()?;
    m.add_function(wrap_pyfunction!(instance_bboxes, m)?)?;
    m.add_function(wrap_pyfunction!(net_shapes, m)?)?;
    m.add_function(wrap_pyfunction!(write_instance_table, m)?)?;
    m.add_function(wrap_pyfunction!(write_instance_bbox_csv, m)?)?;
    m.add_function(wrap_pyfunction!(write_instance_parquet, m)?)?;
    m.add_function(wrap_pyfunction!(write_net_shapes_csv, m)?)?;
    m.add_function(wrap_pyfunction!(write_def_cache, m)?)?;
    m.add_function(wrap_pyfunction!(write_net_shapes_csv_from_cache, m)?)?;
    m.add_function(wrap_pyfunction!(write_clock_instances_csv, m)?)?;
    m.add_function(wrap_pyfunction!(write_clock_instances_csv_from_cache, m)?)?;
    m.add_function(wrap_pyfunction!(clock_tree_cells, m)?)?;
    m.add_function(wrap_pyfunction!(clock_object_instances, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn parquet_writer_renames_columns_in_order() {
        use parquet::file::reader::{FileReader, SerializedFileReader};

        let lef = tempfile_path("def_tools_columns.lef");
        std::fs::write(&lef, "MACRO INV\n  SIZE 1.0 BY 1.0 ;\nEND INV\n").unwrap();
        let def = tempfile_path("def_tools_columns.def");
        std::fs::write(
            &def,
            "VERSION 5.8 ;\nUNITS DISTANCE MICRONS 1000 ;\nCOMPONENTS 1 ;\n  - u1 INV + PLACED ( 10 20 ) N ;\nEND COMPONENTS\n",
        )
        .unwrap();
        let output = tempfile_path("def_tools_columns.parquet");

        let columns = parse_columns(
            Some(vec![
                ("instance".to_string(), "name".to_string()),
                ("base_cell".to_string(), "cell".to_string()),
            ]),
            false,
            false,
        )
        .unwrap();
        let (rows, row_groups) =
            write_instance_parquet_impl(&[lef.clone()], &def, &output, 1024, &columns, None, None)
                .unwrap();
        assert_eq!((rows, row_groups), (1, 1));

        let reader = SerializedFileReader::new(File::open(&output).unwrap()).unwrap();
        let schema = reader.metadata().file_metadata().schema_descr();
        let names: Vec<&str> = schema
            .columns()
            .iter()
            .map(|column| column.name())
            .collect();
        assert_eq!(names, ["name", "cell"]);

        std::fs::remove_file(lef).ok();
        std::fs::remove_file(def).ok();
        std::fs::remove_file(output).ok();
    }

    fn tempfile_path(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "{}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            name
        ))
    }
}
