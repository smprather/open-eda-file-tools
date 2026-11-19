//! Integration tests for the public plain-Rust API of `open_eda_file_tools_core`
//! (issue #12): DEF instance bboxes, LEF macro sizes, and the Liberty
//! attribute-only / LibraryIndex surface.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use open_eda_file_tools_core::def::{
    SkippedRoutes, dbu_per_micron, scan_instance_bboxes, scan_net_shapes,
};
use open_eda_file_tools_core::lef::{MacroSize, macro_sizes};
use open_eda_file_tools_core::liberty::{CellAttributes, LibraryIndex, cell_attributes};
use open_eda_file_tools_core::threads::set_threads;

/// A per-test scratch directory under the system temp dir (the core crate has
/// no dev-dependencies by design; the existing unit tests do the same).
fn temp_dir(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "core_api_{}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        name
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const DEF: &str = "VERSION 5.8 ;\n\
UNITS DISTANCE MICRONS 1000 ;\n\
COMPONENTS 2 ;\n\
  - u1 INV + PLACED ( 10 20 ) N ;\n\
  - u2 INV + FIXED ( 30 40 ) E ;\n\
END COMPONENTS\n";

const LEF: &str = "VERSION 5.8 ;\n\
MACRO INV\n\
  SIZE 1.2 BY 2.4 ;\n\
END INV\n";

fn write(tmp: &std::path::Path, name: &str, text: &str) -> PathBuf {
    let path = tmp.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

fn write_def(tmp: &std::path::Path, body: &str) -> PathBuf {
    write(&tmp, "design.def", body)
}

fn write_lef(tmp: &std::path::Path) -> PathBuf {
    write(&tmp, "cells.lef", LEF)
}

#[test]
fn test_scan_instance_bboxes_rows() {
    let tmp = temp_dir("core_api");
    let def = write_def(&tmp, DEF);
    let lef = write_lef(&tmp);
    let rows: Vec<_> = scan_instance_bboxes(&def, &[lef])
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    let first = &rows[0];
    assert_eq!(
        (
            first.instance.as_str(),
            first.macro_name.as_str(),
            first.status.as_str(),
            first.orient.as_str(),
            first.llx,
            first.lly,
            first.urx,
            first.ury,
        ),
        ("u1", "INV", "PLACED", "N", 10, 20, 1210, 2420)
    );
    let second = &rows[1];
    // E orientation swaps width/height: 30..2430 x 40..1240.
    assert_eq!(
        (second.llx, second.lly, second.urx, second.ury),
        (30, 40, 2430, 1240)
    );
}

#[test]
fn test_dbu_per_micron_defaults_to_1000() {
    let tmp = temp_dir("core_api");
    let def = write_def(
        &tmp,
        "VERSION 5.8 ;\nCOMPONENTS 1 ;\n  - u1 INV + PLACED ( 0 0 ) N ;\nEND COMPONENTS\n",
    );
    assert_eq!(dbu_per_micron(&def).unwrap(), 1000);
}

#[test]
fn test_dbu_per_micron_after_vias_section() {
    let tmp = temp_dir("core_api");
    let def = write_def(
        &tmp,
        "VERSION 5.8 ;\nDESIGN d ;\nUNITS DISTANCE MICRONS 2000 ;\n\
         VIAS 1 ;\n  - via1 + RECT Metal1 ( -100 -100 ) ( 100 100 ) + RECT Metal2 ( -100 -100 ) ( 100 100 ) ;\nEND VIAS\n\
         COMPONENTS 1 ;\n  - u1 INV + PLACED ( 0 0 ) N ;\nEND COMPONENTS\nEND DESIGN\n",
    );
    assert_eq!(dbu_per_micron(&def).unwrap(), 2000);
}

#[test]
fn test_dbu_per_micron_units_after_section_end() {
    let tmp = temp_dir("core_api");
    let def = write_def(
        &tmp,
        "VERSION 5.8 ;\nPROPERTYDEFINITIONS\n  COMPONENT weight INTEGER ;\nEND PROPERTYDEFINITIONS\n\
         UNITS DISTANCE MICRONS 2000 ;\n\
         COMPONENTS 1 ;\n  - u1 INV + PLACED ( 0 0 ) N ;\nEND COMPONENTS\nEND DESIGN\n",
    );
    assert_eq!(dbu_per_micron(&def).unwrap(), 2000);
}

#[test]
fn test_dbu_per_micron_without_components() {
    let tmp = temp_dir("core_api");
    let def = write_def(
        &tmp,
        "VERSION 5.8 ;\nDESIGN d ;\nUNITS DISTANCE MICRONS 4000 ;\nNETS 0 ;\nEND NETS\nEND DESIGN\n",
    );
    assert_eq!(dbu_per_micron(&def).unwrap(), 4000);
}

#[test]
fn test_scan_without_components_errors() {
    let tmp = temp_dir("core_api");
    let def = write_def(&tmp, "VERSION 5.8 ;\n");
    let lef = write_lef(&tmp);
    let err = match scan_instance_bboxes(&def, &[lef]) {
        Ok(_) => panic!("expected an error for a DEF without COMPONENTS"),
        Err(err) => err,
    };
    assert_eq!(err.to_string(), "DEF missing COMPONENTS section");
}

#[test]
fn test_scan_unknown_macro_errors() {
    let tmp = temp_dir("core_api");
    let def = write_def(
        &tmp,
        "VERSION 5.8 ;\nUNITS DISTANCE MICRONS 1000 ;\nCOMPONENTS 1 ;\n  - u1 NOPE + PLACED ( 0 0 ) N ;\nEND COMPONENTS\n",
    );
    let lef = write_lef(&tmp);
    let mut iter = scan_instance_bboxes(&def, &[lef]).unwrap();
    let err = iter
        .next()
        .expect("one row")
        .expect_err("unknown macro must fail");
    let message = err.to_string();
    assert!(
        message.contains("missing LEF macro 'NOPE' for instance 'u1'"),
        "{message}"
    );
    // The error is the last item: iteration terminates instead of silently
    // skipping the failed batch's rows.
    assert!(iter.next().is_none());
}

#[test]
fn test_net_shapes_terminate_after_error() {
    let tmp = temp_dir("core_api");
    let def = write_def(
        &tmp,
        "VERSION 5.8 ;\nUNITS DISTANCE MICRONS 1000 ;\nCOMPONENTS 1 ;\n\
  - u1 INV + PLACED ( 0 0 ) N ;\nEND COMPONENTS\n\
NETS 1 ;\n  - n1 ( u1 A ) + ROUTED met2 ( 0 0 ) ( bad * ) ;\nEND NETS\n",
    );
    let mut iter = scan_net_shapes(&def, "n1", "met2").unwrap();
    let err = iter
        .next()
        .expect("one item")
        .expect_err("bad coordinate must fail");
    assert!(
        err.to_string().contains("invalid route point coordinate"),
        "{err}"
    );
    assert!(iter.next().is_none());
}

#[test]
fn test_macro_sizes_microns() {
    let tmp = temp_dir("core_api");
    let lef = write_lef(&tmp);
    let sizes = macro_sizes(&[lef]).unwrap();
    assert_eq!(sizes.len(), 1);
    assert_eq!(
        sizes.get("INV").map(|size| (size.width_um, size.height_um)),
        Some((1.2, 2.4))
    );
}

#[test]
fn test_scan_thread_count_does_not_change_output() {
    let tmp = temp_dir("core_api");
    let def = write_def(&tmp, DEF);
    let lef = write_lef(&tmp);
    let rows = |lefs: &[PathBuf]| -> Vec<(String, i64, i64)> {
        scan_instance_bboxes(&def, lefs)
            .unwrap()
            .map(|row| {
                let row = row.unwrap();
                (row.instance.clone(), row.llx, row.lly)
            })
            .collect()
    };
    let default = rows(&[lef.clone()]);
    set_threads(Some(1)).unwrap();
    let limited = rows(&[lef.clone()]);
    set_threads(None).unwrap();
    assert_eq!(default, limited);
}

#[test]
fn test_scan_net_shapes_and_skipped() {
    let tmp = temp_dir("core_api");
    let def = write_def(
        &tmp,
        "VERSION 5.8 ;\nUNITS DISTANCE MICRONS 1000 ;\nCOMPONENTS 2 ;\n\
  - u1 INV + PLACED ( 0 0 ) N ;\n  - u2 INV + FIXED ( 10 10 ) N ;\nEND COMPONENTS\n\
NETS 1 ;\n  - n1 ( u1 A ) ( u2 B ) + ROUTED met2 ( 0 0 ) ( 100 * ) via12 ( 100 50 ) ( 200 * ) ;\nEND NETS\n",
    );
    let mut iter = scan_net_shapes(&def, "n1", "met2").unwrap();
    let row = iter.next().unwrap().unwrap();
    assert_eq!(
        (row.x1, row.y1, row.x2, row.y2, row.width),
        (0, 0, 100, 0, 0)
    );
    assert!(iter.next().is_none());
    assert_eq!(
        iter.skipped(),
        SkippedRoutes {
            after_via: 2,
            polygon: 0,
            subnet: 0
        }
    );
}

const LIB: &str = "library (tiny) {\n\
  cell (FF1) {\n    area : 10.5 ;\n    ff (IQ, IQN) { clocked_on : CK ; next_state : D ; }\n  }\n\
  cell (PLAIN) {\n    area : 3 ;\n    dont_use : true ;\n  }\n\
}\n";

#[test]
fn test_cell_attributes_iter() {
    let tmp = temp_dir("core_api");
    let lib = write(&tmp, "tiny.lib", LIB);
    let cells: Vec<CellAttributes> = cell_attributes(&lib).unwrap().map(|c| c.unwrap()).collect();
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0].name, "FF1");
    assert!(cells[0].is_sequential);
    assert_eq!(
        cells[0].attributes,
        vec![("area".to_string(), "10.5".to_string())]
    );
    assert_eq!(cells[1].name, "PLAIN");
    assert!(!cells[1].is_sequential);
    assert_eq!(
        cells[1].attributes,
        vec![
            ("area".to_string(), "3".to_string()),
            ("dont_use".to_string(), "true".to_string())
        ]
    );
}

