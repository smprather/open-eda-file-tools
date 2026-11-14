"""Clock-tree identification: the SDC trace, OTW, and clock-data backends.

One hand-built design exercises every case; the expected answer is written
out below, not derived from the code under test.

    clk port -> u_buf -> n_buf -> u_icg.CK -> n_gck -> u_ff1.CK
                               -> u_div.CK ; u_div.Q -> divq -> u_gbuf -> u_ff2.CK
                                                             -> u_and -> u_ff2.D
                               -> u_mem.CLK ; u_mem.Q -> u_post -> u_ff1.D
                               -> u_icg2.E   (a clock on an enable pin)
                               -> u_b[1] -> n[1] -> u_ff3.CK   (escaped names)
    u_en -> u_icg.E
"""

from __future__ import annotations

import gzip
from pathlib import Path

import pytest
from click.testing import CliRunner

from def_tools import cli
from def_tools.clock_data import (
    ClockData,
    ClockDataBackendError,
    ClockDataError,
    ClockDataFormatError,
    clock_instances,
    parse_source,
    read_clock_data,
    register_backend,
)
from def_tools.clock_roles import cell_clock_roles
from def_tools.otw import OtwClock, OtwWindow, read_otw, write_otw

CLOCK_TREE = ["u_b\\[1\\]", "u_buf", "u_gbuf", "u_icg"]

LIB = """\
library (clk) {
  cell (BUF) {
    pin (A) { direction : input; }
    pin (X) { direction : output; function : "A"; }
  }
  cell (AND2) {
    pin (A) { direction : input; }
    pin (B) { direction : input; }
    pin (X) { direction : output; function : "A B"; }
  }
  cell (DFF) {
    ff (IQ, IQN) { next_state : "D"; clocked_on : "CK"; }
    pin (CK) { direction : input; clock : true; }
    pin (D) { direction : input; }
    pin (Q) { direction : output; function : "IQ"; }
  }
  cell (ICG) {
    clock_gating_integrated_cell : "latch_posedge";
    latch (IQ, IQN) { enable : "!CK"; data_in : "E"; }
    pin (CK) { direction : input; clock : true; clock_gate_clock_pin : true; }
    pin (E) { direction : input; clock_gate_enable_pin : true; }
    pin (GCK) { direction : output; clock_gate_out_pin : true; }
  }
  cell (RAM) {
    pin (CLK) { direction : input; clock : true; }
    pin (Q) { direction : output; }
  }
}
"""

LEF = "VERSION 5.8 ;\nUNITS DATABASE MICRONS 1000 ;\n" + "".join(
    f"MACRO {name}\n  SIZE 1.0 BY 1.0 ;\nEND {name}\n"
    for name in ("BUF", "AND2", "DFF", "ICG", "RAM", "BLACKBOX")
)

COMPONENTS = [
    ("u_buf", "BUF"),
    ("u_icg", "ICG"),
    ("u_icg2", "ICG"),
    ("u_ff1", "DFF"),
    ("u_ff2", "DFF"),
    ("u_ff3", "DFF"),
    ("u_div", "DFF"),
    ("u_gbuf", "BUF"),
    ("u_and", "AND2"),
    ("u_mem", "RAM"),
    ("u_post", "BUF"),
    ("u_en", "BUF"),
    ("u_b\\[1\\]", "BUF"),
    ("u_box", "BLACKBOX"),
]

NETS = [
    "clk ( PIN clk ) ( u_buf A )",
    "n_buf ( u_buf X ) ( u_icg CK ) ( u_div CK ) ( u_mem CLK ) ( u_icg2 E ) "
    "( u_b\\[1\\] A ) ( u_box CLK )",
    "n_gck ( u_icg GCK ) ( u_ff1 CK )",
    "divq ( u_div Q ) ( u_gbuf A ) ( u_and A )",
    "n_gbuf ( u_gbuf X ) ( u_ff2 CK )",
    "n_and ( u_and X ) ( u_ff2 D )",
    "n_mem ( u_mem Q ) ( u_post A )",
    "n_post ( u_post X ) ( u_ff1 D )",
    "din ( PIN din ) ( u_en A ) ( u_and B ) ( u_icg2 CK )",
    "n_en ( u_en X ) ( u_icg E )",
    "n_gck2 ( u_icg2 GCK ) ( u_ff3 D )",
    "n\\[1\\] ( u_b\\[1\\] X ) ( u_ff3 CK )",
]

