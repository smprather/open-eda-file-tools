"""OTD (Open Timing Data): this project's open timing-data format.

OTD is a line-oriented text format (``.otd`` / ``.otd.gz``) carrying, per net
or per pin, a clock/data role and optional timing windows. It is specified in
``docs/otd.md`` and is not a dialect of any vendor's TWF.
"""

from __future__ import annotations

import gzip
import math
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path

from liberty_tools._native import read_otd as _read_otd
from liberty_tools._native import write_otd_windows_csv

OTD_VERSION = "1"
ROLES = ("c", "d", "u", "z")
WINDOW_FIELDS = (
    "rise_early",
    "rise_late",
    "fall_early",
    "fall_late",
    "rise_slew_min",
    "rise_slew_max",
    "fall_slew_min",
    "fall_slew_max",
)
PinKey = tuple[str, str]
ObjectKey = str | PinKey
_DRIVER_ATTRIBUTES = ("driver_instance", "driver_pin")


@dataclass(frozen=True)
class OtdClock:
    """A clock; windows and ``clocks=`` name it by position in ``OtdFile.clocks``.

    ``attributes`` are free ``key=value`` pairs (``source``, ``master``, ...).
    """

    name: str
    period: float
    rise_edge: float
    fall_edge: float
    attributes: tuple[tuple[str, str], ...] = ()


@dataclass(frozen=True)
class OtdWindow:
    """One timing window of an object, caused by one edge of one clock.

    ``clock`` is ``None`` (with ``edge`` ``"-"``) for an arrival no clock
    launches. ``values`` follows ``WINDOW_FIELDS``. Use ``None`` for any
    unspecified arrival bound or slew. Times are in the file's ``time_unit``.
    The four optional ranges are ``(min, max)``: the resistance of the source
    driving each transition (in ``res_unit``) and the slack of each transition
    (in ``time_unit``). ``attributes`` carries any further ``key=value`` pairs
    verbatim.
    """

    clock: int | None
    edge: str
    role: str
    values: tuple[float | None, ...]
    rise_res: tuple[float, float] | None = None
    rise_slack: tuple[float, float] | None = None
    fall_res: tuple[float, float] | None = None
    fall_slack: tuple[float, float] | None = None
    attributes: tuple[tuple[str, str], ...] = ()


@dataclass(frozen=True)
class OtdFile:
    """Per-object content of an OTD file (windows are counted, not loaded).

    ``domains`` maps a net name or pin key to the indices of the clocks that
    reach it: its ``clocks=`` attribute, else the clocks of its windows.
    ``net_drivers`` maps a net name to its optional driving pin key.
    ``object_attributes`` holds the other ``key=value`` pairs of an object,
    such as ``load`` in ``cap_unit``.
    """

    design: str
    time_unit: float | None
    clocks: tuple[OtdClock, ...]
    attributes: tuple[tuple[str, str], ...]
    nets: dict[str, str]
    pins: dict[PinKey, str]
    windows: int
    cap_unit: float | None
    res_unit: float | None
    slew_thresholds: tuple[float, float] | None
    domains: dict[ObjectKey, tuple[int, ...]]
    object_attributes: dict[ObjectKey, dict[str, str]]
    net_drivers: dict[str, PinKey] = field(default_factory=dict)

    def domain_names(self, key: ObjectKey) -> tuple[str, ...]:
        """Names of the clocks reaching ``key`` (empty when unknown)."""
        return tuple(self.clocks[index].name for index in self.domains.get(key, ()))


def read_otd(path: Path | str) -> OtdFile:
    """Read an OTD file into its header and per-object roles.

    Raises ``ValueError`` (naming file and line) for a malformed file and an
    ``OSError`` subclass for an unreadable one.
    """
    summary = _read_otd(str(path))
    domains: dict[ObjectKey, tuple[int, ...]] = {}
    object_attributes: dict[ObjectKey, dict[str, str]] = {}
    nets: dict[str, str] = {}
    pins: dict[PinKey, str] = {}
    net_drivers: dict[str, PinKey] = {}

    def note(key: ObjectKey, clocks: list[int], attrs: list[tuple[str, str]]) -> None:
        if clocks:
            domains[key] = tuple(clocks)
        if attrs:
            extra = dict(attrs)
            driver_instance, driver_pin = _DRIVER_ATTRIBUTES
            if isinstance(key, str) and driver_instance in extra:
                net_drivers[key] = (extra.pop(driver_instance), extra.pop(driver_pin))
            if extra:
                object_attributes[key] = extra

    for name, role, clocks, attrs in summary.nets:
        nets[name] = role
        note(name, clocks, attrs)
    for instance, pin, role, clocks, attrs in summary.pins:
        pins[(instance, pin)] = role
        note((instance, pin), clocks, attrs)
    return OtdFile(
        design=summary.design,
        time_unit=summary.time_unit,
        clocks=tuple(
            OtdClock(name, period, rise, fall, tuple(attrs))
            for name, period, rise, fall, attrs in summary.clocks
        ),
        attributes=tuple(summary.attributes),
        nets=nets,
        pins=pins,
        windows=summary.windows,
        cap_unit=summary.cap_unit,
        res_unit=summary.res_unit,
        slew_thresholds=summary.slew_thresholds,
        domains=domains,
        object_attributes=object_attributes,
        net_drivers=net_drivers,
    )


