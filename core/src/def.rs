use rayon::prelude::*;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as FmtWrite;
use std::fs::{File, OpenOptions, create_dir_all};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::connectivity::{Adjacency, Conn, Connectivity, ConnectivityBuilder, NetConns, PORT};
use crate::lef::{Lexer, open_input};

mod targeted;
pub use targeted::{ClockNetRecord, DefConnection, TargetedDefScan, scan_clock_nets_and_instances};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[doc(hidden)]
pub enum ColumnKind {
    Instance,
    BaseCell,
    Status,
    Orient,
    IsSeq,
    IsClock,
    Llx,
    Lly,
    Urx,
    Ury,
}

#[doc(hidden)]
pub struct ColumnSpec {
    pub kind: ColumnKind,
    pub name: String,
}

pub(crate) fn default_columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec {
            kind: ColumnKind::Instance,
            name: "instance".to_string(),
        },
        ColumnSpec {
            kind: ColumnKind::BaseCell,
            name: "base_cell".to_string(),
        },
        ColumnSpec {
            kind: ColumnKind::Llx,
            name: "llx".to_string(),
        },
        ColumnSpec {
            kind: ColumnKind::Lly,
            name: "lly".to_string(),
        },
        ColumnSpec {
            kind: ColumnKind::Urx,
            name: "urx".to_string(),
        },
        ColumnSpec {
            kind: ColumnKind::Ury,
            name: "ury".to_string(),
        },
    ]
}

/// Turn ``(source, output_name)`` pairs from Python into columns.
///
/// ``None`` selects the canonical six columns in their canonical order. The
/// source names are the project's column names; the output names are what the
/// file gets, so a rename is just a different second half of the pair.
///
/// ``is_seq`` and ``is_clock`` are conditional: they exist only when the data
/// behind them was supplied (a Liberty library and clock-cell set,
/// respectively).
#[doc(hidden)]
pub fn parse_columns(
    columns: Option<Vec<(String, String)>>,
    has_sequential: bool,
    has_clock: bool,
) -> Result<Vec<ColumnSpec>, String> {
    let Some(pairs) = columns else {
        return Ok(default_columns());
    };
    if pairs.is_empty() {
        return Err("no columns selected".to_string());
    }
    let mut seen = HashSet::new();
    let mut parsed = Vec::with_capacity(pairs.len());
    for (source, name) in pairs {
        let kind = match source.as_str() {
            "instance" => ColumnKind::Instance,
            "base_cell" => ColumnKind::BaseCell,
            "status" => ColumnKind::Status,
            "orient" => ColumnKind::Orient,
            "is_seq" => {
                if !has_sequential {
                    return Err("column 'is_seq' requires a Liberty library (--lib)".to_string());
                }
                ColumnKind::IsSeq
            }
            "is_clock" => {
                if !has_clock {
                    return Err(
                        "column 'is_clock' requires clock-cell data (--clock-cells, produced by sdc-tools)"
                            .to_string(),
                    );
                }
                ColumnKind::IsClock
            }
            "llx" => ColumnKind::Llx,
            "lly" => ColumnKind::Lly,
            "urx" => ColumnKind::Urx,
            "ury" => ColumnKind::Ury,
            other => {
                return Err(format!(
                    "unknown column '{other}' (choose instance, base_cell, status, orient, is_seq, is_clock, llx, lly, urx, ury)"
                ));
            }
        };
        if name.is_empty() {
            return Err(format!("empty output name for column '{source}'"));
        }
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate output column '{name}'"));
        }
        parsed.push(ColumnSpec { kind, name });
    }
    Ok(parsed)
}

pub(crate) const BATCH_SIZE: usize = 65_536;
/// Byte cap on a batch of net records, which vary from tens of bytes to
/// hundreds of MB.
pub(crate) const BATCH_BYTES: usize = 64 << 20;
pub(crate) const CACHE_VERSION: u32 = 1;
pub(crate) const CACHE_HEADER_LEN: u64 = 20;
pub(crate) const MANIFEST_MAGIC: &[u8; 8] = b"LDTMAN1\0";
pub(crate) const COMPONENTS_MAGIC: &[u8; 8] = b"LDTCMP1\0";
pub(crate) const NET_DATA_MAGIC: &[u8; 8] = b"LDTNDT1\0";
pub(crate) const NET_INDEX_MAGIC: &[u8; 8] = b"LDTNIX1\0";
pub(crate) const NET_MAP_MAGIC: &[u8; 8] = b"LDTNMP1\0";
pub(crate) const CONNECTIVITY_MAGIC: &[u8; 8] = b"LDTCNX1\0";
pub(crate) const NET_INDEX_ENTRY_LEN: u64 = 25;

#[doc(hidden)]
pub type MacroMap = HashMap<String, MacroDef>;

#[derive(Clone, Debug)]
#[doc(hidden)]
pub struct MacroDef {
    pub width: i64,
    pub height: i64,
    pub _origin_x: i64,
    pub _origin_y: i64,
}

#[derive(Debug)]
#[doc(hidden)]
pub struct RawRecord {
    pub bytes: Vec<u8>,
    pub offset: u64,
}

#[derive(Debug)]
pub(crate) struct NetRecord {
    pub bytes: Vec<u8>,
    pub offset: u64,
    pub section: &'static str,
}

/// A placed component whose names borrow from its record bytes, so the
/// writers never allocate per instance.
#[derive(Clone, Copy, Debug)]
#[doc(hidden)]
pub struct Component<'a> {
    pub instance: &'a str,
    pub macro_name: &'a str,
    pub status: &'static str,
    pub orient: &'static str,
    pub llx: i64,
    pub lly: i64,
    pub urx: i64,
    pub ury: i64,
}

impl Component<'_> {
    pub fn to_bbox(self) -> InstanceBBox {
        InstanceBBox {
            instance: self.instance.to_string(),
            macro_name: self.macro_name.to_string(),
            status: self.status.to_string(),
            orient: self.orient.to_string(),
            llx: self.llx,
            lly: self.lly,
            urx: self.urx,
            ury: self.ury,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Placement {
    pub status: &'static str,
    pub x: i64,
    pub y: i64,
    pub orient: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceBBox {
    pub instance: String,
    pub macro_name: String,
    pub status: String,
    pub orient: String,
    pub llx: i64,
    pub lly: i64,
    pub urx: i64,
    pub ury: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetShape {
    pub net: String,
    pub section: String,
    pub route_status: String,
    pub shape_type: String,
    pub layer: String,
    pub width: i64,
    pub x1: i64,
    pub y1: i64,
    pub x2: i64,
    pub y2: i64,
    pub llx: i64,
    pub lly: i64,
    pub urx: i64,
    pub ury: i64,
}

/// Clock-related pin roles for one cell macro.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MacroPinRole {
    /// Case-sensitive macro name used in DEF component records.
    pub macro_name: String,
    /// Input pins that can pass a clock through the cell. For an integrated
    /// clock gate, include its clock input here.
    pub inputs: Vec<String>,
    /// Output pins that can drive a clock net.
    pub outputs: Vec<String>,
    /// Whether the macro ends a clock trace, as a flip-flop or latch does.
    pub endpoint: bool,
    /// Pins that receive a clock. Used by the forward clock-tracing routine.
    pub clock_pins: Vec<String>,
}

impl From<(String, crate::liberty::CellClockRole)> for MacroPinRole {
    fn from((macro_name, role): (String, crate::liberty::CellClockRole)) -> Self {
        Self {
            macro_name,
            inputs: role.inputs,
            outputs: role.outputs,
            endpoint: role.endpoint,
            clock_pins: role.clock_pins,
        }
    }
}

fn role_map(macros: &[MacroPinRole]) -> HashMap<&str, &MacroPinRole> {
    macros
        .iter()
        .map(|role| (role.macro_name.as_str(), role))
        .collect()
}

pub(crate) fn has_pin(pins: &[String], pin: &str) -> bool {
    pins.iter().any(|candidate| candidate == pin)
}

/// Roles by macro id (`None` for a macro without a Liberty cell), built once
/// by name rather than per connection.
fn roles_by_macro<'a>(
    c: &Connectivity,
    macros: &'a [MacroPinRole],
) -> Vec<Option<&'a MacroPinRole>> {
    let by_name = role_map(macros);
    (0..c.macros.len() as u32)
        .map(|id| by_name.get(c.macros.get(id)).copied())
        .collect()
}

/// Trace clock-tree cells forward from symbolic SDC roots in an on-disk DEF.
///
/// Roots are `port:NAME`, `net:NAME`, `pin:INSTANCE/PIN`, or `cell:INSTANCE`.
/// Combinational cells propagate from clock-through inputs to their outputs.
/// Sequential endpoints terminate that propagation and are included when a
/// known clock net is driven by one of their outputs. With `prune`, retain
/// cells on a path to a clock pin and divider drivers on explicit clock roots.
/// Results retain DEF instance spelling and are sorted.
pub fn clock_tree_cells(
    def_path: &Path,
    roots: &[String],
    macros: &[MacroPinRole],
    prune: bool,
) -> Result<Vec<String>, crate::Error> {
    let c = collect_def_connectivity(def_path)?;
    let roles = roles_by_macro(&c, macros);
    let role_of = |instance: u32| -> Option<&MacroPinRole> {
        if instance >= c.component_count {
            return None;
        }
        roles[c.instance_macro[instance as usize] as usize]
    };
    let adjacency = c.adjacency();
    let components = c.component_lookup();
    let mut net_by_name: HashMap<&str, u32> = HashMap::new();
    for net in 0..c.nets.len() as u32 {
        net_by_name.insert(c.nets.get(net), net);
    }

    let mut seeds: Vec<u32> = Vec::new();
    for root in roots {
        collect_root_nets(
            root,
            &c,
            &net_by_name,
            &adjacency,
            &components,
            &roles,
            &mut seeds,
        );
    }
    seeds.sort_unstable();
    seeds.dedup();

    // Each candidate instance is expanded once (`first_visit` below), so the
    // queue is bounded by the seeds plus the connections; `reached_from`
    // still records every reach.
    let mut queue: VecDeque<u32> = seeds.iter().copied().collect();
    let mut seen_nets = vec![false; c.nets.len()];
    // Nets a clock pin hangs on, and the candidates fed by / driving each net.
    let mut sink_nets: Vec<u32> = Vec::new();
    let mut live: HashSet<u32> = HashSet::new();
    let mut reached_from: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut drivers: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut endpoint_drivers = Vec::new();

    while let Some(net_index) = queue.pop_front() {
        if seen_nets[net_index as usize] {
            continue;
        }
        seen_nets[net_index as usize] = true;
        let mut is_sink = false;
        for (instance, pin) in c.conns(net_index) {
            if instance == PORT {
                continue;
            }
            let Some(role) = role_of(instance) else {
                continue;
            };
            let pin = c.pins.get(pin);
            if role.endpoint {
                is_sink |= has_pin(&role.clock_pins, pin);
                if has_pin(&role.outputs, pin) {
                    endpoint_drivers.push((net_index, instance));
                }
                continue;
            }
            if role.outputs.is_empty() || !has_pin(&role.inputs, pin) {
                continue;
            }
            // A clock gate reached on its clock pin is on the tree by itself.
            if has_pin(&role.clock_pins, pin) {
                live.insert(instance);
            }
            let reached = reached_from.entry(instance).or_default();
            let first_visit = reached.is_empty();
            reached.push(net_index);
            if !first_visit {
                continue;
            }
            for (next_net, out_pin) in adjacency.of(instance) {
                if has_pin(&role.outputs, c.pins.get(out_pin)) {
                    drivers.entry(next_net).or_default().push(instance);
                    if !seen_nets[next_net as usize] {
                        queue.push_back(next_net);
                    }
                }
            }
        }
        if is_sink {
            sink_nets.push(net_index);
        }
    }

    if prune {
        // Walk back from every clock pin: a net is live when a clock pin or a
        // live candidate reads it, a candidate when it drives a live net.
        let mut live_nets = vec![false; c.nets.len()];
        let mut pending: Vec<u32> = sink_nets;
        for instance in &live {
            pending.extend(reached_from.get(instance).into_iter().flatten());
        }
        while let Some(net_index) = pending.pop() {
            if live_nets[net_index as usize] {
                continue;
            }
            live_nets[net_index as usize] = true;
            for instance in drivers.get(&net_index).into_iter().flatten() {
                if live.insert(*instance) {
                    pending.extend(reached_from.get(instance).into_iter().flatten());
                }
            }
        }
        live.extend(endpoint_drivers.into_iter().filter_map(|(net, instance)| {
            (live_nets[net as usize] || seeds.binary_search(&net).is_ok()).then_some(instance)
        }));
    } else {
        live.extend(reached_from.into_keys());
        live.extend(endpoint_drivers.into_iter().map(|(_, instance)| instance));
    }
    let mut result = live
        .into_iter()
        .map(|id| c.instances.get(id).to_string())
        .collect::<Vec<_>>();
    result.sort();
    Ok(result)
}

/// DEF escapes `[`, `]` and `/` inside names; other files usually do not.
pub(crate) fn unescaped(name: &str) -> Cow<'_, str> {
    if name.contains('\\') {
        Cow::Owned(name.replace('\\', ""))
    } else {
        Cow::Borrowed(name)
    }
}

/// Instance classification and diagnostics from explicit DEF clock objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockObjectResolution {
    /// Classified instance names, deduplicated and sorted.
    pub instances: Vec<String>,
    /// Input clock net names absent from the DEF, in input order.
    pub unmatched_nets: Vec<String>,
    /// Input clock pins with an absent instance or top-level port, in input order.
    pub unmatched_pins: Vec<(String, String)>,
    /// Instances associated with a supplied clock object whose macros had no
    /// supplied role, deduplicated and sorted.
    pub unclassified: Vec<String>,
}

