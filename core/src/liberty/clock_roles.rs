use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;

use rayon::prelude::*;

use super::{CellData, ParseError, Parser, PinData, index_cells, read_to_memory};
use crate::{Error, threads};

/// How a Liberty cell receives or propagates a clock on its top-level pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellClockRole {
    /// Input pins in declaration order. For a gate, the clock-through pins in
    /// the same order as [Self::clock_pins].
    pub inputs: Vec<String>,
    /// Output pins in declaration order. For a gate, explicit gated-clock
    /// outputs take precedence over the full list.
    pub outputs: Vec<String>,
    /// Whether an incoming clock path stops here. An endpoint's output can
    /// still drive a separate clock net, for example as a frequency divider.
    pub endpoint: bool,
    /// Clock inputs, with explicit `clock : true` pins first, then pins
    /// referenced by the cell's state clock expressions. Gates use their
    /// clock-through inputs.
    pub clock_pins: Vec<String>,
    /// Whether the cell declares `clock_gating_integrated_cell`.
    pub clock_gate: bool,
    /// Whether the cell has an `ff`, `ff_bank`, `latch`, or `latch_bank` group.
    pub sequential: bool,
    /// Whether the cell has a cell-level Liberty `memory () { ... }` group.
    /// A memory is a clock endpoint even without a declared clock pin.
    pub memory: bool,
}

/// Resolve clock roles for the requested cells from plain or gzip Liberty files.
///
/// Libraries are visited in supplied order until all names are found. The
/// first library containing a name supplies its role. Within one library, the
/// last definition of a name wins, as in [super::LibraryIndex]. Unresolved
/// names are omitted from the result.
///
/// Each opened file is indexed by byte range. Only still-needed cell bodies
/// are parsed, in parallel under the [threads] limit, with table data skipped.
/// I/O and selected cell parse errors are returned with the file path.
pub fn cell_clock_roles_for_cells(
    lib_paths: &[PathBuf],
    wanted_cells: &HashSet<String>,
) -> Result<HashMap<String, CellClockRole>, Error> {
    let mut unresolved: HashSet<&str> = wanted_cells.iter().map(String::as_str).collect();
    let mut roles = HashMap::new();
    for path in lib_paths {
        if unresolved.is_empty() {
            break;
        }

        let data = read_to_memory(path)
            .map_err(|err| io::Error::new(err.kind(), format!("{}: {err}", path.display())))?;
        let mut ranges = HashMap::new();
        for (name, start, end) in index_cells(&data).cells {
            if unresolved.contains(name.as_str()) {
                ranges.insert(name, (start, end));
            }
        }
        if ranges.is_empty() {
            continue;
        }
        let mut matches: Vec<_> = ranges.into_iter().collect();
        // Preserve source order for reported errors across worker counts.
        matches.sort_unstable_by_key(|(_, (start, _))| *start);
        let parsed = threads::install(|| {
            matches
                .into_par_iter()
                .map(|(name, (start, end))| {
                    let role = parse_role(&data[start..end]).map_err(|err| {
                        Error::Message(format!("{}: cell {name:?}: {err}", path.display()))
                    })?;
                    Ok((name, role))
                })
                .collect::<Vec<Result<_, Error>>>()
        });
        for result in parsed {
            let (name, role) = result?;
            unresolved.remove(name.as_str());
            roles.insert(name, role);
        }
    }
    Ok(roles)
}

fn parse_role(data: &[u8]) -> Result<CellClockRole, ParseError> {
    let mut parser = Parser::new(Box::new(io::Cursor::new(data)), None, true)?;
    let group = parser.take_word()?;
    parser.expect_symbol(b'(')?;
    let name = parser
        .read_args(&group)?
        .into_iter()
        .next()
        .unwrap_or_default();
    parser.expect_symbol(b'{')?;
    let (cell, memory) = parser.parse_cell_body(name)?;
    Ok(cell_clock_role(&cell, memory))
}

