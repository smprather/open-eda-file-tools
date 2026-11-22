"""Cross-library comparison: match timing arcs, align their grids, diff the values.

Comparing a generated Liberty against a reference one is a normal part of a PDK
flow. Doing it by hand means writing, for every pair of cells, an arc matcher,
a grid intersector, an axis-role normalizer and a differ — and the last two
fail silently: a table whose axes are swapped produces plausible but wrong
deltas.

:func:`compare` does all four, using the same axis-role information the
extraction API exposes, so a mismatch is reported rather than guessed at.
"""

from __future__ import annotations

import math
from collections import defaultdict, deque
from dataclasses import dataclass, field
from itertools import pairwise, product
from typing import TYPE_CHECKING, Any, Iterable, Iterator, Sequence

if TYPE_CHECKING:
    from . import BooleanExpression, LibertyDocument, TimingArc, TimingTable

__all__ = [
    "ArcKey",
    "AxisRoleMismatch",
    "CompareEntry",
    "Comparison",
    "MalformedTable",
    "UnitMismatch",
    "compare",
]

#: Relative tolerance used when matching axis values of two tables.
DEFAULT_TOLERANCE = 1e-9

#: The unit dimensions a table can depend on.
_UNIT_DIMENSIONS = ("time", "capacitive_load", "voltage", "current")

#: Tables whose *values* are times, whatever their axes are named.
_TIME_TABLE_NAMES = frozenset(
    {
        "cell_rise",
        "cell_fall",
        "rise_transition",
        "fall_transition",
        "rise_constraint",
        "fall_constraint",
    }
)

#: Tables whose *values* are currents, whatever their axes are named.
_CURRENT_TABLE_NAMES = frozenset(
    {
        "output_current_rise",
        "output_current_fall",
    }
)

#: The prefixes of the CCS tables whose *values* are currents.
_CURRENT_TABLE_PREFIXES = ("ccst_", "ccsp_")

#: The timing type an arc has when the library does not declare one:
#: Liberty's default.
_DEFAULT_TIMING_TYPE = "combinational"

#: The timing sense an arc has when the library does not declare one:
#: Liberty's default.
_DEFAULT_TIMING_SENSE = "non_unate"


@dataclass(frozen=True)
class ArcKey:
    """Identifies a timing arc inside a document.

    The identity is complete: two arcs with the same key are the same arc.
    It covers the cell, the pin, the related pin, the timing type, the
    timing sense and the condition.

    A library that leaves the ``timing_type`` attribute out describes the
    same arc as one that writes ``combinational`` — Liberty's default — so
    the timing type is normalized here. The same holds for the timing
    sense: an arc without one is ``non_unate``.
    """

    cell: str
    pin: str
    related_pin: str | None
    timing_type: str | None
    when: str | None
    #: The timing sense of the arc (``"positive_unate"`` and the like);
    #: ``non_unate`` when the library declares none.
    timing_sense: str | None = None

    def __post_init__(self) -> None:
        if not self.timing_type:
            object.__setattr__(self, "timing_type", _DEFAULT_TIMING_TYPE)
        if not self.timing_sense:
            object.__setattr__(self, "timing_sense", _DEFAULT_TIMING_SENSE)

    def __str__(self) -> str:
        parts = [f"{self.cell}/{self.pin}"]
        if self.related_pin:
            parts.append(f"from {self.related_pin}")
        if self.timing_type:
            parts.append(f"({self.timing_type})")
        if self.timing_sense:
            parts.append(f"sense {self.timing_sense}")
        if self.when:
            parts.append(f"when {self.when}")
        return " ".join(parts)


@dataclass
class CompareEntry:
    """One compared grid point of one table of one arc."""

    cell: str
    pin: str
    related_pin: str | None
    timing_type: str | None
    when: str | None
    table: str
    index_1: float | None
    index_2: float | None
    index_3: float | None
    #: The physical variable of each axis (``None`` when the table has no
    #: template).
    axis_1: str | None
    axis_2: str | None
    axis_3: str | None
    value_a: float | None
    value_b: float | None
    #: The timing sense of the arc this point belongs to.
    timing_sense: str | None = field(default=None, kw_only=True)

    @property
    def arc(self) -> str:
        """A readable label of the arc this point belongs to."""
        key = ArcKey(
            self.cell,
            self.pin,
            self.related_pin,
            self.timing_type,
            self.when,
            self.timing_sense,
        )
        return str(key)

    @property
    def arc_key(self) -> ArcKey:
        """The structured identity of the arc this point belongs to."""
        return ArcKey(
            self.cell,
            self.pin,
            self.related_pin,
            self.timing_type,
            self.when,
            self.timing_sense,
        )

    @property
    def delta(self) -> float | None:
        """``value_a - value_b``, or ``None`` when only one side has the point."""
        if self.value_a is None or self.value_b is None:
            return None
        return self.value_a - self.value_b

    @property
    def relative_delta(self) -> float | None:
        """``delta`` relative to ``value_b`` (the reference), or ``None``."""
        delta = self.delta
        if delta is None or not self.value_b:
            return None
        return delta / self.value_b

    @property
    def matched(self) -> bool:
        """True when both documents have a value at this point."""
        return self.value_a is not None and self.value_b is not None


@dataclass(frozen=True)
class UnitMismatch:
    """A table that was not compared because the two libraries use
    different units.

    Diffing the numbers of two tables that measure the same quantity in
    different units is meaningless: the numbers are the same but the
    quantities are not (or the other way round, the numbers differ by a
    factor of a thousand). Such a table is reported instead of diffed.

    ``unit_a`` and ``unit_b`` carry the *whole* declaration of the dimension,
    so that every difference is visible in the reported fields: the
    ``time_unit``, ``voltage_unit`` and ``current_unit`` strings as
    declared, and the multiplier and the unit of the capacitive load
    together (``"1 pf"``, ``"1000 pf"``).
    """

    #: The cell the table belongs to.
    cell: str
    #: The pin the table belongs to.
    pin: str
    #: The name of the table.
    table: str
    #: The dimension that differs: ``"time"``, ``"capacitive_load"``,
    #: ``"voltage"`` or ``"current"``.
    dimension: str
    #: The declaration the first document makes of the dimension (``None``
    #: when it declares none).
    unit_a: str | None
    #: The declaration the second document makes of the dimension (``None``
    #: when it declares none).
    unit_b: str | None


