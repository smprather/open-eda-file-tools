from __future__ import annotations

import gzip

from click.testing import CliRunner

from def_tools import instance_bboxes, net_shapes
from def_tools.cli import main


def write_fixture(tmp_path):
    lef = tmp_path / "cells.lef"
    lef.write_text(
        """
VERSION 5.8 ;
UNITS DATABASE MICRONS 1000 ;
MACRO INV
  ORIGIN 0 0 ;
  SIZE 1.2 BY 2.4 ;
END INV
MACRO BUF
  SIZE 3.0 BY 1.0 ;
END BUF
""",
        encoding="utf-8",
    )
    design = tmp_path / "design.def"
    design.write_text(
        """
VERSION 5.8 ;
DIVIDERCHAR "/" ;
BUSBITCHARS "[]" ;
DESIGN tiny ;
UNITS DISTANCE MICRONS 1000 ;
COMPONENTS 4 ;
  - u1 INV + PLACED ( 10 20 ) N ;
  - u2 INV + FIXED ( 30 40 ) E ;
  - u3 BUF + COVER ( 5 6 ) FS ;
  - u4 INV + UNPLACED ;
END COMPONENTS
SPECIALNETS 1 ;
  - VDD ( PIN VDD )
    + ROUTED met1 40 + SHAPE FOLLOWPIN ( 0 100 ) ( 1000 100 )
      NEW met1 20 ( 500 100 ) ( 500 300 )
      NEW met2 50 ( 1 2 ) ( 3 4 ) ;
END SPECIALNETS
NETS 1 ;
  - data ( u1 A ) ( u2 B )
    + ROUTED met1 ( 10 10 ) ( 20 10 ) ;
END NETS
END DESIGN
""",
        encoding="utf-8",
    )
    return lef, design


def test_python_iterator_returns_placed_instance_bboxes(tmp_path):
    lef, design = write_fixture(tmp_path)

    rows = list(instance_bboxes([lef], design))

    assert [row.instance for row in rows] == ["u1", "u2", "u3"]
    assert rows[0].as_tuple() == ("u1", "INV", "PLACED", "N", 10, 20, 1210, 2420)
    assert rows[1].as_tuple() == ("u2", "INV", "FIXED", "E", 30, 40, 2430, 1240)
    assert rows[2].as_tuple() == ("u3", "BUF", "COVER", "FS", 5, 6, 3005, 1006)


def test_cli_writes_streaming_csv(tmp_path):
    lef, design = write_fixture(tmp_path)
    output = tmp_path / "instances.csv"
    runner = CliRunner()

    result = runner.invoke(
        main,
        ["instances", "--lef", str(lef), "--def", str(design), "--output", str(output)],
    )

    assert result.exit_code == 0, result.output
    assert output.read_text(encoding="utf-8").splitlines() == [
        "instance,macro,status,orient,llx,lly,urx,ury",
        "u1,INV,PLACED,N,10,20,1210,2420",
        "u2,INV,FIXED,E,30,40,2430,1240",
        "u3,BUF,COVER,FS,5,6,3005,1006",
    ]


def test_cli_accepts_gzipped_def(tmp_path):
    lef, design = write_fixture(tmp_path)
    gz_design = tmp_path / "design.def.gz"
    output = tmp_path / "instances.csv"
    gz_design.write_bytes(gzip.compress(design.read_bytes()))
    runner = CliRunner()

    result = runner.invoke(
        main,
        ["instances", "--lef", str(lef), "--def", str(gz_design), "--output", str(output)],
    )

    assert result.exit_code == 0, result.output
    lines = output.read_text(encoding="utf-8").splitlines()
    assert lines[0] == "instance,macro,status,orient,llx,lly,urx,ury"
    assert "u1,INV,PLACED,N,10,20,1210,2420" in lines


def test_python_iterator_returns_net_shapes(tmp_path):
    _lef, design = write_fixture(tmp_path)

    rows = list(net_shapes(design, "VDD", "met1"))

    assert len(rows) == 2
    assert (
        rows[0].net,
        rows[0].section,
        rows[0].route_status,
        rows[0].shape_type,
        rows[0].layer,
        rows[0].width,
        rows[0].x1,
        rows[0].y1,
        rows[0].x2,
        rows[0].y2,
        rows[0].llx,
        rows[0].lly,
        rows[0].urx,
        rows[0].ury,
    ) == (
        "VDD",
        "SPECIALNETS",
        "ROUTED",
        "FOLLOWPIN",
        "met1",
        40,
        0,
        100,
        1000,
        100,
        0,
        80,
        1000,
        120,
    )
    assert (
        rows[1].net,
        rows[1].section,
        rows[1].route_status,
        rows[1].shape_type,
        rows[1].layer,
        rows[1].width,
        rows[1].x1,
        rows[1].y1,
        rows[1].x2,
        rows[1].y2,
        rows[1].llx,
        rows[1].lly,
        rows[1].urx,
        rows[1].ury,
    ) == ("VDD", "SPECIALNETS", "ROUTED", "", "met1", 20, 500, 100, 500, 300, 490, 100, 510, 300)


