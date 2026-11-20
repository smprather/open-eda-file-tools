use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::Compression;
use flate2::write::GzEncoder;
use open_eda_file_tools_core::lef::{parse_cell_lef, parse_cell_lef_metadata};
use open_eda_file_tools_core::threads::set_threads;
use std::io::Write;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "issue16_lef_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn gzip(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        let mut encoder =
            GzEncoder::new(std::fs::File::create(&path).unwrap(), Compression::fast());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn targeted_modes_extract_macro_site_and_optional_pin_metadata() {
    let tmp = Scratch::new();
    let path = tmp.write(
        "cells.lef",
        r#"
VERSION 5.8 ;
DIVIDERCHAR "/" ;
BUSBITCHARS "[]" ;
PROPERTYDEFINITIONS
  MACRO SITE STRING ;
END PROPERTYDEFINITIONS
LAYER SITE
  TYPE ROUTING ;
  PROPERTY key "END" SITE ; # A layer name and property value that resemble syntax.
END SITE
VIA testVia DEFAULT
  LAYER SITE ;
  RECT 0 0 1 1 ;
END testVia
SITE CoreSite
  CLASS CORE ;
  SYMMETRY X Y ;
  SIZE .25 BY 1.2 ;
END CoreSite
MACRO Buf
  CLASS core spacer ;
  ORIGIN -.1 +.2 ;
  SIZE 1.25 BY 2.5 ;
  SITE CoreSite ;
  PIN A
    DIRECTION INPUT ;
    USE SIGNAL ;
    PORT
      LAYER SITE ; # END A
      RECT malformed coordinates need no conversion ;
      PROPERTY shape "closing quote protects ; END A" ;
    END
    PORT
      VIA 0 0 testVia ;
    END
  END A
  PIN y
    DIRECTION OUTPUT TRISTATE ;
    USE POWER ;
  END y
  OBS LAYER SITE ; RECT 0 0 1 1 ; END
END Buf
MACRO Bare SIZE 2 BY 3 ; END Bare
END LIBRARY
"#,
    );

    for include_pins in [false, true] {
        let library = parse_cell_lef_metadata(std::slice::from_ref(&path), include_pins).unwrap();
        let buf = library.macro_by_name("Buf").unwrap();
        assert_eq!(
            (buf.width, buf.height, buf.origin_x, buf.origin_y),
            (1.25, 2.5, -0.1, 0.2),
        );
        assert_eq!(buf.class.as_deref(), Some("CORE SPACER"));
        assert_eq!(buf.site.as_deref(), Some("CoreSite"));
        assert_eq!(library.sites().len(), 1);
        let site = library.site_by_name("CoreSite").unwrap();
        assert_eq!(
            (site.name.as_str(), site.width, site.height),
            ("CoreSite", 0.25, 1.2)
        );
        assert!(library.site_by_name("coresite").is_none());
        let bare = library.macro_by_name("Bare").unwrap();
        assert_eq!((bare.origin_x, bare.origin_y), (0.0, 0.0));
        assert!(bare.class.is_none() && bare.site.is_none());

        if include_pins {
            let pins: Vec<_> = buf.pins().iter().map(|pin| pin.name.as_str()).collect();
            assert_eq!(pins, ["A", "y"]);
            let input = buf.pin("A").unwrap();
            assert_eq!(input.direction.as_deref(), Some("input"));
            assert_eq!(input.use_kind.as_deref(), Some("signal"));
            assert!(input.ports.is_empty());
            let output = buf.pin("y").unwrap();
            assert_eq!(output.direction.as_deref(), Some("output"));
            assert_eq!(output.use_kind.as_deref(), Some("power"));
        } else {
            assert!(buf.pins().is_empty());
        }
    }

    let err = parse_cell_lef(&[path]).unwrap_err().to_string();
    assert!(
        err.contains("MACRO Buf PIN A: malformed RECT coordinate"),
        "{err}"
    );
}

#[test]
fn macro_only_mode_does_not_materialize_pin_bodies() {
    let tmp = Scratch::new();
    let path = tmp.write(
        "duplicates.lef",
        "MACRO P SIZE 1 BY 2 ; PIN X END X PIN X END X END P",
    );
    let library = parse_cell_lef_metadata(std::slice::from_ref(&path), false).unwrap();
    assert!(library.macro_by_name("P").unwrap().pins().is_empty());
    let err = parse_cell_lef_metadata(&[path], true)
        .unwrap_err()
        .to_string();
    assert!(err.contains("MACRO P duplicate PIN X"), "{err}");
}

#[test]
fn targeted_merge_keeps_path_order_for_macros_sites_and_errors() {
    let tmp = Scratch::new();
    let first = tmp.write(
        "first.lef",
        "MACRO A SIZE 1 BY 2 ; END A MACRO B SIZE 3 BY 4 ; END B
         SITE S SIZE 1 BY 2 ; END S SITE T SIZE 2 BY 3 ; END T",
    );
    let second = tmp.gzip(
        "second.LEF.GZ",
        "MACRO C SIZE 1 BY 2 ; END C MACRO B SIZE 5 BY 6 ; END B
         MACRO A SIZE 2 BY 3 ; END A MACRO A SIZE 3 BY 4 ; END A
         SITE U SIZE 3 BY 4 ; END U SITE S SIZE 5 BY 6 ; END S",
    );
    let first_bad = tmp.write("first-bad.lef", "SITE Bad SIZE 1 OF 2 ; END Bad");
    let missing = tmp.0.join("missing.lef");
    for thread_count in [1, 3] {
        set_threads(Some(thread_count)).unwrap();
        let library = parse_cell_lef_metadata(&[first.clone(), second.clone()], false).unwrap();
        let macros: Vec<_> = library
            .macros()
            .iter()
            .map(|cell| (cell.name.as_str(), cell.width))
            .collect();
        assert_eq!(macros, [("A", 3.0), ("B", 5.0), ("C", 1.0)]);
        assert_eq!(library.duplicate_macro_names(), ["B", "A"]);
        let sites: Vec<_> = library
            .sites()
            .iter()
            .map(|site| (site.name.as_str(), site.width))
            .collect();
        assert_eq!(sites, [("S", 5.0), ("T", 2.0), ("U", 3.0)]);
        let err = parse_cell_lef_metadata(&[first_bad.clone(), missing.clone()], false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("first-bad.lef: SITE Bad expected BY in SIZE"),
            "{err}"
        );
        let err = parse_cell_lef_metadata(&[missing.clone(), first_bad.clone()], false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing.lef"), "{err}");
    }
    set_threads(None).unwrap();

    let empty = parse_cell_lef_metadata(&[], true).unwrap();
    assert!(empty.macros().is_empty() && empty.sites().is_empty());
}
