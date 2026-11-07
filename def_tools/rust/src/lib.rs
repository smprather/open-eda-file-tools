use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as FmtWrite;
use std::fs::{File, OpenOptions, create_dir_all};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use lef_tools_native::{Lexer, open_input};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ColumnKind {
    Instance,
    BaseCell,
    Llx,
    Lly,
    Urx,
    Ury,
}

struct ColumnSpec {
    kind: ColumnKind,
    name: String,
}

fn default_columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec {
            kind: ColumnKind::Instance,
            name: "instance".to_string(),
        },
        ColumnSpec {
            kind: ColumnKind::BaseCell,
            name: "base_cell".to_string(),
        },
        ColumnSpec {
            kind: ColumnKind::Llx,
            name: "llx".to_string(),
        },
        ColumnSpec {
            kind: ColumnKind::Lly,
            name: "lly".to_string(),
        },
        ColumnSpec {
            kind: ColumnKind::Urx,
            name: "urx".to_string(),
        },
        ColumnSpec {
            kind: ColumnKind::Ury,
            name: "ury".to_string(),
        },
    ]
}

/// Turn ``(source, output_name)`` pairs from Python into columns.
///
/// ``None`` selects the canonical six columns in their canonical order. The
/// source names are the project's column names; the output names are what the
/// file gets, so a rename is just a different second half of the pair.
fn parse_columns(columns: Option<Vec<(String, String)>>) -> Result<Vec<ColumnSpec>, String> {
    let Some(pairs) = columns else {
        return Ok(default_columns());
    };
    if pairs.is_empty() {
        return Err("no columns selected".to_string());
    }
    let mut seen = HashSet::new();
    let mut parsed = Vec::with_capacity(pairs.len());
    for (source, name) in pairs {
        let kind = match source.as_str() {
            "instance" => ColumnKind::Instance,
            "base_cell" => ColumnKind::BaseCell,
            "llx" => ColumnKind::Llx,
            "lly" => ColumnKind::Lly,
            "urx" => ColumnKind::Urx,
            "ury" => ColumnKind::Ury,
            other => {
                return Err(format!(
                    "unknown column '{other}' (choose instance, base_cell, llx, lly, urx, ury)"
                ));
            }
        };
        if name.is_empty() {
            return Err(format!("empty output name for column '{source}'"));
        }
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate output column '{name}'"));
        }
        parsed.push(ColumnSpec { kind, name });
    }
    Ok(parsed)
}

const BATCH_SIZE: usize = 65_536;
const CACHE_VERSION: u32 = 1;
const CACHE_HEADER_LEN: u64 = 20;
const MANIFEST_MAGIC: &[u8; 8] = b"LDTMAN1\0";
const COMPONENTS_MAGIC: &[u8; 8] = b"LDTCMP1\0";
const NET_DATA_MAGIC: &[u8; 8] = b"LDTNDT1\0";
const NET_INDEX_MAGIC: &[u8; 8] = b"LDTNIX1\0";
const NET_MAP_MAGIC: &[u8; 8] = b"LDTNMP1\0";
const CONNECTIVITY_MAGIC: &[u8; 8] = b"LDTCNX1\0";
const NET_INDEX_ENTRY_LEN: u64 = 25;

type MacroMap = HashMap<String, MacroDef>;

#[derive(Clone, Debug)]
struct MacroDef {
    width: i64,
    height: i64,
    _origin_x: i64,
    _origin_y: i64,
}

#[derive(Debug)]
struct RawRecord {
    bytes: Vec<u8>,
    offset: u64,
}

#[derive(Debug)]
struct NetRecord {
    bytes: Vec<u8>,
    offset: u64,
    section: &'static str,
}

#[derive(Clone, Debug)]
struct Placement {
    status: &'static str,
    x: i64,
    y: i64,
    orient: &'static str,
}

#[derive(Clone, Debug)]
struct NetConn {
    instance: Option<String>,
    pin: String,
}

#[derive(Clone, Debug)]
struct NetInfo {
    name: String,
    conns: Vec<NetConn>,
}

struct DefCache {
    components: HashMap<String, String>,
    net_records: Vec<NetRecord>,
}

#[pyclass(frozen, skip_from_py_object)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceBBox {
    #[pyo3(get)]
    pub instance: String,
    #[pyo3(get)]
    pub macro_name: String,
    #[pyo3(get)]
    pub status: String,
    #[pyo3(get)]
    pub orient: String,
    #[pyo3(get)]
    pub llx: i64,
    #[pyo3(get)]
    pub lly: i64,
    #[pyo3(get)]
    pub urx: i64,
    #[pyo3(get)]
    pub ury: i64,
}

#[pyclass(frozen, skip_from_py_object)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetShape {
    #[pyo3(get)]
    pub net: String,
    #[pyo3(get)]
    pub section: String,
    #[pyo3(get)]
    pub route_status: String,
    #[pyo3(get)]
    pub shape_type: String,
    #[pyo3(get)]
    pub layer: String,
    #[pyo3(get)]
    pub width: i64,
    #[pyo3(get)]
    pub x1: i64,
    #[pyo3(get)]
    pub y1: i64,
    #[pyo3(get)]
    pub x2: i64,
    #[pyo3(get)]
    pub y2: i64,
    #[pyo3(get)]
    pub llx: i64,
    #[pyo3(get)]
    pub lly: i64,
    #[pyo3(get)]
    pub urx: i64,
    #[pyo3(get)]
    pub ury: i64,
}

#[pymethods]
impl InstanceBBox {
    fn __repr__(&self) -> String {
        format!(
            "InstanceBBox(instance={:?}, macro_name={:?}, status={:?}, orient={:?}, llx={}, lly={}, urx={}, ury={})",
            self.instance,
            self.macro_name,
            self.status,
            self.orient,
            self.llx,
            self.lly,
            self.urx,
            self.ury
        )
    }

    pub fn as_tuple(&self) -> (String, String, String, String, i64, i64, i64, i64) {
        (
            self.instance.clone(),
            self.macro_name.clone(),
            self.status.clone(),
            self.orient.clone(),
            self.llx,
            self.lly,
            self.urx,
            self.ury,
        )
    }
}

#[pymethods]
impl NetShape {
    fn __repr__(&self) -> String {
        format!(
            "NetShape(net={:?}, section={:?}, route_status={:?}, shape_type={:?}, layer={:?}, width={}, x1={}, y1={}, x2={}, y2={}, llx={}, lly={}, urx={}, ury={})",
            self.net,
            self.section,
            self.route_status,
            self.shape_type,
            self.layer,
            self.width,
            self.x1,
            self.y1,
            self.x2,
            self.y2,
            self.llx,
            self.lly,
            self.urx,
            self.ury
        )
    }
}

#[pyclass(unsendable)]
struct InstanceBBoxIter {
    scanner: DefScanner,
    macros: MacroMap,
    pending: VecDeque<InstanceBBox>,
    done: bool,
}

#[pyclass(unsendable)]
struct NetShapeIter {
    scanner: NetRecordScanner,
    net: Vec<u8>,
    layer: Vec<u8>,
    pending: VecDeque<NetShape>,
    done: bool,
}

#[pymethods]
impl InstanceBBoxIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self) -> PyResult<Option<InstanceBBox>> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                return Ok(Some(item));
            }
            if self.done {
                return Ok(None);
            }
            let batch = self.scanner.next_batch(BATCH_SIZE).map_err(to_py_err)?;
            if batch.is_empty() {
                self.done = true;
                return Ok(None);
            }
            let converted = process_record_batch(&batch, &self.macros).map_err(to_py_err)?;
            self.pending = converted.into();
        }
    }
}

#[pymethods]
impl NetShapeIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self) -> PyResult<Option<NetShape>> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                return Ok(Some(item));
            }
            if self.done {
                return Ok(None);
            }
            let batch = self
                .scanner
                .next_matching_batch(BATCH_SIZE, &self.net)
                .map_err(to_py_err)?;
            if batch.is_empty() {
                self.done = true;
                return Ok(None);
            }
            self.pending = process_net_record_batch(&batch, &self.net, &self.layer)
                .map_err(to_py_err)?
                .into();
        }
    }
}

#[pyfunction]
fn instance_bboxes(lefs: Vec<PathBuf>, def_path: PathBuf) -> PyResult<InstanceBBoxIter> {
    let scanner = DefScanner::from_path(&def_path).map_err(to_py_err)?;
    let (macros, _duplicates) =
        parse_lef_files(&lefs, scanner.dbu_per_micron).map_err(to_py_err)?;
    Ok(InstanceBBoxIter {
        scanner,
        macros,
        pending: VecDeque::new(),
        done: false,
    })
}

#[pyfunction]
fn net_shapes(def_path: PathBuf, net: String, layer: String) -> PyResult<NetShapeIter> {
    Ok(NetShapeIter {
        scanner: NetRecordScanner::from_path(&def_path).map_err(to_py_err)?,
        net: net.into_bytes(),
        layer: layer.into_bytes(),
        pending: VecDeque::new(),
        done: false,
    })
}

#[pyfunction]
fn write_instance_csv(
    py: Python<'_>,
    lefs: Vec<PathBuf>,
    def_path: PathBuf,
    output: Option<PathBuf>,
) -> PyResult<()> {
    py.detach(|| write_instance_csv_impl(&lefs, &def_path, output.as_deref()).map_err(to_py_err))
}

#[pyfunction]
#[pyo3(signature = (lefs, def_path, output, columns = None))]
fn write_instance_table(
    py: Python<'_>,
    lefs: Vec<PathBuf>,
    def_path: PathBuf,
    output: Option<PathBuf>,
    columns: Option<Vec<(String, String)>>,
) -> PyResult<usize> {
    let columns = parse_columns(columns).map_err(PyRuntimeError::new_err)?;
    py.detach(|| {
        write_instance_table_impl(&lefs, &def_path, output.as_deref(), &columns).map_err(to_py_err)
    })
}