@dataclass(frozen=True)
class AxisRoleMismatch:
    """A table that was not compared because the two libraries name its axes
    differently.

    The axes of a table are named by the template of its library: the load
    axis of one library can be called ``total_output_net_capacitance`` and
    the one of the other ``output_capacitance``. When the two libraries name
    the axes of a table differently *and* no role of one side has a
    counterpart in the other, the grids cannot be aligned: matching the axes
    by position would compare a load with a transition, and the deltas would
    be plausible and wrong. Such a table is reported instead of diffed.

    ``roles_a`` and ``roles_b`` hold the axis names of each side, in axis
    order.
    """

    #: The cell the table belongs to.
    cell: str
    #: The pin the table belongs to.
    pin: str
    #: The name of the table.
    table: str
    #: The axis names of the first document, in axis order.
    roles_a: tuple[str | None, ...]
    #: The axis names of the second document, in axis order.
    roles_b: tuple[str | None, ...]


@dataclass(frozen=True)
class MalformedTable:
    """A table whose values do not fill its grid.

    A table's values fill its grid: one value per grid point. A table
    whose values do not fill the grid is malformed — its values cannot be
    lined up with the points they are supposed to belong to — so none of
    them is compared: a delta computed from them would be a number without
    a meaning. Such a table is reported instead of diffed.

    ``expected_values`` is the number of points the grid holds and
    ``actual_values`` the number of values the table holds. When both
    tables of a pair are malformed, the first document's is reported.
    """

    #: The cell the table belongs to.
    cell: str
    #: The pin the table belongs to.
    pin: str
    #: The name of the table.
    table: str
    #: The number of values the table's grid holds.
    expected_values: int
    #: The number of values the table holds.
    actual_values: int


@dataclass
class Comparison:
    """The result of :func:`compare`."""

    #: One entry per compared grid point.
    entries: list[CompareEntry] = field(default_factory=list)
    #: Arcs that exist only in the first document.
    only_in_a: list[ArcKey] = field(default_factory=list)
    #: Arcs that exist only in the second document.
    only_in_b: list[ArcKey] = field(default_factory=list)
    #: Tables that exist only in the first document (per matched arc).
    only_in_a_tables: list[tuple[ArcKey, str]] = field(default_factory=list)
    #: Tables that exist only in the second document (per matched arc).
    only_in_b_tables: list[tuple[ArcKey, str]] = field(default_factory=list)
    #: Tables whose grids have a different number of axes:
    #: ``(arc, table, axes in the first document, axes in the second)``.
    shape_mismatches: list[tuple[ArcKey, str, int, int]] = field(default_factory=list)
    #: Tables that were not compared because the two documents declare
    #: different units for one of their dimensions.
    unit_mismatches: list[UnitMismatch] = field(default_factory=list)
    #: Tables that were not compared because the two documents name their
    #: axes differently and no role of one side has a counterpart in the
    #: other.
    axis_role_mismatches: list[AxisRoleMismatch] = field(default_factory=list)
    #: Tables whose values do not fill their grid: they are reported
    #: instead of diffed, because their numbers cannot be lined up with
    #: the points they belong to.
    malformed_tables: list[MalformedTable] = field(default_factory=list)
    #: Arcs whose identity is not unique: two arcs of the same document
    #: cannot be told apart. Their values are not compared.
    duplicate_arcs: list[ArcKey] = field(default_factory=list)
    #: Cell names that occur more than once in one of the two documents:
    #: the document holds several cell groups with that name. The first
    #: group is the one that is compared, and the repetition is reported
    #: here instead of being swallowed.
    duplicate_cells: list[str] = field(default_factory=list)

    def __len__(self) -> int:
        return len(self.entries)

    def __iter__(self):
        return iter(self.entries)

    @property
    def matched(self) -> list[CompareEntry]:
        """The entries where both documents have a value."""
        return [entry for entry in self.entries if entry.matched]

    @property
    def mismatched(self) -> list[CompareEntry]:
        """The matched entries whose values differ."""
        return [entry for entry in self.matched if entry.delta not in (0.0, -0.0)]

    @property
    def unmatched(self) -> list[CompareEntry]:
        """The entries where only one of the two documents has a value.

        A point that has no value on *either* side is not a difference: both
        documents agree that there is no data there, so it is not reported.
        """
        return [
            entry
            for entry in self.entries
            if (entry.value_a is None) != (entry.value_b is None)
        ]

    def _diagnostics(self) -> dict[str, list]:
        return {
            "only_in_a_arcs": self.only_in_a,
            "only_in_b_arcs": self.only_in_b,
            "only_in_a_tables": self.only_in_a_tables,
            "only_in_b_tables": self.only_in_b_tables,
            "shape_mismatches": self.shape_mismatches,
            "unit_mismatches": self.unit_mismatches,
            "axis_role_mismatches": self.axis_role_mismatches,
            "malformed_tables": self.malformed_tables,
            "duplicate_arcs": self.duplicate_arcs,
            "duplicate_cells": self.duplicate_cells,
        }

    @property
    def has_differences(self) -> bool:
        """Do the two documents differ in any way?

        True when the comparison found a difference of any kind: points whose
        values differ, points that have a value in only one of the two
        documents, arcs or tables that exist in only one document, tables
        whose grids have a different shape, tables that were not compared
        because of a unit or an axis role mismatch, tables whose values do
        not fill their grid, arcs whose identity is not unique, and cell
        names that occur more than once in one of the documents.

        A point that has no value in either document is not a difference:
        both documents agree that there is no data there.
        """
        return any(self._diagnostics().values()) or any(
            (entry.value_a is None) != (entry.value_b is None)
            or (entry.matched and entry.delta != 0.0)
            for entry in self.entries
        )

    def max_abs_delta(self) -> float | None:
        """The largest absolute delta over the matched entries."""
        return max(
            (abs(delta) for entry in self.entries if entry.matched
             and (delta := entry.delta) is not None),
            default=None,
        )

    def summary(self) -> dict[str, Any]:
        """Counts and the worst delta, for a quick report.

        Every kind of difference is counted, so that a consumer gating on
        ``summary()["has_differences"]`` cannot read a missing table or an
        incomparable table as "the libraries are the same".
        """
        matched = mismatched = unmatched = 0
        worst = None
        for entry in self.entries:
            if entry.matched:
                matched += 1
                delta = entry.delta
                mismatched += delta != 0.0
                if delta is not None:
                    magnitude = abs(delta)
                    if worst is None or magnitude > worst:
                        worst = magnitude
            unmatched += (entry.value_a is None) != (entry.value_b is None)
        diagnostics = {name: len(items) for name, items in self._diagnostics().items()}
        return {
            "entries": len(self.entries),
            "matched": matched,
            "mismatched": mismatched,
            "unmatched": unmatched,
            **diagnostics,
            "max_abs_delta": worst,
            "has_differences": bool(mismatched or unmatched or any(diagnostics.values())),
        }

    def to_polars(self):
        """The compared points as a :class:`polars.DataFrame`.

        The frame has the same columns — and the same column types —
        whether or not there is anything to compare, so a consumer can
        read an empty comparison without special-casing it.
        """
        import polars as pl

        schema = {
            "cell": pl.String,
            "pin": pl.String,
            "related_pin": pl.String,
            "timing_type": pl.String,
            "timing_sense": pl.String,
            "when": pl.String,
            "table": pl.String,
            "axis_1": pl.String,
            "axis_2": pl.String,
            "axis_3": pl.String,
            "index_1": pl.Float64,
            "index_2": pl.Float64,
            "index_3": pl.Float64,
            "value_a": pl.Float64,
            "value_b": pl.Float64,
            "delta": pl.Float64,
            "relative_delta": pl.Float64,
        }
        columns = {name: [getattr(entry, name) for entry in self.entries] for name in schema}
        return pl.DataFrame(columns, schema=schema)