/// Resolve explicit clock net names and `(instance, pin)` pairs against a DEF.
///
/// An instance is selected when one of its output pins drives a supplied
/// clock net, or a supplied pair names one of its output pins. A supplied
/// clock-through input selects the instance when it can propagate a clock.
/// A pair with instance `"-"` denotes a top-level port, matched against the
/// DEF net connections.
///
/// DEF net, instance, and top-level port names are matched with backslash
/// escapes removed. For an explicit pin pair, escapes are removed from the
/// supplied pin name before it is compared with the macro role.
/// Returned instance names retain their spelling in the DEF. If more than one
/// role is supplied for a macro, the last role wins.
pub fn clock_object_instances(
    def_path: &Path,
    clock_nets: &[String],
    clock_pins: &[(String, String)],
    macros: &[MacroPinRole],
) -> Result<ClockObjectResolution, crate::Error> {
    let c = collect_def_connectivity(def_path)?;
    let macro_map = role_map(macros);
    let mut cells: HashSet<u32> = HashSet::new();
    let mut unclassified: HashSet<u32> = HashSet::new();

    let mut unmatched_nets = Vec::new();
    if !clock_nets.is_empty() {
        // Later nets win on a duplicated (unescaped) name.
        let net_by_name: HashMap<Cow<'_, str>, u32> = (0..c.nets.len() as u32)
            .map(|net| (unescaped(c.nets.get(net)), net))
            .collect();
        for name in clock_nets {
            let Some(&net) = net_by_name.get(&unescaped(name)) else {
                unmatched_nets.push(name.clone());
                continue;
            };
            for (instance, pin) in c.conns(net) {
                if instance == PORT {
                    continue;
                }
                let Some(macro_name) = c.macro_of(instance) else {
                    continue;
                };
                match macro_map.get(macro_name) {
                    None => {
                        unclassified.insert(instance);
                    }
                    Some(role) => {
                        if has_pin(&role.outputs, c.pins.get(pin)) {
                            cells.insert(instance);
                        }
                    }
                }
            }
        }
    }

    let mut unmatched_pins = Vec::new();
    if !clock_pins.is_empty() {
        // Ascending ids, so the last definition wins on a duplicated name.
        let instance_by_name: HashMap<Cow<'_, str>, u32> = (0..c.component_count)
            .map(|id| (unescaped(c.instances.get(id)), id))
            .collect();
        let mut ports: HashSet<Cow<'_, str>> = HashSet::new();
        for net in 0..c.nets.len() as u32 {
            for (instance, pin) in c.conns(net) {
                if instance == PORT {
                    ports.insert(unescaped(c.pins.get(pin)));
                }
            }
        }
        for (instance, pin) in clock_pins {
            let pin_name = unescaped(pin);
            if instance == "-" {
                if !ports.contains(&pin_name) {
                    unmatched_pins.push((instance.clone(), pin.clone()));
                }
                continue;
            }
            let Some(&id) = instance_by_name.get(&unescaped(instance)) else {
                unmatched_pins.push((instance.clone(), pin.clone()));
                continue;
            };
            let macro_name = c.macro_of(id).unwrap_or_default();
            match macro_map.get(macro_name) {
                None => {
                    unclassified.insert(id);
                }
                Some(role) => {
                    if has_pin(&role.outputs, &pin_name)
                        || (!role.endpoint && has_pin(&role.inputs, &pin_name))
                    {
                        cells.insert(id);
                    }
                }
            }
        }
    }

    let sorted = |set: HashSet<u32>| {
        let mut values: Vec<String> = set
            .into_iter()
            .map(|id| c.instances.get(id).to_string())
            .collect();
        values.sort();
        values
    };
    Ok(ClockObjectResolution {
        instances: sorted(cells),
        unmatched_nets,
        unmatched_pins,
        unclassified: sorted(unclassified),
    })
}

pub(crate) fn collect_root_nets(
    root: &str,
    c: &Connectivity,
    net_by_name: &HashMap<&str, u32>,
    adjacency: &Adjacency,
    components: &HashMap<&str, u32>,
    roles: &[Option<&MacroPinRole>],
    seeds: &mut Vec<u32>,
) {
    // Components first (last duplicate wins), then instances seen only in nets.
    let instance_id = |name: &str| -> Option<u32> {
        components.get(name).copied().or_else(|| {
            (c.component_count..c.instances.len() as u32).find(|&id| c.instances.get(id) == name)
        })
    };
    if let Some(name) = root.strip_prefix("port:") {
        if let Some(&index) = net_by_name.get(name) {
            seeds.push(index);
        }
        for net in 0..c.nets.len() as u32 {
            if c.conns(net)
                .any(|(instance, pin)| instance == PORT && c.pins.get(pin) == name)
            {
                seeds.push(net);
            }
        }
    } else if let Some(name) = root.strip_prefix("net:") {
        if let Some(&index) = net_by_name.get(name) {
            seeds.push(index);
        }
    } else if let Some(path) = root.strip_prefix("pin:") {
        let Some((instance, pin)) = split_root_pin(path) else {
            return;
        };
        if let Some(id) = instance_id(instance) {
            for (net, conn_pin) in adjacency.of(id) {
                if c.pins.get(conn_pin) == pin {
                    seeds.push(net);
                }
            }
        }
    } else if let Some(instance) = root.strip_prefix("cell:") {
        let Some(&id) = components.get(instance) else {
            return;
        };
        let Some(role) = roles[c.instance_macro[id as usize] as usize] else {
            return;
        };
        for (net, pin) in adjacency.of(id) {
            if has_pin(&role.outputs, c.pins.get(pin)) {
                seeds.push(net);
            }
        }
    }
}

pub(crate) fn split_root_pin(path: &str) -> Option<(&str, &str)> {
    path.rsplit_once('/').or_else(|| path.rsplit_once('.'))
}

pub(crate) fn instance_table_widths(
    def_path: &Path,
    columns: &[ColumnSpec],
) -> Result<(Option<usize>, Option<usize>), String> {
    let mut instance_width = columns
        .iter()
        .find(|column| column.kind == ColumnKind::Instance)
        .map(|column| column.name.len());
    let mut macro_width = columns
        .iter()
        .find(|column| column.kind == ColumnKind::BaseCell)
        .map(|column| column.name.len());
    if instance_width.is_none() && macro_width.is_none() {
        return Ok((None, None));
    }

    let mut scanner = DefScanner::from_path(def_path).map_err(|e| e.to_string())?;
    for_each_batch(
        || scanner.next_batch(BATCH_SIZE),
        |batch, overlapped| {
            let (instance_len, macro_len) = crate::threads::install_for(overlapped, || {
                let chunks: Vec<Result<(usize, usize), String>> = batch
                    .par_chunks(CHUNK)
                    .map(|chunk| {
                        let mut longest = (0, 0);
                        for record in chunk {
                            if let Some((instance, macro_name)) = component_name_lengths(record)? {
                                longest = (longest.0.max(instance), longest.1.max(macro_name));
                            }
                        }
                        Ok(longest)
                    })
                    .collect();
                chunks.into_iter().try_fold((0, 0), |acc, chunk| {
                    let chunk = chunk?;
                    Ok::<_, String>((acc.0.max(chunk.0), acc.1.max(chunk.1)))
                })
            })?;
            if let Some(width) = instance_width.as_mut() {
                *width = (*width).max(instance_len);
            }
            if let Some(width) = macro_width.as_mut() {
                *width = (*width).max(macro_len);
            }
            Ok(())
        },
    )?;
    Ok((instance_width, macro_width))
}

/// Feed batches from `next` to `consume` in order until `next` returns an
/// empty batch.
///
/// When the CPU budget allows (`threads::may_overlap`), `next` runs on its
/// own scanner thread one batch ahead, so the sequential scan overlaps the
/// parallel parse; `consume` is told so and must then run its parallel work
/// with `threads::install_for(true, ..)`, which leaves one CPU to the
/// scanner. With a budget of one CPU nothing overlaps.
#[doc(hidden)]
pub fn for_each_batch<T: Send>(
    mut next: impl FnMut() -> io::Result<Vec<T>> + Send,
    mut consume: impl FnMut(Vec<T>, bool) -> Result<(), String>,
) -> Result<(), String> {
    if !crate::threads::may_overlap() {
        loop {
            let batch = next().map_err(|e| e.to_string())?;
            if batch.is_empty() {
                return Ok(());
            }
            consume(batch, false)?;
        }
    }
    std::thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<io::Result<Vec<T>>>(1);
        scope.spawn(move || {
            loop {
                let item = next();
                let last = !matches!(&item, Ok(batch) if !batch.is_empty());
                // A send fails only when the consumer stopped early.
                if sender.send(item).is_err() || last {
                    break;
                }
            }
        });
        for item in receiver {
            let batch = item.map_err(|e| e.to_string())?;
            if batch.is_empty() {
                break;
            }
            consume(batch, true)?;
        }
        Ok(())
    })
}

/// Lengths of a placed component's instance and macro names.
///
/// Mirrors `parse_component_record`'s placement rules without looking up the
/// macro: `+ UNPLACED` and records with no placement are skipped.
pub(crate) fn component_name_lengths(record: &RawRecord) -> Result<Option<(usize, usize)>, String> {
    let mut tokens = ByteTokens::new(&record.bytes);
    tokens.expect(b"-", record.offset, "component record")?;
    let instance = tokens
        .next()
        .ok_or_else(|| format!("expected component name near DEF byte {}", record.offset))?;
    let macro_name = tokens
        .next()
        .ok_or_else(|| format!("expected component model near DEF byte {}", record.offset))?;
    let instance_len = token_to_str(instance, record.offset, "component name")?.len();
    let macro_len = token_to_str(macro_name, record.offset, "component model")?.len();

    let mut placed = false;
    while let Some(token) = tokens.next() {
        if token != b"+" {
            continue;
        }
        let Some(keyword) = tokens.next() else {
            break;
        };
        match keyword {
            b"PLACED" | b"FIXED" | b"COVER" => placed = true,
            b"UNPLACED" => return Ok(None),
            _ => {}
        }
    }
    Ok(placed.then_some((instance_len, macro_len)))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn write_instance_table_stream<W: Write>(
    scanner: &mut DefScanner,
    macros: &MacroMap,
    columns: &[ColumnSpec],
    instance_width: Option<usize>,
    macro_width: Option<usize>,
    sequential: Option<&HashSet<String>>,
    clock: Option<&HashSet<String>>,
    mut out: W,
) -> Result<usize, String> {
    let widths: Vec<usize> = columns
        .iter()
        .map(|column| match column.kind {
            ColumnKind::Instance => instance_width.unwrap_or(column.name.len()),
            ColumnKind::BaseCell => macro_width.unwrap_or(column.name.len()),
            ColumnKind::Status => column.name.len().max(8),
            ColumnKind::Orient => column.name.len().max(6),
            ColumnKind::IsSeq | ColumnKind::IsClock => column.name.len().max(5),
            _ => column.name.len().max(12),
        })
        .collect();
    let line_width = widths.iter().sum::<usize>() + 2 * columns.len().saturating_sub(1);

    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            write!(out, "  ").map_err(|e| e.to_string())?;
        }
        match column.kind {
            ColumnKind::Instance
            | ColumnKind::BaseCell
            | ColumnKind::Status
            | ColumnKind::Orient
            | ColumnKind::IsSeq
            | ColumnKind::IsClock => {
                write!(out, "{:<width$}", column.name, width = widths[index])
            }
            _ => write!(out, "{:>width$}", column.name, width = widths[index]),
        }
        .map_err(|e| e.to_string())?;
    }
    writeln!(out).map_err(|e| e.to_string())?;
    writeln!(out, "{}", "-".repeat(line_width)).map_err(|e| e.to_string())?;

    let dbu = scanner.dbu_per_micron as f64;
    let flag = |value: bool| if value { "true" } else { "false" };
    let render = |buffer: &mut String, row: &Component<'_>| {
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                buffer.push_str("  ");
            }
            let width = widths[index];
            // Writing to a String cannot fail.
            let _ = match column.kind {
                ColumnKind::Instance => write!(buffer, "{:<width$}", row.instance),
                ColumnKind::BaseCell => write!(buffer, "{:<width$}", row.macro_name),
                ColumnKind::Status => write!(buffer, "{:<width$}", row.status),
                ColumnKind::Orient => write!(buffer, "{:<width$}", row.orient),
                ColumnKind::IsSeq => write!(
                    buffer,
                    "{:<width$}",
                    flag(sequential.is_some_and(|set| set.contains(row.macro_name)))
                ),
                ColumnKind::IsClock => write!(
                    buffer,
                    "{:<width$}",
                    flag(clock.is_some_and(|set| set.contains(row.instance)))
                ),
                ColumnKind::Llx => write!(buffer, "{:>width$.3}", row.llx as f64 / dbu),
                ColumnKind::Lly => write!(buffer, "{:>width$.3}", row.lly as f64 / dbu),
                ColumnKind::Urx => write!(buffer, "{:>width$.3}", row.urx as f64 / dbu),
                ColumnKind::Ury => write!(buffer, "{:>width$.3}", row.ury as f64 / dbu),
            };
        }
        buffer.push('\n');
    };
    let count = write_component_rows(scanner, macros, &render, &mut out)?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(count)
}

/// Parse every remaining component batch, render the placed rows in
/// parallel and write them in file order. Returns the row count.
pub(crate) fn write_component_rows<W: Write>(
    scanner: &mut DefScanner,
    macros: &MacroMap,
    render: &(impl Fn(&mut String, &Component<'_>) + Sync),
    out: &mut W,
) -> Result<usize, String> {
    let mut count = 0usize;
    for_each_batch(
        || scanner.next_batch(BATCH_SIZE),
        |batch, overlapped| {
            let (rows, texts) = crate::threads::install_for(overlapped, || {
                let rows = parse_components(&batch, macros)?;
                Ok::<_, String>((rows.len(), render_rows(&rows, render)))
            })?;
            count += rows;
            for text in &texts {
                out.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            }
            Ok(())
        },
    )?;
    Ok(count)
}

#[doc(hidden)]
pub fn write_instance_table_impl(
    lefs: &[PathBuf],
    def_path: &Path,
    output: Option<&Path>,
    columns: &[ColumnSpec],
    sequential: Option<&HashSet<String>>,
    clock: Option<&HashSet<String>>,
) -> Result<usize, String> {
    let (instance_width, macro_width) = instance_table_widths(def_path, columns)?;
    let mut scanner = DefScanner::from_path(def_path).map_err(|e| e.to_string())?;
    let (macros, duplicates) = parse_lef_files(lefs, scanner.dbu_per_micron)?;
    if !duplicates.is_empty() {
        eprintln!(
            "warning: duplicate LEF macros, last definition used: {}",
            duplicates.join(", ")
        );
    }

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_instance_table_stream(
                &mut scanner,
                &macros,
                columns,
                instance_width,
                macro_width,
                sequential,
                clock,
                BufWriter::new(file),
            )
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_instance_table_stream(
                &mut scanner,
                &macros,
                columns,
                instance_width,
                macro_width,
                sequential,
                clock,
                BufWriter::new(lock),
            )
        }
    }
}