#[pyfunction]
#[pyo3(signature = (lefs, def_path, output, columns = None))]
fn write_instance_bbox_csv(
    py: Python<'_>,
    lefs: Vec<PathBuf>,
    def_path: PathBuf,
    output: Option<PathBuf>,
    columns: Option<Vec<(String, String)>>,
) -> PyResult<usize> {
    let columns = parse_columns(columns).map_err(PyRuntimeError::new_err)?;
    py.detach(|| {
        write_instance_bbox_csv_impl(&lefs, &def_path, output.as_deref(), &columns)
            .map_err(to_py_err)
    })
}

#[pyfunction]
#[pyo3(signature = (lefs, def_path, output, batch_size, columns = None))]
fn write_instance_parquet(
    py: Python<'_>,
    lefs: Vec<PathBuf>,
    def_path: PathBuf,
    output: PathBuf,
    batch_size: usize,
    columns: Option<Vec<(String, String)>>,
) -> PyResult<(usize, usize)> {
    let columns = parse_columns(columns).map_err(PyRuntimeError::new_err)?;
    py.detach(|| {
        write_instance_parquet_impl(&lefs, &def_path, &output, batch_size, &columns)
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
) -> PyResult<()> {
    py.detach(|| {
        write_net_shapes_csv_impl(&def_path, &net, &layer, output.as_deref()).map_err(to_py_err)
    })
}

#[pyfunction]
fn write_def_cache(py: Python<'_>, def_path: PathBuf, cache_dir: PathBuf) -> PyResult<()> {
    py.detach(|| write_def_cache_impl(&def_path, &cache_dir).map_err(to_py_err))
}

#[pyfunction]
fn write_net_shapes_csv_from_cache(
    py: Python<'_>,
    cache_dir: PathBuf,
    net: String,
    layer: String,
    output: Option<PathBuf>,
) -> PyResult<()> {
    py.detach(|| {
        write_net_shapes_csv_from_cache_impl(&cache_dir, &net, &layer, output.as_deref())
            .map_err(to_py_err)
    })
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
        write_clock_instances_csv_impl(&def_path, &roots, &clock_cell_patterns, output.as_deref())
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
        write_clock_instances_csv_from_cache_impl(
            &cache_dir,
            &roots,
            &clock_cell_patterns,
            output.as_deref(),
        )
        .map_err(to_py_err)
    })
}

#[pymodule]
fn _def(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<InstanceBBox>()?;
    m.add_class::<InstanceBBoxIter>()?;
    m.add_class::<NetShape>()?;
    m.add_class::<NetShapeIter>()?;
    m.add_function(wrap_pyfunction!(instance_bboxes, m)?)?;
    m.add_function(wrap_pyfunction!(net_shapes, m)?)?;
    m.add_function(wrap_pyfunction!(write_instance_csv, m)?)?;
    m.add_function(wrap_pyfunction!(write_instance_table, m)?)?;
    m.add_function(wrap_pyfunction!(write_instance_bbox_csv, m)?)?;
    m.add_function(wrap_pyfunction!(write_instance_parquet, m)?)?;
    m.add_function(wrap_pyfunction!(write_net_shapes_csv, m)?)?;
    m.add_function(wrap_pyfunction!(write_def_cache, m)?)?;
    m.add_function(wrap_pyfunction!(write_net_shapes_csv_from_cache, m)?)?;
    m.add_function(wrap_pyfunction!(write_clock_instances_csv, m)?)?;
    m.add_function(wrap_pyfunction!(write_clock_instances_csv_from_cache, m)?)?;
    Ok(())
}

fn write_instance_csv_impl(
    lefs: &[PathBuf],
    def_path: &Path,
    output: Option<&Path>,
) -> Result<(), String> {
    let mut scanner = DefScanner::from_path(def_path).map_err(|e| e.to_string())?;
    let (macros, duplicates) = parse_lef_files(lefs, scanner.dbu_per_micron)?;
    if !duplicates.is_empty() {
        eprintln!(
            "warning: duplicate LEF macros, last definition used: {}",
            duplicates.join(", ")
        );
    }

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_csv_stream(&mut scanner, &macros, BufWriter::new(file))
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_csv_stream(&mut scanner, &macros, BufWriter::new(lock))
        }
    }
}

/// Widths for the selected name columns, plus the placed-row count.
///
/// Only columns that are actually selected are measured. This pass tokenizes
/// component records; it needs neither the LEF macros nor the bounding boxes.
fn instance_table_widths(
    def_path: &Path,
    columns: &[ColumnSpec],
) -> Result<(Option<usize>, Option<usize>), String> {
    let mut instance_width = columns
        .iter()
        .find(|column| column.kind == ColumnKind::Instance)
        .map(|column| column.name.len());
    let mut macro_width = columns
        .iter()
        .find(|column| column.kind == ColumnKind::BaseCell)
        .map(|column| column.name.len());
    if instance_width.is_none() && macro_width.is_none() {
        return Ok((None, None));
    }

    let mut scanner = DefScanner::from_path(def_path).map_err(|e| e.to_string())?;
    loop {
        let batch = scanner.next_batch(BATCH_SIZE).map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        for record in &batch {
            if let Some((instance_len, macro_len)) = component_name_lengths(record)? {
                if let Some(width) = instance_width.as_mut() {
                    *width = (*width).max(instance_len);
                }
                if let Some(width) = macro_width.as_mut() {
                    *width = (*width).max(macro_len);
                }
            }
        }
    }
    Ok((instance_width, macro_width))
}

/// Lengths of a placed component's instance and macro names.
///
/// Mirrors `parse_component_record`'s placement rules without looking up the
/// macro: `+ UNPLACED` and records with no placement are skipped.
fn component_name_lengths(record: &RawRecord) -> Result<Option<(usize, usize)>, String> {
    let mut tokens = ByteTokens::new(&record.bytes);
    tokens.expect(b"-", record.offset, "component record")?;
    let instance = tokens
        .next()
        .ok_or_else(|| format!("expected component name near DEF byte {}", record.offset))?;
    let macro_name = tokens
        .next()
        .ok_or_else(|| format!("expected component model near DEF byte {}", record.offset))?;
    let instance_len = token_to_str(instance, record.offset, "component name")?.len();
    let macro_len = token_to_str(macro_name, record.offset, "component model")?.len();

    let mut placed = false;
    while let Some(token) = tokens.next() {
        if token != b"+" {
            continue;
        }
        let Some(keyword) = tokens.next() else {
            break;
        };
        match keyword {
            b"PLACED" | b"FIXED" | b"COVER" => placed = true,
            b"UNPLACED" => return Ok(None),
            _ => {}
        }
    }
    Ok(placed.then_some((instance_len, macro_len)))
}

fn write_instance_table_stream<W: Write>(
    scanner: &mut DefScanner,
    macros: &MacroMap,
    columns: &[ColumnSpec],
    instance_width: Option<usize>,
    macro_width: Option<usize>,
    mut out: W,
) -> Result<usize, String> {
    let widths: Vec<usize> = columns
        .iter()
        .map(|column| match column.kind {
            ColumnKind::Instance => instance_width.unwrap_or(column.name.len()),
            ColumnKind::BaseCell => macro_width.unwrap_or(column.name.len()),
            _ => column.name.len().max(12),
        })
        .collect();
    let line_width = widths.iter().sum::<usize>() + 2 * columns.len().saturating_sub(1);

    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            write!(out, "  ").map_err(|e| e.to_string())?;
        }
        match column.kind {
            ColumnKind::Instance | ColumnKind::BaseCell => {
                write!(out, "{:<width$}", column.name, width = widths[index])
            }
            _ => write!(out, "{:>width$}", column.name, width = widths[index]),
        }
        .map_err(|e| e.to_string())?;
    }
    writeln!(out).map_err(|e| e.to_string())?;
    writeln!(out, "{}", "-".repeat(line_width)).map_err(|e| e.to_string())?;

    let dbu = scanner.dbu_per_micron as f64;
    let mut count = 0usize;
    loop {
        let batch = scanner.next_batch(BATCH_SIZE).map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        let bboxes = process_record_batch(&batch, macros)?;
        let mut buffer = String::with_capacity(bboxes.len() * (line_width + 1));
        for bbox in bboxes {
            for (index, column) in columns.iter().enumerate() {
                if index > 0 {
                    buffer.push_str("  ");
                }
                match column.kind {
                    ColumnKind::Instance => {
                        write!(
                            &mut buffer,
                            "{:<width$}",
                            bbox.instance,
                            width = widths[index]
                        )
                    }
                    ColumnKind::BaseCell => {
                        write!(
                            &mut buffer,
                            "{:<width$}",
                            bbox.macro_name,
                            width = widths[index]
                        )
                    }
                    ColumnKind::Llx => write!(
                        &mut buffer,
                        "{:>width$.3}",
                        bbox.llx as f64 / dbu,
                        width = widths[index]
                    ),
                    ColumnKind::Lly => write!(
                        &mut buffer,
                        "{:>width$.3}",
                        bbox.lly as f64 / dbu,
                        width = widths[index]
                    ),
                    ColumnKind::Urx => write!(
                        &mut buffer,
                        "{:>width$.3}",
                        bbox.urx as f64 / dbu,
                        width = widths[index]
                    ),
                    ColumnKind::Ury => write!(
                        &mut buffer,
                        "{:>width$.3}",
                        bbox.ury as f64 / dbu,
                        width = widths[index]
                    ),
                }
                .map_err(|e| e.to_string())?;
            }
            buffer.push('\n');
            count += 1;
        }
        out.write_all(buffer.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(count)
}

fn write_instance_table_impl(
    lefs: &[PathBuf],
    def_path: &Path,
    output: Option<&Path>,
    columns: &[ColumnSpec],
) -> Result<usize, String> {
    let (instance_width, macro_width) = instance_table_widths(def_path, columns)?;
    let mut scanner = DefScanner::from_path(def_path).map_err(|e| e.to_string())?;
    let (macros, duplicates) = parse_lef_files(lefs, scanner.dbu_per_micron)?;
    if !duplicates.is_empty() {
        eprintln!(
            "warning: duplicate LEF macros, last definition used: {}",
            duplicates.join(", ")
        );
    }

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_instance_table_stream(
                &mut scanner,
                &macros,
                columns,
                instance_width,
                macro_width,
                BufWriter::new(file),
            )
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_instance_table_stream(
                &mut scanner,
                &macros,
                columns,
                instance_width,
                macro_width,
                BufWriter::new(lock),
            )
        }
    }
}