def compare(
    a: LibertyDocument,
    b: LibertyDocument,
    *,
    cells: Sequence[str] | None = None,
    cell: str | None = None,
    pin: str | None = None,
    table: str | None = None,
    interpolate: bool = False,
    grid: dict[str, Sequence[float]] | None = None,
    tolerance: float = DEFAULT_TOLERANCE,
) -> Comparison:
    """Compare two Liberty documents.

    Arcs are matched by pin, related pin, timing type and timing sense;
    within such a group the ``when`` conditions are matched by logical
    equivalence, so ``"A & B"`` in one library matches ``"B & A"`` in the
    other. A condition that is only *implied* by the other describes a
    different arc, so those arcs are reported as existing in only one of the
    documents rather than compared. A library that leaves the ``timing_type``
    attribute out describes the same arc as one that writes ``combinational``,
    Liberty's default, and a library that leaves the ``timing_sense`` out
    describes the same arc as one that writes ``non_unate``.

    Two arcs of the same document that share their whole identity cannot be
    told apart. Such a collision is reported in
    :attr:`Comparison.duplicate_arcs` and neither of the two arcs is
    compared: picking one of them would compare values that do not
    correspond.

    A document that holds several cell groups with the same name is
    malformed: only the first group of a name can be addressed, so that
    group is the one that is compared. The repetition is reported in
    :attr:`Comparison.duplicate_cells` instead of being swallowed.

    The tables of matched arcs are matched by name, and their grids are aligned
    by *axis role* rather than by position: the load axis of one library is
    matched against the load axis of the other even when the two files store
    them in a different order. When the roles are unknown the axes are matched
    by position.

    When the roles are known on both sides but do not correspond — the load
    axis of one library against the transition axis of the other — there is
    nothing to align the grids on, and matching the axes by position would
    compare different quantities. Such a table is not diffed: it is reported
    in :attr:`Comparison.axis_role_mismatches` instead.

    By default only the points that exist in both grids are reported. With
    ``interpolate=True`` the second document is interpolated onto the first
    document's grid (or onto ``grid``), so that every point of the first grid
    gets a value. Points that exist in only one grid are always reported, with
    the missing side left as ``None``.

    A point that has no value in either document — a ``nan`` on both sides —
    is no difference: both documents agree that there is no data there, so it
    is not reported as unmatched.

    A table that depends on a unit the two documents declare differently — a
    time, a capacitive load, a voltage or a current — is not diffed: its
    numbers would measure different physical quantities. Such a table is
    reported in :attr:`Comparison.unit_mismatches` instead.

    A table whose values do not fill its grid is malformed: its values
    cannot be lined up with the points they belong to. None of them is
    compared, and the table is reported in
    :attr:`Comparison.malformed_tables` instead. A value that is not a
    number (``nan``, ``inf``) is no value at all: the point it belongs to
    is reported as missing rather than as a delta of ``nan``.

    Args:
        a: The first document (the one being validated).
        b: The second document (the reference).
        cells: Cell names to compare. Defaults to every cell of ``a``.
        cell: A single cell name to compare (a shorthand for ``cells=[cell]``).
        pin: Only compare arcs of this pin.
        table: Only compare tables with this name.
        interpolate: Interpolate ``b`` onto ``a``'s grid instead of comparing
            only the points the grids share.
        grid: The requested grid for ``interpolate=True``, as a mapping from
            an axis role (or ``"index_1"`` … ``"index_3"``) to the values to
            compare at. Defaults to the grid of ``a``.
        tolerance: Relative tolerance when matching axis values.

    Returns:
        A :class:`Comparison` with the compared points and the arcs and tables
        that exist in only one of the two documents.
    """
    names = list(cells) if cells is not None else None
    if cell is not None:
        names = [cell] if names is None else [*names, cell]
    if names is None:
        names = a.cells()
    # The same cell can be named twice: `cells=["u1"], cell="u1"` names it
    # twice, and a malformed library can hold two groups of the same name.
    # Reading its arcs twice is not an identity collision, so the names are
    # deduplicated (the first occurrence wins) before the arcs are read.
    names = list(dict.fromkeys(names))

    # A document that holds several cell groups with the same name is
    # malformed: only the first group of a name can be addressed, so the
    # repetition is reported. It is a property of the document, so it is
    # looked for in the whole document and not only in the compared cells.
    duplicate_cells = _duplicate_cell_names(a, b)

    arcs_a, duplicates_a = _collect_arcs(a, names, pin)
    arcs_b, duplicates_b = _collect_arcs(b, names, pin)

    # An arc whose identity is not unique cannot be matched to anything:
    # it is excluded from both documents and reported instead.
    duplicates = list(dict.fromkeys([*duplicates_a, *duplicates_b]))
    for key in duplicates:
        arcs_a.pop(key, None)
        arcs_b.pop(key, None)

    result = Comparison(duplicate_arcs=duplicates, duplicate_cells=duplicate_cells)
    units = _unit_context(a, b)
    matched_pairs, only_in_a, only_in_b = _match_arcs(arcs_a, arcs_b)
    result.only_in_a = only_in_a
    result.only_in_b = only_in_b
    for key_a, key_b in matched_pairs:
        arc_a = arcs_a[key_a]
        arc_b = arcs_b[key_b]
        names_a = set(arc_a.tables())
        names_b = set(arc_b.tables())
        for name in sorted(names_a - names_b):
            if table is not None and name != table:
                continue
            result.only_in_a_tables.append((key_a, name))
        for name in sorted(names_b - names_a):
            if table is not None and name != table:
                continue
            result.only_in_b_tables.append((key_b, name))
        for table_name in sorted(names_a & names_b):
            if table is not None and table_name != table:
                continue
            table_a = arc_a.table(table_name)
            table_b = arc_b.table(table_name)
            axes_a = _axes(table_a)
            axes_b = _axes(table_b)
            if axes_a and axes_b and len(axes_a) != len(axes_b):
                # The two tables have a different number of axes: there is no
                # grid to intersect, and interpolating one onto the other
                # would invent values. Report the mismatch instead.
                result.shape_mismatches.append(
                    (key_a, table_name, len(axes_a), len(axes_b))
                )
                continue
            if not axes_a and not axes_b:
                # A CCS wave container (`output_current_*`): the data lives in
                # the nested `vector` tables, not in a grid of its own.
                vectors_a = table_a.vectors()
                vectors_b = table_b.vectors()
                if len(vectors_a) != len(vectors_b):
                    result.only_in_a_tables.append(
                        (key_a, f"{table_name} ({len(vectors_a)} waves)")
                    )
                    result.only_in_b_tables.append(
                        (key_b, f"{table_name} ({len(vectors_b)} waves)")
                    )
                for index in range(min(len(vectors_a), len(vectors_b))):
                    result.entries.extend(
                        _compare_table(
                            key_a,
                            f"{table_name}[{index}]",
                            vectors_a[index],
                            vectors_b[index],
                            interpolate=interpolate,
                            grid=grid,
                            tolerance=tolerance,
                            unit_context=units,
                            unit_mismatches=result.unit_mismatches,
                            axis_role_mismatches=result.axis_role_mismatches,
                            malformed_tables=result.malformed_tables,
                        )
                    )
                continue
            result.entries.extend(
                _compare_table(
                    key_a,
                    table_name,
                    table_a,
                    table_b,
                    interpolate=interpolate,
                    grid=grid,
                    tolerance=tolerance,
                    unit_context=units,
                    unit_mismatches=result.unit_mismatches,
                    axis_role_mismatches=result.axis_role_mismatches,
                    malformed_tables=result.malformed_tables,
                )
            )
    return result


