//! Flat name tables and CSR arrays for DEF connectivity.
//!
//! A design with millions of instances has tens of millions of connections.
//! Storing each as `(String, String)` costs one allocation per name; here
//! every distinct name lives once in a [`NameTable`] and a connection is two
//! `u32` ids in compressed-sparse-row arrays.
//!
//! Instance ids: `0..component_count` follow COMPONENTS order (a duplicate
//! name keeps both entries; lookups resolve to the last definition). Names
//! seen only in nets get the following ids in first-seen order. [`PORT`]
//! marks a `( PIN name )` connection.

use rayon::prelude::*;
use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};

use hashbrown::HashTable;

/// Instance id of a `( PIN name )` connection.
pub(crate) const PORT: u32 = u32::MAX;

/// Nets resolved per parallel task. Fixed, so ids never depend on thread count.
const CHUNK: usize = 2048;

/// Names stored back to back; an id is the push order.
#[derive(Default, Debug)]
pub(crate) struct NameTable {
    text: String,
    ends: Vec<usize>,
}

impl NameTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Append `name` and return its id. Never deduplicates.
    pub(crate) fn push(&mut self, name: &str) -> u32 {
        let id = u32::try_from(self.ends.len()).expect("name table exceeds u32 ids");
        self.text.push_str(name);
        self.ends.push(self.text.len());
        id
    }

    pub(crate) fn get(&self, id: u32) -> &str {
        let id = id as usize;
        let start = if id == 0 { 0 } else { self.ends[id - 1] };
        &self.text[start..self.ends[id]]
    }

    pub(crate) fn len(&self) -> usize {
        self.ends.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// Names in id order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &str> {
        (0..self.ends.len() as u32).map(|id| self.get(id))
    }
}

/// One connection of a net.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Conn<'a> {
    Port(&'a str),
    Pin { instance: &'a str, pin: &'a str },
}

/// A net record as the scanner yields it.
#[derive(Debug)]
pub(crate) struct NetConns<'a> {
    pub name: &'a str,
    pub conns: Vec<Conn<'a>>,
}

#[derive(Debug)]
pub(crate) struct Connectivity {
    pub instances: NameTable,
    pub component_count: u32,
    /// Distinct macro names.
    pub macros: NameTable,
    /// Per component, an index into `macros`.
    pub instance_macro: Vec<u32>,
    /// Distinct pin and port names.
    pub pins: NameTable,
    /// One entry per net record, file order.
    pub nets: NameTable,
    /// `conn_start[n]..conn_start[n + 1]` are net `n`'s connections.
    pub conn_start: Vec<usize>,
    /// Instance id or [`PORT`].
    pub conn_instance: Vec<u32>,
    /// Index into `pins`.
    pub conn_pin: Vec<u32>,
}

impl Connectivity {
    /// `(instance or PORT, pin)` of each connection of `net`.
    pub(crate) fn conns(&self, net: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
        let range = self.conn_start[net as usize]..self.conn_start[net as usize + 1];
        range.map(|i| (self.conn_instance[i], self.conn_pin[i]))
    }

    /// The macro of a component; `None` for an instance seen only in nets.
    pub(crate) fn macro_of(&self, instance: u32) -> Option<&str> {
        (instance < self.component_count)
            .then(|| self.macros.get(self.instance_macro[instance as usize]))
    }

    // Used by the clock trace and instance scans in Tasks 4-6.
    #[allow(dead_code)]
    /// Component name to id; the last definition of a duplicate wins.
    pub(crate) fn component_lookup(&self) -> HashMap<&str, u32> {
        (0..self.component_count)
            .map(|id| (self.instances.get(id), id))
            .collect()
    }

