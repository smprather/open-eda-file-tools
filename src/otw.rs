//! OTW (Open Timing Windows) reader.
//!
//! OTW is this project's own open, line-oriented timing-window format; it is
//! not a dialect of any vendor's TWF. The full specification is `docs/otw.md`.
//!
//! ```text
//! otw 1
//! design top
//! time_unit 1e-9
//! clock clk 10 0 5
//! n clk_leaf_3 c clocks=0 freq=1e8
//! w 0 r c 0.10 0.14 - - 0.02 0.03 - - rise_res=0.59:0.61 rise_slack=0.31:0.37
//! p u_gate CK c
//! p - clk c
//! ```
//!
//! An object (`n` net, `p` pin) is declared exactly once and states its own
//! role, so there is never a per-record conflict to arbitrate. `w` lines are
//! the timing windows of the object declared above them. `clock`, `n`, `p` and
//! `w` lines may end with `key=value` attributes; unknown keys are preserved.

use pyo3::exceptions::{PyOSError, PyValueError};
use pyo3::prelude::*;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::lef::open_input;

pub const OTW_VERSION: &str = "1";
const ROLES: &[u8] = b"cduz";
const WINDOW_VALUES: usize = 8;
const WINDOW_COLUMNS: [&str; WINDOW_VALUES] = [
    "rise_early",
    "rise_late",
    "fall_early",
    "fall_late",
    "rise_slew_min",
    "rise_slew_max",
    "fall_slew_min",
    "fall_slew_max",
];

#[derive(Debug)]
pub enum OtwError {
    Io(String),
    Malformed(String),
}

impl OtwError {
    fn into_py(self) -> PyErr {
        match self {
            OtwError::Io(message) => PyOSError::new_err(message),
            OtwError::Malformed(message) => PyValueError::new_err(message),
        }
    }
}

/// The object a run of `w` lines belongs to.
pub enum Object<'a> {
    Net { name: &'a str },
    Pin { instance: &'a str, pin: &'a str },
}

/// Trailing `key=value` tokens of a line, in file order.
pub type Attrs<'a> = [(&'a str, &'a str)];

/// One `w` line: launching clock index (`None` for `-`, no launching clock),
/// launch edge (`r`, `f` or `-`), window role, 8 values, attributes.
pub struct Window<'a> {
    pub clock: Option<usize>,
    pub edge: &'a str,
    pub role: &'a str,
    pub values: [&'a str; WINDOW_VALUES],
    pub attrs: &'a Attrs<'a>,
}

pub trait Visitor {
    fn object(
        &mut self,
        object: &Object<'_>,
        role: &str,
        attrs: &Attrs<'_>,
    ) -> Result<(), OtwError>;
    fn window(&mut self, window: &Window<'_>) -> Result<(), OtwError>;
}

/// `(name, period, rise_edge, fall_edge, attributes)`.
pub type ClockDecl = (String, f64, f64, f64, Vec<(String, String)>);

#[derive(Default)]
pub struct Header {
    pub design: String,
    pub time_unit: Option<f64>,
    /// Farads per capacitance unit and ohms per resistance unit.
    pub cap_unit: Option<f64>,
    pub res_unit: Option<f64>,
    /// Lower and upper slew measurement thresholds, in percent of the swing.
    pub slew_thresholds: Option<(f64, f64)>,
    /// Declared clocks; a `w` line or a `clocks=` attribute names one by index.
    pub clocks: Vec<ClockDecl>,
    /// Every other header line, `(key, rest of line)`, in file order.
    pub attributes: Vec<(String, String)>,
}

fn role_token<'a>(token: Option<&'a str>, what: &str) -> Result<&'a str, String> {
    match token {
        Some(role) if role.len() == 1 && ROLES.contains(&role.as_bytes()[0]) => Ok(role),
        Some(role) => Err(format!(
            "{what} role must be one of c, d, u, z; found `{role}`"
        )),
        None => Err(format!("{what} has no role")),
    }
}

fn number(token: Option<&str>, what: &str) -> Result<f64, String> {
    let text = token.ok_or_else(|| format!("missing {what}"))?;
    text.parse::<f64>()
        .map_err(|_| format!("{what} is not a number: `{text}`"))
}