SDC = (
    "create_clock -name clk -period 10 [get_ports clk]\n"
    "create_generated_clock -name div -source [get_ports clk] -divide_by 2 "
    "[get_pins u_div/Q]\n"
)

NET_OTW = """\
otw 1
design clk_design
time_unit 1e-9
clock clk 10 0 5
n clk c
n n_buf c
w 0 r c 0.10 0.14 - - 0.02 0.03 - -
w 0 r d 0.60 0.90 0.60 0.95 0.04 0.06 0.04 0.07
n n_gck c
n divq c
n n_gbuf c
n n[1] c
n n_and d
n n_en d
n din u
end 9 0 2
"""

PIN_OTW = """\
otw 1
clock clk 10 0 5
i -
p clk c
i u_buf
p A c
p X c
i u_icg
p CK c
p GCK c
p E d
i u_icg2
p E c
i u_ff1
p CK c
i u_div
p CK c
p Q c
i u_gbuf
p A c
i u_mem
p CLK c
i u_b[1]
p X c
i u_and
p A d
end 0 14 0
"""


@pytest.fixture
def design(tmp_path: Path) -> dict[str, Path]:
    files = {
        "lef": tmp_path / "cells.lef",
        "def": tmp_path / "design.def",
        "lib": tmp_path / "cells.lib",
        "sdc": tmp_path / "design.sdc",
        "net_otw": tmp_path / "nets.otw",
        "pin_otw": tmp_path / "pins.otw",
    }
    files["lef"].write_text(LEF, encoding="utf-8")
    files["def"].write_text(
        "VERSION 5.8 ;\nDESIGN clk_design ;\nUNITS DISTANCE MICRONS 1000 ;\n"
        f"COMPONENTS {len(COMPONENTS)} ;\n"
        + "".join(f"  - {name} {macro} + PLACED ( 0 0 ) N ;\n" for name, macro in COMPONENTS)
        + f"END COMPONENTS\nNETS {len(NETS)} ;\n"
        + "".join(f"  - {net} ;\n" for net in NETS)
        + "END NETS\nEND DESIGN\n",
        encoding="utf-8",
    )
    files["lib"].write_text(LIB, encoding="utf-8")
    files["sdc"].write_text(SDC, encoding="utf-8")
    files["net_otw"].write_text(NET_OTW, encoding="utf-8")
    files["pin_otw"].write_text(PIN_OTW, encoding="utf-8")
    return files


def instances_cli(design: dict[str, Path], *extra: str, lib: bool = True):
    """Run ``def-tools instances``; returns the result and the is_clock column."""
    output = design["def"].with_name("instances.csv")
    output.unlink(missing_ok=True)
    args = ["instances", "--lef", str(design["lef"]), "--def", str(design["def"])]
    if lib:
        args += ["--lib", str(design["lib"])]
    args += ["--columns", "instance,is_clock", "--output", str(output), *extra]
    result = CliRunner().invoke(cli.main, args)
    rows: dict[str, str] = {}
    if output.exists():
        lines = output.read_text(encoding="utf-8").splitlines()[1:]
        rows = dict(line.split(",") for line in lines)
    return result, rows


# --- Liberty clock roles ----------------------------------------------------


def test_clock_roles_separate_gates_endpoints_and_logic(design) -> None:
    roles = cell_clock_roles([design["lib"]])

    assert roles["ICG"].clock_gate and not roles["ICG"].endpoint
    assert roles["ICG"].inputs == ("CK",)  # the enable pin carries no clock
    assert roles["ICG"].outputs == ("GCK",)
    assert roles["ICG"].sequential  # is_seq is unchanged: it has a latch
    assert roles["DFF"].endpoint and roles["DFF"].clock_pins == ("CK",)
    assert roles["RAM"].endpoint and not roles["RAM"].sequential
    assert not roles["AND2"].endpoint and roles["AND2"].inputs == ("A", "B")


