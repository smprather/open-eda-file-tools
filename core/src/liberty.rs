use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Unit scales resolved from the library header and inherited by every cell
/// and pin, so that scalar accessors can report values with their scale
/// attached (and SI-converted variants where useful).
#[derive(Clone, Copy, Default)]
pub struct UnitContext {
    /// Seconds per library time unit (`time_unit : "1ns"` -> 1e-9).
    pub time: Option<f64>,
    /// Farads per library capacitive-load unit
    /// (`capacitive_load_unit (1, ff)` -> 1e-15).
    pub capacitance: Option<f64>,
    /// Watts per library power unit (voltage * current).
    pub power: Option<f64>,
}

impl UnitContext {
    pub fn from_library(
        voltage: Option<&str>,
        current: Option<&str>,
        time: Option<&str>,
        capacitance_multiplier: Option<f64>,
        capacitance_unit: Option<&str>,
        leakage_power: Option<&str>,
    ) -> Self {
        let voltage = voltage.and_then(unit_scale);
        let current = current.and_then(unit_scale);
        let time = time.and_then(unit_scale);
        let capacitance = capacitance_unit
            .and_then(unit_scale)
            .map(|scale| scale * capacitance_multiplier.unwrap_or(1.0));
        // Leakage power uses its own unit when the library declares one
        // (`leakage_power_unit : "1pW"`); otherwise it is voltage * current.
        let power = leakage_power
            .and_then(unit_scale)
            .or_else(|| match (voltage, current) {
                (Some(v), Some(c)) => Some(v * c),
                _ => None,
            });
        Self {
            time,
            capacitance,
            power,
        }
    }
}

/// The kind of group that owns a pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    Bus,
    Bundle,
}

impl ContainerKind {
    /// The Liberty keyword of the group this kind stands for.
    pub fn keyword(&self) -> &'static str {
        match self {
            ContainerKind::Bus => "bus",
            ContainerKind::Bundle => "bundle",
        }
    }
}

/// What makes an edited attribute unique: the cell, the pin, the container
/// that owns the pin and the attribute's key.
pub type EditKey = (
    String,
    Option<String>,
    Option<(ContainerKind, String)>,
    String,
);

/// A pending attribute edit recorded by `Cell::set_attribute` /
/// `Pin::set_attribute`. The writer replays these against the original
/// source text.
#[derive(Clone, Debug)]
pub struct AttrEdit {
    pub cell: String,
    /// `None` for a cell-level attribute, `Some(pin)` for a pin attribute.
    pub pin: Option<String>,
    /// The bus or bundle that owns the pin, `None` for a cell-level pin.
    pub container: Option<(ContainerKind, String)>,
    pub key: String,
    pub value: String,
}

#[derive(Clone)]
pub struct Document {
    pub library_name: String,
    pub voltage_unit: Option<String>,
    pub current_unit: Option<String>,
    pub time_unit: Option<String>,
    /// Second arg of `capacitive_load_unit (1, ff)` -> "ff".
    pub capacitive_load_unit: Option<String>,
    /// template name -> [variable_1, variable_2, variable_3] from
    /// `lu_table_template` / `power_lut_template` / `output_current_template`.
    pub templates: Vec<(String, [Option<String>; 3])>,
    /// Ordered library-group simple/complex attributes (technology, delay_model,
    /// nom_*, default_*, thresholds, …) — everything that isn't a sub-group.
    pub attributes: Vec<(String, String)>,
    /// `operating_conditions` groups: name -> simple/complex attributes.
    pub operating_conditions: Vec<(String, Vec<(String, String)>)>,
    /// First arg of `capacitive_load_unit (1, ff)` -> the "1" multiplier.
    pub capacitive_load_multiplier: Option<f64>,
    /// `leakage_power_unit` (e.g. "1pW"), the unit of leakage power values.
    pub leakage_power_unit: Option<String>,
    /// `normalized_driver_waveform` tables: index_1 = input slew, index_2 =
    /// normalized voltage (0..1), values = time. `name` = driver_waveform_name.
    pub driver_waveforms: Vec<TimingTableData>,
    pub cells: Vec<CellData>,
    pub bus_types: Vec<BusTypeData>,
    /// Path the document was parsed from (None for synthetic documents).
    pub source_path: Option<String>,
    /// Byte length of the source, used to detect a changed file before writing.
    pub source_len: usize,
    /// FNV-1a hash of the source bytes.
    pub source_hash: u64,
    /// Byte span of every parsed cell group: (name, start, end).
    pub cell_spans: Vec<(String, usize, usize)>,
    /// Pending attribute edits, shared with the `Cell`/`Pin` handles handed out
    /// by `Document::cell` / `Cell::pin` so that mutations survive the copy.
    pub edits: Arc<Mutex<Vec<AttrEdit>>>,
    /// Unit scales resolved from the library header.
    pub units: UnitContext,
}

/// An `internal_power` group. Despite the Liberty name, the table values are
/// switching **energy** (joules in SI), not power; scale = voltage * current *
/// time unit (see `Document::energy_unit_joules`).

#[derive(Clone)]
pub struct CellData {
    pub name: String,
    pub area: Option<f64>,
    pub pins: Vec<PinData>,
    pub buses: Vec<BusData>,
    pub bundles: Vec<BundleData>,
    pub attributes: Vec<(String, String)>,
    pub dynamic_currents: Vec<DynamicCurrentData>,
    pub ff: Option<FfData>,
    pub latch: Option<LatchData>,
    pub leakage_powers: Vec<LeakagePowerData>,
    pub pg_pins: Vec<PgPinData>,
    pub units: UnitContext,
}

/// A `pg_pin` group (power/ground rail): name + its simple attributes
/// (pg_type, voltage_name, …).
#[derive(Clone)]
pub struct PgPinData {
    pub name: String,
    pub attributes: Vec<(String, String)>,
}

/// A `leakage_power` group: a state-dependent (or default) static leakage value,
/// optionally per power rail.
#[derive(Clone, Default)]
pub struct LeakagePowerData {
    pub value: Option<f64>,
    pub when: Option<String>,
    pub related_pg_pin: Option<String>,
}

/// A flip-flop (`ff` / `ff_bank`) group. The two header args are the
/// state-variable names (e.g. `IQ, IQN`); the rest are boolean expressions.
#[derive(Clone, Default)]
pub struct FfData {
    pub variable1: Option<String>,
    pub variable2: Option<String>,
    pub next_state: Option<String>,
    pub clocked_on: Option<String>,
    pub clocked_on_also: Option<String>,
    pub clear: Option<String>,
    pub preset: Option<String>,
    pub power_down_function: Option<String>,
}

/// A latch (`latch` / `latch_bank`) group.
#[derive(Clone, Default)]
pub struct LatchData {
    pub variable1: Option<String>,
    pub variable2: Option<String>,
    pub enable: Option<String>,
    pub data_in: Option<String>,
    pub clear: Option<String>,
    pub preset: Option<String>,
}

/// CCS power (`dynamic_current`): per-condition power/ground current waveforms.
#[derive(Clone)]
pub struct DynamicCurrentData {
    pub related_inputs: Option<String>,
    pub related_outputs: Option<String>,
    pub when: Option<String>,
    pub switching_groups: Vec<SwitchingGroupData>,
}

#[derive(Clone)]
pub struct SwitchingGroupData {
    pub input_switching_condition: Option<String>,
    pub output_switching_condition: Option<String>,
    pub pg_currents: Vec<PgCurrentData>,
}

#[derive(Clone)]
pub struct PgCurrentData {
    pub pg_pin: Option<String>,
    /// One `vector` per (slew, cap): a current-vs-time wave (reuses the timing
    /// table's index_1/2 = slew/cap, index_3 = time, values = current).
    pub vectors: Vec<TimingTableData>,
}

#[derive(Clone)]
pub struct PinData {
    pub name: String,
    pub direction: Option<String>,
    pub function: Option<String>,
    pub three_state: Option<String>,
    pub power_down_function: Option<String>,
    pub timing_arcs: Vec<TimingArcData>,
    pub internal_powers: Vec<InternalPowerData>,
    pub attributes: Vec<(String, String)>,
    pub units: UnitContext,
}

#[derive(Clone)]
pub struct InternalPowerData {
    pub related_pin: Option<String>,
    pub related_pg_pin: Option<String>,
    pub when: Option<String>,
    pub tables: Vec<TimingTableData>,
}

#[derive(Clone)]
pub struct BusData {
    pub name: String,
    pub direction: Option<String>,
    pub function: Option<String>,
    pub bus_type: Option<String>,
    pub pins: Vec<PinData>,
    pub timing_arcs: Vec<TimingArcData>,
}

#[derive(Clone)]
pub struct BundleData {
    pub name: String,
    pub direction: Option<String>,
    pub function: Option<String>,
    pub members: Vec<String>,
    pub pins: Vec<PinData>,
    pub timing_arcs: Vec<TimingArcData>,
}

#[derive(Clone)]
pub struct BusTypeData {
    pub name: String,
    pub attributes: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct TimingArcData {
    pub related_pin: Option<String>,
    pub timing_type: Option<String>,
    pub when: Option<String>,
    pub sdf_cond: Option<String>,
    pub timing_sense: Option<String>,
    pub tables: Vec<TimingTableData>,
    pub ccsn_stages: Vec<CcsnStageData>,
}

#[derive(Clone)]
pub struct CcsnStageData {
    pub name: String,
    pub is_inverting: Option<String>,
    pub is_needed: Option<String>,
    pub is_pass_gate: Option<String>,
    pub stage_type: Option<String>,
    pub miller_cap_rise: Option<f64>,
    pub miller_cap_fall: Option<f64>,
    pub when: Option<String>,
    pub mode: Option<String>,
    pub dc_current: Option<TimingTableData>,
    pub output_voltage_rise: Vec<TimingTableData>,
    pub output_voltage_fall: Vec<TimingTableData>,
}

#[derive(Clone)]
pub struct TimingTableData {
    pub name: String,
    pub index_1: Vec<f64>,
    pub index_2: Vec<f64>,
    pub index_3: Vec<f64>,
    pub values: Vec<f64>,
    pub template: Option<String>,
    /// Axis variable names resolved from the table's template:
    /// [index_1 role, index_2 role, index_3 role].
    pub axis_vars: [Option<String>; 3],
    pub reference_time: Option<f64>,
    pub vectors: Vec<TimingTableData>,
}

pub fn parse_file_document(
    path: &str,
    filter: Option<HashSet<String>>,
    skip_tables: bool,
) -> Result<Document, ParseLoadError> {
    let reader = open_input(path).map_err(ParseLoadError::Io)?;
    let mut parser = Parser::new(reader, filter, skip_tables).map_err(ParseLoadError::Parse)?;
    let mut document = parser.parse_document().map_err(ParseLoadError::Parse)?;
    document.source_path = Some(path.to_string());
    Ok(document)
}

/// Encode the lexer's token stream as strings (`W<value>` for words, `S<char>`
/// for symbols). Comments and whitespace are not tokens, so two files with
/// equal token streams parse identically — the basis for `liberty-format`'s
/// functional-transparency guarantee.
pub fn lex_all(reader: Box<dyn Read>) -> Result<Vec<String>, ParseError> {
    let mut lexer = Lexer::new(reader)?;
    let mut out = Vec::new();
    while let Some(tok) = lexer.next_token()? {
        match tok.kind {
            TokenKind::Word(v) => out.push(format!("W{v}")),
            TokenKind::Symbol(b) => out.push(format!("S{}", b as char)),
        }
    }
    Ok(out)
}

pub fn table_position(table: &TimingTableData, idx: usize) -> (usize, usize, usize) {
    if !table.index_3.is_empty() {
        let n3 = table.index_3.len();
        let n2 = table.index_2.len().max(1);
        let plane = n2 * n3;
        (idx / plane, (idx % plane) / n3, idx % n3)
    } else if !table.index_2.is_empty() {
        let n2 = table.index_2.len();
        (idx / n2, idx % n2, 0)
    } else if !table.index_1.is_empty() {
        (idx, 0, 0)
    } else {
        (0, idx, 0)
    }
}

pub fn matches_opt(filter: Option<&str>, actual: &str) -> bool {
    filter.map_or(true, |value| value == actual)
}

pub fn matches_opt_opt(filter: Option<&str>, actual: Option<&str>) -> bool {
    filter.map_or(true, |value| actual == Some(value))
}

pub struct WhenFilter {
    pub query: Option<BoolExpr>,
}

impl WhenFilter {
    pub fn new(query: Option<&str>) -> Result<Self, String> {
        let query = query
            .map(parse_bool_expr)
            .transpose()
            .map_err(|err| format!("invalid when expression: {err}"))?;
        Ok(Self { query })
    }

    pub fn matches(&self, actual: Option<&str>) -> bool {
        let Some(query) = &self.query else {
            return true;
        };
        let Some(actual) = actual else {
            return false;
        };
        parse_bool_expr(actual)
            .map(|actual| bool_implies(&actual, query))
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug)]
pub enum BoolExpr {
    Var(String),
    Const(bool),
    Not(Box<BoolExpr>),
    And(Box<BoolExpr>, Box<BoolExpr>),
    Or(Box<BoolExpr>, Box<BoolExpr>),
    Xor(Box<BoolExpr>, Box<BoolExpr>),
}

impl BoolExpr {
    pub fn eval(&self, true_vars: &HashSet<String>) -> bool {
        match self {
            BoolExpr::Var(name) => true_vars.contains(name),
            BoolExpr::Const(value) => *value,
            BoolExpr::Not(expr) => !expr.eval(true_vars),
            BoolExpr::And(left, right) => left.eval(true_vars) && right.eval(true_vars),
            BoolExpr::Or(left, right) => left.eval(true_vars) || right.eval(true_vars),
            BoolExpr::Xor(left, right) => left.eval(true_vars) ^ right.eval(true_vars),
        }
    }

