//! Public cell-LEF API.
//!
//! Parses `MACRO` bodies from cell LEF libraries into an immutable model kept
//! entirely in LEF microns. The DEF database-unit conversion used by
//! `instance_bboxes` is applied downstream of this model so a cell library
//! never changes meaning based on a design's `UNITS DISTANCE MICRONS` value.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;

use crate::{Lexer, Token, open_input};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum LefError {
    Io(io::Error),
    Value(String),
}

impl fmt::Display for LefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LefError::Io(err) => write!(f, "{err}"),
            LefError::Value(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<LefError> for PyErr {
    fn from(err: LefError) -> PyErr {
        match err {
            LefError::Io(err) => PyErr::from(err),
            LefError::Value(msg) => PyValueError::new_err(msg),
        }
    }
}

type LefResult<T> = Result<T, LefError>;

fn value_error<T>(msg: String) -> LefResult<T> {
    Err(LefError::Value(msg))
}

/// Keep the `io::ErrorKind` so PyO3 still maps to the right `OSError` subclass
/// while the message carries the offending path.
fn io_error(path: &Path, err: io::Error) -> LefError {
    LefError::Io(io::Error::new(
        err.kind(),
        format!("{}: {err}", path.display()),
    ))
}

/// `"cells.lef: MACRO BUF "` or `"cells.lef: MACRO BUF PIN A: "`.
fn context(path: &Path, macro_name: &str, pin_name: Option<&str>) -> String {
    match pin_name {
        Some(pin) => format!("{}: MACRO {macro_name} PIN {pin}: ", path.display()),
        None => format!("{}: MACRO {macro_name} ", path.display()),
    }
}

// ---------------------------------------------------------------------------
// Micron-domain model
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct RectData {
    layer: String,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

#[derive(Debug, Default)]
pub(crate) struct PortData {
    rects: Vec<Arc<RectData>>,
    unsupported: bool,
}

#[derive(Debug)]
pub(crate) struct PinData {
    name: String,
    direction: Option<String>,
    use_kind: Option<String>,
    ports: Vec<Arc<PortData>>,
}

#[derive(Debug)]
pub struct MacroData {
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pins: Vec<Arc<PinData>>,
    pin_index: HashMap<String, usize>,
}

#[derive(Debug, Default)]
pub struct LibraryData {
    pub macros: Vec<Arc<MacroData>>,
    macro_index: HashMap<String, usize>,
    pub duplicates: Vec<String>,
}

impl LibraryData {
    /// Last definition wins; the macro keeps its first-seen position so macro
    /// order stays deterministic across files.
    fn insert(&mut self, data: MacroData) {
        match self.macro_index.get(&data.name) {
            Some(&index) => {
                let name = data.name.clone();
                self.macros[index] = Arc::new(data);
                if !self.duplicates.contains(&name) {
                    self.duplicates.push(name);
                }
            }
            None => {
                self.macro_index
                    .insert(data.name.clone(), self.macros.len());
                self.macros.push(Arc::new(data));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Lexer wrapper with single-token pushback
// ---------------------------------------------------------------------------

struct LefLexer<R: Read> {
    lexer: Lexer<R>,
    pushed: Option<Token>,
}

impl<R: Read> LefLexer<R> {
    fn new(reader: R) -> Self {
        Self {
            lexer: Lexer::new(reader),
            pushed: None,
        }
    }

    fn next(&mut self) -> LefResult<Option<Token>> {
        if let Some(token) = self.pushed.take() {
            return Ok(Some(token));
        }
        self.lexer.next_token().map_err(LefError::Io)
    }

    fn push(&mut self, token: Token) {
        self.pushed = Some(token);
    }

    fn required(&mut self, missing: impl FnOnce() -> String) -> LefResult<Token> {
        match self.next()? {
            Some(token) => Ok(token),
            None => value_error(missing()),
        }
    }

    fn consume_statement(&mut self, missing: impl Fn() -> String) -> LefResult<()> {
        loop {
            match self.next()? {
                Some(token) if token.text == ";" => return Ok(()),
                Some(_) => continue,
                None => return value_error(missing()),
            }
        }
    }

    /// LEF closes `MACRO`/`PIN` groups with `END <name>` and `PORT`/`OBS`
    /// groups with a bare `END`. Some writers omit the name, so only consume a
    /// following token when it actually matches.
    fn consume_end_name(&mut self, expected: Option<&str>) -> LefResult<()> {
        let Some(expected) = expected else {
            return Ok(());
        };
        if let Some(token) = self.next()?
            && token.text != expected
        {
            self.push(token);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parse `paths` in order.
///
/// `selection` limits which `MACRO` bodies are materialized; unselected bodies
/// are skipped without allocating their contents. `collect_pins` is false for
/// the DEF instance-bounding-box path, which needs only `SIZE` and `ORIGIN`.
///
/// An empty `paths` slice yields an empty library here; rejecting it is the
/// public `parse_lef` entry point's job, so the DEF path keeps its old behavior.
pub fn parse_library(
    paths: &[PathBuf],
    selection: Option<&HashSet<String>>,
    collect_pins: bool,
) -> LefResult<LibraryData> {
    let mut library = LibraryData::default();
    for path in paths {
        let input = open_input(path).map_err(|err| io_error(path, err))?;
        let mut lexer = LefLexer::new(input);
        while let Some(token) = lexer.next()? {
            if !token.text.eq_ignore_ascii_case("MACRO") {
                continue;
            }
            let name = lexer
                .required(|| format!("{}: expected macro name after MACRO", path.display()))?
                .text;
            let selected = selection.is_none_or(|wanted| wanted.contains(&name));
            if selected {
                let data = parse_macro_body(&mut lexer, path, &name, collect_pins)?;
                library.insert(data);
            } else {
                consume_group(&mut lexer, Some(&name), path, "MACRO", &name)?;
            }
        }
    }
    Ok(library)
}

fn parse_macro_body<R: Read>(
    lexer: &mut LefLexer<R>,
    path: &Path,
    macro_name: &str,
    collect_pins: bool,
) -> LefResult<MacroData> {
    let ctx = || context(path, macro_name, None);
    let mut width: Option<f64> = None;
    let mut height: Option<f64> = None;
    let mut origin_x = 0.0f64;
    let mut origin_y = 0.0f64;
    let mut pins: Vec<Arc<PinData>> = Vec::new();
    let mut pin_index: HashMap<String, usize> = HashMap::new();

    loop {
        let Some(token) = lexer.next()? else {
            return value_error(format!("{}missing END", ctx()));
        };
        if token.text.eq_ignore_ascii_case("END") {
            lexer.consume_end_name(Some(macro_name))?;
            break;
        }
        if token.text.eq_ignore_ascii_case("PIN") {
            let pin_name = lexer
                .required(|| format!("{}expected pin name after PIN", ctx()))?
                .text;
            if collect_pins {
                let pin = parse_pin_body(lexer, path, macro_name, &pin_name)?;
                if pin_index.contains_key(&pin.name) {
                    return value_error(format!("{}duplicate PIN {}", ctx(), pin.name));
                }
                pin_index.insert(pin.name.clone(), pins.len());
                pins.push(Arc::new(pin));
            } else {
                consume_group(lexer, Some(&pin_name), path, "PIN", &pin_name)?;
            }
            continue;
        }
        if token.text.eq_ignore_ascii_case("OBS") || token.text.eq_ignore_ascii_case("DENSITY") {
            consume_group(lexer, None, path, &token.text, macro_name)?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("ORIGIN") {
            origin_x = parse_number(lexer, &ctx, "ORIGIN x")?;
            origin_y = parse_number(lexer, &ctx, "ORIGIN y")?;
            lexer.consume_statement(|| format!("{}unterminated ORIGIN", ctx()))?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("SIZE") {
            let parsed_width = parse_number(lexer, &ctx, "SIZE width")?;
            let by = lexer.required(|| format!("{}expected BY in SIZE", ctx()))?;
            if !by.text.eq_ignore_ascii_case("BY") {
                return value_error(format!("{}expected BY in SIZE, got '{}'", ctx(), by.text));
            }
            let parsed_height = parse_number(lexer, &ctx, "SIZE height")?;
            lexer.consume_statement(|| format!("{}unterminated SIZE", ctx()))?;
            width = Some(parsed_width);
            height = Some(parsed_height);
            continue;
        }
        lexer.consume_statement(|| format!("{}unterminated {} statement", ctx(), token.text))?;
    }

    let (Some(width), Some(height)) = (width, height) else {
        return value_error(format!("{}missing SIZE", ctx()));
    };
    Ok(MacroData {
        name: macro_name.to_string(),
        width,
        height,
        origin_x,
        origin_y,
        pins,
        pin_index,
    })
}

fn parse_pin_body<R: Read>(
    lexer: &mut LefLexer<R>,
    path: &Path,
    macro_name: &str,
    pin_name: &str,
) -> LefResult<PinData> {
    let ctx = || context(path, macro_name, Some(pin_name));
    let mut direction: Option<String> = None;
    let mut use_kind: Option<String> = None;
    let mut ports: Vec<Arc<PortData>> = Vec::new();

    loop {
        let Some(token) = lexer.next()? else {
            return value_error(format!("{}missing END", ctx()));
        };
        if token.text.eq_ignore_ascii_case("END") {
            lexer.consume_end_name(Some(pin_name))?;
            break;
        }
        if token.text.eq_ignore_ascii_case("PORT") {
            ports.push(Arc::new(parse_port_body(lexer, &ctx)?));
            continue;
        }
        if token.text.eq_ignore_ascii_case("DIRECTION") {
            let value = lexer.required(|| format!("{}expected DIRECTION value", ctx()))?;
            direction = Some(value.text.to_ascii_lowercase());
            lexer.consume_statement(|| format!("{}unterminated DIRECTION", ctx()))?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("USE") {
            let value = lexer.required(|| format!("{}expected USE value", ctx()))?;
            use_kind = Some(value.text.to_ascii_lowercase());
            lexer.consume_statement(|| format!("{}unterminated USE", ctx()))?;
            continue;
        }
        lexer.consume_statement(|| format!("{}unterminated {} statement", ctx(), token.text))?;
    }

    Ok(PinData {
        name: pin_name.to_string(),
        direction,
        use_kind,
        ports,
    })
}

fn parse_port_body<R: Read, F: Fn() -> String>(
    lexer: &mut LefLexer<R>,
    ctx: &F,
) -> LefResult<PortData> {
    let mut layer: Option<String> = None;
    let mut rects: Vec<Arc<RectData>> = Vec::new();
    let mut unsupported = false;

    loop {
        let Some(token) = lexer.next()? else {
            return value_error(format!("{}PORT missing END", ctx()));
        };
        if token.text.eq_ignore_ascii_case("END") {
            break;
        }
        if token.text.eq_ignore_ascii_case("LAYER") {
            let value = lexer.required(|| format!("{}expected LAYER name", ctx()))?;
            layer = Some(value.text);
            lexer.consume_statement(|| format!("{}unterminated LAYER", ctx()))?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("RECT") {
            let Some(layer_name) = layer.as_deref() else {
                return value_error(format!("{}RECT appears before LAYER", ctx()));
            };
            let mut first = lexer.required(|| format!("{}truncated RECT", ctx()))?;
            if first.text.eq_ignore_ascii_case("MASK") {
                let _mask = lexer.required(|| format!("{}expected RECT MASK number", ctx()))?;
                first = lexer.required(|| format!("{}truncated RECT", ctx()))?;
            }
            if first.text.eq_ignore_ascii_case("ITERATE") {
                unsupported = true;
                lexer.consume_statement(|| format!("{}unterminated RECT", ctx()))?;
                continue;
            }
            let x0 = parse_coordinate(&first, ctx)?;
            let y0 = parse_rect_coordinate(lexer, ctx)?;
            let x1 = parse_rect_coordinate(lexer, ctx)?;
            let y1 = parse_rect_coordinate(lexer, ctx)?;
            lexer.consume_statement(|| format!("{}unterminated RECT", ctx()))?;
            rects.push(Arc::new(RectData {
                layer: layer_name.to_string(),
                x0: x0.min(x1),
                y0: y0.min(y1),
                x1: x0.max(x1),
                y1: y0.max(y1),
            }));
            continue;
        }
        if token.text.eq_ignore_ascii_case("POLYGON")
            || token.text.eq_ignore_ascii_case("PATH")
            || token.text.eq_ignore_ascii_case("VIA")
        {
            unsupported = true;
            lexer.consume_statement(|| format!("{}unterminated {}", ctx(), token.text))?;
            continue;
        }
        lexer.consume_statement(|| format!("{}unterminated {} statement", ctx(), token.text))?;
    }

    Ok(PortData { rects, unsupported })
}

/// Skip a group without allocating its contents, tracking nested groups so the
/// terminating `END` is matched at the right level.
fn consume_group<R: Read>(
    lexer: &mut LefLexer<R>,
    end_name: Option<&str>,
    path: &Path,
    kind: &str,
    label: &str,
) -> LefResult<()> {
    loop {
        let Some(token) = lexer.next()? else {
            return value_error(format!("{}: unterminated {kind} {label}", path.display()));
        };
        if token.text.eq_ignore_ascii_case("END") {
            lexer.consume_end_name(end_name)?;
            return Ok(());
        }
        if token.text.eq_ignore_ascii_case("PIN") {
            let pin_name = lexer
                .required(|| format!("{}: expected pin name after PIN", path.display()))?
                .text;
            consume_group(lexer, Some(&pin_name), path, "PIN", &pin_name)?;
            continue;
        }
        if token.text.eq_ignore_ascii_case("PORT")
            || token.text.eq_ignore_ascii_case("OBS")
            || token.text.eq_ignore_ascii_case("DENSITY")
        {
            consume_group(lexer, None, path, &token.text, label)?;
            continue;
        }
        lexer.consume_statement(|| {
            format!(
                "{}: unterminated {} statement in {kind} {label}",
                path.display(),
                token.text
            )
        })?;
    }
}

fn parse_number<R: Read, F: Fn() -> String>(
    lexer: &mut LefLexer<R>,
    ctx: &F,
    what: &str,
) -> LefResult<f64> {
    let token = lexer.required(|| format!("{}expected {what}", ctx()))?;
    match token.text.parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(value),
        _ => value_error(format!("{}invalid {what}: '{}'", ctx(), token.text)),
    }
}

fn parse_rect_coordinate<R: Read, F: Fn() -> String>(
    lexer: &mut LefLexer<R>,
    ctx: &F,
) -> LefResult<f64> {
    let token = lexer.required(|| format!("{}truncated RECT", ctx()))?;
    parse_coordinate(&token, ctx)
}

fn parse_coordinate<F: Fn() -> String>(token: &Token, ctx: &F) -> LefResult<f64> {
    match token.text.parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(value),
        _ => value_error(format!(
            "{}malformed RECT coordinate '{}'",
            ctx(),
            token.text
        )),
    }
}

// ---------------------------------------------------------------------------
// Python classes
// ---------------------------------------------------------------------------

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
    let data = py.detach(|| parse_library(&paths, selection.as_ref(), true))?;
    Ok(LefLibrary {
        inner: Arc::new(data),
    })
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<LefLibrary>()?;
    m.add_class::<LefMacro>()?;
    m.add_class::<LefPin>()?;
    m.add_class::<LefPort>()?;
    m.add_class::<LefRect>()?;
    m.add_function(wrap_pyfunction!(parse_lef, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempfile_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "{}_{}_cell_lef_test.lef",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn parse_str(text: &str) -> LibraryData {
        let path = tempfile_path();
        std::fs::write(&path, text).unwrap();
        let result = parse_library(std::slice::from_ref(&path), None, true);
        std::fs::remove_file(&path).unwrap();
        result.unwrap()
    }

    const BUF: &str = "MACRO BUF\n  ORIGIN -0.5 0.25 ;\n  SIZE 5.76 BY 3.33 ;\n  PIN A\n    DIRECTION INPUT ;\n    USE SIGNAL ;\n    PORT\n      LAYER li1 ;\n      RECT 0.125 1.35 1.43 1.78 ;\n    END\n  END A\n  OBS\n    LAYER met1 ;\n    RECT 0 0 1 1 ;\n  END\nEND BUF\n";

    #[test]
    fn parses_macro_geometry_in_microns() {
        let library = parse_str(BUF);
        let macro_data = &library.macros[0];
        assert_eq!(macro_data.name, "BUF");
        assert_eq!(macro_data.width, 5.76);
        assert_eq!(macro_data.height, 3.33);
        assert_eq!(macro_data.origin_x, -0.5);
        assert_eq!(macro_data.origin_y, 0.25);
    }

    #[test]
    fn parses_pin_ports_and_skips_obs() {
        let library = parse_str(BUF);
        let pin = &library.macros[0].pins[0];
        assert_eq!(pin.name, "A");
        assert_eq!(pin.direction.as_deref(), Some("input"));
        assert_eq!(pin.use_kind.as_deref(), Some("signal"));
        let rect = &pin.ports[0].rects[0];
        assert_eq!(rect.layer, "li1");
        assert_eq!(
            (rect.x0, rect.y0, rect.x1, rect.y1),
            (0.125, 1.35, 1.43, 1.78)
        );
        assert!(!pin.ports[0].unsupported);
    }

    #[test]
    fn normalizes_reversed_rect_coordinates() {
        let library = parse_str(
            "MACRO M\n SIZE 1 BY 1 ;\n PIN P\n  PORT\n   LAYER li1 ;\n   RECT 2 3 1 0 ;\n  END\n END P\nEND M\n",
        );
        let rect = &library.macros[0].pins[0].ports[0].rects[0];
        assert_eq!((rect.x0, rect.y0, rect.x1, rect.y1), (1.0, 0.0, 2.0, 3.0));
    }

    #[test]
    fn flags_unsupported_geometry_without_dropping_rects() {
        let library = parse_str(
            "MACRO M\n SIZE 1 BY 1 ;\n PIN P\n  PORT\n   LAYER li1 ;\n   POLYGON 0 0 1 0 1 1 ;\n   RECT 0 0 1 1 ;\n  END\n END P\nEND M\n",
        );
        let port = &library.macros[0].pins[0].ports[0];
        assert!(port.unsupported);
        assert_eq!(port.rects.len(), 1);
    }

    #[test]
    fn reports_rect_before_layer_with_context() {
        let path = tempfile_path();
        std::fs::write(
            &path,
            "MACRO BUF\n SIZE 1 BY 1 ;\n PIN A\n  PORT\n   RECT 0 0 1 1 ;\n  END\n END A\nEND BUF\n",
        )
        .unwrap();
        let err = parse_library(std::slice::from_ref(&path), None, true).unwrap_err();
        std::fs::remove_file(&path).unwrap();
        let message = err.to_string();
        assert!(
            message.contains("MACRO BUF PIN A: RECT appears before LAYER"),
            "{message}"
        );
    }

    #[test]
    fn last_duplicate_macro_wins() {
        let library = parse_str("MACRO M\n SIZE 1 BY 1 ;\nEND M\nMACRO M\n SIZE 2 BY 2 ;\nEND M\n");
        assert_eq!(library.macros.len(), 1);
        assert_eq!(library.macros[0].width, 2.0);
        assert_eq!(library.duplicates, vec!["M".to_string()]);
    }

    #[test]
    fn macro_filter_skips_unselected_bodies() {
        let path = tempfile_path();
        std::fs::write(
            &path,
            "MACRO KEEP\n SIZE 1 BY 1 ;\nEND KEEP\nMACRO DROP\n SIZE 2 BY 2 ;\n PIN P\n  PORT\n   LAYER li1 ;\n   RECT 0 0 1 1 ;\n  END\n END P\nEND DROP\n",
        )
        .unwrap();
        let wanted: HashSet<String> = HashSet::from(["KEEP".to_string()]);
        let library = parse_library(std::slice::from_ref(&path), Some(&wanted), true).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(library.macros.len(), 1);
        assert_eq!(library.macros[0].name, "KEEP");
    }
}
