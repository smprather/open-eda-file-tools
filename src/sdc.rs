use std::path::PathBuf;

use open_eda_file_tools_core::sdc;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

#[pyfunction]
fn evaluate_sdc(py: Python<'_>, path: PathBuf) -> PyResult<(Vec<Vec<String>>, Option<String>)> {
    py.detach(move || {
        sdc::evaluate(path)
            .map(|evaluation| (evaluation.records, evaluation.error))
            .map_err(|error| PyValueError::new_err(error.to_string()))
    })
}

type ClockRow = (
    String,
    Option<f64>,
    Vec<f64>,
    Vec<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Vec<f64>,
    bool,
);

#[pyfunction]
fn sdc_clocks(py: Python<'_>, path: PathBuf) -> PyResult<Vec<ClockRow>> {
    py.detach(move || {
        let clocks = sdc::clocks(path).map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(clocks
            .into_iter()
            .map(|clock| {
                (
                    clock.name,
                    clock.period,
                    clock.waveform,
                    clock.sources,
                    clock.generated_from,
                    clock.divide_by,
                    clock.multiply_by,
                    clock.edges,
                    clock.invert,
                )
            })
            .collect())
    })
}

pub(crate) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(evaluate_sdc, module)?)?;
    module.add_function(wrap_pyfunction!(sdc_clocks, module)?)
}
