//! SPEF / DSPF parasitic-exchange parser.
//!
//! The parallel reader parses the header and `*NAME_MAP` first, then
//! processes independent `*D_NET` and `*R_NET` blocks with Rayon. Summary
//! queries can also use [`SummaryMode::Stream`] to buffer one block at a time.
//!
//! DSPF shares the section layout and uses SPICE-style connection attributes
//! such as `*C x y` and `*D cell`. Inline `$` comments can carry layer or
//! coordinate metadata and are retained as extensions.

use flate2::read::GzDecoder;
use rayon::prelude::*;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use crate::Error;

/// SPEF or DSPF header metadata and resolved port names.
#[derive(Clone, Debug, PartialEq)]
pub struct PexHeader {
    pub standard: String,
    pub design: String,
    pub date: String,
    pub vendor: String,
    pub program: String,
    pub version: String,
    pub design_flow: String,
    pub divider: String,
    pub delimiter: String,
    pub bus_delimiter: String,
    pub t_unit: String,
    pub c_unit: String,
    pub r_unit: String,
    pub l_unit: String,
    pub ports: Vec<(String, String)>,
}

impl Default for PexHeader {
    fn default() -> Self {
        Self {
            standard: String::new(),
            design: String::new(),
            date: String::new(),
            vendor: String::new(),
            program: String::new(),
            version: String::new(),
            design_flow: String::new(),
            divider: "/".to_string(),
            delimiter: ":".to_string(),
            bus_delimiter: "[]".to_string(),
            t_unit: String::new(),
            c_unit: String::new(),
            r_unit: String::new(),
            l_unit: String::new(),
            ports: Vec::new(),
        }
    }
}

/// A net terminal and its connection attributes.
#[derive(Clone, Debug, PartialEq)]
pub struct PexConn {
    pub name: String,
    pub direction: String,
    pub attrs: Vec<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

/// A ground or coupling capacitor. Ground capacitors have no second node.
#[derive(Clone, Debug, PartialEq)]
pub struct PexCap {
    pub id: String,
    pub node1: String,
    pub node2: Option<String>,
    pub value: f64,
    pub ext: Option<String>,
}

/// A resistor between two nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct PexRes {
    pub id: String,
    pub node1: String,
    pub node2: String,
    pub value: f64,
    pub ext: Option<String>,
}

/// An electrical net and its optional connection, capacitor, and resistor entries.
#[derive(Clone, Debug, PartialEq)]
pub struct PexNet {
    pub name: String,
    /// Total capacitance declared in the net header.
    pub total_cap: f64,
    pub ground_cap: f64,
    pub coupling_cap: f64,
    pub conns: Vec<PexConn>,
    pub caps: Vec<PexCap>,
    pub res: Vec<PexRes>,
    pub ext: Option<String>,
}

/// Per-net capacitance and resistance totals in the units from the file header.
#[derive(Clone, Debug, PartialEq)]
pub struct PexSummary {
    pub name: String,
    /// Total capacitance declared in the net header.
    pub total_cap: f64,
    pub ground_cap: f64,
    pub coupling_cap: f64,
    pub cap_count: usize,
    pub res_count: usize,
    /// Sum of the resistor values in this net.
    pub res_total: f64,
}

/// Header and electrical nets in file order.
#[derive(Clone, Debug, PartialEq)]
pub struct PexFile {
    pub header: PexHeader,
    pub nets: Vec<PexNet>,
}

/// Histogram of the total capacitance declared for each electrical net.
#[derive(Clone, Debug, PartialEq)]
pub struct PexHistogram {
    pub min: f64,
    pub max: f64,
    /// Bin edges, including the upper edge of the last bin.
    pub bounds: Vec<f64>,
    pub counts: Vec<u64>,
}

struct Line<'a> {
    start: usize,
    end: usize,
    bytes: &'a [u8],
}

struct Lines<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;
    fn next(&mut self) -> Option<Line<'a>> {
        if self.pos >= self.data.len() {
            return None;
        }
        let start = self.pos;
        let end = self.data[start..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(self.data.len(), |offset| start + offset);
        self.pos = if end < self.data.len() { end + 1 } else { end };
        let mut bytes = &self.data[start..end];
        if bytes.last() == Some(&b'\r') {
            bytes = &bytes[..bytes.len() - 1];
        }
        Some(Line { start, end, bytes })
    }
}

fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn split_extension(line: &[u8]) -> (&[u8], Option<String>) {
    if let Some(index) = line.iter().position(|&byte| byte == b'$') {
        let ext = String::from_utf8_lossy(trim(&line[index + 1..])).into_owned();
        (
            &line[..index],
            if ext.is_empty() { None } else { Some(ext) },
        )
    } else {
        (line, None)
    }
}

