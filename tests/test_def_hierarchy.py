from __future__ import annotations

import csv
import gzip
from pathlib import Path

from click.testing import CliRunner

from def_tools.cli import main


def design(path: Path, name: str, units: int, bounds: str, rows: list[str]) -> Path:
    data = (
        'VERSION 5.8 ;\nDIVIDERCHAR "/" ;\n'
        f"DESIGN {name} ;\nUNITS DISTANCE MICRONS {units} ;\n"
        f"DIEAREA {bounds} ;\nCOMPONENTS {len(rows)} ;\n"
        + "\n".join(rows)
        + "\nEND COMPONENTS\nEND DESIGN\n"
    )
    if path.suffix == ".gz":
        path.write_bytes(gzip.compress(data.encode()))
    else:
        path.write_text(data)
    return path


def inputs(tmp_path: Path) -> tuple[Path, Path, Path]:
    lef = tmp_path / "cells.lef"
    lef.write_text("MACRO INV\n  SIZE 2 BY 1 ;\nEND INV\n")
    root = design(
        tmp_path / "top.def",
        "top",
        1000,
        "( 0 0 ) ( 500000 500000 )",
        [
            "- west tile + FIXED ( 100000 200000 ) N ;",
            "- east tile + PLACED ( 200000 100000 ) W ;",
            "- local INV + PLACED ( 1000 5000 ) N ;",
        ],
    )
    block = design(
        tmp_path / "child.def.gz",
        "block_layout",
        2000,
        "( -2000 4000 ) ( 18000 44000 )",
        ["- logic/leaf INV + PLACED ( 4000 10000 ) FS ;"],
    )
    return lef, root, block


def test_instances_expands_repeated_blocks_in_top_coordinates(tmp_path: Path) -> None:
    lef, root, block = inputs(tmp_path)
    output = tmp_path / "instances.csv"
    run = CliRunner().invoke(
        main,
        [
            "instances",
            "--lef",
            str(lef),
            "--def",
            str(root),
            "--block-def",
            f"tile={block}",
            "--output",
            str(output),
            "--columns",
            "instance,base_cell,status,orient,llx,lly,urx,ury",
        ],
    )
    assert run.exit_code == 0, run.output
    assert list(csv.reader(output.read_text().splitlines())) == [
        ["instance", "base_cell", "status", "orient", "llx", "lly", "urx", "ury"],
        ["west", "tile", "FIXED", "N", "100.0", "200.0", "110.0", "220.0"],
        ["west/logic/leaf", "INV", "PLACED", "FS", "103.0", "203.0", "105.0", "204.0"],
        ["east", "tile", "PLACED", "W", "200.0", "100.0", "220.0", "110.0"],
        ["east/logic/leaf", "INV", "PLACED", "FW", "216.0", "103.0", "217.0", "105.0"],
        ["local", "INV", "PLACED", "N", "1.0", "5.0", "3.0", "6.0"],
    ]


def test_hierarchy_api_composes_nested_mirrors_and_unequal_units(
    tmp_path: Path,
) -> None:
    from def_tools import read_def_hierarchy

    lef, root, block = inputs(tmp_path)
    design(
        root,
        "top",
        1000,
        "( 0 0 ) ( 100000 100000 )",
        [
            "- upper tile + PLACED ( 10000 20000 ) FN ;",
        ],
    )
    design(
        block,
        "middle",
        2000,
        "( -2000 4000 ) ( 18000 44000 )",
        [
            "- lower core + FIXED ( 4000 10000 ) E ;",
        ],
    )
    leaf = design(
        tmp_path / "core.def",
        "core",
        1500,
        "( -1500 3000 ) ( 7500 3000 ) ( 7500 9000 ) ( -1500 9000 )",
        [
            "- leaf INV + PLACED ( 0 4500 ) N ;",
        ],
    )
    hierarchy = read_def_hierarchy([lef], root, [("tile", block), ("core", leaf)])
    assert hierarchy.dbu_per_micron == 6000
    first = hierarchy.instances()
    second = hierarchy.instances()
    del hierarchy
    expected = [
        ("upper", "tile", "PLACED", "FN", 10, 20, 20, 40),
        ("upper/lower", "core", "FIXED", "FE", 13, 23, 17, 29),
        ("upper/lower/leaf", "INV", "PLACED", "FE", 15, 26, 16, 28),
    ]
    for iterator in (first, second):
        assert [row.as_tuple() for row in iterator] == [
            (*row[:4], *(value * 6000 for value in row[4:])) for row in expected
        ]