pub(crate) fn write_instance_bbox_csv_stream<W: Write>(
    scanner: &mut DefScanner,
    macros: &MacroMap,
    columns: &[ColumnSpec],
    sequential: Option<&HashSet<String>>,
    clock: Option<&HashSet<String>>,
    mut out: W,
) -> Result<usize, String> {
    let mut header = String::new();
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            header.push(',');
        }
        append_csv_field(&mut header, &column.name);
    }
    header.push('\n');
    out.write_all(header.as_bytes())
        .map_err(|e| e.to_string())?;

    let dbu = scanner.dbu_per_micron as f64;
    let flag = |value: bool| if value { "true" } else { "false" };
    let render = |buffer: &mut String, row: &Component<'_>| {
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                buffer.push(',');
            }
            match column.kind {
                ColumnKind::Instance => append_csv_field(buffer, row.instance),
                ColumnKind::BaseCell => append_csv_field(buffer, row.macro_name),
                ColumnKind::Status => append_csv_field(buffer, row.status),
                ColumnKind::Orient => append_csv_field(buffer, row.orient),
                ColumnKind::IsSeq => append_csv_field(
                    buffer,
                    flag(sequential.is_some_and(|set| set.contains(row.macro_name))),
                ),
                ColumnKind::IsClock => append_csv_field(
                    buffer,
                    flag(clock.is_some_and(|set| set.contains(row.instance))),
                ),
                ColumnKind::Llx => append_csv_micron(buffer, row.llx as f64 / dbu),
                ColumnKind::Lly => append_csv_micron(buffer, row.lly as f64 / dbu),
                ColumnKind::Urx => append_csv_micron(buffer, row.urx as f64 / dbu),
                ColumnKind::Ury => append_csv_micron(buffer, row.ury as f64 / dbu),
            }
        }
        buffer.push('\n');
    };
    let count = write_component_rows(scanner, macros, &render, &mut out)?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(count)
}

#[doc(hidden)]
pub fn write_instance_bbox_csv_impl(
    lefs: &[PathBuf],
    def_path: &Path,
    output: Option<&Path>,
    columns: &[ColumnSpec],
    sequential: Option<&HashSet<String>>,
    clock: Option<&HashSet<String>>,
) -> Result<usize, String> {
    let mut scanner = DefScanner::from_path(def_path).map_err(|e| e.to_string())?;
    let (macros, duplicates) = parse_lef_files(lefs, scanner.dbu_per_micron)?;
    if !duplicates.is_empty() {
        eprintln!(
            "warning: duplicate LEF macros, last definition used: {}",
            duplicates.join(", ")
        );
    }

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_instance_bbox_csv_stream(
                &mut scanner,
                &macros,
                columns,
                sequential,
                clock,
                BufWriter::new(file),
            )
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_instance_bbox_csv_stream(
                &mut scanner,
                &macros,
                columns,
                sequential,
                clock,
                BufWriter::new(lock),
            )
        }
    }
}

#[doc(hidden)]
pub fn write_net_shapes_csv_impl(
    def_path: &Path,
    net: &str,
    layer: &str,
    output: Option<&Path>,
) -> Result<SkippedRoutes, String> {
    let mut scanner = NetRecordScanner::from_path(def_path).map_err(|e| e.to_string())?;
    let net_bytes = net.as_bytes();
    let layer_bytes = layer.as_bytes();

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_net_shapes_csv_stream(&mut scanner, net_bytes, layer_bytes, BufWriter::new(file))
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_net_shapes_csv_stream(&mut scanner, net_bytes, layer_bytes, BufWriter::new(lock))
        }
    }
}

/// The skip counts as a Python dict, built with the GIL held.
#[doc(hidden)]
pub fn write_def_cache_impl(def_path: &Path, cache_dir: &Path) -> Result<(), String> {
    create_dir_all(cache_dir).map_err(|e| format!("{}: {e}", cache_dir.display()))?;
    write_manifest(cache_dir)?;

    let components_path = cache_dir.join("components.bin");
    let net_data_path = cache_dir.join("nets.dat");
    let net_index_path = cache_dir.join("nets.idx");
    let connectivity_path = cache_dir.join("connectivity.bin");

    let mut components = BufWriter::new(
        File::create(&components_path)
            .map_err(|e| format!("{}: {e}", components_path.display()))?,
    );
    let mut net_data = BufWriter::new(
        File::create(&net_data_path).map_err(|e| format!("{}: {e}", net_data_path.display()))?,
    );
    let mut net_index = BufWriter::new(
        File::create(&net_index_path).map_err(|e| format!("{}: {e}", net_index_path.display()))?,
    );
    let mut connectivity = BufWriter::new(
        File::create(&connectivity_path)
            .map_err(|e| format!("{}: {e}", connectivity_path.display()))?,
    );

    write_header(&mut components, COMPONENTS_MAGIC, 0)?;
    write_header(&mut net_data, NET_DATA_MAGIC, 0)?;
    write_header(&mut net_index, NET_INDEX_MAGIC, 0)?;
    write_header(&mut connectivity, CONNECTIVITY_MAGIC, 0)?;

    let mut lexer = Lexer::new(open_input(def_path).map_err(|e| e.to_string())?);
    let mut component_count = 0u64;
    let mut components_seen = false;
    let mut net_count = 0u64;
    let mut connectivity_count = 0u64;
    let mut data_offset = CACHE_HEADER_LEN;
    let mut net_map: HashMap<String, Vec<u64>> = HashMap::new();

    let mut writer = NetCacheWriter {
        data: &mut net_data,
        index: &mut net_index,
        connectivity: &mut connectivity,
        net_map: &mut net_map,
        data_offset: &mut data_offset,
        net_count: &mut net_count,
        connectivity_count: &mut connectivity_count,
    };
    while let Some(token) = lexer.next_token().map_err(|e| e.to_string())? {
        if token.text == "-" {
            // A record of a section the cache does not store.
            lexer.skip_until_semicolon().map_err(|e| e.to_string())?;
        } else if token.text.eq_ignore_ascii_case("COMPONENTS") {
            let _count = lexer.next_token().map_err(|e| e.to_string())?;
            expect_semicolon(&mut lexer).map_err(|e| e.to_string())?;
            if components_seen && *writer.net_count > 0 {
                return Err(second_components_error(token.offset));
            }
            components_seen = true;
            stream_components_to_cache(&mut lexer, &mut components, &mut component_count)?;
        } else if token.text.eq_ignore_ascii_case("NETS") {
            let _count = lexer.next_token().map_err(|e| e.to_string())?;
            expect_semicolon(&mut lexer).map_err(|e| e.to_string())?;
            stream_nets_to_cache(&mut lexer, "NETS", &mut writer)?;
        } else if token.text.eq_ignore_ascii_case("SPECIALNETS") {
            let _count = lexer.next_token().map_err(|e| e.to_string())?;
            expect_semicolon(&mut lexer).map_err(|e| e.to_string())?;
            stream_nets_to_cache(&mut lexer, "SPECIALNETS", &mut writer)?;
        }
    }

    components.flush().map_err(|e| e.to_string())?;
    net_data.flush().map_err(|e| e.to_string())?;
    net_index.flush().map_err(|e| e.to_string())?;
    connectivity.flush().map_err(|e| e.to_string())?;

    patch_header_count(&components_path, component_count)?;
    patch_header_count(&net_data_path, net_count)?;
    patch_header_count(&net_index_path, net_count)?;
    patch_header_count(&connectivity_path, connectivity_count)?;
    write_net_map_bin_from_map(cache_dir, &net_map)?;
    Ok(())
}

#[doc(hidden)]
pub fn write_net_shapes_csv_from_cache_impl(
    cache_dir: &Path,
    net: &str,
    layer: &str,
    output: Option<&Path>,
) -> Result<SkippedRoutes, String> {
    read_manifest(cache_dir)?;
    let records = load_net_records_for_net(cache_dir, net)?;
    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_net_shapes_records_csv(
                &records,
                net.as_bytes(),
                layer.as_bytes(),
                BufWriter::new(file),
            )
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_net_shapes_records_csv(
                &records,
                net.as_bytes(),
                layer.as_bytes(),
                BufWriter::new(lock),
            )
        }
    }
}

#[doc(hidden)]
pub fn write_clock_instances_csv_impl(
    def_path: &Path,
    roots: &[(String, String, String)],
    clock_cell_patterns: &[String],
    output: Option<&Path>,
) -> Result<(), String> {
    let c = collect_def_connectivity(def_path).map_err(|err| err.to_string())?;
    let patterns: Vec<String> = if clock_cell_patterns.is_empty() {
        vec!["clk".to_string()]
    } else {
        clock_cell_patterns
            .iter()
            .map(|value| value.to_ascii_lowercase())
            .collect()
    };

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_clock_instances_csv_stream(&c, roots, &patterns, BufWriter::new(file))
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_clock_instances_csv_stream(&c, roots, &patterns, BufWriter::new(lock))
        }
    }
}

#[doc(hidden)]
pub fn write_clock_instances_csv_from_cache_impl(
    cache_dir: &Path,
    roots: &[(String, String, String)],
    clock_cell_patterns: &[String],
    output: Option<&Path>,
) -> Result<(), String> {
    read_manifest(cache_dir)?;
    let c = load_connectivity(cache_dir)?;
    let patterns: Vec<String> = if clock_cell_patterns.is_empty() {
        vec!["clk".to_string()]
    } else {
        clock_cell_patterns
            .iter()
            .map(|value| value.to_ascii_lowercase())
            .collect()
    };

    match output {
        Some(path) => {
            let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            write_clock_instances_csv_stream(&c, roots, &patterns, BufWriter::new(file))
        }
        None => {
            let stdout = io::stdout();
            let lock = stdout.lock();
            write_clock_instances_csv_stream(&c, roots, &patterns, BufWriter::new(lock))
        }
    }
}

pub(crate) fn write_clock_instances_csv_stream<W: Write>(
    c: &Connectivity,
    roots: &[(String, String, String)],
    clock_cell_patterns: &[String],
    mut out: W,
) -> Result<(), String> {
    let adjacency = c.adjacency();

    writeln!(
        out,
        "clock,root_type,root_name,instance,macro,net,is_clock_cell"
    )
    .map_err(|e| e.to_string())?;

    let mut buffer = String::with_capacity(64 * 1024);
    for (clock, root_type, root_name) in roots {
        let initial_nets = seed_clock_nets(c, root_type, root_name);
        let rows = traverse_clock_instances(c, &adjacency, &initial_nets, clock_cell_patterns);
        for (instance, macro_name, net, is_clock_cell) in rows {
            append_csv_field(&mut buffer, clock);
            buffer.push(',');
            append_csv_field(&mut buffer, root_type);
            buffer.push(',');
            append_csv_field(&mut buffer, root_name);
            buffer.push(',');
            append_csv_field(&mut buffer, instance);
            buffer.push(',');
            append_csv_field(&mut buffer, macro_name);
            buffer.push(',');
            append_csv_field(&mut buffer, net);
            let _ = writeln!(&mut buffer, ",{}", is_clock_cell);
        }
    }

    out.write_all(buffer.as_bytes())
        .map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

pub(crate) fn write_net_shapes_csv_stream<W: Write>(
    scanner: &mut NetRecordScanner,
    net: &[u8],
    layer: &[u8],
    mut out: W,
) -> Result<SkippedRoutes, String> {
    writeln!(
        out,
        "net,section,route_status,shape_type,layer,width,x1,y1,x2,y2,llx,lly,urx,ury"
    )
    .map_err(|e| e.to_string())?;
    let mut skipped = SkippedRoutes::default();
    loop {
        let batch = scanner
            .next_matching_batch(BATCH_SIZE, net)
            .map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        let (shapes, batch_skipped) = process_net_record_batch(&batch, net, layer)?;
        skipped.add(batch_skipped);
        let mut buffer = String::with_capacity(shapes.len() * 128);
        for shape in shapes {
            append_net_shape_csv_row(&mut buffer, &shape);
        }
        out.write_all(buffer.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(skipped)
}

pub(crate) fn write_net_shapes_records_csv<W: Write>(
    records: &[NetRecord],
    net: &[u8],
    layer: &[u8],
    mut out: W,
) -> Result<SkippedRoutes, String> {
    writeln!(
        out,
        "net,section,route_status,shape_type,layer,width,x1,y1,x2,y2,llx,lly,urx,ury"
    )
    .map_err(|e| e.to_string())?;
    let (shapes, skipped) = process_net_record_batch(records, net, layer)?;
    let mut buffer = String::with_capacity(shapes.len() * 128);
    for shape in shapes {
        append_net_shape_csv_row(&mut buffer, &shape);
    }
    out.write_all(buffer.as_bytes())
        .map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(skipped)
}

/// Records per parallel task: large enough to amortize scheduling, small
/// enough to balance 65k-record batches across many cores.
pub(crate) const CHUNK: usize = 2048;

/// Parse a batch into placed components, in record order. Chunks parse in
/// parallel; the first error in record order is reported.
#[doc(hidden)]
pub fn parse_components<'a>(
    batch: &'a [RawRecord],
    macros: &MacroMap,
) -> Result<Vec<Component<'a>>, String> {
    parse_components_with_fallback(batch, macros, None)
}

fn parse_components_with_fallback<'a>(
    batch: &'a [RawRecord],
    macros: &MacroMap,
    fallback: Option<&MacroDef>,
) -> Result<Vec<Component<'a>>, String> {
    let chunks: Vec<Result<Vec<Component<'a>>, String>> = crate::threads::install(|| {
        batch
            .par_chunks(CHUNK)
            .map(|chunk| {
                let mut rows = Vec::with_capacity(chunk.len());
                for record in chunk {
                    if let Some(row) = parse_component_record(record, macros, fallback)? {
                        rows.push(row);
                    }
                }
                Ok(rows)
            })
            .collect()
    });
    let mut rows = Vec::with_capacity(batch.len());
    for chunk in chunks {
        rows.extend(chunk?);
    }
    Ok(rows)
}

/// Render rows to text in parallel chunks, concatenated in row order.
pub(crate) fn render_rows<T: Sync>(
    rows: &[T],
    render: impl Fn(&mut String, &T) + Sync,
) -> Vec<String> {
    crate::threads::install(|| {
        rows.par_chunks(CHUNK)
            .map(|chunk| {
                let mut text = String::with_capacity(chunk.len() * 64);
                for row in chunk {
                    render(&mut text, row);
                }
                text
            })
            .collect()
    })
}

