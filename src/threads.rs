//! Python surface of the CPU limit; the state lives in
//! `open_eda_file_tools_core::threads`.

pub use open_eda_file_tools_core::threads::{
    budget, install, install_beside_scanner, install_for, may_overlap,
};

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// Limit native readers to `threads` CPUs; `None` or 0 removes the limit.
#[pyfunction]
#[pyo3(signature = (threads = None))]
fn set_threads(threads: Option<usize>) -> PyResult<()> {
    open_eda_file_tools_core::threads::set_threads(threads)
        .map_err(|err| PyValueError::new_err(err.to_string()))
}

/// The number of CPUs native readers will use.
#[pyfunction]
fn get_threads() -> usize {
    open_eda_file_tools_core::threads::get_threads()
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(set_threads, m)?)?;
    m.add_function(wrap_pyfunction!(get_threads, m)?)?;
    Ok(())
}
