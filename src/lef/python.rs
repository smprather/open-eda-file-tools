//! Python classes over the core cell-LEF model.

use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use open_eda_file_tools_core::lef::{
    LefError, LibraryData, MacroData, PinData, PortData, RectData, parse_library,
};

fn lef_error_to_py(err: LefError) -> PyErr {
    match err {
        LefError::Io(err) => PyErr::from(err),
        LefError::Value(msg) => PyValueError::new_err(msg),
    }
}

/// Render an optional string the way Python's own `repr` would.
fn optional_repr(value: Option<&str>) -> String {
    match value {
        Some(text) => format!("{text:?}"),
        None => "None".to_string(),
    }
}

#[pyclass(frozen, name = "LefRect", module = "lef_tools")]
pub struct LefRect {
    inner: Arc<RectData>,
}

#[pymethods]
impl LefRect {
    #[getter]
    fn layer(&self) -> String {
        self.inner.layer.clone()
    }

    #[getter]
    fn x0_microns(&self) -> f64 {
        self.inner.x0
    }

    #[getter]
    fn y0_microns(&self) -> f64 {
        self.inner.y0
    }

    #[getter]
    fn x1_microns(&self) -> f64 {
        self.inner.x1
    }

    #[getter]
    fn y1_microns(&self) -> f64 {
        self.inner.y1
    }

    fn __repr__(&self) -> String {
        format!(
            "LefRect(layer={:?}, x0_microns={}, y0_microns={}, x1_microns={}, y1_microns={})",
            self.inner.layer, self.inner.x0, self.inner.y0, self.inner.x1, self.inner.y1
        )
    }
}

#[pyclass(frozen, name = "LefPort", module = "lef_tools")]
pub struct LefPort {
    inner: Arc<PortData>,
}

#[pymethods]
impl LefPort {
    fn rects(&self) -> Vec<LefRect> {
        self.inner
            .rects
            .iter()
            .map(|rect| LefRect {
                inner: Arc::clone(rect),
            })
            .collect()
    }

    #[getter]
    fn has_unsupported_geometry(&self) -> bool {
        self.inner.unsupported
    }

    fn __repr__(&self) -> String {
        format!(
            "LefPort(rects={}, has_unsupported_geometry={})",
            self.inner.rects.len(),
            if self.inner.unsupported {
                "True"
            } else {
                "False"
            }
        )
    }
}

#[pyclass(frozen, name = "LefPin", module = "lef_tools")]
pub struct LefPin {
    inner: Arc<PinData>,
}

#[pymethods]
impl LefPin {
    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }

    #[getter]
    fn direction(&self) -> Option<String> {
        self.inner.direction.clone()
    }

    #[getter]
    #[pyo3(name = "use")]
    fn use_kind(&self) -> Option<String> {
        self.inner.use_kind.clone()
    }

    fn ports(&self) -> Vec<LefPort> {
        self.inner
            .ports
            .iter()
            .map(|port| LefPort {
                inner: Arc::clone(port),
            })
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "LefPin(name={:?}, direction={}, use={}, ports={})",
            self.inner.name,
            optional_repr(self.inner.direction.as_deref()),
            optional_repr(self.inner.use_kind.as_deref()),
            self.inner.ports.len()
        )
    }
}

#[pyclass(frozen, name = "LefMacro", module = "lef_tools")]
pub struct LefMacro {
    inner: Arc<MacroData>,
}

#[pymethods]
impl LefMacro {
    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }

    #[getter]
    fn width_microns(&self) -> f64 {
        self.inner.width
    }

    #[getter]
    fn height_microns(&self) -> f64 {
        self.inner.height
    }

    #[getter]
    fn origin_x_microns(&self) -> f64 {
        self.inner.origin_x
    }

    #[getter]
    fn origin_y_microns(&self) -> f64 {
        self.inner.origin_y
    }

    fn pin_names(&self) -> Vec<String> {
        self.inner.pins.iter().map(|pin| pin.name.clone()).collect()
    }

    fn pins(&self) -> Vec<LefPin> {
        self.inner
            .pins
            .iter()
            .map(|pin| LefPin {
                inner: Arc::clone(pin),
            })
            .collect()
    }

    fn pin(&self, name: &str) -> PyResult<LefPin> {
        match self.inner.pin_index.get(name) {
            Some(&index) => Ok(LefPin {
                inner: Arc::clone(&self.inner.pins[index]),
            }),
            None => Err(PyKeyError::new_err(name.to_string())),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "LefMacro(name={:?}, width_microns={}, height_microns={}, pins={})",
            self.inner.name,
            self.inner.width,
            self.inner.height,
            self.inner.pins.len()
        )
    }
}

#[pyclass(frozen, name = "LefLibrary", module = "lef_tools")]
pub struct LefLibrary {
    inner: Arc<LibraryData>,
}

#[pymethods]
impl LefLibrary {
    fn macro_names(&self) -> Vec<String> {
        self.inner
            .macros
            .iter()
            .map(|item| item.name.clone())
            .collect()
    }

    fn macros(&self) -> Vec<LefMacro> {
        self.inner
            .macros
            .iter()
            .map(|item| LefMacro {
                inner: Arc::clone(item),
            })
            .collect()
    }

    #[pyo3(name = "macro")]
    fn get_macro(&self, name: &str) -> PyResult<LefMacro> {
        match self.inner.macro_index.get(name) {
            Some(&index) => Ok(LefMacro {
                inner: Arc::clone(&self.inner.macros[index]),
            }),
            None => Err(PyKeyError::new_err(name.to_string())),
        }
    }

    fn duplicate_macro_names(&self) -> Vec<String> {
        self.inner.duplicates.clone()
    }

    fn __repr__(&self) -> String {
        format!("LefLibrary(macros={})", self.inner.macros.len())
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn extract_paths(paths: &Bound<'_, PyAny>) -> PyResult<Vec<PathBuf>> {
    if let Ok(single) = paths.extract::<PathBuf>() {
        return Ok(vec![single]);
    }
    paths.extract::<Vec<PathBuf>>().map_err(|_| {
        PyTypeError::new_err("paths must be a path or a sequence of paths".to_string())
    })
}

#[pyfunction]
#[pyo3(signature = (paths, *, macros = None))]
pub fn parse_lef(
    py: Python<'_>,
    paths: &Bound<'_, PyAny>,
    macros: Option<Vec<String>>,
) -> PyResult<LefLibrary> {
    let paths = extract_paths(paths)?;
    if paths.is_empty() {
        return Err(PyValueError::new_err("no LEF paths given"));
    }
    let selection: Option<HashSet<String>> = macros.map(|names| names.into_iter().collect());
    let data = py
        .detach(|| parse_library(&paths, selection.as_ref(), true))
        .map_err(lef_error_to_py)?;
    Ok(LefLibrary {
        inner: Arc::new(data),
    })
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<LefLibrary>()?;
    m.add_class::<LefMacro>()?;
    m.add_class::<LefPin>()?;
    m.add_class::<LefPort>()?;
    m.add_class::<LefRect>()?;
    m.add_function(wrap_pyfunction!(parse_lef, m)?)?;
    Ok(())
}