pub(crate) fn process_record_batch(
    batch: &[RawRecord],
    macros: &MacroMap,
) -> Result<Vec<InstanceBBox>, String> {
    crate::threads::install(|| {
        Ok(parse_components(batch, macros)?
            .into_iter()
            .map(Component::to_bbox)
            .collect())
    })
}

pub(crate) fn process_net_record_batch(
    batch: &[NetRecord],
    net: &[u8],
    layer: &[u8],
) -> Result<(Vec<NetShape>, SkippedRoutes), String> {
    let nested: Result<Vec<(Vec<NetShape>, SkippedRoutes)>, String> =
        crate::threads::install(|| {
            batch
                .par_iter()
                .map(|record| parse_net_shapes_record(record, net, layer))
                .collect()
        });
    let mut shapes = Vec::new();
    let mut skipped = SkippedRoutes::default();
    for (mut record_shapes, record_skipped) in nested? {
        shapes.append(&mut record_shapes);
        skipped.add(record_skipped);
    }
    Ok((shapes, skipped))
}

fn parse_component_record<'a>(
    record: &'a RawRecord,
    macros: &MacroMap,
    fallback: Option<&MacroDef>,
) -> Result<Option<Component<'a>>, String> {
    let mut tokens = ByteTokens::new(&record.bytes);
    tokens.expect(b"-", record.offset, "component record")?;
    let instance = tokens
        .next()
        .ok_or_else(|| format!("expected component name near DEF byte {}", record.offset))?;
    let macro_name = tokens
        .next()
        .ok_or_else(|| format!("expected component model near DEF byte {}", record.offset))?;
    let instance_str = token_to_str(instance, record.offset, "component name")?;
    let macro_str = token_to_str(macro_name, record.offset, "component model")?;
    let mut placement = None;

    while let Some(token) = tokens.next() {
        if token != b"+" {
            continue;
        }
        let Some(keyword) = tokens.next() else {
            break;
        };
        match keyword {
            b"PLACED" | b"FIXED" | b"COVER" => {
                tokens.expect(b"(", record.offset, "placement point")?;
                let x = tokens
                    .next_i64(record.offset, "placement x")?
                    .ok_or_else(|| {
                        format!("expected placement x near DEF byte {}", record.offset)
                    })?;
                let y = tokens
                    .next_i64(record.offset, "placement y")?
                    .ok_or_else(|| {
                        format!("expected placement y near DEF byte {}", record.offset)
                    })?;
                tokens.expect(b")", record.offset, "placement point")?;
                let orient = tokens.next().ok_or_else(|| {
                    format!("expected orientation near DEF byte {}", record.offset)
                })?;
                placement = Some(Placement {
                    status: placement_status(keyword),
                    x,
                    y,
                    orient: orientation(orient).ok_or_else(|| {
                        format!(
                            "unsupported orientation '{}' near DEF byte {}",
                            String::from_utf8_lossy(orient),
                            record.offset
                        )
                    })?,
                });
            }
            b"UNPLACED" => return Ok(None),
            _ => {}
        }
    }

    let Some(placement) = placement else {
        return Ok(None);
    };

    let macro_def = macros.get(macro_str).or(fallback).ok_or_else(|| {
        format!(
            "missing LEF macro '{}' for instance '{}' near DEF byte {}",
            macro_str, instance_str, record.offset
        )
    })?;
    let (width, height) = match placement.orient {
        "N" | "S" | "FN" | "FS" => (macro_def.width, macro_def.height),
        "E" | "W" | "FE" | "FW" => (macro_def.height, macro_def.width),
        other => {
            return Err(format!(
                "unsupported orientation '{other}' for instance '{instance_str}' near DEF byte {}",
                record.offset
            ));
        }
    };
    Ok(Some(Component {
        instance: instance_str,
        macro_name: macro_str,
        status: placement.status,
        orient: placement.orient,
        llx: placement.x,
        lly: placement.y,
        urx: placement.x.checked_add(width).ok_or_else(|| {
            format!(
                "bbox x coordinate out of range for {instance_str:?} near DEF byte {}",
                record.offset
            )
        })?,
        ury: placement.y.checked_add(height).ok_or_else(|| {
            format!(
                "bbox y coordinate out of range for {instance_str:?} near DEF byte {}",
                record.offset
            )
        })?,
    }))
}

pub(crate) fn parse_component_identity(
    bytes: &[u8],
    offset: u64,
) -> Result<(String, String), String> {
    let mut tokens = ByteTokens::new(bytes);
    tokens.expect(b"-", offset, "component record")?;
    let instance = tokens
        .next()
        .ok_or_else(|| format!("expected component name near DEF byte {offset}"))?;
    let macro_name = tokens
        .next()
        .ok_or_else(|| format!("expected component model near DEF byte {offset}"))?;
    Ok((
        token_to_str(instance, offset, "component name")?.to_string(),
        token_to_str(macro_name, offset, "component model")?.to_string(),
    ))
}

fn parse_net_shapes_record(
    record: &NetRecord,
    target_net: &[u8],
    target_layer: &[u8],
) -> Result<(Vec<NetShape>, SkippedRoutes), String> {
    let mut tokens = ByteTokens::new(&record.bytes);
    tokens.expect(b"-", record.offset, "net record")?;
    let net = tokens
        .next()
        .ok_or_else(|| format!("expected net name near DEF byte {}", record.offset))?;
    if !eq_token(net, target_net) {
        return Ok((Vec::new(), SkippedRoutes::default()));
    }
    let net_str = token_to_str(net, record.offset, "net name")?;

    let offset = record.offset;
    let out_of_range = || format!("route coordinate out of range near DEF byte {offset}");

    let mut shapes = Vec::new();
    let mut skipped = SkippedRoutes::default();
    let mut path = RoutePath::default();
    // A status with no layer (DEF 5.8 `+ FIXED + RECT ...`) is the status of
    // the next `+ RECT`/`+ POLYGON`/`+ VIA` only, as in the reference parser.
    let mut patch_status: Option<&'static str> = None;

    while let Some(token) = tokens.next() {
        if token == b"+" {
            let Some(keyword) = tokens.next() else {
                break;
            };
            match keyword {
                b"ROUTED" | b"FIXED" | b"COVER" | b"NOSHIELD" | b"SHIELD" => {
                    let status = if keyword == b"SHIELD" {
                        let _shield_net = tokens.next();
                        "SHIELD"
                    } else {
                        route_status(keyword)
                    };
                    if tokens
                        .peek()
                        .is_some_and(|next| next != b"+" && next != b";")
                    {
                        let layer = tokens.next();
                        let width = tokens.next_i64_or_rewind().unwrap_or(0);
                        path.start(status, layer, width);
                    } else {
                        path.end();
                        patch_status = Some(status);
                    }
                }
                b"SHAPE" => {
                    path.shape = tokens
                        .next()
                        .and_then(|value| std::str::from_utf8(value).ok())
                        .unwrap_or("");
                }
                b"STYLE" | b"MASK" => {
                    let _value = tokens.next();
                }
                b"RECT" => {
                    path.end();
                    let status = patch_status.take().unwrap_or("ROUTED");
                    let layer = tokens.next();
                    let mut corners = [(0, 0); 2];
                    for corner in &mut corners {
                        tokens.expect(b"(", offset, "special RECT point")?;
                        let point = read_route_point(&mut tokens, None, offset)?;
                        *corner = (point.x, point.y);
                    }
                    if let Some(layer) = layer.filter(|layer| eq_token(layer, target_layer)) {
                        let context = RouteContext::new(record, net_str, status, "RECT", layer, 0)?;
                        let bbox = rect_bbox(corners[0], corners[1]);
                        shapes.push(net_shape(&context, bbox, bbox));
                    }
                }
                b"POLYGON" => {
                    path.end();
                    patch_status = None;
                    if tokens
                        .next()
                        .is_some_and(|layer| eq_token(layer, target_layer))
                    {
                        skipped.polygon += 1;
                    }
                    tokens.skip_point_groups();
                }
                b"VIA" => {
                    path.end();
                    patch_status = None;
                    let _name = tokens.next();
                    tokens.skip_orientation();
                    tokens.skip_point_groups();
                }
                b"SUBNET" => {
                    path.end();
                    if skip_subnet(&mut tokens) {
                        skipped.subnet += 1;
                    }
                }
                // Every other net-level keyword ends the current path, so
                // its parenthesized values are never read as route points.
                _ => path.end(),
            }
            continue;
        }

        // Outside a path (before the first status, or after a keyword that
        // ended one) nothing is route geometry: the values of `+ VPIN` or
        // `+ PROPERTY kind RECT` are skipped.
        let Some(status) = path.status else {
            if token == b"(" {
                tokens.skip_until_close();
            }
            continue;
        };

        if token == b"NEW" {
            let layer = tokens.next();
            let width = tokens.next_i64_or_rewind().unwrap_or(0);
            path.start(status, layer, width);
            continue;
        }

        match token {
            b"VIRTUAL" => {
                tokens.expect(b"(", offset, "virtual point")?;
                path.previous = Some(read_route_point(&mut tokens, path.previous, offset)?);
            }
            b"TAPER" => {}
            b"MASK" | b"TAPERRULE" | b"STYLE" => {
                let _value = tokens.next();
            }
            b"RECT" => {
                let deltas = read_rect_deltas(&mut tokens, offset)?;
                let Some(previous) = path.previous else {
                    return Err(format!(
                        "RECT with no previous point near DEF byte {offset}"
                    ));
                };
                match path.layer {
                    Some(layer) if eq_token(layer, target_layer) => {
                        let corner = |dx: i64, dy: i64| {
                            previous
                                .x
                                .checked_add(dx)
                                .zip(previous.y.checked_add(dy))
                                .ok_or_else(out_of_range)
                        };
                        let bbox =
                            rect_bbox(corner(deltas[0], deltas[1])?, corner(deltas[2], deltas[3])?);
                        let context = RouteContext::new(record, net_str, status, "RECT", layer, 0)?;
                        shapes.push(net_shape(&context, bbox, bbox));
                    }
                    Some(_) => {}
                    None => skipped.after_via += 1,
                }
            }
            b"(" => {
                let point = read_route_point(&mut tokens, path.previous, offset)?;
                if let Some(previous) = path.previous {
                    match path.layer {
                        Some(layer) if eq_token(layer, target_layer) => {
                            let context = RouteContext::new(
                                record, net_str, status, path.shape, layer, path.width,
                            )?;
                            shapes.push(
                                make_net_shape(&context, previous, point)
                                    .ok_or_else(out_of_range)?,
                            );
                        }
                        Some(_) => {}
                        None => skipped.after_via += 1,
                    }
                }
                path.previous = Some(point);
            }
            // Any other bare word after a point is a via: the next
            // coordinates are on the via's other routing layer, which is
            // unknown here, so those segments are skipped and counted.
            _ if path.previous.is_some() => {
                tokens.skip_orientation();
                if tokens.peek() == Some(b"DO") {
                    // DO numX BY numY STEP stepX stepY
                    for _ in 0..7 {
                        tokens.next();
                    }
                }
                path.layer = None;
            }
            _ => {}
        }
    }

    Ok((shapes, skipped))
}

/// The route path being read: its status, layer, width, `SHAPE` and previous
/// point. `layer` is `None` after a via, whose other routing layer is unknown
/// without via definitions.
#[derive(Default)]
struct RoutePath<'a> {
    status: Option<&'static str>,
    layer: Option<&'a [u8]>,
    width: i64,
    shape: &'a str,
    previous: Option<RoutePoint>,
}

impl<'a> RoutePath<'a> {
    /// Start a path (a status keyword with a layer, or `NEW`).
    fn start(&mut self, status: &'static str, layer: Option<&'a [u8]>, width: i64) {
        *self = Self {
            status: Some(status),
            layer,
            width,
            shape: "",
            previous: None,
        };
    }

    /// End the path: nothing is route geometry until the next status.
    fn end(&mut self) {
        *self = Self::default();
    }
}

/// Skip a `+ SUBNET` body up to the next net-level `+`. True when it carries
/// wiring: a status keyword outside its parenthesized pin list.
fn skip_subnet(tokens: &mut ByteTokens<'_>) -> bool {
    let mut depth = 0usize;
    let mut wired = false;
    while let Some(token) = tokens.peek() {
        if token == b";" || (token == b"+" && depth == 0) {
            break;
        }
        tokens.next();
        match token {
            b"(" => depth += 1,
            b")" => depth = depth.saturating_sub(1),
            b"ROUTED" | b"FIXED" | b"COVER" | b"NOSHIELD" if depth == 0 => wired = true,
            _ => {}
        }
    }
    wired
}

/// Read the `( dx1 dy1 dx2 dy2 )` that follows an in-path `RECT`.
fn read_rect_deltas(tokens: &mut ByteTokens<'_>, offset: u64) -> Result<[i64; 4], String> {
    tokens.expect(b"(", offset, "RECT deltas")?;
    let mut deltas = [0; 4];
    for delta in &mut deltas {
        *delta = tokens
            .next_i64(offset, "RECT delta")?
            .ok_or_else(|| format!("expected RECT delta near DEF byte {offset}"))?;
    }
    tokens.expect(b")", offset, "RECT deltas")?;
    Ok(deltas)
}

/// `llx, lly, urx, ury` of the rectangle spanned by two corners.
fn rect_bbox(a: (i64, i64), b: (i64, i64)) -> [i64; 4] {
    [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
}

/// Route pieces a record could not place: segments after a via (the via's
/// other layer is unknown without via definitions), `+ POLYGON` patches and
/// `+ SUBNET` wiring.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkippedRoutes {
    pub after_via: u64,
    pub polygon: u64,
    pub subnet: u64,
}

impl SkippedRoutes {
    pub fn add(&mut self, other: SkippedRoutes) {
        self.after_via += other.after_via;
        self.polygon += other.polygon;
        self.subnet += other.subnet;
    }
}

/// A route coordinate with an optional wire extension value.
#[derive(Clone, Copy)]
pub(crate) struct RoutePoint {
    x: i64,
    y: i64,
    ext: Option<i64>,
}

/// Everything a shape row needs, filled once per segment.
pub(crate) struct RouteContext<'a> {
    net: &'a str,
    section: &'static str,
    status: &'static str,
    shape_type: &'a str,
    layer: &'a str,
    width: i64,
}

impl<'a> RouteContext<'a> {
    fn new(
        record: &NetRecord,
        net: &'a str,
        status: &'static str,
        shape_type: &'a str,
        layer: &'a [u8],
        width: i64,
    ) -> Result<Self, String> {
        Ok(Self {
            net,
            section: record.section,
            status,
            shape_type,
            layer: token_to_str(layer, record.offset, "route layer")?,
            width,
        })
    }
}