def test_hierarchy_has_all_def_orientation_transforms(tmp_path: Path) -> None:
    from def_tools import read_def_hierarchy

    lef, root, block = inputs(tmp_path)
    expected = {
        "N": (1, 3, 3, 4),
        "S": (7, 16, 9, 17),
        "E": (3, 7, 4, 9),
        "W": (16, 1, 17, 3),
        "FN": (7, 3, 9, 4),
        "FS": (1, 16, 3, 17),
        "FE": (16, 7, 17, 9),
        "FW": (3, 1, 4, 3),
    }
    design(
        root,
        "top",
        1000,
        "( 0 0 ) ( 100000 100000 )",
        [f"- {orient.lower()} tile + PLACED ( 0 0 ) {orient} ;" for orient in expected],
    )
    design(
        block,
        "block",
        1000,
        "( 0 0 ) ( 10000 20000 )",
        [
            "- leaf INV + PLACED ( 1000 3000 ) N ;",
            "- unplaced INV + UNPLACED ;",
        ],
    )
    hierarchy = read_def_hierarchy([lef], root, [("tile", block)])
    rows = list(hierarchy.instances())
    assert len(rows) == 16
    for row, (orient, bounds) in zip(rows[1::2], expected.items(), strict=True):
        assert row.as_tuple() == (
            f"{orient.lower()}/leaf",
            "INV",
            "PLACED",
            orient,
            *(coord * 1000 for coord in bounds),
        )


def test_hierarchy_supports_parquet_table_and_offline_cli(tmp_path: Path) -> None:
    import polars as pl

    from def_tools.instances import main as offline_main

    lef, root, block = inputs(tmp_path)
    common = ["--lef", str(lef), "--def", str(root), "--block-def", f"tile={block}"]
    clock = tmp_path / "clock.txt"
    clock.write_text("east/logic/leaf\n")
    output = tmp_path / "instances.parquet"
    run = CliRunner().invoke(
        main,
        [
            "instances",
            *common,
            "--format",
            "parquet",
            "--batch-size",
            "2",
            "--clock-cells",
            str(clock),
            "--output",
            str(output),
        ],
    )
    assert run.exit_code == 0, run.output
    assert pl.read_parquet(output).select("instance", "is_clock").rows() == [
        ("west", False),
        ("west/logic/leaf", False),
        ("east", False),
        ("east/logic/leaf", True),
        ("local", False),
    ]
    table = tmp_path / "instances.txt"
    assert (
        offline_main(
            [
                *common,
                "--format",
                "table",
                "--columns",
                "instance,llx",
                "--output",
                str(table),
            ]
        )
        == 0
    )
    assert table.read_text().splitlines()[2].split() == ["west", "100.000"]
    assert table.read_text().splitlines()[5].split() == ["east/logic/leaf", "216.000"]


def test_hierarchy_rejects_cycles_duplicate_mappings_and_truncated_input(
    tmp_path: Path,
) -> None:
    import pytest

    from def_tools import read_def_hierarchy

    lef, root, block = inputs(tmp_path)
    with pytest.raises(RuntimeError, match="cyclic DEF hierarchy.*top.def.*top.def"):
        read_def_hierarchy([lef], root, [("tile", root)])
    with pytest.raises(RuntimeError, match="duplicate block DEF mapping.*tile"):
        read_def_hierarchy([lef], root, [("tile", block), ("tile", block)])
    broken = tmp_path / "broken.def"
    broken.write_text("DIEAREA ( 0 0 ) ( 10000 20000 ) ;\nCOMPONENTS 1 ;\n")
    with pytest.raises(RuntimeError, match="broken.def.*missing END COMPONENTS"):
        read_def_hierarchy([lef], root, [("tile", broken)])
    broken.write_text(
        "DIEAREA ( 0 0 ) ( 10000 20000 ) ;\nCOMPONENTS 1 ;\nEND COMPONENTS\n"
    )
    with pytest.raises(
        RuntimeError, match="COMPONENTS declares 1 records but contains 0"
    ):
        read_def_hierarchy([lef], root, [("tile", broken)])
    broken.write_text("COMPONENTS 0 ;\nEND COMPONENTS\n")
    with pytest.raises(RuntimeError, match="tile.*no DIEAREA.*broken.def"):
        read_def_hierarchy([lef], root, [("tile", broken)])
