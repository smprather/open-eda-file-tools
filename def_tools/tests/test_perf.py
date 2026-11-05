"""Performance and scaling tests.

Skipped unless ``--runperf`` is passed: these generate multi-hundred-MB
designs and take minutes.

    uv run pytest tests/test_perf.py --runperf
    uv run pytest tests/test_perf.py --runperf --perf-instances 2500000

Thresholds are deliberately loose.  They exist to catch a design being
loaded into memory or an order-of-magnitude slowdown, not to police a
20% drift between machines.
"""

from __future__ import annotations

import gzip
import resource
import subprocess
import sys
import time
from pathlib import Path

import pytest

import def_tools as lt
from tests.gen.design_gen import DesignSpec, generate

pytestmark = pytest.mark.perf

#: Floor on parse throughput.  Measured ~2.1M instances/s on a laptop;
#: this catches a 7x regression without flaking on slow CI.
MIN_INSTANCES_PER_SEC = 300_000


@pytest.fixture(scope="module")
def large_design(request, tmp_path_factory):
    n = request.config.getoption("--perf-instances")
    out = tmp_path_factory.mktemp("perf")
    manifest = generate(DesignSpec(instances=n, seed=1, gzip=True), out)
    return out, manifest


def _uncompressed_size(path: Path) -> int:
    total = 0
    with gzip.open(path, "rb") as fh:
        while chunk := fh.read(1 << 22):
            total += len(chunk)
    return total


def test_generated_design_is_representatively_large(large_design):
    """A benchmark on an unrepresentative file measures nothing."""
    out, manifest = large_design
    size = _uncompressed_size(out / manifest["def"])
    per_instance = size / manifest["counts"]["components"]

    # Reference design: 223 bytes of DEF per instance.
    assert per_instance == pytest.approx(223, rel=0.25)
    assert size > 50_000_000


def test_instance_parse_throughput(large_design):
    out, manifest = large_design

    start = time.perf_counter()
    count = sum(1 for _ in lt.instance_bboxes([out / manifest["lef"]], out / manifest["def"]))
    elapsed = time.perf_counter() - start

    assert count == manifest["counts"]["components"], "correctness first"
    rate = count / elapsed
    print(f"\ninstance_bboxes: {count} in {elapsed:.2f}s = {rate / 1000:.0f}k inst/s")
    assert rate > MIN_INSTANCES_PER_SEC


def test_parse_memory_does_not_scale_with_design_size(tmp_path):
    """The parser must stream, not buffer the design.

    Compared across a 4x size step rather than against an absolute
    ceiling, so the result does not depend on the machine or on
    interpreter baseline memory.  A parser that reads the file into
    memory shows ~4x growth; a streaming one stays roughly flat.
    """
    small = generate(DesignSpec(instances=200_000, seed=2, gzip=True), tmp_path / "small")
    large = generate(DesignSpec(instances=800_000, seed=2, gzip=True), tmp_path / "large")

    small_rss = _peak_rss_parsing(tmp_path / "small", small)
    large_rss = _peak_rss_parsing(tmp_path / "large", large)

    print(f"\npeak RSS: 200k inst {small_rss / 1e6:.0f} MB, 800k inst {large_rss / 1e6:.0f} MB")
    assert large_rss < small_rss * 2, "memory appears to scale with design size"


def _peak_rss_parsing(out: Path, manifest: dict) -> int:
    """Peak RSS of a subprocess that parses the design.

    Run out-of-process so the measurement covers the Rust extension's
    allocations and is not polluted by the test session's own heap.
    """
    script = (
        "import def_tools as lt, sys;"
        "sys.exit(0 if sum(1 for _ in lt.instance_bboxes([sys.argv[1]], sys.argv[2])) else 1)"
    )
    before = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    subprocess.run(
        [sys.executable, "-c", script, str(out / manifest["lef"]), str(out / manifest["def"])],
        check=True,
    )
    after = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    return max(after, before) * 1024  # ru_maxrss is KB on Linux


def test_def_cache_round_trip_at_scale(large_design, tmp_path):
    out, manifest = large_design
    cache = tmp_path / "cache"

    start = time.perf_counter()
    lt.write_def_cache(out / manifest["def"], cache)
    build = time.perf_counter() - start

    assert cache.is_dir()
    start = time.perf_counter()
    csv = tmp_path / "shapes.csv"
    lt.write_net_shapes_csv_from_cache(cache, "VPWR", "met1", csv)
    query = time.perf_counter() - start

    rows = csv.read_text(encoding="utf-8").splitlines()
    print(f"\ncache build {build:.2f}s, query {query:.3f}s, {len(rows) - 1} shapes")

    # One FOLLOWPIN rail per row.
    assert len(rows) - 1 == manifest["layout"]["rows"]
    # A cache exists so repeated queries beat re-parsing; if a query costs
    # as much as the build, the index is not being used.
    assert query < build


def test_streaming_query_matches_cached_query(large_design, tmp_path):
    """Correctness must survive the scale, not just the small fixtures."""
    out, manifest = large_design
    cache = tmp_path / "cache2"
    lt.write_def_cache(out / manifest["def"], cache)

    direct = tmp_path / "direct.csv"
    cached = tmp_path / "cached.csv"
    lt.write_net_shapes_csv(out / manifest["def"], "VGND", "met4", direct)
    lt.write_net_shapes_csv_from_cache(cache, "VGND", "met4", cached)

    assert direct.read_text(encoding="utf-8") == cached.read_text(encoding="utf-8")


def test_generation_is_fast_enough_to_be_worth_regenerating(tmp_path):
    """The generator replaces stored collateral only if regenerating is cheap."""
    start = time.perf_counter()
    manifest = generate(DesignSpec(instances=250_000, seed=3, gzip=True), tmp_path)
    elapsed = time.perf_counter() - start

    rate = manifest["counts"]["components"] / elapsed
    print(f"\ngeneration: {rate / 1000:.0f}k inst/s")
    assert rate > 10_000, "generating a design should not dominate the test run"