def test_clock_gate_without_pin_attributes_uses_the_latch_clock(tmp_path: Path) -> None:
    lib = tmp_path / "bare.lib"
    lib.write_text(
        "library (bare) {\n  cell (ICG) {\n"
        '    clock_gating_integrated_cell : "latch_posedge";\n'
        '    latch (IQ, IQN) { enable : "!CLK"; data_in : "EN"; }\n'
        "    pin (CLK) { direction : input; }\n"
        "    pin (EN) { direction : input; }\n"
        "    pin (GCLK) { direction : output; }\n"
        "  }\n}\n",
        encoding="utf-8",
    )

    role = cell_clock_roles([lib])["ICG"]

    assert role.inputs == ("CLK",)
    assert role.outputs == ("GCLK",)


# --- SDC forward trace (issue: datapath leak, ICG traversal) ----------------


def test_sdc_trace_goes_through_gates_and_skips_the_divider_datapath(design) -> None:
    from sdc_tools import clock_cells

    result = clock_cells(design["def"], [design["lib"]], design["sdc"])

    assert list(result.cells) == CLOCK_TREE


def test_sdc_trace_unpruned_reports_every_cell_it_touches(design) -> None:
    from sdc_tools import clock_cells

    result = clock_cells(design["def"], [design["lib"]], design["sdc"], prune=False)

    # u_and reads the divided clock but only ever reaches a data pin.
    assert list(result.cells) == ["u_and", *CLOCK_TREE]


def test_sdc_clock_cells_cli_no_prune(design) -> None:
    from sdc_tools.cli import main

    args = ["clock-cells", "--sdc", str(design["sdc"]), "--def", str(design["def"])]
    args += ["--lib", str(design["lib"])]
    pruned = CliRunner().invoke(main, args)
    unpruned = CliRunner().invoke(main, [*args, "--no-prune"])

    assert pruned.output.splitlines() == CLOCK_TREE
    assert unpruned.output.splitlines() == ["u_and", *CLOCK_TREE]


# --- OTW --------------------------------------------------------------------


def test_net_based_otw_marks_the_drivers_of_clock_nets(design) -> None:
    roles = cell_clock_roles([design["lib"]])

    result = clock_instances(design["net_otw"], design["def"], roles)

    assert list(result.cells) == CLOCK_TREE
    assert result.unmatched_nets == ()
    # u_box sits on a clock net but has no Liberty cell: reported, not guessed.
    assert result.unclassified == ("u_box",)


def test_pin_based_otw_gives_the_same_answer(design) -> None:
    roles = cell_clock_roles([design["lib"]])

    result = clock_instances(design["pin_otw"], design["def"], roles)

    assert list(result.cells) == CLOCK_TREE
    assert result.unmatched_pins == ()


def test_unmatched_objects_are_reported(design, tmp_path: Path) -> None:
    source = tmp_path / "extra.otw"
    source.write_text("otw 1\nn n_buf c\nn gone c\ni ghost\np A c\ni -\np nope c\nend 2 2 0\n", encoding="utf-8")

    result = clock_instances(source, design["def"], cell_clock_roles([design["lib"]]))

    assert result.cells == ("u_buf",)
    assert result.unmatched_nets == ("gone",)
    assert result.unmatched_pins == (("ghost", "A"), ("-", "nope"))
    assert len(result.warnings()) == 3


def test_source_for_another_design_is_an_error(design, tmp_path: Path) -> None:
    source = tmp_path / "other.otw"
    source.write_text("otw 1\nn some_other_clk c\nend 1 0 0\n", encoding="utf-8")

    with pytest.raises(ClockDataError, match="does not describe this design"):
        clock_instances(source, design["def"], cell_clock_roles([design["lib"]]))


def test_write_otw_round_trips_through_gzip(tmp_path: Path) -> None:
    path = tmp_path / "round.otw.gz"
    window = OtwWindow(0, "r", "c", (0.1, 0.14, None, None, 0.02, 0.03, None, None))

    write_otw(
        path,
        design="top",
        time_unit=1e-9,
        clocks=[OtwClock("clk", 10.0, 0.0, 5.0)],
        attributes=[("source", "unit test")],
        nets={"clk": "c", "d0": "d"},
        pins={("u1", "CK"): "c", ("-", "clk"): "c"},
        windows={"clk": [window]},
    )
    parsed = read_otw(path)

    assert gzip.open(path, "rt").readline() == "otw 1\n"
    assert parsed.design == "top"
    assert parsed.time_unit == 1e-9
    assert parsed.clocks == (OtwClock("clk", 10.0, 0.0, 5.0),)
    assert parsed.attributes == (("source", "unit test"),)
    assert parsed.nets == {"clk": "c", "d0": "d"}
    assert parsed.pins == {("u1", "CK"): "c", ("-", "clk"): "c"}
    assert parsed.windows == 1
    assert parsed.domains == {"clk": (0,)}  # derived from its window