fn write_instance_bbox_csv_stream<W: Write>(
    scanner: &mut DefScanner,
    macros: &MacroMap,
    columns: &[ColumnSpec],
    mut out: W,
) -> Result<usize, String> {
    let mut header = String::new();
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            header.push(',');
        }
        append_csv_field(&mut header, &column.name);
    }
    header.push('\n');
    out.write_all(header.as_bytes())
        .map_err(|e| e.to_string())?;

    let dbu = scanner.dbu_per_micron as f64;
    let mut count = 0usize;
    loop {
        let batch = scanner.next_batch(BATCH_SIZE).map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        let bboxes = process_record_batch(&batch, macros)?;
        let mut buffer = String::with_capacity(bboxes.len() * 64);
        for bbox in bboxes {
            for (index, column) in columns.iter().enumerate() {
                if index > 0 {
                    buffer.push(',');
                }
                match column.kind {
                    ColumnKind::Instance => append_csv_field(&mut buffer, &bbox.instance),
                    ColumnKind::BaseCell => append_csv_field(&mut buffer, &bbox.macro_name),
                    ColumnKind::Llx => append_csv_micron(&mut buffer, bbox.llx as f64 / dbu),
                    ColumnKind::Lly => append_csv_micron(&mut buffer, bbox.lly as f64 / dbu),
                    ColumnKind::Urx => append_csv_micron(&mut buffer, bbox.urx as f64 / dbu),
                    ColumnKind::Ury => append_csv_micron(&mut buffer, bbox.ury as f64 / dbu),
                }
            }
            buffer.push('\n');
            count += 1;
        }
        out.write_all(buffer.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(count)
}

fn write_instance_bbox_csv_impl(
    lefs: &[PathBuf],
    def_path: &Path,
    output: Option<&Path>,
    columns: &[ColumnSpec],
) -> Result<usize, String> {
    let mut scanner = DefScanner::from_path(def_path).map_err(|e| e.to_string())?;
    let (macros, duplicates) = parse_lef_files(lefs, scanner.dbu_per_micron)?;
    if !duplicates.is_empty() {
        eprintln!(
            "warning: duplicate LEF macros, last definition used: {}",
            duplicates.join(", ")
        );
    }

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_instance_bbox_csv_stream(&mut scanner, &macros, columns, BufWriter::new(file))
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_instance_bbox_csv_stream(&mut scanner, &macros, columns, BufWriter::new(lock))
        }
    }
}

fn write_instance_parquet_impl(
    lefs: &[PathBuf],
    def_path: &Path,
    output: &Path,
    batch_size: usize,
    columns: &[ColumnSpec],
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
                ColumnKind::Instance | ColumnKind::BaseCell => DataType::Utf8,
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
    loop {
        let batch = scanner.next_batch(chunk).map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        let bboxes = process_record_batch(&batch, &macros)?;
        if bboxes.is_empty() {
            continue;
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
                    bboxes
                        .iter()
                        .map(|item| item.instance.as_str())
                        .collect::<Vec<_>>(),
                )),
                ColumnKind::BaseCell => Arc::new(StringArray::from(
                    bboxes
                        .iter()
                        .map(|item| item.macro_name.as_str())
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
    }
    let metadata = writer.close().map_err(|e| e.to_string())?;
    Ok((count, metadata.num_row_groups()))
}

fn write_net_shapes_csv_impl(
    def_path: &Path,
    net: &str,
    layer: &str,
    output: Option<&Path>,
) -> Result<(), String> {
    let mut scanner = NetRecordScanner::from_path(def_path).map_err(|e| e.to_string())?;
    let net_bytes = net.as_bytes();
    let layer_bytes = layer.as_bytes();

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_net_shapes_csv_stream(&mut scanner, net_bytes, layer_bytes, BufWriter::new(file))
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_net_shapes_csv_stream(&mut scanner, net_bytes, layer_bytes, BufWriter::new(lock))
        }
    }
}

fn write_def_cache_impl(def_path: &Path, cache_dir: &Path) -> Result<(), String> {
    create_dir_all(cache_dir).map_err(|e| format!("{}: {e}", cache_dir.display()))?;
    write_manifest(cache_dir)?;

    let components_path = cache_dir.join("components.bin");
    let net_data_path = cache_dir.join("nets.dat");
    let net_index_path = cache_dir.join("nets.idx");
    let connectivity_path = cache_dir.join("connectivity.bin");

    let mut components = BufWriter::new(
        File::create(&components_path)
            .map_err(|e| format!("{}: {e}", components_path.display()))?,
    );
    let mut net_data = BufWriter::new(
        File::create(&net_data_path).map_err(|e| format!("{}: {e}", net_data_path.display()))?,
    );
    let mut net_index = BufWriter::new(
        File::create(&net_index_path).map_err(|e| format!("{}: {e}", net_index_path.display()))?,
    );
    let mut connectivity = BufWriter::new(
        File::create(&connectivity_path)
            .map_err(|e| format!("{}: {e}", connectivity_path.display()))?,
    );

    write_header(&mut components, COMPONENTS_MAGIC, 0)?;
    write_header(&mut net_data, NET_DATA_MAGIC, 0)?;
    write_header(&mut net_index, NET_INDEX_MAGIC, 0)?;
    write_header(&mut connectivity, CONNECTIVITY_MAGIC, 0)?;

    let mut lexer = Lexer::new(open_input(def_path).map_err(|e| e.to_string())?);
    let mut component_count = 0u64;
    let mut net_count = 0u64;
    let mut connectivity_count = 0u64;
    let mut data_offset = CACHE_HEADER_LEN;
    let mut net_map: HashMap<String, Vec<u64>> = HashMap::new();

    while let Some(token) = lexer.next_token().map_err(|e| e.to_string())? {
        if token.text.eq_ignore_ascii_case("COMPONENTS") {
            let _count = lexer.next_token().map_err(|e| e.to_string())?;
            expect_semicolon(&mut lexer).map_err(|e| e.to_string())?;
            stream_components_to_cache(&mut lexer, &mut components, &mut component_count)?;
        } else if token.text.eq_ignore_ascii_case("NETS") {
            let _count = lexer.next_token().map_err(|e| e.to_string())?;
            expect_semicolon(&mut lexer).map_err(|e| e.to_string())?;
            stream_nets_to_cache(
                &mut lexer,
                "NETS",
                &mut net_data,
                &mut net_index,
                &mut connectivity,
                &mut net_map,
                &mut data_offset,
                &mut net_count,
                &mut connectivity_count,
            )?;
        } else if token.text.eq_ignore_ascii_case("SPECIALNETS") {
            let _count = lexer.next_token().map_err(|e| e.to_string())?;
            expect_semicolon(&mut lexer).map_err(|e| e.to_string())?;
            stream_nets_to_cache(
                &mut lexer,
                "SPECIALNETS",
                &mut net_data,
                &mut net_index,
                &mut connectivity,
                &mut net_map,
                &mut data_offset,
                &mut net_count,
                &mut connectivity_count,
            )?;
        }
    }

    components.flush().map_err(|e| e.to_string())?;
    net_data.flush().map_err(|e| e.to_string())?;
    net_index.flush().map_err(|e| e.to_string())?;
    connectivity.flush().map_err(|e| e.to_string())?;

    patch_header_count(&components_path, component_count)?;
    patch_header_count(&net_data_path, net_count)?;
    patch_header_count(&net_index_path, net_count)?;
    patch_header_count(&connectivity_path, connectivity_count)?;
    write_net_map_bin_from_map(cache_dir, &net_map)?;
    Ok(())
}

fn write_net_shapes_csv_from_cache_impl(
    cache_dir: &Path,
    net: &str,
    layer: &str,
    output: Option<&Path>,
) -> Result<(), String> {
    read_manifest(cache_dir)?;
    let records = load_net_records_for_net(cache_dir, net)?;
    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_net_shapes_records_csv(
                &records,
                net.as_bytes(),
                layer.as_bytes(),
                BufWriter::new(file),
            )
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_net_shapes_records_csv(
                &records,
                net.as_bytes(),
                layer.as_bytes(),
                BufWriter::new(lock),
            )
        }
    }
}

fn write_clock_instances_csv_impl(
    def_path: &Path,
    roots: &[(String, String, String)],
    clock_cell_patterns: &[String],
    output: Option<&Path>,
) -> Result<(), String> {
    let (components, nets) = collect_def_connectivity(def_path)?;
    let patterns: Vec<String> = if clock_cell_patterns.is_empty() {
        vec!["clk".to_string()]
    } else {
        clock_cell_patterns
            .iter()
            .map(|value| value.to_ascii_lowercase())
            .collect()
    };

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_clock_instances_csv_stream(
                &components,
                &nets,
                roots,
                &patterns,
                BufWriter::new(file),
            )
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_clock_instances_csv_stream(
                &components,
                &nets,
                roots,
                &patterns,
                BufWriter::new(lock),
            )
        }
    }
}

