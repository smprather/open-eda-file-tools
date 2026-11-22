"""Verify the decompressed output and cell order from the combo builder."""

import gzip
import subprocess
import sys
from pathlib import Path

import liberty_tools as lt


def test_combo_copies_preserve_nested_groups_and_source_order(tmp_path, monkeypatch):
    script = Path(__file__).resolve().parents[1] / "scripts" / "build_combo_lib.py"
    path_a = tmp_path / "a.lib.gz"
    path_b = tmp_path / "b.lib.gz"
    output = tmp_path / "combo.lib.gz"
    values = '          "0, 1",\n' * 1023 + '          "0, 1"\n'
    cell_a = (
        '  cell (first) {\n'
        '    extension () {\n'
        '      cell (nested) {}\n'
        '    }\n'
        '    pin (Z) {\n'
        '      timing () {\n'
        '        cell_rise (t) {\n'
        '          values (\n' + values +
        '          );\n'
        '        }\n'
        '      }\n'
        '    }\n'
        '  }\n'
    )
    cell_b = '    cell (second) { area : 1; }\n'
    path_a.write_bytes(gzip.compress(
        (f'/* leading comment */\n  library (a) {{\n{cell_a}\n}}\n').encode()
    ))
    path_b.write_bytes(gzip.compress(
        (f'library (b) {{\n}}\n{cell_b}}}\n').encode()
    ))

    monkeypatch.setenv("GZ_LEVEL", "1")
    subprocess.run(
        [sys.executable, str(script), str(output), "combined", "--copies", "3", str(path_a),
         str(path_b)],
        check=True,
        capture_output=True,
    )

    assert gzip.decompress(output.read_bytes()) == (
        'library (combined) {\n'
        + cell_a + '\n' + cell_b
        + cell_a.replace('(first)', '(first_c2)') + '\n' + cell_b.replace('(second)', '(second_c2)')
        + cell_a.replace('(first)', '(first_c3)') + '\n' + cell_b.replace('(second)', '(second_c3)')
        + '}\n'
    ).encode()
    assert lt.parse_file(output).cells() == [
        "first", "second", "first_c2", "second_c2", "first_c3", "second_c3",
    ]