    pub fn collect_vars(&self, vars: &mut Vec<String>) {
        match self {
            BoolExpr::Var(name) => {
                if !vars.iter().any(|item| item == name) {
                    vars.push(name.clone());
                }
            }
            BoolExpr::Const(_) => {}
            BoolExpr::Not(expr) => expr.collect_vars(vars),
            BoolExpr::And(left, right) | BoolExpr::Or(left, right) | BoolExpr::Xor(left, right) => {
                left.collect_vars(vars);
                right.collect_vars(vars);
            }
        }
    }
}

pub fn bool_implies(actual: &BoolExpr, query: &BoolExpr) -> bool {
    let mut vars = Vec::new();
    actual.collect_vars(&mut vars);
    query.collect_vars(&mut vars);
    let assignment_count = 1usize.checked_shl(vars.len() as u32).unwrap_or(0);
    if assignment_count == 0 {
        return false;
    }

    for mask in 0..assignment_count {
        let mut true_vars = HashSet::new();
        for (idx, name) in vars.iter().enumerate() {
            if (mask & (1usize << idx)) != 0 {
                true_vars.insert(name.clone());
            }
        }
        if actual.eval(&true_vars) && !query.eval(&true_vars) {
            return false;
        }
    }
    true
}

pub fn parse_bool_expr(input: &str) -> Result<BoolExpr, String> {
    let mut parser = BoolParser::new(input);
    let expr = parser.parse_or()?;
    parser.skip_ws();
    if parser.is_eof() {
        Ok(expr)
    } else {
        Err(format!("unexpected token at byte {}", parser.pos))
    }
}

pub struct BoolParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> BoolParser<'a> {
    pub fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    pub fn parse_or(&mut self) -> Result<BoolExpr, String> {
        let mut expr = self.parse_xor()?;
        loop {
            self.skip_ws();
            if self.consume('|') || self.consume('+') {
                let right = self.parse_xor()?;
                expr = BoolExpr::Or(Box::new(expr), Box::new(right));
            } else {
                return Ok(expr);
            }
        }
    }

    pub fn parse_xor(&mut self) -> Result<BoolExpr, String> {
        let mut expr = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.consume('^') {
                let right = self.parse_and()?;
                expr = BoolExpr::Xor(Box::new(expr), Box::new(right));
            } else {
                return Ok(expr);
            }
        }
    }

    pub fn parse_and(&mut self) -> Result<BoolExpr, String> {
        let mut expr = self.parse_not()?;
        loop {
            self.skip_ws();
            if self.consume('&') || self.consume('*') {
                let right = self.parse_not()?;
                expr = BoolExpr::And(Box::new(expr), Box::new(right));
            } else if self.at_primary_start() {
                // Liberty Table 7-4: a space is AND, so `A B` == `A & B`. The
                // suffix `'` is excluded below, so `A'B` parses as Not(A) & B.
                let right = self.parse_not()?;
                expr = BoolExpr::And(Box::new(expr), Box::new(right));
            } else {
                return Ok(expr);
            }
        }
    }

    /// True if the next non-whitespace char can begin a primary/`not` term,
    /// i.e. an implicit (juxtaposition) AND should be inserted here. False for
    /// the binary/suffix operators and `)` (which terminate an AND chain) and
    /// for end-of-input.
    pub fn at_primary_start(&self) -> bool {
        let mut probe = self.pos;
        while let Some(ch) = self.input[probe..].chars().next() {
            if ch.is_whitespace() {
                probe += ch.len_utf8();
            } else {
                return !matches!(ch, '|' | '+' | '^' | '&' | '*' | '\'' | ')');
            }
        }
        false
    }

    pub fn parse_not(&mut self) -> Result<BoolExpr, String> {
        self.skip_ws();
        if self.consume('!') || self.consume('~') {
            Ok(BoolExpr::Not(Box::new(self.parse_not()?)))
        } else {
            let mut expr = self.parse_primary()?;
            loop {
                self.skip_ws();
                if self.consume('\'') {
                    expr = BoolExpr::Not(Box::new(expr));
                } else {
                    return Ok(expr);
                }
            }
        }
    }

    pub fn parse_primary(&mut self) -> Result<BoolExpr, String> {
        self.skip_ws();
        if self.consume('(') {
            let expr = self.parse_or()?;
            self.skip_ws();
            if !self.consume(')') {
                return Err(format!("expected ')' at byte {}", self.pos));
            }
            return Ok(expr);
        }
        let ident = self.parse_ident()?;
        match ident.as_str() {
            "1" | "true" | "TRUE" => Ok(BoolExpr::Const(true)),
            "0" | "false" | "FALSE" => Ok(BoolExpr::Const(false)),
            _ => Ok(BoolExpr::Var(ident)),
        }
    }

    pub fn parse_ident(&mut self) -> Result<String, String> {
        self.skip_ws();
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch.is_whitespace()
                || matches!(
                    ch,
                    '!' | '~' | '&' | '*' | '|' | '+' | '^' | '\'' | '(' | ')'
                )
            {
                break;
            }
            self.pos += ch.len_utf8();
        }
        if self.pos == start {
            Err(format!("expected identifier at byte {}", self.pos))
        } else {
            Ok(self.input[start..self.pos].to_string())
        }
    }

    pub fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(ch) if ch.is_whitespace()) {
            self.pos += self.peek().unwrap().len_utf8();
        }
    }

    pub fn consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }

    pub fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    pub fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }
}

// ---- canonical / minimized boolean forms -----------------------------------
//
// Enumeration is over the *mentioned* variables (support discovery needs the
// full truth table), so the 2^n cost is bounded by this cap rather than the
// support size. Real Liberty `function`/`when` expressions use a handful of
// variables; 12 is a generous ceiling and keeps both the truth-table and the
// Quine-McCluskey minimizer fast.
pub const BOOL_VAR_CAP: usize = 12;

/// Render one product term: literals sorted alphabetically by variable name,
/// each as `VAR` or `!VAR`, joined by `*`.
pub fn render_term(mut lits: Vec<(String, bool)>) -> String {
    lits.sort_by(|a, b| a.0.cmp(&b.0));
    lits.into_iter()
        .map(|(name, neg)| if neg { format!("!{name}") } else { name })
        .collect::<Vec<_>>()
        .join("*")
}

/// Canonical sum-of-minterms over the expression's *support* (vacuous variables
/// eliminated, so `A & (B | !B)` canonicalizes to `A`). Returns the sorted
/// support, the sorted product-term strings, and their `+`-joined canonical
/// string. Tautology -> "1", contradiction -> "0". `mentioned` must be the
/// sorted, cap-checked list of variables the expression names.
pub fn canonical_minterm_parts(
    expr: &BoolExpr,
    mentioned: &[String],
) -> (Vec<String>, Vec<String>, String) {
    let n = mentioned.len();
    let rows = 1usize << n;

    // Full truth table over mentioned vars; mentioned[0] is the MSB.
    let mut outputs = vec![false; rows];
    for (mask, out) in outputs.iter_mut().enumerate() {
        let mut true_vars = HashSet::new();
        for (i, name) in mentioned.iter().enumerate() {
            if (mask >> (n - 1 - i)) & 1 == 1 {
                true_vars.insert(name.clone());
            }
        }
        *out = expr.eval(&true_vars);
    }

    // Support: a var is kept iff flipping only it changes the output somewhere.
    let mut in_support = vec![false; n];
    for (j, sup) in in_support.iter_mut().enumerate() {
        let jbit = n - 1 - j;
        for mask in 0..rows {
            if (mask >> jbit) & 1 == 0 && outputs[mask] != outputs[mask | (1 << jbit)] {
                *sup = true;
                break;
            }
        }
    }
    let support: Vec<String> = mentioned
        .iter()
        .zip(&in_support)
        .filter(|&(_, &keep)| keep)
        .map(|(name, _)| name.clone())
        .collect();

    if !outputs.iter().any(|&b| b) {
        return (support, Vec::new(), "0".to_string());
    }
    if outputs.iter().all(|&b| b) {
        return (support, Vec::new(), "1".to_string());
    }

    // One product term per ON row of the support-only truth table.
    let k = support.len();
    let support_bits: Vec<usize> = support
        .iter()
        .map(|name| n - 1 - mentioned.iter().position(|m| m == name).unwrap())
        .collect();
    let mut parts: Vec<String> = Vec::new();
    for srow in 0..(1usize << k) {
        let mut mask = 0usize;
        for (s, &bitpos) in support_bits.iter().enumerate() {
            if (srow >> (k - 1 - s)) & 1 == 1 {
                mask |= 1 << bitpos;
            }
        }
        if !outputs[mask] {
            continue;
        }
        let lits: Vec<(String, bool)> = support
            .iter()
            .enumerate()
            .map(|(s, name)| (name.clone(), (srow >> (k - 1 - s)) & 1 == 0))
            .collect();
        parts.push(render_term(lits));
    }
    parts.sort();
    let canonical = parts.join(" + ");
    (support, parts, canonical)
}

/// Convert our `BoolExpr` into the minimizer crate's `Bool`, mapping variable
/// names to indices by their position in `mentioned` (XOR is expanded since the
/// crate has no XOR node).
pub fn boolexpr_to_qmc(expr: &BoolExpr, mentioned: &[String]) -> quine_mc_cluskey::Bool {
    use quine_mc_cluskey::Bool;
    match expr {
        BoolExpr::Const(true) => Bool::True,
        BoolExpr::Const(false) => Bool::False,
        BoolExpr::Var(name) => Bool::Term(mentioned.iter().position(|m| m == name).unwrap() as u8),
        BoolExpr::Not(e) => Bool::Not(Box::new(boolexpr_to_qmc(e, mentioned))),
        BoolExpr::And(l, r) => Bool::And(vec![
            boolexpr_to_qmc(l, mentioned),
            boolexpr_to_qmc(r, mentioned),
        ]),
        BoolExpr::Or(l, r) => Bool::Or(vec![
            boolexpr_to_qmc(l, mentioned),
            boolexpr_to_qmc(r, mentioned),
        ]),
        BoolExpr::Xor(l, r) => Bool::Or(vec![
            Bool::And(vec![
                boolexpr_to_qmc(l, mentioned),
                Bool::Not(Box::new(boolexpr_to_qmc(r, mentioned))),
            ]),
            Bool::And(vec![
                Bool::Not(Box::new(boolexpr_to_qmc(l, mentioned))),
                boolexpr_to_qmc(r, mentioned),
            ]),
        ]),
    }
}

pub fn collect_qmc_literals(
    b: &quine_mc_cluskey::Bool,
    mentioned: &[String],
    out: &mut Vec<(String, bool)>,
) {
    use quine_mc_cluskey::Bool;
    match b {
        Bool::Term(i) => out.push((mentioned[*i as usize].clone(), false)),
        Bool::Not(inner) => {
            if let Bool::Term(i) = inner.as_ref() {
                out.push((mentioned[*i as usize].clone(), true));
            }
        }
        Bool::And(items) => items
            .iter()
            .for_each(|it| collect_qmc_literals(it, mentioned, out)),
        _ => {}
    }
}

pub fn render_qmc_sop(b: &quine_mc_cluskey::Bool, mentioned: &[String]) -> String {
    use quine_mc_cluskey::Bool;
    match b {
        Bool::True => "1".to_string(),
        Bool::False => "0".to_string(),
        Bool::Or(terms) => {
            let mut parts: Vec<String> = terms
                .iter()
                .map(|t| {
                    let mut lits = Vec::new();
                    collect_qmc_literals(t, mentioned, &mut lits);
                    render_term(lits)
                })
                .collect();
            parts.sort();
            parts.join(" + ")
        }
        other => {
            let mut lits = Vec::new();
            collect_qmc_literals(other, mentioned, &mut lits);
            render_term(lits)
        }
    }
}

/// Minimized sum-of-products (Quine-McCluskey). `simplify()` can return several
/// equally-minimal covers; render each and pick the lexicographically smallest
/// so the result is deterministic. Constants short-circuit via `canonical`.
pub fn minimized_sop(expr: &BoolExpr, mentioned: &[String], canonical: &str) -> String {
    if canonical == "0" || canonical == "1" {
        return canonical.to_string();
    }
    let mut rendered: Vec<String> = boolexpr_to_qmc(expr, mentioned)
        .simplify()
        .iter()
        .map(|sol| render_qmc_sop(sol, mentioned))
        .collect();
    rendered.sort();
    rendered
        .into_iter()
        .next()
        .unwrap_or_else(|| canonical.to_string())
}

pub fn open_input(path: &str) -> io::Result<Box<dyn Read>> {
    let file = File::open(path)?;
    let reader = BufReader::with_capacity(128 * 1024, file);
    if path.ends_with(".gz") {
        Ok(Box::new(GzDecoder::new(reader)))
    } else {
        Ok(Box::new(reader))
    }
}

// ---- byte-offset cell indexing (lazy-parse backend) ------------------------
//
// Scan the whole (decompressed) file once, recording the byte range of every
// top-level `cell (...) {...}` group, so the viewer can list cells and parse one
// at a time instead of parsing the entire file up front. The scanner is lexer-
// faithful about strings/comments but does *not* build any AST.

pub fn read_to_memory(path: &str) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    open_input(path)?.read_to_end(&mut buf)?;
    Ok(buf)
}

pub fn is_index_symbol(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'{' | b'}' | b':' | b';' | b',')
}

