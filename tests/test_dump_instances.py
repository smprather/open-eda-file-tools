"""Tests for the unified ``def-tools instances`` subcommand."""

from __future__ import annotations

from pathlib import Path

import polars as pl
import pytest
from click.testing import CliRunner

from def_tools import cli


def write_fixture(tmp_path: Path) -> tuple[Path, Path]:
    """A two-instance design small enough to assert exact output."""
    lef = tmp_path / "cells.lef"
    lef.write_text(
        "VERSION 5.8 ;\n"
        "UNITS DATABASE MICRONS 1000 ;\n"
        "MACRO INV\n"
        "  ORIGIN 0 0 ;\n"
        "  SIZE 1.2 BY 2.4 ;\n"
        "END INV\n",
        encoding="utf-8",
    )
    design = tmp_path / "design.def"
    design.write_text(
        "VERSION 5.8 ;\n"
        "DESIGN tiny ;\n"
        "UNITS DISTANCE MICRONS 1000 ;\n"
        "COMPONENTS 2 ;\n"
        "  - u1 INV + PLACED ( 10 20 ) N ;\n"
        "  - u2 INV + PLACED ( 30 40 ) E ;\n"
        "END COMPONENTS\n"
        "END DESIGN\n",
        encoding="utf-8",
    )
    return lef, design


@pytest.mark.parametrize(
    ("text", "value"),
    [
        ("1", 1),
        ("1b", 1),
        ("1KB", 1024),
        ("1kib", 1024),
        ("1.5MiB", int(1.5 * 1024**2)),
        (" 2 GB ", 2 * 1024**3),
        ("1T", 1024**4),
    ],
)
def test_parse_size(text: str, value: int) -> None:
    assert cli.parse_size(text) == value


@pytest.mark.parametrize("text", ["", "12XB", "-1", "1..5GB", "GB", "1e3"])
def test_parse_size_rejects_invalid(text: str) -> None:
    with pytest.raises(ValueError, match="invalid size"):
        cli.parse_size(text)


def test_parse_column_spec_renames_and_keeps_order() -> None:
    assert cli.parse_column_spec("instance=name, base_cell,status,orient,llx=llx_um") == [
        ("instance", "name"),
        ("base_cell", "base_cell"),
        ("status", "status"),
        ("orient", "orient"),
        ("llx", "llx_um"),
    ]


@pytest.mark.parametrize(
    "text",
    ["", "instance,", "bogus", "instance=", "instance=name,base_cell=name"],
)
def test_parse_column_spec_rejects_invalid(text: str) -> None:
    with pytest.raises(ValueError, match="column|empty"):
        cli.parse_column_spec(text)


def test_memory_helpers() -> None:
    assert cli.memory_ceiling(1000) == 1100
    assert cli.effective_batch_size(1024**3, 1000) == 1000


def test_instances_default_csv_microns_and_columns(tmp_path: Path) -> None:
    lef, design = write_fixture(tmp_path)
    output = tmp_path / "instances.csv"

    result = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(lef),
            "--def",
            str(design),
            "--columns",
            "instance=name,base_cell=cell,status,orient,llx",
            "--output",
            str(output),
        ],
    )

    assert result.exit_code == 0, result.output
    assert output.read_text(encoding="utf-8").splitlines() == [
        "name,cell,status,orient,llx",
        "u1,INV,PLACED,N,0.01",
        "u2,INV,PLACED,E,0.03",
    ]


