"""OTW (Open Timing Windows): this project's open timing-window format.

OTW is a line-oriented text format (``.otw`` / ``.otw.gz``) carrying, per net
or per pin, a clock/data role and optional timing windows. It is specified in
``docs/otw.md`` and is not a dialect of any vendor's TWF.
"""

from __future__ import annotations

import gzip
import math
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from liberty_tools._native import read_otw as _read_otw
from liberty_tools._native import write_otw_windows_csv

OTW_VERSION = "1"
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


@dataclass(frozen=True)
class OtwClock:
    """A clock; windows and ``clocks=`` name it by position in ``OtwFile.clocks``.

    ``attributes`` are free ``key=value`` pairs (``source``, ``master``, ...).
    """

    name: str
    period: float
    rise_edge: float
    fall_edge: float
    attributes: tuple[tuple[str, str], ...] = ()


@dataclass(frozen=True)
class OtwWindow:
    """One timing window of an object, caused by one edge of one clock.

    ``clock`` is ``None`` (with ``edge`` ``"-"``) for an arrival no clock
    launches. ``values`` follows ``WINDOW_FIELDS``; ``None`` means that
    transition does not occur. Times are in the file's ``time_unit``.
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
class OtwFile:
    """Per-object content of an OTW file (windows are counted, not loaded).

    ``domains`` maps a net name or pin key to the indices of the clocks that
    reach it: its ``clocks=`` attribute, else the clocks of its windows.
    ``object_attributes`` holds the other ``key=value`` pairs of an object
    (``freq`` in hertz, and anything a writer added). Objects with neither are
    absent from those two maps.
    """

    design: str
    time_unit: float | None
    clocks: tuple[OtwClock, ...]
    attributes: tuple[tuple[str, str], ...]
    nets: dict[str, str]
    pins: dict[PinKey, str]
    windows: int
    cap_unit: float | None
    res_unit: float | None
    slew_thresholds: tuple[float, float] | None
    domains: dict[ObjectKey, tuple[int, ...]]
    object_attributes: dict[ObjectKey, dict[str, str]]

    def domain_names(self, key: ObjectKey) -> tuple[str, ...]:
        """Names of the clocks reaching ``key`` (empty when unknown)."""
        return tuple(self.clocks[index].name for index in self.domains.get(key, ()))


def read_otw(path: Path | str) -> OtwFile:
    """Read an OTW file into its header and per-object roles.

    Raises ``ValueError`` (naming file and line) for a malformed file and an
    ``OSError`` subclass for an unreadable one.
    """
    summary = _read_otw(str(path))
    domains: dict[ObjectKey, tuple[int, ...]] = {}
    object_attributes: dict[ObjectKey, dict[str, str]] = {}
    nets: dict[str, str] = {}
    pins: dict[PinKey, str] = {}

    def note(key: ObjectKey, clocks: list[int], attrs: list[tuple[str, str]]) -> None:
        if clocks:
            domains[key] = tuple(clocks)
        if attrs:
            object_attributes[key] = dict(attrs)

    for name, role, clocks, attrs in summary.nets:
        nets[name] = role
        note(name, clocks, attrs)
    for instance, pin, role, clocks, attrs in summary.pins:
        pins[(instance, pin)] = role
        note((instance, pin), clocks, attrs)
    return OtwFile(
        design=summary.design,
        time_unit=summary.time_unit,
        clocks=tuple(
            OtwClock(name, period, rise, fall, tuple(attrs))
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
    )


def _token(value: str, what: str) -> str:
    if not value or any(ch.isspace() for ch in value) or value.startswith("#"):
        raise ValueError(f"{what} {value!r} is not a valid OTW token")
    return value


def _role(value: str, what: str) -> str:
    if value not in ROLES:
        raise ValueError(f"{what}: role must be one of {', '.join(ROLES)}; got {value!r}")
    return value


def _decimal(value: float, precision: int) -> str:
    """``value`` as an OTW decimal: fixed point, no exponent, no padding."""
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


def write_otw(
    path: Path | str,
    *,
    design: str = "",
    time_unit: float | None = None,
    cap_unit: float | None = None,
    res_unit: float | None = None,
    slew_thresholds: tuple[float, float] | None = None,
    clocks: Sequence[OtwClock] = (),
    attributes: Sequence[tuple[str, str]] = (),
    nets: Mapping[str, str] | None = None,
    pins: Mapping[PinKey, str] | None = None,
    windows: Mapping[ObjectKey, Sequence[OtwWindow]] | None = None,
    domains: Mapping[ObjectKey, Sequence[int]] | None = None,
    object_attributes: Mapping[ObjectKey, Mapping[str, str]] | None = None,
    precision: int = 6,
) -> None:
    """Write an OTW file; a ``.gz`` suffix gzips it.

    ``nets`` maps net name to role, ``pins`` maps ``(instance, pin)`` to role
    (instance ``"-"`` is a top-level port). ``windows`` optionally maps a net
    name or pin key to its windows, ``domains`` to the indices of the clocks
    reaching it (written as ``clocks=``), and ``object_attributes`` to further
    ``key=value`` pairs such as ``freq`` (hertz). An object with a clock window
    must have role ``c``; anything the reader would reject raises
    ``ValueError`` here.

    Window values and ranges are written as fixed-point decimals with at most
    ``precision`` fractional digits (OTW body numbers carry no exponent), so
    pick ``time_unit`` to suit: with ``1e-9`` the default keeps femtoseconds.

    This writer is a convenience for tests and converters of modest size; it
    builds the file in memory. Each net and each instance is an independent
    group, so a high-volume writer can format groups on many threads and
    append them in any order.
    """
    nets = nets or {}
    pins = pins or {}
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

    lines = [f"otw {OTW_VERSION}"]
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
        "otw", "design", "time_unit", "cap_unit", "res_unit", "slew_thresholds",
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
        extra = object_attributes.get(key, {})
        if "freq" in extra and not float(extra["freq"]) >= 0:
            raise ValueError(f"{key!r}: freq must be a non-negative number of hertz")
        return suffix + _attributes(extra.items(), ("clocks",), repr(key))

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
    "OTW_VERSION",
    "ROLES",
    "WINDOW_FIELDS",
    "OtwClock",
    "OtwFile",
    "OtwWindow",
    "read_otw",
    "write_otw",
    "write_otw_windows_csv",
]
