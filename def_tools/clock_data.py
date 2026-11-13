"""Clock/data classification sources for ``def-tools instances``.

A *backend* reads one source file and answers, per net, pin or instance, which
role it plays: ``c`` clock network, ``d`` data, ``u`` unconstrained, ``z``
never switches. This module then maps the clock objects onto DEF instances.
The seam is the answer, not a file format, so a proprietary reader can be
plugged in without this project knowing its format.

Built-in backends:

``otw``    OTW, this project's open timing-window format (``docs/otw.md``).
``names``  A plain list of clock-tree instance names (``--clock-cells``).

Other backends (a vendor ``twf`` reader, for example) live outside this
project and plug in by name, through any of:

* the ``open_eda_file_tools.clock_data`` entry-point group of an installed
  package (``name = "package.module:reader"``);
* the ``OPEN_EDA_CLOCK_DATA_BACKENDS`` environment variable, a comma-separated
  list of ``name=package.module:reader``, for builds without package metadata;
* ``register_backend(name, reader)`` from Python.

A backend that is not installed is an error, never an empty answer.
"""

from __future__ import annotations

import importlib
import json
import os
import re
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from importlib.metadata import entry_points
from pathlib import Path

from .clock_roles import CellClockRole, native_macro_roles

ROLES = ("c", "d", "u", "z")
CLOCK_ROLE = "c"
ENTRY_POINT_GROUP = "open_eda_file_tools.clock_data"
BACKENDS_ENV = "OPEN_EDA_CLOCK_DATA_BACKENDS"

EXIT_BACKEND_MISSING = 3
EXIT_SOURCE_MALFORMED = 4

PinKey = tuple[str, str]


class ClockDataError(Exception):
    """A clock-data source could not be turned into an answer."""

    exit_code = 1


class ClockDataBackendError(ClockDataError):
    """No backend is available for the requested source."""

    exit_code = EXIT_BACKEND_MISSING


class ClockDataFormatError(ClockDataError):
    """The backend ran, but the source is malformed or carries no answer."""

    exit_code = EXIT_SOURCE_MALFORMED


@dataclass(frozen=True)
class ClockData:
    """What a backend returns: a role for every object the source describes.

    An object appears once, with one role. A backend whose source holds
    several records per object must apply the precedence rule itself: any
    clock record makes the object ``c``.

    ``pins`` is keyed by ``(instance, pin)``; instance ``"-"`` is a top-level
    port. Names are written as in the DEF (DEF backslash escapes optional).

    ``domains`` is optional: the names of the clocks reaching an object, keyed
    by net name, pin key or instance name. A backend that does not know leaves
    the object out; it is carried for callers and does not affect ``is_clock``.
    """

    backend: str
    source: Path
    nets: Mapping[str, str] = field(default_factory=dict)
    pins: Mapping[PinKey, str] = field(default_factory=dict)
    instances: Mapping[str, str] = field(default_factory=dict)
    domains: Mapping[str | PinKey, tuple[str, ...]] = field(default_factory=dict)

    def clock_nets(self) -> list[str]:
        return [name for name, role in self.nets.items() if role == CLOCK_ROLE]

    def clock_pins(self) -> list[PinKey]:
        return [key for key, role in self.pins.items() if role == CLOCK_ROLE]

    def clock_instances(self) -> list[str]:
        return [name for name, role in self.instances.items() if role == CLOCK_ROLE]


@dataclass(frozen=True)
class ClockInstances:
    """Clock-tree instances, plus everything that could not be decided.

    ``unmatched_*`` name clock objects the DEF does not contain.
    ``unclassified`` instances sit on a clock object but their macro has no
    Liberty cell, so whether they distribute the clock is unknown; they are
    reported here and are *not* in ``cells``.
    """

    cells: tuple[str, ...]
    unmatched_nets: tuple[str, ...] = ()
    unmatched_pins: tuple[PinKey, ...] = ()
    unclassified: tuple[str, ...] = ()

    def warnings(self) -> list[str]:
        notes: list[str] = []
        if self.unmatched_nets:
            notes.append(
                f"{len(self.unmatched_nets)} clock net(s) are not in the DEF "
                f"(first: {self.unmatched_nets[0]})"
            )
        if self.unmatched_pins:
            instance, pin = self.unmatched_pins[0]
            notes.append(
                f"{len(self.unmatched_pins)} clock pin(s) are not in the DEF "
                f"(first: {instance} {pin})"
            )
        if self.unclassified:
            notes.append(
                f"{len(self.unclassified)} instance(s) on clock objects have no Liberty "
                f"cell and are left unmarked (first: {self.unclassified[0]})"
            )
        return notes


