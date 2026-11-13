//! SPEF / DSPF parasitic-exchange parser.
//!
//! The file is read once and net blocks are parsed in parallel (Rayon). Header
//! and `*NAME_MAP` are parsed sequentially, then every `*D_NET`/`*R_NET` block
//! is independent and maps to one result.
//!
//! DSPF is the same section skeleton with SPICE-style connection attributes
//! (`*C x y`, `*L`, `*S`, `*D cell`) and `$` comments. `$` comments are kept
//! verbatim as extensions so layer/coordinate metadata is not lost.

use flate2::read::GzDecoder;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use rayon::prelude::*;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexHeader {
    #[pyo3(get)]
    pub standard: String,
    #[pyo3(get)]
    pub design: String,
    #[pyo3(get)]
    pub date: String,
    #[pyo3(get)]
    pub vendor: String,
    #[pyo3(get)]
    pub program: String,
    #[pyo3(get)]
    pub version: String,
    #[pyo3(get)]
    pub design_flow: String,
    #[pyo3(get)]
    pub divider: String,
    #[pyo3(get)]
    pub delimiter: String,
    #[pyo3(get)]
    pub bus_delimiter: String,
    #[pyo3(get)]
    pub t_unit: String,
    #[pyo3(get)]
    pub c_unit: String,
    #[pyo3(get)]
    pub r_unit: String,
    #[pyo3(get)]
    pub l_unit: String,
    #[pyo3(get)]
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

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexConn {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub direction: String,
    #[pyo3(get)]
    pub attrs: Vec<String>,
    #[pyo3(get)]
    pub x: Option<f64>,
    #[pyo3(get)]
    pub y: Option<f64>,
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexCap {
    #[pyo3(get)]
    pub id: String,
    #[pyo3(get)]
    pub node1: String,
    #[pyo3(get)]
    pub node2: Option<String>,
    #[pyo3(get)]
    pub value: f64,
    #[pyo3(get)]
    pub ext: Option<String>,
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexRes {
    #[pyo3(get)]
    pub id: String,
    #[pyo3(get)]
    pub node1: String,
    #[pyo3(get)]
    pub node2: String,
    #[pyo3(get)]
    pub value: f64,
    #[pyo3(get)]
    pub ext: Option<String>,
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexNet {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub total_cap: f64,
    #[pyo3(get)]
    pub ground_cap: f64,
    #[pyo3(get)]
    pub coupling_cap: f64,
    #[pyo3(get)]
    pub conns: Vec<PexConn>,
    #[pyo3(get)]
    pub caps: Vec<PexCap>,
    #[pyo3(get)]
    pub res: Vec<PexRes>,
    #[pyo3(get)]
    pub ext: Option<String>,
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexSummary {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub total_cap: f64,
    #[pyo3(get)]
    pub ground_cap: f64,
    #[pyo3(get)]
    pub coupling_cap: f64,
    #[pyo3(get)]
    pub cap_count: usize,
    #[pyo3(get)]
    pub res_count: usize,
    #[pyo3(get)]
    pub res_total: f64,
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexFile {
    #[pyo3(get)]
    pub header: PexHeader,
    #[pyo3(get)]
    pub nets: Vec<PexNet>,
}

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PexHistogram {
    #[pyo3(get)]
    pub min: f64,
    #[pyo3(get)]
    pub max: f64,
    #[pyo3(get)]
    pub bounds: Vec<f64>,
    #[pyo3(get)]
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

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    let gz = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"));
    if gz {
        let file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
        GzDecoder::new(BufReader::new(file))
            .read_to_end(&mut data)
            .map_err(|err| format!("{}: {err}", path.display()))?;
    } else {
        data = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
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

fn header_value<'a>(line: &'a [u8], keyword: &str) -> Option<String> {
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

fn parse_header(data: &[u8]) -> Result<(PexHeader, Vec<Option<String>>, usize), String> {
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
    Ok((header, names, first_net))
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

fn parse_all(
    path: &Path,
    keep_entries: bool,
) -> Result<(PexHeader, Vec<PexNet>, Vec<PexSummary>), String> {
    let data = read_bytes(path)?;
    let (header, names, first_net) = parse_header(&data)?;
    let delimiter = header.delimiter.chars().next().unwrap_or(':');
    let blocks = index_blocks(&data, first_net)?;
    let parsed: Vec<(PexNet, PexSummary)> = blocks
        .par_iter()
        .map(|(start, end)| parse_net_block(&data[*start..*end], &names, delimiter, keep_entries))
        .collect::<Result<Vec<_>, _>>()?;
    let (nets, summaries): (Vec<_>, Vec<_>) = parsed.into_iter().unzip();
    Ok((header, nets, summaries))
}

fn open_reader(path: &Path) -> Result<Box<dyn BufRead>, String> {
    let file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
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
fn summaries_streaming(path: &Path) -> Result<Vec<PexSummary>, String> {
    let mut reader = open_reader(path)?;
    let mut header_buf = Vec::new();
    let mut pending: Option<Vec<u8>> = None;
    loop {
        let mut line = Vec::new();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|err| format!("{}: {err}", path.display()))?
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
    let (header, names, _) = parse_header(&header_buf)?;
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
                .map_err(|err| format!("{}: {err}", path.display()))?
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
        return Err(format!(
            "{}: unterminated net block (missing *END)",
            path.display()
        ));
    }
    Ok(summaries)
}

fn wants_stream(mode: &str) -> bool {
    matches!(mode, "stream" | "low-memory" | "low_memory" | "memory")
}

#[pyfunction]
#[pyo3(signature = (path, keep_entries=true, mode="auto"))]
fn parse_pex(py: Python<'_>, path: PathBuf, keep_entries: bool, mode: &str) -> PyResult<PexFile> {
    if mode == "stream" && keep_entries {
        return Err(PyRuntimeError::new_err(
            "mode='stream' returns summaries only; use keep_entries=false",
        ));
    }
    py.detach(|| -> Result<PexFile, String> {
        let (header, nets, _) = parse_all(&path, keep_entries)?;
        Ok(PexFile { header, nets })
    })
    .map_err(PyRuntimeError::new_err)
}

#[pyfunction]
#[pyo3(signature = (path, mode="auto"))]
fn pex_summaries(py: Python<'_>, path: PathBuf, mode: &str) -> PyResult<Vec<PexSummary>> {
    let stream = wants_stream(mode);
    py.detach(|| -> Result<Vec<PexSummary>, String> {
        if stream {
            summaries_streaming(&path)
        } else {
            let (_, _, summaries) = parse_all(&path, false)?;
            Ok(summaries)
        }
    })
    .map_err(PyRuntimeError::new_err)
}

#[pyfunction]
#[pyo3(signature = (path, bins=20, mode="auto"))]
fn pex_histogram(py: Python<'_>, path: PathBuf, bins: usize, mode: &str) -> PyResult<PexHistogram> {
    let stream = wants_stream(mode);
    py.detach(|| -> Result<PexHistogram, String> {
        let summaries = if stream {
            summaries_streaming(&path)?
        } else {
            let (_, _, summaries) = parse_all(&path, false)?;
            summaries
        };
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
    })
    .map_err(PyRuntimeError::new_err)
}

#[pyfunction]
fn pex_header(py: Python<'_>, path: PathBuf) -> PyResult<PexHeader> {
    py.detach(|| -> Result<PexHeader, String> {
        let data = read_bytes(&path)?;
        let (header, _, _) = parse_header(&data)?;
        Ok(header)
    })
    .map_err(PyRuntimeError::new_err)
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
