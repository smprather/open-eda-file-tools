//! Python bindings for the SPEF/DSPF reader in `open_eda_file_tools_core::pex`.

use open_eda_file_tools_core::pex::{self, SummaryMode};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use std::path::PathBuf;

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexHeader(pex::PexHeader);

#[pymethods]
impl PexHeader {
    #[getter]
    fn standard(&self) -> &str {
        &self.0.standard
    }

    #[getter]
    fn design(&self) -> &str {
        &self.0.design
    }

    #[getter]
    fn date(&self) -> &str {
        &self.0.date
    }

    #[getter]
    fn vendor(&self) -> &str {
        &self.0.vendor
    }

    #[getter]
    fn program(&self) -> &str {
        &self.0.program
    }

    #[getter]
    fn version(&self) -> &str {
        &self.0.version
    }

    #[getter]
    fn design_flow(&self) -> &str {
        &self.0.design_flow
    }

    #[getter]
    fn divider(&self) -> &str {
        &self.0.divider
    }

    #[getter]
    fn delimiter(&self) -> &str {
        &self.0.delimiter
    }

    #[getter]
    fn bus_delimiter(&self) -> &str {
        &self.0.bus_delimiter
    }

    #[getter]
    fn t_unit(&self) -> &str {
        &self.0.t_unit
    }

    #[getter]
    fn c_unit(&self) -> &str {
        &self.0.c_unit
    }

    #[getter]
    fn r_unit(&self) -> &str {
        &self.0.r_unit
    }

    #[getter]
    fn l_unit(&self) -> &str {
        &self.0.l_unit
    }

    #[getter]
    fn ports(&self) -> &[(String, String)] {
        &self.0.ports
    }
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexConn(pex::PexConn);

#[pymethods]
impl PexConn {
    #[getter]
    fn name(&self) -> &str {
        &self.0.name
    }

    #[getter]
    fn direction(&self) -> &str {
        &self.0.direction
    }

    #[getter]
    fn attrs(&self) -> &[String] {
        &self.0.attrs
    }

    #[getter]
    fn x(&self) -> Option<f64> {
        self.0.x
    }

    #[getter]
    fn y(&self) -> Option<f64> {
        self.0.y
    }
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexCap(pex::PexCap);

#[pymethods]
impl PexCap {
    #[getter]
    fn id(&self) -> &str {
        &self.0.id
    }

    #[getter]
    fn node1(&self) -> &str {
        &self.0.node1
    }

    #[getter]
    fn node2(&self) -> Option<&str> {
        self.0.node2.as_deref()
    }

    #[getter]
    fn value(&self) -> f64 {
        self.0.value
    }

    #[getter]
    fn ext(&self) -> Option<&str> {
        self.0.ext.as_deref()
    }
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexRes(pex::PexRes);

#[pymethods]
impl PexRes {
    #[getter]
    fn id(&self) -> &str {
        &self.0.id
    }

    #[getter]
    fn node1(&self) -> &str {
        &self.0.node1
    }

    #[getter]
    fn node2(&self) -> &str {
        &self.0.node2
    }

    #[getter]
    fn value(&self) -> f64 {
        self.0.value
    }

    #[getter]
    fn ext(&self) -> Option<&str> {
        self.0.ext.as_deref()
    }
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexNet(pex::PexNet);

#[pymethods]
impl PexNet {
    #[getter]
    fn name(&self) -> &str {
        &self.0.name
    }

    #[getter]
    fn total_cap(&self) -> f64 {
        self.0.total_cap
    }

    #[getter]
    fn ground_cap(&self) -> f64 {
        self.0.ground_cap
    }

    #[getter]
    fn coupling_cap(&self) -> f64 {
        self.0.coupling_cap
    }

    #[getter]
    fn conns(&self) -> Vec<PexConn> {
        self.0.conns.iter().cloned().map(PexConn).collect()
    }

    #[getter]
    fn caps(&self) -> Vec<PexCap> {
        self.0.caps.iter().cloned().map(PexCap).collect()
    }

    #[getter]
    fn res(&self) -> Vec<PexRes> {
        self.0.res.iter().cloned().map(PexRes).collect()
    }

