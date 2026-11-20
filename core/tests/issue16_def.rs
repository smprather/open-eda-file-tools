use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use open_eda_file_tools_core::def::{self, MacroPinRole};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn fixture(def: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "issue16-def-{}-{}.def",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, def).unwrap();
    path
}

#[test]
fn generated_clock_includes_divider_but_input_clock_does_not_forward_data() {
    let path = fixture(
        r#"
        PROPERTYDEFINITIONS
          COMPONENT marker STRING "NETS" ;
        END PROPERTYDEFINITIONS
        COMPONENTS 3 ;
        - div\[0\] FF + PLACED ( 0 0 ) N ;
        - sink FF + PLACED ( 0 0 ) N ;
        - data BUF + PLACED ( 0 0 ) N ;
        END COMPONENTS
        NETS 3 ;
        - root ( PIN clk ) ( div\[0\] CK ) ;
        - generated ( div\[0\] Q ) ( sink CK ) ;
        - clk ( div\[0\] QN ) ( data A ) ;
        END NETS
        END DESIGN
    "#,
    );
    let roles = [
        MacroPinRole {
            macro_name: "FF".into(),
            inputs: vec!["CK".into()],
            outputs: vec!["Q".into(), "QN".into()],
            endpoint: true,
            clock_pins: vec!["CK".into()],
        },
        MacroPinRole {
            macro_name: "BUF".into(),
            inputs: vec!["A".into()],
            outputs: vec!["Z".into()],
            ..MacroPinRole::default()
        },
    ];
    for prune in [true, false] {
        assert_eq!(
            def::clock_tree_cells(&path, &["pin:div[0]/Q".into()], &roles, prune).unwrap(),
            vec![r"div\[0\]"]
        );
        assert!(
            def::clock_tree_cells(&path, &["port:clk".into()], &roles, prune)
                .unwrap()
                .is_empty()
        );
    }
    let explicit = def::clock_object_instances(&path, &["generated".into()], &[], &roles).unwrap();
    assert_eq!(explicit.instances, vec![r"div\[0\]"]);
    let pin = def::clock_object_instances(
        &path,
        &[],
        &[("div[0]".into(), "Q".into()), ("sink".into(), "CK".into())],
        &roles,
    )
    .unwrap();
    assert_eq!(pin.instances, vec![r"div\[0\]"]);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn targeted_scan_streams_bbox_and_only_selected_regular_net_connections() {
    let path = fixture(
        r#"
        UNITS DISTANCE MICRONS 2000 ;
        PROPERTYDEFINITIONS
          COMPONENT marker STRING "NETS" ;
        END PROPERTYDEFINITIONS
        BEGINEXT "vendor"
          COMPONENTS "ENDEXT" ;
        ENDEXT
        ROW r site 0 0 N DO 100 BY 1 STEP 125 0 ;
        SPECIALNETS 1 ;
        - n\[0\] ( PIN power ) ( invalid connection ignored ) ;
        END SPECIALNETS
        NETS 2 ;
        - noise ( not parsable by connection reader ) ;
        - n\[0\] ( PIN clk ) ( u Q + SYNTHESIZED ) + ROUTED M1 ( 0 0 ) ( 100 0 ) ;
        END NETS
        COMPONENTS 2 ;
        - u UNSEEN_MACRO + PLACED ( 10 20 ) W ;
        - skip UNSEEN_MACRO + UNPLACED ;
        END COMPONENTS
        END DESIGN
    "#,
    );
    let mut rows = Vec::new();
    let result = def::scan_clock_nets_and_instances(
        &path,
        &[],
        &HashSet::from(["n[0]".to_string()]),
        |row| {
            rows.push(row);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(result.dbu_per_micron, 2000);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].llx, rows[0].lly, rows[0].urx, rows[0].ury),
        (10, 20, 510, 520)
    );
    assert_eq!(result.clock_nets.len(), 1);
    let net = &result.clock_nets[0];
    assert_eq!(net.name, r"n\[0\]");
    assert_eq!(
        (
            net.connections[0].instance.as_deref(),
            net.connections[0].pin.as_str()
        ),
        (None, "clk")
    );
    assert_eq!(
        (
            net.connections[1].instance.as_deref(),
            net.connections[1].pin.as_str()
        ),
        (Some("u"), "Q")
    );
    std::fs::remove_file(path).unwrap();
}