fn is_comment(line: &[u8]) -> bool {
    line.starts_with(b"//") || line.starts_with(b"/*")
}

fn resolve_name(token: &str, names: &[Option<String>], delimiter: char) -> String {
    if let Some(rest) = token.strip_prefix('*') {
        let digits = rest
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits > 0 {
            if let Ok(id) = rest[..digits].parse::<usize>() {
                if let Some(Some(name)) = names.get(id) {
                    return format!("{name}{}", &rest[digits..]);
                }
            }
        }
    } else if let Some(index) = token.find(delimiter) {
        let (head, tail) = token.split_at(index);
        if let Some(rest) = head.strip_prefix('*') {
            if let Ok(id) = rest.parse::<usize>() {
                if let Some(Some(name)) = names.get(id) {
                    return format!("{name}{tail}");
                }
            }
        }
    }
    token.to_string()
}

fn path_io_error(path: &Path, err: io::Error) -> io::Error {
    io::Error::new(err.kind(), format!("{}: {err}", path.display()))
}

fn read_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    let gz = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"));
    if gz {
        let file = File::open(path).map_err(|err| path_io_error(path, err))?;
        GzDecoder::new(BufReader::new(file))
            .read_to_end(&mut data)
            .map_err(|err| path_io_error(path, err))?;
    } else {
        data = std::fs::read(path).map_err(|err| path_io_error(path, err))?;
    }
    Ok(data)
}

fn net_start(line: &[u8]) -> bool {
    line.starts_with(b"*D_NET")
        || line.starts_with(b"*R_NET")
        || line.starts_with(b"*D_PNET")
        || line.starts_with(b"*R_PNET")
}

fn is_physical(line: &[u8]) -> bool {
    line.starts_with(b"*D_PNET") || line.starts_with(b"*R_PNET")
}

fn header_keyword(line: &[u8]) -> Option<&str> {
    let rest = line.strip_prefix(b"*")?;
    let end = rest
        .iter()
        .position(|byte| byte.is_ascii_whitespace())
        .unwrap_or(rest.len());
    std::str::from_utf8(&rest[..end]).ok()
}

fn header_value(line: &[u8], keyword: &str) -> Option<String> {
    let text = std::str::from_utf8(line).ok()?;
    let rest = text.strip_prefix('*')?.strip_prefix(keyword)?;
    if rest.is_empty() {
        return Some(String::new());
    }
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    Some(unquote(rest.trim()))
}

