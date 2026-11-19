"""Time every native DEF entry point on one design directory.

The directory holds ``design.def`` (or ``design.def.gz``) and either
``design.lef`` or ``lefs.txt`` (one LEF path per line), plus an optional
``bench.cfg`` of ``key=value`` lines overriding the defaults below. A
``tests/gen/design_gen.py`` output directory works as is.

Usage:
    uv run --no-sync python scripts/def_bench.py DIR [--threads N] [--reps 3]

Build first with ``uv run maturin develop --release``; plain ``uv run`` can
install a stale cached wheel.
"""

from __future__ import annotations

import argparse
import tempfile
import time
from collections.abc import Callable
from pathlib import Path

from liberty_tools import _native

DEFAULTS = {
    "net": "n_1000",  # a signal net for net-shapes
    "layer": "met2",
    "power": "VPWR",  # a power net for net-shapes
    "player": "met1",
    "clock": "port:clk",  # clock root for the clock trace
}


def design_inputs(directory: Path) -> tuple[Path, list[str], dict[str, str]]:
    """The DEF, its LEFs and the bench settings of a design directory."""
    def_path = directory / "design.def"
    if not def_path.exists():
        def_path = directory / "design.def.gz"
    lefs_file = directory / "lefs.txt"
    lefs = lefs_file.read_text().split() if lefs_file.exists() else [str(directory / "design.lef")]
    settings = dict(DEFAULTS)
    cfg = directory / "bench.cfg"
    if cfg.exists():
        for line in cfg.read_text().split():
            key, _, value = line.partition("=")
            settings[key] = value
    return def_path, lefs, settings


def main() -> None:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("directory", type=Path)
    parser.add_argument("--threads", type=int, default=None)
    parser.add_argument("--reps", type=int, default=3)
    args = parser.parse_args()
    _native.set_threads(args.threads)
    def_path, lefs, cfg = design_inputs(args.directory)

    with tempfile.TemporaryDirectory() as scratch:
        out = Path(scratch)

        def timed(name: str, work: Callable[[], object]) -> None:
            best, note = float("inf"), ""
            for _ in range(args.reps):
                start = time.perf_counter()
                try:
                    work()
                except RuntimeError as err:
                    note = f"  (error: {str(err)[:70]})"
                best = min(best, time.perf_counter() - start)
            print(f"{name:24s} {best:7.3f} s{note}", flush=True)

        timed("instance_bboxes iter", lambda: sum(1 for _ in _native.instance_bboxes(lefs, def_path)))
        timed("instances csv", lambda: _native.write_instance_bbox_csv(lefs, def_path, out / "i.csv"))
        timed("instances parquet", lambda: _native.write_instance_parquet(lefs, def_path, out / "i.parquet", 65536))
        timed("instances table", lambda: _native.write_instance_table(lefs, def_path, out / "i.txt"))
        timed("net-shapes power", lambda: _native.write_net_shapes_csv(def_path, cfg["power"], cfg["player"], out / "p.csv"))
        timed("net-shapes signal", lambda: _native.write_net_shapes_csv(def_path, cfg["net"], cfg["layer"], out / "s.csv"))
        timed("cache create", lambda: _native.write_def_cache(def_path, out / "cache"))
        timed("clock trace", lambda: _native.clock_tree_cells(def_path, [cfg["clock"]], [], True))
    print(f"threads: {_native.get_threads()}")


if __name__ == "__main__":
    main()
