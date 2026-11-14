//! OTW (Open Timing Windows) reader.
//!
//! OTW is this project's own open, line-oriented timing-window format; it is
//! not a dialect of any vendor's TWF. The normative specification is
//! `docs/otw.md`.
//!
//! ```text
//! otw 1
//! design top
//! time_unit 1e-9
//! clock clk 10 0 5
//! n clk_leaf_3 c clocks=0
//! w 0 r c 0.10 0.14 - - 0.02 0.03 - - rise_res=0.59:0.61
//! i u_gate
//! p CK c
//! p E d
//! i -
//! p clk c
//! end 1 3 1
//! ```
//!
//! The body is a sequence of independent *groups*: an `n` line with its `w`
//! lines, or an `i` line with its `p` lines (each with its `w` lines). No group
//! refers to another, so the reader cuts the input into blocks at group
//! boundaries and parses the blocks in parallel.

use pyo3::exceptions::{PyOSError, PyValueError};
use pyo3::prelude::*;
use rayon::prelude::*;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::lef::open_input;

pub const OTW_VERSION: &str = "1";
/// Target size of one parallel parse block.
const BLOCK_BYTES: usize = 4 << 20;
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
/// Window attributes that hold a `min:max` range (or one number for both).
const RANGE_ATTRS: [&str; 4] = ["rise_res", "rise_slack", "fall_res", "fall_slack"];

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

/// Receives the records of one block. Blocks are parsed concurrently, each
/// into its own sink, and the sinks are then merged in file order.
pub trait Sink: Default + Send {
    fn net(&mut self, name: &str, role: &str, attrs: &Attrs<'_>);
    fn pin(
        &mut self,
        instance: &str,
        first_of_instance: bool,
        pin: &str,
        role: &str,
        attrs: &Attrs<'_>,
    );
    fn window(&mut self, window: &Window<'_>);
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

type Tokens<'a> = std::str::SplitAsciiWhitespace<'a>;

fn role_token<'a>(token: Option<&'a str>, what: &str) -> Result<&'a str, String> {
    match token {
        Some(role) if role.len() == 1 && ROLES.contains(&role.as_bytes()[0]) => Ok(role),
        Some(role) => Err(format!(
            "{what} role must be one of c, d, u, z; found `{role}`"
        )),
        None => Err(format!("{what} has no role")),
    }
}

/// A header or attribute number: any finite real, exponent allowed.
fn real(token: Option<&str>, what: &str) -> Result<f64, String> {
    let text = token.ok_or_else(|| format!("missing {what}"))?;
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{what} is not a number: `{text}`"))
}

/// A body number: `[-]digits[.digits]`, no exponent. Checked without
/// converting, so a reader that only needs roles never parses a float.
fn is_decimal(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
    let all_digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    all_digits(whole) && all_digits(fraction)
}

fn check_range(key: &str, value: &str) -> Result<(), String> {
    let (low, high) = value.split_once(':').unwrap_or((value, value));
    if is_decimal(low) && is_decimal(high) {
        Ok(())
    } else {
        Err(format!(
            "{key} must be a decimal or a min:max pair of decimals: `{value}`"
        ))
    }
}

fn no_more(tokens: &mut Tokens<'_>, what: &str) -> Result<(), String> {
    match tokens.next() {
        Some(extra) => Err(format!("unexpected token `{extra}` after {what}")),
        None => Ok(()),
    }
}

/// The rest of a line must be `key=value` attributes.
fn attributes<'a>(tokens: &mut Tokens<'a>, what: &str) -> Result<Vec<(&'a str, &'a str)>, String> {
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
                if real(Some(value), "freq")? < 0.0 {
                    return Err(format!(
                        "freq must be a non-negative number of hertz: `{value}`"
                    ));
                }
            }
            "value" => {
                if !matches!(*value, "0" | "1") {
                    return Err(format!("value must be 0 or 1: `{value}`"));
                }
            }
            "load" | "drive_rise" | "drive_fall" => check_range(key, value)?,
            _ => {}
        }
    }
    Ok(())
}

/// A line as ASCII text, without its line ending or surrounding blanks.
fn ascii_line(raw: &[u8]) -> Result<&str, String> {
    if !raw.is_ascii() {
        return Err("line contains a non-ASCII byte".to_string());
    }
    // ASCII is valid UTF-8, so this cannot fail.
    Ok(std::str::from_utf8(raw).unwrap_or_default().trim())
}

