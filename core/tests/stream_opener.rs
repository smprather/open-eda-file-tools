use std::io::{self, Cursor, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Once;

use flate2::{Compression, write::GzEncoder};
use open_eda_file_tools_core::{Error, def, input, lef, liberty, pex};

const FIRST_LEF: &str = "MACRO BUF CLASS CORE ; SIZE 2 BY 3 ; END BUF";
const SECOND_LEF: &str = "MACRO INV CLASS CORE ; SIZE 1 BY 3 ; END INV";
const DEF: &str = "\
    UNITS DISTANCE MICRONS 2000 ;\n\
    DIEAREA ( 0 0 ) ( 20000 20000 ) ;\n\
    COMPONENTS 1 ;\n\
    - u1 BUF + PLACED ( 4000 5000 ) N ;\n\
    END COMPONENTS\n\
    END DESIGN\n";
const SPEF: &str = "\
    *SPEF \"IEEE 1481-1999\"\n\
    *DESIGN \"virtual\"\n\
    *C_UNIT 1 PF\n\
    *D_NET clk 0.5\n\
    *END\n";

fn install_opener() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        input::set_stream_opener(Box::new(|path| {
            let source = match path.to_str() {
                Some("virtual/first.lef.bz2") => FIRST_LEF,
                Some("virtual/second.lef.zst") => SECOND_LEF,
                Some("virtual/chip.def.xz") => DEF,
                Some("virtual/top.def") => {
                    "COMPONENTS 2 ; - a A + PLACED ( 0 0 ) N ; - b B + PLACED ( 4000 0 ) N ; END COMPONENTS"
                }
                Some("virtual/block.def") => {
                    "DIEAREA ( 0 0 ) ( 1000 1000 ) ; COMPONENTS 0 ; END COMPONENTS"
                }
                Some("virtual//block.def") => {
                    "DIEAREA ( 0 0 ) ( 2000 1000 ) ; COMPONENTS 0 ; END COMPONENTS"
                }
                Some("virtual/cells.lib.zst") => "library (virtual) { cell (BUF) { area : 2; } }",
                Some("virtual/chip.spef.xz") => SPEF,
                Some("virtual/denied.def") => return Err(ErrorKind::PermissionDenied.into()),
                _ => return Ok(None),
            };
            let reader: Box<dyn Read + Send> = Box::new(Cursor::new(source.as_bytes()));
            Ok(Some(reader))
        }))
        .unwrap();
    });
}

#[test]
fn reads_virtual_lefs_and_def_through_public_scanners() {
    install_opener();
    let lefs = [
        PathBuf::from("virtual/first.lef.bz2"),
        PathBuf::from("virtual/second.lef.zst"),
    ];
    let library = lef::parse_cell_lef(&lefs).unwrap();
    let names: Vec<_> = library.macros().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["BUF", "INV"]);

    let path = Path::new("virtual/chip.def.xz");
    let rows = def::scan_instance_bboxes(path, &lefs)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].instance, "u1");
    assert_eq!(
        (rows[0].llx, rows[0].lly, rows[0].urx, rows[0].ury),
        (4000, 5000, 8000, 11000)
    );

    let hierarchy = def::read_def_hierarchy(path, &lefs, &[]).unwrap();
    assert_eq!(hierarchy.dbu_per_micron(), 2000);
    assert_eq!(hierarchy.instances().count(), 1);
}

#[test]
fn hierarchy_preserves_virtual_paths_distinguished_by_the_opener() {
    install_opener();
    let blocks = [
        ("A".to_owned(), PathBuf::from("virtual/block.def")),
        ("B".to_owned(), PathBuf::from("virtual//block.def")),
    ];
    let hierarchy = def::read_def_hierarchy(Path::new("virtual/top.def"), &[], &blocks).unwrap();
    let instances: Vec<_> = hierarchy.instances().collect::<Result<_, _>>().unwrap();
    let widths: Vec<_> = instances.iter().map(|bbox| bbox.urx - bbox.llx).collect();
    assert_eq!(widths, [1000, 2000]);
}

#[test]
fn opens_virtual_liberty_and_both_pex_summary_modes() {
    install_opener();
    let index = liberty::LibraryIndex::open(Path::new("virtual/cells.lib.zst")).unwrap();
    assert_eq!(index.library_name(), "virtual");
    assert_eq!(index.cell_names(), ["BUF"]);

    let path = Path::new("virtual/chip.spef.xz");
    assert_eq!(pex::header(path).unwrap().design, "virtual");
    for mode in [pex::SummaryMode::Parallel, pex::SummaryMode::Stream] {
        let summaries = pex::summaries(path, mode).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].name, "clk");
        assert_eq!(summaries[0].total_cap, 0.5);
    }
}

#[test]
fn declining_falls_back_to_magic_byte_gzip_detection() -> io::Result<()> {
    install_opener();
    let path = std::env::temp_dir().join(format!("stream_opener_{}.def", std::process::id()));
    let file = std::fs::File::create(&path)?;
    let mut encoder = GzEncoder::new(file, Compression::default());
    encoder.write_all(DEF.as_bytes())?;
    encoder.finish()?;

    let units = def::dbu_per_micron(&path);
    std::fs::remove_file(&path)?;
    assert_eq!(units.unwrap(), 2000);
    Ok(())
}

#[test]
fn keeps_callback_errors_and_rejects_replacing_the_opener() {
    install_opener();
    let Error::Io(error) = def::dbu_per_micron(Path::new("virtual/denied.def")).unwrap_err() else {
        panic!("expected an I/O error");
    };
    assert_eq!(error.kind(), ErrorKind::PermissionDenied);
    let error = input::set_stream_opener(Box::new(|_| Ok(None))).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::AlreadyExists);
    assert_eq!(
        def::dbu_per_micron(Path::new("virtual/chip.def.xz")).unwrap(),
        2000
    );
}