    #[getter]
    fn ext(&self) -> Option<&str> {
        self.0.ext.as_deref()
    }
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexSummary(pex::PexSummary);

#[pymethods]
impl PexSummary {
    #[getter]
    fn name(&self) -> &str {
        &self.0.name
    }

    #[getter]
    fn total_cap(&self) -> f64 {
        self.0.total_cap
    }

    #[getter]
    fn ground_cap(&self) -> f64 {
        self.0.ground_cap
    }

    #[getter]
    fn coupling_cap(&self) -> f64 {
        self.0.coupling_cap
    }

    #[getter]
    fn cap_count(&self) -> usize {
        self.0.cap_count
    }

    #[getter]
    fn res_count(&self) -> usize {
        self.0.res_count
    }

    #[getter]
    fn res_total(&self) -> f64 {
        self.0.res_total
    }
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexFile(pex::PexFile);

#[pymethods]
impl PexFile {
    #[getter]
    fn header(&self) -> PexHeader {
        PexHeader(self.0.header.clone())
    }

    #[getter]
    fn nets(&self) -> Vec<PexNet> {
        self.0.nets.iter().cloned().map(PexNet).collect()
    }
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexHistogram(pex::PexHistogram);

#[pymethods]
impl PexHistogram {
    #[getter]
    fn min(&self) -> f64 {
        self.0.min
    }

    #[getter]
    fn max(&self) -> f64 {
        self.0.max
    }

    #[getter]
    fn bounds(&self) -> &[f64] {
        &self.0.bounds
    }

    #[getter]
    fn counts(&self) -> &[u64] {
        &self.0.counts
    }
}

fn summary_mode(mode: &str) -> SummaryMode {
    match mode {
        "stream" | "low-memory" | "low_memory" | "memory" => SummaryMode::Stream,
        _ => SummaryMode::Parallel,
    }
}

#[pyfunction]
#[pyo3(signature = (path, keep_entries=true, mode="auto"))]
fn parse_pex(py: Python<'_>, path: PathBuf, keep_entries: bool, mode: &str) -> PyResult<PexFile> {
    if mode == "stream" && keep_entries {
        return Err(PyRuntimeError::new_err(
            "mode='stream' returns summaries only; use keep_entries=false",
        ));
    }
    py.detach(|| pex::parse(&path, keep_entries))
        .map(PexFile)
        .map_err(|err| PyRuntimeError::new_err(err.to_string()))
}

#[pyfunction]
#[pyo3(signature = (path, mode="auto"))]
fn pex_summaries(py: Python<'_>, path: PathBuf, mode: &str) -> PyResult<Vec<PexSummary>> {
    let mode = summary_mode(mode);
    py.detach(|| pex::summaries(&path, mode).map(|nets| nets.into_iter().map(PexSummary).collect()))
        .map_err(|err| PyRuntimeError::new_err(err.to_string()))
}

#[pyfunction]
#[pyo3(signature = (path, bins=20, mode="auto"))]
fn pex_histogram(py: Python<'_>, path: PathBuf, bins: usize, mode: &str) -> PyResult<PexHistogram> {
    let mode = summary_mode(mode);
    py.detach(|| pex::histogram(&path, bins, mode))
        .map(PexHistogram)
        .map_err(|err| PyRuntimeError::new_err(err.to_string()))
}

#[pyfunction]
fn pex_header(py: Python<'_>, path: PathBuf) -> PyResult<PexHeader> {
    py.detach(|| pex::header(&path))
        .map(PexHeader)
        .map_err(|err| PyRuntimeError::new_err(err.to_string()))
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PexHeader>()?;
    m.add_class::<PexConn>()?;
    m.add_class::<PexCap>()?;
    m.add_class::<PexRes>()?;
    m.add_class::<PexNet>()?;
    m.add_class::<PexSummary>()?;
    m.add_class::<PexFile>()?;
    m.add_class::<PexHistogram>()?;
    m.add_function(wrap_pyfunction!(parse_pex, m)?)?;
    m.add_function(wrap_pyfunction!(pex_summaries, m)?)?;
    m.add_function(wrap_pyfunction!(pex_histogram, m)?)?;
    m.add_function(wrap_pyfunction!(pex_header, m)?)?;
    Ok(())
}