/// `true` when a line starts the body (`n`, `i`) or ends the file (`end`).
fn starts_body(keyword: &str) -> bool {
    matches!(keyword, "n" | "i" | "end")
}

/// The header, the offset of the first body line, and the lines before it.
type ParsedHeader = (Header, usize, u64);
/// `(line, message)`.
type LineError = (u64, String);

/// Parse the header at the start of `data`.
///
/// Returns the header, the offset of the first body line and the number of
/// lines before it; `Ok(None)` when `data` ends inside the header and more
/// input exists. Errors are `(line, message)`.
fn parse_header(data: &[u8], at_eof: bool) -> Result<Option<ParsedHeader>, LineError> {
    let mut header = Header::default();
    let mut versioned = false;
    let mut offset = 0usize;
    let mut line_no = 0u64;
    while offset < data.len() {
        let end = match data[offset..].iter().position(|byte| *byte == b'\n') {
            Some(length) => offset + length,
            None if at_eof => data.len(),
            None => return Ok(None),
        };
        let fail = |message: String| (line_no + 1, message);
        let line = ascii_line(&data[offset..end]).map_err(fail)?;
        let next = (end + 1).min(data.len());
        if line.is_empty() || line.starts_with('#') {
            offset = next;
            line_no += 1;
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
            no_more(&mut tokens, "the version").map_err(fail)?;
            versioned = true;
        } else if starts_body(keyword) {
            return Ok(Some((header, offset, line_no)));
        } else {
            match keyword {
                "p" | "w" => {
                    return Err(fail(format!("`{keyword}` line before any `n` or `i` line")));
                }
                "design" => {
                    header.design = tokens
                        .next()
                        .ok_or_else(|| fail("`design` line has no name".to_string()))?
                        .to_string();
                    no_more(&mut tokens, "the design name").map_err(fail)?;
                }
                "time_unit" | "cap_unit" | "res_unit" => {
                    let value = real(tokens.next(), keyword).map_err(fail)?;
                    if value <= 0.0 {
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
                    let low = real(tokens.next(), "lower slew threshold").map_err(fail)?;
                    let high = real(tokens.next(), "upper slew threshold").map_err(fail)?;
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
                    let period = real(tokens.next(), "clock period").map_err(fail)?;
                    let rise = real(tokens.next(), "clock rise edge").map_err(fail)?;
                    let fall = real(tokens.next(), "clock fall edge").map_err(fail)?;
                    let attrs = attributes(&mut tokens, "the clock fall edge")
                        .map_err(fail)?
                        .into_iter()
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect();
                    if header.clocks.iter().any(|clock| clock.0 == name) {
                        return Err(fail(format!("clock `{name}` is declared more than once")));
                    }
                    header.clocks.push((name, period, rise, fall, attrs));
                }
                _ => {
                    let rest = line[keyword.len()..].trim().to_string();
                    header.attributes.push((keyword.to_string(), rest));
                }
            }
        }
        offset = next;
        line_no += 1;
    }
    if !at_eof {
        return Ok(None);
    }
    if !versioned {
        return Err((
            line_no.max(1),
            "not an OTW file: the first line must be `otw <version>`".to_string(),
        ));
    }
    Ok(Some((header, data.len(), line_no)))
}

/// What one block contained.
struct BlockOut<S> {
    sink: S,
    lines: u64,
    nets: u64,
    pins: u64,
    windows: u64,
    /// The block holds at least one record line (comments and blanks excluded).
    has_records: bool,
    /// Counts on the `end` line, when the block holds it.
    end: Option<(u64, u64, u64)>,
    /// First error: line within the block (1-based) and message.
    error: Option<(u64, String)>,
}

fn count(token: Option<&str>, what: &str) -> Result<u64, String> {
    let text = token.ok_or_else(|| format!("`end` line is missing the {what} count"))?;
    text.parse::<u64>()
        .map_err(|_| format!("`end` {what} count is not a number: `{text}`"))
}

fn parse_block<S: Sink>(data: &[u8], declared_clocks: usize) -> BlockOut<S> {
    let mut out = BlockOut {
        sink: S::default(),
        lines: 0,
        nets: 0,
        pins: 0,
        windows: 0,
        has_records: false,
        end: None,
        error: None,
    };
    // Role of the object the following `w` lines belong to.
    let mut owner: Option<u8> = None;
    // The open `i` group: its instance and the pins seen so far.
    let mut instance: Option<&str> = None;
    let mut group_pins: Vec<&str> = Vec::new();

    for raw in data.split(|byte| *byte == b'\n') {
        out.lines += 1;
        let mut step = || -> Result<(), String> {
            let line = ascii_line(raw)?;
            if line.is_empty() || line.starts_with('#') {
                return Ok(());
            }
            if out.end.is_some() {
                return Err("content after the `end` line".to_string());
            }
            out.has_records = true;
            let mut tokens = line.split_ascii_whitespace();
            match tokens.next().unwrap_or_default() {
                "w" => {
                    let Some(owner_role) = owner else {
                        return Err("`w` line with no `n` or `p` object above it".to_string());
                    };
                    let clock_text = tokens
                        .next()
                        .ok_or_else(|| "`w` line has no clock index".to_string())?;
                    let clock = if clock_text == "-" {
                        None
                    } else {
                        Some(
                            clock_text
                                .parse::<usize>()
                                .ok()
                                .filter(|index| *index < declared_clocks)
                                .ok_or_else(|| {
                                    format!(
                                        "`w` clock index `{clock_text}` does not name one of the {declared_clocks} declared clocks"
                                    )
                                })?,
                        )
                    };
                    let edge = match (tokens.next(), clock) {
                        (Some(edge @ ("r" | "f")), Some(_)) => edge,
                        (Some(edge @ "-"), None) => edge,
                        (other, Some(_)) => {
                            return Err(format!(
                                "`w` launch edge must be r or f; found `{}`",
                                other.unwrap_or("")
                            ));
                        }
                        (other, None) => {
                            return Err(format!(
                                "`w` with no launching clock (`-`) takes edge `-`; found `{}`",
                                other.unwrap_or("")
                            ));
                        }
                    };
                    let role = role_token(tokens.next(), "`w`")?;
                    if role == "c" && owner_role != b'c' {
                        return Err(format!(
                            "clock window on an object declared `{}`: an object with any clock window must be declared `c`",
                            owner_role as char
                        ));
                    }
                    let mut values = [""; WINDOW_VALUES];
                    for (slot, column) in values.iter_mut().zip(WINDOW_COLUMNS) {
                        let value = tokens
                            .next()
                            .ok_or_else(|| format!("`w` line is missing {column}"))?;
                        if value != "-" && !is_decimal(value) {
                            return Err(format!(
                                "{column} must be a decimal without exponent, or `-`: `{value}`"
                            ));
                        }
                        *slot = value;
                    }
                    let attrs = attributes(&mut tokens, "the eight window values")?;
                    for (key, value) in &attrs {
                        if RANGE_ATTRS.contains(key) {
                            check_range(key, value)?;
                        }
                    }
                    out.windows += 1;
                    out.sink.window(&Window {
                        clock,
                        edge,
                        role,
                        values,
                        attrs: &attrs,
                    });
                }
                "p" => {
                    let Some(owner_instance) = instance else {
                        return Err("`p` line outside an `i` group".to_string());
                    };
                    let pin = tokens
                        .next()
                        .ok_or_else(|| "`p` line has no pin name".to_string())?;
                    let role = role_token(tokens.next(), "`p`")?;
                    let attrs = attributes(&mut tokens, "the role")?;
                    check_object_attrs(&attrs, declared_clocks)?;
                    if group_pins.contains(&pin) {
                        return Err(format!(
                            "pin `{owner_instance} {pin}` is declared more than once"
                        ));
                    }
                    out.sink
                        .pin(owner_instance, group_pins.is_empty(), pin, role, &attrs);
                    group_pins.push(pin);
                    owner = Some(role.as_bytes()[0]);
                    out.pins += 1;
                }
                "n" => {
                    let name = tokens
                        .next()
                        .ok_or_else(|| "`n` line has no net name".to_string())?;
                    let role = role_token(tokens.next(), "`n`")?;
                    let attrs = attributes(&mut tokens, "the role")?;
                    check_object_attrs(&attrs, declared_clocks)?;
                    instance = None;
                    owner = Some(role.as_bytes()[0]);
                    out.nets += 1;
                    out.sink.net(name, role, &attrs);
                }
                "i" => {
                    let name = tokens
                        .next()
                        .ok_or_else(|| "`i` line has no instance name".to_string())?;
                    no_more(&mut tokens, "the instance name")?;
                    instance = Some(name);
                    group_pins.clear();
                    owner = None;
                }
                "end" => {
                    let nets = count(tokens.next(), "net")?;
                    let pins = count(tokens.next(), "pin")?;
                    let windows = count(tokens.next(), "window")?;
                    no_more(&mut tokens, "the `end` counts")?;
                    out.end = Some((nets, pins, windows));
                }
                keyword => {
                    return Err(format!(
                        "unknown record `{keyword}`; header lines must come before the first object"
                    ));
                }
            }
            Ok(())
        };
        if let Err(message) = step() {
            out.error = Some((out.lines, message));
            break;
        }
    }
    out
}

/// Hands out the input in blocks that end at a group boundary.
struct Blocks<R: Read> {
    reader: R,
    buffer: Vec<u8>,
    at_eof: bool,
}

impl<R: Read> Blocks<R> {
    /// Read until the buffer holds `target` bytes or the input ends.
    fn fill(&mut self, target: usize) -> io::Result<()> {
        while !self.at_eof && self.buffer.len() < target {
            let start = self.buffer.len();
            self.buffer.resize(target, 0);
            let mut filled = start;
            while filled < target {
                match self.reader.read(&mut self.buffer[filled..]) {
                    Ok(0) => {
                        self.at_eof = true;
                        break;
                    }
                    Ok(read) => filled += read,
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                    Err(err) => return Err(err),
                }
            }
            self.buffer.truncate(filled);
        }
        Ok(())
    }

    /// The next block of body lines, cut just before an `n`, `i` or `end` line.
    fn next_block(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut target = BLOCK_BYTES;
        loop {
            self.fill(target)?;
            if self.at_eof {
                if self.buffer.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(std::mem::take(&mut self.buffer)));
            }
            let cut = self
                .buffer
                .windows(2)
                .rposition(|pair| pair[0] == b'\n' && matches!(pair[1], b'n' | b'i' | b'e'));
            if let Some(newline) = cut {
                let rest = self.buffer.split_off(newline + 1);
                return Ok(Some(std::mem::replace(&mut self.buffer, rest)));
            }
            target *= 2;
        }
    }
}