fn parse_header(data: &[u8]) -> (PexHeader, Vec<Option<String>>, usize) {
    let mut header = PexHeader::default();
    let mut names: Vec<Option<String>> = vec![None];
    let mut section = "";
    let mut first_net = data.len();
    let mut name_map_entries: Vec<(usize, String)> = Vec::new();
    let mut ports: Vec<(String, String)> = Vec::new();

    for line in (Lines { data, pos: 0 }) {
        if line.start >= first_net {
            break;
        }
        let (content, _) = split_extension(line.bytes);
        let content = trim(content);
        if content.is_empty() || is_comment(content) {
            continue;
        }
        if content[0] == b'*' {
            if section == "name_map" {
                let rest = &content[1..];
                if rest.first().is_some_and(u8::is_ascii_digit) {
                    let digits = rest
                        .iter()
                        .position(|byte| !byte.is_ascii_digit())
                        .unwrap_or(rest.len());
                    if let (Ok(id), Ok(name)) = (
                        std::str::from_utf8(&rest[..digits])
                            .unwrap_or("")
                            .parse::<usize>(),
                        std::str::from_utf8(trim(&rest[digits..])),
                    ) {
                        name_map_entries.push((id, name.to_string()));
                    }
                    continue;
                }
            }
            if section == "ports" {
                let text = std::str::from_utf8(content).unwrap_or("");
                let mut fields = text.split_ascii_whitespace();
                if fields.next() == Some("*P") {
                    if let (Some(name), Some(direction)) = (fields.next(), fields.next()) {
                        ports.push((name.to_string(), direction.to_string()));
                    }
                    continue;
                }
            }
            if net_start(content) {
                first_net = line.start;
                break;
            }
            let keyword = header_keyword(content).unwrap_or("");
            match keyword {
                "SPEF" | "DSPF" => {
                    header.standard = header_value(content, keyword).unwrap_or_default();
                }
                "DESIGN" => header.design = header_value(content, "DESIGN").unwrap_or_default(),
                "DATE" => header.date = header_value(content, "DATE").unwrap_or_default(),
                "VENDOR" => header.vendor = header_value(content, "VENDOR").unwrap_or_default(),
                "PROGRAM" => header.program = header_value(content, "PROGRAM").unwrap_or_default(),
                "VERSION" => header.version = header_value(content, "VERSION").unwrap_or_default(),
                "DESIGN_FLOW" => {
                    header.design_flow = header_value(content, "DESIGN_FLOW").unwrap_or_default();
                }
                "DIVIDER" => header.divider = header_value(content, "DIVIDER").unwrap_or_default(),
                "DELIMITER" => {
                    header.delimiter = header_value(content, "DELIMITER").unwrap_or_default();
                }
                "BUS_DELIMITER" => {
                    header.bus_delimiter =
                        header_value(content, "BUS_DELIMITER").unwrap_or_default();
                }
                "T_UNIT" => header.t_unit = header_value(content, "T_UNIT").unwrap_or_default(),
                "C_UNIT" => header.c_unit = header_value(content, "C_UNIT").unwrap_or_default(),
                "R_UNIT" => header.r_unit = header_value(content, "R_UNIT").unwrap_or_default(),
                "L_UNIT" => header.l_unit = header_value(content, "L_UNIT").unwrap_or_default(),
                "NAME_MAP" => section = "name_map",
                "PORTS" => section = "ports",
                "POWER_NETS" | "GROUND_NETS" | "DEFINE" | "PDEFINE" => section = "skip",
                _ => section = "",
            }
            continue;
        }
        match section {
            "name_map" => {
                if let Some(rest) = content.strip_prefix(b"*") {
                    let digits = rest
                        .iter()
                        .position(|byte| !byte.is_ascii_digit())
                        .unwrap_or(rest.len());
                    if digits > 0 {
                        if let (Ok(id), Ok(name)) = (
                            std::str::from_utf8(&rest[..digits])
                                .unwrap_or("")
                                .parse::<usize>(),
                            std::str::from_utf8(trim(&rest[digits..])),
                        ) {
                            name_map_entries.push((id, name.to_string()));
                        }
                    }
                }
            }
            "ports" => {
                let text = std::str::from_utf8(content).unwrap_or("");
                let mut fields = text.split_ascii_whitespace();
                if fields.next() == Some("*P") {
                    if let (Some(name), Some(direction)) = (fields.next(), fields.next()) {
                        ports.push((name.to_string(), direction.to_string()));
                    }
                }
            }
            _ => {}
        }
    }

    let max_id = name_map_entries
        .iter()
        .map(|(id, _)| *id)
        .max()
        .unwrap_or(0);
    names.resize(max_id + 1, None);
    for (id, name) in &name_map_entries {
        if let Some(slot) = names.get_mut(*id) {
            *slot = Some(name.clone());
        }
    }
    let delimiter = header.delimiter.chars().next().unwrap_or(':');
    header.ports = ports
        .into_iter()
        .map(|(name, direction)| (resolve_name(&name, &names, delimiter), direction))
        .collect();
    (header, names, first_net)
}

fn index_blocks(data: &[u8], start: usize) -> Result<Vec<(usize, usize)>, String> {
    let mut blocks = Vec::new();
    let mut current: Option<(usize, bool)> = None;
    for line in (Lines { data, pos: start }) {
        let (content, _) = split_extension(line.bytes);
        let content = trim(content);
        if content.is_empty() || is_comment(content) {
            continue;
        }
        if current.is_none() && net_start(content) {
            current = Some((line.start, is_physical(content)));
            continue;
        }
        if let Some((block_start, physical)) = current {
            if content.starts_with(b"*END") {
                if !physical {
                    blocks.push((block_start, line.end));
                }
                current = None;
            }
        }
    }
    if current.is_some() {
        return Err("unterminated net block (missing *END)".to_string());
    }
    Ok(blocks)
}