@dataclass(frozen=True)
class _UnitContext:
    """The unit declarations of two documents, and the dimensions that differ."""

    #: The declaration each document makes of each dimension.
    units_a: dict[str, str | None]
    units_b: dict[str, str | None]
    #: The dimensions whose declaration differs between the two documents.
    mismatched: frozenset[str]


def _strip_unit(value: str | None) -> str | None:
    """The unit string without surrounding whitespace."""
    if value is None:
        return None
    text = value.strip()
    return text or None


def _unit_declarations(
    document: LibertyDocument,
) -> dict[str, tuple[str | None, float | None]]:
    """The declaration of every unit dimension: ``(unit, multiplier)``."""
    return {
        "time": (_strip_unit(document.time_unit), None),
        "capacitive_load": (
            _strip_unit(document.capacitive_load_unit),
            document.capacitive_load_multiplier,
        ),
        "voltage": (_strip_unit(document.voltage_unit), None),
        "current": (_strip_unit(document.current_unit), None),
    }


def _declaration_text(
    dimension: str,
    unit: str | None,
    multiplier: float | None,
) -> str | None:
    """The whole declaration of a dimension, as it is reported.

    The capacitive load is declared as a multiplier *and* a unit
    (``capacitive_load_unit (1, pf)``), so both are reported: the multiplier
    alone is a difference the reader must see.
    """
    if dimension == "capacitive_load" and multiplier is not None:
        return f"{multiplier:g} {unit}" if unit else f"{multiplier:g}"
    return unit


def _unit_context(a: LibertyDocument, b: LibertyDocument) -> _UnitContext:
    """The unit declarations of two documents and the dimensions that differ.

    The unit strings are compared verbatim (after stripping whitespace); the
    multiplier of the capacitive load unit is compared numerically.
    """
    declarations_a = _unit_declarations(a)
    declarations_b = _unit_declarations(b)
    mismatched = {
        dimension
        for dimension in _UNIT_DIMENSIONS
        if declarations_a[dimension] != declarations_b[dimension]
    }
    return _UnitContext(
        units_a={
            dimension: _declaration_text(dimension, *value)
            for dimension, value in declarations_a.items()
        },
        units_b={
            dimension: _declaration_text(dimension, *value)
            for dimension, value in declarations_b.items()
        },
        mismatched=frozenset(mismatched),
    )


