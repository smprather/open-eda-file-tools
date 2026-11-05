"""Shared fixtures and opt-in markers.

Performance tests generate multi-million-instance designs and are far too
slow for the default suite, so they are skipped unless ``--runperf`` is
passed.  Everything else runs by default.
"""

from __future__ import annotations

import pytest

from tests.gen.design_gen import DesignSpec, generate


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        "--runperf",
        action="store_true",
        default=False,
        help="run performance tests (generates large designs; minutes, GBs of disk)",
    )
    parser.addoption(
        "--perf-instances",
        type=int,
        default=1_000_000,
        help="instance count for performance tests (default: 1000000)",
    )


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line("markers", "perf: performance test, needs --runperf")


def pytest_collection_modifyitems(config: pytest.Config, items: list[pytest.Item]) -> None:
    if config.getoption("--runperf"):
        return
    skip = pytest.mark.skip(reason="needs --runperf")
    for item in items:
        if "perf" in item.keywords:
            item.add_marker(skip)


@pytest.fixture(scope="session")
def small_design(tmp_path_factory: pytest.TempPathFactory):
    """A generated design big enough to contain every DEF construct.

    Session-scoped: generating it is cheap but not free, and every test
    that uses it only reads.
    """
    out = tmp_path_factory.mktemp("small_design")
    spec = DesignSpec(instances=4000, seed=7, gzip=False)
    manifest = generate(spec, out)
    return out, manifest


@pytest.fixture(scope="session")
def gzipped_design(tmp_path_factory: pytest.TempPathFactory):
    """The same design written gzipped, to cover the compressed read path."""
    out = tmp_path_factory.mktemp("gzipped_design")
    spec = DesignSpec(instances=4000, seed=7, gzip=True)
    manifest = generate(spec, out)
    return out, manifest