    // Used by the clock trace and instance scans in Tasks 4-6.
    #[allow(dead_code)]
    /// Per-instance `(net, pin)` lists in net order, then connection order.
    pub(crate) fn adjacency(&self) -> Adjacency {
        let count = self.instances.len();
        let mut start = vec![0usize; count + 1];
        for &instance in &self.conn_instance {
            if instance != PORT {
                start[instance as usize + 1] += 1;
            }
        }
        for i in 0..count {
            start[i + 1] += start[i];
        }
        let total = start[count];
        let mut net = vec![0u32; total];
        let mut pin = vec![0u32; total];
        let mut next = start.clone();
        for n in 0..self.nets.len() {
            for i in self.conn_start[n]..self.conn_start[n + 1] {
                let instance = self.conn_instance[i];
                if instance != PORT {
                    let slot = next[instance as usize];
                    next[instance as usize] += 1;
                    net[slot] = n as u32;
                    pin[slot] = self.conn_pin[i];
                }
            }
        }
        Adjacency { start, net, pin }
    }
}

// Used by the clock trace and instance scans in Tasks 4-6.
#[allow(dead_code)]
/// Connections grouped by instance (CSR).
#[derive(Debug)]
pub(crate) struct Adjacency {
    start: Vec<usize>,
    net: Vec<u32>,
    pin: Vec<u32>,
}

// Used by the clock trace and instance scans in Tasks 4-6.
#[allow(dead_code)]
impl Adjacency {
    /// `(net, pin)` of each connection of `instance`.
    pub(crate) fn of(&self, instance: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
        let range = self.start[instance as usize]..self.start[instance as usize + 1];
        range.map(|i| (self.net[i], self.pin[i]))
    }
}

/// A name table with a hash index over the names it stores (no second copy).
#[derive(Default)]
struct Interner {
    names: NameTable,
    index: HashTable<u32>,
}

impl Interner {
    fn find(&self, state: &RandomState, name: &str) -> Option<u32> {
        let hash = state.hash_one(name);
        self.index
            .find(hash, |&id| self.names.get(id) == name)
            .copied()
    }

    /// The id of `name`, pushing it when new.
    fn intern(&mut self, state: &RandomState, name: &str) -> u32 {
        let hash = state.hash_one(name);
        if let Some(&id) = self.index.find(hash, |&id| self.names.get(id) == name) {
            return id;
        }
        let id = self.names.push(name);
        let names = &self.names;
        self.index
            .insert_unique(hash, id, |&id| state.hash_one(names.get(id)));
        id
    }

    /// Push `name` and make the lookup resolve to the new id (last wins).
    fn push_last_wins(&mut self, state: &RandomState, name: &str) -> u32 {
        let hash = state.hash_one(name);
        let id = self.names.push(name);
        let names = &self.names;
        match self.index.entry(
            hash,
            |&other| other != id && names.get(other) == name,
            |&id| state.hash_one(names.get(id)),
        ) {
            hashbrown::hash_table::Entry::Occupied(mut e) => *e.get_mut() = id,
            hashbrown::hash_table::Entry::Vacant(e) => {
                e.insert(id);
            }
        }
        id
    }
}

/// One chunk of nets resolved against the component snapshot.
struct Resolved<'a> {
    pins: NameTable,
    /// Per connection: instance id, [`PORT`], or [`UNKNOWN`].
    instance: Vec<u32>,
    pin: Vec<u32>,
    /// Names of the [`UNKNOWN`] instances, in connection order.
    unknown: Vec<&'a str>,
    /// Connections per net.
    lens: Vec<usize>,
}

/// Placeholder for a name that is not a component.
const UNKNOWN: u32 = u32::MAX - 1;

pub(crate) struct ConnectivityBuilder {
    state: RandomState,
    instances: Interner,
    component_count: u32,
    macros: Interner,
    instance_macro: Vec<u32>,
    pins: Interner,
    nets: NameTable,
    conn_start: Vec<usize>,
    conn_instance: Vec<u32>,
    conn_pin: Vec<u32>,
}

impl Default for ConnectivityBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectivityBuilder {
    pub(crate) fn new() -> Self {
        Self {
            state: RandomState::new(),
            instances: Interner::default(),
            component_count: 0,
            macros: Interner::default(),
            instance_macro: Vec::new(),
            pins: Interner::default(),
            nets: NameTable::new(),
            conn_start: vec![0],
            conn_instance: Vec::new(),
            conn_pin: Vec::new(),
        }
    }