#[test]
fn test_attributes_on_late_declared_units() {
    let tmp = temp_dir("core_api");
    let lib = write(
        &tmp,
        "late.lib",
        "library (tiny) {\n\
  cell (PLAIN) { area : 3 ; }\n\
  voltage_unit : \"1V\" ;\n  time_unit : \"1ns\" ;\n}\n",
    );
    let index = LibraryIndex::open(&lib).unwrap();
    let cell = index.cell("PLAIN").unwrap();
    assert_eq!(cell.units.time, Some(1e-9));
}

#[test]
fn test_attributes_iter_survives_error_cell() {
    let tmp = temp_dir("core_api");
    let lib = write(
        &tmp,
        "broken.lib",
        "library (tiny) {\n\
  cell (GOOD) { area : 1 ; }\n\
  cell (BROKEN) { ( 1 2 ) ; }\n\
  cell (ALSO_GOOD) { area : 2 ; }\n\
}\n",
    );
    let mut iter = cell_attributes(&lib).unwrap();
    assert_eq!(iter.next().unwrap().unwrap().name, "GOOD");
    assert!(iter.next().unwrap().is_err());
    assert_eq!(iter.next().unwrap().unwrap().name, "ALSO_GOOD");
    assert!(iter.next().is_none());
}

#[test]
fn test_library_index_open_cell_and_source() {
    let tmp = temp_dir("core_api");
    let lib = write(&tmp, "tiny.lib", LIB);
    let index = LibraryIndex::open(&lib).unwrap();
    assert_eq!(index.library_name(), "tiny");
    assert_eq!(
        index.cell_names(),
        vec!["FF1".to_string(), "PLAIN".to_string()]
    );
    assert_eq!(index.num_cells(), 2);
    let cell = index.cell("FF1").unwrap();
    assert!(cell.ff.is_some());
    let source = index.cell_source("PLAIN").unwrap();
    assert!(source.contains("dont_use"));
    assert!(index.cell("MISSING").is_err());
    assert!(index.cell_source("MISSING").is_err());
}

#[test]
fn test_macro_sizes_respects_duplicate_last_wins() {
    let tmp = temp_dir("core_api");
    let lef = write(
        &tmp,
        "dups.lef",
        "MACRO M\n SIZE 1 BY 1 ;\nEND M\nMACRO M\n SIZE 2 BY 2 ;\nEND M\n",
    );
    let sizes = macro_sizes(&[lef]).unwrap();
    assert_eq!(sizes.len(), 1);
    let MacroSize {
        width_um,
        height_um,
    } = sizes["M"];
    assert_eq!((width_um, height_um), (2.0, 2.0));
}

#[test]
fn test_open_rejects_non_utf8_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let path = PathBuf::from(OsStr::from_bytes(b"not-utf8-\xff.lib"));
    let err = match LibraryIndex::open(&path) {
        Ok(_) => panic!("non-UTF-8 path must not open"),
        Err(err) => err,
    };
    assert!(format!("{err:?}").contains("not valid UTF-8"), "{err:?}");
    assert!(cell_attributes(&path).is_err());
}