/// Read one routing point. Call with the `(` token already consumed; reads
/// `x|*`, `y|*`, an optional `ext|*`, then `)`. A `*` repeats the previous
/// point's coordinate (or extension); the first point of a sequence cannot
/// use one.
pub(crate) fn read_route_point(
    tokens: &mut ByteTokens<'_>,
    previous: Option<RoutePoint>,
    offset: u64,
) -> Result<RoutePoint, String> {
    let read_coord = |tokens: &mut ByteTokens<'_>, prev: Option<i64>| -> Result<i64, String> {
        let token = tokens
            .next()
            .ok_or_else(|| format!("expected route point coordinate near DEF byte {offset}"))?;
        if token == b"*" {
            prev.ok_or_else(|| format!("'*' with no previous point near DEF byte {offset}"))
        } else {
            parse_i64_bytes(token).ok_or_else(|| {
                format!(
                    "invalid route point coordinate: '{}' near DEF byte {offset}",
                    String::from_utf8_lossy(token)
                )
            })
        }
    };
    let x = read_coord(tokens, previous.map(|point| point.x))?;
    let y = read_coord(tokens, previous.map(|point| point.y))?;
    let saved = tokens.pos;
    let ext = match tokens.next() {
        None => None,
        Some(token) if token == b")" => {
            tokens.pos = saved;
            None
        }
        Some(b"*") => previous.and_then(|point| point.ext),
        Some(token) => Some(parse_i64_bytes(token).ok_or_else(|| {
            format!(
                "invalid route point extension: '{}' near DEF byte {offset}",
                String::from_utf8_lossy(token)
            )
        })?),
    };
    tokens.expect(b")", offset, "route point")?;
    Ok(RoutePoint { x, y, ext })
}

/// Connection lists longer than this split into pieces that parse in
/// parallel: a power net that lists every cell is one record of hundreds of
/// MB.
pub(crate) const SPLIT_BYTES: usize = 1 << 20;

pub(crate) fn parse_net_connections_record(record: &NetRecord) -> Result<NetConns<'_>, String> {
    let mut tokens = ByteTokens::new(&record.bytes);
    tokens.expect(b"-", record.offset, "net record")?;
    let net = tokens
        .next()
        .ok_or_else(|| format!("expected net name near DEF byte {}", record.offset))?;
    let net_name = token_to_str(net, record.offset, "net name")?;
    let body = &record.bytes[tokens.pos..];
    let conns = match split_connection_list(body) {
        Some(pieces) => {
            let parsed: Vec<Result<Vec<Conn<'_>>, String>> = crate::threads::install(|| {
                pieces
                    .par_iter()
                    .map(|piece| parse_connection_list(piece, record.offset))
                    .collect()
            });
            let mut conns = Vec::new();
            for piece in parsed {
                conns.extend(piece?);
            }
            conns
        }
        None => parse_connection_list(body, record.offset)?,
    };
    Ok(NetConns {
        name: net_name,
        conns,
    })
}

/// `( instance pin )` / `( PIN port )` pairs up to the first `+` outside
/// parentheses. A connection may carry `+ SYNTHESIZED` before its `)`.
pub(crate) fn parse_connection_list<'a>(
    bytes: &'a [u8],
    offset: u64,
) -> Result<Vec<Conn<'a>>, String> {
    let mut tokens = ByteTokens::new(bytes);
    let mut conns = Vec::new();
    while let Some(token) = tokens.next() {
        if token == b"+" {
            break;
        }
        if token != b"(" {
            continue;
        }
        let Some(first) = tokens.next() else {
            break;
        };
        let Some(second) = tokens.next() else {
            break;
        };
        match tokens.next() {
            Some(b")") => {}
            Some(b"+") => {
                tokens.expect(b"SYNTHESIZED", offset, "net connection")?;
                tokens.expect(b")", offset, "net connection")?;
            }
            Some(other) => {
                return Err(format!(
                    "expected ')' for net connection, got '{}' near DEF byte {offset}",
                    String::from_utf8_lossy(other)
                ));
            }
            None => return Err(format!("expected net connection near DEF byte {offset}")),
        }
        if first == b"PIN" {
            conns.push(Conn::Port(token_to_str(second, offset, "port name")?));
        } else {
            conns.push(Conn::Pin {
                instance: token_to_str(first, offset, "component name")?,
                pin: token_to_str(second, offset, "pin name")?,
            });
        }
    }
    Ok(conns)
}

fn net_shape(context: &RouteContext<'_>, ends: [i64; 4], bbox: [i64; 4]) -> NetShape {
    let [x1, y1, x2, y2] = ends;
    let [llx, lly, urx, ury] = bbox;
    NetShape {
        net: context.net.to_string(),
        section: context.section.to_string(),
        route_status: context.status.to_string(),
        shape_type: context.shape_type.to_string(),
        layer: context.layer.to_string(),
        width: context.width,
        x1,
        y1,
        x2,
        y2,
        llx,
        lly,
        urx,
        ury,
    }
}

/// Offset of the first `+` outside parentheses and quotes, or the length.
fn connection_list_end(body: &[u8]) -> usize {
    let mut depth = 0usize;
    let mut start = 0;
    while start < body.len() {
        // The next quote or `+`; parentheses are counted only in the bytes
        // skipped, so the common case is two memchr hops.
        let rest = &body[start..];
        let plus = memchr::memchr(b'+', rest);
        let quote = memchr::memchr(b'"', &rest[..plus.unwrap_or(rest.len())]);
        let Some(index) = quote.or(plus) else {
            break;
        };
        let skipped = &rest[..index];
        let opens = memchr::memchr_iter(b'(', skipped).count();
        let closes = memchr::memchr_iter(b')', skipped).count();
        depth = (depth + opens).saturating_sub(closes);
        if quote.is_some() {
            // Skip the quoted text; an unterminated quote ends the scan.
            match memchr::memchr(b'"', &rest[index + 1..]) {
                Some(close) => start += index + close + 2,
                None => return body.len(),
            }
        } else if depth == 0 {
            return start + index;
        } else {
            start += index + 1;
        }
    }
    body.len()
}

/// Cut a long connection list into pieces that parse independently, or
/// `None` to parse it whole.
///
/// The list ends at the first `+` outside parentheses. Pieces start at a
/// `(` whose previous non-blank byte is `)`: a parse with no error is
/// between connections there, and a misaligned parse is an error inside the
/// piece before. Lists with quotes are never split, since `(`, `)` and `+`
/// inside a quoted token are not delimiters.
pub(crate) fn split_connection_list(body: &[u8]) -> Option<Vec<&[u8]>> {
    let end = connection_list_end(body);
    let list = &body[..end];
    if list.len() <= SPLIT_BYTES || memchr::memchr(b'"', list).is_some() {
        return None;
    }
    let mut pieces = Vec::with_capacity(list.len() / SPLIT_BYTES + 1);
    let mut start = 0;
    while list.len() - start > SPLIT_BYTES {
        let mut cut = start + SPLIT_BYTES;
        let found = loop {
            match memchr::memchr(b'(', &list[cut..]) {
                None => break None,
                Some(index) => {
                    let at = cut + index;
                    let before = list[..at]
                        .iter()
                        .rposition(|byte| !byte.is_ascii_whitespace());
                    if before.is_some_and(|position| list[position] == b')') {
                        break Some(at);
                    }
                    cut = at + 1;
                }
            }
        };
        let Some(at) = found else {
            break;
        };
        pieces.push(&list[start..at]);
        start = at;
    }
    pieces.push(&list[start..]);
    Some(pieces)
}

fn make_net_shape(
    context: &RouteContext<'_>,
    start: RoutePoint,
    end: RoutePoint,
) -> Option<NetShape> {
    let low = context.width / 2;
    let high = context.width - low;
    let ext = |point: RoutePoint| point.ext.unwrap_or(0);
    let (mut llx, mut urx) = (start.x.min(end.x), start.x.max(end.x));
    let (mut lly, mut ury) = (start.y.min(end.y), start.y.max(end.y));
    let vertical = start.x == end.x;
    let horizontal = start.y == end.y;
    if vertical {
        let (bottom, top) = if start.y <= end.y {
            (start, end)
        } else {
            (end, start)
        };
        lly = lly.checked_sub(ext(bottom))?;
        ury = ury.checked_add(ext(top))?;
    }
    if horizontal {
        let (left, right) = if start.x <= end.x {
            (start, end)
        } else {
            (end, start)
        };
        llx = llx.checked_sub(ext(left))?;
        urx = urx.checked_add(ext(right))?;
    }
    if vertical || !horizontal {
        llx = llx.checked_sub(low)?;
        urx = urx.checked_add(high)?;
    }
    if horizontal || !vertical {
        lly = lly.checked_sub(low)?;
        ury = ury.checked_add(high)?;
    }
    Some(net_shape(
        context,
        [start.x, start.y, end.x, end.y],
        [llx, lly, urx, ury],
    ))
}

/// Components and per-net connections of a DEF.
///
/// Only each net's connection list is kept: the routing after the first
/// `+` is skipped while scanning, never copied. Net records parse in
/// parallel batches as the scan goes. Records of a NETS/SPECIALNETS section
/// met before COMPONENTS are held until the components are known, so
/// `add_component` never follows `add_nets`.
fn collect_def_connectivity(def_path: &Path) -> Result<Connectivity, crate::Error> {
    let mut lexer = Lexer::new(open_input(def_path)?);
    let mut scan = ConnectivityScan::default();

    while let Some(token) = lexer.next_token()? {
        if token.text == "-" {
            // A record of a section this pass does not read.
            lexer.skip_until_semicolon()?;
        } else if token.text.eq_ignore_ascii_case("COMPONENTS") {
            let _count = lexer.next_token()?;
            expect_semicolon(&mut lexer)?;
            if scan.nets_added {
                return Err(second_components_error(token.offset).into());
            }
            collect_component_section(&mut lexer, &mut scan.builder)?;
            scan.components_seen = true;
            scan.release_held()?;
        } else if token.text.eq_ignore_ascii_case("NETS") {
            let _count = lexer.next_token()?;
            expect_semicolon(&mut lexer)?;
            collect_net_connectivity(&mut lexer, "NETS", &mut scan)?;
        } else if token.text.eq_ignore_ascii_case("SPECIALNETS") {
            let _count = lexer.next_token()?;
            expect_semicolon(&mut lexer)?;
            collect_net_connectivity(&mut lexer, "SPECIALNETS", &mut scan)?;
        }
    }
    scan.release_held()?;
    Ok(scan.builder.finish())
}

/// The error for a second COMPONENTS section once nets were resolved; the
/// DEF-backed readers and `write_def_cache` report it identically.
fn second_components_error(offset: u64) -> String {
    format!(
        "second COMPONENTS section near DEF byte {offset}: a DEF has one COMPONENTS section, and nets were already resolved against the first"
    )
}

/// The builder plus the net records waiting for COMPONENTS.
#[derive(Default)]
pub(crate) struct ConnectivityScan {
    builder: ConnectivityBuilder,
    held: Vec<NetRecord>,
    components_seen: bool,
    nets_added: bool,
}

impl ConnectivityScan {
    /// Resolve `records` into the builder (bounded batches stay with the
    /// caller; this takes one).
    fn add_records(&mut self, records: &[NetRecord]) -> Result<(), String> {
        let nets = parse_connectivity_records(records)?;
        self.builder.add_nets(&nets);
        self.nets_added |= !nets.is_empty();
        Ok(())
    }

    /// Resolve a scanned batch now, or hold it until COMPONENTS ends.
    fn take_batch(&mut self, batch: &mut Vec<NetRecord>) -> Result<(), String> {
        if self.components_seen {
            self.add_records(batch)?;
            batch.clear();
        } else {
            self.held.append(batch);
        }
        Ok(())
    }

    /// Resolve the held records, in batches, once COMPONENTS is known.
    fn release_held(&mut self) -> Result<(), String> {
        let mut held = std::mem::take(&mut self.held);
        let mut start = 0;
        while start < held.len() {
            let mut end = start;
            let mut bytes = 0usize;
            while end < held.len() && end - start < BATCH_SIZE && bytes < BATCH_BYTES {
                bytes += held[end].bytes.len();
                end += 1;
            }
            self.add_records(&held[start..end])?;
            start = end;
        }
        held.clear();
        Ok(())
    }
}

pub(crate) fn parse_connectivity_records(
    records: &[NetRecord],
) -> Result<Vec<NetConns<'_>>, String> {
    crate::threads::install(|| {
        records
            .par_iter()
            .map(parse_net_connections_record)
            .collect()
    })
}

fn collect_component_section<R: Read>(
    lexer: &mut Lexer<R>,
    builder: &mut ConnectivityBuilder,
) -> Result<(), crate::Error> {
    let mut bytes = Vec::with_capacity(256);
    while let Some(token) = lexer.next_token()? {
        if token.text.eq_ignore_ascii_case("END") {
            let _section = lexer.next_token()?;
            return Ok(());
        }
        if token.text != "-" {
            continue;
        }
        bytes.clear();
        bytes.push(b'-');
        lexer.read_until_semicolon(&mut bytes)?;
        let (instance, macro_name) = parse_component_identity(&bytes, token.offset)?;
        builder.add_component(&instance, &macro_name);
    }
    Ok(())
}

/// Read one NETS/SPECIALNETS section, keeping each net's connection list.
fn collect_net_connectivity<R: Read>(
    lexer: &mut Lexer<R>,
    section: &'static str,
    scan: &mut ConnectivityScan,
) -> Result<(), crate::Error> {
    let mut batch: Vec<NetRecord> = Vec::new();
    let mut batch_bytes = 0usize;
    while let Some(token) = lexer.next_token()? {
        if token.text.eq_ignore_ascii_case("END") {
            let _section = lexer.next_token()?;
            break;
        }
        if token.text != "-" {
            continue;
        }
        let mut bytes = Vec::with_capacity(128);
        bytes.push(b'-');
        let routed = lexer.read_until_plus_or_semicolon(&mut bytes)?;
        if routed {
            lexer.skip_until_semicolon()?;
        }
        batch_bytes += bytes.len();
        batch.push(NetRecord {
            bytes,
            offset: token.offset,
            section,
        });
        if batch.len() >= BATCH_SIZE || batch_bytes >= BATCH_BYTES {
            scan.take_batch(&mut batch)?;
            batch_bytes = 0;
        }
    }
    Ok(scan.take_batch(&mut batch)?)
}