def _dimension_of_role(role: str | None) -> str | None:
    """The unit dimension an axis role names, if any."""
    if not role:
        return None
    text = role.lower()
    if "transition" in text or "time" in text:
        return "time"
    if "capacitance" in text:
        return "capacitive_load"
    if "voltage" in text:
        return "voltage"
    if "current" in text:
        return "current"
    return None


def _affected_dimensions(
    table_name: str,
    tables: Iterable[TimingTable],
) -> set[str]:
    """The unit dimensions a table depends on.

    A table depends on a dimension when one of its axes names it, or when
    its values carry it (a ``cell_rise`` holds times even when no axis says
    so, and an ``output_current_rise`` holds currents).
    """
    dimensions: set[str] = set()
    name = table_name.split("[", 1)[0]
    if name in _TIME_TABLE_NAMES:
        dimensions.add("time")
    if name in _CURRENT_TABLE_NAMES or name.startswith(_CURRENT_TABLE_PREFIXES):
        dimensions.add("current")
    for table in tables:
        for role in table.axis_roles():
            dimension = _dimension_of_role(role)
            if dimension is not None:
                dimensions.add(dimension)
    return dimensions


def _duplicate_cell_names(*documents: LibertyDocument) -> list[str]:
    """The cell names that occur more than once in the given documents.

    A document lists a cell name once per cell group, so a name that occurs
    more than once means the document holds several groups with that name.
    Such a document is malformed: only the first group of a name can be
    addressed, so the repetition is reported rather than swallowed.

    The names of all the documents are counted per document, so a name that
    is repeated in one of them is reported once — and a document that is
    compared with itself is not a document that repeats its cell names.
    """
    duplicates: set[str] = set()
    for document in documents:
        counts: dict[str, int] = {}
        for name in document.cells():
            counts[name] = counts.get(name, 0) + 1
        duplicates.update(name for name, count in counts.items() if count > 1)
    return sorted(duplicates)


def _collect_arcs(
    document: LibertyDocument,
    cells: Iterable[str],
    pin: str | None,
) -> tuple[dict[ArcKey, TimingArc], list[ArcKey]]:
    """Every timing arc of the given cells, keyed by its identity.

    Two arcs whose identity is the same cannot be told apart, so the
    collision is *reported* instead of resolved: the second arc does not
    overwrite the first one, and neither of them is returned.
    """
    arcs: dict[ArcKey, TimingArc] = {}
    duplicates: list[ArcKey] = []
    collided: set[ArcKey] = set()
    for cell_name in cells:
        try:
            cell = document.cell(cell_name)
        except KeyError:
            continue
        for pin_name, arc_list in _cell_arcs(cell).items():
            if pin is not None and pin_name != pin:
                continue
            for arc in arc_list:
                key = ArcKey(
                    cell_name,
                    pin_name,
                    arc.related_pin,
                    arc.timing_type,
                    arc.when,
                    arc.timing_sense,
                )
                if key in collided:
                    continue
                if key in arcs:
                    collided.add(key)
                    duplicates.append(key)
                    del arcs[key]
                    continue
                arcs[key] = arc
    return arcs, duplicates


def _cell_arcs(cell: Any) -> dict[str, list[TimingArc]]:
    """Timing arcs of a cell, including the pins of its buses and bundles."""
    arcs: dict[str, list[TimingArc]] = {}
    for pin_name in cell.pins():
        arcs[pin_name] = list(cell.pin(pin_name).timing_arcs())
    for bus_name in cell.buses():
        bus = cell.bus(bus_name)
        arcs.setdefault(bus_name, []).extend(bus.timing_arcs())
        for pin_name in bus.pins():
            pin_obj = bus.pin(pin_name)
            arcs.setdefault(pin_name, []).extend(pin_obj.timing_arcs())
    for bundle_name in cell.bundles():
        bundle = cell.bundle(bundle_name)
        arcs.setdefault(bundle_name, []).extend(bundle.timing_arcs())
        for pin_name in bundle.pins():
            pin_obj = bundle.pin(pin_name)
            arcs.setdefault(pin_name, []).extend(pin_obj.timing_arcs())
    return arcs


def _match_arcs(
    arcs_a: dict[ArcKey, TimingArc],
    arcs_b: dict[ArcKey, TimingArc],
) -> tuple[list[tuple[ArcKey, ArcKey]], list[ArcKey], list[ArcKey]]:
    """Pair up the arcs of two documents.

    Returns the matched pairs and the arcs that exist in only one document.
    """
    pairs: list[tuple[ArcKey, ArcKey]] = []
    matched_a: set[ArcKey] = set()
    matched_b: set[ArcKey] = set()

    groups_a: dict[tuple, list[ArcKey]] = {}
    for key in arcs_a:
        groups_a.setdefault(_arc_group(key), []).append(key)
    groups_b: dict[tuple, list[ArcKey]] = {}
    for key in arcs_b:
        groups_b.setdefault(_arc_group(key), []).append(key)

    for group, keys_a in groups_a.items():
        keys_b = groups_b.get(group, [])
        if not keys_b:
            continue
        group_pairs = _match_when_conditions(keys_a, keys_b)
        for key_a, key_b in group_pairs:
            pairs.append((key_a, key_b))
            matched_a.add(key_a)
            matched_b.add(key_b)

    only_in_a = [key for key in arcs_a if key not in matched_a]
    only_in_b = [key for key in arcs_b if key not in matched_b]
    return pairs, only_in_a, only_in_b


def _arc_group(key: ArcKey) -> tuple:
    """The part of an arc's identity that does not depend on ``when``."""
    return (
        key.cell,
        key.pin,
        key.related_pin,
        key.timing_type,
        key.timing_sense,
    )