/// Parse `path`: the header sequentially, then the body block by block in
/// parallel. `merge` receives each block's sink in file order. Returns the
/// header and the number of `w` lines.
pub fn parse_otw<S: Sink>(
    path: &Path,
    mut merge: impl FnMut(S) -> Result<(), OtwError>,
) -> Result<(Header, u64), OtwError> {
    let io_err = |err: io::Error| OtwError::Io(format!("{}: {err}", path.display()));
    let malformed = |line: u64, message: String| {
        OtwError::Malformed(format!("{}:{line}: {message}", path.display()))
    };
    let mut blocks = Blocks {
        reader: open_input(path).map_err(io_err)?,
        buffer: Vec::new(),
        at_eof: false,
    };

    let mut target = BLOCK_BYTES;
    let (header, mut line_base) = loop {
        blocks.fill(target).map_err(io_err)?;
        match parse_header(&blocks.buffer, blocks.at_eof) {
            Ok(Some((header, body_start, lines))) => {
                blocks.buffer.drain(..body_start);
                break (header, lines);
            }
            Ok(None) => target *= 2,
            Err((line, message)) => return Err(malformed(line, message)),
        }
    };

    let declared_clocks = header.clocks.len();
    let batch_size = rayon::current_num_threads().max(1);
    let (mut nets, mut pins, mut windows) = (0u64, 0u64, 0u64);
    let mut end: Option<(u64, u64, u64)> = None;
    loop {
        let mut batch = Vec::with_capacity(batch_size);
        while batch.len() < batch_size {
            match blocks.next_block().map_err(io_err)? {
                Some(block) => batch.push(block),
                None => break,
            }
        }
        if batch.is_empty() {
            break;
        }
        let parsed: Vec<BlockOut<S>> = batch
            .par_iter()
            .map(|block| parse_block(block, declared_clocks))
            .collect();
        for (block, out) in batch.iter().zip(parsed) {
            if end.is_some() && out.has_records {
                return Err(malformed(
                    line_base + 1,
                    "content after the `end` line".to_string(),
                ));
            }
            if let Some((line, message)) = out.error {
                return Err(malformed(line_base + line, message));
            }
            // A block that ends with a newline splits into one empty tail.
            line_base += out.lines - u64::from(block.ends_with(b"\n"));
            nets += out.nets;
            pins += out.pins;
            windows += out.windows;
            end = end.or(out.end);
            merge(out.sink)?;
        }
    }

    match end {
        None => Err(OtwError::Malformed(format!(
            "{}: no `end` line: the file is truncated",
            path.display()
        ))),
        Some(declared) if declared != (nets, pins, windows) => Err(OtwError::Malformed(format!(
            "{}: `end {} {} {}` does not match the file, which holds {nets} nets, {pins} pins and {windows} windows",
            path.display(),
            declared.0,
            declared.1,
            declared.2
        ))),
        Some(_) => Ok((header, windows)),
    }
}

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
    /// `(net, role, clock domains, attributes)` in file order. The domains are
    /// the `clocks=` attribute, or failing that the clocks of the windows.
    nets: Vec<NetRow>,
    /// `(instance, pin, role, clock domains, attributes)` in file order;
    /// instance `-` is a top-level port.
    pins: Vec<PinRow>,
    /// Number of `w` lines.
    windows: u64,
}

