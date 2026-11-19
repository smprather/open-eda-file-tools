"""Compare a reference build of the native module with the installed one.

Every DEF output is produced by both builds and compared byte for byte (or
value for value): instance CSV/table/Parquet with default and renamed
columns, the instance iterator, net shapes (file and iterator), every cache
file, the clock trace, clock-object resolution and the clock-instances CSV
(direct and from the cache). Errors count as outputs: both builds must fail
the same way.

Build the reference from the commit to compare against, outside the tree:

    git worktree add /tmp/ref <commit>
    (cd /tmp/ref && CARGO_TARGET_DIR=/tmp/ref-target cargo build --release --lib)

Usage:
    uv run --no-sync python scripts/def_equivalence.py /tmp/ref-target/release/lib_native.so DIR [--threads N]

DIR is a design directory as described in ``scripts/def_bench.py``. Exits 1
on any difference. A change that alters output on purpose is expected to
show up here; list the intended differences in the commit message.
"""

from __future__ import annotations

import argparse
import filecmp
import importlib.util
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path
from types import ModuleType

from liberty_tools import _native as new

sys.path.insert(0, str(Path(__file__).resolve().parent))
from def_bench import design_inputs  # noqa: E402


def load_reference(path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location("_native", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def outcome(work: Callable[[], object]) -> tuple[object, ...]:
    try:
        return ("ok", work())
    except Exception as err:  # noqa: BLE001 - errors are compared, not handled
        return ("error", type(err).__name__, str(err))


def main() -> int:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("reference", type=Path)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--threads", type=int, default=None)
    args = parser.parse_args()
    old = load_reference(args.reference)
    new.set_threads(args.threads)
    def_path, lefs, cfg = design_inputs(args.directory)
    deff = str(def_path)
    failures = 0

    def check(name: str, a: object, b: object) -> None:
        nonlocal failures
        same = a == b
        failures += not same
        print(f"{'ok  ' if same else 'DIFF'} {name}", flush=True)

    with tempfile.TemporaryDirectory() as scratch:
        tmp = Path(scratch)

        def both_files(name: str, call: Callable[[ModuleType, str], object]) -> None:
            a, b = tmp / f"old_{name}", tmp / f"new_{name}"
            check(f"{name} result", outcome(lambda: call(old, str(a))), outcome(lambda: call(new, str(b))))
            check(f"{name} bytes", a.read_bytes() if a.exists() else None, b.read_bytes() if b.exists() else None)

        flags = (["sg13g2_dfrbpq_1", "sky130_fd_sc_hd__dfxtp_1"], ["u0", "core/u_dec_0/reg[0]/U0"])
        column_sets = (
            None,
            [("instance", "i"), ("status", "s"), ("orient", "o"), ("llx", "x"), ("ury", "y")],
            [("base_cell", "cell"), ("is_seq", "seq"), ("is_clock", "clk")],
        )
        for index, columns in enumerate(column_sets):
            for fn in ("write_instance_bbox_csv", "write_instance_table"):
                extra = (columns, *flags) if columns else ()
                both_files(f"{fn}_{index}", lambda m, out, fn=fn, extra=extra: getattr(m, fn)(lefs, deff, out, *extra))
        both_files("parquet", lambda m, out: m.write_instance_parquet(lefs, deff, out, 50000))
        check(
            "instance_bboxes iter",
            outcome(lambda: [b.as_tuple() for b in old.instance_bboxes(lefs, deff)]),
            outcome(lambda: [b.as_tuple() for b in new.instance_bboxes(lefs, deff)]),
        )

        for net, layer in ((cfg["power"], cfg["player"]), (cfg["net"], cfg["layer"]), ("no_such_net", "x")):
            both_files(f"net_shapes_{net}_{layer}", lambda m, out, net=net, layer=layer: m.write_net_shapes_csv(deff, net, layer, out))
            check(
                f"net_shapes iter {net} {layer}",
                outcome(lambda net=net, layer=layer: [repr(s) for s in old.net_shapes(deff, net, layer)]),
                outcome(lambda net=net, layer=layer: [repr(s) for s in new.net_shapes(deff, net, layer)]),
            )

        ca, cb = tmp / "old_cache", tmp / "new_cache"
        check("cache result", outcome(lambda: old.write_def_cache(deff, str(ca))), outcome(lambda: new.write_def_cache(deff, str(cb))))
        if ca.is_dir() and cb.is_dir():
            listing = filecmp.dircmp(ca, cb)
            check("cache file list", sorted(listing.left_list), sorted(listing.right_list))
            for name in listing.common_files:
                check(f"cache {name}", (ca / name).read_bytes(), (cb / name).read_bytes())

        root = cfg["clock"]
        roots = [root, "net:" + cfg["net"]]
        macros = [
            ("sg13g2_buf_1", ["A"], ["X"], False, []),
            ("sg13g2_dfrbpq_1", ["D", "CLK", "RESET_B"], ["Q"], True, ["CLK"]),
            ("sky130_fd_sc_hd__buf_1", ["A"], ["X"], False, []),
        ]
        for prune in (True, False):
            check(
                f"clock_tree_cells prune={prune}",
                outcome(lambda prune=prune: old.clock_tree_cells(deff, roots, macros, prune)),
                outcome(lambda prune=prune: new.clock_tree_cells(deff, roots, macros, prune)),
            )
        objects = ([cfg["net"], root.split(":", 1)[1], "nope"], [("-", root.split(":", 1)[1]), ("x", "y")])
        check(
            "clock_object_instances",
            outcome(lambda: old.clock_object_instances(deff, *objects, macros)),
            outcome(lambda: new.clock_object_instances(deff, *objects, macros)),
        )
        sdc_roots = [("clk", "port", root.split(":", 1)[1])]
        both_files("clock_instances", lambda m, out: m.write_clock_instances_csv(deff, sdc_roots, [], out))
        if ca.is_dir() and cb.is_dir():
            a, b = tmp / "old_cic.csv", tmp / "new_cic.csv"
            check(
                "clock_instances from cache",
                outcome(lambda: old.write_clock_instances_csv_from_cache(str(ca), sdc_roots, [], str(a))),
                outcome(lambda: new.write_clock_instances_csv_from_cache(str(cb), sdc_roots, [], str(b))),
            )
            check("clock_instances from cache bytes", a.read_bytes(), b.read_bytes())

    print(f"differences: {failures}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
