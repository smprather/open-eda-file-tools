# DEF Connectivity Memory Layout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The clock trace, clock-object resolution and clock-instances report hold DEF connectivity in flat arrays, with no heap allocation per connection, and give identical results.

**Architecture:** A new crate-internal module `core/src/connectivity.rs` holds the model. Names live in append-only `NameTable`s (one `String` plus end offsets). Connections are two `u32` columns (instance id, pin id) in CSR form per net. A per-instance adjacency index is built on demand. Net records still parse in parallel, into views that borrow the record bytes; a builder resolves the names. The DEF scan and the cache loaders both feed the builder. The cache file formats do not change.

**Tech Stack:** Rust (the `open-eda-file-tools-core` crate: rayon, `hashbrown` 0.17 `HashTable`, already vendored), pytest.

**Spec:** No spec document. Requirements come from the 2026-11-19 measurements below and from the existing behavior of the consumers, which `scripts/def_equivalence.py` pins.

**Revised 2026-11-19 after issue #12:** the DEF code moved from `src/def.rs` to the plain-Rust core crate (`core/src/def.rs`, `core/src/lef/lexer.rs`); `src/def.rs` is now only the pyo3 bindings and the Parquet writer. Paths, gates and visibility below are updated for that layout; the design is unchanged.

## Background (2026-11-19, commit 0812a7d)

`clock_tree_cells` on a 458 MB generated DEF (2M instances, power nets that list ~4M connections each) takes 2.66 s:

- 1.18 s collecting connectivity;
- 1.02 s building the `HashMap<&str, Vec<(usize, &str)>>` instance index;
- 0.63 s just freeing memory at the end.

Each connection is a `NetConn { instance: Option<String>, pin: String }` and each component a `HashMap<String, String>` entry, about 20M small allocations. On the 118 MB routed cva6 DEF the same split is 0.15 / 0.03 / 0.03 s. Consumers (all in `core/src/def.rs`; the pyo3 bindings in `src/def.rs` call the `*_impl` entry points, whose signatures do not change):

- `clock_tree_cells_impl`, `collect_root_nets`;
- `clock_object_instances_impl`;
- `write_clock_instances_csv_stream`, `seed_clock_nets`, `traverse_clock_instances`;
- cache: `stream_components_to_cache`, `NetCacheWriter`, `write_net_info`, `load_components_bin`, `load_connectivity_bin`.

There is also a pre-existing defect. A connection `( u1 A + SYNTHESIZED )` (legal DEF) ends the connection list at its `+`, and `parse_connection_list` then fails with `expected ')'`.

## Global Constraints

