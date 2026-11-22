"""Measure release Liberty workloads and emit JSON.

Run with the interpreter used for the native release build:
    .venv/bin/python scripts/liberty_bench.py parse [INPUT] [--iterations 7]
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import platform
import resource
import statistics
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path

import liberty_tools as lt
from liberty_tools import _native

RUNTIME_ROOT = Path(_native.__file__).resolve().parents[1]

ROOT = Path(__file__).resolve().parents[1]
IHP = Path(
    "/home/mylesp/pdk/IHP-Open-PDK/ihp-sg13g2/libs.ref/sg13g2_stdcell/lib/"
    "sg13g2_stdcell_typ_1p20V_25C.lib"
)


def generate(
    path: Path,
    count: int,
    *,
    tables: bool = False,
    comment: str = "",
    one_pin: bool = False,
) -> None:
    """Write a deterministic library with fixed-width cell names."""
    axis = "1, 2, 3, 4, 5, 6, 7"
    values = ", ".join(f'"{axis}"' for _ in range(7))
    timing = (
        (
            f'timing () {{ related_pin : A; cell_rise (t) {{ index_1 ("{axis}"); '
            f'index_2 ("{axis}"); values ({values}); }} }}'
        )
        if tables
        else ""
    )
    output_pin = "" if one_pin else f" pin (Z) {{ direction : output; {timing} }}"
    with path.open("w", encoding="utf-8") as out:
        out.write("library (bench) {\n")
        for n in range(count):
            out.write(
                f"  cell (c{n:07d}) {{ {comment} area : 1; pin (A) {{ direction : input; }}{output_pin} }}\n"
            )
        out.write("}\n")


def measure(work: Callable[[], object], iterations: int) -> dict:
    work()
    samples = []
    for _ in range(iterations):
        start = time.perf_counter()
        result = work()
        samples.append((time.perf_counter() - start) * 1000)
        del result
    return {"median_ms": statistics.median(samples), "samples_ms": samples}


def frame_memory(path: Path, cell: str | None) -> dict:
    import polars  # noqa: F401

    doc = lt.parse_file(path)
    baseline = peak_rss()
    frame = doc.to_polars(cell=cell)
    peak = peak_rss()
    return {
        "rss_before_kib": baseline,
        "rss_after_kib": peak,
        "frame_bytes": frame.estimated_size(),
        "frame_shape": frame.shape,
        "schema": str(frame.schema),
    }


def format_cli(path: Path, output: Path) -> None:
    from liberty_tools.cli import format_command

    with redirect_stdout(StringIO()):
        format_command.main([str(path), "-o", str(output)], standalone_mode=False)


def writer_memory(path: Path) -> dict:
    doc = lt.parse_file(path)
    for name in dict.fromkeys(doc.cells()):
        doc.cell(name).set_area(1.12345)
    baseline = peak_rss()
    rendered = doc.to_string()
    peak = peak_rss()
    return {
        "rss_before_kib": baseline,
        "rss_after_kib": peak,
        "output_sha256": hashlib.sha256(rendered.encode()).hexdigest(),
    }


def format_memory(path: Path, phase: str) -> dict:
    from liberty_format import format_text, reindent

    if phase == "cli":
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "output.lib"
            baseline = peak_rss()
            start = time.perf_counter()
            format_cli(path, output)
            elapsed = time.perf_counter() - start
            peak = peak_rss()
            digest = hashlib.sha256(output.read_bytes()).hexdigest()
    else:
        raw = path.read_bytes()
        data = (gzip.decompress(raw) if path.suffix == ".gz" else raw).decode()
        del raw
        baseline = peak_rss()
        start = time.perf_counter()
        output_text = (reindent if phase == "reindent" else format_text)(data)
        elapsed = time.perf_counter() - start
        peak = peak_rss()
        digest = hashlib.sha256(output_text.encode()).hexdigest()
    return {
        "rss_before_kib": baseline,
        "rss_after_kib": peak,
        "elapsed_ms": elapsed * 1000,
        "output_sha256": digest,
    }


def peak_rss() -> int:
    # Linux ru_maxrss can include the parent's peak across fork and exec.
    if sys.platform == "linux":
        for line in Path("/proc/self/status").read_text().splitlines():
            if line.startswith("VmHWM:"):
                return int(line.split()[1])
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return rss // 1024 if sys.platform == "darwin" else rss


def worker(workload: str, path: Path, *args: str) -> dict:
    output = subprocess.check_output(
        [sys.executable, __file__, workload, str(path), *args], text=True
    )
    return json.loads(output.splitlines()[-1])


def compare_scaling(tmp: Path, iterations: int) -> list[dict]:
    def write_arcs(path: Path, arcs: str):
        path.write_text(
            "library (l) { lu_table_template (t) { variable_1 : input_net_transition; } "
            "cell (C) { pin (Y) { direction : output; " + arcs + " } } }"
        )

    a, b = tmp / "a.lib", tmp / "b.lib"
    cases = []
    for count in (512, 1024):
        axis = ", ".join(str(i) for i in range(count))
        write_arcs(
            a,
            f'timing () {{ cell_rise (t) {{ index_1 ("{axis}"); values ("{axis}"); }} }}',
        )
        doc = lt.parse_file(a)
        result = lt.compare(doc, doc)
        assert len(result) == count and not result.has_differences
        cases.append(
            {
                "phase": "equal axis",
                "axis_samples": count,
                **measure(lambda: lt.compare(doc, doc), iterations),
            }
        )
    for count in (64, 128):
        arcs = []
        for i in range(count):
            term = " & ".join(
                f"X{bit}" if i & (1 << bit) else f"!X{bit}" for bit in range(7)
            )
            arcs.append(
                f'timing () {{ when : "{term}"; cell_rise (t) {{ index_1 ("1, 2"); values ("1, 2"); }} }}'
            )
        write_arcs(a, "\n".join(arcs))
        write_arcs(b, "\n".join(reversed(arcs)))
        first, second = lt.parse_file(a), lt.parse_file(b)
        result = lt.compare(first, second)
        assert len(result) == count * 2 and not result.has_differences
        cases.append(
            {
                "phase": "reversed minterms",
                "arcs": count,
                **measure(lambda: lt.compare(first, second), iterations),
            }
        )
    return cases


def boolean_scaling(tmp: Path, iterations: int) -> list[dict]:
    condition = " | ".join(f"X{i}" for i in range(12))
    path = tmp / "condition.lib"
    cases = []
    for count in (1, 16):
        arc = f'timing () {{ when : "{condition}"; cell_rise (scalar) {{ values ("1"); }} }}'
        path.write_text(f"library (l) {{ cell (C) {{ pin (Y) {{ {arc * count} }} }} }}")
        doc = lt.parse_file(path)
        query = lt.BooleanExpression(condition)
        assert len(doc.timing_tables(when=query.raw)) == count
        cases.append(
            {
                "repeats": count,
                "variables": 12,
                **measure(lambda: doc.timing_tables(when=query.raw), iterations),
            }
        )
    return cases


def combo_case(tmp: Path, path: Path, data: bytes, iterations: int) -> dict:
    source, output = tmp / "combo-input.lib.gz", tmp / "combo-output.lib.gz"
    source.write_bytes(gzip.compress(data, mtime=0))
    command = [
        sys.executable,
        str(RUNTIME_ROOT / "scripts/build_combo_lib.py"),
        str(output),
        "combo",
        "--copies",
        "16",
        str(source),
    ]

    def build():
        subprocess.run(
            command,
            check=True,
            stdout=subprocess.DEVNULL,
            env=os.environ | {"GZ_LEVEL": "6"},
        )

    timings = measure(build, iterations)
    rendered = gzip.decompress(output.read_bytes())
    return {
        "input": path.name,
        "decoded_bytes": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
        "copies": 16,
        "gzip_level": 6,
        "output_sha256": hashlib.sha256(rendered).hexdigest(),
        **timings,
    }


def annotation_cases(iterations: int) -> list[dict]:
    javascript = r"""
    const fs = require("node:fs");
    const vm = require("node:vm");
    const crypto = require("node:crypto");
    const source = fs.readFileSync(process.argv[1], "utf8")
      .split('document.addEventListener("keydown", _treeKey);')[0];
    const annotate = vm.runInNewContext(source + "\n_annotateIndices");
    const results = [];
    for (const count of [2000, 4000, 8000]) {
      const text = 'vector (t) {\n  index_1 ("1");\n  values ("2");\n}\n'.repeat(count);
      const expected = annotate(text, {t: ["slew"]});
      const samples = [];
      for (let i = 0; i < Number(process.argv[2]); i++) {
        const start = performance.now();
        const result = annotate(text, {t: ["slew"]});
        samples.push(performance.now() - start);
        if (result !== expected) throw new Error("annotation output changed");
      }
      const ordered = samples.slice().sort((a, b) => a - b);
      const median = (ordered[Math.floor((ordered.length - 1) / 2)] +
        ordered[Math.floor(ordered.length / 2)]) / 2;
      results.push({vectors: count, input_bytes: text.length, median_ms: median, samples_ms: samples,
        output_sha256: crypto.createHash("sha256").update(expected).digest("hex")});
    }
    console.log(JSON.stringify(results));
    """
    return json.loads(
        subprocess.check_output(
            [
                "node",
                "-e",
                javascript,
                str(RUNTIME_ROOT / "viewer/static/app.js"),
                str(iterations),
            ],
            text=True,
        )
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "workload",
        choices=(
            "parse",
            "select",
            "attributes",
            "traverse",
            "frame",
            "format",
            "viewer",
            "compare",
            "writer",
            "boolean",
            "combo",
            "annotation",
            "_frame_memory",
            "_format_memory",
            "_writer_memory",
        ),
    )
    parser.add_argument("input", nargs="?", type=Path)
    parser.add_argument("--cell", default=None)
    parser.add_argument(
        "--phase", choices=("reindent", "string", "cli"), default="string"
    )
    parser.add_argument("--iterations", type=int, default=5)
    parser.add_argument(
        "--revision", default=None, help="Override revision for a saved baseline"
    )
    args = parser.parse_args()
    if args.workload == "_frame_memory":
        print(json.dumps(frame_memory(args.input, args.cell)))
        return
    if args.workload == "_format_memory":
        print(json.dumps(format_memory(args.input, args.phase)))
        return
    if args.workload == "_writer_memory":
        print(json.dumps(writer_memory(args.input)))
        return
    if args.iterations < 5:
        parser.error("use at least five iterations")
    import polars

    with tempfile.TemporaryDirectory(prefix="liberty-bench-") as scratch:
        tmp = Path(scratch)
        paths = (
            [args.input]
            if args.input
            else [ROOT / "dev.lib", *([IHP] if IHP.is_file() else [])]
        )
        sizes = {
            "parse": [4000],
            "select": [4000],
            "attributes": [4000],
            "traverse": [4000, 8000, 16000],
            "frame": [2000],
            "format": [100000],
            "writer": [500, 1000, 2000, 4000],
            "viewer": [16000],
        }
        if not args.input:
            for size in sizes.get(args.workload, []):
                path = tmp / f"generated-{size}.lib"
                generate(
                    path,
                    size,
                    tables=args.workload == "frame",
                    one_pin=args.workload == "format",
                    comment="/*" + "x" * 2048 + "*/"
                    if args.workload == "writer"
                    else "",
                )
                paths.append(path)
        cases = []
        if args.workload in ("frame", "writer"):
            memory_workload = (
                "_frame_memory" if args.workload == "frame" else "_writer_memory"
            )
            memories = {path: worker(memory_workload, path) for path in paths}
        else:
            memories = {}
        if args.workload == "boolean":
            cases.extend(boolean_scaling(tmp, args.iterations))
            paths = []
        if args.workload == "annotation":
            cases.extend(annotation_cases(args.iterations))
            paths = []
        for path in paths:
            raw = path.read_bytes()
            data = gzip.decompress(raw) if path.suffix == ".gz" else raw
            if args.workload == "combo":
                cases.append(combo_case(tmp, path, data, args.iterations))
                continue
            if args.workload in ("parse", "select") and path.suffix != ".gz":
                zipped = tmp / f"{path.name}.gz"
                zipped.write_bytes(gzip.compress(data, mtime=0))
                inputs = (path, zipped)
            else:
                inputs = (path,)
            for source in inputs:
                results = run_case(args.workload, source, args.iterations, data)
                for result in results:
                    measured_memory = (
                        memories.get(source, {})
                        if result.get("phase") in ("whole", "render")
                        else {}
                    )
                    cases.append(result | measured_memory)
        if args.workload == "compare" and args.input is None:
            cases.extend(compare_scaling(tmp, args.iterations))

    print(
        json.dumps(
            {
                "workload": args.workload,
                "revision": args.revision
                or subprocess.check_output(
                    ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
                ).strip(),
                "native": _native.__file__,
                "profile": "release",
                "python": platform.python_version(),
                "polars": polars.__version__,
                "machine": platform.platform(),
                "iterations": args.iterations,
                "cases": cases,
            },
            indent=2,
        )
    )


def _sequential_flags(path: Path) -> list[tuple[str, bool]]:
    flags = getattr(_native, "_sequential_flags", None)
    if flags is not None:
        return flags(str(path))
    doc = lt.parse_file(path, skip_tables=True)
    return [
        (cell.name, cell.ff() is not None or cell.latch() is not None)
        for cell in map(doc.cell, dict.fromkeys(doc.cells()))
    ]


def run_case(workload: str, path: Path, iterations: int, data: bytes) -> list[dict]:
    base = {
        "input": path.name,
        "input_bytes": path.stat().st_size,
        "decoded_bytes": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
    }
    if workload == "format":
        from liberty_format import format_text, reindent

        original = data.decode()
        formatted = reindent(original)
        compare_tokens = getattr(_native, "_compare_token_strings", None)

        def verify():
            if compare_tokens is not None:
                assert compare_tokens(original, formatted)[0]
            else:
                assert _native.tokenize_str(original) == _native.tokenize_str(formatted)

        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "formatted.lib"
            cli_timing = measure(lambda: format_cli(path, output), iterations)
        return [
            base
            | {"phase": "reindent"}
            | measure(lambda: reindent(original), iterations)
            | worker("_format_memory", path, "--phase", "reindent"),
            base | {"phase": "verify"} | measure(verify, iterations),
            base
            | {"phase": "format_text"}
            | measure(lambda: format_text(original), iterations)
            | worker("_format_memory", path, "--phase", "string"),
            base
            | {"phase": "CLI"}
            | cli_timing
            | worker("_format_memory", path, "--phase", "cli"),
        ]
    index = lt.LibraryIndex.open(path)
    names = index.cell_names()
    base["cells"] = len(names)
    if workload == "parse":
        assert list(dict.fromkeys(lt.parse_file(path).cells())) == names
        return [base | measure(lambda: lt.parse_file(path), iterations)]
    if workload == "select":
        choices = [names[0], names[-1]] if names else []
        absent = "__absent__"
        while absent in names:
            absent += "_"
        return [
            base
            | {"cell": name}
            | measure(lambda: lt.parse_file(path, cells=[name]), iterations)
            for name in dict.fromkeys([*choices, absent])
        ]
    if workload == "attributes":
        return [
            base
            | {"phase": "open"}
            | measure(lambda: lt.LibraryIndex.open(path), iterations),
            base
            | {"phase": "structural sequential flags"}
            | measure(lambda: _sequential_flags(path), iterations),
        ]
    if workload == "viewer":
        from viewer.data import LibertyData

        viewer = LibertyData.load(str(path))
        return [
            base | {"cell": name} | measure(lambda: viewer.cell_tree(name), iterations)
            for name in names[:2]
        ] + [
            base
            | {"phase": "filtered page"}
            | measure(lambda: viewer.cell_names("0", 100, 100), iterations)
        ]
    doc = lt.parse_file(path, skip_tables=workload == "traverse")
    if workload == "traverse":
        return [
            base
            | measure(lambda: sum(doc.cell(n).area or 0 for n in names), iterations)
        ]
    if workload == "frame":
        cases = [base | {"phase": "whole"} | measure(doc.to_polars, iterations)]
        if names:
            cases.append(
                base
                | {"phase": "filtered", "cell": names[0]}
                | worker("_frame_memory", path, "--cell", names[0])
                | measure(lambda: doc.to_polars(cell=names[0]), iterations)
            )
        return cases
    if workload == "compare":
        result = lt.compare(doc, doc)
        return [
            base
            | {"phase": "compare", "entries": len(result)}
            | measure(lambda: lt.compare(doc, doc), iterations),
            base
            | {"phase": "summary", "entries": len(result)}
            | measure(result.summary, iterations),
        ]
    if workload == "writer":
        start = time.perf_counter()
        for name in names:
            doc.cell(name).set_area(1.12345)
        setup = (time.perf_counter() - start) * 1000
        rendered = doc.to_string()
        raw = rendered.encode()
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "written.lib"
            write = measure(lambda: output.write_bytes(raw), iterations)
            assert output.read_bytes() == raw
        return [
            base
            | {
                "phase": "render",
                "setup_ms": setup,
                "output_sha256": hashlib.sha256(raw).hexdigest(),
            }
            | measure(doc.to_string, iterations),
            base | {"phase": "file I/O", "output_bytes": len(raw)} | write,
        ]
    raise ValueError(workload)


if __name__ == "__main__":
    main()
