"""Type stubs for the shared PyO3 extension module ``liberty_tools._native``.

Liberty members are exposed dynamically through ``__getattr__`` while the typed
wrapper classes in ``liberty_tools`` (and ``liberty_tools/__init__.pyi``)
define the public Liberty surface. The LEF and DEF native classes and functions
are declared explicitly below.
"""

from collections.abc import Iterator, Sequence
from os import PathLike
from typing import Any, final

_StrPath = str | PathLike[str]

@final
class LefRect:
    """One ``RECT`` from a pin port, in LEF microns."""

    @property
    def layer(self) -> str: ...
    @property
    def x0_microns(self) -> float: ...
    @property
    def y0_microns(self) -> float: ...
    @property
    def x1_microns(self) -> float: ...
    @property
    def y1_microns(self) -> float: ...

@final
class LefPort:
    """One ``PORT`` group of a pin."""

    def rects(self) -> list[LefRect]: ...
    @property
    def has_unsupported_geometry(self) -> bool: ...

@final
class LefPin:
    """One ``PIN`` of a macro."""

    @property
    def name(self) -> str: ...
    @property
    def direction(self) -> str | None: ...
    @property
    def use(self) -> str | None: ...
    def ports(self) -> list[LefPort]: ...

@final
class LefMacro:
    """One ``MACRO`` of a cell LEF library, in LEF microns."""

    @property
    def name(self) -> str: ...
    @property
    def width_microns(self) -> float: ...
    @property
    def height_microns(self) -> float: ...
    @property
    def origin_x_microns(self) -> float: ...
    @property
    def origin_y_microns(self) -> float: ...
    def pin_names(self) -> list[str]: ...
    def pins(self) -> list[LefPin]: ...
    def pin(self, name: str) -> LefPin: ...

@final
class LefLibrary:
    """Macros parsed from one or more cell LEF files."""

    def macro_names(self) -> list[str]: ...
    def macros(self) -> list[LefMacro]: ...
    def macro(self, name: str) -> LefMacro: ...
    def duplicate_macro_names(self) -> list[str]: ...

def parse_lef(
    paths: _StrPath | Sequence[_StrPath],
    *,
    macros: Sequence[str] | None = None,
) -> LefLibrary: ...

@final
class InstanceBBox:
    @property
    def instance(self) -> str: ...
    @property
    def macro_name(self) -> str: ...
    @property
    def status(self) -> str: ...
    @property
    def orient(self) -> str: ...
    @property
    def llx(self) -> int: ...
    @property
    def lly(self) -> int: ...
    @property
    def urx(self) -> int: ...
    @property
    def ury(self) -> int: ...
    def as_tuple(self) -> tuple[str, str, str, str, int, int, int, int]: ...

@final
class InstanceBBoxIter:
    def __iter__(self) -> Iterator[InstanceBBox]: ...
    def __next__(self) -> InstanceBBox: ...

@final
class NetShape:
    @property
    def net(self) -> str: ...
    @property
    def section(self) -> str: ...
    @property
    def route_status(self) -> str: ...
    @property
    def shape_type(self) -> str: ...
    @property
    def layer(self) -> str: ...
    @property
    def width(self) -> int: ...
    @property
    def x1(self) -> int: ...
    @property
    def y1(self) -> int: ...
    @property
    def x2(self) -> int: ...
    @property
    def y2(self) -> int: ...
    @property
    def llx(self) -> int: ...
    @property
    def lly(self) -> int: ...
    @property
    def urx(self) -> int: ...
    @property
    def ury(self) -> int: ...

@final
class NetShapeIter:
    def __iter__(self) -> Iterator[NetShape]: ...
    def __next__(self) -> NetShape: ...