- Build with `uv run --no-sync maturin develop --release` after any Rust change (thin LTO, ~20 s incremental), then test with `uv run --no-sync pytest -q`. Plain `uv run` can reinstall a stale cached wheel and triggers a slow fat-LTO `dist` build.
- Gates before every commit: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets` (warning count must not rise above the baseline recorded at the start of the work), `uv run --no-sync ruff check .`, `uv run --no-sync ty check`.
- Output identity: after every task, `uv run --no-sync python scripts/def_equivalence.py <ref .so> <dir>` reports `differences: 0`. Run it on a generated design (`-n 200000`) at default threads and at `--threads 1`, and on a routed DEF when one is available. The reference is built from the commit before Task 1 (Task 1 changes behavior only for DEFs containing `+ SYNTHESIZED`, which the generator does not write).
- Cache format unchanged: `CACHE_VERSION` stays 1, and cache files are byte-identical (the equivalence script compares them).
- Parallel work stays inside `crate::threads::install` (re-entrant: a nested install keeps the outer pool).
- Visibility: everything this plan adds is crate-internal (`pub(crate)` or private), per the three tiers in `core/src/lib.rs`. The documented Rust API (`docs/ai-api.md` §8) and the bindings' entry points do not change.
- New dependency: `hashbrown = { version = "0.17", default-features = false }` goes in `core/Cargo.toml` (it has no dependencies of its own with default features off). Update every place that lists the core crate's dependencies (`docs/ai-api.md` §8, `CLAUDE.md` if it lists them).
- DEF names compare exactly as today: case-sensitive for instances and pins, with backslash escapes removed only in `clock_object_instances` lookups.
- Documentation synchronization mandate (`CLAUDE.md`). This plan changes no public API; update `CLAUDE.md` (architecture invariants), `docs/def.md` (performance table) and `docs/sdc.md` if numbers move.
- Indent with spaces only.

## Review Focus

- Duplicate component names: the last definition's macro wins, exactly as the `HashMap` insert does today. (Task 2 test `duplicate_component_last_definition_wins`.)
- Instances referenced by nets but absent from COMPONENTS (`( * VDD )`, typos): kept, with no macro, never matched to a component. (Task 2 test `unknown_instances_get_ids_past_the_components`.)
- NETS before COMPONENTS (non-conforming order): the same results as conforming order. (Task 3 test `test_nets_before_components`.)
- First-visit order in `clock-instances`: the `net` column records the first net that reached an instance in BFS order. Adjacency must list each instance's nets in net order, then connection order, as `instance_to_nets` does today. (Task 2 test `adjacency_keeps_net_then_connection_order`; Task 6 equivalence.)
- Thread limit 1 vs all: identical connectivity. (Task 2 test `parallel_and_single_thread_builds_agree`.)

## Decisions this plan makes

1. Instance ids: `0..component_count` follow COMPONENTS order (duplicates keep both entries). Names seen only in nets get ids `component_count..` in first-seen order. `PORT = u32::MAX` marks `( PIN name )`.
2. Name lookup during the build uses `hashbrown::HashTable<u32>`, hashing the name stored in the `NameTable`. This avoids a second copy of every name and the self-borrow a `HashMap<&str, u32>` would need. Hasher: `std::hash::RandomState`.
3. Records from a NETS/SPECIALNETS section met before COMPONENTS are kept (connection part only) and resolved after the scan.
4. Adjacency is built sequentially (a counting pass plus a fill pass, O(connections)). It lists entries in net order, then connection order.
5. `unescaped` returns `Cow<'_, str>`, so names without a backslash are not copied.

---

### Task 1: `+ SYNTHESIZED` inside a connection

**Files:**
- Modify: `core/src/lef/lexer.rs` — `read_until_plus_or_semicolon`, `scan_record`
- Modify: `core/src/def.rs` — `parse_connection_list`, `split_connection_list`
- Test: `tests/test_def_scanning.py`

**Interfaces:**
- Produces: `Lexer::read_until_plus_or_semicolon(&mut self, out: &mut Vec<u8>) -> io::Result<bool>`, now stopping at the first `+` outside quotes **and outside parentheses** (same name, doc comment updated).

- [ ] **Step 1: Write the failing test.**

```python
def test_synthesized_connections_are_kept(tmp_path):
    lef, design = write_design(tmp_path, components=3)
    text = design.read_text(encoding="utf-8").replace("( u1 A )", "( u1 A + SYNTHESIZED )")
    design.write_text(text, encoding="utf-8")
    cells = _native.clock_tree_cells(design, ["port:clk"], [("INV", ["A"], ["Y"], False, [])], False)
    assert cells == ["u0", "u1", "u2"]
    _native.write_def_cache(design, tmp_path / "cache")  # must not raise
```

  Add a Rust lexer test `connection_part_skips_plus_inside_parentheses`. Input `- n1 ( u1 A + SYNTHESIZED ) ( u2 B ) + ROUTED M1 ( 0 0 ) ;` must yield `- n1 ( u1 A + SYNTHESIZED ) ( u2 B ) ` and return `true`.

- [ ] **Step 2: Run the tests and see them fail.** Run: `cargo test -p open-eda-file-tools-core lexer` and `uv run --no-sync pytest tests/test_def_scanning.py -q`. Expected: FAIL (`expected ')' for net connection, got '+'`).

- [ ] **Step 3: Implement.** In `scan_record` with `stop_at_plus`, track parenthesis depth outside quotes and comments, and stop at `+` only at depth 0. `parse_connection_list` accepts an optional `+ SYNTHESIZED` before the closing `)`. `split_connection_list` finds the list end with the same depth-0 rule instead of `memchr(b'+')`.

- [ ] **Step 4: Run the tests and see them pass**, plus `uv run --no-sync pytest -q`.

- [ ] **Step 5: Commit.** `git commit -m "DEF connectivity: keep + SYNTHESIZED connections"`

### Task 2: `core/src/connectivity.rs` model and builder