pub(crate) fn stream_components_to_cache<R: Read, W: Write>(
    lexer: &mut Lexer<R>,
    out: &mut W,
    count: &mut u64,
) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(256);
    while let Some(token) = lexer.next_token().map_err(|e| e.to_string())? {
        if token.text.eq_ignore_ascii_case("END") {
            let _section = lexer.next_token().map_err(|e| e.to_string())?;
            return Ok(());
        }
        if token.text != "-" {
            continue;
        }
        bytes.clear();
        bytes.push(b'-');
        lexer
            .read_until_semicolon(&mut bytes)
            .map_err(|e| e.to_string())?;
        let (instance, macro_name) = parse_component_identity(&bytes, token.offset)?;
        write_string(out, &instance)?;
        write_string(out, &macro_name)?;
        *count += 1;
    }
    Ok(())
}

/// Everything `stream_nets_to_cache` writes, and the running counters.
pub(crate) struct NetCacheWriter<'a, W1: Write, W2: Write, W3: Write> {
    data: &'a mut W1,
    index: &'a mut W2,
    connectivity: &'a mut W3,
    net_map: &'a mut HashMap<String, Vec<u64>>,
    data_offset: &'a mut u64,
    net_count: &'a mut u64,
    connectivity_count: &'a mut u64,
}

impl<W1: Write, W2: Write, W3: Write> NetCacheWriter<'_, W1, W2, W3> {
    /// Parse a batch's connectivity in parallel, then write every record in
    /// order.
    pub fn flush(&mut self, batch: &mut Vec<NetRecord>) -> Result<(), String> {
        if batch.is_empty() {
            return Ok(());
        }
        let infos = parse_connectivity_records(batch)?;
        for (record, net_info) in batch.iter().zip(&infos) {
            let net_name = record_net_name(&record.bytes, record.offset)?;
            self.net_map
                .entry(net_name)
                .or_default()
                .push(*self.net_count);

            write_u8(self.index, section_id(record.section))?;
            write_u64(self.index, record.offset)?;
            write_u64(self.index, *self.data_offset)?;
            write_u64(self.index, record.bytes.len() as u64)?;
            self.data
                .write_all(&record.bytes)
                .map_err(|e| e.to_string())?;
            *self.data_offset += record.bytes.len() as u64;

            write_net_info(self.connectivity, net_info)?;
            *self.net_count += 1;
            *self.connectivity_count += 1;
        }
        drop(infos);
        batch.clear();
        Ok(())
    }
}

pub(crate) fn stream_nets_to_cache<R: Read, W1: Write, W2: Write, W3: Write>(
    lexer: &mut Lexer<R>,
    section: &'static str,
    writer: &mut NetCacheWriter<'_, W1, W2, W3>,
) -> Result<(), String> {
    let mut batch: Vec<NetRecord> = Vec::new();
    let mut batch_bytes = 0usize;
    while let Some(token) = lexer.next_token().map_err(|e| e.to_string())? {
        if token.text.eq_ignore_ascii_case("END") {
            let _section = lexer.next_token().map_err(|e| e.to_string())?;
            break;
        }
        if token.text != "-" {
            continue;
        }
        let mut bytes = Vec::with_capacity(1024);
        bytes.push(b'-');
        lexer
            .read_until_semicolon(&mut bytes)
            .map_err(|e| e.to_string())?;
        batch_bytes += bytes.len();
        batch.push(NetRecord {
            bytes,
            offset: token.offset,
            section,
        });
        if batch.len() >= BATCH_SIZE || batch_bytes >= BATCH_BYTES {
            writer.flush(&mut batch)?;
            batch_bytes = 0;
        }
    }
    writer.flush(&mut batch)
}

pub(crate) fn seed_clock_nets(c: &Connectivity, root_type: &str, root_name: &str) -> Vec<u32> {
    let pin_root = if root_type == "pin" {
        split_pin_name(root_name)
    } else {
        None
    };
    let mut seeds = Vec::new();
    for net in 0..c.nets.len() as u32 {
        if c.nets.get(net) == root_name {
            seeds.push(net);
            continue;
        }
        let hit = c.conns(net).any(|(instance, pin)| match root_type {
            "port" => instance == PORT && c.pins.get(pin) == root_name,
            "pin" => match pin_root {
                Some((inst, pin_name)) => {
                    instance != PORT
                        && c.instances.get(instance) == inst
                        && c.pins.get(pin) == pin_name
                }
                None => false,
            },
            _ => false,
        });
        if hit {
            seeds.push(net);
        }
    }
    seeds
}