fn no_more(tokens: &mut std::str::SplitAsciiWhitespace<'_>, what: &str) -> Result<(), String> {
    match tokens.next() {
        Some(extra) => Err(format!("unexpected token `{extra}` after {what}")),
        None => Ok(()),
    }
}

/// Window attributes that hold a `min:max` range (or one number for both).
const RANGE_ATTRS: [&str; 4] = ["rise_res", "rise_slack", "fall_res", "fall_slack"];

fn check_range(key: &str, value: &str) -> Result<(), String> {
    let (low, high) = value.split_once(':').unwrap_or((value, value));
    number(Some(low), key)?;
    number(Some(high), key)?;
    Ok(())
}

/// The rest of a line must be `key=value` attributes.
fn attributes<'a>(
    tokens: &mut std::str::SplitAsciiWhitespace<'a>,
    what: &str,
) -> Result<Vec<(&'a str, &'a str)>, String> {
    let mut attrs = Vec::new();
    for token in tokens {
        match token.split_once('=') {
            Some((key, value)) if !key.is_empty() && !value.is_empty() => {
                attrs.push((key, value));
            }
            _ => {
                return Err(format!(
                    "unexpected token `{token}` after {what}; only key=value attributes may follow"
                ));
            }
        }
    }
    Ok(attrs)
}

/// Parse a `clocks=0,2` attribute value against the declared clock count.
pub fn clock_indices(value: &str, declared: usize) -> Result<Vec<usize>, String> {
    let mut indices = Vec::new();
    for item in value.split(',') {
        let index = item
            .parse::<usize>()
            .ok()
            .filter(|index| *index < declared)
            .ok_or_else(|| {
                format!(
                    "`clocks={value}`: `{item}` does not name one of the {declared} declared clocks"
                )
            })?;
        if !indices.contains(&index) {
            indices.push(index);
        }
    }
    indices.sort_unstable();
    Ok(indices)
}