fn parse_net_block(
    block: &[u8],
    names: &[Option<String>],
    delimiter: char,
    keep_entries: bool,
) -> Result<(PexNet, PexSummary), String> {
    let mut lines = Lines {
        data: block,
        pos: 0,
    };
    let Some(first) = lines.next() else {
        return Err("empty net block".to_string());
    };
    let (first_content, first_ext) = split_extension(first.bytes);
    let first_text = std::str::from_utf8(trim(first_content)).unwrap_or("");
    let mut fields = first_text.split_ascii_whitespace();
    let kind = fields.next().unwrap_or("");
    if kind != "*D_NET" && kind != "*R_NET" {
        return Err(format!("unsupported net header {kind}"));
    }
    let raw_name = fields.next().unwrap_or("");
    let name = resolve_name(raw_name, names, delimiter);
    let total_cap = fields
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0);

    let mut mode = "";
    let mut conns = Vec::new();
    let mut caps = Vec::new();
    let mut res = Vec::new();
    let mut ground_cap = 0.0;
    let mut coupling_cap = 0.0;
    let mut cap_count = 0usize;
    let mut res_count = 0usize;
    let mut res_total = 0.0;

    for line in lines {
        let (content, ext) = split_extension(line.bytes);
        let content = trim(content);
        if content.is_empty() || is_comment(content) {
            continue;
        }
        let text = std::str::from_utf8(content).unwrap_or("");
        if content.starts_with(b"*") {
            match text.split_ascii_whitespace().next().unwrap_or("") {
                "*CONN" => {
                    mode = "conn";
                    continue;
                }
                "*CAP" => {
                    mode = "cap";
                    continue;
                }
                "*RES" => {
                    mode = "res";
                    continue;
                }
                "*INDUC" => {
                    mode = "induc";
                    continue;
                }
                "*END" => break,
                _ => {}
            }
        }
        match mode {
            "conn" => {
                let mut fields = text.split_ascii_whitespace();
                let tag = fields.next().unwrap_or("");
                if tag != "*P" && tag != "*I" {
                    continue;
                }
                let raw = fields.next().unwrap_or("");
                let direction = fields.next().unwrap_or("").to_string();
                let attrs: Vec<String> = fields.map(str::to_string).collect();
                let mut x = None;
                let mut y = None;
                let mut index = 0;
                while index < attrs.len() {
                    if attrs[index] == "*C" && index + 2 < attrs.len() {
                        x = attrs[index + 1].parse().ok();
                        y = attrs[index + 2].parse().ok();
                        index += 3;
                    } else {
                        index += 1;
                    }
                }
                if keep_entries {
                    conns.push(PexConn {
                        name: resolve_name(raw, names, delimiter),
                        direction,
                        attrs,
                        x,
                        y,
                    });
                }
            }
            "cap" => {
                let fields: Vec<&str> = text.split_ascii_whitespace().collect();
                if fields.len() < 3 {
                    continue;
                }
                let id = fields[0].to_string();
                let value: f64 = fields[fields.len() - 1].parse().unwrap_or(0.0);
                let nodes = &fields[1..fields.len() - 1];
                let node1 = resolve_name(nodes[0], names, delimiter);
                let node2 = nodes
                    .get(1)
                    .map(|node| resolve_name(node, names, delimiter));
                if node2.is_some() {
                    coupling_cap += value;
                } else {
                    ground_cap += value;
                }
                cap_count += 1;
                if keep_entries {
                    caps.push(PexCap {
                        id,
                        node1,
                        node2,
                        value,
                        ext,
                    });
                }
            }
            "res" => {
                let fields: Vec<&str> = text.split_ascii_whitespace().collect();
                if fields.len() < 4 {
                    continue;
                }
                let value: f64 = fields[3].parse().unwrap_or(0.0);
                res_total += value;
                res_count += 1;
                if keep_entries {
                    res.push(PexRes {
                        id: fields[0].to_string(),
                        node1: resolve_name(fields[1], names, delimiter),
                        node2: resolve_name(fields[2], names, delimiter),
                        value,
                        ext,
                    });
                }
            }
            _ => {}
        }
    }

    let net = PexNet {
        name: name.clone(),
        total_cap,
        ground_cap,
        coupling_cap,
        conns,
        caps,
        res,
        ext: first_ext,
    };
    let summary = PexSummary {
        name,
        total_cap,
        ground_cap,
        coupling_cap,
        cap_count,
        res_count,
        res_total,
    };
    Ok((net, summary))
}

