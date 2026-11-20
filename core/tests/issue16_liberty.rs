use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use flate2::Compression;
use flate2::write::GzEncoder;
use open_eda_file_tools_core::Error;
use open_eda_file_tools_core::liberty::{CellClockRole, cell_clock_roles_for_cells};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "issue16_liberty_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, body: &str) -> PathBuf {
        let path = self.0.join(name);
        let library = format!("library (test) {{\n{body}\n}}");
        if name.ends_with(".gz") {
            let mut gzip = GzEncoder::new(File::create(&path).unwrap(), Compression::default());
            gzip.write_all(library.as_bytes()).unwrap();
            gzip.finish().unwrap();
        } else {
            fs::write(&path, library).unwrap();
        }
        path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wanted(names: &[&str]) -> HashSet<String> {
    names.iter().map(|&name| name.to_owned()).collect()
}

#[test]
fn cell_roles_preserve_top_level_pin_order_and_state_clock_priority() {
    let dir = TestDir::new();
    let path = dir.write(
        "cells.lib",
        r#"
        cell (BUF) {
            pin (B) { direction : input; }
            pin (Z) { direction : output; clock : true; }
            pin (A) { direction : input; clock : false; }
            pin (IO) { direction : inout; clock : true; }
            bus (MEMBERS) {
                direction : input;
                pin (MEMBERS[0]) { direction : input; clock : true; }
            }
        }
        cell (FF) {
            pin (CK) { direction : input; }
            pin (CK[0]) { direction : input; }
            pin (CK_LONG) { direction : input; }
            pin (CK_ALT) { direction : input; clock : "TrUE"; }
            pin (Q) { direction : output; }
            ff_bank (IQ, IQN, 2) {
                clocked_on : "!CK_LONG + CK[0]";
                clocked_on_also : "CK_ALT' | CK[0]";
            }
        }
        cell (LATCH) {
            pin (E) { direction : input; }
            latch_bank (IQ, IQN, 2) { enable : "E"; }
        }
        cell (CLOCK_PIN) {
            pin (CLK) { direction : input; clock : true; }
        }
        "#,
    );
    let roles =
        cell_clock_roles_for_cells(&[path], &wanted(&["BUF", "FF", "LATCH", "CLOCK_PIN"])).unwrap();

    assert_eq!(
        roles["BUF"],
        CellClockRole {
            inputs: vec!["B".into(), "A".into()],
            outputs: vec!["Z".into()],
            endpoint: false,
            clock_pins: vec![],
            clock_gate: false,
            sequential: false,
            memory: false,
        }
    );
    let ff = &roles["FF"];
    assert_eq!(ff.inputs, ["CK", "CK[0]", "CK_LONG", "CK_ALT"]);
    assert_eq!(ff.outputs, ["Q"]);
    assert_eq!(ff.clock_pins, ["CK_ALT", "CK[0]", "CK_LONG"]);
    assert!(ff.endpoint && ff.sequential && !ff.clock_gate && !ff.memory);
    assert_eq!(roles["LATCH"].clock_pins, ["E"]);
    assert!(roles["LATCH"].endpoint && roles["LATCH"].sequential);
    assert_eq!(roles["CLOCK_PIN"].clock_pins, ["CLK"]);
    assert!(roles["CLOCK_PIN"].endpoint && !roles["CLOCK_PIN"].sequential);
}