def _match_when_conditions(
    keys_a: list[ArcKey],
    keys_b: list[ArcKey],
) -> list[tuple[ArcKey, ArcKey]]:
    """Pair arcs of one group by their ``when`` condition.

    Only *logically equivalent* conditions are paired. A condition that merely
    implies the other describes a different arc — comparing their values
    would report a delta between two things that do not correspond — so those
    arcs are reported as existing in only one of the documents instead.
    """
    condition_keys: dict[str | None, tuple[bool, str | None]] = {}

    def condition_key(when: str | None) -> tuple[bool, str | None]:
        if when not in condition_keys:
            expr = _condition_expr(when)
            condition_keys[when] = (False, when) if expr is None else (True, expr.canonical())
        return condition_keys[when]

    candidates: dict[tuple[bool, str | None], deque[ArcKey]] = defaultdict(deque)
    for key_b in keys_b:
        candidates[condition_key(key_b.when)].append(key_b)

    logical_pairs: list[tuple[ArcKey, ArcKey]] = []
    raw_pairs: list[tuple[ArcKey, ArcKey]] = []
    for key_a in keys_a:
        key = condition_key(key_a.when)
        remaining = candidates.get(key)
        if remaining:
            pairs = logical_pairs if key[0] else raw_pairs
            pairs.append((key_a, remaining.popleft()))
    return logical_pairs + raw_pairs


def _condition_expr(when: str | None) -> BooleanExpression | None:
    """Parse a ``when`` condition, or None when it cannot be parsed."""
    if when is None:
        return None
    from . import BooleanExpression

    try:
        return BooleanExpression(when)
    except ValueError:
        return None


def _compare_table(
    key: ArcKey,
    table_name: str,
    table_a: TimingTable,
    table_b: TimingTable,
    *,
    interpolate: bool,
    grid: dict[str, Sequence[float]] | None,
    tolerance: float,
    unit_context: _UnitContext | None = None,
    unit_mismatches: list[UnitMismatch] | None = None,
    axis_role_mismatches: list[AxisRoleMismatch] | None = None,
    malformed_tables: list[MalformedTable] | None = None,
) -> list[CompareEntry]:
    """Compare one pair of tables.

    A table that depends on a unit dimension the two documents declare
    differently is not compared: its numbers would mean different physical
    quantities, so the mismatch is reported instead. The same holds for a
    table whose axes the two documents name differently: matching the axes
    by position would compare different quantities.

    A table whose values do not fill its grid is malformed: its values
    cannot be lined up with the points they belong to, so none of them is
    used and the table is reported instead.
    """
    if unit_context is not None:
        differing = sorted(
            _affected_dimensions(table_name, (table_a, table_b))
            & unit_context.mismatched
        )
        if differing:
            if unit_mismatches is not None:
                for dimension in differing:
                    unit_mismatches.append(
                        UnitMismatch(
                            cell=key.cell,
                            pin=key.pin,
                            table=table_name,
                            dimension=dimension,
                            unit_a=unit_context.units_a.get(dimension),
                            unit_b=unit_context.units_b.get(dimension),
                        )
                    )
            return []
    roles_a = table_a.axis_roles()
    roles_b = table_b.axis_roles()
    axes_a = _axes(table_a)
    axes_b = _axes(table_b)

    # A table's values fill its grid. A table whose values do not fill it
    # is malformed: its numbers cannot be lined up with the points they
    # belong to, so none of them is used — the table is reported instead.
    values_a = table_a.values
    values_b = table_b.values
    malformed_a = _malformed_values(axes_a, values_a)
    malformed_b = _malformed_values(axes_b, values_b)
    if malformed_a is not None or malformed_b is not None:
        malformed = malformed_a if malformed_a is not None else malformed_b
        if malformed is not None and malformed_tables is not None:
            malformed_tables.append(
                MalformedTable(
                    cell=key.cell,
                    pin=key.pin,
                    table=table_name,
                    expected_values=malformed[0],
                    actual_values=malformed[1],
                )
            )
        if malformed_a is not None:
            values_a = []
        if malformed_b is not None:
            values_b = []

    # Map b's axes onto a's axes, by role when the roles are known. The
    # values are permuted with the axes: the flat list is row-major over the
    # table's own axis order.
    order_b = _axis_order(roles_a, roles_b, len(axes_a), len(axes_b))
    if order_b is None:
        # The two tables name their axes differently and no role of one side
        # has a counterpart in the other: matching the axes by position
        # would compare a load with a transition. The table is reported
        # instead of diffed.
        if axis_role_mismatches is not None:
            axis_role_mismatches.append(
                AxisRoleMismatch(
                    cell=key.cell,
                    pin=key.pin,
                    table=table_name,
                    roles_a=tuple(roles_a),
                    roles_b=tuple(roles_b),
                )
            )
        return []
    axes_b_ordered = [axes_b[index] for index in order_b]
    values_b_ordered = _permute_values(axes_b, order_b, values_b)

    entries: list[CompareEntry] = []
    if interpolate:
        points = _requested_points(table_a, grid)
        for point in points:
            entries.append(
                CompareEntry(
                    cell=key.cell,
                    pin=key.pin,
                    related_pin=key.related_pin,
                    timing_type=key.timing_type,
                    when=key.when,
                    table=table_name,
                    timing_sense=key.timing_sense,
                    index_1=point[0] if len(point) > 0 else None,
                    index_2=point[1] if len(point) > 1 else None,
                    index_3=point[2] if len(point) > 2 else None,
                    axis_1=roles_a[0] if len(roles_a) > 0 else None,
                    axis_2=roles_a[1] if len(roles_a) > 1 else None,
                    axis_3=roles_a[2] if len(roles_a) > 2 else None,
                    value_a=_interpolate(axes_a, values_a, point),
                    value_b=_interpolate(axes_b_ordered, values_b_ordered, point),
                )
            )
        return entries

    if (
        axes_a
        and malformed_a is None
        and malformed_b is None
        and _unique_equal_axes(axes_a, axes_b_ordered, tolerance)
    ):
        return [
            _entry(
                key, table_name, roles_a, point,
                a if math.isfinite(a) else None, b if math.isfinite(b) else None,
            )
            for point, a, b in zip(product(*axes_a), values_a, values_b_ordered, strict=True)
        ]

    # Points both grids have, and points only one of them has.
    for indices_a, indices_b in _grid_points(axes_a, axes_b_ordered, tolerance):
        point = tuple(axes_a[axis][index] for axis, index in enumerate(indices_a))
        value_a = _flat_value(axes_a, values_a, indices_a)
        value_b = (
            None
            if indices_b is None
            else _flat_value(axes_b_ordered, values_b_ordered, indices_b)
        )
        entries.append(_entry(key, table_name, roles_a, point, value_a, value_b))
    for indices_b in _points_only_in(axes_b_ordered, axes_a, tolerance):
        point = tuple(axes_b_ordered[axis][index] for axis, index in enumerate(indices_b))
        entries.append(
            _entry(
                key,
                table_name,
                roles_a,
                point,
                None,
                _flat_value(axes_b_ordered, values_b_ordered, indices_b),
            )
        )
    return entries