/// Instances reached from the seed nets as `(instance, macro, first net,
/// is_clock_cell)`, sorted by instance name. The first net is the first one in
/// breadth-first order that reached the instance.
pub(crate) fn traverse_clock_instances<'a>(
    c: &'a Connectivity,
    adjacency: &Adjacency,
    initial_nets: &[u32],
    patterns: &[String],
) -> Vec<(&'a str, &'a str, &'a str, bool)> {
    // Each instance is expanded once (on its first visit), so the queue holds
    // at most the seeds plus one entry per connection of a clock cell.
    let mut queue: VecDeque<u32> = initial_nets.iter().copied().collect();
    let mut seen_nets = vec![false; c.nets.len()];
    // Per instance id: the first net that reached it and its clock-cell flag.
    let mut seen_instances: Vec<Option<(u32, bool)>> = vec![None; c.instances.len()];

    while let Some(net) = queue.pop_front() {
        if seen_nets[net as usize] {
            continue;
        }
        seen_nets[net as usize] = true;
        for (instance, _) in c.conns(net) {
            if instance == PORT {
                continue;
            }
            let slot = &mut seen_instances[instance as usize];
            if slot.is_some() {
                // Already visited: a clock cell's nets were queued on its
                // first visit, so queueing them again only adds duplicates
                // (this is what keeps the queue O(nets + connections)).
                continue;
            }
            let is_clock = is_clock_cell(c.macro_of(instance).unwrap_or_default(), patterns);
            *slot = Some((net, is_clock));
            if is_clock {
                for (next_net, _) in adjacency.of(instance) {
                    if !seen_nets[next_net as usize] {
                        queue.push_back(next_net);
                    }
                }
            }
        }
    }

    let mut rows: Vec<_> = seen_instances
        .iter()
        .enumerate()
        .filter_map(|(id, seen)| {
            let (net, is_clock) = (*seen)?;
            let id = id as u32;
            Some((
                c.instances.get(id),
                c.macro_of(id).unwrap_or_default(),
                c.nets.get(net),
                is_clock,
            ))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    rows
}

pub(crate) fn split_pin_name(name: &str) -> Option<(&str, &str)> {
    name.rsplit_once('/').or_else(|| name.rsplit_once('.'))
}

pub(crate) fn is_clock_cell(macro_name: &str, patterns: &[String]) -> bool {
    let lower = macro_name.to_ascii_lowercase();
    patterns.iter().any(|pattern| lower.contains(pattern))
}

pub(crate) fn write_manifest(cache_dir: &Path) -> Result<(), String> {
    let file = File::create(cache_dir.join("manifest.bin"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("manifest.bin").display()))?;
    let mut out = BufWriter::new(file);
    write_header(&mut out, MANIFEST_MAGIC, 0)?;
    out.flush().map_err(|e| e.to_string())
}

pub(crate) fn read_manifest(cache_dir: &Path) -> Result<(), String> {
    let file = File::open(cache_dir.join("manifest.bin"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("manifest.bin").display()))?;
    let mut input = BufReader::new(file);
    read_header(&mut input, MANIFEST_MAGIC)?;
    Ok(())
}

pub(crate) fn write_net_map_bin_from_map(
    cache_dir: &Path,
    by_net: &HashMap<String, Vec<u64>>,
) -> Result<(), String> {
    let file = File::create(cache_dir.join("net_index.bin"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("net_index.bin").display()))?;
    let mut out = BufWriter::new(file);
    write_header(&mut out, NET_MAP_MAGIC, by_net.len() as u64)?;
    let mut rows: Vec<_> = by_net.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    for (net, indices) in rows {
        write_string(&mut out, net)?;
        write_u64(&mut out, indices.len() as u64)?;
        for index in indices {
            write_u64(&mut out, *index)?;
        }
    }
    out.flush().map_err(|e| e.to_string())
}

pub(crate) fn load_net_records_for_net(
    cache_dir: &Path,
    net: &str,
) -> Result<Vec<NetRecord>, String> {
    let wanted = load_net_record_indices(cache_dir, net)?;
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let mut index = File::open(cache_dir.join("nets.idx"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("nets.idx").display()))?;
    let count = read_header(&mut index, NET_INDEX_MAGIC)?;
    let mut data = File::open(cache_dir.join("nets.dat"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("nets.dat").display()))?;
    read_header(&mut data, NET_DATA_MAGIC)?;

    let mut records = Vec::with_capacity(wanted.len());
    for record_index in wanted {
        if record_index >= count {
            return Err(format!("cache net index {record_index} out of range"));
        }
        index
            .seek(SeekFrom::Start(
                CACHE_HEADER_LEN + record_index * NET_INDEX_ENTRY_LEN,
            ))
            .map_err(|e| e.to_string())?;
        let section = section_name(read_u8(&mut index)?)?;
        let offset = read_u64(&mut index)?;
        let data_offset = read_u64(&mut index)?;
        let len = read_u64(&mut index)?;
        data.seek(SeekFrom::Start(data_offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![0; len as usize];
        data.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        records.push(NetRecord {
            bytes,
            offset,
            section,
        });
    }
    Ok(records)
}

pub(crate) fn load_net_record_indices(cache_dir: &Path, net: &str) -> Result<Vec<u64>, String> {
    let file = File::open(cache_dir.join("net_index.bin"))
        .map_err(|e| format!("{}: {e}", cache_dir.join("net_index.bin").display()))?;
    let mut input = BufReader::new(file);
    let count = read_header(&mut input, NET_MAP_MAGIC)?;
    for _ in 0..count {
        let name = read_string(&mut input)?;
        let record_count = read_u64(&mut input)?;
        if name.eq_ignore_ascii_case(net) {
            let mut indices = Vec::with_capacity(record_count as usize);
            for _ in 0..record_count {
                indices.push(read_u64(&mut input)?);
            }
            return Ok(indices);
        }
        for _ in 0..record_count {
            let _ = read_u64(&mut input)?;
        }
    }
    Ok(Vec::new())
}

pub(crate) fn write_net_info<W: Write>(out: &mut W, net: &NetConns<'_>) -> Result<(), String> {
    write_string(out, net.name)?;
    write_u64(out, net.conns.len() as u64)?;
    for conn in &net.conns {
        match *conn {
            Conn::Pin { instance, pin } => {
                write_u8(out, 1)?;
                write_string(out, instance)?;
                write_string(out, pin)?;
            }
            Conn::Port(pin) => {
                write_u8(out, 0)?;
                write_string(out, pin)?;
            }
        }
    }
    Ok(())
}

/// Nets read per `add_nets` call when loading the cache.
const LOAD_BATCH_NETS: usize = BATCH_SIZE;

/// One cache batch: every name in one buffer, nets as ranges into it.
#[derive(Default)]
struct LoadArena {
    text: String,
    /// `(name, first conn, one past the last conn)` per net.
    nets: Vec<(Range<usize>, usize, usize)>,
    /// `(instance, pin)`; no instance for a port.
    conns: Vec<(Option<Range<usize>>, Range<usize>)>,
}

impl LoadArena {
    fn clear(&mut self) {
        self.text.clear();
        self.nets.clear();
        self.conns.clear();
    }

    fn push(&mut self, name: &str) -> Range<usize> {
        let start = self.text.len();
        self.text.push_str(name);
        start..self.text.len()
    }

    fn add_all(&self, builder: &mut ConnectivityBuilder) {
        let nets: Vec<NetConns<'_>> = self
            .nets
            .iter()
            .map(|(name, first, last)| NetConns {
                name: &self.text[name.clone()],
                conns: self.conns[*first..*last]
                    .iter()
                    .map(|(instance, pin)| match instance {
                        Some(instance) => Conn::Pin {
                            instance: &self.text[instance.clone()],
                            pin: &self.text[pin.clone()],
                        },
                        None => Conn::Port(&self.text[pin.clone()]),
                    })
                    .collect(),
            })
            .collect();
        builder.add_nets(&nets);
    }
}

/// Components, then nets, from a cache directory. Names are read through
/// one reused buffer; nets reach the builder in bounded batches.
pub(crate) fn load_connectivity(cache_dir: &Path) -> Result<Connectivity, String> {
    let mut builder = ConnectivityBuilder::new();
    let mut buf = String::new();

    let path = cache_dir.join("components.bin");
    let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut input = BufReader::new(file);
    let count = read_header(&mut input, COMPONENTS_MAGIC)?;
    let mut macro_name = String::new();
    for _ in 0..count {
        read_string_into(&mut input, &mut buf)?;
        read_string_into(&mut input, &mut macro_name)?;
        builder.add_component(&buf, &macro_name);
    }

    let path = cache_dir.join("connectivity.bin");
    let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut input = BufReader::new(file);
    let count = read_header(&mut input, CONNECTIVITY_MAGIC)?;
    let mut arena = LoadArena::default();
    for _ in 0..count {
        read_string_into(&mut input, &mut buf)?;
        let name = arena.push(&buf);
        let conn_count = read_u64(&mut input)?;
        let first = arena.conns.len();
        for _ in 0..conn_count {
            let instance = if read_u8(&mut input)? == 1 {
                read_string_into(&mut input, &mut buf)?;
                Some(arena.push(&buf))
            } else {
                None
            };
            read_string_into(&mut input, &mut buf)?;
            let pin = arena.push(&buf);
            arena.conns.push((instance, pin));
        }
        arena.nets.push((name, first, arena.conns.len()));
        if arena.nets.len() >= LOAD_BATCH_NETS || arena.text.len() >= BATCH_BYTES {
            arena.add_all(&mut builder);
            arena.clear();
        }
    }
    arena.add_all(&mut builder);
    Ok(builder.finish())
}

pub(crate) fn write_header<W: Write>(
    out: &mut W,
    magic: &[u8; 8],
    count: u64,
) -> Result<(), String> {
    out.write_all(magic).map_err(|e| e.to_string())?;
    write_u32(out, CACHE_VERSION)?;
    write_u64(out, count)
}

pub(crate) fn read_header<R: Read>(input: &mut R, magic: &[u8; 8]) -> Result<u64, String> {
    let mut got = [0u8; 8];
    input.read_exact(&mut got).map_err(|e| e.to_string())?;
    if &got != magic {
        return Err("invalid cache file magic".to_string());
    }
    let version = read_u32(input)?;
    if version != CACHE_VERSION {
        return Err(format!("unsupported cache version {version}"));
    }
    read_u64(input)
}

pub(crate) fn patch_header_count(path: &Path, count: u64) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.seek(SeekFrom::Start(12)).map_err(|e| e.to_string())?;
    file.write_all(&count.to_le_bytes())
        .map_err(|e| e.to_string())
}

pub(crate) fn write_string<W: Write>(out: &mut W, value: &str) -> Result<(), String> {
    write_u64(out, value.len() as u64)?;
    out.write_all(value.as_bytes()).map_err(|e| e.to_string())
}

pub(crate) fn read_string<R: Read>(input: &mut R) -> Result<String, String> {
    let len = read_u64(input)?;
    let mut bytes = vec![0; len as usize];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Read a length-prefixed string into `buf`, replacing its contents.
pub(crate) fn read_string_into<R: Read>(input: &mut R, buf: &mut String) -> Result<(), String> {
    let len = read_u64(input)?;
    let mut bytes = std::mem::take(buf).into_bytes();
    bytes.clear();
    // `read_to_end` grows as bytes arrive, so a corrupt length fails at EOF
    // instead of touching `len` bytes first.
    input
        .by_ref()
        .take(len)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != len {
        return Err("failed to fill whole buffer".to_string());
    }
    *buf = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn write_u8<W: Write>(out: &mut W, value: u8) -> Result<(), String> {
    out.write_all(&[value]).map_err(|e| e.to_string())
}

pub(crate) fn read_u8<R: Read>(input: &mut R) -> Result<u8, String> {
    let mut bytes = [0u8; 1];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes[0])
}

pub(crate) fn write_u32<W: Write>(out: &mut W, value: u32) -> Result<(), String> {
    out.write_all(&value.to_le_bytes())
        .map_err(|e| e.to_string())
}

pub(crate) fn read_u32<R: Read>(input: &mut R) -> Result<u32, String> {
    let mut bytes = [0u8; 4];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(u32::from_le_bytes(bytes))
}

pub(crate) fn write_u64<W: Write>(out: &mut W, value: u64) -> Result<(), String> {
    out.write_all(&value.to_le_bytes())
        .map_err(|e| e.to_string())
}

pub(crate) fn read_u64<R: Read>(input: &mut R) -> Result<u64, String> {
    let mut bytes = [0u8; 8];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(u64::from_le_bytes(bytes))
}

pub(crate) fn section_id(section: &str) -> u8 {
    if section == "SPECIALNETS" { 2 } else { 1 }
}

pub(crate) fn section_name(id: u8) -> Result<&'static str, String> {
    match id {
        1 => Ok("NETS"),
        2 => Ok("SPECIALNETS"),
        _ => Err(format!("invalid cached net section id {id}")),
    }
}

pub(crate) fn record_net_name(bytes: &[u8], offset: u64) -> Result<String, String> {
    let mut tokens = ByteTokens::new(bytes);
    tokens.expect(b"-", offset, "net record")?;
    let net = tokens
        .next()
        .ok_or_else(|| format!("expected net name near DEF byte {offset}"))?;
    Ok(token_to_str(net, offset, "net name")?.to_string())
}

/// Parse cell LEF into the shared micron model, then convert macro size and
/// origin into the DEF's database units for instance bounding boxes. Pin and
/// port geometry is not materialized on this path.
#[doc(hidden)]
pub fn parse_lef_files(
    paths: &[PathBuf],
    dbu_per_micron: i64,
) -> Result<(MacroMap, Vec<String>), String> {
    load_lef_macros(paths, dbu_per_micron).map_err(|err| err.to_string())
}

/// `parse_lef_files` with the failure kept typed: an unreadable LEF stays an
/// `io::Error` so the public API can report its `ErrorKind`.
fn load_lef_macros(
    paths: &[PathBuf],
    dbu_per_micron: i64,
) -> Result<(MacroMap, Vec<String>), crate::Error> {
    let library = crate::lef::parse_library(paths, None, false)?;
    let macros = macro_sizes_in_dbu(&library, dbu_per_micron)?;
    Ok((macros, library.duplicates))
}

fn macro_sizes_in_dbu(
    library: &crate::lef::LibraryData,
    dbu_per_micron: i64,
) -> Result<MacroMap, crate::Error> {
    let mut macros = HashMap::with_capacity(library.macros().len());
    for item in library.macros() {
        macros.insert(
            item.name.clone(),
            MacroDef {
                width: to_dbu(item.width, dbu_per_micron, &item.name, "SIZE width")?,
                height: to_dbu(item.height, dbu_per_micron, &item.name, "SIZE height")?,
                _origin_x: to_dbu(item.origin_x, dbu_per_micron, &item.name, "ORIGIN x")?,
                _origin_y: to_dbu(item.origin_y, dbu_per_micron, &item.name, "ORIGIN y")?,
            },
        );
    }
    Ok(macros)
}

pub(crate) fn to_dbu(
    microns: f64,
    dbu_per_micron: i64,
    macro_name: &str,
    context: &str,
) -> Result<i64, String> {
    let scaled = microns * dbu_per_micron as f64;
    if !scaled.is_finite() || scaled < i64::MIN as f64 || scaled > i64::MAX as f64 {
        return Err(format!(
            "MACRO {macro_name} {context} out of range: '{microns}'"
        ));
    }
    Ok(scaled.round() as i64)
}

/// Reads the DEF header up to and including the `COMPONENTS` keyword. Returns
/// the `UNITS DISTANCE MICRONS` value (if any) and whether `COMPONENTS` was
/// found; the lexer is left just after that keyword. A `-` record of an
/// earlier section (VIAS, NONDEFAULTRULES, ...) is skipped whole, and every
/// other token is stepped over one at a time, so a section end such as
/// `END VIAS` (which has no `;`) never swallows the statement after it.
fn read_def_header(lexer: &mut Lexer<Box<dyn Read + Send>>) -> io::Result<(Option<i64>, bool)> {
    let mut dbu_per_micron = None;
    while let Some(token) = lexer.next_token()? {
        if token.text == "-" {
            lexer.skip_until_semicolon()?;
        } else if token.text.eq_ignore_ascii_case("UNITS") {
            parse_def_units(lexer, &mut dbu_per_micron)?;
        } else if token.text.eq_ignore_ascii_case("COMPONENTS") {
            return Ok((dbu_per_micron, true));
        }
    }
    Ok((dbu_per_micron, false))
}

#[doc(hidden)]
pub struct DefScanner {
    lexer: Lexer<Box<dyn Read + Send>>,
    pub dbu_per_micron: i64,
    done: bool,
}

impl DefScanner {
    pub fn from_path(path: &Path) -> io::Result<Self> {
        let input = open_input(path)?;
        let mut lexer = Lexer::new(input);
        let (dbu_per_micron, found_components) = read_def_header(&mut lexer)?;
        if found_components {
            let _count = lexer.next_token()?;
            expect_semicolon(&mut lexer)?;
            return Ok(Self {
                lexer,
                dbu_per_micron: dbu_per_micron.unwrap_or(1000),
                done: false,
            });
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "DEF missing COMPONENTS section",
        ))
    }

    pub fn next_batch(&mut self, max_items: usize) -> io::Result<Vec<RawRecord>> {
        let mut items = Vec::with_capacity(max_items);
        while items.len() < max_items && !self.done {
            match self.next_record()? {
                Some(item) => items.push(item),
                None => self.done = true,
            }
        }
        Ok(items)
    }

    pub fn next_record(&mut self) -> io::Result<Option<RawRecord>> {
        loop {
            let token = match self.lexer.next_token()? {
                Some(token) => token,
                None => return Ok(None),
            };
            if token.text.eq_ignore_ascii_case("END") {
                let section = self.lexer.next_token()?;
                if section
                    .as_ref()
                    .is_some_and(|t| t.text.eq_ignore_ascii_case("COMPONENTS"))
                {
                    return Ok(None);
                }
                continue;
            }
            if token.text != "-" {
                continue;
            }
            let offset = token.offset;
            let mut bytes = Vec::with_capacity(96);
            bytes.push(b'-');
            self.lexer.read_until_semicolon(&mut bytes)?;
            return Ok(Some(RawRecord { bytes, offset }));
        }
    }
}

pub(crate) struct NetRecordScanner {
    lexer: Lexer<Box<dyn Read + Send>>,
    section: Option<&'static str>,
    done: bool,
}

impl NetRecordScanner {
    pub fn from_path(path: &Path) -> io::Result<Self> {
        Ok(Self {
            lexer: Lexer::new(open_input(path)?),
            section: None,
            done: false,
        })
    }
    pub fn next_matching_batch(
        &mut self,
        max_items: usize,
        net: &[u8],
    ) -> io::Result<Vec<NetRecord>> {
        let mut items = Vec::with_capacity(16.min(max_items));
        while items.len() < max_items && !self.done {
            match self.next_matching_record(net)? {
                Some(item) => items.push(item),
                None => self.done = true,
            }
        }
        Ok(items)
    }

    /// The next NETS/SPECIALNETS record whose name is `net`.
    ///
    /// Records are skipped at memchr speed without being copied: those of
    /// other sections entirely, net records once their name does not match.
    pub fn next_matching_record(&mut self, net: &[u8]) -> io::Result<Option<NetRecord>> {
        loop {
            let token = match self.lexer.next_token()? {
                Some(token) => token,
                None => return Ok(None),
            };
            if token.text == "-" {
                let Some(section) = self.section else {
                    self.lexer.skip_until_semicolon()?;
                    continue;
                };
                let Some(name) = self.lexer.next_token()? else {
                    return Ok(None);
                };
                if name.text == ";" {
                    continue;
                }
                if !eq_token(name.text.as_bytes(), net) {
                    self.lexer.skip_until_semicolon()?;
                    continue;
                }
                let mut bytes = Vec::with_capacity(1024);
                bytes.extend_from_slice(b"- ");
                let plain = !name.text.bytes().any(|byte| {
                    byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b'+' | b';' | b'"')
                });
                if plain {
                    bytes.extend_from_slice(name.text.as_bytes());
                } else {
                    bytes.push(b'"');
                    bytes.extend_from_slice(name.text.as_bytes());
                    bytes.push(b'"');
                }
                bytes.push(b' ');
                self.lexer.read_until_semicolon(&mut bytes)?;
                return Ok(Some(NetRecord {
                    bytes,
                    offset: token.offset,
                    section,
                }));
            }

            if token.text.eq_ignore_ascii_case("SPECIALNETS") {
                let _count = self.lexer.next_token()?;
                expect_semicolon(&mut self.lexer)?;
                self.section = Some("SPECIALNETS");
                continue;
            }
            if token.text.eq_ignore_ascii_case("NETS") {
                let _count = self.lexer.next_token()?;
                expect_semicolon(&mut self.lexer)?;
                self.section = Some("NETS");
                continue;
            }
            if token.text.eq_ignore_ascii_case("END") {
                let section = self.lexer.next_token()?;
                if section
                    .as_ref()
                    .is_some_and(|t| t.text.eq_ignore_ascii_case("SPECIALNETS"))
                    && self.section == Some("SPECIALNETS")
                {
                    self.section = None;
                    continue;
                }
                if section
                    .as_ref()
                    .is_some_and(|t| t.text.eq_ignore_ascii_case("NETS"))
                    && self.section == Some("NETS")
                {
                    self.section = None;
                    continue;
                }
            }
        }
    }
}

pub(crate) fn parse_def_units<R: Read>(
    lexer: &mut Lexer<R>,
    dbu_per_micron: &mut Option<i64>,
) -> io::Result<()> {
    let distance = lexer.next_token()?;
    let microns = lexer.next_token()?;
    let value = lexer.next_token()?;
    if distance
        .as_ref()
        .is_some_and(|t| t.text.eq_ignore_ascii_case("DISTANCE"))
        && microns
            .as_ref()
            .is_some_and(|t| t.text.eq_ignore_ascii_case("MICRONS"))
    {
        if let Some(token) = value {
            *dbu_per_micron = Some(token.text.parse::<i64>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid DEF UNITS value")
            })?);
        }
    }
    consume_until_semicolon_io(lexer)
}

pub(crate) fn expect_semicolon<R: Read>(lexer: &mut Lexer<R>) -> io::Result<()> {
    expect_text(lexer, ";")
}

pub(crate) fn expect_text<R: Read>(lexer: &mut Lexer<R>, expected: &str) -> io::Result<()> {
    let token = lexer.next_token()?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("expected '{expected}'"),
        )
    })?;
    if token.text == expected {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected '{expected}', got '{}'", token.text),
        ))
    }
}

pub(crate) fn consume_until_semicolon_io<R: Read>(lexer: &mut Lexer<R>) -> io::Result<()> {
    while let Some(token) = lexer.next_token()? {
        if token.text == ";" {
            break;
        }
    }
    Ok(())
}