/// Skip whitespace, `#`/`//`/`* */` comments, and `\`+newline continuations.
pub fn skip_trivia(d: &[u8], mut i: usize) -> usize {
    let n = d.len();
    loop {
        while i < n && d[i].is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < n && d[i] == b'\\' && (d[i + 1] == b'\n' || d[i + 1] == b'\r') {
            i += 2;
        } else if i < n && d[i] == b'#' {
            while i < n && d[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < n && d[i] == b'/' && d[i + 1] == b'/' {
            while i < n && d[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < n && d[i] == b'/' && d[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(d[i] == b'*' && d[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
        } else {
            break;
        }
    }
    i
}

/// Consume a quoted string starting at `d[i] == quote`; return index after the
/// closing quote, honoring backslash escapes.
pub fn skip_quoted(d: &[u8], mut i: usize) -> usize {
    let n = d.len();
    let quote = d[i];
    i += 1;
    while i < n {
        if d[i] == b'\\' {
            i += 2;
            continue;
        }
        if d[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    i
}

/// Read one token (bare word or quoted string) at `i`; return (value, next).
pub fn read_token(d: &[u8], i: usize) -> (String, usize) {
    let n = d.len();
    if i < n && (d[i] == b'"' || d[i] == b'\'') {
        let end = skip_quoted(d, i);
        let inner = &d[i + 1..end.saturating_sub(1).max(i + 1)];
        return (String::from_utf8_lossy(inner).into_owned(), end);
    }
    let start = i;
    let mut j = i;
    while j < n && !d[j].is_ascii_whitespace() && !is_index_symbol(d[j]) {
        j += 1;
    }
    (String::from_utf8_lossy(&d[start..j]).into_owned(), j)
}

/// At `d[i] == '('`, read the comma-separated args (quotes stripped); return
/// (args, index after the matching ')').
pub fn read_paren_args(d: &[u8], mut i: usize) -> (Vec<String>, usize) {
    let n = d.len();
    i += 1; // past '('
    let mut args = Vec::new();
    let mut cur = String::new();
    while i < n {
        i = skip_trivia(d, i);
        if i >= n {
            break;
        }
        match d[i] {
            b')' => {
                i += 1;
                break;
            }
            b',' => {
                args.push(cur.trim().to_string());
                cur.clear();
                i += 1;
            }
            b'"' | b'\'' => {
                let end = skip_quoted(d, i);
                cur.push_str(&String::from_utf8_lossy(
                    &d[i + 1..end.saturating_sub(1).max(i + 1)],
                ));
                i = end;
            }
            _ => {
                cur.push(d[i] as char);
                i += 1;
            }
        }
    }
    if !cur.trim().is_empty() || !args.is_empty() {
        args.push(cur.trim().to_string());
    }
    (args, i)
}

/// At `d[i] == '{'`, return the index just past the matching `}` (string/comment
/// aware).
pub fn match_braces(d: &[u8], mut i: usize) -> usize {
    let n = d.len();
    let mut depth = 0usize;
    while i < n {
        match d[i] {
            b'"' | b'\'' => i = skip_quoted(d, i),
            b'#' => {
                while i < n && d[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && d[i + 1] == b'/' => {
                while i < n && d[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && d[i + 1] == b'*' => {
                i += 2;
                while i + 1 < n && !(d[i] == b'*' && d[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(n);
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => i += 1,
        }
    }
    i
}

pub fn skip_to_semicolon(d: &[u8], mut i: usize) -> usize {
    let n = d.len();
    while i < n {
        match d[i] {
            b'"' | b'\'' => i = skip_quoted(d, i),
            b';' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// The top-level structure of a Liberty file, found by a byte-level scan.
pub struct LibraryScan {
    /// Name of the library group.
    pub name: String,
    /// Byte range of the library body before the first cell group.
    pub preamble: (usize, usize),
    /// Byte ranges of the top-level items that follow the first cell group
    /// (units, templates, operating conditions, …), in source order.
    pub late_items: Vec<(usize, usize)>,
    /// (cell name, start, end) of every cell group.
    pub cells: Vec<(String, usize, usize)>,
}

/// Index every top-level `cell` group. Lenient: a stray `}` that drops
/// depth early does not stop cell discovery.
///
/// The scan also records the top-level items that follow the first cell:
/// a library may declare its units or its lookup-table templates *after*
/// the cells that use them, and `parse_file` resolves them there too.
pub fn index_cells(d: &[u8]) -> LibraryScan {
    let n = d.len();
    let mut i = skip_trivia(d, 0);
    let (_kw, j) = read_token(d, i);
    i = skip_trivia(d, j);
    let mut lib_name = String::new();
    if i < n && d[i] == b'(' {
        let (args, k) = read_paren_args(d, i);
        lib_name = args.into_iter().next().unwrap_or_default();
        i = skip_trivia(d, k);
    }
    if i < n && d[i] == b'{' {
        i += 1;
    }
    let body_start = i;
    let mut cells = Vec::new();
    // Byte ranges of the top-level items that are not cells.
    let mut items: Vec<(usize, usize)> = Vec::new();
    let mut first_cell: Option<usize> = None;
    while i < n {
        i = skip_trivia(d, i);
        if i >= n {
            break;
        }
        if d[i] == b'}' {
            i += 1; // library close or stray — keep scanning
            continue;
        }
        let tok_start = i;
        let (name, j) = read_token(d, i);
        if name.is_empty() {
            i += 1;
            continue;
        }
        i = skip_trivia(d, j);
        if i < n && d[i] == b'(' {
            let (args, k) = read_paren_args(d, i);
            i = skip_trivia(d, k);
            if i < n && d[i] == b'{' {
                let end = match_braces(d, i);
                let is_cell = name == "cell";
                if is_cell {
                    if first_cell.is_none() {
                        first_cell = Some(tok_start);
                    }
                    cells.push((args.into_iter().next().unwrap_or_default(), tok_start, end));
                }
                i = end;
                let s = skip_trivia(d, i);
                if s < n && d[s] == b';' {
                    i = s + 1;
                }
                if !is_cell {
                    items.push((tok_start, i));
                }
            } else if i < n && d[i] == b';' {
                i += 1;
                items.push((tok_start, i));
            }
        } else if i < n && d[i] == b':' {
            i = skip_to_semicolon(d, i + 1);
            items.push((tok_start, i));
        } else {
            i += 1;
        }
    }
    let header_end = first_cell.unwrap_or(i);
    // The items before the first cell are part of the preamble already; only
    // the ones after it have to be collected separately.
    let late_items = match first_cell {
        Some(start) => items
            .into_iter()
            .filter(|(item_start, _)| *item_start >= start)
            .collect(),
        None => Vec::new(),
    };
    LibraryScan {
        name: lib_name,
        preamble: (body_start, header_end),
        late_items,
        cells,
    }
}

pub fn parse_slice_as_library(
    name: &str,
    body: &[u8],
    skip_tables: bool,
) -> Result<Document, ParseError> {
    let mut buf = Vec::with_capacity(body.len() + 32);
    buf.extend_from_slice(format!("library ({name}) {{\n").as_bytes());
    buf.extend_from_slice(body);
    buf.extend_from_slice(b"\n}\n");
    let reader: Box<dyn Read> = Box::new(io::Cursor::new(buf));
    Parser::new(reader, None, skip_tables)?.parse_document()
}

/// Cell-offset index over an in-memory Liberty file: list cells cheaply, parse
/// one cell at a time. Built for multi-GB files where a full parse is too slow.
pub struct LibraryIndex {
    pub data: Vec<u8>,
    pub library_name: String,
    pub order: Vec<String>,
    pub ranges: HashMap<String, (usize, usize)>,
    pub header: Document,
}

impl LibraryIndex {
    pub fn parse_cell_by_name(&self, name: &str) -> Result<Cell, CellLookupError> {
        let &(start, end) = self
            .ranges
            .get(name)
            .ok_or_else(|| CellLookupError::Unknown(name.to_string()))?;
        let mut doc = parse_slice_as_library(&self.library_name, &self.data[start..end], false)
            .map_err(CellLookupError::Parse)?;
        // The cell slice is parsed on its own, so the library header's units
        // and templates have to be applied to it here.
        if let Some(cell) = doc.cells.first_mut() {
            apply_units(std::slice::from_mut(cell), self.header.units);
            resolve_axis_vars(std::slice::from_mut(cell), &self.header.templates);
        }
        doc.cells
            .into_iter()
            .next()
            .ok_or_else(|| CellLookupError::Empty(name.to_string()))
    }

    pub fn cell_source_by_name(&self, name: &str) -> Result<String, CellSourceError> {
        let &(start, end) = self
            .ranges
            .get(name)
            .ok_or_else(|| CellSourceError::Unknown(name.to_string()))?;
        Ok(String::from_utf8_lossy(&self.data[start..end]).into_owned())
    }
}

pub fn open_library_index(path: &str) -> Result<LibraryIndex, ParseLoadError> {
    let data = read_to_memory(path).map_err(ParseLoadError::Io)?;
    let scan = index_cells(&data);
    // The library header is the preamble *and* the top-level items that
    // follow the first cell: a library may declare its units or its
    // lookup-table templates after the cells that use them, and the full
    // parser resolves them there too.
    let mut header_bytes = Vec::new();
    header_bytes.extend_from_slice(&data[scan.preamble.0..scan.preamble.1]);
    for (start, end) in &scan.late_items {
        if !header_bytes.ends_with(b"\n") {
            header_bytes.push(b'\n');
        }
        header_bytes.extend_from_slice(&data[*start..*end]);
    }
    let header =
        parse_slice_as_library(&scan.name, &header_bytes, false).map_err(ParseLoadError::Parse)?;
    let mut order = Vec::with_capacity(scan.cells.len());
    let mut ranges = HashMap::with_capacity(scan.cells.len());
    for (name, start, end) in scan.cells {
        if !ranges.contains_key(&name) {
            order.push(name.clone());
        }
        ranges.insert(name, (start, end));
    }
    Ok(LibraryIndex {
        data,
        library_name: scan.name,
        order,
        ranges,
        header,
    })
}

#[derive(Debug, Clone)]
pub enum TokenKind {
    Word(String),
    Symbol(u8),
}

#[derive(Debug, Clone)]
pub struct Token {
    kind: TokenKind,
    line: usize,
    column: usize,
    /// Byte offset of the first byte of the token in the (decompressed) source.
    start: usize,
    /// Byte offset just past the last byte of the token.
    end: usize,
}

pub struct Lexer {
    reader: Box<dyn Read>,
    peeked: VecDeque<u8>,
    line: usize,
    column: usize,
    eof: bool,
    /// Byte offset of the next byte to consume (source position).
    offset: usize,
    /// FNV-1a hash over every byte consumed so far. Used to detect that the
    /// source file changed underneath a parsed document before writing it back.
    hash: u64,
}

pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

impl Lexer {
    pub fn new(reader: Box<dyn Read>) -> Result<Self, ParseError> {
        Ok(Self {
            reader,
            peeked: VecDeque::new(),
            line: 1,
            column: 1,
            eof: false,
            offset: 0,
            hash: FNV_OFFSET,
        })
    }

    pub fn next_token(&mut self) -> Result<Option<Token>, ParseError> {
        self.skip_ws_and_comments()?;
        if self.peek()?.is_none() {
            return Ok(None);
        }

        let line = self.line;
        let column = self.column;
        let start = self.offset;
        let byte = self.bump()?.unwrap();
        if is_symbol(byte) {
            return Ok(Some(Token {
                kind: TokenKind::Symbol(byte),
                line,
                column,
                start,
                end: self.offset,
            }));
        }
        if byte == b'"' || byte == b'\'' {
            let value = self.read_quoted(byte, line, column)?;
            return Ok(Some(Token {
                kind: TokenKind::Word(value),
                line,
                column,
                start,
                end: self.offset,
            }));
        }

        let mut value = Vec::new();
        value.push(byte);
        while let Some(next) = self.peek()? {
            if next.is_ascii_whitespace() || is_symbol(next) {
                break;
            }
            if next == b'/' && matches!(self.peek_next()?, Some(b'*' | b'/')) {
                break;
            }
            if next == b'\\' {
                self.bump()?;
                match self.peek()? {
                    Some(b'\n') => {
                        self.bump()?;
                    }
                    Some(b'\r') => {
                        self.bump()?;
                        if self.peek()? == Some(b'\n') {
                            self.bump()?;
                        }
                    }
                    Some(escaped) => {
                        value.push(b'\\');
                        value.push(escaped);
                        self.bump()?;
                    }
                    None => value.push(b'\\'),
                }
                continue;
            }
            value.push(self.bump()?.unwrap());
        }

        Ok(Some(Token {
            kind: TokenKind::Word(String::from_utf8_lossy(&value).into_owned()),
            line,
            column,
            start,
            end: self.offset,
        }))
    }

    pub fn read_quoted(
        &mut self,
        quote: u8,
        start_line: usize,
        start_column: usize,
    ) -> Result<String, ParseError> {
        let mut value = Vec::new();
        let mut escaped = false;
        while let Some(byte) = self.bump()? {
            if escaped {
                match byte {
                    b'\n' => {}
                    b'\r' => {
                        if self.peek()? == Some(b'\n') {
                            self.bump()?;
                        }
                    }
                    _ => value.push(byte),
                }
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                continue;
            }
            if byte == quote {
                return Ok(String::from_utf8_lossy(&value).into_owned());
            }
            value.push(byte);
        }
        Err(ParseError::new(
            start_line,
            start_column,
            "unterminated quoted string",
        ))
    }

    pub fn skip_ws_and_comments(&mut self) -> Result<(), ParseError> {
        loop {
            loop {
                if matches!(self.peek()?, Some(byte) if byte.is_ascii_whitespace()) {
                    self.bump()?;
                } else if self.peek()? == Some(b'\\')
                    && matches!(self.peek_next()?, Some(b'\n' | b'\r'))
                {
                    self.bump()?;
                    if self.peek()? == Some(b'\r') {
                        self.bump()?;
                        if self.peek()? == Some(b'\n') {
                            self.bump()?;
                        }
                    } else {
                        self.bump()?;
                    }
                } else {
                    break;
                }
            }
            match (self.peek()?, self.peek_next()?) {
                (Some(b'#'), _) => self.skip_line(),
                (Some(b'/'), Some(b'/')) => {
                    self.bump()?;
                    self.bump()?;
                    self.skip_line();
                }
                (Some(b'/'), Some(b'*')) => {
                    let line = self.line;
                    let column = self.column;
                    self.bump()?;
                    self.bump()?;
                    self.skip_block_comment(line, column)?;
                }
                _ => return Ok(()),
            }
        }
    }

    pub fn skip_line(&mut self) {
        while let Ok(Some(byte)) = self.bump() {
            if byte == b'\n' {
                break;
            }
        }
    }

    pub fn skip_block_comment(&mut self, line: usize, column: usize) -> Result<(), ParseError> {
        let mut prev = 0;
        while let Some(byte) = self.bump()? {
            if prev == b'*' && byte == b'/' {
                return Ok(());
            }
            prev = byte;
        }
        Err(ParseError::new(line, column, "unterminated block comment"))
    }

    /// Skip bytes until the brace nesting level in `depth` reaches zero.
    ///
    /// Strings and comments are honored so a brace inside `"..."` or a comment
    /// does not change the depth. This is the fast path for table bodies: it
    /// hashes and positions every byte but allocates no tokens or values.
    pub fn skip_braces(&mut self, mut depth: usize) -> Result<(), ParseError> {
        while depth > 0 {
            let Some(byte) = self.bump()? else {
                return Err(ParseError::new(
                    self.line,
                    self.column,
                    "unexpected end of file while skipping group",
                ));
            };
            match byte {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                b'"' | b'\'' => self.skip_quoted_bytes(byte)?,
                b'#' => self.skip_line(),
                b'/' => match self.peek()? {
                    Some(b'/') => {
                        self.bump()?;
                        self.skip_line();
                    }
                    Some(b'*') => {
                        let line = self.line;
                        let column = self.column;
                        self.bump()?;
                        self.skip_block_comment(line, column)?;
                    }
                    _ => {}
                },
                b'\\' => {
                    if matches!(self.peek()?, Some(b'\n' | b'\r')) {
                        let first = self.bump()?.unwrap_or(b'\n');
                        if first == b'\r' && self.peek()? == Some(b'\n') {
                            self.bump()?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn skip_quoted_bytes(&mut self, quote: u8) -> Result<(), ParseError> {
        let line = self.line;
        let column = self.column;
        let mut escaped = false;
        while let Some(byte) = self.bump()? {
            if escaped {
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                continue;
            }
            if byte == quote {
                return Ok(());
            }
        }
        Err(ParseError::new(line, column, "unterminated quoted string"))
    }

    pub fn bump(&mut self) -> Result<Option<u8>, ParseError> {
        self.fill_peek(1)?;
        let Some(byte) = self.peeked.pop_front() else {
            return Ok(None);
        };
        if byte == b'\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        self.offset += 1;
        self.hash ^= byte as u64;
        self.hash = self.hash.wrapping_mul(FNV_PRIME);
        Ok(Some(byte))
    }

    pub fn peek(&mut self) -> Result<Option<u8>, ParseError> {
        self.fill_peek(1)?;
        Ok(self.peeked.front().copied())
    }

    pub fn peek_next(&mut self) -> Result<Option<u8>, ParseError> {
        self.fill_peek(2)?;
        Ok(self.peeked.get(1).copied())
    }

    pub fn fill_peek(&mut self, len: usize) -> Result<(), ParseError> {
        while !self.eof && self.peeked.len() < len {
            let mut byte = [0_u8; 1];
            match self.reader.read(&mut byte)? {
                0 => self.eof = true,
                _ => self.peeked.push_back(byte[0]),
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct LibraryUnits {
    voltage: Option<String>,
    current: Option<String>,
    time: Option<String>,
    capacitive_load: Option<String>,
    capacitive_load_multiplier: Option<f64>,
    leakage_power: Option<String>,
    templates: Vec<(String, [Option<String>; 3])>,
    attributes: Vec<(String, String)>,
    operating_conditions: Vec<(String, Vec<(String, String)>)>,
    driver_waveforms: Vec<TimingTableData>,
}

pub struct Parser {
    lexer: Lexer,
    current: Option<Token>,
    cell_filter: Option<HashSet<String>>,
    /// Skip floating-point table bodies (`cell_rise`, power, CCSN) without
    /// materializing them. Structural metadata (pins, arcs, scalars) is kept.
    skip_tables: bool,
    /// End offset of the most recently consumed token. `advance()` overwrites
    /// it, so it is only meaningful right after a token has been consumed.
    previous_end: usize,
    /// Byte span (start, end) of every parsed cell group, in source order.
    cell_spans: Vec<(String, usize, usize)>,
}

impl Parser {
    pub fn new(
        reader: Box<dyn Read>,
        cell_filter: Option<HashSet<String>>,
        skip_tables: bool,
    ) -> Result<Self, ParseError> {
        let mut parser = Self {
            lexer: Lexer::new(reader)?,
            current: None,
            cell_filter,
            skip_tables,
            previous_end: 0,
            cell_spans: Vec::new(),
        };
        parser.advance()?;
        Ok(parser)
    }

    pub fn parse_document(&mut self) -> Result<Document, ParseError> {
        let mut library_name = String::new();
        let mut cells = Vec::new();
        let mut bus_types = Vec::new();
        let mut units = LibraryUnits::default();

        while self.current.is_some() {
            // A '}' here has no matching open group at top level: the file has
            // unbalanced braces (e.g. the ASAP7 SIMPLE group ships a spurious
            // '}' that closes `library` early). Report it instead of failing
            // later with a cryptic "expected identifier" at the next token.
            if let Some(Token {
                kind: TokenKind::Symbol(b'}'),
                line,
                column,
                ..
            }) = self.current
            {
                return Err(ParseError::new(
                    line,
                    column,
                    "unexpected '}': unbalanced braces (a group has an extra \
                     closing brace, or an opening brace is missing earlier)",
                ));
            }
            let name_pos = self.current_position();
            let name = self.take_word()?;
            let args = if self.consume_symbol(b'(')? {
                self.read_args(&name)?
            } else {
                Vec::new()
            };
            self.expect_symbol(b'{')?;
            if name == "library" {
                library_name = args.first().cloned().unwrap_or_default();
                self.parse_library_body(&mut cells, &mut bus_types, &mut units)?;
            } else {
                // The only valid top-level group is `library`. Anything else
                // here (commonly a `cell` left orphaned by a premature library
                // close) signals unbalanced braces.
                let (line, column) = name_pos;
                return Err(ParseError::new(
                    line,
                    column,
                    &format!(
                        "unexpected top-level {name:?} group: expected a single \
                         'library' group (unbalanced braces may have closed it early)"
                    ),
                ));
            }
            self.consume_symbol(b';')?;
        }

        if library_name.is_empty() {
            return Err(self.error_here("missing top-level library group"));
        }

        // The library header is only fully known once the body has been read
        // (units and templates may be declared after the cells that use them),
        // so resolve them now.
        let unit_context = UnitContext::from_library(
            units.voltage.as_deref(),
            units.current.as_deref(),
            units.time.as_deref(),
            units.capacitive_load_multiplier,
            units.capacitive_load.as_deref(),
            units.leakage_power.as_deref(),
        );
        apply_units(&mut cells, unit_context);
        resolve_axis_vars(&mut cells, &units.templates);
        resolve_axis_vars_in(&mut units.driver_waveforms, &units.templates);

        Ok(Document {
            library_name,
            voltage_unit: units.voltage,
            current_unit: units.current,
            time_unit: units.time,
            capacitive_load_unit: units.capacitive_load,
            templates: units.templates,
            attributes: units.attributes,
            operating_conditions: units.operating_conditions,
            capacitive_load_multiplier: units.capacitive_load_multiplier,
            leakage_power_unit: units.leakage_power,
            driver_waveforms: units.driver_waveforms,
            cells,
            bus_types,
            source_path: None,
            source_len: self.lexer.offset,
            source_hash: self.lexer.hash,
            cell_spans: std::mem::take(&mut self.cell_spans),
            edits: Arc::new(Mutex::new(Vec::new())),
            units: unit_context,
        })
    }

    pub fn parse_library_body(
        &mut self,
        cells: &mut Vec<CellData>,
        bus_types: &mut Vec<BusTypeData>,
        units: &mut LibraryUnits,
    ) -> Result<(), ParseError> {
        while !self.consume_symbol(b'}')? {
            let item_start = self.current_offset();
            let name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&name)?;
                if self.consume_symbol(b'{')? {
                    match name.as_str() {
                        "cell" => {
                            let cell_name = args.first().cloned().unwrap_or_default();
                            if self
                                .cell_filter
                                .as_ref()
                                .map_or(true, |filter| filter.contains(&cell_name))
                            {
                                cells.push(self.parse_cell_body(cell_name.clone())?);
                                // `previous_end` is the end of the cell's closing
                                // brace: the group text is [item_start, end).
                                self.cell_spans
                                    .push((cell_name, item_start, self.previous_end));
                            } else {
                                self.skip_group_body()?;
                            }
                        }
                        "type" => {
                            bus_types.push(
                                self.parse_bus_type_body(
                                    args.first().cloned().unwrap_or_default(),
                                )?,
                            );
                        }
                        "operating_conditions" => {
                            let oc_name = args.first().cloned().unwrap_or_default();
                            let attrs = self.parse_operating_conditions_body()?;
                            units.operating_conditions.push((oc_name, attrs));
                        }
                        "lu_table_template" | "power_lut_template" | "output_current_template" => {
                            let tmpl_name = args.first().cloned().unwrap_or_default();
                            let vars = self.parse_template_body()?;
                            units.templates.push((tmpl_name, vars));
                        }
                        "normalized_driver_waveform" => {
                            let tmpl = args.first().cloned();
                            if self.skip_tables {
                                self.skip_group_body_fast()?;
                            } else {
                                units
                                    .driver_waveforms
                                    .push(self.parse_timing_table_body(String::new(), tmpl)?);
                            }
                        }
                        _ => {
                            self.skip_group_body()?;
                        }
                    }
                    self.consume_symbol(b';')?;
                } else {
                    // Complex attribute, e.g. `capacitive_load_unit (1, ff)`.
                    if name == "capacitive_load_unit" {
                        units.capacitive_load = args.get(1).cloned();
                        units.capacitive_load_multiplier =
                            args.first().and_then(|value| parse_number(value));
                    }
                    units.attributes.push((name, args.join(", ")));
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match name.as_str() {
                    "voltage_unit" => units.voltage = Some(value.clone()),
                    "current_unit" => units.current = Some(value.clone()),
                    "time_unit" => units.time = Some(value.clone()),
                    "leakage_power_unit" => units.leakage_power = Some(value.clone()),
                    _ => {}
                }
                units.attributes.push((name, value));
            } else {
                return Err(self.error_here("expected '(' or ':' after library item name"));
            }
        }
        Ok(())
    }

    pub fn parse_cell_body(&mut self, name: String) -> Result<CellData, ParseError> {
        let mut area = None;
        let mut pins = Vec::new();
        let mut buses = Vec::new();
        let mut bundles = Vec::new();
        let mut attributes = Vec::new();
        let mut dynamic_currents = Vec::new();
        let mut ff = None;
        let mut latch = None;
        let mut leakage_powers = Vec::new();
        let mut pg_pins = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    match item_name.as_str() {
                        "pin" => {
                            pins.push(
                                self.parse_pin_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        "bus" => {
                            buses.push(
                                self.parse_bus_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        "bundle" => {
                            bundles.push(
                                self.parse_bundle_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        "dynamic_current" => {
                            dynamic_currents.push(self.parse_dynamic_current_body()?);
                        }
                        // `ff_bank`/`latch_bank` reuse the same parser; the extra
                        // bit-width arg is ignored. `statetable` is a tabular
                        // state-transition group, not a boolean expression, so
                        // it stays skipped.
                        "ff" | "ff_bank" => {
                            ff = Some(self.parse_ff_body(&args)?);
                        }
                        "latch" | "latch_bank" => {
                            latch = Some(self.parse_latch_body(&args)?);
                        }
                        "leakage_power" => {
                            leakage_powers.push(self.parse_leakage_power_body()?);
                        }
                        "pg_pin" => {
                            pg_pins.push(
                                self.parse_pg_pin_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        _ => {
                            self.skip_group_body()?;
                        }
                    }
                    self.consume_symbol(b';')?;
                } else {
                    attributes.push((item_name, args.join(", ")));
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                if item_name == "area" {
                    area = parse_number(&value);
                }
                attributes.push((item_name, value));
            } else {
                return Err(self.error_here("expected '(' or ':' after cell item name"));
            }
        }
        Ok(CellData {
            name,
            area,
            pins,
            buses,
            bundles,
            attributes,
            dynamic_currents,
            ff,
            latch,
            leakage_powers,
            pg_pins,
            units: UnitContext::default(),
        })
    }

    /// Parse a `pg_pin` body: capture its simple `name : value` attributes;
    /// nested groups are skipped.
    pub fn parse_pg_pin_body(&mut self, name: String) -> Result<PgPinData, ParseError> {
        let mut attributes = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                attributes.push((item_name, value));
            } else {
                return Err(self.error_here("expected '(' or ':' in pg_pin"));
            }
        }
        Ok(PgPinData { name, attributes })
    }

    /// Parse a `leakage_power` body: a `value` plus optional `when` /
    /// `related_pg_pin`. Any nested groups are skipped.
    pub fn parse_leakage_power_body(&mut self) -> Result<LeakagePowerData, ParseError> {
        let mut leak = LeakagePowerData::default();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "value" => leak.value = parse_number(&value),
                    "when" => leak.when = Some(value),
                    "related_pg_pin" => leak.related_pg_pin = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' in leakage_power"));
            }
        }
        Ok(leak)
    }

    /// Parse an `ff` / `ff_bank` body. `args` are the header arguments; the
    /// first two are the state-variable names.
    pub fn parse_ff_body(&mut self, args: &[String]) -> Result<FfData, ParseError> {
        let mut ff = FfData {
            variable1: args.first().cloned(),
            variable2: args.get(1).cloned(),
            ..FfData::default()
        };
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "next_state" => ff.next_state = Some(value),
                    "clocked_on" => ff.clocked_on = Some(value),
                    "clocked_on_also" => ff.clocked_on_also = Some(value),
                    "clear" => ff.clear = Some(value),
                    "preset" => ff.preset = Some(value),
                    "power_down_function" => ff.power_down_function = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' in ff group"));
            }
        }
        Ok(ff)
    }

    /// Parse a `latch` / `latch_bank` body.
    pub fn parse_latch_body(&mut self, args: &[String]) -> Result<LatchData, ParseError> {
        let mut latch = LatchData {
            variable1: args.first().cloned(),
            variable2: args.get(1).cloned(),
            ..LatchData::default()
        };
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "enable" => latch.enable = Some(value),
                    "data_in" => latch.data_in = Some(value),
                    "clear" => latch.clear = Some(value),
                    "preset" => latch.preset = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' in latch group"));
            }
        }
        Ok(latch)
    }

    pub fn parse_dynamic_current_body(&mut self) -> Result<DynamicCurrentData, ParseError> {
        let mut related_inputs = None;
        let mut related_outputs = None;
        let mut when = None;
        let mut switching_groups = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    if item_name == "switching_group" {
                        switching_groups.push(self.parse_switching_group_body()?);
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    // Complex attr (e.g. typical_capacitances) — ignored.
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "related_inputs" => related_inputs = Some(value),
                    "related_outputs" => related_outputs = Some(value),
                    "when" => when = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' in dynamic_current"));
            }
        }
        Ok(DynamicCurrentData {
            related_inputs,
            related_outputs,
            when,
            switching_groups,
        })
    }

    pub fn parse_switching_group_body(&mut self) -> Result<SwitchingGroupData, ParseError> {
        let mut input_switching_condition = None;
        let mut output_switching_condition = None;
        let mut pg_currents = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    if item_name == "pg_current" {
                        pg_currents.push(self.parse_pg_current_body(args.first().cloned())?);
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    // `input_switching_condition (rise)` etc. are complex attrs.
                    match item_name.as_str() {
                        "input_switching_condition" => {
                            input_switching_condition = args.first().cloned();
                        }
                        "output_switching_condition" => {
                            output_switching_condition = args.first().cloned();
                        }
                        _ => {}
                    }
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let _ = self.read_simple_attribute_value()?;
            } else {
                return Err(self.error_here("expected '(' or ':' in switching_group"));
            }
        }
        Ok(SwitchingGroupData {
            input_switching_condition,
            output_switching_condition,
            pg_currents,
        })
    }

    pub fn parse_pg_current_body(
        &mut self,
        pg_pin: Option<String>,
    ) -> Result<PgCurrentData, ParseError> {
        let mut vectors = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    if item_name == "vector" {
                        let tmpl = args.first().cloned();
                        vectors.push(self.parse_timing_table_body("vector".to_string(), tmpl)?);
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let _ = self.read_simple_attribute_value()?;
            } else {
                return Err(self.error_here("expected '(' or ':' in pg_current"));
            }
        }
        Ok(PgCurrentData { pg_pin, vectors })
    }

    pub fn parse_pin_body(&mut self, name: String) -> Result<PinData, ParseError> {
        let mut direction = None;
        let mut function = None;
        let mut three_state = None;
        let mut power_down_function = None;
        let mut timing_arcs = Vec::new();
        let mut internal_powers = Vec::new();
        let mut attributes = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    match item_name.as_str() {
                        "timing" => timing_arcs.push(self.parse_timing_body()?),
                        "internal_power" => internal_powers.push(self.parse_internal_power_body()?),
                        _ => self.skip_group_body()?,
                    }
                    self.consume_symbol(b';')?;
                } else {
                    // Complex attribute, e.g. `rise_capacitance_range (0.55, 0.74)`.
                    attributes.push((item_name, args.join(", ")));
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "direction" => direction = Some(value.clone()),
                    "function" => function = Some(value.clone()),
                    "three_state" => three_state = Some(value.clone()),
                    "power_down_function" => power_down_function = Some(value.clone()),
                    _ => {}
                }
                attributes.push((item_name, value));
            } else {
                return Err(self.error_here("expected '(' or ':' after pin item name"));
            }
        }
        Ok(PinData {
            name,
            direction,
            function,
            three_state,
            power_down_function,
            timing_arcs,
            internal_powers,
            attributes,
            units: UnitContext::default(),
        })
    }

    pub fn parse_internal_power_body(&mut self) -> Result<InternalPowerData, ParseError> {
        let mut related_pin = None;
        let mut related_pg_pin = None;
        let mut when = None;
        let mut tables = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    if is_power_table_group(&item_name) {
                        if self.skip_tables {
                            self.skip_group_body_fast()?;
                        } else {
                            let template = args.first().cloned();
                            tables.push(self.parse_timing_table_body(item_name, template)?);
                        }
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "related_pin" => related_pin = Some(value),
                    "related_pg_pin" => related_pg_pin = Some(value),
                    "when" => when = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after internal_power item name"));
            }
        }
        Ok(InternalPowerData {
            related_pin,
            related_pg_pin,
            when,
            tables,
        })
    }

    pub fn parse_bus_body(&mut self, name: String) -> Result<BusData, ParseError> {
        let mut direction = None;
        let mut function = None;
        let mut bus_type = None;
        let mut pins = Vec::new();
        let mut timing_arcs = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    match item_name.as_str() {
                        "pin" => {
                            pins.push(
                                self.parse_pin_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        "timing" => {
                            timing_arcs.push(self.parse_timing_body()?);
                        }
                        _ => {
                            self.skip_group_body()?;
                        }
                    }
                    self.consume_symbol(b';')?;
                } else {
                    if item_name == "members" {
                        pins.extend(members_from_args(&args).into_iter().map(|name| PinData {
                            name,
                            direction: direction.clone(),
                            function: None,
                            three_state: None,
                            power_down_function: None,
                            timing_arcs: Vec::new(),
                            internal_powers: Vec::new(),
                            attributes: Vec::new(),
                            units: UnitContext::default(),
                        }));
                    }
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "direction" => direction = Some(value),
                    "function" => function = Some(value),
                    "bus_type" => bus_type = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after bus item name"));
            }
        }
        Ok(BusData {
            name,
            direction,
            function,
            bus_type,
            pins,
            timing_arcs,
        })
    }

    pub fn parse_bundle_body(&mut self, name: String) -> Result<BundleData, ParseError> {
        let mut direction = None;
        let mut function = None;
        let mut members = Vec::new();
        let mut pins = Vec::new();
        let mut timing_arcs = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    match item_name.as_str() {
                        "pin" => {
                            pins.push(
                                self.parse_pin_body(args.first().cloned().unwrap_or_default())?,
                            );
                        }
                        "timing" => {
                            timing_arcs.push(self.parse_timing_body()?);
                        }
                        _ => {
                            self.skip_group_body()?;
                        }
                    }
                    self.consume_symbol(b';')?;
                } else {
                    if item_name == "members" {
                        members = members_from_args(&args);
                    }
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "direction" => direction = Some(value),
                    "function" => function = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after bundle item name"));
            }
        }
        Ok(BundleData {
            name,
            direction,
            function,
            members,
            pins,
            timing_arcs,
        })
    }

    pub fn parse_bus_type_body(&mut self, name: String) -> Result<BusTypeData, ParseError> {
        let mut attributes = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b':')? {
                attributes.push((item_name, self.read_simple_attribute_value()?));
            } else if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                } else {
                    self.consume_symbol(b';')?;
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after type item name"));
            }
        }
        Ok(BusTypeData { name, attributes })
    }

    /// Parse an `operating_conditions` body: its simple/complex attributes
    /// (`nom_voltage`, `nom_temperature`, `process`, …). Nested groups are
    /// skipped.
    pub fn parse_operating_conditions_body(&mut self) -> Result<Vec<(String, String)>, ParseError> {
        let mut attributes = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                attributes.push((item_name, args.join(", ")));
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                attributes.push((item_name, value));
            } else {
                return Err(self.error_here("expected '(' or ':' in operating_conditions"));
            }
        }
        Ok(attributes)
    }

    /// Read `variable_1/2/3` from a lookup-table template group, skipping the
    /// index rows. Returns the three axis variable names (any may be None).
    pub fn parse_template_body(&mut self) -> Result<[Option<String>; 3], ParseError> {
        let mut vars: [Option<String>; 3] = [None, None, None];
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let _args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    self.skip_group_body()?;
                }
                self.consume_symbol(b';')?;
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "variable_1" => vars[0] = Some(value),
                    "variable_2" => vars[1] = Some(value),
                    "variable_3" => vars[2] = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' in template body"));
            }
        }
        Ok(vars)
    }

    pub fn parse_timing_body(&mut self) -> Result<TimingArcData, ParseError> {
        let mut related_pin = None;
        let mut timing_type = None;
        let mut when = None;
        let mut sdf_cond = None;
        let mut timing_sense = None;
        let mut tables = Vec::new();
        let mut ccsn_stages = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    if is_ccsn_stage_group(&item_name) {
                        ccsn_stages.push(self.parse_ccsn_stage_body(item_name)?);
                    } else if is_timing_table_group(&item_name) {
                        if self.skip_tables {
                            self.skip_group_body_fast()?;
                        } else {
                            let template = args.first().cloned();
                            tables.push(self.parse_timing_table_body(item_name, template)?);
                        }
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    self.consume_symbol(b';')?;
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "related_pin" => related_pin = Some(value),
                    "timing_type" => timing_type = Some(value),
                    "when" => when = Some(value),
                    "sdf_cond" => sdf_cond = Some(value),
                    "timing_sense" => timing_sense = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after timing item name"));
            }
        }
        Ok(TimingArcData {
            related_pin,
            timing_type,
            when,
            sdf_cond,
            timing_sense,
            tables,
            ccsn_stages,
        })
    }

    pub fn parse_ccsn_stage_body(&mut self, name: String) -> Result<CcsnStageData, ParseError> {
        let mut is_inverting = None;
        let mut is_needed = None;
        let mut is_pass_gate = None;
        let mut stage_type = None;
        let mut miller_cap_rise = None;
        let mut miller_cap_fall = None;
        let mut when = None;
        let mut mode = None;
        let mut dc_current = None;
        let mut output_voltage_rise = Vec::new();
        let mut output_voltage_fall = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    match item_name.as_str() {
                        "dc_current" => {
                            if self.skip_tables {
                                self.skip_group_body_fast()?;
                            } else {
                                let template = args.first().cloned();
                                dc_current =
                                    Some(self.parse_timing_table_body(item_name, template)?);
                            }
                        }
                        "output_voltage_rise" => {
                            if self.skip_tables {
                                self.skip_group_body_fast()?;
                            } else {
                                let table = self.parse_timing_table_body(item_name, None)?;
                                output_voltage_rise = table.vectors;
                            }
                        }
                        "output_voltage_fall" => {
                            if self.skip_tables {
                                self.skip_group_body_fast()?;
                            } else {
                                let table = self.parse_timing_table_body(item_name, None)?;
                                output_voltage_fall = table.vectors;
                            }
                        }
                        "propagated_noise_high" | "propagated_noise_low" => {
                            // TODO: deferred 4D propagated-noise parsing.
                            self.skip_group_body()?;
                        }
                        _ => self.skip_group_body()?,
                    }
                    self.consume_symbol(b';')?;
                } else {
                    self.consume_symbol(b';')?;
                    if item_name == "mode" {
                        mode = Some(args.join(", "));
                    }
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                match item_name.as_str() {
                    "is_inverting" => is_inverting = Some(value),
                    "is_needed" => is_needed = Some(value),
                    "is_pass_gate" => is_pass_gate = Some(value),
                    "stage_type" => stage_type = Some(value),
                    "miller_cap_rise" => miller_cap_rise = parse_number(&value),
                    "miller_cap_fall" => miller_cap_fall = parse_number(&value),
                    "when" => when = Some(value),
                    _ => {}
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after ccsn stage item name"));
            }
        }
        Ok(CcsnStageData {
            name,
            is_inverting,
            is_needed,
            is_pass_gate,
            stage_type,
            miller_cap_rise,
            miller_cap_fall,
            when,
            mode,
            dc_current,
            output_voltage_rise,
            output_voltage_fall,
        })
    }

    pub fn parse_timing_table_body(
        &mut self,
        mut name: String,
        template: Option<String>,
    ) -> Result<TimingTableData, ParseError> {
        let mut index_1 = Vec::new();
        let mut index_2 = Vec::new();
        let mut index_3 = Vec::new();
        let mut values = Vec::new();
        let mut reference_time = None;
        let mut vectors = Vec::new();
        while !self.consume_symbol(b'}')? {
            let item_name = self.take_word()?;
            if self.consume_symbol(b'(')? {
                let args = self.read_args(&item_name)?;
                if self.consume_symbol(b'{')? {
                    // A nested group: CCS `output_current_*` / `ccsn_*` tables
                    // contain `vector` sub-groups, each a current-vs-time wave.
                    if item_name == "vector" {
                        let vtemplate = args.first().cloned();
                        vectors.push(self.parse_timing_table_body(item_name, vtemplate)?);
                    } else {
                        self.skip_group_body()?;
                    }
                    self.consume_symbol(b';')?;
                } else {
                    self.consume_symbol(b';')?;
                    match item_name.as_str() {
                        "index_1" => index_1 = parse_float_list(args.first()),
                        "index_2" => index_2 = parse_float_list(args.first()),
                        "index_3" => index_3 = parse_float_list(args.first()),
                        "values" => values = parse_float_args(&args),
                        _ => {}
                    }
                }
            } else if self.consume_symbol(b':')? {
                let value = self.read_simple_attribute_value()?;
                if item_name == "reference_time" {
                    reference_time = parse_number(&value);
                } else if item_name == "driver_waveform_name" {
                    // normalized_driver_waveform groups name themselves here, not
                    // in the group header (which holds the template name).
                    name = value;
                }
            } else {
                return Err(self.error_here("expected '(' or ':' after table item name"));
            }
        }
        Ok(TimingTableData {
            name,
            index_1,
            index_2,
            index_3,
            values,
            template,
            axis_vars: [None, None, None],
            reference_time,
            vectors,
        })
    }

    pub fn skip_group_body(&mut self) -> Result<(), ParseError> {
        let mut depth = 1usize;
        while let Some(token) = self.current.clone() {
            self.advance()?;
            match token.kind {
                TokenKind::Symbol(b'{') => depth += 1,
                TokenKind::Symbol(b'}') => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err(self.error_here("unexpected end of file while skipping group"))
    }

    /// Skip a group at the byte level instead of tokenizing its body.
    ///
    /// The parser has already consumed the opening `{`, so `current` is the
    /// first body token. It accounts for that token, then lets the lexer skip
    /// the remaining bytes while handling strings, comments and escapes.
    pub fn skip_group_body_fast(&mut self) -> Result<(), ParseError> {
        let Some(token) = self.current.take() else {
            return Err(self.error_here("unexpected end of file while skipping group"));
        };
        let mut depth = 1usize;
        match token.kind {
            TokenKind::Symbol(b'{') => depth += 1,
            TokenKind::Symbol(b'}') => depth -= 1,
            _ => {}
        }
        if depth > 0 {
            self.lexer.skip_braces(depth)?;
        }
        self.current = self.lexer.next_token()?;
        Ok(())
    }

    pub fn read_args(&mut self, name: &str) -> Result<Vec<String>, ParseError> {
        let mut args = Vec::new();
        let allow_spaces = matches!(
            name,
            "input_switching_condition" | "output_switching_condition"
        );
        while !self.consume_symbol(b')')? {
            args.push(self.read_value()?);
            if self.consume_symbol(b')')? {
                break;
            }
            if !allow_spaces {
                self.expect_symbol(b',')?;
            } else {
                self.consume_symbol(b',')?;
            }
        }
        Ok(args)
    }

    pub fn read_value(&mut self) -> Result<String, ParseError> {
        let base = self.take_word()?;
        let index = if self.consume_symbol(b'[')? {
            let sel = self.take_word()?;
            self.expect_symbol(b']')?;
            Some(sel)
        } else {
            None
        };
        Ok(indexed_value(&base, index.as_deref()))
    }

    pub fn read_simple_attribute_value(&mut self) -> Result<String, ParseError> {
        let mut parts = Vec::new();
        while !self.consume_symbol(b';')? {
            if self.current.is_none() {
                return Err(self.error_here("unexpected end of file in attribute value"));
            }
            parts.push(self.token_to_string()?);
            self.advance()?;
        }
        Ok(parts.join(" "))
    }

    pub fn take_word(&mut self) -> Result<String, ParseError> {
        match self.current.clone() {
            Some(Token {
                kind: TokenKind::Word(value),
                ..
            }) => {
                self.advance()?;
                Ok(value)
            }
            Some(token) => Err(ParseError::new(
                token.line,
                token.column,
                "expected identifier or value",
            )),
            None => Err(self.error_here("unexpected end of file")),
        }
    }

    pub fn token_to_string(&self) -> Result<String, ParseError> {
        match &self.current {
            Some(Token {
                kind: TokenKind::Word(value),
                ..
            }) => Ok(value.clone()),
            Some(Token {
                kind: TokenKind::Symbol(byte),
                ..
            }) => Ok((*byte as char).to_string()),
            None => Err(self.error_here("unexpected end of file")),
        }
    }

    pub fn consume_symbol(&mut self, symbol: u8) -> Result<bool, ParseError> {
        if matches!(
            self.current,
            Some(Token {
                kind: TokenKind::Symbol(current),
                ..
            }) if current == symbol
        ) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn expect_symbol(&mut self, symbol: u8) -> Result<(), ParseError> {
        if self.consume_symbol(symbol)? {
            Ok(())
        } else {
            Err(self.error_here(&format!("expected '{}'", symbol as char)))
        }
    }

    pub fn advance(&mut self) -> Result<(), ParseError> {
        if let Some(token) = &self.current {
            self.previous_end = token.end;
        }
        self.current = self.lexer.next_token()?;
        Ok(())
    }

    /// Byte offset of the first byte of the current token.
    pub fn current_offset(&self) -> usize {
        match &self.current {
            Some(token) => token.start,
            None => self.lexer.offset,
        }
    }

    pub fn current_position(&self) -> (usize, usize) {
        match &self.current {
            Some(token) => (token.line, token.column),
            None => (self.lexer.line, self.lexer.column),
        }
    }

    pub fn error_here(&self, message: &str) -> ParseError {
        let (line, column) = self.current_position();
        ParseError::new(line, column, message)
    }
}

pub fn is_symbol(byte: u8) -> bool {
    matches!(
        byte,
        b',' | b'{' | b'}' | b'(' | b')' | b'[' | b']' | b';' | b':'
    )
}

/// Reassemble a value that carries an index suffix. The lexer splits
/// `A[0]` into the words `A` and `0` and the symbols `[` and `]`; the value
/// is put back together the same way everywhere.
pub fn indexed_value(base: &str, index: Option<&str>) -> String {
    match index {
        Some(index) => format!("{base}[{index}]"),
        None => base.to_string(),
    }
}

pub fn is_timing_table_group(name: &str) -> bool {
    matches!(
        name,
        "cell_rise"
            | "cell_fall"
            | "rise_transition"
            | "fall_transition"
            | "rise_constraint"
            | "fall_constraint"
            | "cell_degradation"
            | "retaining_rise"
            | "retaining_fall"
            | "retain_rise_slew"
            | "retain_fall_slew"
            | "output_current_rise"
            | "output_current_fall"
            | "ccst_rise"
            | "ccst_fall"
            | "ccsp_rise"
            | "ccsp_fall"
            | "receiver_capacitance1_rise"
            | "receiver_capacitance1_fall"
            | "receiver_capacitance2_rise"
            | "receiver_capacitance2_fall"
    )
}

pub fn is_ccsn_stage_group(name: &str) -> bool {
    matches!(
        name,
        "ccsn_first_stage"
            | "ccsn_last_stage"
            | "ccsn_rise_first_stage"
            | "ccsn_rise_last_stage"
            | "ccsn_fall_first_stage"
            | "ccsn_fall_last_stage"
    )
}

pub fn is_power_table_group(name: &str) -> bool {
    matches!(name, "rise_power" | "fall_power" | "power")
}

/// Parse a Liberty unit string (e.g. "1mA", "1ps", "1V") into its SI scale.
/// Returns magnitude * SI-prefix factor; "1mA" -> 1e-3, "1ps" -> 1e-12.
pub fn unit_scale(value: &str) -> Option<f64> {
    let value = value.trim();
    let split = value.find(|ch: char| ch.is_ascii_alphabetic())?;
    let (number, unit) = value.split_at(split);
    let magnitude: f64 = number.trim().parse().unwrap_or(1.0);
    // A multi-char unit (e.g. "mA", "ps") carries an SI prefix; "V"/"A"/"s" do not.
    let prefix_factor = if unit.len() >= 2 {
        match unit.chars().next()? {
            'f' => 1e-15,
            'p' => 1e-12,
            'n' => 1e-9,
            'u' => 1e-6,
            'm' => 1e-3,
            'k' => 1e3,
            'M' => 1e6,
            'G' => 1e9,
            _ => 1.0,
        }
    } else {
        1.0
    };
    Some(magnitude * prefix_factor)
}

pub fn parse_number(value: &str) -> Option<f64> {
    value.parse::<f64>().ok()
}

/// Position of a coordinate on an axis, allowing for floating point noise.
/// Returns None when the coordinate is not on the axis.
pub fn find_axis_position(values: &[f64], target: f64) -> Option<usize> {
    values.iter().position(|value| {
        let tolerance = 1e-12 + 1e-9 * value.abs().max(target.abs());
        (value - target).abs() <= tolerance
    })
}

/// Flat position of a grid point in the flat `values` list: `index_3` varies
/// fastest, then `index_2`, then `index_1`.
pub fn flat_index(
    index_1: usize,
    index_2: usize,
    index_3: usize,
    len_2: usize,
    len_3: usize,
) -> usize {
    let len_2 = len_2.max(1);
    let len_3 = len_3.max(1);
    index_1 * len_2 * len_3 + index_2 * len_3 + index_3
}

/// The axis-variable names a keyword belongs to. Two names in the same group
/// mean the same physical quantity, so a load axis named
/// `output_capacitance` matches one named `total_output_net_capacitance`.
pub fn role_synonyms(name: &str) -> &'static [&'static str] {
    match name {
        "load"
        | "cap"
        | "capacitance"
        | "output_cap"
        | "output_capacitance"
        | "total_output_net_capacitance"
        | "output_net_capacitance" => &[
            "total_output_net_capacitance",
            "output_net_capacitance",
            "output_capacitance",
            "capacitance",
        ],
        "transition" | "slew" | "input_slew" | "input_transition" | "input_net_transition" => {
            &["input_net_transition", "input_transition"]
        }
        "time" => &["time"],
        "voltage" | "vin" | "input_voltage" => &["input_voltage", "voltage"],
        "current" | "iout" | "output_current" => &["output_current", "current"],
        _ => &[],
    }
}

/// Does an axis role answer to the given keyword? Accepts the exact template
/// variable name, its synonyms and the usual shorthands (`load`, `transition`,
/// `slew`, …).
pub fn role_matches_role(role: &str, keyword: &str) -> bool {
    if role == keyword {
        return true;
    }
    let group = role_synonyms(keyword);
    !group.is_empty() && group.contains(&role)
}

/// Value of an attribute list entry, or None if the key is absent.
pub fn attribute_value(attributes: &[(String, String)], key: &str) -> Option<String> {
    attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

/// Parse the first number of an attribute value. Complex attributes are stored
/// as `"1.0, 2.0"`; the leading value is the one that carries the meaning for
/// the scalar accessors.
pub fn parse_first_number(value: &str) -> Option<f64> {
    value
        .split(',')
        .next()
        .and_then(|item| item.trim().parse::<f64>().ok())
}

/// FNV-1a hash of a byte slice (must match the lexer's running hash).
pub fn fnv1a(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ---- writer ---------------------------------------------------------------
//
// The writer is *surgical*: it keeps the original source text and rewrites only
// the attributes that were changed through `Cell::set_attribute` /
// `Pin::set_attribute`. Everything else — comments, spacing, unknown groups —
// survives byte for byte.

impl Document {
    /// The value of a library-group attribute, if present.
    pub fn get(&self, key: &str) -> Option<String> {
        attribute_value(&self.attributes, key)
    }

    /// SI scale (joules) for `internal_power` energy values: voltage * current *
    /// time unit. ASAP7 (1V, 1mA, 1ps) -> 1e-15 = femtojoules.
    pub fn energy_unit_joules(&self) -> Option<f64> {
        Some(
            unit_scale(self.voltage_unit.as_deref()?)?
                * unit_scale(self.current_unit.as_deref()?)?
                * unit_scale(self.time_unit.as_deref()?)?,
        )
    }

    /// Ordered library-group simple/complex attributes (technology, delay_model,
    /// nom_*, default_*, thresholds, …).
    pub fn attributes(&self) -> Vec<(String, String)> {
        self.attributes.clone()
    }

    /// `normalized_driver_waveform` tables.
    pub fn driver_waveforms(&self) -> Vec<TimingTableData> {
        self.driver_waveforms.clone()
    }

    /// The operating condition used for the `nom_*` accessors: the group named
    /// by the library attribute `default_operating_conditions`, else the group
    /// named "default", else the first group.
    pub fn default_operating_condition(&self) -> Option<&Vec<(String, String)>> {
        let named = self.get("default_operating_conditions");
        self.operating_conditions
            .iter()
            .find(|(name, _)| Some(name.as_str()) == named.as_deref())
            .or_else(|| {
                self.operating_conditions
                    .iter()
                    .find(|(name, _)| name == "default")
            })
            .or_else(|| self.operating_conditions.first())
            .map(|(_, attributes)| attributes)
    }

    /// The source text with the pending edits applied.
    ///
    /// Pure Rust — no Python token is involved — so the caller can run it
    /// with the GIL released.
    pub fn render_source_text(&self) -> Result<String, WriteError> {
        let source = read_source_text(
            self.source_path.as_deref(),
            self.source_len,
            self.source_hash,
        )?;
        let edits = self.edits.lock().map_err(|_| WriteError::Poisoned)?;
        apply_edits_to_source(source, &self.cell_spans, &edits)
    }

    /// Write the rendered document to `path` (gzip-compressed when the path
    /// ends in `.gz`).
    ///
    /// Pure Rust — no Python token is involved — so the caller can run it
    /// with the GIL released.
    pub fn write_to_path(&self, path: &str) -> Result<(), WriteError> {
        write_source_text(path, &self.render_source_text()?)
    }
}

/// Write `text` to `path` (gzip-compressed when the path ends in `.gz`).
///
/// Pure Rust — no Python token is involved — so the caller can run it with
/// the GIL released.
pub fn write_source_text(path: &str, text: &str) -> Result<(), WriteError> {
    let bytes = text.as_bytes();
    if path.ends_with(".gz") {
        let file = File::create(path).map_err(|err| WriteError::Io(err.to_string()))?;
        let mut encoder = GzEncoder::new(file, flate2::Compression::default());
        encoder
            .write_all(bytes)
            .map_err(|err| WriteError::Io(err.to_string()))?;
        encoder
            .finish()
            .map_err(|err| WriteError::Io(err.to_string()))?;
    } else {
        std::fs::write(path, bytes).map_err(|err| WriteError::Io(err.to_string()))?;
    }
    Ok(())
}

/// Read the original source text of a document, verifying that the file has
/// not changed since it was parsed.
///
/// Pure Rust: it takes no Python token, so the caller can run it with the GIL
/// released.
pub fn read_source_text(
    source_path: Option<&str>,
    source_len: usize,
    source_hash: u64,
) -> Result<String, WriteError> {
    let path = source_path.ok_or(WriteError::NoSource)?;
    let data = read_to_memory(path).map_err(|err| WriteError::Io(err.to_string()))?;
    if data.len() != source_len || fnv1a(&data) != source_hash {
        return Err(WriteError::Changed(format!(
            "source file {path:?} changed since it was parsed; re-parse it before writing"
        )));
    }
    String::from_utf8(data).map_err(|err| WriteError::Io(err.to_string()))
}

/// Apply the pending attribute edits to the source text of a document.
///
/// Only the cells that were edited are re-written; every other byte of the
/// source is kept as it is.
///
/// Pure Rust: it takes no Python token, so the caller can run it with the GIL
/// released.
pub fn apply_edits_to_source(
    source: String,
    cell_spans: &[(String, usize, usize)],
    edits: &[AttrEdit],
) -> Result<String, WriteError> {
    if edits.is_empty() {
        return Ok(source);
    }
    // Only the last edit of an attribute counts: applying two edits to the
    // same span would make the second one land in the middle of the
    // first one's text. The surviving edits keep the order they were made
    // in, so the written file is deterministic.
    let mut chosen: Vec<&AttrEdit> = Vec::new();
    {
        // Walk the log backwards and keep the first occurrence of every
        // key; that is the last edit of the attribute. Reversing afterwards
        // puts the survivors back in the order the edits were made in.
        let mut seen: HashSet<EditKey> = HashSet::new();
        for edit in edits.iter().rev() {
            let key = (
                edit.cell.clone(),
                edit.pin.clone(),
                edit.container.clone(),
                edit.key.clone(),
            );
            if seen.insert(key) {
                chosen.push(edit);
            }
        }
        chosen.reverse();
    }
    let mut by_cell: HashMap<&str, Vec<&AttrEdit>> = HashMap::new();
    for edit in chosen {
        by_cell.entry(edit.cell.as_str()).or_default().push(edit);
    }
    // Patch only the cell that `Document::cell` resolves to — the first
    // cell of that name in the source. A second cell with the same name
    // is a different cell and must not be rewritten.
    let mut spans: Vec<&(String, usize, usize)> = Vec::new();
    for cell_name in by_cell.keys() {
        let span = cell_spans
            .iter()
            .find(|(name, _, _)| name.as_str() == *cell_name)
            .ok_or_else(|| {
                WriteError::Invalid(format!(
                    "cell {cell_name:?} is not in the source text; its edits cannot be \
                     written back"
                ))
            })?;
        spans.push(span);
    }
    // Patch from the last cell backwards so earlier offsets stay valid.
    spans.sort_by_key(|(_, start, _)| std::cmp::Reverse(*start));
    let mut text = source;
    for (name, start, end) in spans {
        let cell_text = text.get(*start..*end).ok_or_else(|| {
            WriteError::Invalid(format!("cell {name:?} is outside the source text"))
        })?;
        let patched = patch_cell_text(cell_text, name, &by_cell[name.as_str()])?;
        text.replace_range(*start..*end, &patched);
    }
    Ok(text)
}

/// Replace an attribute's value in an ordered attribute list, or append it
/// when the key is not present yet.
pub fn set_attribute_in(attributes: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(entry) = attributes.iter_mut().find(|(name, _)| name == key) {
        entry.1 = value.to_string();
    } else {
        attributes.push((key.to_string(), value.to_string()));
    }
}

/// A group header found in a cell's source text.
pub struct SourceGroup {
    keyword: String,
    arg: Option<String>,
    /// Byte offset of the group's header (its keyword).
    start: usize,
    /// Byte offset of the group's closing brace.
    close: usize,
}

/// The first argument of a group header, reassembled the way the parser does
/// it: `A[0]` is lexed as the words `A` and `0` and the symbols `[` and `]`.
pub fn group_argument(tokens: &[Token], first: usize) -> Option<String> {
    let base = match tokens.get(first).map(|token| &token.kind) {
        Some(TokenKind::Word(value)) => value.clone(),
        _ => return None,
    };
    let index = match tokens.get(first + 1).map(|token| &token.kind) {
        Some(TokenKind::Symbol(b'[')) => match tokens.get(first + 2).map(|token| &token.kind) {
            Some(TokenKind::Word(index)) => Some(index.clone()),
            _ => None,
        },
        _ => None,
    };
    Some(indexed_value(&base, index.as_deref()))
}

/// A simple or complex attribute found in a cell's source text.
pub struct SourceAttribute {
    /// Index into the group stack.
    group: usize,
    key: String,
    /// Byte range of the whole attribute (name through value).
    start: usize,
    end: usize,
    complex: bool,
}

/// Scan one cell group's source text and return its groups and attributes.
pub fn scan_source_groups(
    text: &str,
) -> Result<(Vec<SourceGroup>, Vec<SourceAttribute>), ParseError> {
    let mut lexer = Lexer::new(Box::new(io::Cursor::new(text.as_bytes().to_vec())))?;
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token()? {
        tokens.push(token);
    }
    if tokens.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut groups: Vec<SourceGroup> = Vec::new();
    let mut attributes: Vec<SourceAttribute> = Vec::new();
    // Indices of the currently open groups, outermost first.
    let mut open: Vec<usize> = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        match &tokens[index].kind {
            TokenKind::Symbol(b'}') => {
                if let Some(group) = open.pop() {
                    groups[group].close = tokens[index].start;
                }
                index += 1;
            }
            TokenKind::Word(word) => {
                let is_paren = matches!(
                    tokens.get(index + 1).map(|token| &token.kind),
                    Some(TokenKind::Symbol(b'('))
                );
                if is_paren {
                    // Find the matching ')'.
                    let mut depth = 0i32;
                    let mut cursor = index + 1;
                    let mut close = tokens.len().saturating_sub(1);
                    while cursor < tokens.len() {
                        match tokens[cursor].kind {
                            TokenKind::Symbol(b'(') => depth += 1,
                            TokenKind::Symbol(b')') => {
                                depth -= 1;
                                if depth == 0 {
                                    close = cursor;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        cursor += 1;
                    }
                    let opens_group = matches!(
                        tokens.get(close + 1).map(|token| &token.kind),
                        Some(TokenKind::Symbol(b'{'))
                    );
                    if opens_group {
                        let arg = group_argument(&tokens, index + 2);
                        groups.push(SourceGroup {
                            keyword: word.clone(),
                            arg,
                            start: tokens[index].start,
                            close: 0,
                        });
                        open.push(groups.len() - 1);
                        index = close + 2;
                        continue;
                    }
                    attributes.push(SourceAttribute {
                        group: open.last().copied().unwrap_or(0),
                        key: word.clone(),
                        start: tokens[index].start,
                        end: tokens[close].end,
                        complex: true,
                    });
                    index = close + 1;
                    continue;
                }
                let is_colon = matches!(
                    tokens.get(index + 1).map(|token| &token.kind),
                    Some(TokenKind::Symbol(b':'))
                );
                if is_colon {
                    // Simple attribute: the value runs to the ';'.
                    let mut cursor = index + 2;
                    let mut end = tokens[index].end;
                    while cursor < tokens.len() {
                        if matches!(tokens[cursor].kind, TokenKind::Symbol(b';')) {
                            break;
                        }
                        end = tokens[cursor].end;
                        cursor += 1;
                    }
                    attributes.push(SourceAttribute {
                        group: open.last().copied().unwrap_or(0),
                        key: word.clone(),
                        start: tokens[index].start,
                        end,
                        complex: false,
                    });
                    index = cursor;
                    continue;
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    Ok((groups, attributes))
}

/// Render an attribute value so that reading the file back yields exactly
/// this value.
///
/// A value is written unquoted only when it cannot confuse the lexer: it must
/// be non-empty and consist of letters, digits and `_`, `+`, `.`, `-` only.
/// Every other value is written as a quoted string, where `\` becomes `\\` and
/// `"` becomes `\"` — the inverse of what the lexer's `read_quoted` does.
/// Quoting by a safe pattern rather than by a blacklist of dangerous
/// characters keeps the rule closed: a value with a `:`, a bracket or any
/// other punctuation in it still parses back to itself.
pub fn render_attribute_value(value: &str) -> String {
    let is_plain = !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '+' | '.' | '-'));
    if is_plain {
        return value.to_string();
    }
    let mut rendered = String::with_capacity(value.len() + 2);
    rendered.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => rendered.push_str("\\\\"),
            '"' => rendered.push_str("\\\""),
            _ => rendered.push(ch),
        }
    }
    rendered.push('"');
    rendered
}

/// Render the arguments of a complex attribute, keeping it complex: a value of
/// "0.1, 0.2" becomes `(0.1, 0.2)`, not a single quoted string. Each part is
/// quoted with the same rule as a simple value, so that the arguments parse
/// back to the same parts.
pub fn render_complex_attribute_value(value: &str) -> String {
    let parts: Vec<String> = value
        .split(',')
        .map(|part| render_attribute_value(part.trim()))
        .collect();
    parts.join(", ")
}

/// The group an edit's pin refers to.
///
/// A pin edit that names its container (the bus or bundle the pin was taken
/// from) is patched in the pin group inside that container. A pin edit
/// without a container is patched in the only pin group of that name in the
/// cell: when the name matches more than one group the write fails instead
/// of patching an arbitrary one.
pub fn resolve_pin_group(
    groups: &[SourceGroup],
    cell_name: &str,
    pin: &str,
    container: Option<&(ContainerKind, String)>,
) -> Result<usize, String> {
    let candidates: Vec<usize> = groups
        .iter()
        .enumerate()
        .filter(|(_, group)| group.keyword == "pin" && group.arg.as_deref() == Some(pin))
        .map(|(index, _)| index)
        .collect();
    let Some((kind, container_name)) = container else {
        return match candidates.len() {
            1 => Ok(candidates[0]),
            0 => Err(format!("cell {cell_name:?} has no pin {pin:?} to edit")),
            _ => Err(format!(
                "cell {cell_name:?} has {} pins named {pin:?} ({}); the edit does not say \
                 which one to patch",
                candidates.len(),
                describe_pin_locations(groups, &candidates)
            )),
        };
    };
    let container_group = groups
        .iter()
        .position(|group| {
            group.keyword == kind.keyword() && group.arg.as_deref() == Some(container_name.as_str())
        })
        .ok_or_else(|| {
            format!(
                "cell {cell_name:?} has no {} {container_name:?} to edit",
                kind.keyword()
            )
        })?;
    let inside: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&index| {
            groups[index].start > groups[container_group].start
                && groups[index].close <= groups[container_group].close
        })
        .collect();
    match inside.len() {
        1 => Ok(inside[0]),
        0 => Err(format!(
            "cell {cell_name:?} has no pin {pin:?} in {} {container_name:?} to edit",
            kind.keyword()
        )),
        _ => Err(format!(
            "cell {cell_name:?} has {} pins named {pin:?} in {} {container_name:?}; the edit \
             does not say which one to patch",
            inside.len(),
            kind.keyword()
        )),
    }
}

/// Where the candidate pin groups live, for the ambiguity message: the
/// innermost group that contains them (`bus (A)`, `bundle (B)`, …).
pub fn describe_pin_locations(groups: &[SourceGroup], candidates: &[usize]) -> String {
    candidates
        .iter()
        .map(|&index| {
            groups
                .iter()
                .filter(|group| {
                    group.start < groups[index].start && group.close >= groups[index].close
                })
                .max_by_key(|group| group.start)
                .and_then(|group| {
                    group
                        .arg
                        .as_deref()
                        .map(|arg| format!("{} ({arg})", group.keyword))
                })
                .unwrap_or_else(|| "the cell".to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Apply attribute edits to one cell group's source text.
///
/// Every edit is turned into a `(start, end, text)` range against the
/// *original* text — an insertion is an empty range — and all of them are
/// applied in a single back-to-front pass, so no edit can shift another.
pub fn patch_cell_text(
    cell_text: &str,
    cell_name: &str,
    edits: &[&AttrEdit],
) -> Result<String, WriteError> {
    let (groups, attributes) = scan_source_groups(cell_text).map_err(WriteError::Parse)?;

    let cell_group = groups
        .iter()
        .position(|group| group.keyword == "cell")
        .ok_or_else(|| {
            WriteError::Invalid(format!(
                "cell {cell_name:?} has no cell group in the source"
            ))
        })?;

    // (start, end, replacement, order) against the original text.
    let mut patches: Vec<(usize, usize, String, usize)> = Vec::new();

    for (order, edit) in edits.iter().enumerate() {
        let target_group = match &edit.pin {
            None => cell_group,
            Some(pin) => resolve_pin_group(&groups, cell_name, pin, edit.container.as_ref())
                .map_err(WriteError::Invalid)?,
        };
        let existing = attributes
            .iter()
            .find(|attribute| attribute.group == target_group && attribute.key == edit.key);
        match existing {
            Some(attribute) => {
                let rendered = if attribute.complex {
                    format!(
                        "{} ({})",
                        edit.key,
                        render_complex_attribute_value(&edit.value)
                    )
                } else {
                    format!("{} : {}", edit.key, render_attribute_value(&edit.value))
                };
                patches.push((attribute.start, attribute.end, rendered, order));
            }
            None => {
                // Add a new attribute to the group.
                let group = &groups[target_group];
                let line_start = cell_text[..group.close]
                    .rfind('\n')
                    .map(|position| position + 1)
                    .unwrap_or(0);
                let before_brace = &cell_text[line_start..group.close];
                let brace_alone_on_line = before_brace.trim().is_empty();
                let closing_indent: String = before_brace
                    .chars()
                    .take_while(|ch| ch.is_whitespace())
                    .collect();
                let rendered = format!("{} : {};", edit.key, render_attribute_value(&edit.value));
                let newline = if cell_text.contains("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                };
                if brace_alone_on_line {
                    // The group ends on a line of its own: add the attribute
                    // as a new line above it, one level deeper.
                    let indent = attributes
                        .iter()
                        .find(|attribute| attribute.group == target_group)
                        .and_then(|attribute| {
                            let start = cell_text[..attribute.start]
                                .rfind('\n')
                                .map(|position| position + 1)
                                .unwrap_or(0);
                            let line = &cell_text[start..attribute.start];
                            let whitespace: String =
                                line.chars().take_while(|ch| ch.is_whitespace()).collect();
                            if whitespace.len() == line.len() && !whitespace.is_empty() {
                                Some(whitespace)
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| format!("{closing_indent}  "));
                    patches.push((
                        line_start,
                        line_start,
                        format!("{indent}{rendered}{newline}"),
                        order,
                    ));
                } else {
                    // The group is written on one line: keep it that way and
                    // add the attribute in front of the closing brace.
                    let separator = if cell_text[..group.close]
                        .chars()
                        .next_back()
                        .map(|ch| ch.is_whitespace())
                        .unwrap_or(false)
                    {
                        ""
                    } else {
                        " "
                    };
                    patches.push((
                        group.close,
                        group.close,
                        format!("{separator}{rendered} "),
                        order,
                    ));
                }
            }
        }
    }

    // Apply the edits back to front so the offsets stay valid. Edits at the
    // same offset are applied in reverse order, so that they end up in the
    // order they were made.
    patches
        .sort_by_key(|(start, _, _, order)| (std::cmp::Reverse(*start), std::cmp::Reverse(*order)));
    let mut text = cell_text.to_string();
    for (start, end, rendered, _) in patches {
        text.replace_range(start..end, &rendered);
    }
    Ok(text)
}

/// Attach the library's unit scales to every cell and pin, so that scalar
/// accessors can report values with their scale attached.
pub fn apply_units(cells: &mut [CellData], ctx: UnitContext) {
    for cell in cells {
        cell.units = ctx;
        for pin in &mut cell.pins {
            pin.units = ctx;
        }
        for bus in &mut cell.buses {
            for pin in &mut bus.pins {
                pin.units = ctx;
            }
        }
        for bundle in &mut cell.bundles {
            for pin in &mut bundle.pins {
                pin.units = ctx;
            }
        }
    }
}

/// Look up a table template by name and return its axis variable names.
pub fn template_vars(
    templates: &[(String, [Option<String>; 3])],
    name: &Option<String>,
) -> Option<[Option<String>; 3]> {
    let name = name.as_ref()?;
    templates
        .iter()
        .find(|(template, _)| template == name)
        .map(|(_, vars)| vars.clone())
}

/// Resolve the axis variable names of every table in a list (recursively, so
/// CCS `vector` sub-tables are covered too).
pub fn resolve_axis_vars_in(
    tables: &mut [TimingTableData],
    templates: &[(String, [Option<String>; 3])],
) {
    for table in tables.iter_mut() {
        table.axis_vars = template_vars(templates, &table.template).unwrap_or([None, None, None]);
        resolve_axis_vars_in(&mut table.vectors, templates);
    }
}

/// Resolve the axis variable names of every table reachable from the cells.
pub fn resolve_axis_vars(cells: &mut [CellData], templates: &[(String, [Option<String>; 3])]) {
    pub fn resolve_arcs(arcs: &mut [TimingArcData], templates: &[(String, [Option<String>; 3])]) {
        for arc in arcs.iter_mut() {
            resolve_axis_vars_in(&mut arc.tables, templates);
            for stage in arc.ccsn_stages.iter_mut() {
                if let Some(table) = stage.dc_current.as_mut() {
                    table.axis_vars =
                        template_vars(templates, &table.template).unwrap_or([None, None, None]);
                    resolve_axis_vars_in(&mut table.vectors, templates);
                }
                resolve_axis_vars_in(&mut stage.output_voltage_rise, templates);
                resolve_axis_vars_in(&mut stage.output_voltage_fall, templates);
            }
        }
    }
    pub fn resolve_pins(pins: &mut [PinData], templates: &[(String, [Option<String>; 3])]) {
        for pin in pins.iter_mut() {
            resolve_arcs(&mut pin.timing_arcs, templates);
            for group in pin.internal_powers.iter_mut() {
                resolve_axis_vars_in(&mut group.tables, templates);
            }
        }
    }
    for cell in cells.iter_mut() {
        resolve_pins(&mut cell.pins, templates);
        for bus in cell.buses.iter_mut() {
            resolve_arcs(&mut bus.timing_arcs, templates);
            resolve_pins(&mut bus.pins, templates);
        }
        for bundle in cell.bundles.iter_mut() {
            resolve_arcs(&mut bundle.timing_arcs, templates);
            resolve_pins(&mut bundle.pins, templates);
        }
        for dynamic in cell.dynamic_currents.iter_mut() {
            for group in dynamic.switching_groups.iter_mut() {
                for current in group.pg_currents.iter_mut() {
                    resolve_axis_vars_in(&mut current.vectors, templates);
                }
            }
        }
    }
}

pub fn parse_float_list(value: Option<&String>) -> Vec<f64> {
    value
        .map(|items| {
            items
                .split(',')
                .filter_map(|item| item.trim().parse::<f64>().ok())
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_float_args(values: &[String]) -> Vec<f64> {
    values
        .iter()
        .flat_map(|items| items.split(','))
        .filter_map(|item| item.trim().parse::<f64>().ok())
        .collect()
}

pub fn members_from_args(args: &[String]) -> Vec<String> {
    args.iter()
        .flat_map(|arg| arg.split_whitespace())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl ParseError {
    pub fn new(line: usize, column: usize, message: &str) -> Self {
        Self {
            line,
            column,
            message: message.to_string(),
        }
    }
}

#[derive(Debug)]
pub enum ParseLoadError {
    Io(io::Error),
    Parse(ParseError),
}

impl ParseLoadError {}

#[derive(Debug)]
pub enum CellLookupError {
    Unknown(String),
    Parse(ParseError),
    Empty(String),
}

impl CellLookupError {}

#[derive(Debug)]
pub enum CellSourceError {
    Unknown(String),
}

impl CellSourceError {}

/// Failure of the writer path.
///
/// The writer runs with the GIL released, so it cannot build a `PyErr`
/// itself: it reports the failure with this plain enum and the caller
/// converts it to a `PyErr` outside the detached closure.
pub enum WriteError {
    /// The document was not parsed from a file.
    NoSource,
    /// The source file could not be read or written.
    Io(String),
    /// The source file changed since the document was parsed.
    Changed(String),
    /// The document's edit log is poisoned.
    Poisoned,
    /// An edit does not apply to the source text.
    Invalid(String),
    /// The source text could not be lexed.
    Parse(ParseError),
}

impl WriteError {}

impl From<io::Error> for ParseError {
    fn from(value: io::Error) -> Self {
        Self::new(0, 0, &value.to_string())
    }
}

/// Public aliases matching the issue #12 API sketch; the canonical data names
/// keep their `Data` suffix internally.
pub type Cell = CellData;
pub type Pin = PinData;
pub type InternalPower = InternalPowerData;
pub type Bus = BusData;
pub type Bundle = BundleData;
pub type BusType = BusTypeData;
pub type TimingArc = TimingArcData;
pub type CcsnStage = CcsnStageData;
pub type TimingTable = TimingTableData;
pub type PgPin = PgPinData;
pub type LeakagePower = LeakagePowerData;
pub type Ff = FfData;
pub type Latch = LatchData;
pub type DynamicCurrent = DynamicCurrentData;
pub type SwitchingGroup = SwitchingGroupData;
pub type PgCurrent = PgCurrentData;

// ---------------------------------------------------------------------------
// Public Rust API (issue #12): attribute-only Liberty + indexed cell access.
// ---------------------------------------------------------------------------

/// Cell-level attributes for `is_seq` classification: the cell's simple
/// attributes plus whether it has an `ff` or `latch` group.
#[derive(Clone, Debug, PartialEq)]
pub struct CellAttributes {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub is_sequential: bool,
}

/// Owning, lazy attribute scan: the index (file bytes) is held inside the
/// iterator and cells parse one at a time with `skip_tables=true`, so a
/// multi-GB library never holds every cell's attributes at once.
pub struct CellAttributesIter {
    index: LibraryIndex,
    position: usize,
}

impl CellAttributesIter {
    pub fn open(path: &Path) -> Result<Self, ParseLoadError> {
        Ok(Self {
            index: LibraryIndex::open(path)?,
            position: 0,
        })
    }
}

impl Iterator for CellAttributesIter {
    type Item = Result<CellAttributes, CellLookupError>;

    fn next(&mut self) -> Option<Self::Item> {
        let name = self.index.order.get(self.position)?.clone();
        self.position += 1;
        Some(self.index.cell_attributes(&name))
    }
}

/// Iterate every cell's attributes with table bodies skipped.
pub fn cell_attributes(path: &Path) -> Result<CellAttributesIter, ParseLoadError> {
    CellAttributesIter::open(path)
}

impl LibraryIndex {
    /// Index an in-memory Liberty file: `path` may be plain or `.gz`. A
    /// non-UTF-8 path is an error — never silently a different file.
    pub fn open(path: &Path) -> Result<Self, ParseLoadError> {
        let path = path.to_str().ok_or_else(|| {
            ParseLoadError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "library path is not valid UTF-8",
            ))
        })?;
        open_library_index(path)
    }

    pub fn library_name(&self) -> &str {
        &self.library_name
    }

    pub fn cell_names(&self) -> Vec<String> {
        self.order.clone()
    }

    pub fn num_cells(&self) -> usize {
        self.order.len()
    }

    /// Parse one cell fully (pins, arcs, tables), resolving the library
    /// header's units and templates.
    pub fn cell(&self, name: &str) -> Result<Cell, CellLookupError> {
        self.parse_cell_by_name(name)
    }

    /// Parse one cell structurally only (`skip_tables`): simple attributes and
    /// `ff`/`latch` presence, no table bodies materialized.
    pub fn cell_attributes(&self, name: &str) -> Result<CellAttributes, CellLookupError> {
        let &(start, end) = self
            .ranges
            .get(name)
            .ok_or_else(|| CellLookupError::Unknown(name.to_string()))?;
        let doc = parse_slice_as_library(&self.library_name, &self.data[start..end], true)
            .map_err(CellLookupError::Parse)?;
        let cell = doc
            .cells
            .into_iter()
            .next()
            .ok_or_else(|| CellLookupError::Empty(name.to_string()))?;
        Ok(CellAttributes {
            name: cell.name.clone(),
            is_sequential: cell.ff.is_some() || cell.latch.is_some(),
            attributes: cell.attributes,
        })
    }

    /// Raw Liberty source text of one cell, sliced from the in-memory buffer.
    /// O(cell size) — independent of the total file size.
    pub fn cell_source(&self, name: &str) -> Result<String, CellSourceError> {
        self.cell_source_by_name(name)
    }

    // -- library header pass-throughs (parsed once at open) --
    pub fn voltage_unit(&self) -> Option<String> {
        self.header.voltage_unit.clone()
    }

    pub fn current_unit(&self) -> Option<String> {
        self.header.current_unit.clone()
    }

    pub fn time_unit(&self) -> Option<String> {
        self.header.time_unit.clone()
    }

    pub fn capacitive_load_unit(&self) -> Option<String> {
        self.header.capacitive_load_unit.clone()
    }

    pub fn energy_unit_joules(&self) -> Option<f64> {
        self.header.energy_unit_joules()
    }

    /// Lookup-table templates: name -> per-axis variables, `None` slots kept.
    pub fn templates(&self) -> HashMap<String, Vec<Option<String>>> {
        self.header.templates()
    }

    pub fn attributes(&self) -> Vec<(String, String)> {
        self.header.attributes()
    }
}

impl Document {
    /// Lookup-table templates: name -> [variable_1, variable_2, variable_3],
    /// with `None` slots preserved so 2-axis templates stay 3 wide.
    pub fn templates(&self) -> HashMap<String, Vec<Option<String>>> {
        self.templates
            .iter()
            .map(|(name, vars)| (name.clone(), vars.to_vec()))
            .collect()
    }
}
