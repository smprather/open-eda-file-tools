//! Exercise SPEF/DSPF from the same API available to downstream Rust crates.

use std::fs::File;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;

use flate2::{Compression, write::GzEncoder};
use open_eda_file_tools_core::{Error, pex};
use pex::SummaryMode;

const SPEF: &str = r#"*SPEF "IEEE 1481-1999"
*DESIGN "tiny"
*DELIMITER :
*T_UNIT 1 NS
*C_UNIT 1 PF
*R_UNIT 1 OHM
*NAME_MAP
*1 clk
*2 u1
*PORTS
*P *1 I
*D_NET *1 0.5
*CONN
*P *1 I
*I *2:A I *D buf *C 10 20
*CAP
1 *1 0.125
2 *1 *2:A 0.375
*RES
1 *1 *2:A 3.5
2 *2:A *2:B 1.5
*END
*R_NET next 0.25
*END
"#;

const DSPF: &str = r#"*DSPF "1.0"
*DESIGN "zip"
*C_UNIT 1 FF
*D_NET w 0.75 $ layer=M1
*CONN
*I instance:A I *C 1.5 2.5
*CAP
1 w 0.25
2 w instance:A 0.5 $ layer=M2
*RES
1 w instance:A 7 $ layer=M1
*END
"#;

fn scratch_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pex_api_{}_{}", std::process::id(), name))
}

#[test]
fn spef_metadata_and_per_net_rc() {
    let path = scratch_path("tiny.spef");
    std::fs::write(&path, SPEF).unwrap();

    let header = pex::header(&path).unwrap();
    assert_eq!(header.standard, "IEEE 1481-1999");
    assert_eq!(header.design, "tiny");
    assert_eq!(header.c_unit, "1 PF");
    assert_eq!(header.r_unit, "1 OHM");
    assert_eq!(header.ports, [("clk".to_owned(), "I".to_owned())]);

    let parsed = pex::parse(&path, true).unwrap();
    assert_eq!(parsed.header, header);
    let net = &parsed.nets[0];
    assert_eq!(net.name, "clk");
    assert_eq!(net.conns[1].name, "u1:A");
    assert_eq!(net.caps[0].node2, None);
    assert_eq!(net.caps[1].node2.as_deref(), Some("u1:A"));
    assert_eq!(net.res[1].node2, "u1:B");

    let summaries = pex::summaries(&path, SummaryMode::Parallel).unwrap();
    assert_eq!(
        pex::summaries(&path, SummaryMode::Stream).unwrap(),
        summaries
    );
    let names: Vec<_> = summaries.iter().map(|net| net.name.as_str()).collect();
    assert_eq!(names, ["clk", "next"]);
    let rc = &summaries[0];
    assert_eq!(
        (rc.total_cap, rc.ground_cap, rc.coupling_cap),
        (0.5, 0.125, 0.375)
    );
    assert_eq!((rc.cap_count, rc.res_count, rc.res_total), (2, 2, 5.0));

    let histogram = pex::histogram(&path, 2, SummaryMode::Stream).unwrap();
    assert_eq!(histogram.bounds, [0.25, 0.375, 0.5]);
    assert_eq!(histogram.counts, [1, 1]);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn gzipped_dspf_keeps_extensions() {
    let path = scratch_path("tiny.dspf.gz");
    let file = File::create(&path).unwrap();
    let mut encoder = GzEncoder::new(file, Compression::default());
    encoder.write_all(DSPF.as_bytes()).unwrap();
    encoder.finish().unwrap();

    let header = pex::header(&path).unwrap();
    assert_eq!(header.standard, "1.0");
    assert_eq!(header.design, "zip");

    let parsed = pex::parse(&path, true).unwrap();
    let net = &parsed.nets[0];
    assert_eq!(net.ext.as_deref(), Some("layer=M1"));
    assert_eq!((net.conns[0].x, net.conns[0].y), (Some(1.5), Some(2.5)));
    assert_eq!(net.caps[1].ext.as_deref(), Some("layer=M2"));
    assert_eq!(net.res[0].value, 7.0);

    let summary = &pex::summaries(&path, SummaryMode::Stream).unwrap()[0];
    assert_eq!(
        (summary.total_cap, summary.coupling_cap, summary.res_total),
        (0.75, 0.5, 7.0)
    );
    let parsed = pex::parse(&path, false).unwrap();
    let net = &parsed.nets[0];
    assert_eq!(
        (net.total_cap, net.ground_cap, net.coupling_cap),
        (0.75, 0.25, 0.5)
    );
    assert!(net.conns.is_empty() && net.caps.is_empty() && net.res.is_empty());

    std::fs::remove_file(path).unwrap();
}

#[test]
fn reports_missing_file_and_unterminated_net() {
    let missing = scratch_path("missing.spef.gz");
    let err = pex::header(&missing).unwrap_err();
    assert!(matches!(&err, Error::Io(err) if err.kind() == ErrorKind::NotFound));
    assert!(err.to_string().contains(&missing.display().to_string()));

    let path = scratch_path("malformed.spef");
    std::fs::write(&path, "*DESIGN d\n*D_NET n 1\n").unwrap();
    assert_eq!(pex::header(&path).unwrap().design, "d");
    for mode in [SummaryMode::Parallel, SummaryMode::Stream] {
        let err = pex::summaries(&path, mode).unwrap_err();
        assert!(
            err.to_string()
                .contains("unterminated net block (missing *END)")
        );
    }
    std::fs::remove_file(path).unwrap();
}