pub(crate) struct ByteTokens<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ByteTokens<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// The next token, or `None` at end of input. Named `next` to match the
    /// pre-core API; it is a tokenizer cursor, not `Iterator::next`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<&'a [u8]> {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        if self.pos >= self.bytes.len() {
            return None;
        }
        let start = self.pos;
        let byte = self.bytes[self.pos];
        if matches!(byte, b'(' | b')' | b'+' | b';') {
            self.pos += 1;
            return Some(&self.bytes[start..self.pos]);
        }
        if byte == b'-' {
            let next = self.bytes.get(self.pos + 1).copied();
            if !next.is_some_and(|value| value.is_ascii_digit() || value == b'.') {
                self.pos += 1;
                return Some(&self.bytes[start..self.pos]);
            }
        }
        if byte == b'"' {
            self.pos += 1;
            let value_start = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
                self.pos += 1;
            }
            let value_end = self.pos;
            if self.pos < self.bytes.len() {
                self.pos += 1;
            }
            return Some(&self.bytes[value_start..value_end]);
        }
        self.pos += 1;
        while self.pos < self.bytes.len()
            && !self.bytes[self.pos].is_ascii_whitespace()
            && !matches!(self.bytes[self.pos], b'(' | b')' | b'+' | b';')
        {
            self.pos += 1;
        }
        Some(&self.bytes[start..self.pos])
    }

    pub fn expect(&mut self, expected: &[u8], offset: u64, context: &str) -> Result<(), String> {
        let token = self
            .next()
            .ok_or_else(|| format!("expected {context} near DEF byte {offset}"))?;
        if token == expected {
            Ok(())
        } else {
            Err(format!(
                "expected '{}' for {context}, got '{}' near DEF byte {offset}",
                String::from_utf8_lossy(expected),
                String::from_utf8_lossy(token)
            ))
        }
    }

    pub fn next_i64(&mut self, offset: u64, context: &str) -> Result<Option<i64>, String> {
        let Some(token) = self.next() else {
            return Ok(None);
        };
        parse_i64_bytes(token)
            .ok_or_else(|| {
                format!(
                    "invalid {context}: '{}' near DEF byte {offset}",
                    String::from_utf8_lossy(token)
                )
            })
            .map(Some)
    }

    pub fn next_i64_or_rewind(&mut self) -> Option<i64> {
        let original = self.pos;
        let token = self.next()?;
        match parse_i64_bytes(token) {
            Some(value) => Some(value),
            None => {
                self.pos = original;
                None
            }
        }
    }

    pub fn skip_until_close(&mut self) {
        while let Some(token) = self.next() {
            if token == b")" {
                break;
            }
        }
    }

    /// The next token, not consumed.
    pub fn peek(&self) -> Option<&'a [u8]> {
        ByteTokens {
            bytes: self.bytes,
            pos: self.pos,
        }
        .next()
    }

    /// Consume an orientation (`N`, `FS`, ...) when one is next.
    pub fn skip_orientation(&mut self) {
        if self
            .peek()
            .is_some_and(|token| orientation(token).is_some())
        {
            self.next();
        }
    }

    /// Consume consecutive parenthesized groups, such as the points of a
    /// `+ POLYGON` or `+ VIA`.
    pub fn skip_point_groups(&mut self) {
        while self.peek() == Some(b"(") {
            self.next();
            self.skip_until_close();
        }
    }
}

pub(crate) fn eq_token(left: &[u8], right: &[u8]) -> bool {
    left.eq_ignore_ascii_case(right)
}

pub(crate) fn parse_i64_bytes(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() {
        return None;
    }
    let mut index = 0;
    let mut sign = 1i64;
    if bytes[0] == b'-' {
        sign = -1;
        index = 1;
    }
    if index == bytes.len() {
        return None;
    }
    let mut value = 0i64;
    while index < bytes.len() {
        let byte = bytes[index];
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as i64)?;
        index += 1;
    }
    value.checked_mul(sign)
}

pub(crate) fn token_to_str<'a>(
    token: &'a [u8],
    offset: u64,
    context: &str,
) -> Result<&'a str, String> {
    std::str::from_utf8(token)
        .map_err(|_| format!("invalid UTF-8 in {context} near DEF byte {offset}"))
}

pub(crate) fn placement_status(token: &[u8]) -> &'static str {
    match token {
        b"FIXED" => "FIXED",
        b"COVER" => "COVER",
        _ => "PLACED",
    }
}

pub(crate) fn route_status(token: &[u8]) -> &'static str {
    match token {
        b"FIXED" => "FIXED",
        b"COVER" => "COVER",
        b"NOSHIELD" => "NOSHIELD",
        _ => "ROUTED",
    }
}

pub(crate) fn orientation(token: &[u8]) -> Option<&'static str> {
    match token {
        b"N" => Some("N"),
        b"S" => Some("S"),
        b"E" => Some("E"),
        b"W" => Some("W"),
        b"FN" => Some("FN"),
        b"FS" => Some("FS"),
        b"FE" => Some("FE"),
        b"FW" => Some("FW"),
        _ => None,
    }
}

pub(crate) fn append_net_shape_csv_row(out: &mut String, shape: &NetShape) {
    append_csv_field(out, &shape.net);
    out.push(',');
    append_csv_field(out, &shape.section);
    out.push(',');
    append_csv_field(out, &shape.route_status);
    out.push(',');
    append_csv_field(out, &shape.shape_type);
    out.push(',');
    append_csv_field(out, &shape.layer);
    let _ = writeln!(
        out,
        ",{},{},{},{},{},{},{},{},{}",
        shape.width,
        shape.x1,
        shape.y1,
        shape.x2,
        shape.y2,
        shape.llx,
        shape.lly,
        shape.urx,
        shape.ury
    );
}

pub(crate) fn append_csv_field(out: &mut String, value: &str) {
    if value
        .bytes()
        .any(|byte| matches!(byte, b',' | b'"' | b'\n' | b'\r'))
    {
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

/// Write a micron value the way Python's `str(float)` does: shortest
/// round-trip, but keep the `.0` on integral values.
pub(crate) fn append_csv_micron(out: &mut String, value: f64) {
    let start = out.len();
    let _ = write!(out, "{value}");
    if !out[start..].contains('.') {
        out.push_str(".0");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn parse_one_macro(lef: &str, dbu: i64) -> MacroMap {
        let path = tempfile_path("def_tools_macro_test.lef");
        std::fs::write(&path, lef).unwrap();
        let result = parse_lef_files(std::slice::from_ref(&path), dbu);
        std::fs::remove_file(&path).unwrap();
        result.unwrap().0
    }

    #[test]
    pub fn parses_lef_macro_size_in_def_dbu() {
        let macros = parse_one_macro(
            "MACRO INV\n  ORIGIN 0 0 ;\n  SIZE 1.2 BY 2.4 ;\nEND INV\n",
            1000,
        );
        let inv = macros.get("INV").unwrap();
        assert_eq!(inv.width, 1200);
        assert_eq!(inv.height, 2400);
    }

    #[test]
    pub fn parses_negative_macro_origin() {
        let macros = parse_one_macro(
            "MACRO PAD\n  ORIGIN -0.1 -2.5 ;\n  SIZE 10 BY 20 ;\nEND PAD\n",
            1000,
        );
        let pad = macros.get("PAD").unwrap();
        assert_eq!(pad._origin_x, -100);
        assert_eq!(pad._origin_y, -2500);
    }

    #[test]
    pub fn computes_rotated_bbox() {
        let macros = parse_one_macro("MACRO INV\nSIZE 1.2 BY 2.4 ;\nEND INV\n", 1000);
        let raw = RawRecord {
            bytes: b"- u1 INV + PLACED ( 10 20 ) E".to_vec(),
            offset: 0,
        };
        let bbox = parse_component_record(&raw, &macros, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            (bbox.llx, bbox.lly, bbox.urx, bbox.ury),
            (10, 20, 2410, 1220)
        );
    }

    #[test]
    pub fn parses_def_component_tail() {
        let def = "VERSION 5.8 ;\nUNITS DISTANCE MICRONS 1000 ;\nCOMPONENTS 1 ;\n- u1 INV + PLACED ( 10 20 ) FS ;\nEND COMPONENTS\n";
        let tmp = tempfile_path("def_tools_test.def");
        std::fs::write(&tmp, def).unwrap();
        let mut scanner = DefScanner::from_path(&tmp).unwrap();
        let item = scanner.next_record().unwrap().unwrap();
        let bbox = parse_component_record(
            &item,
            &parse_one_macro("MACRO INV\nSIZE 1.2 BY 2.4 ;\nEND INV\n", 1000),
            None,
        )
        .unwrap()
        .unwrap();
        std::fs::remove_file(&tmp).unwrap();
        assert_eq!(bbox.instance, "u1");
        assert_eq!(bbox.macro_name, "INV");
        assert_eq!(bbox.status, "PLACED");
        assert_eq!(bbox.orient, "FS");
    }

    #[test]
    pub fn reads_gzipped_def() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let def = b"VERSION 5.8 ;\nUNITS DISTANCE MICRONS 1000 ;\nCOMPONENTS 1 ;\n- u-1 INV + PLACED ( 10 20 ) N ;\nEND COMPONENTS\n";
        let tmp = tempfile_path("def_tools_test.def.gz");
        let file = File::create(&tmp).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::fast());
        encoder.write_all(def).unwrap();
        encoder.finish().unwrap();

        let mut scanner = DefScanner::from_path(&tmp).unwrap();
        let item = scanner.next_record().unwrap().unwrap();
        let macros = parse_one_macro("MACRO INV\nSIZE 1.2 BY 2.4 ;\nEND INV\n", 1000);
        let bbox = parse_component_record(&item, &macros, None)
            .unwrap()
            .unwrap();
        std::fs::remove_file(&tmp).unwrap();
        assert_eq!(bbox.instance, "u-1");
        assert_eq!(bbox.macro_name, "INV");
    }

    pub fn tempfile_path(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "{}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            name
        ))
    }
}

// ---------------------------------------------------------------------------
// Public Rust streaming API.
// ---------------------------------------------------------------------------

/// Streaming DEF `COMPONENTS` -> instance bounding boxes in DEF database
/// units. Each box starts at the DEF placement point and uses the LEF
/// `SIZE`, the component orientation, and the design's unit scale.
/// Errors surface as items and end the scan: the first `Err` is the
/// last item, so a consumer never silently skips past a gap. Rows returned
/// before the error are unaffected; the error replaces the remaining rows of
/// its internal batch (up to 65,536 records).
pub struct InstanceBBoxIter {
    scanner: DefScanner,
    macros: MacroMap,
    pending: VecDeque<InstanceBBox>,
    done: bool,
}

impl std::fmt::Debug for InstanceBBoxIter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceBBoxIter")
            .field("pending", &self.pending.len())
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

impl InstanceBBoxIter {
    /// Open a DEF and read the macro dimensions from the supplied LEF paths.
    pub fn from_path(def: &Path, lefs: &[PathBuf]) -> Result<Self, crate::Error> {
        let scanner = DefScanner::from_path(def)?;
        let (macros, _duplicates) = load_lef_macros(lefs, scanner.dbu_per_micron)?;
        Ok(Self::with_macros(scanner, macros))
    }

    /// Open a DEF and convert the parsed library's macro dimensions to that
    /// design's database units. The iterator owns the converted dimensions.
    pub fn from_library(
        def: &Path,
        library: &crate::lef::LibraryData,
    ) -> Result<Self, crate::Error> {
        let scanner = DefScanner::from_path(def)?;
        let macros = macro_sizes_in_dbu(library, scanner.dbu_per_micron)?;
        Ok(Self::with_macros(scanner, macros))
    }

    fn with_macros(scanner: DefScanner, macros: MacroMap) -> Self {
        Self {
            scanner,
            macros,
            pending: VecDeque::new(),
            done: false,
        }
    }
}

impl Iterator for InstanceBBoxIter {
    type Item = Result<InstanceBBox, crate::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                return Some(Ok(item));
            }
            if self.done {
                return None;
            }
            let batch = match self.scanner.next_batch(BATCH_SIZE) {
                Ok(batch) => batch,
                Err(err) => {
                    self.done = true;
                    return Some(Err(err.into()));
                }
            };
            if batch.is_empty() {
                self.done = true;
                return None;
            }
            match process_record_batch(&batch, &self.macros) {
                Ok(converted) => self.pending = converted.into(),
                Err(err) => {
                    self.done = true;
                    return Some(Err(err.into()));
                }
            }
        }
    }
}

/// Scan every placed component of `def` with the macro sizes from `lefs`.
pub fn scan_instance_bboxes(
    def: &Path,
    lefs: &[PathBuf],
) -> Result<InstanceBBoxIter, crate::Error> {
    InstanceBBoxIter::from_path(def, lefs)
}

/// Scan placed components using a library from [`crate::lef::parse_cell_lef`].
/// The same library can be reused across DEF files with different unit scales.
pub fn scan_instance_bboxes_with_library(
    def: &Path,
    library: &crate::lef::LibraryData,
) -> Result<InstanceBBoxIter, crate::Error> {
    InstanceBBoxIter::from_library(def, library)
}

/// The DEF's `UNITS DISTANCE MICRONS` value (default 1000): the scale
/// consumers need to convert the DBU integer bbox fields to microns. Reads up
/// to the `COMPONENTS` section (the whole file when there is none); a DEF with
/// no `UNITS` line yields 1000, and one without a `COMPONENTS` section is fine
/// here.
pub fn dbu_per_micron(def: &Path) -> Result<i64, crate::Error> {
    let input = open_input(def)?;
    let mut lexer = Lexer::new(input);
    let (dbu_per_micron, _) = read_def_header(&mut lexer)?;
    Ok(dbu_per_micron.unwrap_or(1000))
}

/// Streaming routed shapes of one net on one layer, in DEF database units.
/// `skipped()` reports the route pieces the parser could not place on a
/// layer (segments after a via, `+ POLYGON`, `+ SUBNET`).
pub struct NetShapeIter {
    scanner: NetRecordScanner,
    net: Vec<u8>,
    layer: Vec<u8>,
    pending: VecDeque<NetShape>,
    skipped: SkippedRoutes,
    done: bool,
}

impl std::fmt::Debug for NetShapeIter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetShapeIter")
            .field("net", &String::from_utf8_lossy(&self.net))
            .field("layer", &String::from_utf8_lossy(&self.layer))
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

impl NetShapeIter {
    pub fn from_path(def: &Path, net: &str, layer: &str) -> Result<Self, crate::Error> {
        Ok(Self {
            scanner: NetRecordScanner::from_path(def)?,
            net: net.as_bytes().to_vec(),
            layer: layer.as_bytes().to_vec(),
            pending: VecDeque::new(),
            skipped: SkippedRoutes::default(),
            done: false,
        })
    }

    /// Route pieces skipped so far.
    pub fn skipped(&self) -> SkippedRoutes {
        self.skipped
    }
}

impl Iterator for NetShapeIter {
    type Item = Result<NetShape, crate::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                return Some(Ok(item));
            }
            if self.done {
                return None;
            }
            let batch = match self.scanner.next_matching_batch(BATCH_SIZE, &self.net) {
                Ok(batch) => batch,
                Err(err) => {
                    self.done = true;
                    return Some(Err(err.into()));
                }
            };
            if batch.is_empty() {
                self.done = true;
                return None;
            }
            match process_net_record_batch(&batch, &self.net, &self.layer) {
                Ok((shapes, skipped)) => {
                    self.skipped.add(skipped);
                    self.pending = shapes.into();
                }
                Err(err) => {
                    self.done = true;
                    return Some(Err(err.into()));
                }
            }
        }
    }
}

/// Scan the routed shapes of `net` on `layer`.
pub fn scan_net_shapes(def: &Path, net: &str, layer: &str) -> Result<NetShapeIter, crate::Error> {
    NetShapeIter::from_path(def, net, layer)
}