    /// Add a COMPONENTS entry. Must come before every `add_nets`.
    pub(crate) fn add_component(&mut self, instance: &str, macro_name: &str) {
        debug_assert!(self.nets.is_empty(), "add_component after add_nets");
        self.instances.push_last_wins(&self.state, instance);
        let macro_id = self.macros.intern(&self.state, macro_name);
        self.instance_macro.push(macro_id);
        self.component_count += 1;
    }

    /// Append nets. Names resolve in parallel chunks against the components;
    /// the sequential merge assigns ids to unknown instances first-seen.
    pub(crate) fn add_nets(&mut self, nets: &[NetConns<'_>]) {
        let state = &self.state;
        let instances = &self.instances;
        let resolved: Vec<Resolved<'_>> = crate::threads::install(|| {
            nets.par_chunks(CHUNK)
                .map(|chunk| resolve_chunk(state, instances, chunk))
                .collect()
        });
        for (chunk, r) in nets.chunks(CHUNK).zip(resolved) {
            let map: Vec<u32> = r
                .pins
                .iter()
                .map(|name| self.pins.intern(&self.state, name))
                .collect();
            let mut unknown = r.unknown.iter();
            for &i in &r.instance {
                self.conn_instance.push(if i == UNKNOWN {
                    self.instances.intern(&self.state, unknown.next().unwrap())
                } else {
                    i
                });
            }
            self.conn_pin.extend(r.pin.iter().map(|&p| map[p as usize]));
            for (net, len) in chunk.iter().zip(&r.lens) {
                self.nets.push(net.name);
                let last = *self.conn_start.last().unwrap();
                self.conn_start.push(last + len);
            }
        }
    }

    pub(crate) fn finish(self) -> Connectivity {
        Connectivity {
            instances: self.instances.names,
            component_count: self.component_count,
            macros: self.macros.names,
            instance_macro: self.instance_macro,
            pins: self.pins.names,
            nets: self.nets,
            conn_start: self.conn_start,
            conn_instance: self.conn_instance,
            conn_pin: self.conn_pin,
        }
    }
}