fn write_clock_instances_csv_from_cache_impl(
    cache_dir: &Path,
    roots: &[(String, String, String)],
    clock_cell_patterns: &[String],
    output: Option<&Path>,
) -> Result<(), String> {
    read_manifest(cache_dir)?;
    let components = load_components_bin(cache_dir)?;
    let nets = load_connectivity_bin(cache_dir)?;
    let patterns: Vec<String> = if clock_cell_patterns.is_empty() {
        vec!["clk".to_string()]
    } else {
        clock_cell_patterns
            .iter()
            .map(|value| value.to_ascii_lowercase())
            .collect()
    };

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_clock_instances_csv_stream(
                &components,
                &nets,
                roots,
                &patterns,
                BufWriter::new(file),
            )
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_clock_instances_csv_stream(
                &components,
                &nets,
                roots,
                &patterns,
                BufWriter::new(lock),
            )
        }
    }
}

fn write_csv_stream<W: Write>(
    scanner: &mut DefScanner,
    macros: &MacroMap,
    mut out: W,
) -> Result<(), String> {
    writeln!(out, "instance,macro,status,orient,llx,lly,urx,ury").map_err(|e| e.to_string())?;
    loop {
        let batch = scanner.next_batch(BATCH_SIZE).map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        let bboxes = process_record_batch(&batch, macros)?;
        let mut buffer = String::with_capacity(bboxes.len() * 96);
        for bbox in bboxes {
            append_csv_row(&mut buffer, &bbox);
        }
        out.write_all(buffer.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())
}

fn write_clock_instances_csv_stream<W: Write>(
    components: &HashMap<String, String>,
    nets: &[NetInfo],
    roots: &[(String, String, String)],
    clock_cell_patterns: &[String],
    mut out: W,
) -> Result<(), String> {
    let mut instance_to_nets: HashMap<&str, Vec<usize>> = HashMap::new();
    for (net_index, net) in nets.iter().enumerate() {
        for conn in &net.conns {
            if let Some(instance) = &conn.instance {
                instance_to_nets
                    .entry(instance.as_str())
                    .or_default()
                    .push(net_index);
            }
        }
    }

    writeln!(
        out,
        "clock,root_type,root_name,instance,macro,net,is_clock_cell"
    )
    .map_err(|e| e.to_string())?;

    let mut buffer = String::with_capacity(64 * 1024);
    for (clock, root_type, root_name) in roots {
        let initial_nets = seed_clock_nets(nets, root_type, root_name);
        let rows = traverse_clock_instances(
            components,
            nets,
            &instance_to_nets,
            &initial_nets,
            clock_cell_patterns,
        );
        for (instance, macro_name, net, is_clock_cell) in rows {
            append_csv_field(&mut buffer, clock);
            buffer.push(',');
            append_csv_field(&mut buffer, root_type);
            buffer.push(',');
            append_csv_field(&mut buffer, root_name);
            buffer.push(',');
            append_csv_field(&mut buffer, &instance);
            buffer.push(',');
            append_csv_field(&mut buffer, &macro_name);
            buffer.push(',');
            append_csv_field(&mut buffer, &net);
            let _ = writeln!(&mut buffer, ",{}", is_clock_cell);
        }
    }

    out.write_all(buffer.as_bytes())
        .map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

fn write_net_shapes_csv_stream<W: Write>(
    scanner: &mut NetRecordScanner,
    net: &[u8],
    layer: &[u8],
    mut out: W,
) -> Result<(), String> {
    writeln!(
        out,
        "net,section,route_status,shape_type,layer,width,x1,y1,x2,y2,llx,lly,urx,ury"
    )
    .map_err(|e| e.to_string())?;
    loop {
        let batch = scanner
            .next_matching_batch(BATCH_SIZE, net)
            .map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        let shapes = process_net_record_batch(&batch, net, layer)?;
        let mut buffer = String::with_capacity(shapes.len() * 128);
        for shape in shapes {
            append_net_shape_csv_row(&mut buffer, &shape);
        }
        out.write_all(buffer.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())
}

fn write_net_shapes_records_csv<W: Write>(
    records: &[NetRecord],
    net: &[u8],
    layer: &[u8],
    mut out: W,
) -> Result<(), String> {
    writeln!(
        out,
        "net,section,route_status,shape_type,layer,width,x1,y1,x2,y2,llx,lly,urx,ury"
    )
    .map_err(|e| e.to_string())?;
    let shapes = process_net_record_batch(records, net, layer)?;
    let mut buffer = String::with_capacity(shapes.len() * 128);
    for shape in shapes {
        append_net_shape_csv_row(&mut buffer, &shape);
    }
    out.write_all(buffer.as_bytes())
        .map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

fn process_record_batch(
    batch: &[RawRecord],
    macros: &MacroMap,
) -> Result<Vec<InstanceBBox>, String> {
    batch
        .par_iter()
        .filter_map(|record| match parse_component_record(record, macros) {
            Ok(Some(bbox)) => Some(Ok(bbox)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        })
        .collect()
}

fn process_net_record_batch(
    batch: &[NetRecord],
    net: &[u8],
    layer: &[u8],
) -> Result<Vec<NetShape>, String> {
    let nested: Result<Vec<Vec<NetShape>>, String> = batch
        .par_iter()
        .map(|record| parse_net_shapes_record(record, net, layer))
        .collect();
    Ok(nested?.into_iter().flatten().collect())
}

fn parse_component_record(
    record: &RawRecord,
    macros: &MacroMap,
) -> Result<Option<InstanceBBox>, String> {
    let mut tokens = ByteTokens::new(&record.bytes);
    tokens.expect(b"-", record.offset, "component record")?;
    let instance = tokens
        .next()
        .ok_or_else(|| format!("expected component name near DEF byte {}", record.offset))?;
    let macro_name = tokens
        .next()
        .ok_or_else(|| format!("expected component model near DEF byte {}", record.offset))?;
    let instance_str = token_to_str(instance, record.offset, "component name")?;
    let macro_str = token_to_str(macro_name, record.offset, "component model")?;
    let mut placement = None;

    while let Some(token) = tokens.next() {
        if token != b"+" {
            continue;
        }
        let Some(keyword) = tokens.next() else {
            break;
        };
        match keyword {
            b"PLACED" | b"FIXED" | b"COVER" => {
                tokens.expect(b"(", record.offset, "placement point")?;
                let x = tokens
                    .next_i64(record.offset, "placement x")?
                    .ok_or_else(|| {
                        format!("expected placement x near DEF byte {}", record.offset)
                    })?;
                let y = tokens
                    .next_i64(record.offset, "placement y")?
                    .ok_or_else(|| {
                        format!("expected placement y near DEF byte {}", record.offset)
                    })?;
                tokens.expect(b")", record.offset, "placement point")?;
                let orient = tokens.next().ok_or_else(|| {
                    format!("expected orientation near DEF byte {}", record.offset)
                })?;
                placement = Some(Placement {
                    status: placement_status(keyword),
                    x,
                    y,
                    orient: orientation(orient).ok_or_else(|| {
                        format!(
                            "unsupported orientation '{}' near DEF byte {}",
                            String::from_utf8_lossy(orient),
                            record.offset
                        )
                    })?,
                });
            }
            b"UNPLACED" => return Ok(None),
            _ => {}
        }
    }

    let Some(placement) = placement else {
        return Ok(None);
    };

    let macro_def = macros.get(macro_str).ok_or_else(|| {
        format!(
            "missing LEF macro '{}' for instance '{}' near DEF byte {}",
            macro_str, instance_str, record.offset
        )
    })?;
    let (width, height) = match placement.orient {
        "N" | "S" | "FN" | "FS" => (macro_def.width, macro_def.height),
        "E" | "W" | "FE" | "FW" => (macro_def.height, macro_def.width),
        other => {
            return Err(format!(
                "unsupported orientation '{other}' for instance '{instance_str}' near DEF byte {}",
                record.offset
            ));
        }
    };
    Ok(Some(InstanceBBox {
        instance: instance_str.to_string(),
        macro_name: macro_str.to_string(),
        status: placement.status.to_string(),
        orient: placement.orient.to_string(),
        llx: placement.x,
        lly: placement.y,
        urx: placement.x + width,
        ury: placement.y + height,
    }))
}

fn parse_component_identity(record: &RawRecord) -> Result<(String, String), String> {
    let mut tokens = ByteTokens::new(&record.bytes);
    tokens.expect(b"-", record.offset, "component record")?;
    let instance = tokens
        .next()
        .ok_or_else(|| format!("expected component name near DEF byte {}", record.offset))?;
    let macro_name = tokens
        .next()
        .ok_or_else(|| format!("expected component model near DEF byte {}", record.offset))?;
    Ok((
        token_to_str(instance, record.offset, "component name")?.to_string(),
        token_to_str(macro_name, record.offset, "component model")?.to_string(),
    ))
}

fn parse_net_shapes_record(
    record: &NetRecord,
    target_net: &[u8],
    target_layer: &[u8],
) -> Result<Vec<NetShape>, String> {
    let mut tokens = ByteTokens::new(&record.bytes);
    tokens.expect(b"-", record.offset, "net record")?;
    let net = tokens
        .next()
        .ok_or_else(|| format!("expected net name near DEF byte {}", record.offset))?;
    if !eq_token(net, target_net) {
        return Ok(Vec::new());
    }
    let net_str = token_to_str(net, record.offset, "net name")?;

    let mut shapes = Vec::new();
    let mut current_status: Option<&'static str> = None;
    let mut current_layer: Option<&[u8]> = None;
    let mut current_width = 0i64;
    let mut current_shape = "";
    let mut last_point: Option<(i64, i64)> = None;

    while let Some(token) = tokens.next() {
        if token == b"+" {
            let Some(keyword) = tokens.next() else {
                break;
            };
            match keyword {
                b"ROUTED" | b"FIXED" | b"COVER" | b"NOSHIELD" => {
                    current_status = Some(route_status(keyword));
                    current_layer = tokens.next();
                    current_width = tokens.next_i64_or_rewind().unwrap_or(0);
                    current_shape = "";
                    last_point = None;
                }
                b"SHIELD" => {
                    current_status = Some("SHIELD");
                    let _shield_net = tokens.next();
                    current_layer = tokens.next();
                    current_width = tokens.next_i64_or_rewind().unwrap_or(0);
                    current_shape = "";
                    last_point = None;
                }
                b"SHAPE" => {
                    current_shape = tokens
                        .next()
                        .and_then(|value| std::str::from_utf8(value).ok())
                        .unwrap_or("");
                }
                _ => {}
            }
            continue;
        }

        if token == b"NEW" {
            current_layer = tokens.next();
            current_width = tokens.next_i64_or_rewind().unwrap_or(0);
            current_shape = "";
            last_point = None;
            continue;
        }

        if token == b"(" {
            if current_layer.is_none() || current_status.is_none() {
                tokens.skip_until_close();
                continue;
            }
            let x = tokens
                .next_i64(record.offset, "route point x")?
                .ok_or_else(|| format!("expected route point x near DEF byte {}", record.offset))?;
            let y = tokens
                .next_i64(record.offset, "route point y")?
                .ok_or_else(|| format!("expected route point y near DEF byte {}", record.offset))?;
            tokens.expect(b")", record.offset, "route point")?;
            if let (Some((x1, y1)), Some(layer), Some(status)) =
                (last_point, current_layer, current_status)
            {
                if eq_token(layer, target_layer) {
                    shapes.push(make_net_shape(
                        net_str,
                        record.section,
                        status,
                        current_shape,
                        token_to_str(layer, record.offset, "route layer")?,
                        current_width,
                        x1,
                        y1,
                        x,
                        y,
                    ));
                }
            }
            last_point = Some((x, y));
        }
    }

    Ok(shapes)
}

fn parse_net_connections_record(record: &NetRecord) -> Result<NetInfo, String> {
    let mut tokens = ByteTokens::new(&record.bytes);
    tokens.expect(b"-", record.offset, "net record")?;
    let net = tokens
        .next()
        .ok_or_else(|| format!("expected net name near DEF byte {}", record.offset))?;
    let net_name = token_to_str(net, record.offset, "net name")?.to_string();
    let mut conns = Vec::new();

    while let Some(token) = tokens.next() {
        if token == b"+" {
            break;
        }
        if token != b"(" {
            continue;
        }
        let Some(first) = tokens.next() else {
            break;
        };
        let Some(second) = tokens.next() else {
            break;
        };
        tokens.expect(b")", record.offset, "net connection")?;
        if first == b"PIN" {
            conns.push(NetConn {
                instance: None,
                pin: token_to_str(second, record.offset, "port name")?.to_string(),
            });
        } else {
            conns.push(NetConn {
                instance: Some(token_to_str(first, record.offset, "component name")?.to_string()),
                pin: token_to_str(second, record.offset, "pin name")?.to_string(),
            });
        }
    }

    Ok(NetInfo {
        name: net_name,
        conns,
    })
}

fn make_net_shape(
    net: &str,
    section: &str,
    route_status: &str,
    shape_type: &str,
    layer: &str,
    width: i64,
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
) -> NetShape {
    let low = width / 2;
    let high = width - low;
    let (mut llx, mut urx) = if x1 <= x2 { (x1, x2) } else { (x2, x1) };
    let (mut lly, mut ury) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };
    if x1 == x2 {
        llx -= low;
        urx += high;
    }
    if y1 == y2 {
        lly -= low;
        ury += high;
    }
    if x1 != x2 && y1 != y2 {
        llx -= low;
        urx += high;
        lly -= low;
        ury += high;
    }
    NetShape {
        net: net.to_string(),
        section: section.to_string(),
        route_status: route_status.to_string(),
        shape_type: shape_type.to_string(),
        layer: layer.to_string(),
        width,
        x1,
        y1,
        x2,
        y2,
        llx,
        lly,
        urx,
        ury,
    }
}

fn collect_def_connectivity(
    def_path: &Path,
) -> Result<(HashMap<String, String>, Vec<NetInfo>), String> {
    let cache = collect_def_cache(def_path)?;
    let nets = parse_connectivity_records(&cache.net_records)?;
    Ok((cache.components, nets))
}

fn collect_def_cache(def_path: &Path) -> Result<DefCache, String> {
    let mut lexer = Lexer::new(open_input(def_path).map_err(|e| e.to_string())?);
    let mut components = HashMap::new();
    let mut net_records = Vec::new();

    while let Some(token) = lexer.next_token().map_err(|e| e.to_string())? {
        if token.text.eq_ignore_ascii_case("COMPONENTS") {
            let _count = lexer.next_token().map_err(|e| e.to_string())?;
            expect_semicolon(&mut lexer).map_err(|e| e.to_string())?;
            collect_component_section(&mut lexer, &mut components)?;
        } else if token.text.eq_ignore_ascii_case("NETS") {
            let _count = lexer.next_token().map_err(|e| e.to_string())?;
            expect_semicolon(&mut lexer).map_err(|e| e.to_string())?;
            collect_raw_net_section(&mut lexer, "NETS", &mut net_records)?;
        } else if token.text.eq_ignore_ascii_case("SPECIALNETS") {
            let _count = lexer.next_token().map_err(|e| e.to_string())?;
            expect_semicolon(&mut lexer).map_err(|e| e.to_string())?;
            collect_raw_net_section(&mut lexer, "SPECIALNETS", &mut net_records)?;
        }
    }

    Ok(DefCache {
        components,
        net_records,
    })
}

fn parse_connectivity_records(records: &[NetRecord]) -> Result<Vec<NetInfo>, String> {
    records
        .par_iter()
        .map(parse_net_connections_record)
        .collect()
}

fn collect_component_section<R: Read>(
    lexer: &mut Lexer<R>,
    components: &mut HashMap<String, String>,
) -> Result<(), String> {
    while let Some(token) = lexer.next_token().map_err(|e| e.to_string())? {
        if token.text.eq_ignore_ascii_case("END") {
            let _section = lexer.next_token().map_err(|e| e.to_string())?;
            return Ok(());
        }
        if token.text != "-" {
            continue;
        }
        let offset = token.offset;
        let mut bytes = Vec::with_capacity(96);
        bytes.push(b'-');
        lexer
            .read_until_semicolon(&mut bytes)
            .map_err(|e| e.to_string())?;
        let (instance, macro_name) = parse_component_identity(&RawRecord { bytes, offset })?;
        components.insert(instance, macro_name);
    }
    Ok(())
}

fn collect_raw_net_section<R: Read>(
    lexer: &mut Lexer<R>,
    section: &'static str,
    records: &mut Vec<NetRecord>,
) -> Result<(), String> {
    while let Some(token) = lexer.next_token().map_err(|e| e.to_string())? {
        if token.text.eq_ignore_ascii_case("END") {
            let _section = lexer.next_token().map_err(|e| e.to_string())?;
            return Ok(());
        }
        if token.text != "-" {
            continue;
        }
        let offset = token.offset;
        let mut bytes = Vec::with_capacity(1024);
        bytes.push(b'-');
        lexer
            .read_until_semicolon(&mut bytes)
            .map_err(|e| e.to_string())?;
        records.push(NetRecord {
            bytes,
            offset,
            section,
        });
    }
    Ok(())
}

fn stream_components_to_cache<R: Read, W: Write>(
    lexer: &mut Lexer<R>,
    out: &mut W,
    count: &mut u64,
) -> Result<(), String> {
    while let Some(token) = lexer.next_token().map_err(|e| e.to_string())? {
        if token.text.eq_ignore_ascii_case("END") {
            let _section = lexer.next_token().map_err(|e| e.to_string())?;
            return Ok(());
        }
        if token.text != "-" {
            continue;
        }
        let offset = token.offset;
        let mut bytes = Vec::with_capacity(96);
        bytes.push(b'-');
        lexer
            .read_until_semicolon(&mut bytes)
            .map_err(|e| e.to_string())?;
        let (instance, macro_name) = parse_component_identity(&RawRecord { bytes, offset })?;
        write_string(out, &instance)?;
        write_string(out, &macro_name)?;
        *count += 1;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stream_nets_to_cache<R: Read, W1: Write, W2: Write, W3: Write>(
    lexer: &mut Lexer<R>,
    section: &'static str,
    data: &mut W1,
    index: &mut W2,
    connectivity: &mut W3,
    net_map: &mut HashMap<String, Vec<u64>>,
    data_offset: &mut u64,
    net_count: &mut u64,
    connectivity_count: &mut u64,
) -> Result<(), String> {
    while let Some(token) = lexer.next_token().map_err(|e| e.to_string())? {
        if token.text.eq_ignore_ascii_case("END") {
            let _section = lexer.next_token().map_err(|e| e.to_string())?;
            return Ok(());
        }
        if token.text != "-" {
            continue;
        }
        let offset = token.offset;
        let mut bytes = Vec::with_capacity(1024);
        bytes.push(b'-');
        lexer
            .read_until_semicolon(&mut bytes)
            .map_err(|e| e.to_string())?;

        let record = NetRecord {
            bytes,
            offset,
            section,
        };
        let net_name = record_net_name(&record.bytes, record.offset)?;
        net_map.entry(net_name).or_default().push(*net_count);

        write_u8(index, section_id(record.section))?;
        write_u64(index, record.offset)?;
        write_u64(index, *data_offset)?;
        write_u64(index, record.bytes.len() as u64)?;
        data.write_all(&record.bytes).map_err(|e| e.to_string())?;
        *data_offset += record.bytes.len() as u64;

        let net_info = parse_net_connections_record(&record)?;
        write_net_info(connectivity, &net_info)?;
        *net_count += 1;
        *connectivity_count += 1;
    }
    Ok(())
}

fn seed_clock_nets(nets: &[NetInfo], root_type: &str, root_name: &str) -> Vec<usize> {
    let mut seeds = Vec::new();
    for (index, net) in nets.iter().enumerate() {
        if net.name == root_name {
            seeds.push(index);
            continue;
        }
        for conn in &net.conns {
            match root_type {
                "port" => {
                    if conn.instance.is_none() && conn.pin == root_name {
                        seeds.push(index);
                        break;
                    }
                }
                "pin" => {
                    if let Some((inst, pin)) = split_pin_name(root_name) {
                        if conn.instance.as_deref() == Some(inst) && conn.pin == pin {
                            seeds.push(index);
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    seeds
}

fn traverse_clock_instances(
    components: &HashMap<String, String>,
    nets: &[NetInfo],
    instance_to_nets: &HashMap<&str, Vec<usize>>,
    initial_nets: &[usize],
    patterns: &[String],
) -> Vec<(String, String, String, bool)> {
    let mut queue: VecDeque<usize> = initial_nets.iter().copied().collect();
    let mut seen_nets = vec![false; nets.len()];
    let mut seen_instances: HashMap<String, (String, String, bool)> = HashMap::new();

    while let Some(net_index) = queue.pop_front() {
        if net_index >= nets.len() || seen_nets[net_index] {
            continue;
        }
        seen_nets[net_index] = true;
        let net = &nets[net_index];
        for conn in &net.conns {
            let Some(instance) = &conn.instance else {
                continue;
            };
            let macro_name = components.get(instance).cloned().unwrap_or_default();
            let is_clock_cell = is_clock_cell(&macro_name, patterns);
            seen_instances
                .entry(instance.clone())
                .or_insert_with(|| (macro_name.clone(), net.name.clone(), is_clock_cell));
            if is_clock_cell {
                if let Some(next_nets) = instance_to_nets.get(instance.as_str()) {
                    for next_net in next_nets {
                        if !seen_nets[*next_net] {
                            queue.push_back(*next_net);
                        }
                    }
                }
            }
        }
    }

    let mut rows: Vec<_> = seen_instances
        .into_iter()
        .map(|(instance, (macro_name, net, is_clock_cell))| {
            (instance, macro_name, net, is_clock_cell)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn split_pin_name(name: &str) -> Option<(&str, &str)> {
    name.rsplit_once('/').or_else(|| name.rsplit_once('.'))
}

fn is_clock_cell(macro_name: &str, patterns: &[String]) -> bool {
    let lower = macro_name.to_ascii_lowercase();
    patterns.iter().any(|pattern| lower.contains(pattern))
}

fn write_manifest(cache_dir: &Path) -> Result<(), String> {
    let file = File::create(cache_dir.join("manifest.bin"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("manifest.bin").display()))?;
    let mut out = BufWriter::new(file);
    write_header(&mut out, MANIFEST_MAGIC, 0)?;
    out.flush().map_err(|e| e.to_string())
}

fn read_manifest(cache_dir: &Path) -> Result<(), String> {
    let file = File::open(cache_dir.join("manifest.bin"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("manifest.bin").display()))?;
    let mut input = BufReader::new(file);
    read_header(&mut input, MANIFEST_MAGIC)?;
    Ok(())
}

fn load_components_bin(cache_dir: &Path) -> Result<HashMap<String, String>, String> {
    let file = File::open(cache_dir.join("components.bin"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("components.bin").display()))?;
    let mut input = BufReader::new(file);
    let count = read_header(&mut input, COMPONENTS_MAGIC)?;
    let mut components = HashMap::with_capacity(count.min(usize::MAX as u64) as usize);
    for _ in 0..count {
        let instance = read_string(&mut input)?;
        let macro_name = read_string(&mut input)?;
        components.insert(instance, macro_name);
    }
    Ok(components)
}

fn write_net_map_bin_from_map(
    cache_dir: &Path,
    by_net: &HashMap<String, Vec<u64>>,
) -> Result<(), String> {
    let file = File::create(cache_dir.join("net_index.bin"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("net_index.bin").display()))?;
    let mut out = BufWriter::new(file);
    write_header(&mut out, NET_MAP_MAGIC, by_net.len() as u64)?;
    let mut rows: Vec<_> = by_net.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    for (net, indices) in rows {
        write_string(&mut out, net)?;
        write_u64(&mut out, indices.len() as u64)?;
        for index in indices {
            write_u64(&mut out, *index)?;
        }
    }
    out.flush().map_err(|e| e.to_string())
}

fn load_net_records_for_net(cache_dir: &Path, net: &str) -> Result<Vec<NetRecord>, String> {
    let wanted = load_net_record_indices(cache_dir, net)?;
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let mut index = File::open(cache_dir.join("nets.idx"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("nets.idx").display()))?;
    let count = read_header(&mut index, NET_INDEX_MAGIC)?;
    let mut data = File::open(cache_dir.join("nets.dat"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("nets.dat").display()))?;
    read_header(&mut data, NET_DATA_MAGIC)?;

    let mut records = Vec::with_capacity(wanted.len());
    for record_index in wanted {
        if record_index >= count {
            return Err(format!("cache net index {record_index} out of range"));
        }
        index
            .seek(SeekFrom::Start(
                CACHE_HEADER_LEN + record_index * NET_INDEX_ENTRY_LEN,
            ))
            .map_err(|e| e.to_string())?;
        let section = section_name(read_u8(&mut index)?)?;
        let offset = read_u64(&mut index)?;
        let data_offset = read_u64(&mut index)?;
        let len = read_u64(&mut index)?;
        data.seek(SeekFrom::Start(data_offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![0; len as usize];
        data.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        records.push(NetRecord {
            bytes,
            offset,
            section,
        });
    }
    Ok(records)
}

fn load_net_record_indices(cache_dir: &Path, net: &str) -> Result<Vec<u64>, String> {
    let file = File::open(cache_dir.join("net_index.bin"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("net_index.bin").display()))?;
    let mut input = BufReader::new(file);
    let count = read_header(&mut input, NET_MAP_MAGIC)?;
    for _ in 0..count {
        let name = read_string(&mut input)?;
        let record_count = read_u64(&mut input)?;
        if name.eq_ignore_ascii_case(net) {
            let mut indices = Vec::with_capacity(record_count as usize);
            for _ in 0..record_count {
                indices.push(read_u64(&mut input)?);
            }
            return Ok(indices);
        }
        for _ in 0..record_count {
            let _ = read_u64(&mut input)?;
        }
    }
    Ok(Vec::new())
}

fn write_net_info<W: Write>(out: &mut W, net: &NetInfo) -> Result<(), String> {
    write_string(out, &net.name)?;
    write_u64(out, net.conns.len() as u64)?;
    for conn in &net.conns {
        match &conn.instance {
            Some(instance) => {
                write_u8(out, 1)?;
                write_string(out, instance)?;
            }
            None => {
                write_u8(out, 0)?;
            }
        }
        write_string(out, &conn.pin)?;
    }
    Ok(())
}

fn load_connectivity_bin(cache_dir: &Path) -> Result<Vec<NetInfo>, String> {
    let file = File::open(cache_dir.join("connectivity.bin"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("connectivity.bin").display()))?;
    let mut input = BufReader::new(file);
    let count = read_header(&mut input, CONNECTIVITY_MAGIC)?;
    let mut nets = Vec::with_capacity(count.min(usize::MAX as u64) as usize);
    for _ in 0..count {
        let name = read_string(&mut input)?;
        let conn_count = read_u64(&mut input)?;
        let mut conns = Vec::with_capacity(conn_count.min(usize::MAX as u64) as usize);
        for _ in 0..conn_count {
            let has_instance = read_u8(&mut input)?;
            let instance = if has_instance == 1 {
                Some(read_string(&mut input)?)
            } else {
                None
            };
            let pin = read_string(&mut input)?;
            conns.push(NetConn { instance, pin });
        }
        nets.push(NetInfo { name, conns });
    }
    Ok(nets)
}

fn write_header<W: Write>(out: &mut W, magic: &[u8; 8], count: u64) -> Result<(), String> {
    out.write_all(magic).map_err(|e| e.to_string())?;
    write_u32(out, CACHE_VERSION)?;
    write_u64(out, count)
}

fn read_header<R: Read>(input: &mut R, magic: &[u8; 8]) -> Result<u64, String> {
    let mut got = [0u8; 8];
    input.read_exact(&mut got).map_err(|e| e.to_string())?;
    if &got != magic {
        return Err("invalid cache file magic".to_string());
    }
    let version = read_u32(input)?;
    if version != CACHE_VERSION {
        return Err(format!("unsupported cache version {version}"));
    }
    read_u64(input)
}

fn patch_header_count(path: &Path, count: u64) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.seek(SeekFrom::Start(12)).map_err(|e| e.to_string())?;
    file.write_all(&count.to_le_bytes())
        .map_err(|e| e.to_string())
}

fn write_string<W: Write>(out: &mut W, value: &str) -> Result<(), String> {
    write_u64(out, value.len() as u64)?;
    out.write_all(value.as_bytes()).map_err(|e| e.to_string())
}

fn read_string<R: Read>(input: &mut R) -> Result<String, String> {
    let len = read_u64(input)?;
    let mut bytes = vec![0; len as usize];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

fn write_u8<W: Write>(out: &mut W, value: u8) -> Result<(), String> {
    out.write_all(&[value]).map_err(|e| e.to_string())
}

fn read_u8<R: Read>(input: &mut R) -> Result<u8, String> {
    let mut bytes = [0u8; 1];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes[0])
}

fn write_u32<W: Write>(out: &mut W, value: u32) -> Result<(), String> {
    out.write_all(&value.to_le_bytes())
        .map_err(|e| e.to_string())
}

fn read_u32<R: Read>(input: &mut R) -> Result<u32, String> {
    let mut bytes = [0u8; 4];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(u32::from_le_bytes(bytes))
}

fn write_u64<W: Write>(out: &mut W, value: u64) -> Result<(), String> {
    out.write_all(&value.to_le_bytes())
        .map_err(|e| e.to_string())
}

fn read_u64<R: Read>(input: &mut R) -> Result<u64, String> {
    let mut bytes = [0u8; 8];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(u64::from_le_bytes(bytes))
}

fn section_id(section: &str) -> u8 {
    if section == "SPECIALNETS" { 2 } else { 1 }
}

fn section_name(id: u8) -> Result<&'static str, String> {
    match id {
        1 => Ok("NETS"),
        2 => Ok("SPECIALNETS"),
        _ => Err(format!("invalid cached net section id {id}")),
    }
}

fn record_net_name(bytes: &[u8], offset: u64) -> Result<String, String> {
    let mut tokens = ByteTokens::new(bytes);
    tokens.expect(b"-", offset, "net record")?;
    let net = tokens
        .next()
        .ok_or_else(|| format!("expected net name near DEF byte {offset}"))?;
    Ok(token_to_str(net, offset, "net name")?.to_string())
}

/// Parse cell LEF into the shared micron model, then convert macro size and
/// origin into the DEF's database units for instance bounding boxes. Pin and
/// port geometry is not materialized on this path.
fn parse_lef_files(
    paths: &[PathBuf],
    dbu_per_micron: i64,
) -> Result<(MacroMap, Vec<String>), String> {
    let library = lef_tools_native::parse_library(paths, None, false).map_err(|e| e.to_string())?;
    let mut macros = HashMap::with_capacity(library.macros.len());
    for item in &library.macros {
        macros.insert(
            item.name.clone(),
            MacroDef {
                width: to_dbu(item.width, dbu_per_micron, &item.name, "SIZE width")?,
                height: to_dbu(item.height, dbu_per_micron, &item.name, "SIZE height")?,
                _origin_x: to_dbu(item.origin_x, dbu_per_micron, &item.name, "ORIGIN x")?,
                _origin_y: to_dbu(item.origin_y, dbu_per_micron, &item.name, "ORIGIN y")?,
            },
        );
    }
    Ok((macros, library.duplicates))
}

fn to_dbu(
    microns: f64,
    dbu_per_micron: i64,
    macro_name: &str,
    context: &str,
) -> Result<i64, String> {
    let scaled = microns * dbu_per_micron as f64;
    if !scaled.is_finite() || scaled < i64::MIN as f64 || scaled > i64::MAX as f64 {
        return Err(format!(
            "MACRO {macro_name} {context} out of range: '{microns}'"
        ));
    }
    Ok(scaled.round() as i64)
}

struct DefScanner {
    lexer: Lexer<Box<dyn Read + Send>>,
    dbu_per_micron: i64,
    done: bool,
}

impl DefScanner {
    fn from_path(path: &Path) -> io::Result<Self> {
        let input = open_input(path)?;
        let mut lexer = Lexer::new(input);
        let mut dbu_per_micron = None;
        while let Some(token) = lexer.next_token()? {
            if token.text.eq_ignore_ascii_case("UNITS") {
                parse_def_units(&mut lexer, &mut dbu_per_micron)?;
            } else if token.text.eq_ignore_ascii_case("COMPONENTS") {
                let _count = lexer.next_token()?;
                expect_semicolon(&mut lexer)?;
                return Ok(Self {
                    lexer,
                    dbu_per_micron: dbu_per_micron.unwrap_or(1000),
                    done: false,
                });
            }
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "DEF missing COMPONENTS section",
        ))
    }

    fn next_batch(&mut self, max_items: usize) -> io::Result<Vec<RawRecord>> {
        let mut items = Vec::with_capacity(max_items);
        while items.len() < max_items && !self.done {
            match self.next_record()? {
                Some(item) => items.push(item),
                None => self.done = true,
            }
        }
        Ok(items)
    }

    fn next_record(&mut self) -> io::Result<Option<RawRecord>> {
        loop {
            let token = match self.lexer.next_token()? {
                Some(token) => token,
                None => return Ok(None),
            };
            if token.text.eq_ignore_ascii_case("END") {
                let section = self.lexer.next_token()?;
                if section
                    .as_ref()
                    .is_some_and(|t| t.text.eq_ignore_ascii_case("COMPONENTS"))
                {
                    return Ok(None);
                }
                continue;
            }
            if token.text != "-" {
                continue;
            }
            let offset = token.offset;
            let mut bytes = Vec::with_capacity(96);
            bytes.push(b'-');
            self.lexer.read_until_semicolon(&mut bytes)?;
            return Ok(Some(RawRecord { bytes, offset }));
        }
    }
}

struct NetRecordScanner {
    lexer: Lexer<Box<dyn Read + Send>>,
    section: Option<&'static str>,
    done: bool,
}

impl NetRecordScanner {
    fn from_path(path: &Path) -> io::Result<Self> {
        Ok(Self {
            lexer: Lexer::new(open_input(path)?),
            section: None,
            done: false,
        })
    }

    fn next_matching_batch(&mut self, max_items: usize, net: &[u8]) -> io::Result<Vec<NetRecord>> {
        let mut items = Vec::with_capacity(16.min(max_items));
        while items.len() < max_items && !self.done {
            match self.next_matching_record(net)? {
                Some(item) => items.push(item),
                None => self.done = true,
            }
        }
        Ok(items)
    }

    fn next_matching_record(&mut self, net: &[u8]) -> io::Result<Option<NetRecord>> {
        loop {
            let token = match self.lexer.next_token()? {
                Some(token) => token,
                None => return Ok(None),
            };

            if token.text.eq_ignore_ascii_case("SPECIALNETS") {
                let _count = self.lexer.next_token()?;
                expect_semicolon(&mut self.lexer)?;
                self.section = Some("SPECIALNETS");
                continue;
            }
            if token.text.eq_ignore_ascii_case("NETS") {
                let _count = self.lexer.next_token()?;
                expect_semicolon(&mut self.lexer)?;
                self.section = Some("NETS");
                continue;
            }
            if token.text.eq_ignore_ascii_case("END") {
                let section = self.lexer.next_token()?;
                if section
                    .as_ref()
                    .is_some_and(|t| t.text.eq_ignore_ascii_case("SPECIALNETS"))
                    && self.section == Some("SPECIALNETS")
                {
                    self.section = None;
                    continue;
                }
                if section
                    .as_ref()
                    .is_some_and(|t| t.text.eq_ignore_ascii_case("NETS"))
                    && self.section == Some("NETS")
                {
                    self.section = None;
                    continue;
                }
            }

            let Some(section) = self.section else {
                continue;
            };
            if token.text != "-" {
                continue;
            }
            let offset = token.offset;
            let mut bytes = Vec::with_capacity(1024);
            bytes.push(b'-');
            self.lexer.read_until_semicolon(&mut bytes)?;
            if record_net_matches(&bytes, net) {
                return Ok(Some(NetRecord {
                    bytes,
                    offset,
                    section,
                }));
            }
        }
    }
}

fn parse_def_units<R: Read>(
    lexer: &mut Lexer<R>,
    dbu_per_micron: &mut Option<i64>,
) -> io::Result<()> {
    let distance = lexer.next_token()?;
    let microns = lexer.next_token()?;
    let value = lexer.next_token()?;
    if distance
        .as_ref()
        .is_some_and(|t| t.text.eq_ignore_ascii_case("DISTANCE"))
        && microns
            .as_ref()
            .is_some_and(|t| t.text.eq_ignore_ascii_case("MICRONS"))
    {
        if let Some(token) = value {
            *dbu_per_micron = Some(token.text.parse::<i64>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid DEF UNITS value")
            })?);
        }
    }
    consume_until_semicolon_io(lexer)
}

fn expect_semicolon<R: Read>(lexer: &mut Lexer<R>) -> io::Result<()> {
    expect_text(lexer, ";")
}

fn expect_text<R: Read>(lexer: &mut Lexer<R>, expected: &str) -> io::Result<()> {
    let token = lexer.next_token()?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("expected '{expected}'"),
        )
    })?;
    if token.text == expected {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected '{expected}', got '{}'", token.text),
        ))
    }
}

fn consume_until_semicolon_io<R: Read>(lexer: &mut Lexer<R>) -> io::Result<()> {
    while let Some(token) = lexer.next_token()? {
        if token.text == ";" {
            break;
        }
    }
    Ok(())
}

struct ByteTokens<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ByteTokens<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn next(&mut self) -> Option<&'a [u8]> {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        if self.pos >= self.bytes.len() {
            return None;
        }
        let start = self.pos;
        let byte = self.bytes[self.pos];
        if matches!(byte, b'(' | b')' | b'+' | b';') {
            self.pos += 1;
            return Some(&self.bytes[start..self.pos]);
        }
        if byte == b'-' {
            let next = self.bytes.get(self.pos + 1).copied();
            if !next.is_some_and(|value| value.is_ascii_digit() || value == b'.') {
                self.pos += 1;
                return Some(&self.bytes[start..self.pos]);
            }
        }
        if byte == b'"' {
            self.pos += 1;
            let value_start = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
                self.pos += 1;
            }
            let value_end = self.pos;
            if self.pos < self.bytes.len() {
                self.pos += 1;
            }
            return Some(&self.bytes[value_start..value_end]);
        }
        self.pos += 1;
        while self.pos < self.bytes.len()
            && !self.bytes[self.pos].is_ascii_whitespace()
            && !matches!(self.bytes[self.pos], b'(' | b')' | b'+' | b';')
        {
            self.pos += 1;
        }
        Some(&self.bytes[start..self.pos])
    }

    fn expect(&mut self, expected: &[u8], offset: u64, context: &str) -> Result<(), String> {
        let token = self
            .next()
            .ok_or_else(|| format!("expected {context} near DEF byte {offset}"))?;
        if token == expected {
            Ok(())
        } else {
            Err(format!(
                "expected '{}' for {context}, got '{}' near DEF byte {offset}",
                String::from_utf8_lossy(expected),
                String::from_utf8_lossy(token)
            ))
        }
    }

    fn next_i64(&mut self, offset: u64, context: &str) -> Result<Option<i64>, String> {
        let Some(token) = self.next() else {
            return Ok(None);
        };
        parse_i64_bytes(token)
            .ok_or_else(|| {
                format!(
                    "invalid {context}: '{}' near DEF byte {offset}",
                    String::from_utf8_lossy(token)
                )
            })
            .map(Some)
    }

    fn next_i64_or_rewind(&mut self) -> Option<i64> {
        let original = self.pos;
        let token = self.next()?;
        match parse_i64_bytes(token) {
            Some(value) => Some(value),
            None => {
                self.pos = original;
                None
            }
        }
    }

    fn skip_until_close(&mut self) {
        while let Some(token) = self.next() {
            if token == b")" {
                break;
            }
        }
    }
}

fn record_net_matches(bytes: &[u8], target_net: &[u8]) -> bool {
    let mut tokens = ByteTokens::new(bytes);
    if tokens.next() != Some(&b"-"[..]) {
        return false;
    }
    tokens.next().is_some_and(|net| eq_token(net, target_net))
}

fn eq_token(left: &[u8], right: &[u8]) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn parse_i64_bytes(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() {
        return None;
    }
    let mut index = 0;
    let mut sign = 1i64;
    if bytes[0] == b'-' {
        sign = -1;
        index = 1;
    }
    if index == bytes.len() {
        return None;
    }
    let mut value = 0i64;
    while index < bytes.len() {
        let byte = bytes[index];
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as i64)?;
        index += 1;
    }
    value.checked_mul(sign)
}

fn token_to_str<'a>(token: &'a [u8], offset: u64, context: &str) -> Result<&'a str, String> {
    std::str::from_utf8(token)
        .map_err(|_| format!("invalid UTF-8 in {context} near DEF byte {offset}"))
}