def _malformed_values(
    axes: Sequence[Sequence[float]],
    values: Sequence[float],
) -> tuple[int, int] | None:
    """The expected and actual value counts of a malformed table, if any.

    A table's values fill its grid: one value per grid point. A table
    whose values do not fill the grid is malformed — they cannot be lined
    up with the points they are supposed to belong to — so none of them is
    used.

    Returns the expected and the actual number of values, or None when the
    table is well formed.
    """
    expected = 1
    for axis in axes:
        expected *= len(axis)
    if not axes and not values:
        # A table with neither a grid nor values holds nothing: there is no
        # grid its values could contradict.
        return None
    if len(values) != expected:
        return (expected, len(values))
    return None


def _entry(
    key: ArcKey,
    table_name: str,
    roles: Sequence[str | None],
    point: Sequence[float],
    value_a: float | None,
    value_b: float | None,
) -> CompareEntry:
    """Build one comparison entry."""
    return CompareEntry(
        cell=key.cell,
        pin=key.pin,
        related_pin=key.related_pin,
        timing_type=key.timing_type,
        when=key.when,
        table=table_name,
        timing_sense=key.timing_sense,
        index_1=point[0] if len(point) > 0 else None,
        index_2=point[1] if len(point) > 1 else None,
        index_3=point[2] if len(point) > 2 else None,
        axis_1=roles[0] if len(roles) > 0 else None,
        axis_2=roles[1] if len(roles) > 1 else None,
        axis_3=roles[2] if len(roles) > 2 else None,
        value_a=value_a,
        value_b=value_b,
    )


def _axes(table: TimingTable) -> list[list[float]]:
    """The non-empty index axes of a table, in index order."""
    axes = []
    for values in (table.index_1, table.index_2, table.index_3):
        if values:
            axes.append(list(values))
    return axes


def _axis_order(
    roles_a: Sequence[str | None],
    roles_b: Sequence[str | None],
    count_a: int,
    count_b: int,
) -> list[int] | None:
    """Map b's axes onto a's axes, by role when the roles are known.

    Returns the indices of b's axes, in a's axis order, or None when the
    roles are known on both sides but do not correspond: a role of one table
    has no counterpart in the other, so the axes cannot be aligned and
    matching them by position would compare different quantities.

    When the roles are unknown — on either side — the axes are matched by
    position, which is all that can be done then.
    """
    if count_a != count_b:
        return list(range(count_b))
    if all(role is not None for role in roles_a[:count_a]) and all(
        role is not None for role in roles_b[:count_b]
    ):
        order: list[int] = []
        for role_a in roles_a[:count_a]:
            for index, role_b in enumerate(roles_b[:count_b]):
                if _same_axis(role_a, role_b) and index not in order:
                    order.append(index)
                    break
            else:
                # A role that is known on both sides has no counterpart:
                # the two tables do not describe the same quantities.
                return None
        return order
    return list(range(count_b))


def _same_axis(role_a: str | None, role_b: str | None) -> bool:
    """Do two axis variable names mean the same physical quantity?

    Uses the same synonym table as ``TimingTable.lookup``, so that a load axis
    named ``output_capacitance`` matches one named
    ``total_output_net_capacitance`` — and so that the shorthands
    ``lookup()`` accepts (``load``, ``slew``, ``transition``) name the axes
    they belong to.
    """
    if role_a is None or role_b is None:
        return False
    if role_a == role_b:
        return True
    from . import _axis_role_matches

    # The synonym table is looked up by the keyword, so both directions are
    # tried: a shorthand names a group, but is not part of it.
    return _axis_role_matches(role_b, role_a) or _axis_role_matches(role_a, role_b)


def _match_value(values: Sequence[float], target: float, tolerance: float) -> int | None:
    """Index of the value matching ``target`` within the tolerance."""
    for index, value in enumerate(values):
        if abs(value - target) <= tolerance * max(abs(value), abs(target), 1.0):
            return index
    return None


def _unique_equal_axes(
    axes_a: Sequence[Sequence[float]],
    axes_b: Sequence[Sequence[float]],
    tolerance: float,
) -> bool:
    """Whether each coordinate matches only itself in the other grid."""
    if not 0 <= tolerance < 1 or axes_a != axes_b:
        return False
    for axis in axes_a:
        if not all(map(math.isfinite, axis)):
            return False
        # For tolerance < 1, if a coordinate matches another distinct one on a
        # strictly increasing axis, it also matches its neighbor in that direction.
        if any(right - left <= tolerance * max(abs(left), abs(right), 1.0)
               for left, right in pairwise(axis)):
            return False
    return True


def _grid_points(
    axes_a: Sequence[Sequence[float]],
    axes_b: Sequence[Sequence[float]],
    tolerance: float,
) -> Iterator[tuple[tuple[int, ...], tuple[int, ...] | None]]:
    """Every point of a's grid, with b's position when b has that point."""
    if len(axes_a) != len(axes_b):
        return
    matches = [
        [_match_value(axis_b, value_a, tolerance) for value_a in axis_a]
        for axis_a, axis_b in zip(axes_a, axes_b)
    ]
    for indices in product(*(range(len(axis)) for axis in axes_a)):
        other = tuple(
            match for axis, index in zip(matches, indices)
            if (match := axis[index]) is not None
        )
        yield indices, other if len(other) == len(indices) else None