fn parse_all(path: &Path, keep_entries: bool) -> Result<(PexFile, Vec<PexSummary>), Error> {
    let data = read_bytes(path)?;
    let (header, names, first_net) = parse_header(&data);
    let delimiter = header.delimiter.chars().next().unwrap_or(':');
    let blocks = index_blocks(&data, first_net)?;
    let parsed: Vec<(PexNet, PexSummary)> = crate::threads::install(|| {
        blocks
            .par_iter()
            .map(|(start, end)| {
                parse_net_block(&data[*start..*end], &names, delimiter, keep_entries)
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    let (nets, summaries): (Vec<_>, Vec<_>) = parsed.into_iter().unzip();
    Ok((PexFile { header, nets }, summaries))
}

fn open_reader(path: &Path) -> io::Result<Box<dyn BufRead>> {
    let file = File::open(path).map_err(|err| path_io_error(path, err))?;
    let gz = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"));
    if gz {
        Ok(Box::new(BufReader::new(GzDecoder::new(file))))
    } else {
        Ok(Box::new(BufReader::new(file)))
    }
}

/// Low-memory pass: one net block is buffered at a time, never the whole file.
fn summaries_streaming(path: &Path) -> Result<Vec<PexSummary>, Error> {
    let mut reader = open_reader(path)?;
    let mut header_buf = Vec::new();
    let mut pending: Option<Vec<u8>> = None;
    loop {
        let mut line = Vec::new();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|err| path_io_error(path, err))?
            == 0
        {
            break;
        }
        if net_start(trim(&line)) {
            pending = Some(line);
            break;
        }
        header_buf.extend_from_slice(&line);
    }
    let (header, names, _) = parse_header(&header_buf);
    let delimiter = header.delimiter.chars().next().unwrap_or(':');

    let mut summaries = Vec::new();
    let mut current: Option<(Vec<u8>, bool)> = None;
    loop {
        let line = if let Some(pending_line) = pending.take() {
            pending_line
        } else {
            let mut next = Vec::new();
            if reader
                .read_until(b'\n', &mut next)
                .map_err(|err| path_io_error(path, err))?
                == 0
            {
                break;
            }
            next
        };
        let content = trim(&line);
        if let Some((block, _physical)) = current.as_mut() {
            block.extend_from_slice(&line);
            if content.starts_with(b"*END") {
                let (block, physical) = current.take().unwrap();
                if !physical {
                    let (_, summary) = parse_net_block(&block, &names, delimiter, false)?;
                    summaries.push(summary);
                }
            }
        } else if net_start(content) {
            current = Some((line.clone(), is_physical(content)));
        }
    }
    if current.is_some() {
        return Err(format!("{}: unterminated net block (missing *END)", path.display()).into());
    }
    Ok(summaries)
}

/// Select how per-net summaries are read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SummaryMode {
    /// Read the whole file and parse net blocks in parallel.
    #[default]
    Parallel,
    /// Buffer one net block at a time to limit memory use.
    Stream,
}

/// Read the header and resolve port names without parsing net blocks.
///
/// Accepts plain text or a gzip-compressed file with a `.gz` suffix.
pub fn header(path: &Path) -> Result<PexHeader, Error> {
    let data = read_bytes(path)?;
    let (header, _, _) = parse_header(&data);
    Ok(header)
}

/// Read the header and electrical nets from a plain or gzip-compressed file.
///
/// Set `keep_entries` to `false` to leave the connection, capacitor, and
/// resistor lists empty while retaining each net's capacitance totals.
/// Use [`summaries`] to include per-net resistor counts and totals.
pub fn parse(path: &Path, keep_entries: bool) -> Result<PexFile, Error> {
    let (file, _) = parse_all(path, keep_entries)?;
    Ok(file)
}

/// Read per-net electrical RC totals in file order without retaining entries.
///
/// Accepts plain text or a gzip-compressed file with a `.gz` suffix.
pub fn summaries(path: &Path, mode: SummaryMode) -> Result<Vec<PexSummary>, Error> {
    match mode {
        SummaryMode::Parallel => {
            let (_, summaries) = parse_all(path, false)?;
            Ok(summaries)
        }
        SummaryMode::Stream => summaries_streaming(path),
    }
}

/// Bin each electrical net by its declared total capacitance.
///
/// Returns empty bounds and counts if there are no electrical nets or if
/// `bins` is zero.
pub fn histogram(path: &Path, bins: usize, mode: SummaryMode) -> Result<PexHistogram, Error> {
    let summaries = summaries(path, mode)?;
    if summaries.is_empty() || bins == 0 {
        return Ok(PexHistogram {
            min: 0.0,
            max: 0.0,
            bounds: Vec::new(),
            counts: Vec::new(),
        });
    }
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for summary in &summaries {
        min = min.min(summary.total_cap);
        max = max.max(summary.total_cap);
    }
    if !(max > min) {
        max = min + 1.0;
    }
    let width = (max - min) / bins as f64;
    let mut counts = vec![0u64; bins];
    let mut bounds = Vec::with_capacity(bins + 1);
    for index in 0..=bins {
        bounds.push(min + width * index as f64);
    }
    for summary in &summaries {
        let mut index = ((summary.total_cap - min) / width) as isize;
        if index < 0 {
            index = 0;
        }
        if index as usize >= bins {
            index = bins as isize - 1;
        }
        counts[index as usize] += 1;
    }
    Ok(PexHistogram {
        min,
        max,
        bounds,
        counts,
    })
}