fn placement_status(token: &[u8]) -> &'static str {
    match token {
        b"FIXED" => "FIXED",
        b"COVER" => "COVER",
        _ => "PLACED",
    }
}

fn route_status(token: &[u8]) -> &'static str {
    match token {
        b"FIXED" => "FIXED",
        b"COVER" => "COVER",
        b"NOSHIELD" => "NOSHIELD",
        _ => "ROUTED",
    }
}

fn orientation(token: &[u8]) -> Option<&'static str> {
    match token {
        b"N" => Some("N"),
        b"S" => Some("S"),
        b"E" => Some("E"),
        b"W" => Some("W"),
        b"FN" => Some("FN"),
        b"FS" => Some("FS"),
        b"FE" => Some("FE"),
        b"FW" => Some("FW"),
        _ => None,
    }
}

fn append_csv_row(out: &mut String, bbox: &InstanceBBox) {
    append_csv_field(out, &bbox.instance);
    out.push(',');
    append_csv_field(out, &bbox.macro_name);
    let _ = writeln!(
        out,
        ",{},{},{},{},{},{}",
        bbox.status, bbox.orient, bbox.llx, bbox.lly, bbox.urx, bbox.ury
    );
}

fn append_net_shape_csv_row(out: &mut String, shape: &NetShape) {
    append_csv_field(out, &shape.net);
    out.push(',');
    append_csv_field(out, &shape.section);
    out.push(',');
    append_csv_field(out, &shape.route_status);
    out.push(',');
    append_csv_field(out, &shape.shape_type);
    out.push(',');
    append_csv_field(out, &shape.layer);
    let _ = writeln!(
        out,
        ",{},{},{},{},{},{},{},{},{}",
        shape.width,
        shape.x1,
        shape.y1,
        shape.x2,
        shape.y2,
        shape.llx,
        shape.lly,
        shape.urx,
        shape.ury
    );
}