**Files:**
- Create: `core/src/connectivity.rs` (register with `pub(crate) mod connectivity;` in `core/src/lib.rs`)
- Modify: `core/Cargo.toml` (add `hashbrown = { version = "0.17", default-features = false }`; run `cargo build --offline -p open-eda-file-tools-core` to confirm the vendored crate resolves; then run `uv run --no-sync python scripts/check_vendored_crates.py`); the dependency lists in `docs/ai-api.md` §8 (and `CLAUDE.md` if it lists them)
- Modify: `core/src/threads.rs` — hoist the `SERIAL` mutex of its test module to module level as `#[cfg(test)] pub(crate) static TEST_LIMIT_LOCK: Mutex<()>`, used by every unit test in the crate that writes the global limit
- Test: unit tests in `core/src/connectivity.rs`

**Interfaces:**
- Produces (all `pub(crate)`, used by Tasks 3–7; the signatures below say `pub` for brevity):

```rust
pub struct NameTable { /* text: String, ends: Vec<usize> */ }
impl NameTable {
    pub fn new() -> Self;
    pub fn push(&mut self, name: &str) -> u32;        // never deduplicates
    pub fn get(&self, id: u32) -> &str;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn iter(&self) -> impl Iterator<Item = &str>; // in id order
}

pub const PORT: u32 = u32::MAX;

pub enum Conn<'a> { Port(&'a str), Pin { instance: &'a str, pin: &'a str } }
pub struct NetConns<'a> { pub name: &'a str, pub conns: Vec<Conn<'a>> }

pub struct Connectivity {
    pub instances: NameTable,     // Decision 1
    pub component_count: u32,
    pub macros: NameTable,        // distinct macro names
    pub instance_macro: Vec<u32>, // len == component_count, index into macros
    pub pins: NameTable,          // distinct pin and port names
    pub nets: NameTable,          // one entry per net record, file order
    pub conn_start: Vec<usize>,   // len == nets.len() + 1
    pub conn_instance: Vec<u32>,  // instance id or PORT
    pub conn_pin: Vec<u32>,       // index into pins
}
impl Connectivity {
    pub fn conns(&self, net: u32) -> impl Iterator<Item = (u32, u32)> + '_;   // (instance or PORT, pin)
    pub fn macro_of(&self, instance: u32) -> Option<&str>;                   // None past component_count
    pub fn component_lookup(&self) -> HashMap<&str, u32>;                    // last definition wins
    pub fn adjacency(&self) -> Adjacency;                                    // Decision 4
}
pub struct Adjacency { /* start: Vec<usize>, net: Vec<u32>, pin: Vec<u32> */ }
impl Adjacency { pub fn of(&self, instance: u32) -> impl Iterator<Item = (u32, u32)> + '_; } // (net, pin)

pub struct ConnectivityBuilder { /* ... */ }
impl ConnectivityBuilder {
    pub fn new() -> Self;
    pub fn add_component(&mut self, instance: &str, macro_name: &str);
    pub fn add_nets(&mut self, nets: &[NetConns<'_>]);   // resolves in parallel chunks of 2048 nets
    pub fn finish(self) -> Connectivity;
}
```

  `add_nets` resolves instance names in parallel against a read-only snapshot of the component table. Each chunk interns pins into a local table; the sequential merge then maps local pin ids to global ones and assigns ids to unknown instances in first-seen order. Calling `add_component` after `add_nets` is a programming error (`debug_assert!`); Task 3 guarantees the order.

- [ ] **Step 1: Write the failing Rust tests** in `#[cfg(test)] mod tests`:
  - `name_table_round_trips`: push `"a"`, `""`, `"bc"`; `get` returns them; `len == 3`.
  - `duplicate_component_last_definition_wins`: components `u1 INV`, `u1 BUF`; one net `( u1 A )`. Expect `macro_of(conn instance) == Some("BUF")` and `component_lookup()["u1"]` to be the second id.
  - `unknown_instances_get_ids_past_the_components`: components `u1`; nets `( * VDD ) ( u9 A ) ( * VSS )`. Expect ids `component_count` (for `*`) and `component_count + 1` (for `u9`); the second `*` reuses the first id; `macro_of` is `None` for both.
  - `ports_use_the_port_id`: `( PIN clk )` gives `(PORT, pin id of "clk")`.
  - `adjacency_keeps_net_then_connection_order`: nets `n0 ( u1 A ) ( u1 B )` and `n1 ( u1 Y )`. Expect `adjacency().of(u1)` to yield `[(0, A), (0, B), (1, Y)]`.
  - `parallel_and_single_thread_builds_agree`: 10,000 random nets (a fixed-seed generator in the test; no new dev-dependency) built with no limit and with `set_threads(Some(1))` give equal `conn_instance`, `conn_pin` and `pins`. Hold `threads::TEST_LIMIT_LOCK` for the whole test and restore `set_threads(None)` even on panic. (A test-local rayon pool does not work: `threads::install` only stays inside pools it created.)