def instance_bboxes(lefs: Sequence[_StrPath], def_path: _StrPath) -> InstanceBBoxIter: ...
def net_shapes(def_path: _StrPath, net: str, layer: str) -> NetShapeIter: ...
def write_instance_bbox_csv(
    lefs: Sequence[_StrPath],
    def_path: _StrPath,
    output: _StrPath | None,
    columns: Sequence[tuple[str, str]] | None = None,
    sequential: Sequence[str] | None = None,
    clock: Sequence[str] | None = None,
) -> int: ...
def write_instance_parquet(
    lefs: Sequence[_StrPath],
    def_path: _StrPath,
    output: _StrPath,
    batch_size: int,
    columns: Sequence[tuple[str, str]] | None = None,
    sequential: Sequence[str] | None = None,
    clock: Sequence[str] | None = None,
) -> tuple[int, int]: ...
def write_instance_table(
    lefs: Sequence[_StrPath],
    def_path: _StrPath,
    output: _StrPath | None,
    columns: Sequence[tuple[str, str]] | None = None,
    sequential: Sequence[str] | None = None,
    clock: Sequence[str] | None = None,
) -> int: ...
def write_net_shapes_csv(
    def_path: _StrPath, net: str, layer: str, output: _StrPath | None
) -> None: ...
def write_def_cache(def_path: _StrPath, cache_dir: _StrPath) -> None: ...
def write_net_shapes_csv_from_cache(
    cache_dir: _StrPath, net: str, layer: str, output: _StrPath | None
) -> None: ...
def write_clock_instances_csv(
    def_path: _StrPath,
    roots: Sequence[tuple[str, str, str]],
    clock_cell_patterns: Sequence[str],
    output: _StrPath | None,
) -> None: ...
def write_clock_instances_csv_from_cache(
    cache_dir: _StrPath,
    roots: Sequence[tuple[str, str, str]],
    clock_cell_patterns: Sequence[str],
    output: _StrPath | None,
) -> None: ...
_MacroRole = tuple[str, Sequence[str], Sequence[str], bool, Sequence[str]]
def clock_tree_cells(
    def_path: _StrPath,
    roots: Sequence[str],
    macros: Sequence[_MacroRole],
    prune: bool = True,
) -> list[str]: ...
def clock_object_instances(
    def_path: _StrPath,
    nets: Sequence[str],
    pins: Sequence[tuple[str, str]],
    macros: Sequence[_MacroRole],
) -> tuple[list[str], list[str], list[tuple[str, str]], list[str]]: ...
@final
class OtwSummary:
    design: str
    time_unit: float | None
    cap_unit: float | None
    res_unit: float | None
    slew_thresholds: tuple[float, float] | None
    clocks: list[tuple[str, float, float, float, list[tuple[str, str]]]]
    attributes: list[tuple[str, str]]
    nets: list[tuple[str, str, list[int], list[tuple[str, str]]]]
    pins: list[tuple[str, str, str, list[int], list[tuple[str, str]]]]
    windows: int
def read_otw(path: _StrPath) -> OtwSummary: ...
def write_otw_windows_csv(path: _StrPath, output: _StrPath | None = None) -> int: ...
@final
class PexHeader:
    standard: str
    design: str
    date: str
    vendor: str
    program: str
    version: str
    design_flow: str
    divider: str
    delimiter: str
    bus_delimiter: str
    t_unit: str
    c_unit: str
    r_unit: str
    l_unit: str
    ports: list[tuple[str, str]]

@final
class PexConn:
    name: str
    direction: str
    attrs: list[str]
    x: float | None
    y: float | None

@final
class PexCap:
    id: str
    node1: str
    node2: str | None
    value: float
    ext: str | None

@final
class PexRes:
    id: str
    node1: str
    node2: str
    value: float
    ext: str | None

@final
class PexNet:
    name: str
    total_cap: float
    ground_cap: float
    coupling_cap: float
    conns: list[PexConn]
    caps: list[PexCap]
    res: list[PexRes]
    ext: str | None

@final
class PexSummary:
    name: str
    total_cap: float
    ground_cap: float
    coupling_cap: float
    cap_count: int
    res_count: int
    res_total: float

@final
class PexFile:
    header: PexHeader
    nets: list[PexNet]

@final
class PexHistogram:
    min: float
    max: float
    bounds: list[float]
    counts: list[int]

def parse_pex(
    path: _StrPath, keep_entries: bool = True, mode: str = "auto"
) -> PexFile: ...
def pex_summaries(path: _StrPath, mode: str = "auto") -> list[PexSummary]: ...
def pex_histogram(
    path: _StrPath, bins: int = 20, mode: str = "auto"
) -> PexHistogram: ...
def pex_header(path: _StrPath) -> PexHeader: ...
def __getattr__(name: str) -> Any: ...