def test_instances_variadic_and_glob_lef(tmp_path: Path) -> None:
    lef_dir = tmp_path / "lefs"
    lef_dir.mkdir()
    lef1 = lef_dir / "01_inv.lef"
    lef2 = lef_dir / "02_buf.lef"
    lef1.write_text(
        "VERSION 5.8 ;\nMACRO INV\n  SIZE 1.2 BY 2.4 ;\nEND INV\n",
        encoding="utf-8",
    )
    lef2.write_text(
        "VERSION 5.8 ;\nMACRO BUF\n  SIZE 3.0 BY 1.0 ;\nEND BUF\n",
        encoding="utf-8",
    )
    design = tmp_path / "design.def"
    design.write_text(
        "VERSION 5.8 ;\n"
        "DESIGN tiny ;\n"
        "UNITS DISTANCE MICRONS 1000 ;\n"
        "COMPONENTS 2 ;\n"
        "  - u1 INV + PLACED ( 10 20 ) N ;\n"
        "  - u2 BUF + PLACED ( 30 40 ) N ;\n"
        "END COMPONENTS\n"
        "END DESIGN\n",
        encoding="utf-8",
    )
    out_var = tmp_path / "variadic.csv"
    out_glob = tmp_path / "glob.csv"

    r_var = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(lef1),
            str(lef2),
            "--def",
            str(design),
            "--output",
            str(out_var),
        ],
    )
    assert r_var.exit_code == 0, r_var.output

    r_glob = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(lef_dir / "*.lef"),
            "--def",
            str(design),
            "--output",
            str(out_glob),
        ],
    )
    assert r_glob.exit_code == 0, r_glob.output
    assert out_var.read_text(encoding="utf-8") == out_glob.read_text(encoding="utf-8")


def test_instances_unmatched_glob_fails(tmp_path: Path) -> None:
    _lef, design = write_fixture(tmp_path)
    result = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(tmp_path / "nonexistent_*.lef"),
            "--def",
            str(design),
        ],
    )
    assert result.exit_code != 0
    assert "no files match pattern" in result.output


def test_instances_table_and_parquet_formats(tmp_path: Path) -> None:
    lef, design = write_fixture(tmp_path)
    table_out = tmp_path / "instances.table"
    parquet_out = tmp_path / "instances.parquet"

    r_table = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(lef),
            "--def",
            str(design),
            "--format",
            "table",
            "--columns",
            "instance=name,base_cell=cell",
            "--output",
            str(table_out),
        ],
    )
    assert r_table.exit_code == 0, r_table.output
    lines = table_out.read_text(encoding="utf-8").splitlines()
    assert lines[0].split() == ["name", "cell"]
    assert lines[2].split() == ["u1", "INV"]
    assert lines[3].split() == ["u2", "INV"]

    r_pq = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(lef),
            "--def",
            str(design),
            "--format",
            "parquet",
            "--output",
            str(parquet_out),
        ],
    )
    assert r_pq.exit_code == 0, r_pq.output
    df = pl.read_parquet(parquet_out)
    assert df.columns == ["instance", "base_cell", "llx", "lly", "urx", "ury"]
    assert df["instance"].to_list() == ["u1", "u2"]


def test_instances_aborts_on_impossible_memory_budget(tmp_path: Path) -> None:
    lef, design = write_fixture(tmp_path)
    output = tmp_path / "instances.parquet"

    result = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(lef),
            "--def",
            str(design),
            "--format",
            "parquet",
            "--max-memory",
            "1",
            "--output",
            str(output),
        ],
    )

    assert result.exit_code == 1
    assert not output.exists()


def test_failed_run_keeps_an_existing_output(tmp_path: Path) -> None:
    _lef, design = write_fixture(tmp_path)
    output = tmp_path / "instances.csv"
    output.write_text("sentinel\n", encoding="utf-8")

    result = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(tmp_path / "missing.lef"),
            "--def",
            str(design),
            "--format",
            "csv",
            "--output",
            str(output),
        ],
    )

    assert result.exit_code == 1
    assert output.read_text(encoding="utf-8") == "sentinel\n"


def test_failed_run_removes_a_new_output(tmp_path: Path) -> None:
    _lef, design = write_fixture(tmp_path)
    output = tmp_path / "instances.csv"

    result = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(tmp_path / "missing.lef"),
            "--def",
            str(design),
            "--format",
            "csv",
            "--output",
            str(output),
        ],
    )

    assert result.exit_code == 1
    assert not output.exists()


