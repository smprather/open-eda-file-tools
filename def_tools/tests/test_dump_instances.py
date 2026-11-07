"""Tests for the dump_instances CLI and the def-dump-instances surface."""

from __future__ import annotations

import argparse
from pathlib import Path

import pytest

from def_tools import dump_instances


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
    assert dump_instances.parse_size(text) == value


@pytest.mark.parametrize("text", ["", "12XB", "-1", "1..5GB", "GB", "1e3"])
def test_parse_size_rejects_invalid(text: str) -> None:
    with pytest.raises(argparse.ArgumentTypeError):
        dump_instances.parse_size(text)


def test_parse_column_spec_renames_and_keeps_order() -> None:
    assert dump_instances.parse_column_spec("instance=name, base_cell,llx=llx_um") == [
        ("instance", "name"),
        ("base_cell", "base_cell"),
        ("llx", "llx_um"),
    ]


@pytest.mark.parametrize(
    "text",
    ["", "instance,", "bogus", "instance=", "instance=name,base_cell=name"],
)
def test_parse_column_spec_rejects_invalid(text: str) -> None:
    with pytest.raises(argparse.ArgumentTypeError):
        dump_instances.parse_column_spec(text)


def test_memory_helpers() -> None:
    assert dump_instances.memory_ceiling(1000) == 1100
    assert dump_instances.effective_batch_size(1024**3, 1000) == 1000


def test_dump_csv_columns(tmp_path: Path) -> None:
    lef, design = write_fixture(tmp_path)
    output = tmp_path / "instances.csv"

    rc = dump_instances.main(
        [
            "--lef",
            str(lef),
            "--def",
            str(design),
            "--format",
            "csv",
            "--columns",
            "instance=name,base_cell=cell,llx",
            "--output",
            str(output),
        ]
    )

    assert rc == 0
    assert output.read_text(encoding="utf-8").splitlines() == [
        "name,cell,llx",
        "u1,INV,0.01",
        "u2,INV,0.03",
    ]


def test_dump_table_columns(tmp_path: Path) -> None:
    lef, design = write_fixture(tmp_path)
    output = tmp_path / "instances.table"

    rc = dump_instances.main(
        [
            "--lef",
            str(lef),
            "--def",
            str(design),
            "--format",
            "table",
            "--columns",
            "instance=name,base_cell=cell",
            "--output",
            str(output),
        ]
    )

    assert rc == 0
    lines = output.read_text(encoding="utf-8").splitlines()
    assert lines[0].split() == ["name", "cell"]
    assert lines[2].split() == ["u1", "INV"]
    assert lines[3].split() == ["u2", "INV"]


def test_dump_aborts_on_impossible_memory_budget(tmp_path: Path) -> None:
    lef, design = write_fixture(tmp_path)
    output = tmp_path / "instances.parquet"

    rc = dump_instances.main(
        [
            "--lef",
            str(lef),
            "--def",
            str(design),
            "--max-memory",
            "1",
            "--output",
            str(output),
        ]
    )

    assert rc == 1
    assert not output.exists()


def test_failed_run_keeps_an_existing_output(tmp_path: Path) -> None:
    _lef, design = write_fixture(tmp_path)
    output = tmp_path / "instances.csv"
    output.write_text("sentinel\n", encoding="utf-8")

    rc = dump_instances.main(
        [
            "--lef",
            str(tmp_path / "missing.lef"),
            "--def",
            str(design),
            "--format",
            "csv",
            "--output",
            str(output),
        ]
    )

    assert rc == 1
    assert output.read_text(encoding="utf-8") == "sentinel\n"


def test_failed_run_removes_a_new_output(tmp_path: Path) -> None:
    _lef, design = write_fixture(tmp_path)
    output = tmp_path / "instances.csv"

    rc = dump_instances.main(
        [
            "--lef",
            str(tmp_path / "missing.lef"),
            "--def",
            str(design),
            "--format",
            "csv",
            "--output",
            str(output),
        ]
    )

    assert rc == 1
    assert not output.exists()