fn check_object_attrs(attrs: &Attrs<'_>, declared: usize) -> Result<(), String> {
    for (key, value) in attrs {
        match *key {
            "clocks" => {
                clock_indices(value, declared)?;
            }
            "freq" => {
                let hertz = number(Some(value), "freq")?;
                if !(hertz.is_finite() && hertz >= 0.0) {
                    return Err(format!(
                        "freq must be a non-negative number of hertz: `{value}`"
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Parse `path`, feeding objects and windows to `visitor` in file order.
pub fn parse_otw<V: Visitor>(path: &Path, visitor: &mut V) -> Result<Header, OtwError> {
    let io_err = |err: io::Error| OtwError::Io(format!("{}: {err}", path.display()));
    let mut reader = BufReader::with_capacity(1 << 20, open_input(path).map_err(io_err)?);
    let mut header = Header::default();
    let mut raw = Vec::with_capacity(256);
    let mut line_no = 0u64;
    let mut versioned = false;
    let mut in_body = false;
    // Role of the object the following `w` lines belong to.
    let mut object_role: Option<u8> = None;

    loop {
        raw.clear();
        if reader.read_until(b'\n', &mut raw).map_err(io_err)? == 0 {
            break;
        }
        line_no += 1;
        let fail = |message: String| {
            OtwError::Malformed(format!("{}:{line_no}: {message}", path.display()))
        };
        let line = std::str::from_utf8(&raw)
            .map_err(|_| fail("line is not valid UTF-8".to_string()))?
            .trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_ascii_whitespace();
        let keyword = tokens.next().unwrap_or_default();

        if !versioned {
            if keyword != "otw" {
                return Err(fail(
                    "not an OTW file: the first line must be `otw <version>`".to_string(),
                ));
            }
            match tokens.next() {
                Some(OTW_VERSION) => {}
                Some(version) => {
                    return Err(fail(format!(
                        "unsupported OTW version `{version}` (this reader supports {OTW_VERSION})"
                    )));
                }
                None => return Err(fail("`otw` line has no version".to_string())),
            }
            versioned = true;
            continue;
        }

        match keyword {
            "w" => {
                let Some(owner) = object_role else {
                    return Err(fail("`w` line before any `n` or `p` object".to_string()));
                };
                let clock_text = tokens
                    .next()
                    .ok_or_else(|| fail("`w` line has no clock index".to_string()))?;
                let clock = if clock_text == "-" {
                    None
                } else {
                    Some(
                        clock_text
                            .parse::<usize>()
                            .ok()
                            .filter(|index| *index < header.clocks.len())
                            .ok_or_else(|| {
                                fail(format!(
                                    "`w` clock index `{clock_text}` does not name one of the {} declared clocks",
                                    header.clocks.len()
                                ))
                            })?,
                    )
                };
                let edge = match (tokens.next(), clock) {
                    (Some(edge @ ("r" | "f")), Some(_)) => edge,
                    (Some(edge @ "-"), None) => edge,
                    (other, Some(_)) => {
                        return Err(fail(format!(
                            "`w` launch edge must be r or f; found `{}`",
                            other.unwrap_or("")
                        )));
                    }
                    (other, None) => {
                        return Err(fail(format!(
                            "`w` with no launching clock (`-`) takes edge `-`; found `{}`",
                            other.unwrap_or("")
                        )));
                    }
                };
                let role = role_token(tokens.next(), "`w`").map_err(fail)?;
                if role == "c" && owner != b'c' {
                    return Err(fail(format!(
                        "clock window on an object declared `{}`: an object with any clock window must be declared `c`",
                        owner as char
                    )));
                }
                let mut values = [""; WINDOW_VALUES];
                for (slot, column) in values.iter_mut().zip(WINDOW_COLUMNS) {
                    *slot = tokens
                        .next()
                        .ok_or_else(|| fail(format!("`w` line is missing {column}")))?;
                }
                let attrs = attributes(&mut tokens, "the eight window values").map_err(fail)?;
                for (key, value) in &attrs {
                    if RANGE_ATTRS.contains(key) {
                        check_range(key, value).map_err(fail)?;
                    }
                }
                visitor.window(&Window {
                    clock,
                    edge,
                    role,
                    values,
                    attrs: &attrs,
                })?;
            }
            "n" => {
                in_body = true;
                let name = tokens
                    .next()
                    .ok_or_else(|| fail("`n` line has no net name".to_string()))?;
                let role = role_token(tokens.next(), "`n`").map_err(fail)?;
                let attrs = attributes(&mut tokens, "the role").map_err(fail)?;
                check_object_attrs(&attrs, header.clocks.len()).map_err(fail)?;
                object_role = Some(role.as_bytes()[0]);
                visitor
                    .object(&Object::Net { name }, role, &attrs)
                    .map_err(|err| relabel(err, path, line_no))?;
            }
            "p" => {
                in_body = true;
                let instance = tokens
                    .next()
                    .ok_or_else(|| fail("`p` line has no instance".to_string()))?;
                let pin = tokens
                    .next()
                    .ok_or_else(|| fail("`p` line has no pin name".to_string()))?;
                let role = role_token(tokens.next(), "`p`").map_err(fail)?;
                let attrs = attributes(&mut tokens, "the role").map_err(fail)?;
                check_object_attrs(&attrs, header.clocks.len()).map_err(fail)?;
                object_role = Some(role.as_bytes()[0]);
                visitor
                    .object(&Object::Pin { instance, pin }, role, &attrs)
                    .map_err(|err| relabel(err, path, line_no))?;
            }
            _ if in_body => {
                return Err(fail(format!(
                    "unknown record `{keyword}`; header lines must come before the first object"
                )));
            }
            "design" => {
                header.design = tokens
                    .next()
                    .ok_or_else(|| fail("`design` line has no name".to_string()))?
                    .to_string();
            }
            "time_unit" | "cap_unit" | "res_unit" => {
                let value = number(tokens.next(), keyword).map_err(fail)?;
                if !(value.is_finite() && value > 0.0) {
                    return Err(fail(format!("{keyword} must be a positive number")));
                }
                no_more(&mut tokens, keyword).map_err(fail)?;
                match keyword {
                    "time_unit" => header.time_unit = Some(value),
                    "cap_unit" => header.cap_unit = Some(value),
                    _ => header.res_unit = Some(value),
                }
            }
            "slew_thresholds" => {
                let low = number(tokens.next(), "lower slew threshold").map_err(fail)?;
                let high = number(tokens.next(), "upper slew threshold").map_err(fail)?;
                no_more(&mut tokens, "the slew thresholds").map_err(fail)?;
                if !(0.0 <= low && low < high && high <= 100.0) {
                    return Err(fail(
                        "slew_thresholds must be two percentages with 0 <= low < high <= 100"
                            .to_string(),
                    ));
                }
                header.slew_thresholds = Some((low, high));
            }
            "clock" => {
                let name = tokens
                    .next()
                    .ok_or_else(|| fail("`clock` line has no name".to_string()))?
                    .to_string();
                let period = number(tokens.next(), "clock period").map_err(fail)?;
                let rise = number(tokens.next(), "clock rise edge").map_err(fail)?;
                let fall = number(tokens.next(), "clock fall edge").map_err(fail)?;
                let attrs = attributes(&mut tokens, "the clock fall edge")
                    .map_err(fail)?
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect();
                header.clocks.push((name, period, rise, fall, attrs));
            }
            _ => {
                let rest = line[keyword.len()..].trim().to_string();
                header.attributes.push((keyword.to_string(), rest));
            }
        }
    }

    if !versioned {
        return Err(OtwError::Malformed(format!(
            "{}: not an OTW file: the first line must be `otw <version>`",
            path.display()
        )));
    }
    Ok(header)
}

/// Visitors report problems without knowing where they are; add the location.
fn relabel(err: OtwError, path: &Path, line_no: u64) -> OtwError {
    match err {
        OtwError::Malformed(message) => {
            OtwError::Malformed(format!("{}:{line_no}: {message}", path.display()))
        }
        other => other,
    }
}

/// `(role, clock domains, other attributes)` of one object. The domains are
/// the object's `clocks=` attribute, or failing that the clocks of its windows.
type ObjectInfo = (String, Vec<usize>, Vec<(String, String)>);

type NetRow = (String, String, Vec<usize>, Vec<(String, String)>);
type PinRow = (String, String, String, Vec<usize>, Vec<(String, String)>);

/// Per-object view of an OTW file.
#[pyclass(get_all, frozen)]
pub struct OtwSummary {
    design: String,
    time_unit: Option<f64>,
    cap_unit: Option<f64>,
    res_unit: Option<f64>,
    slew_thresholds: Option<(f64, f64)>,
    clocks: Vec<ClockDecl>,
    attributes: Vec<(String, String)>,
    /// `(net, role, clock domains, attributes)` in file order.
    nets: Vec<NetRow>,
    /// `(instance, pin, role, clock domains, attributes)` in file order;
    /// instance `-` is a top-level port.
    pins: Vec<PinRow>,
    /// Number of `w` lines.
    windows: u64,
}

#[derive(Default)]
struct Collector {
    nets: Vec<(String, ObjectInfo)>,
    pins: Vec<(String, String, ObjectInfo)>,
    seen_nets: HashSet<String>,
    seen_pins: HashSet<(String, String)>,
    windows: u64,
    /// The current object is a net (else a pin), and has no `clocks=`.
    current_is_net: bool,
    derive_domains: bool,
}

impl Collector {
    fn info(&mut self, role: &str, attrs: &Attrs<'_>) -> Result<ObjectInfo, OtwError> {
        let mut domains = Vec::new();
        let mut others = Vec::new();
        self.derive_domains = true;
        for (key, value) in attrs {
            if *key == "clocks" {
                // Already validated by the parser against the declared clocks.
                domains = clock_indices(value, usize::MAX).map_err(OtwError::Malformed)?;
                self.derive_domains = false;
            } else {
                others.push(((*key).to_string(), (*value).to_string()));
            }
        }
        Ok((role.to_string(), domains, others))
    }
}

impl Visitor for Collector {
    fn object(
        &mut self,
        object: &Object<'_>,
        role: &str,
        attrs: &Attrs<'_>,
    ) -> Result<(), OtwError> {
        let info = self.info(role, attrs)?;
        match object {
            Object::Net { name } => {
                if !self.seen_nets.insert((*name).to_string()) {
                    return Err(OtwError::Malformed(format!(
                        "net `{name}` is declared more than once"
                    )));
                }
                self.current_is_net = true;
                self.nets.push(((*name).to_string(), info));
            }
            Object::Pin { instance, pin } => {
                if !self
                    .seen_pins
                    .insert(((*instance).to_string(), (*pin).to_string()))
                {
                    return Err(OtwError::Malformed(format!(
                        "pin `{instance} {pin}` is declared more than once"
                    )));
                }
                self.current_is_net = false;
                self.pins
                    .push(((*instance).to_string(), (*pin).to_string(), info));
            }
        }
        Ok(())
    }

    fn window(&mut self, window: &Window<'_>) -> Result<(), OtwError> {
        self.windows += 1;
        if let (true, Some(clock)) = (self.derive_domains, window.clock) {
            let domains = if self.current_is_net {
                self.nets.last_mut().map(|(_, info)| &mut info.1)
            } else {
                self.pins.last_mut().map(|(_, _, info)| &mut info.1)
            };
            if let Some(domains) = domains
                && let Err(position) = domains.binary_search(&clock)
            {
                domains.insert(position, clock);
            }
        }
        Ok(())
    }
}

pub fn summarize(path: &Path) -> Result<OtwSummary, OtwError> {
    let mut collector = Collector::default();
    let header = parse_otw(path, &mut collector)?;
    Ok(OtwSummary {
        design: header.design,
        time_unit: header.time_unit,
        cap_unit: header.cap_unit,
        res_unit: header.res_unit,
        slew_thresholds: header.slew_thresholds,
        clocks: header.clocks,
        attributes: header.attributes,
        nets: collector
            .nets
            .into_iter()
            .map(|(name, (role, domains, attrs))| (name, role, domains, attrs))
            .collect(),
        pins: collector
            .pins
            .into_iter()
            .map(|(instance, pin, (role, domains, attrs))| (instance, pin, role, domains, attrs))
            .collect(),
        windows: collector.windows,
    })
}

fn csv_field(out: &mut String, value: &str) {
    if value.contains([',', '"', '\n', '\r']) {
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

fn join_attrs(attrs: &Attrs<'_>, skip: &[&str]) -> String {
    let mut text = String::new();
    for (key, value) in attrs {
        if skip.contains(key) {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(key);
        text.push('=');
        text.push_str(value);
    }
    text
}

/// Columns after the object prefix: clock, edge, window_role, the eight
/// values, the four range attributes, window_attrs.
const WINDOW_CSV_COLUMNS: usize = 3 + WINDOW_VALUES + RANGE_ATTRS.len() + 1;

/// Streams one CSV row per window; an object without windows gets one row.
struct CsvDump<W: Write> {
    out: W,
    prefix: String,
    pending: bool,
    line: String,
    rows: u64,
}

impl<W: Write> CsvDump<W> {
    fn emit(&mut self, window: Option<&Window<'_>>) -> Result<(), OtwError> {
        self.line.clear();
        self.line.push_str(&self.prefix);
        match window {
            Some(window) => {
                use std::fmt::Write as _;
                self.line.push(',');
                if let Some(clock) = window.clock {
                    let _ = write!(self.line, "{clock}");
                }
                self.line.push(',');
                if window.edge != "-" {
                    self.line.push_str(window.edge);
                }
                self.line.push(',');
                self.line.push_str(window.role);
                for value in window.values {
                    self.line.push(',');
                    if value != "-" {
                        self.line.push_str(value);
                    }
                }
                for key in RANGE_ATTRS {
                    self.line.push(',');
                    if let Some((_, value)) = window.attrs.iter().find(|(name, _)| *name == key) {
                        self.line.push_str(value);
                    }
                }
                self.line.push(',');
                csv_field(&mut self.line, &join_attrs(window.attrs, &RANGE_ATTRS));
            }
            None => self.line.push_str(&",".repeat(WINDOW_CSV_COLUMNS)),
        }
        self.line.push('\n');
        self.rows += 1;
        self.out
            .write_all(self.line.as_bytes())
            .map_err(|err| OtwError::Io(err.to_string()))
    }

    fn flush_pending(&mut self) -> Result<(), OtwError> {
        if self.pending {
            self.pending = false;
            self.emit(None)?;
        }
        Ok(())
    }
}

impl<W: Write> Visitor for CsvDump<W> {
    fn object(
        &mut self,
        object: &Object<'_>,
        role: &str,
        attrs: &Attrs<'_>,
    ) -> Result<(), OtwError> {
        self.flush_pending()?;
        self.prefix.clear();
        match object {
            Object::Net { name } => {
                self.prefix.push_str("net,,");
                csv_field(&mut self.prefix, name);
            }
            Object::Pin { instance, pin } => {
                self.prefix.push_str("pin,");
                csv_field(&mut self.prefix, instance);
                self.prefix.push(',');
                csv_field(&mut self.prefix, pin);
            }
        }
        self.prefix.push(',');
        self.prefix.push_str(role);
        self.prefix.push(',');
        csv_field(&mut self.prefix, &join_attrs(attrs, &[]));
        self.pending = true;
        Ok(())
    }

    fn window(&mut self, window: &Window<'_>) -> Result<(), OtwError> {
        self.pending = false;
        self.emit(Some(window))
    }
}

fn write_windows_csv<W: Write>(path: &Path, mut out: W) -> Result<u64, OtwError> {
    let io_err = |err: io::Error| OtwError::Io(err.to_string());
    let mut head = String::from("kind,instance,name,role,object_attrs,clock,edge,window_role");
    for column in WINDOW_COLUMNS {
        head.push(',');
        head.push_str(column);
    }
    for column in RANGE_ATTRS {
        head.push(',');
        head.push_str(column);
    }
    head.push_str(",window_attrs\n");
    out.write_all(head.as_bytes()).map_err(io_err)?;
    let mut dump = CsvDump {
        out,
        prefix: String::new(),
        pending: false,
        line: String::new(),
        rows: 0,
    };
    parse_otw(path, &mut dump)?;
    dump.flush_pending()?;
    dump.out.flush().map_err(io_err)?;
    Ok(dump.rows)
}

/// Read an OTW file (`.otw` or `.otw.gz`) into its per-object summary.
#[pyfunction]
fn read_otw(py: Python<'_>, path: PathBuf) -> PyResult<OtwSummary> {
    py.detach(|| summarize(&path)).map_err(OtwError::into_py)
}

/// Dump every object and window as CSV; returns the row count.
#[pyfunction]
#[pyo3(signature = (path, output = None))]
fn write_otw_windows_csv(py: Python<'_>, path: PathBuf, output: Option<PathBuf>) -> PyResult<u64> {
    py.detach(|| match &output {
        Some(target) => {
            let file = File::create(target)
                .map_err(|err| OtwError::Io(format!("{}: {err}", target.display())))?;
            write_windows_csv(&path, BufWriter::new(file))
        }
        None => write_windows_csv(&path, BufWriter::new(io::stdout().lock())),
    })
    .map_err(OtwError::into_py)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<OtwSummary>()?;
    m.add_function(wrap_pyfunction!(read_otw, m)?)?;
    m.add_function(wrap_pyfunction!(write_otw_windows_csv, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(name: &str, text: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("otw-{}-{name}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    fn summary_of(name: &str, text: &str) -> Result<OtwSummary, OtwError> {
        let path = write_temp(name, text);
        let result = summarize(&path);
        std::fs::remove_file(&path).unwrap();
        result
    }

    fn malformed(name: &str, text: &str) -> String {
        match summary_of(name, text) {
            Err(OtwError::Malformed(message)) => message,
            _ => panic!("expected a malformed-file error"),
        }
    }

    const SAMPLE: &str = "\
# produced by a test
otw 1
design top
time_unit 1e-9
cap_unit 1e-12
res_unit 1e3
slew_thresholds 10 90
source unit test
clock clk 10 0 5 source=clk
clock div2 20 0 10 master=0
n clk c clocks=0,1 freq=1e8
w 0 r c 0.10 0.14 - - 0.02 0.03 - - rise_res=0.59:0.61 rise_slack=0.37 x_k=2
w 1 f d 0.50 0.90 0.55 0.95 0.04 0.06 0.04 0.07
n data d
w 1 r d 1 2 1 2 0.1 0.2 0.1 0.2
w - - u 1 2 1 2 0.1 0.2 0.1 0.2
n quiet z
p u_gate CK c
p - clk c
";

    fn text(value: &str) -> String {
        value.to_string()
    }

    #[test]
    fn summary_reads_header_nets_pins_domains_and_attributes() {
        let summary = summary_of("summary.otw", SAMPLE).unwrap();

        assert_eq!(summary.design, "top");
        assert_eq!(summary.time_unit, Some(1e-9));
        assert_eq!(summary.cap_unit, Some(1e-12));
        assert_eq!(summary.res_unit, Some(1e3));
        assert_eq!(summary.slew_thresholds, Some((10.0, 90.0)));
        assert_eq!(
            summary.clocks[1],
            (
                text("div2"),
                20.0,
                0.0,
                10.0,
                vec![(text("master"), text("0"))]
            )
        );
        assert_eq!(
            summary.attributes,
            vec![(text("source"), text("unit test"))]
        );
        assert_eq!(
            summary.nets,
            vec![
                (
                    text("clk"),
                    text("c"),
                    vec![0, 1],
                    vec![(text("freq"), text("1e8"))]
                ),
                // No `clocks=`: the domains are the clocks of its windows.
                (text("data"), text("d"), vec![1], vec![]),
                (text("quiet"), text("z"), vec![], vec![]),
            ]
        );
        assert_eq!(
            summary.pins,
            vec![
                (text("u_gate"), text("CK"), text("c"), vec![], vec![]),
                (text("-"), text("clk"), text("c"), vec![], vec![]),
            ]
        );
        assert_eq!(summary.windows, 4);
    }

    #[test]
    fn csv_dump_has_a_row_per_window_and_per_windowless_object() {
        let path = write_temp("dump.otw", SAMPLE);
        let mut out = Vec::new();
        let rows = write_windows_csv(&path, &mut out).unwrap();
        std::fs::remove_file(&path).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(rows, 7);
        assert_eq!(
            lines[1],
            "net,,clk,c,clocks=0,1 freq=1e8,0,r,c,0.10,0.14,,,0.02,0.03,,,0.59:0.61,0.37,,,x_k=2"
                .replace("clocks=0,1 freq=1e8", "\"clocks=0,1 freq=1e8\"")
        );
        assert_eq!(lines[4], "net,,data,d,,,,u,1,2,1,2,0.1,0.2,0.1,0.2,,,,,");
        assert_eq!(
            lines[5],
            format!("net,,quiet,z,{}", ",".repeat(WINDOW_CSV_COLUMNS))
        );
        assert_eq!(lines.iter().map(|l| l.split(',').count()).min(), Some(21));
    }

    #[test]
    fn rejects_what_it_cannot_read_and_names_the_line() {
        assert!(malformed("a.otw", "(TIMING_WINDOWS\n").contains("not an OTW file"));
        assert!(malformed("b.otw", "otw 2\n").contains("unsupported OTW version"));
        assert!(malformed("c.otw", "otw 1\nn a x\n").contains(":2: `n` role"));
        assert!(malformed("d.otw", "otw 1\nn a d\nn a c\n").contains("more than once"));
        assert!(malformed("e.otw", "otw 1\nw 0 r c 1 2 3 4 5 6 7 8\n").contains("before any"));
        assert!(
            malformed("f.otw", "otw 1\nn a c\nw 0 r c 1 2 3 4 5 6 7 8\n")
                .contains("declared clocks")
        );
        assert!(
            malformed(
                "g.otw",
                "otw 1\nclock k 1 0 0.5\nn a d\nw 0 r c 1 2 3 4 5 6 7 8\n"
            )
            .contains("must be declared `c`")
        );
        assert!(
            malformed("h.otw", "otw 1\nclock k 1 0 0.5\nn a c\nw 0 r c 1 2 3\n")
                .contains("missing fall_late")
        );
        assert!(malformed("i.otw", "otw 1\nn a c\ndesign late\n").contains("unknown record"));
        assert!(malformed("j.otw", "otw 1\nn a c stray\n").contains("key=value"));
        assert!(malformed("k.otw", "otw 1\nn a c clocks=0\n").contains("declared clocks"));
        assert!(malformed("l.otw", "otw 1\nn a c freq=-1\n").contains("non-negative"));
        assert!(
            malformed("m.otw", "otw 1\nn a d\nw - r d 1 2 3 4 5 6 7 8\n")
                .contains("takes edge `-`")
        );
        assert!(
            malformed(
                "n.otw",
                "otw 1\nn a d\nw - - d 1 2 3 4 5 6 7 8 rise_slack=soon\n"
            )
            .contains("rise_slack is not a number")
        );
        assert!(malformed("o.otw", "otw 1\nslew_thresholds 90 10\n").contains("0 <= low < high"));
        assert!(malformed("p.otw", "otw 1\nres_unit 0\n").contains("res_unit must be"));
    }
}