def write_flag_fixture(tmp_path: Path) -> tuple[Path, Path, Path, Path]:
    """A combinational and a sequential instance, with Liberty and clock cells."""
    lef = tmp_path / "flag_cells.lef"
    lef.write_text(
        "VERSION 5.8 ;\n"
        "UNITS DATABASE MICRONS 1000 ;\n"
        "MACRO INV\n  SIZE 1.2 BY 2.4 ;\nEND INV\n"
        "MACRO DFF\n  SIZE 2.0 BY 2.0 ;\nEND DFF\n",
        encoding="utf-8",
    )
    design = tmp_path / "flag_design.def"
    design.write_text(
        "VERSION 5.8 ;\n"
        "DESIGN flags ;\n"
        "UNITS DISTANCE MICRONS 1000 ;\n"
        "COMPONENTS 2 ;\n"
        "  - u1 INV + PLACED ( 10 20 ) N ;\n"
        "  - u2 DFF + PLACED ( 30 40 ) N ;\n"
        "END COMPONENTS\n"
        "END DESIGN\n",
        encoding="utf-8",
    )
    liberty = tmp_path / "flag.lib"
    liberty.write_text(
        "library (flag) {\n"
        "  cell (INV) {\n"
        "    pin (A) { direction : input; }\n"
        "    pin (X) { direction : output; function : \"A\"; }\n"
        "  }\n"
        "  cell (DFF) {\n"
        "    ff (IQ, IQN) { next_state : \"D\"; clocked_on : \"CK\"; }\n"
        "    pin (CK) { direction : input; clock : true; }\n"
        "    pin (D) { direction : input; }\n"
        "    pin (Q) { direction : output; function : \"IQ\"; }\n"
        "  }\n"
        "}\n",
        encoding="utf-8",
    )
    clock_cells = tmp_path / "clock_cells.txt"
    clock_cells.write_text("u1\n# comment\n\n", encoding="utf-8")
    return lef, design, liberty, clock_cells


def test_flag_columns_appear_with_their_inputs(tmp_path: Path) -> None:
    lef, design, liberty, clock_cells = write_flag_fixture(tmp_path)
    output = tmp_path / "instances.csv"

    result = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(lef),
            "--def",
            str(design),
            "--lib",
            str(liberty),
            "--clock-cells",
            str(clock_cells),
            "--output",
            str(output),
        ],
    )

    assert result.exit_code == 0, result.output
    assert output.read_text(encoding="utf-8").splitlines() == [
        "instance,base_cell,llx,lly,urx,ury,is_seq,is_clock",
        "u1,INV,0.01,0.02,1.21,2.42,false,true",
        "u2,DFF,0.03,0.04,2.03,2.04,true,false",
    ]


def test_flag_columns_require_their_inputs(tmp_path: Path) -> None:
    lef, design, _liberty, _clock_cells = write_flag_fixture(tmp_path)

    result = CliRunner().invoke(
        cli.main,
        [
            "instances",
            "--lef",
            str(lef),
            "--def",
            str(design),
            "--columns",
            "instance,is_seq,is_clock",
        ],
    )

    assert result.exit_code == 1
    assert "is_seq" in result.output
    assert "Liberty" in result.output


def test_flag_columns_are_booleans_in_parquet(tmp_path: Path) -> None:
    from def_tools import write_instance_parquet

    lef, design, _liberty, _clock_cells = write_flag_fixture(tmp_path)
    output = tmp_path / "instances.parquet"

    written, _ = write_instance_parquet(
        [lef],
        design,
        output,
        1024,
        columns=[("instance", "instance"), ("is_seq", "is_seq"), ("is_clock", "is_clock")],
        sequential=["DFF"],
        clock=["u1"],
    )

    assert written == 2
    frame = pl.read_parquet(output)
    assert frame["is_seq"].to_list() == [False, True]
    assert frame["is_clock"].to_list() == [True, False]
    assert frame.schema["is_seq"] == pl.Boolean