Backend = Callable[[Path], ClockData]


def read_clock_cells(path: Path) -> set[str]:
    """Read clock-tree instance names: a JSON list or one name per line."""
    text = path.read_text(encoding="utf-8")
    stripped = text.lstrip()
    if stripped.startswith("["):
        return {str(value) for value in json.loads(stripped)}
    return {
        line.strip()
        for line in text.splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    }


def _names_backend(path: Path) -> ClockData:
    names = read_clock_cells(path)
    return ClockData("names", path, instances=dict.fromkeys(sorted(names), CLOCK_ROLE))


def _otw_backend(path: Path) -> ClockData:
    from .otw import read_otw

    parsed = read_otw(path)
    domains = {key: parsed.domain_names(key) for key in parsed.domains}
    return ClockData("otw", path, nets=parsed.nets, pins=parsed.pins, domains=domains)


_BUILTIN: dict[str, Backend] = {"names": _names_backend, "otw": _otw_backend}
_REGISTERED: dict[str, Backend] = {}
_SUFFIX_BACKENDS = (
    (".otw", "otw"),
    (".otw.gz", "otw"),
    (".twf", "twf"),
    (".twf.gz", "twf"),
)
_BACKEND_NAME = re.compile(r"[A-Za-z][A-Za-z0-9_-]+")


def register_backend(name: str, reader: Backend) -> None:
    """Register ``reader`` as backend ``name`` for this process.

    Built-in names cannot be replaced: two readers answering to one name
    would silently disagree.
    """
    if not _BACKEND_NAME.fullmatch(name):
        raise ValueError(f"invalid backend name: {name!r}")
    if name in _BUILTIN:
        raise ValueError(f"backend {name!r} is built in and cannot be replaced")
    _REGISTERED[name] = reader


def _entry_point_backends() -> dict[str, Backend]:
    found: dict[str, Backend] = {}
    for entry in entry_points(group=ENTRY_POINT_GROUP):
        if entry.name not in _BUILTIN:
            found[entry.name] = entry.load()
    return found


def _environment_backends() -> dict[str, Backend]:
    found: dict[str, Backend] = {}
    for item in os.environ.get(BACKENDS_ENV, "").split(","):
        item = item.strip()
        if not item:
            continue
        name, _, target = item.partition("=")
        module_name, _, attribute = target.partition(":")
        if not (_BACKEND_NAME.fullmatch(name) and module_name and attribute):
            raise ClockDataBackendError(
                f"{BACKENDS_ENV}: expected name=package.module:reader, got {item!r}"
            )
        if name in _BUILTIN:
            continue
        try:
            found[name] = getattr(importlib.import_module(module_name), attribute)
        except (ImportError, AttributeError) as exc:
            raise ClockDataBackendError(
                f"{BACKENDS_ENV}: cannot load backend `{name}` from {target}: {exc}"
            ) from exc
    return found


def backends() -> dict[str, Backend]:
    """Every available backend.

    Later sources win: entry points, the environment variable,
    ``register_backend``, then the built-ins (which nothing can replace).
    """
    return {
        **_entry_point_backends(),
        **_environment_backends(),
        **_REGISTERED,
        **_BUILTIN,
    }


def parse_source(spec: str | Path) -> tuple[str, Path]:
    """Split ``[BACKEND:]PATH``; without a prefix the suffix picks the backend."""
    text = str(spec)
    prefix, separator, rest = text.partition(":")
    if separator and rest and _BACKEND_NAME.fullmatch(prefix) and not Path(text).exists():
        return prefix, Path(rest)
    lowered = text.lower()
    for suffix, backend in _SUFFIX_BACKENDS:
        if lowered.endswith(suffix):
            return backend, Path(text)
    raise ClockDataBackendError(
        f"--clock-data {text}: cannot tell the backend from the file name; "
        f"write it as BACKEND:PATH (available: {', '.join(sorted(backends()))})"
    )