fn name_hash(name: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish()
}

#[derive(Default)]
struct Collector {
    nets: Vec<NetRow>,
    pins: Vec<PinRow>,
    net_hashes: Vec<u64>,
    /// `(hash of the instance name, index of its first pin)` per `i` group.
    instances: Vec<(u64, usize)>,
    /// The current object is a net (else a pin), and has no `clocks=`.
    current_is_net: bool,
    derive_domains: bool,
}

impl Collector {
    fn split(&mut self, attrs: &Attrs<'_>) -> (Vec<usize>, Vec<(String, String)>) {
        let mut domains = Vec::new();
        let mut others = Vec::new();
        self.derive_domains = true;
        for (key, value) in attrs {
            if *key == "clocks" {
                // Already validated by the parser against the declared clocks.
                domains = clock_indices(value, usize::MAX).unwrap_or_default();
                self.derive_domains = false;
            } else {
                others.push(((*key).to_string(), (*value).to_string()));
            }
        }
        (domains, others)
    }
}

impl Sink for Collector {
    fn net(&mut self, name: &str, role: &str, attrs: &Attrs<'_>) {
        let (domains, others) = self.split(attrs);
        self.current_is_net = true;
        self.net_hashes.push(name_hash(name));
        self.nets
            .push((name.to_string(), role.to_string(), domains, others));
    }