def _points_only_in(
    axes: Sequence[Sequence[float]],
    other: Sequence[Sequence[float]],
    tolerance: float,
) -> Iterator[tuple[int, ...]]:
    """The positions of the grid points only ``axes`` has."""
    return (indices for indices, match in _grid_points(axes, other, tolerance) if match is None)


def _permute_values(
    axes: Sequence[Sequence[float]],
    order: Sequence[int],
    values: Sequence[float],
) -> Sequence[float]:
    """Reorder a table's flat values to a new axis order.

    The flat list is row-major over the table's own axis order, so permuting
    the axes permutes the values with them.
    """
    if list(order) == list(range(len(axes))):
        return values
    out: list[float] = []
    original_indices = [0] * len(axes)
    for new_indices in _all_index_combinations(
        [len(axes[axis]) for axis in order]
    ):
        for position, axis_index in enumerate(order):
            original_indices[axis_index] = new_indices[position]
        flat = 0
        for position in range(len(axes)):
            flat = flat * len(axes[position]) + original_indices[position]
        out.append(values[flat] if flat < len(values) else float("nan"))
    return out


def _all_index_combinations(sizes: Sequence[int]) -> list[tuple[int, ...]]:
    """Every combination of index positions for the given axis sizes."""
    combinations: list[tuple[int, ...]] = [()]
    for size in sizes:
        combinations = [
            combination + (index,)
            for combination in combinations
            for index in range(size)
        ]
    return combinations


def _flat_value(
    axes: Sequence[Sequence[float]],
    values: Sequence[float],
    indices: Sequence[int],
) -> float | None:
    """The table value at a grid position, or None when it has none.

    A value that is not a number (``nan``, ``inf``) is no value at all:
    the point it belongs to has no data, not a data that cannot be
    compared.
    """
    flat = 0
    for axis, index in zip(axes, indices):
        flat = flat * len(axis) + index
    if flat >= len(values):
        return None
    value = values[flat]
    if not math.isfinite(value):
        return None
    return value


def _requested_points(
    table: TimingTable,
    grid: dict[str, Sequence[float]] | None,
) -> list[tuple[float, ...]]:
    """The grid points to compare at, in a's axis order."""
    axes = _axes(table)
    roles = table.axis_roles()
    if not grid:
        points: list[tuple[float, ...]] = []
        _cartesian_values(axes, 0, [], points)
        return points
    per_axis: list[list[float]] = []
    used: set[str] = set()
    for index, axis in enumerate(axes):
        requested = None
        for key, values in grid.items():
            if key == f"index_{index + 1}":
                requested = values
                used.add(key)
                break
            role = roles[index] if index < len(roles) else None
            if role is not None and _same_axis(role, key):
                requested = values
                used.add(key)
                break
        per_axis.append([float(value) for value in (requested if requested is not None else axis)])
    unknown = [key for key in grid if key not in used]
    if unknown:
        raise ValueError(
            f"the requested grid has no axis named {unknown[0]!r}; "
            f"the table's axes are {roles[: len(axes)]}"
        )
    points = []
    _cartesian_values(per_axis, 0, [], points)
    return points


def _cartesian_values(
    per_axis: Sequence[Sequence[float]],
    depth: int,
    current: list[float],
    out: list[tuple[float, ...]],
) -> None:
    """Every combination of the per-axis values."""
    if depth == len(per_axis):
        out.append(tuple(current))
        return
    for value in per_axis[depth]:
        current.append(value)
        _cartesian_values(per_axis, depth + 1, current, out)
        current.pop()


def _interpolate(
    axes: Sequence[Sequence[float]],
    values: Sequence[float],
    point: Sequence[float],
) -> float | None:
    """Interpolate the table at a point, or None when it is outside the grid.

    The interpolation needs every corner of the box the point falls into:
    a corner that has no value — or a value that is not a number — leaves
    the result unknown, and an unknown result is reported as missing rather
    than guessed at.
    """
    if not axes:
        if not values:
            return None
        value = values[0]
        return value if math.isfinite(value) else None
    if len(axes) != len(point):
        # The point has a different number of coordinates than the table has
        # axes: interpolating would silently drop a dimension.
        return None
    brackets: list[tuple[int, int, float]] = []
    for axis, coordinate in zip(axes, point):
        bracket = _bracket(axis, coordinate)
        if bracket is None:
            return None
        brackets.append(bracket)
    total = 0.0
    for corner in _corners(brackets):
        flat = 0
        weight = 1.0
        for axis, (low, high, ratio), position in zip(axes, brackets, corner):
            flat = flat * len(axis) + position
            if low == high:
                factor = 1.0
            elif position == low:
                factor = 1.0 - ratio
            else:
                factor = ratio
            weight *= factor
        if flat >= len(values):
            # The box reaches a point the table has no value for: the
            # result is unknown, not zero.
            return None
        value = values[flat]
        if not math.isfinite(value):
            return None
        total += value * weight
    return total


def _bracket(axis: Sequence[float], coordinate: float) -> tuple[int, int, float] | None:
    """The bracketing positions of a coordinate and the interpolation ratio."""
    if len(axis) == 1:
        return (0, 0, 0.0) if abs(axis[0] - coordinate) < 1e-12 else None
    for index in range(len(axis) - 1):
        low = axis[index]
        high = axis[index + 1]
        if coordinate >= low and coordinate <= high:
            span = high - low
            ratio = 0.0 if span == 0.0 else (coordinate - low) / span
            return (index, index + 1, ratio)
    return None


def _corners(brackets: Sequence[tuple[int, int, float]]) -> list[tuple[int, ...]]:
    """Every corner of the interpolation box."""
    corners: list[tuple[int, ...]] = [()]
    for low, high, _ in brackets:
        choices = (low,) if low == high else (low, high)
        corners = [corner + (choice,) for corner in corners for choice in choices]
    return corners