def test_otw_carries_domains_frequency_slack_and_free_attributes(tmp_path: Path) -> None:
    path = tmp_path / "rich.otw"
    clocks = [
        OtwClock("clk", 10.0, 0.0, 5.0, (("source", "clk"),)),
        OtwClock("div2", 20.0, 0.0, 10.0, (("master", "0"),)),
    ]
    values = (0.1, 0.2, 0.1, 0.2, 0.01, 0.02, 0.01, 0.02)

    write_otw(
        path,
        time_unit=1e-9,
        cap_unit=1e-12,
        res_unit=1e3,
        slew_thresholds=(10, 90),
        attributes=[("corner", "ss_125c"), ("default_input_slew", "0.05")],
        clocks=clocks,
        nets={"gated": "c", "async_in": "u"},
        pins={("u_icg", "CK"): "c"},
        domains={"gated": [1, 0], ("u_icg", "CK"): [0]},
        object_attributes={"gated": {"freq": "5e7", "load": "0.0032"}},
        windows={
            "gated": [
                OtwWindow(
                    0, "r", "c", values,
                    rise_res=(0.59, 0.61), rise_slack=(0.31, 0.37), attributes=(("k", "2"),),
                )
            ],
            "async_in": [OtwWindow(None, "-", "u", values)],
        },
    )
    text = path.read_text(encoding="utf-8").splitlines()
    parsed = read_otw(path)
    data = read_clock_data(path)

    assert "n gated c clocks=0,1 freq=5e7 load=0.0032" in text
    assert (
        "w 0 r c 0.1 0.2 0.1 0.2 0.01 0.02 0.01 0.02 "
        "rise_res=0.59:0.61 rise_slack=0.31:0.37 k=2"
    ) in text
    assert (parsed.cap_unit, parsed.res_unit, parsed.slew_thresholds) == (1e-12, 1e3, (10.0, 90.0))
    assert ("corner", "ss_125c") in parsed.attributes
    assert "w - - u 0.1 0.2 0.1 0.2 0.01 0.02 0.01 0.02" in text
    assert text[-3:] == ["i u_icg", "p CK c clocks=0", "end 2 1 2"]
    assert parsed.clocks == tuple(clocks)
    assert parsed.domains == {"gated": (0, 1), ("u_icg", "CK"): (0,)}
    assert parsed.domain_names("gated") == ("clk", "div2")
    assert parsed.object_attributes == {"gated": {"freq": "5e7", "load": "0.0032"}}
    assert data.domains == {"gated": ("clk", "div2"), ("u_icg", "CK"): ("clk",)}


def test_write_otw_numbers_are_fixed_point_without_exponent(tmp_path: Path) -> None:
    path = tmp_path / "small.otw"
    values = (1e-5, 2.5, None, None, 1e-9, 0.0, None, None)

    write_otw(
        path,
        clocks=[OtwClock("clk", 10.0, 0.0, 5.0)],
        nets={"n": "d"},
        windows={"n": [OtwWindow(0, "r", "d", values, rise_slack=(-0.25, 1e-7))]},
        precision=7,
    )

    assert "w 0 r d 0.00001 2.5 - - 0 0 - - rise_slack=-0.25:0.0000001" in path.read_text()
    assert read_otw(path).windows == 1


def test_gzip_members_may_be_concatenated(tmp_path: Path) -> None:
    # A writer may compress groups independently and append the members.
    path = tmp_path / "members.otw.gz"
    parts = [b"otw 1\nn a c\n", b"i u\np Z c\n", b"n b d\nend 2 1 0\n"]
    path.write_bytes(b"".join(gzip.compress(part) for part in parts))

    parsed = read_otw(path)

    assert parsed.nets == {"a": "c", "b": "d"}
    assert parsed.pins == {("u", "Z"): "c"}