def test_cli_writes_net_shape_csv_from_gzip(tmp_path):
    _lef, design = write_fixture(tmp_path)
    gz_design = tmp_path / "design.def.gz"
    output = tmp_path / "shapes.csv"
    gz_design.write_bytes(gzip.compress(design.read_bytes()))
    runner = CliRunner()

    result = runner.invoke(
        main,
        [
            "net-shapes",
            "--def",
            str(gz_design),
            "--net",
            "VDD",
            "--layer",
            "met1",
            "--output",
            str(output),
        ],
    )

    assert result.exit_code == 0, result.output
    assert output.read_text(encoding="utf-8").splitlines() == [
        "net,section,route_status,shape_type,layer,width,x1,y1,x2,y2,llx,lly,urx,ury",
        "VDD,SPECIALNETS,ROUTED,FOLLOWPIN,met1,40,0,100,1000,100,0,80,1000,120",
        "VDD,SPECIALNETS,ROUTED,,met1,20,500,100,500,300,490,100,510,300",
    ]


def test_cli_queries_net_shapes_from_cache_dir(tmp_path):
    _lef, design = write_fixture(tmp_path)
    gz_design = tmp_path / "design.def.gz"
    cache_dir = tmp_path / "design-cache"
    output = tmp_path / "shapes.csv"
    gz_design.write_bytes(gzip.compress(design.read_bytes()))
    runner = CliRunner()

    create_result = runner.invoke(
        main,
        ["cache", "create", "--def", str(gz_design), "--output", str(cache_dir)],
    )
    assert create_result.exit_code == 0, create_result.output

    query_result = runner.invoke(
        main,
        [
            "net-shapes",
            "--cache-dir",
            str(cache_dir),
            "--net",
            "VDD",
            "--layer",
            "met1",
            "--output",
            str(output),
        ],
    )

    assert query_result.exit_code == 0, query_result.output
    assert output.read_text(encoding="utf-8").splitlines() == [
        "net,section,route_status,shape_type,layer,width,x1,y1,x2,y2,llx,lly,urx,ury",
        "VDD,SPECIALNETS,ROUTED,FOLLOWPIN,met1,40,0,100,1000,100,0,80,1000,120",
        "VDD,SPECIALNETS,ROUTED,,met1,20,500,100,500,300,490,100,510,300",
    ]


def test_cli_finds_clock_instances_from_sdc_and_cache(tmp_path):
    design = tmp_path / "clock.def"
    design.write_text(
        """
VERSION 5.8 ;
DIVIDERCHAR "/" ;
DESIGN clocked ;
COMPONENTS 3 ;
  - clkbuf1 sky130_fd_sc_hd__clkbuf_1 + PLACED ( 0 0 ) N ;
  - flop1 sky130_fd_sc_hd__dfxtp_1 + PLACED ( 10 0 ) N ;
  - comb1 sky130_fd_sc_hd__and2_1 + PLACED ( 20 0 ) N ;
END COMPONENTS
NETS 3 ;
  - clk1 ( PIN clk1 ) ( clkbuf1 A ) ;
  - clk_leaf ( clkbuf1 X ) ( flop1 CLK ) ;
  - data ( flop1 D ) ( comb1 A ) ;
END NETS
END DESIGN
""",
        encoding="utf-8",
    )
    sdc = tmp_path / "base.sdc"
    sdc.write_text(
        "create_clock -name clk_main -period 10 [get_ports {clk1}]\n",
        encoding="utf-8",
    )
    cache_dir = tmp_path / "clock-cache"
    output = tmp_path / "clock.csv"
    runner = CliRunner()

    create_result = runner.invoke(
        main,
        ["cache", "create", "--def", str(design), "--output", str(cache_dir)],
    )
    assert create_result.exit_code == 0, create_result.output

    query_result = runner.invoke(
        main,
        [
            "clock-instances",
            "--sdc",
            str(sdc),
            "--cache-dir",
            str(cache_dir),
            "--output",
            str(output),
        ],
    )

    assert query_result.exit_code == 0, query_result.output
    assert output.read_text(encoding="utf-8").splitlines() == [
        "clock,root_type,root_name,instance,macro,net,is_clock_cell",
        "clk_main,port,clk1,clkbuf1,sky130_fd_sc_hd__clkbuf_1,clk1,true",
        "clk_main,port,clk1,flop1,sky130_fd_sc_hd__dfxtp_1,clk_leaf,false",
    ]
