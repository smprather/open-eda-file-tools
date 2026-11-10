from __future__ import annotations

import tomllib

from click.testing import CliRunner

from def_tools import benchmark_targets, write_io_config
from def_tools.cli import main


def test_write_io_config_benchmarks_target_and_writes_toml(tmp_path):
    target = tmp_path / "scratch"
    output = tmp_path / "def-tools-io-config.toml"

    results = write_io_config([target], output, probe_mib=1, engine="internal")

    assert len(results) == 1
    assert results[0].writable is True
    assert results[0].seq_write_mib_s > 0
    assert results[0].seq_read_mib_s > 0
    config = tomllib.loads(output.read_text(encoding="utf-8"))
    resolved = str(target.resolve())
    if results[0].kind == "ram":
        # tmpfs-backed tmp_path (e.g. /tmp on tmpfs) is genuinely RAM.
        assert resolved in config["storage"]["ram_dirs"]
    else:
        assert resolved in config["storage"]["scratch_dirs"]
    assert config["cache"]["enabled"] is True
    assert str(target.resolve()) in config["benchmarks"]


def test_benchmark_targets_reports_unwritable_file(tmp_path):
    target_file = tmp_path / "not-a-dir"
    target_file.write_text("x", encoding="utf-8")

    result = benchmark_targets([target_file], probe_mib=1, engine="internal")[0]

    assert result.writable is False
    assert result.error


def test_cli_configure_io(tmp_path):
    target = tmp_path / "fast"
    output = tmp_path / "io.toml"
    runner = CliRunner()

    result = runner.invoke(
        main,
        [
            "configure-io",
            "--target",
            str(target),
            "--output",
            str(output),
            "--probe-mib",
            "1",
            "--engine",
            "internal",
        ],
    )

    assert result.exit_code == 0, result.output
    assert "wrote" in result.output
    config = tomllib.loads(output.read_text(encoding="utf-8"))
    assert config["io"]["stage_while_parsing"] is True
