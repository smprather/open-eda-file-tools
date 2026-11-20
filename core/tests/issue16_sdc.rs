use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use open_eda_file_tools_core::sdc::{Clock, clocks, evaluate};
use open_eda_file_tools_core::{def, liberty};

struct Fixture(PathBuf);

impl Fixture {
    fn new(script: &str) -> Self {
        Self::with_extension("sdc", script)
    }

    fn with_extension(extension: &str, text: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "core sdc {} {}.{extension}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, text).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn traces_sdc_clocks_through_def_using_targeted_liberty_roles() {
    if !tcl_available() {
        return;
    }
    let sdc = Fixture::new(
        r#"
        create_clock -period 4 [get_ports input_clk]
        create_generated_clock -source [get_ports input_clk] -divide_by 2 [get_pins div/Q]
        "#,
    );
    let layout = Fixture::with_extension(
        "def",
        r#"
        COMPONENTS 3 ;
        - div FF + PLACED ( 0 0 ) N ;
        - clkbuf BUF + PLACED ( 10 0 ) N ;
        - sink FF + PLACED ( 20 0 ) N ;
        END COMPONENTS
        NETS 3 ;
        - input_clk ( PIN input_clk ) ( div CK ) ;
        - divided ( div Q ) ( clkbuf A ) ;
        - buffered ( clkbuf Z ) ( sink CK ) ;
        END NETS
        END DESIGN
        "#,
    );
    let lib = Fixture::with_extension(
        "lib",
        r#"
        library (test) {
            cell (FF) {
                ff (IQ, IQN) { clocked_on : CK; }
                pin (CK) { direction : input; }
                pin (Q) { direction : output; }
            }
            cell (BUF) {
                pin (A) { direction : input; }
                pin (Z) { direction : output; }
            }
        }
        "#,
    );
    let roles: Vec<_> = liberty::cell_clock_roles_for_cells(
        std::slice::from_ref(&lib.0),
        &HashSet::from(["FF".into(), "BUF".into()]),
    )
    .unwrap()
    .into_iter()
    .map(def::MacroPinRole::from)
    .collect();
    let roots: Vec<_> = clocks(&sdc.0)
        .unwrap()
        .into_iter()
        .flat_map(|clock| clock.sources)
        .collect();
    assert_eq!(
        def::clock_tree_cells(&layout.0, &roots, &roles, true).unwrap(),
        ["clkbuf", "div"]
    );
}

fn tcl_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let candidates = std::env::var_os("TCLSH")
            .filter(|value| !value.is_empty())
            .map_or_else(
                || vec![OsString::from("tclsh8.6"), OsString::from("tclsh")],
                |path| vec![path],
            );
        candidates.iter().any(|candidate| {
            Command::new(candidate)
                .arg("-")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok()
        })
    })
}

#[test]
fn evaluates_real_tcl_and_records_unknown_constraints() {
    if !tcl_available() {
        return;
    }
    let file = Fixture::new(
        r#"
        set period [expr {4 + 4}]
        foreach port {clk_a clk_b} {
            create_clock -period $period [get_ports $port]
        }
        set_clock_uncertainty 0.25 [get_clocks clk_a]
        set last_result "ordinary Tcl result"
        "#,
    );
    let evaluation = evaluate(&file.0).unwrap();
    assert_eq!(evaluation.error, None);
    assert_eq!(
        evaluation.records,
        [
            vec!["create_clock", "-period", "8", "port:clk_a"],
            vec!["create_clock", "-period", "8", "port:clk_b"],
            vec!["unknown", "set_clock_uncertainty", "0.25", "clock:clk_a"],
        ]
    );
    let found = clocks(&file.0).unwrap();
    assert_eq!(
        found,
        ["clk_a", "clk_b"].map(|name| Clock {
            name: name.into(),
            period: Some(8.0),
            waveform: vec![0.0, 4.0],
            sources: vec![format!("port:{name}")],
            generated_from: None,
            divide_by: None,
            multiply_by: None,
            edges: vec![],
            invert: false,
        })
    );
}