def _token(value: str, what: str) -> str:
    if not value or any(ch.isspace() for ch in value) or value.startswith("#"):
        raise ValueError(f"{what} {value!r} is not a valid OTD token")
    return value


def _role(value: str, what: str) -> str:
    if value not in ROLES:
        raise ValueError(f"{what}: role must be one of {', '.join(ROLES)}; got {value!r}")
    return value


def _decimal(value: float, precision: int) -> str:
    """``value`` as an OTD decimal: fixed point, no exponent, no padding."""
    if not math.isfinite(value):
        raise ValueError(f"value {value!r} is not finite")
    text = f"{value:.{precision}f}"
    if "." in text:
        text = text.rstrip("0").rstrip(".")
    return "0" if text in ("-0", "") else text


def _attributes(pairs, reserved: tuple[str, ...], what: str) -> str:
    text = ""
    for key, value in pairs:
        if key in reserved or "=" in key:
            raise ValueError(f"{what}: attribute {key!r} is not allowed here")
        text += f" {_token(key, 'attribute')}={_token(str(value), 'attribute value')}"
    return text


def write_otd(
    path: Path | str,
    *,
    design: str = "",
    time_unit: float | None = None,
    cap_unit: float | None = None,
    res_unit: float | None = None,
    slew_thresholds: tuple[float, float] | None = None,
    clocks: Sequence[OtdClock] = (),
    attributes: Sequence[tuple[str, str]] = (),
    nets: Mapping[str, str] | None = None,
    pins: Mapping[PinKey, str] | None = None,
    net_drivers: Mapping[str, PinKey] | None = None,
    windows: Mapping[ObjectKey, Sequence[OtdWindow]] | None = None,
    domains: Mapping[ObjectKey, Sequence[int]] | None = None,
    object_attributes: Mapping[ObjectKey, Mapping[str, str]] | None = None,
    precision: int = 6,
) -> None:
    """Write an OTD file; a ``.gz`` suffix gzips it.

    ``nets`` maps net name to role, ``pins`` maps ``(instance, pin)`` to role
    (instance ``"-"`` is a top-level port). ``net_drivers`` optionally maps a
    declared net to its driving ``(instance, pin)`` pair. ``windows`` maps a
    net name or pin key to its windows, ``domains`` to the indices of the
    clocks reaching it (written as ``clocks=``), and ``object_attributes`` to
    further ``key=value`` pairs such as ``load``. An object with a clock
    window must have role ``c``; anything the reader would reject raises
    ``ValueError`` here.

    Window values and ranges are written as fixed-point decimals with at most
    ``precision`` fractional digits (OTD body numbers carry no exponent), so
    pick ``time_unit`` to suit: with ``1e-9`` the default keeps femtoseconds.

    This writer is a convenience for tests and converters of modest size; it
    builds the file in memory. Each net and each instance is an independent
    group, so a high-volume writer can format groups on many threads and
    append them in any order.
    """
    nets = nets or {}
    pins = pins or {}
    net_drivers = net_drivers or {}
    windows = windows or {}
    domains = domains or {}
    object_attributes = object_attributes or {}
    unknown = [
        key
        for key in (*windows, *domains, *object_attributes)
        if key not in nets and key not in pins
    ]
    if unknown:
        raise ValueError(f"windows or attributes given for undeclared object {unknown[0]!r}")
    for net in net_drivers:
        if net not in nets:
            raise ValueError(f"driver given for undeclared net {net!r}")

    lines = [f"otd {OTD_VERSION}"]
    if design:
        lines.append(f"design {_token(design, 'design')}")
    for key, unit in (("time_unit", time_unit), ("cap_unit", cap_unit), ("res_unit", res_unit)):
        if unit is not None:
            if not (math.isfinite(unit) and unit > 0):
                raise ValueError(f"{key} must be a positive number")
            lines.append(f"{key} {unit:g}")
    if slew_thresholds is not None:
        low, high = slew_thresholds
        if not 0 <= low < high <= 100:
            raise ValueError("slew_thresholds must satisfy 0 <= low < high <= 100")
        lines.append(f"slew_thresholds {low:g} {high:g}")
    reserved = {
        "otd", "design", "time_unit", "cap_unit", "res_unit", "slew_thresholds",
        "clock", "n", "i", "p", "w", "end",
    }
    for key, value in attributes:
        if key in reserved or "\n" in value:
            raise ValueError(f"invalid header attribute {key!r}")
        lines.append(f"{_token(key, 'attribute')} {value}".rstrip())
    for clock in clocks:
        lines.append(
            f"clock {_token(clock.name, 'clock')} {clock.period:g} "
            f"{clock.rise_edge:g} {clock.fall_edge:g}"
            + _attributes(clock.attributes, (), clock.name)
        )

    def object_suffix(key: ObjectKey) -> str:
        suffix = ""
        indices = domains.get(key)
        if indices:
            if any(not 0 <= index < len(clocks) for index in indices):
                raise ValueError(f"{key!r}: clock domain index is undeclared")
            suffix += " clocks=" + ",".join(str(index) for index in sorted(set(indices)))
        if isinstance(key, str) and key in net_drivers:
            instance, pin = net_drivers[key]
            for attr, name in zip(_DRIVER_ATTRIBUTES, (instance, pin)):
                if not name or any(not "!" <= char <= "~" for char in name):
                    raise ValueError(f"{key!r}: {attr} must be a visible ASCII name")
                suffix += f" {attr}={name}"
        extra = object_attributes.get(key, {})
        return suffix + _attributes(extra.items(), ("clocks", *_DRIVER_ATTRIBUTES), repr(key))

    def emit_windows(key: ObjectKey, role: str) -> None:
        for window in windows.get(key, ()):
            if window.clock is None:
                if window.edge != "-":
                    raise ValueError(f"{key!r}: a window with no clock takes edge '-'")
            elif not 0 <= window.clock < len(clocks):
                raise ValueError(f"{key!r}: window clock index {window.clock} is undeclared")
            elif window.edge not in ("r", "f"):
                raise ValueError(f"{key!r}: window edge must be 'r' or 'f'")
            if _role(window.role, repr(key)) == "c" and role != "c":
                raise ValueError(f"{key!r}: an object with a clock window must have role 'c'")
            if len(window.values) != len(WINDOW_FIELDS):
                raise ValueError(f"{key!r}: a window carries {len(WINDOW_FIELDS)} values")
            values = " ".join(
                "-" if value is None else _decimal(value, precision) for value in window.values
            )
            slack = [
                (name, f"{_decimal(value[0], precision)}:{_decimal(value[1], precision)}")
                for name, value in (
                    ("rise_res", window.rise_res),
                    ("rise_slack", window.rise_slack),
                    ("fall_res", window.fall_res),
                    ("fall_slack", window.fall_slack),
                )
                if value is not None
            ]
            clock = "-" if window.clock is None else window.clock
            lines.append(
                f"w {clock} {window.edge} {window.role} {values}"
                + _attributes(slack, (), repr(key))
                + _attributes(
                    window.attributes,
                    ("rise_res", "rise_slack", "fall_res", "fall_slack"),
                    repr(key),
                )
            )

    for name, role in nets.items():
        lines.append(f"n {_token(name, 'net')} {_role(role, name)}" + object_suffix(name))
        emit_windows(name, role)
    # All pins of an instance form one `i` group.
    by_instance: dict[str, list[tuple[str, str]]] = {}
    for (instance, pin), role in pins.items():
        by_instance.setdefault(instance, []).append((pin, role))
    for instance, members in by_instance.items():
        lines.append(f"i {_token(instance, 'instance')}")
        for pin, role in members:
            lines.append(
                f"p {_token(pin, 'pin')} {_role(role, f'{instance} {pin}')}"
                + object_suffix((instance, pin))
            )
            emit_windows((instance, pin), role)
    window_count = sum(len(windows.get(key, ())) for key in (*nets, *pins))
    lines.append(f"end {len(nets)} {len(pins)} {window_count}")

    text = "\n".join(lines) + "\n"
    target = Path(path)
    if target.suffix == ".gz":
        with gzip.open(target, "wt", encoding="utf-8") as stream:
            stream.write(text)
    else:
        target.write_text(text, encoding="utf-8")


__all__ = [
    "OTD_VERSION",
    "ROLES",
    "WINDOW_FIELDS",
    "OtdClock",
    "OtdFile",
    "OtdWindow",
    "read_otd",
    "write_otd",
    "write_otd_windows_csv",
]