def test_write_otw_refuses_what_the_reader_would_reject(tmp_path: Path) -> None:
    window = OtwWindow(0, "r", "c", (None,) * 8)
    clocks = [OtwClock("clk", 10.0, 0.0, 5.0)]

    with pytest.raises(ValueError, match="clock window"):
        write_otw(tmp_path / "a.otw", clocks=clocks, nets={"n": "d"}, windows={"n": [window]})
    with pytest.raises(ValueError, match="role"):
        write_otw(tmp_path / "b.otw", nets={"n": "C"})
    with pytest.raises(ValueError, match="token"):
        write_otw(tmp_path / "c.otw", nets={"a b": "c"})


# --- backend seam -----------------------------------------------------------


def test_backend_is_chosen_by_prefix_or_suffix(tmp_path: Path) -> None:
    assert parse_source("otw:/x/y.txt") == ("otw", Path("/x/y.txt"))
    assert parse_source("/x/design.otw.gz") == ("otw", Path("/x/design.otw.gz"))
    assert parse_source("/x/design.twf.gz") == ("twf", Path("/x/design.twf.gz"))
    with pytest.raises(ClockDataBackendError, match="BACKEND:PATH"):
        parse_source("/x/design.txt")


def test_missing_backend_and_malformed_source_are_distinct_errors(tmp_path: Path) -> None:
    twf = tmp_path / "design.twf"
    twf.write_text("(TIMING_WINDOWS)\n", encoding="utf-8")
    bad = tmp_path / "bad.otw"
    bad.write_text("otw 1\nn clk q\nend 1 0 0\n", encoding="utf-8")
    empty = tmp_path / "empty.otw"
    empty.write_text("otw 1\nend 0 0 0\n", encoding="utf-8")

    with pytest.raises(ClockDataBackendError, match="no `twf` clock-data backend"):
        read_clock_data(twf)
    with pytest.raises(ClockDataFormatError, match="bad.otw:2"):
        read_clock_data(bad)
    with pytest.raises(ClockDataFormatError, match="describes no net"):
        read_clock_data(empty)


def test_registered_backend_supplies_the_answer(design) -> None:
    def reader(path: Path) -> ClockData:
        return ClockData("unit-vendor", path, nets={"n_gck": "c"}, instances={"u_post": "c"})

    register_backend("unit-vendor", reader)
    result = clock_instances(
        f"unit-vendor:{design['def']}", design["def"], cell_clock_roles([design["lib"]])
    )

    assert result.cells == ("u_icg", "u_post")
    with pytest.raises(ValueError, match="built in"):
        register_backend("otw", reader)


def test_backend_with_an_unknown_role_is_rejected(tmp_path: Path) -> None:
    register_backend("unit-bad", lambda path: ClockData("unit-bad", path, nets={"a": "C"}))

    with pytest.raises(ClockDataFormatError, match="role 'C'"):
        read_clock_data(f"unit-bad:{tmp_path}")


# --- def-tools CLI ----------------------------------------------------------


@pytest.mark.parametrize("source", ["net_otw", "pin_otw"])
def test_instances_cli_fills_is_clock_from_otw(design, source: str) -> None:
    result, rows = instances_cli(design, "--clock-data", str(design[source]))

    assert result.exit_code == 0, result.output
    assert sorted(name for name, flag in rows.items() if flag == "true") == CLOCK_TREE
    assert len(rows) == len(COMPONENTS)


def test_instances_cli_default_columns_gain_is_clock(design, tmp_path: Path) -> None:
    output = tmp_path / "out.csv"
    result = CliRunner().invoke(
        cli.main,
        ["instances", "--lef", str(design["lef"]), "--def", str(design["def"])]
        + ["--lib", str(design["lib"]), "--clock-data", f"otw:{design['net_otw']}"]
        + ["--output", str(output)],
    )

    assert result.exit_code == 0, result.output
    header = output.read_text(encoding="utf-8").splitlines()[0]
    assert header == "instance,base_cell,llx,lly,urx,ury,is_seq,is_clock"
    assert "u_box" in result.stderr  # the unclassified instance is warned about