#[test]
fn resolves_generated_clocks_in_file_order() {
    if !tcl_available() {
        return;
    }
    let file = Fixture::new(
        r#"
        create_clock -name main -period 10 -waveform {2.5 7.5} [get_ports clk]
        create_clock -name aux -period 8 [get_nets aux_n]
        create_generated_clock -name divided -source [get_ports clk] -divide_by 2.0             -edges {1 3 5} -invert [get_pins u_div/Q]
        create_generated_clock -master_clock aux -source [get_ports clk]             -multiply_by 4 [get_cells u_mul]
        create_generated_clock -name unresolved -master_clock future -divide_by 2
        "#,
    );
    let found = clocks(&file.0).unwrap();
    assert_eq!(found[0].waveform, [2.5, 7.5]);
    assert_eq!(found[1].sources, ["net:aux_n"]);
    assert_eq!(
        found[2],
        Clock {
            name: "divided".into(),
            period: Some(20.0),
            waveform: vec![],
            sources: vec!["pin:u_div/Q".into()],
            generated_from: Some("main".into()),
            divide_by: Some(2),
            multiply_by: None,
            edges: vec![1.0, 3.0, 5.0],
            invert: true,
        }
    );
    assert_eq!(found[3].name, "u_mul");
    assert_eq!(found[3].sources, ["cell:u_mul"]);
    assert_eq!(found[3].generated_from.as_deref(), Some("aux"));
    assert_eq!(found[3].multiply_by, Some(4));
    assert_eq!(found[3].period, Some(2.0));
    assert_eq!(found[4].generated_from.as_deref(), Some("future"));
    assert_eq!(found[4].period, None);
}

#[test]
fn symbolic_collections_keep_one_root_per_endpoint() {
    if !tcl_available() {
        return;
    }
    let file = Fixture::new(
        r#"
        create_clock -period 12 [get_ports {first {clock 🌀 with space}}]
        create_clock -period 20 [get_ports other]
        create_generated_clock -source [get_ports {{clock 🌀 with space}}]             -divide_by 2 [get_pins u/Q]
        "#,
    );
    let found = clocks(&file.0).unwrap();
    assert_eq!(found[0].name, "first");
    assert_eq!(found[0].sources, ["port:first", "port:clock 🌀 with space"]);
    assert_eq!(found[2].generated_from.as_deref(), Some("first"));
    assert_eq!(found[2].period, Some(24.0));
}

#[test]
fn keeps_partial_records_and_reports_script_errors_with_the_path() {
    if !tcl_available() {
        return;
    }
    let file = Fixture::new("create_clock -period 4 [get_ports clk]\nerror {broken design}");
    let evaluation = evaluate(&file.0).unwrap();
    assert_eq!(evaluation.records.len(), 1);
    assert_eq!(evaluation.error.as_deref(), Some("broken design"));
    let error = clocks(&file.0).unwrap_err().to_string();
    assert!(error.contains(&*file.0.to_string_lossy()), "{error}");
    assert!(error.contains("broken design"), "{error}");
}

#[test]
fn reports_invalid_clock_options_with_the_path() {
    if !tcl_available() {
        return;
    }
    for (script, option) in [
        ("create_clock -period", "-period"),
        ("create_clock -waveform {0 invalid}", "-waveform"),
        ("create_generated_clock -divide_by NaN", "-divide_by"),
    ] {
        let file = Fixture::new(script);
        let error = clocks(&file.0).unwrap_err().to_string();
        assert!(error.contains(&*file.0.to_string_lossy()), "{error}");
        assert!(error.contains(option), "{error}");
    }
}

#[test]
fn tcl_only_sees_safe_commands() {
    if !tcl_available() {
        return;
    }
    let file = Fixture::new(
        r#"
        foreach command {open exec source socket load} {
            if {[llength [info commands $command]] != 0} {
                error "unsafe command visible: $command"
            }
        }
        if {[info exists env(PATH)]} { error "process environment visible" }
        create_clock -name "safe\nclk\u0000" -period 4 [get_ports clk]
        "#,
    );
    let found = clocks(&file.0).unwrap();
    assert_eq!(found[0].name, "safe\nclk\0");
}