fn cell_clock_role(cell: &CellData, memory: bool) -> CellClockRole {
    let inputs: Vec<_> = cell
        .pins
        .iter()
        .filter(|pin| pin.direction.as_deref() == Some("input"))
        .collect();
    let outputs: Vec<_> = cell
        .pins
        .iter()
        .filter(|pin| pin.direction.as_deref() == Some("output"))
        .collect();
    let clock_inputs = flagged_pins(&inputs, "clock");
    let ff = cell.ff.as_ref();
    let latch = cell.latch.as_ref();
    let sequential = ff.is_some() || latch.is_some();
    let state_clocks: Vec<_> = [
        ff.and_then(|ff| ff.clocked_on.as_deref()),
        ff.and_then(|ff| ff.clocked_on_also.as_deref()),
        latch.and_then(|latch| latch.enable.as_deref()),
    ]
    .into_iter()
    .flatten()
    .flat_map(|expression| expression_pins(expression, &inputs))
    .collect();

    if cell
        .attributes
        .iter()
        .any(|(key, _)| key == "clock_gating_integrated_cell")
    {
        let through = [
            flagged_pins(&inputs, "clock_gate_clock_pin"),
            clock_inputs,
            state_clocks,
        ]
        .into_iter()
        .find(|pins| !pins.is_empty())
        .unwrap_or(inputs);
        let gated_outputs = flagged_pins(&outputs, "clock_gate_out_pin");
        let gated_outputs = if gated_outputs.is_empty() {
            outputs
        } else {
            gated_outputs
        };
        let clock_pins = unique_names(through.into_iter().filter(|pin| {
            !pin_flag(pin, "clock_gate_enable_pin") && !pin_flag(pin, "clock_gate_test_pin")
        }));
        return CellClockRole {
            inputs: clock_pins.clone(),
            outputs: pin_names(gated_outputs),
            endpoint: false,
            clock_pins,
            clock_gate: true,
            sequential,
            memory,
        };
    }

    let clock_pins = unique_names(clock_inputs.into_iter().chain(state_clocks));
    CellClockRole {
        inputs: pin_names(inputs),
        outputs: pin_names(outputs),
        endpoint: sequential || memory || !clock_pins.is_empty(),
        clock_pins,
        clock_gate: false,
        sequential,
        memory,
    }
}

fn pin_names(pins: Vec<&PinData>) -> Vec<String> {
    pins.into_iter().map(|pin| pin.name.clone()).collect()
}

fn unique_names<'a>(pins: impl IntoIterator<Item = &'a PinData>) -> Vec<String> {
    let mut seen = HashSet::new();
    pins.into_iter()
        .filter(|pin| seen.insert(pin.name.as_str()))
        .map(|pin| pin.name.clone())
        .collect()
}

fn pin_flag(pin: &PinData, attribute: &str) -> bool {
    pin.attributes
        .iter()
        .find(|(key, _)| key == attribute)
        .is_some_and(|(_, value)| value.trim().trim_matches('"').eq_ignore_ascii_case("true"))
}

fn flagged_pins<'a>(pins: &[&'a PinData], attribute: &str) -> Vec<&'a PinData> {
    pins.iter()
        .copied()
        .filter(|pin| pin_flag(pin, attribute))
        .collect()
}

fn expression_pins<'a>(mut expression: &str, inputs: &[&'a PinData]) -> Vec<&'a PinData> {
    // Match the identifiers used by the Python clock-role extractor, including
    // numeric bus selectors. Clock-role inference only needs pin references.
    let mut names = HashSet::new();
    while let Some(start) = expression.find(|ch: char| ch.is_ascii_alphabetic() || ch == '_') {
        let candidate = &expression[start..];
        let base_end = candidate
            .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .unwrap_or(candidate.len());
        let mut end = base_end;
        if let Some(index) = candidate[base_end..].strip_prefix('[') {
            let digits = index.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 && index.as_bytes().get(digits) == Some(&b']') {
                end += digits + 2;
            }
        }
        names.insert(&candidate[..end]);
        expression = &candidate[end..];
    }
    inputs
        .iter()
        .copied()
        .filter(|pin| names.contains(pin.name.as_str()))
        .collect()
}