#[test]
fn clock_gates_use_explicit_clock_pins_and_ordered_fallbacks() {
    let dir = TestDir::new();
    let path = dir.write(
        "gates.lib",
        r#"
        cell (EXPLICIT) {
            clock_gating_integrated_cell : latch_posedge;
            latch (IQ, IQN) { enable : "ALT"; }
            pin (E) { direction : input; clock_gate_enable_pin : true; clock_gate_clock_pin : true; }
            pin (T) { direction : input; clock_gate_test_pin : true; clock_gate_clock_pin : true; }
            pin (ALT) { direction : input; clock : true; }
            pin (CLK) { direction : input; clock_gate_clock_pin : true; }
            pin (NOT_CLK) { direction : input; clock_gate_out_pin : true; }
            pin (GCLK) { direction : output; clock_gate_out_pin : true; }
            pin (OTHER) { direction : output; clock_gate_clock_pin : true; }
        }
        cell (CLOCK) {
            clock_gating_integrated_cell : latch_posedge;
            latch (IQ, IQN) { enable : "ALT"; }
            pin (ALT) { direction : input; }
            pin (CLK) { direction : input; clock : true; }
            pin (Q) { direction : output; }
        }
        cell (STATE) {
            clock_gating_integrated_cell : latch_posedge;
            latch (IQ, IQN) { enable : "!CLK"; }
            pin (E) { direction : input; }
            pin (CLK) { direction : input; }
            pin (Q) { direction : output; }
        }
        cell (UNFLAGGED) {
            clock_gating_integrated_cell : combinational;
            pin (E) { direction : input; clock_gate_enable_pin : true; }
            pin (T) { direction : input; clock_gate_test_pin : true; }
            pin (CLK) { direction : input; }
            pin (Q) { direction : output; }
        }
        "#,
    );
    let roles = cell_clock_roles_for_cells(
        &[path],
        &wanted(&["EXPLICIT", "CLOCK", "STATE", "UNFLAGGED"]),
    )
    .unwrap();

    for (name, output, sequential) in [
        ("EXPLICIT", "GCLK", true),
        ("CLOCK", "Q", true),
        ("STATE", "Q", true),
        ("UNFLAGGED", "Q", false),
    ] {
        let role = &roles[name];
        assert_eq!(role.inputs, ["CLK"], "{name}");
        assert_eq!(role.clock_pins, ["CLK"], "{name}");
        assert_eq!(role.outputs, [output], "{name}");
        assert_eq!(role.sequential, sequential, "{name}");
        assert!(role.clock_gate && !role.endpoint && !role.memory, "{name}");
    }
}

#[test]
fn real_memory_group_marks_an_endpoint_without_a_declared_clock() {
    let dir = TestDir::new();
    let path = dir.write(
        "memory.lib",
        r#"
        cell (BLOCK) {
            memory () { type : ram; address_width : 4; word_width : 8; }
            pin (D) { direction : input; }
            pin (Q) { direction : output; }
        }
        cell (SRAM_NAME_ONLY) {
            memory : true;
            pin (A) { direction : input; }
        }
        "#,
    );
    let roles = cell_clock_roles_for_cells(&[path], &wanted(&["BLOCK", "SRAM_NAME_ONLY"])).unwrap();

    let memory = &roles["BLOCK"];
    assert!(memory.memory && memory.endpoint && !memory.sequential);
    assert_eq!(memory.inputs, ["D"]);
    assert_eq!(memory.outputs, ["Q"]);
    assert!(memory.clock_pins.is_empty());
    assert!(!roles["SRAM_NAME_ONLY"].memory && !roles["SRAM_NAME_ONLY"].endpoint);
}

#[test]
fn stops_before_an_unreadable_library_once_all_cells_are_resolved() {
    let dir = TestDir::new();
    let first = dir.write(
        "first.lib.gz",
        r#"
        cell ("shared") { pin (OLD) { direction : input; } }
        cell (shared) { pin (FIRST) { direction : input; } }
        cell (unwanted) { direction }
        "#,
    );
    let second = dir.write(
        "second.lib",
        r#"
        cell (shared) { direction }
        cell (other) { pin (A) { direction : input; } }
        "#,
    );
    let missing = dir.0.join("unreadable.lib.gz");
    let roles =
        cell_clock_roles_for_cells(&[first, second, missing], &wanted(&["shared", "other"]))
            .unwrap();

    assert_eq!(roles.len(), 2);
    assert_eq!(roles["shared"].inputs, ["FIRST"]);
    assert_eq!(roles["other"].inputs, ["A"]);
}

#[test]
fn targeted_cells_skip_table_bodies_and_report_structural_errors() {
    let dir = TestDir::new();
    let path = dir.write(
        "tables.lib",
        r#"
        normalized_driver_waveform (template) { invalid }
        cell (kept) {
            pin (A) { direction : input; }
            pin (Y) {
                direction : output;
                timing () { cell_rise (template) { invalid } }
            }
        }
        cell (bad) { invalid }
        "#,
    );
    let roles = cell_clock_roles_for_cells(&[path.clone()], &wanted(&["kept", "absent"])).unwrap();
    assert_eq!(roles.len(), 1);
    assert_eq!(roles["kept"].outputs, ["Y"]);

    let error = cell_clock_roles_for_cells(&[path], &wanted(&["bad"]))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("tables.lib") && error.contains("bad"),
        "{error}"
    );
}

#[test]
fn only_opens_libraries_while_cell_names_remain_unresolved() {
    let dir = TestDir::new();
    let missing = dir.0.join("unreadable.lib");
    assert!(
        cell_clock_roles_for_cells(&[missing.clone()], &wanted(&[]))
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        cell_clock_roles_for_cells(&[missing], &wanted(&["unresolved"])),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound
    ));
}