fn append_csv_field(out: &mut String, value: &str) {
    if value
        .bytes()
        .any(|byte| matches!(byte, b',' | b'"' | b'\n' | b'\r'))
    {
        out.push('"');
        for ch in value.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    } else {
        out.push_str(value);
    }
}

/// Write a micron value the way Python's `str(float)` does: shortest
/// round-trip, but keep the `.0` on integral values.
fn append_csv_micron(out: &mut String, value: f64) {
    let start = out.len();
    let _ = write!(out, "{value}");
    if !out[start..].contains('.') {
        out.push_str(".0");
    }
}

fn to_py_err<E: ToString>(err: E) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one_macro(lef: &str, dbu: i64) -> MacroMap {
        let path = tempfile_path("def_tools_macro_test.lef");
        std::fs::write(&path, lef).unwrap();
        let result = parse_lef_files(std::slice::from_ref(&path), dbu);
        std::fs::remove_file(&path).unwrap();
        result.unwrap().0
    }

    #[test]
    fn parses_lef_macro_size_in_def_dbu() {
        let macros = parse_one_macro(
            "MACRO INV\n  ORIGIN 0 0 ;\n  SIZE 1.2 BY 2.4 ;\nEND INV\n",
            1000,
        );
        let inv = macros.get("INV").unwrap();
        assert_eq!(inv.width, 1200);
        assert_eq!(inv.height, 2400);
    }

    #[test]
    fn parses_negative_macro_origin() {
        let macros = parse_one_macro(
            "MACRO PAD\n  ORIGIN -0.1 -2.5 ;\n  SIZE 10 BY 20 ;\nEND PAD\n",
            1000,
        );
        let pad = macros.get("PAD").unwrap();
        assert_eq!(pad._origin_x, -100);
        assert_eq!(pad._origin_y, -2500);
    }

    #[test]
    fn computes_rotated_bbox() {
        let macros = parse_one_macro("MACRO INV\nSIZE 1.2 BY 2.4 ;\nEND INV\n", 1000);
        let raw = RawRecord {
            bytes: b"- u1 INV + PLACED ( 10 20 ) E".to_vec(),
            offset: 0,
        };
        let bbox = parse_component_record(&raw, &macros).unwrap().unwrap();
        assert_eq!(
            (bbox.llx, bbox.lly, bbox.urx, bbox.ury),
            (10, 20, 2410, 1220)
        );
    }

    #[test]
    fn parses_def_component_tail() {
        let def = "VERSION 5.8 ;\nUNITS DISTANCE MICRONS 1000 ;\nCOMPONENTS 1 ;\n- u1 INV + PLACED ( 10 20 ) FS ;\nEND COMPONENTS\n";
        let tmp = tempfile_path("def_tools_test.def");
        std::fs::write(&tmp, def).unwrap();
        let mut scanner = DefScanner::from_path(&tmp).unwrap();
        let item = scanner.next_record().unwrap().unwrap();
        let bbox = parse_component_record(
            &item,
            &parse_one_macro("MACRO INV\nSIZE 1.2 BY 2.4 ;\nEND INV\n", 1000),
        )
        .unwrap()
        .unwrap();
        std::fs::remove_file(&tmp).unwrap();
        assert_eq!(bbox.instance, "u1");
        assert_eq!(bbox.macro_name, "INV");
        assert_eq!(bbox.status, "PLACED");
        assert_eq!(bbox.orient, "FS");
    }

    #[test]
    fn reads_gzipped_def() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let def = b"VERSION 5.8 ;\nUNITS DISTANCE MICRONS 1000 ;\nCOMPONENTS 1 ;\n- u-1 INV + PLACED ( 10 20 ) N ;\nEND COMPONENTS\n";
        let tmp = tempfile_path("def_tools_test.def.gz");
        let file = File::create(&tmp).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::fast());
        encoder.write_all(def).unwrap();
        encoder.finish().unwrap();

        let mut scanner = DefScanner::from_path(&tmp).unwrap();
        let item = scanner.next_record().unwrap().unwrap();
        let macros = parse_one_macro("MACRO INV\nSIZE 1.2 BY 2.4 ;\nEND INV\n", 1000);
        let bbox = parse_component_record(&item, &macros).unwrap().unwrap();
        std::fs::remove_file(&tmp).unwrap();
        assert_eq!(bbox.instance, "u-1");
        assert_eq!(bbox.macro_name, "INV");
    }

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

        let columns = parse_columns(Some(vec![
            ("instance".to_string(), "name".to_string()),
            ("base_cell".to_string(), "cell".to_string()),
        ]))
        .unwrap();
        let (rows, row_groups) =
            write_instance_parquet_impl(&[lef.clone()], &def, &output, 1024, &columns).unwrap();
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