- [ ] **Step 2: Run the tests and see them fail.** Run: `cargo test -p open-eda-file-tools-core connectivity`. Expected: compile failure (module missing).

- [ ] **Step 3: Implement** the module to the interface above.

- [ ] **Step 4: Run the tests and see them pass.** `cargo test -p open-eda-file-tools-core connectivity`, then the full gates.

- [ ] **Step 5: Commit.** `git commit -m "connectivity: flat name tables and CSR connections"`

### Task 3: Build `Connectivity` from the DEF and the cache

**Files:**
- Modify: `core/src/def.rs` — `parse_net_connections_record` (returns `NetConns<'_>`), `parse_connection_list`, `collect_def_connectivity`, `collect_component_section`, `collect_net_connectivity`, `NetCacheWriter::flush`, `write_net_info` (takes `&NetConns<'_>`), `load_components_bin` + `load_connectivity_bin` (replaced by `load_connectivity`)
- Test: `tests/test_def_scanning.py`

**Interfaces:**
- Consumes: Task 2's module.
- Produces:
  - `fn collect_def_connectivity(def_path: &Path) -> Result<Connectivity, String>`
  - `fn load_connectivity(cache_dir: &Path) -> Result<Connectivity, String>`: reads `components.bin` then `connectivity.bin` through a reused `String` buffer (`read_string_into(input, &mut String)`), with no `String` per record.
  - Temporary adapter, deleted in Task 7: `fn legacy_tables(c: &Connectivity) -> (HashMap<String, String>, Vec<NetInfo>)`, so the consumers compile unchanged in this task.

- [ ] **Step 1: Write the failing test.**

```python
def test_nets_before_components(tmp_path):
    lef, design = write_design(tmp_path, components=3)
    text = design.read_text(encoding="utf-8")
    components = text[text.index("COMPONENTS"):text.index("END COMPONENTS") + len("END COMPONENTS\n")]
    nets = text[text.index("NETS 3 ;"):text.index("END NETS") + len("END NETS\n")]
    design.write_text(text.replace(components, "").replace(nets, nets + components), encoding="utf-8")
    cells = _native.clock_tree_cells(design, ["port:clk"], [("INV", ["A"], ["Y"], False, [])], False)
    assert cells == ["u0", "u1", "u2"]
```

  Today this already passes, because `HashMap` collection does not care about section order. It is a guard for Decision 3: run it before and after the change.

