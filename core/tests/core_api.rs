//! Integration tests for the public plain-Rust API of `open_eda_file_tools_core`
//! DEF instance bboxes and clock objects, LEF cells, and Liberty
//! attributes and LibraryIndex.

use std::path::{Path, PathBuf};

use open_eda_file_tools_core::Error;
use open_eda_file_tools_core::def::{
    ClockObjectResolution, MacroPinRole, SkippedRoutes, clock_object_instances, dbu_per_micron,
    scan_instance_bboxes, scan_instance_bboxes_with_library, scan_net_shapes,
};
use open_eda_file_tools_core::lef::{MacroSize, macro_sizes, parse_cell_lef};
use open_eda_file_tools_core::liberty::{
    CellAttributes, LibraryIndex, ParseLoadError, cell_attributes,
};
use open_eda_file_tools_core::threads::{self, set_threads};

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
    let err = scan_instance_bboxes(&def, &[lef]).unwrap_err();
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
fn test_cell_lef_exposes_pin_metadata_from_last_macro_definition() {
    let tmp = temp_dir("cell_lef");
    let first = write_lef(&tmp);
    let second = write(
        &tmp,
        "updated.lef",
        "MACRO INV\n\
         ORIGIN -0.2 0.3 ;\nSIZE 1.25 BY 2.5 ;\n\
         PIN A\nDIRECTION INPUT ;\nEND A\n\
         PIN Y\nDIRECTION OUTPUT TRISTATE ;\nEND Y\n\
         PIN VPWR\nDIRECTION INOUT ;\nUSE POWER ;\nEND VPWR\n\
         PIN VGND\nUSE GROUND ;\nEND VGND\n\
         END INV\n",
    );

    let library = parse_cell_lef(&[first, second]).unwrap();
    assert_eq!(library.macros().len(), 1);
    assert_eq!(library.duplicate_macro_names(), ["INV"]);
    let cell = library.macro_by_name("INV").unwrap();
    assert_eq!(cell.name, "INV");
    assert_eq!(
        (cell.width, cell.height, cell.origin_x, cell.origin_y),
        (1.25, 2.5, -0.2, 0.3)
    );
    let pins: Vec<_> = cell
        .pins()
        .iter()
        .map(|pin| {
            (
                pin.name.as_str(),
                pin.direction.as_deref(),
                pin.use_kind.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        pins,
        [
            ("A", Some("input"), None),
            ("Y", Some("output"), None),
            ("VPWR", Some("inout"), Some("power")),
            ("VGND", None, Some("ground")),
        ]
    );
    assert_eq!(cell.pin("VPWR").unwrap().use_kind.as_deref(), Some("power"));
}

#[test]
fn test_scan_instance_bboxes_reuses_lef_after_source_is_removed() {
    let tmp = temp_dir("bbox_library");
    let lef = write_lef(&tmp);
    let def = write_def(&tmp, DEF);
    let def_2000 = write(&tmp, "other.def", &DEF.replace("1000", "2000"));
    let library = parse_cell_lef(std::slice::from_ref(&lef)).unwrap();
    let path_rows: Vec<_> = scan_instance_bboxes(&def, std::slice::from_ref(&lef))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    std::fs::remove_file(lef).unwrap();

    let shared_rows: Vec<_> = scan_instance_bboxes_with_library(&def, &library)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(shared_rows, path_rows);
    let scaled_rows: Vec<_> = scan_instance_bboxes_with_library(&def_2000, &library)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!((scaled_rows[0].urx, scaled_rows[0].ury), (2410, 4820));
    assert_eq!((scaled_rows[1].urx, scaled_rows[1].ury), (4830, 2440));
}

#[test]
fn test_clock_object_resolution_classifies_explicit_nets_and_pins() {
    let tmp = temp_dir("clock_objects");
    let path = write_def(
        &tmp,
        r"COMPONENTS 5 ;
  - u\[0\] BUF ;
  - z\/buf BUF ;
  - sink BUF ;
  - ff FF ;
  - unknown NO_ROLE ;
END COMPONENTS
NETS 1 ;
  - n\/1 ( u\[0\] Y ) ( sink A ) ( ff Q ) ( unknown X ) ( PIN clk ) ;
END NETS
",
    );
    let macros = [
        MacroPinRole {
            macro_name: "BUF".into(),
            inputs: vec!["A".into()],
            outputs: vec!["Y".into()],
            endpoint: false,
            clock_pins: vec![],
        },
        MacroPinRole {
            macro_name: "FF".into(),
            inputs: vec!["CK".into()],
            outputs: vec!["Q".into()],
            endpoint: true,
            clock_pins: vec!["CK".into()],
        },
    ];
    let nets = ["missing_z".into(), "n/1".into(), "missing_a".into()];
    let pins = [
        ("z/buf".into(), "A".into()),
        (r"z\/buf".into(), "Y".into()),
        ("u[0]".into(), "UNKNOWN_PIN".into()),
        ("ff".into(), "CK".into()),
        ("-".into(), "clk".into()),
        ("gone".into(), "A".into()),
        ("-".into(), "missing_port".into()),
    ];
    let resolution = clock_object_instances(&path, &nets, &pins, &macros).unwrap();
    assert_eq!(
        resolution,
        ClockObjectResolution {
            instances: vec!["ff".into(), r"u\[0\]".into(), r"z\/buf".into()],
            unmatched_nets: vec!["missing_z".into(), "missing_a".into()],
            unmatched_pins: vec![
                ("gone".into(), "A".into()),
                ("-".into(), "missing_port".into()),
            ],
            unclassified: vec!["unknown".into()],
        }
    );
}

#[test]
fn test_clock_object_resolution_keeps_tokenizer_errors_typed() {
    let tmp = temp_dir("clock_error");
    let path = write_def(&tmp, "NETS 1 ;\n  - n ( PIN A ) + ROUTED M1 ( 1 2 )");
    let err = clock_object_instances(&path, &[], &[], &[]).unwrap_err();
    assert!(
        matches!(&err, Error::Io(io) if io.kind() == std::io::ErrorKind::UnexpectedEof),
        "{err:?}"
    );
}

#[test]
fn test_scan_thread_count_does_not_change_output() {
    // 10,000 components span several scan chunks, so the parallel path runs
    // and a row-order or content bug under a different thread count shows.
    const COMPONENTS: i64 = 10_000;
    let mut text =
        format!("VERSION 5.8 ;\nUNITS DISTANCE MICRONS 1000 ;\nCOMPONENTS {COMPONENTS} ;\n");
    for i in 0..COMPONENTS {
        let (macro_name, orient) = if i % 3 == 0 {
            ("BUF", "FS")
        } else {
            ("INV", "N")
        };
        text.push_str(&format!(
            "  - u{i} {macro_name} + PLACED ( {} {} ) {orient} ;\n",
            i * 7,
            (i % 97) * 13
        ));
    }
    text.push_str("END COMPONENTS\n");

    let tmp = temp_dir("core_api");
    let def = write_def(&tmp, &text);
    let lef = write(
        &tmp,
        "two_macros.lef",
        "VERSION 5.8 ;\nMACRO INV\n  SIZE 1.2 BY 2.4 ;\nEND INV\n\
MACRO BUF\n  SIZE 2.0 BY 2.4 ;\nEND BUF\n",
    );
    let rows = || -> Vec<(String, String, i64, i64, i64, i64)> {
        scan_instance_bboxes(&def, &[lef.clone()])
            .unwrap()
            .map(|row| {
                let row = row.unwrap();
                (
                    row.instance.clone(),
                    row.macro_name.clone(),
                    row.llx,
                    row.lly,
                    row.urx,
                    row.ury,
                )
            })
            .collect()
    };

    let default = rows();
    assert_eq!(default.len(), COMPONENTS as usize);
    assert_eq!(default[0].0, "u0");
    assert_eq!(default[9_999].0, "u9999");

    let outcome = std::panic::catch_unwind(|| {
        set_threads(Some(1)).unwrap();
        assert_eq!(threads::get_threads(), 1);
        let one = rows();
        set_threads(Some(3)).unwrap();
        let three = rows();
        (one, three)
    });
    set_threads(None).unwrap();
    // The error is a real `std::error::Error` (what `anyhow` requires), not a
    // bare `String`.
    fn into_error<E: std::error::Error + Send + Sync + 'static>(
        result: Result<(), E>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(result?)
    }
    let rejected = into_error(set_threads(Some(5000))).expect_err("5000 must be rejected");
    assert_eq!(
        rejected.to_string(),
        "threads must be between 1 and 4096, got 5000"
    );
    set_threads(None).unwrap();
    let (one, three) = outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    assert_eq!(default, one);
    assert_eq!(default, three);
    assert_eq!(default, rows());
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
#[cfg(unix)]
fn test_open_rejects_non_utf8_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let path = PathBuf::from(OsStr::from_bytes(b"not-utf8-\xff.lib"));
    let err = LibraryIndex::open(&path).unwrap_err();
    assert!(format!("{err:?}").contains("not valid UTF-8"), "{err:?}");
    assert!(cell_attributes(&path).is_err());
}

fn open_index(path: &Path) -> Result<LibraryIndex, Box<dyn std::error::Error>> {
    Ok(LibraryIndex::open(path)?)
}

fn cell_of(index: &LibraryIndex, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    index.cell(name)?;
    Ok(())
}

fn source_of(index: &LibraryIndex, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(index.cell_source(name)?)
}

fn dbu_of(path: &Path) -> Result<i64, Box<dyn std::error::Error>> {
    Ok(dbu_per_micron(path)?)
}

#[test]
fn test_errors_convert_to_box_dyn_error() {
    let tmp = temp_dir("box_err");
    let missing = tmp.join("missing");
    // The wrappers are transparent: the I/O failure is reachable by
    // downcasting, and is not repeated as `source()` (anyhow's `{:#}` would
    // print the same text twice).
    let open_err = open_index(&missing).unwrap_err();
    match open_err.downcast_ref::<ParseLoadError>() {
        Some(ParseLoadError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        other => panic!("expected ParseLoadError::Io, got {other:?}"),
    }
    assert!(open_err.source().is_none());
    let dbu_err = dbu_of(&missing).unwrap_err();
    match dbu_err.downcast_ref::<Error>() {
        Some(Error::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        other => panic!("expected Error::Io, got {other:?}"),
    }
    assert!(dbu_err.source().is_none());

    let lib = write(&tmp, "tiny.lib", LIB);
    let index = LibraryIndex::open(&lib).unwrap();
    assert_eq!(
        cell_of(&index, "NOPE").unwrap_err().to_string(),
        "unknown cell \"NOPE\""
    );
    assert_eq!(
        source_of(&index, "NOPE").unwrap_err().to_string(),
        "unknown cell \"NOPE\""
    );
}

#[test]
fn test_missing_files_report_not_found() {
    use std::io::ErrorKind;

    let tmp = temp_dir("not_found");
    let missing = tmp.join("missing");
    let lef = write_lef(&tmp);

    fn kind(err: Error) -> ErrorKind {
        match err {
            Error::Io(e) => e.kind(),
            other => panic!("expected an io error, got {other:?}"),
        }
    }
    let err = scan_instance_bboxes(&missing, &[lef]).unwrap_err();
    assert_eq!(kind(err), ErrorKind::NotFound);
    let def = write_def(&tmp, DEF);
    let err = scan_instance_bboxes(&def, std::slice::from_ref(&missing)).unwrap_err();
    assert_eq!(kind(err), ErrorKind::NotFound);
    assert_eq!(
        kind(dbu_per_micron(&missing).unwrap_err()),
        ErrorKind::NotFound
    );
    let err = scan_net_shapes(&missing, "n", "M1").unwrap_err();
    assert_eq!(kind(err), ErrorKind::NotFound);
    assert_eq!(
        kind(macro_sizes(&[missing.clone()]).unwrap_err()),
        ErrorKind::NotFound
    );
    assert_eq!(
        kind(parse_cell_lef(std::slice::from_ref(&missing)).unwrap_err()),
        ErrorKind::NotFound
    );
    assert_eq!(
        kind(clock_object_instances(&missing, &[], &[], &[]).unwrap_err()),
        ErrorKind::NotFound
    );

    for result in [
        LibraryIndex::open(&missing).unwrap_err(),
        cell_attributes(&missing).unwrap_err(),
    ] {
        match result {
            ParseLoadError::Io(e) => assert_eq!(e.kind(), ErrorKind::NotFound),
            other => panic!("expected an io error, got {other}"),
        }
    }
}