def read_clock_data(spec: str | Path) -> ClockData:
    """Read a ``[BACKEND:]PATH`` source through its backend.

    Raises ``ClockDataBackendError`` when the backend is not installed and
    ``ClockDataFormatError`` when the source is malformed or describes no
    object; an unreadable file raises ``OSError``.
    """
    name, path = parse_source(spec)
    available = backends()
    reader = available.get(name)
    if reader is None:
        hint = (
            " Vendor TWF dialects are proprietary and are not read by this project: "
            "convert the file to OTW (docs/otw.md) or install a package that provides "
            f"a `twf` backend (`{ENTRY_POINT_GROUP}` entry point or {BACKENDS_ENV})."
            if name == "twf"
            else ""
        )
        raise ClockDataBackendError(
            f"--clock-data {name}:{path}: no `{name}` clock-data backend is installed "
            f"(available: {', '.join(sorted(available))}).{hint}"
        )
    try:
        data = reader(path)
    except ClockDataError:
        raise
    except ValueError as exc:
        raise ClockDataFormatError(str(exc)) from exc
    for kind, objects in (("net", data.nets), ("pin", data.pins), ("instance", data.instances)):
        for key, role in objects.items():
            if role not in ROLES:
                raise ClockDataFormatError(
                    f"{path}: backend `{name}` gave {kind} {key!r} the role {role!r}; "
                    f"roles are {', '.join(ROLES)}"
                )
    if not (data.nets or data.pins or data.instances) and name != "names":
        raise ClockDataFormatError(
            f"{path}: the source describes no net, pin or instance, so nothing can be "
            "classified"
        )
    return data


def resolve_clock_instances(
    data: ClockData,
    def_path: Path | str,
    roles: Mapping[str, CellClockRole] | None,
) -> ClockInstances:
    """Map the clock objects in ``data`` onto DEF instances.

    * A clock **instance** is taken as given.
    * A clock **net** marks the instances that drive it, flip-flops, latches
      and memories excepted (a divider flop driving a generated clock is a
      clock *source*, not a clock-tree cell).
    * A clock **pin** marks its instance when the instance is not such an
      endpoint and the pin is an output or a clock-through input (for an
      integrated clock gate: its clock pin, never its enable).

    An instance with several nets or pins is a clock cell when any one of
    them marks it. Nets and pins need ``roles`` (Liberty pin directions).
    """
    from liberty_tools._native import clock_object_instances

    cells = set(data.clock_instances())
    nets = data.clock_nets()
    pins = data.clock_pins()
    if not nets and not pins:
        return ClockInstances(cells=tuple(sorted(cells)))
    if not roles:
        raise ClockDataError(
            f"{data.source}: mapping clock nets/pins to instances needs Liberty pin "
            "directions; supply --lib"
        )
    found, unmatched_nets, unmatched_pins, unclassified = clock_object_instances(
        str(def_path), nets, pins, native_macro_roles(dict(roles))
    )
    if len(unmatched_nets) == len(nets) and len(unmatched_pins) == len(pins):
        raise ClockDataError(
            f"{data.source}: none of its {len(nets) + len(pins)} clock nets/pins exist in "
            f"{def_path}; the source does not describe this design"
        )
    cells.update(found)
    return ClockInstances(
        cells=tuple(sorted(cells)),
        unmatched_nets=tuple(unmatched_nets),
        unmatched_pins=tuple(unmatched_pins),
        unclassified=tuple(unclassified),
    )


def clock_instances(
    spec: str | Path,
    def_path: Path | str,
    roles: Mapping[str, CellClockRole] | None = None,
) -> ClockInstances:
    """Read a ``[BACKEND:]PATH`` source and resolve it to clock-tree instances."""
    return resolve_clock_instances(read_clock_data(spec), def_path, roles)


__all__ = [
    "BACKENDS_ENV",
    "CLOCK_ROLE",
    "ENTRY_POINT_GROUP",
    "EXIT_BACKEND_MISSING",
    "EXIT_SOURCE_MALFORMED",
    "ROLES",
    "Backend",
    "ClockData",
    "ClockDataBackendError",
    "ClockDataError",
    "ClockDataFormatError",
    "ClockInstances",
    "backends",
    "clock_instances",
    "parse_source",
    "read_clock_cells",
    "read_clock_data",
    "register_backend",
    "resolve_clock_instances",
]