    fn pin(
        &mut self,
        instance: &str,
        first_of_instance: bool,
        pin: &str,
        role: &str,
        attrs: &Attrs<'_>,
    ) {
        let (domains, others) = self.split(attrs);
        self.current_is_net = false;
        if first_of_instance {
            self.instances.push((name_hash(instance), self.pins.len()));
        }
        self.pins.push((
            instance.to_string(),
            pin.to_string(),
            role.to_string(),
            domains,
            others,
        ));
    }

    fn window(&mut self, window: &Window<'_>) {
        if let (true, Some(clock)) = (self.derive_domains, window.clock) {
            let domains = if self.current_is_net {
                self.nets.last_mut().map(|row| &mut row.2)
            } else {
                self.pins.last_mut().map(|row| &mut row.3)
            };
            if let Some(domains) = domains
                && let Err(position) = domains.binary_search(&clock)
            {
                domains.insert(position, clock);
            }
        }
    }
}

pub fn summarize(path: &Path) -> Result<OtwSummary, OtwError> {
    let mut nets: Vec<NetRow> = Vec::new();
    let mut pins: Vec<PinRow> = Vec::new();
    // Name hash -> index of the first object with it; a hit is confirmed by
    // comparing the names, so a hash collision is never reported as a duplicate.
    let mut seen_nets: HashMap<u64, usize> = HashMap::new();
    let mut seen_instances: HashMap<u64, usize> = HashMap::new();
    let (header, windows) = parse_otw(path, |block: Collector| {
        let net_base = nets.len();
        let pin_base = pins.len();
        nets.extend(block.nets);
        pins.extend(block.pins);
        for (offset, hash) in block.net_hashes.into_iter().enumerate() {
            let index = net_base + offset;
            if let Some(&first) = seen_nets.get(&hash) {
                if nets[first].0 == nets[index].0 {
                    return Err(OtwError::Malformed(format!(
                        "{}: net `{}` is declared more than once",
                        path.display(),
                        nets[index].0
                    )));
                }
            } else {
                seen_nets.insert(hash, index);
            }
        }
        for (hash, offset) in block.instances {
            let index = pin_base + offset;
            if let Some(&first) = seen_instances.get(&hash) {
                if pins[first].0 == pins[index].0 {
                    return Err(OtwError::Malformed(format!(
                        "{}: instance `{}` has more than one `i` group",
                        path.display(),
                        pins[index].0
                    )));
                }
            } else {
                seen_instances.insert(hash, index);
            }
        }
        Ok(())
    })?;
    Ok(OtwSummary {
        design: header.design,
        time_unit: header.time_unit,
        cap_unit: header.cap_unit,
        res_unit: header.res_unit,
        slew_thresholds: header.slew_thresholds,
        clocks: header.clocks,
        attributes: header.attributes,
        nets,
        pins,
        windows,
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

/// Formats one CSV row per window; an object without windows gets one row.
#[derive(Default)]
struct CsvBlock {
    text: String,
    prefix: String,
    pending: bool,
    rows: u64,
    windows: u64,
}

impl CsvBlock {
    fn flush_pending(&mut self) {
        if self.pending {
            self.pending = false;
            self.text.push_str(&self.prefix);
            self.text.push_str(&",".repeat(WINDOW_CSV_COLUMNS));
            self.text.push('\n');
            self.rows += 1;
        }
    }

    fn start(&mut self, kind: &str, instance: &str, name: &str, role: &str, attrs: &Attrs<'_>) {
        self.flush_pending();
        self.prefix.clear();
        self.prefix.push_str(kind);
        self.prefix.push(',');
        csv_field(&mut self.prefix, instance);
        self.prefix.push(',');
        csv_field(&mut self.prefix, name);
        self.prefix.push(',');
        self.prefix.push_str(role);
        self.prefix.push(',');
        csv_field(&mut self.prefix, &join_attrs(attrs, &[]));
        self.pending = true;
    }
}

impl Sink for CsvBlock {
    fn net(&mut self, name: &str, role: &str, attrs: &Attrs<'_>) {
        self.start("net", "", name, role, attrs);
    }

    fn pin(&mut self, instance: &str, _first: bool, pin: &str, role: &str, attrs: &Attrs<'_>) {
        self.start("pin", instance, pin, role, attrs);
    }

    fn window(&mut self, window: &Window<'_>) {
        use std::fmt::Write as _;
        self.pending = false;
        self.windows += 1;
        self.text.push_str(&self.prefix);
        self.text.push(',');
        if let Some(clock) = window.clock {
            let _ = write!(self.text, "{clock}");
        }
        self.text.push(',');
        if window.edge != "-" {
            self.text.push_str(window.edge);
        }
        self.text.push(',');
        self.text.push_str(window.role);
        for value in window.values {
            self.text.push(',');
            if value != "-" {
                self.text.push_str(value);
            }
        }
        for key in RANGE_ATTRS {
            self.text.push(',');
            if let Some((_, value)) = window.attrs.iter().find(|(name, _)| *name == key) {
                self.text.push_str(value);
            }
        }
        self.text.push(',');
        csv_field(&mut self.text, &join_attrs(window.attrs, &RANGE_ATTRS));
        self.text.push('\n');
        self.rows += 1;
    }
}

fn write_windows_csv<W: Write>(path: &Path, mut out: W) -> Result<u64, OtwError> {
    let io_err = |err: io::Error| OtwError::Io(err.to_string());
    let mut head = String::from("kind,instance,name,role,object_attrs,clock,edge,window_role");
    for column in WINDOW_COLUMNS.iter().chain(&RANGE_ATTRS) {
        head.push(',');
        head.push_str(column);
    }
    head.push_str(",window_attrs\n");
    out.write_all(head.as_bytes()).map_err(io_err)?;
    let mut rows = 0u64;
    parse_otw(path, |mut block: CsvBlock| {
        block.flush_pending();
        rows += block.rows;
        out.write_all(block.text.as_bytes()).map_err(io_err)
    })?;
    out.flush().map_err(io_err)?;
    Ok(rows)
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
            _ => panic!("expected a malformed-file error for {name}"),
        }
    }

    fn text(value: &str) -> String {
        value.to_string()
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
n quiet z value=0
i u_gate
p CK c
w 0 r c 0.2 0.3 - - 0.1 0.1 - -
p E d
i -
p clk c
end 3 3 5
";

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
                (
                    text("quiet"),
                    text("z"),
                    vec![],
                    vec![(text("value"), text("0"))]
                ),
            ]
        );
        assert_eq!(summary.windows, 5);
        assert_eq!(
            summary.pins,
            vec![
                (text("u_gate"), text("CK"), text("c"), vec![0], vec![]),
                (text("u_gate"), text("E"), text("d"), vec![], vec![]),
                (text("-"), text("clk"), text("c"), vec![], vec![]),
            ]
        );
    }

    #[test]
    fn csv_dump_has_a_row_per_window_and_per_windowless_object() {
        let path = write_temp("dump.otw", SAMPLE);
        let mut out = Vec::new();
        let rows = write_windows_csv(&path, &mut out).unwrap();
        std::fs::remove_file(&path).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(rows, 8);
        assert_eq!(
            lines[1],
            "net,,clk,c,\"clocks=0,1 freq=1e8\",0,r,c,0.10,0.14,,,0.02,0.03,,,0.59:0.61,0.37,,,x_k=2"
        );
        assert_eq!(lines[4], "net,,data,d,,,,u,1,2,1,2,0.1,0.2,0.1,0.2,,,,,");
        assert_eq!(
            lines[5],
            format!("net,,quiet,z,value=0{}", ",".repeat(WINDOW_CSV_COLUMNS))
        );
        assert_eq!(lines[6], "pin,u_gate,CK,c,,0,r,c,0.2,0.3,,,0.1,0.1,,,,,,,");
        assert_eq!(
            lines[8],
            format!("pin,-,clk,c,{}", ",".repeat(WINDOW_CSV_COLUMNS))
        );
    }

    #[test]
    fn blocks_cut_at_group_boundaries_give_the_same_answer() {
        // Enough groups to span many parallel blocks, with a mid-file error
        // check below that depends on line numbers adding up across blocks.
        let mut body = String::from("otw 1\nclock k 1 0 0.5\n");
        let groups = 400_000u64;
        for index in 0..groups {
            body.push_str(&format!(
                "n net_{index} d\nw 0 r d 1 2 1 2 0.1 0.2 0.1 0.2\n"
            ));
            body.push_str(&format!(
                "i inst_{index}\np A d\np Z c\nw 0 f c 1 2 - - 0.1 0.2 - -\n"
            ));
        }
        let good = format!("{body}end {groups} {} {}\n", groups * 2, groups * 2);
        assert!(good.len() > 4 * BLOCK_BYTES);
        let summary = summary_of("many.otw", &good).unwrap();
        assert_eq!(summary.nets.len() as u64, groups);
        assert_eq!(summary.pins.len() as u64, groups * 2);
        assert_eq!(summary.nets[399_999].0, "net_399999");
        assert_eq!(
            summary.pins[799_999],
            (text("inst_399999"), text("Z"), text("c"), vec![0], vec![])
        );

        let bad_line = 2 + 6 * 300_000 + 1;
        let bad = good.replacen("n net_300000 d", "n net_300000 q", 1);
        assert!(malformed("many_bad.otw", &bad).contains(&format!(":{bad_line}: `n` role")));
    }

    #[test]
    fn rejects_what_it_cannot_read_and_names_the_line() {
        let clock = "otw 1\nclock k 1 0 0.5\n";
        let cases: [(&str, String, &str); 25] = [
            ("a", text("(TIMING_WINDOWS\n"), "not an OTW file"),
            ("b", text("otw 2\n"), "unsupported OTW version"),
            ("c", text("otw 1\nn a x\nend 1 0 0\n"), ":2: `n` role"),
            (
                "d",
                text("otw 1\nn a d\nn a c\nend 2 0 0\n"),
                "more than once",
            ),
            (
                "e",
                text("otw 1\nw 0 r c 1 2 3 4 5 6 7 8\n"),
                ":2: `w` line before any",
            ),
            (
                "f",
                text("otw 1\nn a c\nw 0 r c 1 2 3 4 5 6 7 8\nend 1 0 1\n"),
                "declared clocks",
            ),
            (
                "g",
                format!("{clock}n a d\nw 0 r c 1 2 3 4 5 6 7 8\nend 1 0 1\n"),
                "must be declared `c`",
            ),
            (
                "h",
                format!("{clock}n a c\nw 0 r c 1 2 3\nend 1 0 1\n"),
                "missing fall_late",
            ),
            (
                "i",
                text("otw 1\nn a c\ndesign late\nend 1 0 0\n"),
                ":3: unknown record",
            ),
            ("j", text("otw 1\nn a c stray\nend 1 0 0\n"), "key=value"),
            (
                "k",
                text("otw 1\nn a c clocks=0\nend 1 0 0\n"),
                "declared clocks",
            ),
            (
                "l",
                text("otw 1\nn a c freq=-1\nend 1 0 0\n"),
                "non-negative",
            ),
            (
                "m",
                text("otw 1\nn a d\nw - r d 1 2 3 4 5 6 7 8\nend 1 0 1\n"),
                "takes edge `-`",
            ),
            (
                "n",
                text("otw 1\nn a d\nw - - d 1 2 3 4 5 6 7 8 rise_slack=soon\nend 1 0 1\n"),
                "rise_slack must be",
            ),
            (
                "o",
                text("otw 1\nslew_thresholds 90 10\n"),
                "0 <= low < high",
            ),
            ("p", text("otw 1\nres_unit 0\n"), "res_unit must be"),
            (
                "q",
                text("otw 1\nn a z value=x\nend 1 0 0\n"),
                "value must be 0 or 1",
            ),
            (
                "r",
                text("otw 1\ni -\np a d drive_rise=fast\nend 0 1 0\n"),
                "drive_rise",
            ),
            (
                "s",
                text("otw 1\nn a d\nw - - d 1e-3 2 3 4 5 6 7 8\nend 1 0 1\n"),
                "decimal without exponent",
            ),
            ("t", text("otw 1\nn a d\n"), "no `end` line"),
            (
                "u",
                text("otw 1\nn a d\nend 2 0 0\n"),
                "does not match the file",
            ),
            (
                "v",
                text("otw 1\nn a d\nend 1 0 0\nn b d\n"),
                ":4: content after the `end` line",
            ),
            (
                "w",
                text("otw 1\nn a d\np A d\nend 1 1 0\n"),
                ":3: `p` line outside an `i` group",
            ),
            (
                "x",
                text("otw 1\ni u\np A d\np A c\nend 0 2 0\n"),
                "pin `u A` is declared more than once",
            ),
            (
                "ab",
                text("otw 1\nclock k 1 0 0.5\nclock k 2 0 1\n"),
                ":3: clock `k` is declared more than once",
            ),
        ];
        for (name, content, expected) in cases {
            let message = malformed(&format!("{name}.otw"), &content);
            assert!(message.contains(expected), "{name}: {message}");
        }
        assert!(
            malformed(
                "y.otw",
                "otw 1\ni u\np A d\ni v\np A d\ni u\np B d\nend 0 3 0\n"
            )
            .contains("instance `u` has more than one `i` group")
        );
        assert!(
            malformed("z.otw", "otw 1\ni u\nw - - d 1 2 3 4 5 6 7 8\nend 0 0 1\n")
                .contains("`w` line with no `n` or `p` object above it")
        );
        assert!(malformed("aa.otw", "otw 1\nn caf\u{e9} d\nend 1 0 0\n").contains("non-ASCII"));
        assert!(
            summary_of(
                "ok.otw",
                "otw 1\nn a z value=0\ni -\np o d load=0:0.2\nend 1 1 0\n"
            )
            .is_ok()
        );
        assert!(summary_of("empty.otw", "otw 1\nend 0 0 0\n").is_ok());
        assert!(summary_of("crlf.otw", "otw 1\r\nn a d\r\nend 1 0 0\r\n").is_ok());
    }
}