- [ ] **Step 2: Implement.** The scan feeds `add_component` for each component record. NETS/SPECIALNETS batches are parsed in parallel into `Vec<NetConns<'_>>` (borrowing that batch's records) and passed to `add_nets`; batches seen before COMPONENTS are held until COMPONENTS ends (or EOF). The cache writer writes `NetConns` with the same bytes as before. Consumers call `legacy_tables(&collect_def_connectivity(..)?)`.

- [ ] **Step 3: Verify.** Full gates, then `scripts/def_equivalence.py` on the generated design (default and `--threads 1`). Expected: `differences: 0`.

- [ ] **Step 4: Commit.** `git commit -m "DEF connectivity: build flat Connectivity from DEF and cache"`

### Task 4: Clock trace on `Connectivity`

**Files:**
- Modify: `core/src/def.rs` — `clock_tree_cells_impl`, `collect_root_nets`, `role_map`

**Interfaces:**
- Consumes: `Connectivity`, `Adjacency`, `component_lookup` (Task 2); `collect_def_connectivity` (Task 3).
- Produces: `fn roles_by_macro<'a>(c: &Connectivity, macros: &'a [MacroRole]) -> Vec<Option<RoleRef<'a>>>` (index = macro id; built by name once, not per connection).

- [ ] **Step 1: Implement.** Replace the `&str`-keyed maps with vectors indexed by instance id (`reached_from: Vec<Vec<u32>>` only for instances reached, or a `HashMap<u32, Vec<u32>>`; your choice) and `seen_nets: Vec<bool>`. Use `Adjacency::of` for outputs. `collect_root_nets` resolves `port:`, `net:`, `pin:` and `cell:` roots through `component_lookup` and a `HashMap<&str, u32>` of net names (last net of a duplicated name wins, as today's `net_by_name.insert`). Output names are sorted, as today.

- [ ] **Step 2: Verify.** `uv run --no-sync pytest tests/test_sdc.py tests/test_clock_data.py tests/test_def_scanning.py -q`, then the equivalence script (its `clock_tree_cells prune=True/False` lines must be `ok`).

- [ ] **Step 3: Measure.** Run `uv run --no-sync python <clock_probe.py> <ref .so> <generated 2M design.def>` (a controller-provided scratch script, path given in the dispatch: old vs new `clock_tree_cells` time, each build in its own process). Record both; expected well under the reference (target ≤ 1.3 s once Task 7 removes the adapter).

- [ ] **Step 4: Commit.** `git commit -m "clock trace: run on flat connectivity"`

### Task 5: Clock-object resolution on `Connectivity`

**Files:**
- Modify: `core/src/def.rs` — `clock_object_instances_impl`, `unescaped` (Decision 5)

**Interfaces:**
- Consumes: Task 2/3 interfaces.
- Produces: `fn unescaped(name: &str) -> Cow<'_, str>`

- [ ] **Step 1: Implement.** The net lookup is `HashMap<Cow<str>, u32>` over net names. The instance lookup is `HashMap<Cow<str>, u32>` over `component_lookup()` entries. Ports are the set of `pins.get(pin)` for connections whose instance is `PORT`, unescaped. The return tuple and its sorting are unchanged.

- [ ] **Step 2: Verify.** `uv run --no-sync pytest tests/test_clock_data.py -q` and the equivalence script (`clock_object_instances ok`).

- [ ] **Step 3: Commit.** `git commit -m "clock-data resolution: run on flat connectivity"`

### Task 6: `clock-instances` report on `Connectivity`

**Files:**
- Modify: `core/src/def.rs` — `write_clock_instances_csv_impl`, `write_clock_instances_csv_from_cache_impl`, `write_clock_instances_csv_stream`, `seed_clock_nets`, `traverse_clock_instances`

**Interfaces:**
- Consumes: `collect_def_connectivity`, `load_connectivity` (Task 3); `Adjacency` (Task 2).

- [ ] **Step 1: Implement.** Keep the BFS order and first-visit rule exactly. `seen_instances` becomes a vector indexed by instance id holding `(net id, is_clock_cell)`. The macro is `macro_of(id)` or `""`, and `is_clock_cell` is computed on that string, as today. Rows are sorted by instance name.

- [ ] **Step 2: Verify.** `uv run --no-sync pytest tests/test_instances.py -q` and the equivalence script (`clock_instances` and `clock_instances from cache` lines `ok`).

- [ ] **Step 3: Commit.** `git commit -m "clock-instances: run on flat connectivity"`

### Task 7: Remove the old types; acceptance

**Files:**
- Modify: `core/src/def.rs` — delete `NetConn`, `NetInfo`, `legacy_tables` and every remaining use
- Modify: `CLAUDE.md` (DEF invariants: connectivity lives in `core/src/connectivity.rs`; ids and adjacency order per Decisions 1 and 4), `docs/def.md` (performance table)

- [ ] **Step 1: Delete and compile.** `cargo build --workspace --release` with no reference to the removed types; `cargo clippy --workspace --all-targets` count not above the baseline.

- [ ] **Step 2: Full gates and equivalence.** Run all gates, then `scripts/def_equivalence.py` on the generated design (default and `--threads 1`) and on a routed DEF if available. Expected: `differences: 0` everywhere.

- [ ] **Step 3: Measure and record.** Run `scripts/def_bench.py` on the generated 2M design and on cva6 (paths given in the dispatch). For time and peak memory, old against new, run `<clock_probe.py> <ref .so> <def>` on both designs: it runs each build in its own process and prints `clock_tree_cells` seconds and peak RSS. Expected: `clock trace` ≤ 1.3 s on the generated design, and lower peak RSS. Put the new numbers in `docs/def.md` and remove its sentence saying the memory layout is the clock trace's main cost.

- [ ] **Step 4: Commit.** `git commit -m "DEF connectivity: drop per-connection strings"`