fn resolve_chunk<'a>(
    state: &RandomState,
    instances: &Interner,
    nets: &[NetConns<'a>],
) -> Resolved<'a> {
    let mut pins = Interner::default();
    let mut out = Resolved {
        pins: NameTable::new(),
        instance: Vec::new(),
        pin: Vec::new(),
        unknown: Vec::new(),
        lens: Vec::with_capacity(nets.len()),
    };
    for net in nets {
        out.lens.push(net.conns.len());
        for conn in &net.conns {
            let (instance, pin) = match *conn {
                Conn::Port(name) => (PORT, name),
                Conn::Pin { instance, pin } => match instances.find(state, instance) {
                    Some(id) => (id, pin),
                    None => {
                        out.unknown.push(instance);
                        (UNKNOWN, pin)
                    }
                },
            };
            out.instance.push(instance);
            out.pin.push(pins.intern(state, pin));
        }
    }
    out.pins = pins.names;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads;

    fn net<'a>(name: &'a str, conns: Vec<Conn<'a>>) -> NetConns<'a> {
        NetConns { name, conns }
    }
    fn pin<'a>(instance: &'a str, pin: &'a str) -> Conn<'a> {
        Conn::Pin { instance, pin }
    }

    #[test]
    fn name_table_round_trips() {
        let mut t = NameTable::new();
        assert_eq!(t.push("a"), 0);
        assert_eq!(t.push(""), 1);
        assert_eq!(t.push("bc"), 2);
        assert_eq!((t.get(0), t.get(1), t.get(2)), ("a", "", "bc"));
        assert_eq!(t.len(), 3);
        assert!(!t.is_empty());
        assert_eq!(t.iter().collect::<Vec<_>>(), ["a", "", "bc"]);
    }

    #[test]
    fn duplicate_component_last_definition_wins() {
        let mut b = ConnectivityBuilder::new();
        b.add_component("u1", "INV");
        b.add_component("u1", "BUF");
        b.add_nets(&[net("n", vec![pin("u1", "A")])]);
        let c = b.finish();
        let (inst, _) = c.conns(0).next().unwrap();
        assert_eq!(inst, 1);
        assert_eq!(c.macro_of(inst), Some("BUF"));
        assert_eq!(c.macro_of(0), Some("INV"));
        assert_eq!(c.component_lookup()["u1"], 1);
        assert_eq!(c.component_count, 2);
    }

    #[test]
    fn unknown_instances_get_ids_past_the_components() {
        let mut b = ConnectivityBuilder::new();
        b.add_component("u1", "INV");
        b.add_nets(&[net(
            "n",
            vec![pin("*", "VDD"), pin("u9", "A"), pin("*", "VSS")],
        )]);
        let c = b.finish();
        let ids: Vec<u32> = c.conns(0).map(|(i, _)| i).collect();
        assert_eq!(ids, [1, 2, 1]);
        assert_eq!(c.instances.get(1), "*");
        assert_eq!(c.instances.get(2), "u9");
        assert_eq!(c.macro_of(1), None);
        assert_eq!(c.macro_of(2), None);
    }

    #[test]
    fn ports_use_the_port_id() {
        let mut b = ConnectivityBuilder::new();
        b.add_nets(&[net("n", vec![Conn::Port("clk")])]);
        let c = b.finish();
        let (inst, p) = c.conns(0).next().unwrap();
        assert_eq!(inst, PORT);
        assert_eq!(c.pins.get(p), "clk");
        assert_eq!(c.instances.len(), 0);
    }

    #[test]
    fn adjacency_keeps_net_then_connection_order() {
        let mut b = ConnectivityBuilder::new();
        b.add_component("u1", "INV");
        b.add_nets(&[
            net("n0", vec![pin("u1", "A"), pin("u1", "B")]),
            net("n1", vec![pin("u1", "Y"), Conn::Port("p")]),
        ]);
        let c = b.finish();
        let adj = c.adjacency();
        let names: Vec<(u32, &str)> = adj.of(0).map(|(n, p)| (n, c.pins.get(p))).collect();
        assert_eq!(names, [(0, "A"), (0, "B"), (1, "Y")]);
    }

    fn build(nets: &[NetConns<'_>]) -> Connectivity {
        let mut b = ConnectivityBuilder::new();
        for i in 0..50 {
            b.add_component(&format!("u{i}"), "M");
        }
        b.add_nets(nets);
        b.finish()
    }

    #[test]
    fn parallel_and_single_thread_builds_agree() {
        let _serial = threads::TEST_LIMIT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = threads::set_threads(None);
            }
        }
        let _restore = Restore;
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        type Raw = Vec<(String, Vec<(Option<String>, String)>)>;
        let names: Raw = (0..10_000)
            .map(|n| {
                let conns = (0..(next() % 5 + 1))
                    .map(|_| {
                        let inst = match next() % 10 {
                            0 => None,
                            // some names are not components
                            _ => Some(format!("u{}", next() % 80)),
                        };
                        (inst, format!("p{}", next() % 30))
                    })
                    .collect();
                (format!("net{n}"), conns)
            })
            .collect();
        let nets: Vec<NetConns<'_>> = names
            .iter()
            .map(|(name, conns)| NetConns {
                name,
                conns: conns
                    .iter()
                    .map(|(i, p)| match i {
                        Some(i) => Conn::Pin {
                            instance: i,
                            pin: p,
                        },
                        None => Conn::Port(p),
                    })
                    .collect(),
            })
            .collect();
        threads::set_threads(None).unwrap();
        let many = build(&nets);
        threads::set_threads(Some(1)).unwrap();
        let one = build(&nets);
        assert_eq!(many.conn_instance, one.conn_instance);
        assert_eq!(many.conn_pin, one.conn_pin);
        assert_eq!(many.conn_start, one.conn_start);
        assert!(many.pins.iter().eq(one.pins.iter()));
        assert!(many.instances.iter().eq(one.instances.iter()));
        assert_eq!(many.nets.len(), 10_000);
    }
}