def test_instances_cli_twf_without_a_backend_exits_3(design, tmp_path: Path) -> None:
    twf = tmp_path / "design.twf.gz"
    twf.write_bytes(b"")

    result, rows = instances_cli(design, "--twf", str(twf))

    assert result.exit_code == 3
    assert "no `twf` clock-data backend is installed" in result.stderr
    assert rows == {}  # never an all-false is_clock column


def test_instances_cli_twf_uses_a_plugged_in_backend(
    design, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    plugin = tmp_path / "plugin"
    plugin.mkdir()
    (plugin / "site_twf_reader.py").write_text(
        "from def_tools.clock_data import ClockData\n"
        "def read(path):\n"
        "    return ClockData('twf', path, pins={('u_icg', 'GCK'): 'c', ('u_icg', 'E'): 'd'})\n",
        encoding="utf-8",
    )
    monkeypatch.syspath_prepend(str(plugin))
    monkeypatch.setenv("OPEN_EDA_CLOCK_DATA_BACKENDS", "twf=site_twf_reader:read")
    twf = tmp_path / "design.twf"
    twf.write_text("vendor content this project never parses\n", encoding="utf-8")

    result, rows = instances_cli(design, "--twf", str(twf))

    assert result.exit_code == 0, result.output
    assert [name for name, flag in rows.items() if flag == "true"] == ["u_icg"]


def test_instances_cli_malformed_source_exits_4(design, tmp_path: Path) -> None:
    bad = tmp_path / "bad.otw"
    bad.write_text("otw 1\nn clk c\nn clk d\nend 2 0 0\n", encoding="utf-8")

    result, _rows = instances_cli(design, "--clock-data", str(bad))

    assert result.exit_code == 4
    assert "declared more than once" in result.stderr


def test_instances_cli_net_source_needs_liberty(design) -> None:
    result, _rows = instances_cli(design, "--clock-data", str(design["net_otw"]), lib=False)

    assert result.exit_code == 1
    assert "--lib" in result.stderr


def test_instances_cli_clock_inputs_are_alternatives(design, tmp_path: Path) -> None:
    names = tmp_path / "names.txt"
    names.write_text("u_buf\n", encoding="utf-8")

    result, _rows = instances_cli(
        design, "--clock-data", str(design["net_otw"]), "--clock-cells", str(names)
    )

    assert result.exit_code == 1
    assert "alternatives" in result.stderr


def test_offline_entry_point_reports_the_same_exit_codes(design, tmp_path: Path, capfd) -> None:
    from def_tools.instances import main

    base = ["--lef", str(design["lef"]), "--def", str(design["def"]), "--lib", str(design["lib"])]
    twf = tmp_path / "x.twf"
    twf.write_text("", encoding="utf-8")

    assert main([*base, "--twf", str(twf)]) == 3
    assert main([*base, "--clock-data", str(design["pin_otw"])]) == 0
    assert "u_icg,ICG,0.0,0.0,1.0,1.0,true,true" in capfd.readouterr().out


def test_clock_data_command_reports(design, tmp_path: Path) -> None:
    runner = CliRunner()
    objects = runner.invoke(cli.main, ["clock-data", "--source", str(design["pin_otw"])])
    instances = runner.invoke(
        cli.main,
        ["clock-data", "--source", str(design["net_otw"]), "--report", "instances"]
        + ["--def", str(design["def"]), "--lib", str(design["lib"])],
    )
    windows_csv = tmp_path / "windows.csv"
    windows = runner.invoke(
        cli.main,
        ["clock-data", "--source", str(design["net_otw"]), "--report", "windows"]
        + ["--output", str(windows_csv)],
    )

    assert objects.exit_code == 0, objects.output
    assert objects.stdout.splitlines()[:3] == [
        "kind,instance,name,role",
        "pin,-,clk,c",
        "pin,u_buf,A,c",
    ]
    assert instances.stdout.splitlines() == CLOCK_TREE
    assert windows.exit_code == 0, windows.output
    assert windows_csv.read_text(encoding="utf-8").splitlines()[2] == (
        "net,,n_buf,c,,0,r,c,0.10,0.14,,,0.02,0.03,,,,,,,"
    )
