from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

from . import _native
from .comparison import (
    ArcKey,
    AxisRoleMismatch,
    CompareEntry,
    Comparison,
    MalformedTable,
    UnitMismatch,
    compare,
)


@dataclass(frozen=True)
class MinPulseWidth:
    """The minimum clock pulse width of a pin.

    ``high`` and ``low`` are in the library's time unit. ``source`` says which
    convention the value was read from: ``"attribute"`` for the
    ``min_pulse_width_high`` / ``min_pulse_width_low`` pin attributes,
    ``"timing_group"`` for a ``min_pulse_width`` timing arc.

    The two forms measure slightly different things: the attribute form is
    the minimum high/low *phase* width, the timing group form the minimum
    width *per clock transition*.

    Unpacks as ``high, low``.
    """

    high: float | None
    low: float | None
    source: str

    def __iter__(self):
        yield self.high
        yield self.low


class LibertyDocument:
    """A fully parsed Liberty library.

    Returned by :func:`parse_file`. Use :meth:`cell` to fetch a cell,
    :meth:`timing_tables` / :meth:`to_polars` to extract table data, and
    :meth:`write` / :meth:`to_string` to emit the source with any attribute
    edits applied.
    """

    def __init__(self, native: _native.Document):
        self._native = native

    @property
    def library_name(self) -> str:
        """Name of the ``library`` group."""
        return self._native.library_name

    @property
    def voltage_unit(self) -> str | None:
        """The library's ``voltage_unit`` string, or ``None`` when undeclared."""
        return self._native.voltage_unit

    @property
    def current_unit(self) -> str | None:
        """The library's ``current_unit`` string, or ``None`` when undeclared."""
        return self._native.current_unit

    @property
    def time_unit(self) -> str | None:
        """The library's ``time_unit`` string, or ``None`` when undeclared."""
        return self._native.time_unit

    @property
    def capacitive_load_unit(self) -> str | None:
        """Capacitance unit, e.g. ``"ff"`` from ``capacitive_load_unit (1, ff)``."""
        return self._native.capacitive_load_unit

    def templates(self) -> dict[str, list[str | None]]:
        """Lookup-table templates: name -> ``[variable_1, variable_2, variable_3]``."""
        return self._native.templates()

    def attributes(self) -> list[tuple[str, str]]:
        """Ordered ``(name, value)`` library-group simple/complex attributes."""
        return self._native.attributes()

    def get(self, key: str) -> str | None:
        """Value of a library-group attribute, or ``None`` if it is absent.

        The uniform escape hatch for library-level scalars::

            doc.get("default_max_transition")   # -> "0.1"
        """
        return self._native.get(key)

    def operating_conditions(self) -> list[tuple[str, list[tuple[str, str]]]]:
        """``operating_conditions`` groups: ``(name, attributes)``."""
        return self._native.operating_conditions()

    def operating_condition(self, name: str) -> list[tuple[str, str]]:
        """Attributes of one ``operating_conditions`` group.

        Raises ``KeyError`` when the library has no such group.
        """
        return self._native.operating_condition(name)

    @property
    def capacitive_load_multiplier(self) -> float | None:
        """The multiplier of ``capacitive_load_unit (1, ff)`` — the ``1``."""
        return self._native.capacitive_load_multiplier

    @property
    def leakage_power_unit(self) -> str | None:
        """``leakage_power_unit`` (e.g. ``"1pW"``) — the unit of the leakage
        power values in the library."""
        return self._native.leakage_power_unit

    @property
    def default_max_transition(self) -> float | None:
        """``default_max_transition`` in the library time unit."""
        return self._native.default_max_transition

    @property
    def default_fanout_load(self) -> float | None:
        """``default_fanout_load`` (dimensionless)."""
        return self._native.default_fanout_load

    @property
    def nom_voltage(self) -> float | None:
        """``nom_voltage`` of the default operating condition, in the library
        voltage unit."""
        return self._native.nom_voltage

    @property
    def nom_temperature(self) -> float | None:
        """``nom_temperature`` of the default operating condition."""
        return self._native.nom_temperature

    def to_string(self) -> str:
        """The Liberty source text of this document.

        With no pending edits this is the original file, byte for byte. Edits
        made through :meth:`Cell.set_attribute` / :meth:`Pin.set_attribute` are
        applied to the affected cell groups; every other byte of the file is
        preserved (comments, spacing and groups this library does not model).

        Raises ``ValueError`` when the document was not parsed from a file, or
        when the source file changed since it was parsed.
        """
        return self._native.to_string()

    def write(self, path: str | Path) -> None:
        """Write the document to ``path`` (see :meth:`to_string`).

        A path ending in ``.gz`` is written gzip-compressed.
        """
        self._native.write(str(path))

    def driver_waveforms(self) -> list[TimingTable]:
        """``normalized_driver_waveform`` tables (input slew × normalized voltage
        → time). ``name`` holds the ``driver_waveform_name``."""
        return [TimingTable(w) for w in self._native.driver_waveforms()]

    def energy_unit_joules(self) -> float | None:
        """SI scale (joules) for ``internal_power`` energy values.

        ``internal_power`` is a Liberty misnomer: the table values are switching
        *energy*, not power. The scale is ``voltage_unit * current_unit *
        time_unit``; for ASAP7 (1V, 1mA, 1ps) that is ``1e-15`` (femtojoules).
        """
        return self._native.energy_unit_joules()

    def cells(self) -> list[str]:
        """Names of all cells in the library, in file order."""
        return self._native.cells()

    def cell(self, name: str) -> Cell:
        """The :class:`Cell` named ``name``. Raises ``KeyError`` if absent."""
        return Cell(self._native.cell(name))

    def bus_types(self) -> list[str]:
        """Names of all ``type`` bus definitions, in file order."""
        return self._native.bus_types()

    def bus_type(self, name: str) -> BusType:
        """The :class:`BusType` named ``name``. Raises ``KeyError`` if absent."""
        return BusType(self._native.bus_type(name))

    def timing_tables(self, **filters: Any) -> list[dict[str, Any]]:
        """Flattened timing-table rows (one dict per table point).

        Accepts the same keyword filters as :meth:`to_polars`: ``cell``,
        ``pin``, ``related_pin``, ``timing_type``, ``when`` and ``table``.
        """
        return self._native.timing_tables(**filters)

    def internal_power_tables(self, **filters: Any) -> list[dict[str, Any]]:
        """Flattened ``internal_power`` (rise_power/fall_power) table rows.

        Values are switching energy in library units; multiply by
        :meth:`energy_unit_joules` for joules. Non-propagating energy on input
        pins (input switches, no output switch) appears here too, typically as
        groups with no ``related_pin`` and a 1-D table over input transition.
        """
        return self._native.internal_power_tables(**filters)

    def to_polars(self, kind: str = "timing", **filters: Any):
        """Timing or internal-power table rows as a Polars ``DataFrame``.

        ``kind="timing"`` (the default) calls :meth:`timing_tables`;
        ``kind="power"`` calls :meth:`internal_power_tables`. Both accept the
        keyword filters described there.
        """
        if kind == "timing":
            rows = self.timing_tables(**filters)
        elif kind == "power":
            rows = self.internal_power_tables(**filters)
        else:
            raise ValueError("kind must be 'timing' or 'power'")
        import polars as pl

        # Scan all rows for schema: optional columns (when, related_pin, …) are
        # null in the first rows but become strings later, which trips Polars'
        # default 100-row inference.
        return pl.DataFrame(rows, infer_schema_length=None)


class BooleanExpression:
    """A parsed Liberty boolean expression.

    ``str(expr)`` is a minimized sum-of-products (compact, human-readable).
    ``==`` and ``hash`` compare the canonical sum-of-minterms over the support,
    so two expressions are equal exactly when they are logically equivalent
    (``BooleanExpression("A | B") == BooleanExpression("B | A")``,
    ``BooleanExpression("A & (B | !B)") == BooleanExpression("A")``).

    Construct one directly from any Liberty boolean string —
    ``BooleanExpression("CLK * D * !QN")`` — or obtain it from a parsed field via
    the ``*_expr`` accessors (``pin.function_expr()``, ``arc.when_expr()``, …).
    """

    def __init__(self, source: str | _native.BooleanExpression):
        if isinstance(source, str):
            self._native = _native.BooleanExpression(source)
        else:
            self._native = source

    @property
    def source(self) -> str:
        """The original (verbatim) expression text."""
        return self._native.source

    @property
    def raw(self) -> str:
        """Alias of :attr:`source`."""
        return self._native.raw

    @property
    def variables(self) -> list[str]:
        """Support variables (those the function depends on), sorted."""
        return self._native.variables

    def minterms(self) -> list[str]:
        """Canonical product terms (sum-of-minterms over the support)."""
        return self._native.minterms()

    def canonical(self) -> str:
        """The canonical sum-of-minterms string that backs equality/hashing."""
        return self._native.canonical()

    def minimized(self) -> str:
        """The minimized sum-of-products string (same as ``str(self)``)."""
        return self._native.minimized()

    def eval(self, mapping: dict[str, bool] | None = None, **kwargs: bool) -> bool:
        """Evaluate under a ``{var: bool}`` mapping and/or keyword args; unset
        variables default to false, unknown variables are ignored."""
        return self._native.eval(mapping, **kwargs)

    def implies(self, other: BooleanExpression) -> bool:
        """True if ``self`` logically implies ``other``."""
        return self._native.implies(other._native)

    def __str__(self) -> str:
        return self._native.__str__()

    def __repr__(self) -> str:
        return self._native.__repr__()

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, BooleanExpression):
            return NotImplemented
        return self._native == other._native

    def __hash__(self) -> int:
        return hash(self._native)


def _wrap_expr(native: _native.BooleanExpression | None) -> BooleanExpression | None:
    """Wrap an optional native ``BooleanExpression`` (None stays None)."""
    return BooleanExpression(native) if native is not None else None


def _axis_role_matches(role: str, keyword: str) -> bool:
    """Do two axis variable names mean the same physical quantity?

    Accepts the exact template variable name, its synonyms
    (``output_capacitance`` / ``total_output_net_capacitance``) and the usual
    shorthands (``load``, ``transition``, ``slew``, …).
    """
    return _native.axis_role_matches(role, keyword)


def _constraint_limit(arc: TimingArc, table_name: str) -> float | None:
    """Tightest limit of a constraint table (the largest value it holds)."""
    if table_name not in arc.tables():
        return None
    values = arc.table(table_name).values
    return max(values) if values else None


class Cell:
    """A cell in a Liberty library.

    Fetched by name from :meth:`LibertyDocument.cell`. Exposes the cell's
    pins, buses, bundles, power/ground pins, and sequential groups.
    """

    def __init__(self, native: _native.Cell):
        self._native = native

    @property
    def name(self) -> str:
        """Cell name."""
        return self._native.name

    @property
    def area(self) -> float | None:
        """Cell ``area`` in library area units, or ``None`` when undeclared."""
        return self._native.area

    def pins(self) -> list[str]:
        """Names of the cell's top-level pins, in file order."""
        return self._native.pins()

    def pin(self, name: str) -> Pin:
        """The :class:`Pin` named ``name``. Raises ``KeyError`` if absent."""
        return Pin(self._native.pin(name))

    def buses(self) -> list[str]:
        """Names of the cell's buses, in file order."""
        return self._native.buses()

    def bus(self, name: str) -> Bus:
        """The :class:`Bus` named ``name``. Raises ``KeyError`` if absent."""
        return Bus(self._native.bus(name))

    def bundles(self) -> list[str]:
        """Names of the cell's bundles, in file order."""
        return self._native.bundles()

    def bundle(self, name: str) -> Bundle:
        """The :class:`Bundle` named ``name``. Raises ``KeyError`` if absent."""
        return Bundle(self._native.bundle(name))

    def attributes(self) -> list[tuple[str, str]]:
        """Ordered ``(name, value)`` cell-level simple/complex attributes."""
        return self._native.attributes()

    def get(self, key: str) -> str | None:
        """Value of a cell attribute, or ``None`` if it is absent.

        The uniform escape hatch for cell-level scalars::

            cell.get("cell_leakage_power")   # -> "250.0"
        """
        return self._native.get(key)

    def set_attribute(self, key: str, value: str | float | int) -> None:
        """Set (or add) a cell attribute.

        The change is recorded on the owning document and applied by
        :meth:`LibertyDocument.write` / :meth:`LibertyDocument.to_string`; the
        rest of the file is preserved byte for byte::

            doc.cell("mycell").set_attribute("area", "59.8752")

        Setting ``area`` also updates :attr:`area`.
        """
        self._native.set_attribute(key, value)

    def set_area(self, value: float) -> None:
        """Set the cell ``area`` (the physical cell area, in library area units)."""
        self._native.set_area(value)

    @property
    def cell_leakage_power(self) -> float | None:
        """``cell_leakage_power`` in the library power unit.

        The power unit is the one the library declares as
        ``leakage_power_unit``; when the library declares no leakage unit it
        is voltage × current.
        """
        return self._native.cell_leakage_power

    @property
    def cell_leakage_power_watts(self) -> float | None:
        """``cell_leakage_power`` in watts."""
        return self._native.cell_leakage_power_watts

    @property
    def cell_footprint(self) -> str | None:
        """``cell_footprint`` (the cell's footprint name), if declared."""
        return self._native.cell_footprint

    @property
    def dont_touch(self) -> str | None:
        """``dont_touch`` as written in the file (``"true"`` / ``"false"``)."""
        return self._native.dont_touch

    @property
    def dont_use(self) -> str | None:
        """``dont_use`` as written in the file (``"true"`` / ``"false"``)."""
        return self._native.dont_use

    def dynamic_currents(self) -> list[DynamicCurrent]:
        """CCS power (``dynamic_current``) groups."""
        return [DynamicCurrent(dc) for dc in self._native.dynamic_currents()]

    def ff(self) -> Ff | None:
        """The flip-flop (``ff`` / ``ff_bank``) group, if the cell is sequential."""
        native = self._native.ff()
        return Ff(native) if native is not None else None

    def latch(self) -> Latch | None:
        """The latch (``latch`` / ``latch_bank``) group, if present."""
        native = self._native.latch()
        return Latch(native) if native is not None else None

    def leakage_powers(self) -> list[LeakagePower]:
        """``leakage_power`` groups (state-dependent / default static leakage)."""
        return [LeakagePower(lp) for lp in self._native.leakage_powers()]

    def pg_pins(self) -> list[PgPin]:
        """``pg_pin`` groups (power/ground rails)."""
        return [PgPin(pg) for pg in self._native.pg_pins()]


class Pin:
    """A pin (or bus/bundle member) of a :class:`Cell`.

    Directions and boolean attributes are returned as written in the file;
    the ``*_expr`` accessors return them parsed as
    :class:`BooleanExpression`.
    """

    def __init__(self, native: _native.Pin):
        self._native = native

    @property
    def name(self) -> str:
        """Pin name."""
        return self._native.name

    @property
    def direction(self) -> str | None:
        """``direction`` as written (``"input"``, ``"output"``, …), if declared."""
        return self._native.direction

    @property
    def function(self) -> str | None:
        """``function`` expression text, if declared."""
        return self._native.function

    @property
    def three_state(self) -> str | None:
        """``three_state`` expression text, if declared."""
        return self._native.three_state

    @property
    def power_down_function(self) -> str | None:
        """``power_down_function`` expression text, if declared."""
        return self._native.power_down_function

    def function_expr(self) -> BooleanExpression | None:
        """The ``function`` as a parsed :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.function_expr())

    def three_state_expr(self) -> BooleanExpression | None:
        """The ``three_state`` condition as a :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.three_state_expr())

    def power_down_function_expr(self) -> BooleanExpression | None:
        """The ``power_down_function`` as a :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.power_down_function_expr())

    def timing_arcs(
        self,
        *,
        related_pin: str | None = None,
        timing_type: str | None = None,
        when: str | None = None,
    ) -> list[TimingArc]:
        """Timing arcs that start or end at this pin.

        Optional keyword filters: ``related_pin``, ``timing_type`` and
        ``when``. ``when`` uses implication matching, so ``when="A"`` also
        matches an arc whose condition is ``"A & B"``.
        """
        return [
            TimingArc(arc)
            for arc in self._native.timing_arcs(
                related_pin=related_pin,
                timing_type=timing_type,
                when=when,
            )
        ]

    def internal_power(
        self,
        *,
        related_pin: str | None = None,
        related_pg_pin: str | None = None,
        when: str | None = None,
    ) -> list[InternalPower]:
        """``internal_power`` groups for this pin.

        Optional keyword filters: ``related_pin``, ``related_pg_pin`` and
        ``when``.
        """
        return [
            InternalPower(group)
            for group in self._native.internal_power(
                related_pin=related_pin,
                related_pg_pin=related_pg_pin,
                when=when,
            )
        ]

    def attributes(self) -> list[tuple[str, str]]:
        """Ordered ``(name, value)`` pin-level simple/complex attributes."""
        return self._native.attributes()

    def get(self, key: str) -> str | None:
        """Value of a pin attribute, or ``None`` if it is absent.

        The uniform escape hatch for pin-level scalars::

            pin.get("capacitance")   # -> "0.0044679"
        """
        return self._native.get(key)

    def set_attribute(self, key: str, value: str | float | int) -> None:
        """Set (or add) a pin attribute.

        The change is recorded on the owning document and applied by
        :meth:`LibertyDocument.write` / :meth:`LibertyDocument.to_string`::

            doc.cell("mycell").pin("A").set_attribute("capacitance", 0.0044)
        """
        self._native.set_attribute(key, value)

    @property
    def capacitance(self) -> float | None:
        """``capacitance`` in the library capacitive-load unit."""
        return self._native.capacitance

    @property
    def capacitance_farads(self) -> float | None:
        """``capacitance`` in farads."""
        return self._native.capacitance_farads

    @property
    def rise_capacitance(self) -> float | None:
        """``rise_capacitance`` in the library capacitive-load unit."""
        return self._native.rise_capacitance

    @property
    def rise_capacitance_farads(self) -> float | None:
        """``rise_capacitance`` in farads."""
        return self._native.rise_capacitance_farads

    @property
    def fall_capacitance(self) -> float | None:
        """``fall_capacitance`` in the library capacitive-load unit."""
        return self._native.fall_capacitance

    @property
    def fall_capacitance_farads(self) -> float | None:
        """``fall_capacitance`` in farads."""
        return self._native.fall_capacitance_farads

    @property
    def max_transition(self) -> float | None:
        """``max_transition`` in the library time unit."""
        return self._native.max_transition

    @property
    def max_transition_seconds(self) -> float | None:
        """``max_transition`` in seconds."""
        return self._native.max_transition_seconds

    @property
    def min_pulse_width_high(self) -> float | None:
        """``min_pulse_width_high`` in the library time unit.

        This is the *pin attribute* form of the minimum clock pulse width; see
        :meth:`min_pulse_width` for the normalized accessor.
        """
        return self._native.min_pulse_width_high

    @property
    def min_pulse_width_high_seconds(self) -> float | None:
        """``min_pulse_width_high`` in seconds."""
        return self._native.min_pulse_width_high_seconds

    @property
    def min_pulse_width_low(self) -> float | None:
        """``min_pulse_width_low`` in the library time unit.

        This is the *pin attribute* form of the minimum clock pulse width; see
        :meth:`min_pulse_width` for the normalized accessor.
        """
        return self._native.min_pulse_width_low

    @property
    def min_pulse_width_low_seconds(self) -> float | None:
        """``min_pulse_width_low`` in seconds."""
        return self._native.min_pulse_width_low_seconds

    def min_pulse_width(self) -> MinPulseWidth | None:
        """The minimum clock pulse width, from whichever form the file uses.

        Two conventions exist and a file may use either:

        - the *attribute* form — ``min_pulse_width_high`` / ``min_pulse_width_low``
          pin attributes, one number each;
        - the *timing group* form — a ``min_pulse_width`` timing arc on the
          clock pin itself, whose ``rise_constraint`` / ``fall_constraint``
          tables hold the limit per clock transition.

        The attribute form wins when both are present. For the timing group
        form the reported number is the largest value of the constraint table
        (the tightest limit over the tabulated clock transitions).

        Returns ``None`` when the file expresses the limit in neither form.
        The result unpacks as ``high, low`` and carries the form it was read
        from in :attr:`MinPulseWidth.source`.
        """
        high = self.min_pulse_width_high
        low = self.min_pulse_width_low
        if high is not None or low is not None:
            return MinPulseWidth(high, low, "attribute")
        for arc in self.timing_arcs(timing_type="min_pulse_width"):
            high = _constraint_limit(arc, "rise_constraint")
            low = _constraint_limit(arc, "fall_constraint")
            if high is not None or low is not None:
                return MinPulseWidth(high, low, "timing_group")
        return None


class Bus:
    """A bus defined on a :class:`Cell` (a ``bus`` group)."""

    def __init__(self, native: _native.Bus):
        self._native = native

    @property
    def name(self) -> str:
        return self._native.name

    @property
    def direction(self) -> str | None:
        return self._native.direction

    @property
    def function(self) -> str | None:
        return self._native.function

    def function_expr(self) -> BooleanExpression | None:
        """The bus ``function`` as a parsed :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.function_expr())

    @property
    def bus_type(self) -> str | None:
        return self._native.bus_type

    def pins(self) -> list[str]:
        return self._native.pins()

    def pin(self, name: str) -> Pin:
        return Pin(self._native.pin(name))

    def timing_arcs(
        self,
        *,
        related_pin: str | None = None,
        timing_type: str | None = None,
        when: str | None = None,
    ) -> list[TimingArc]:
        return [
            TimingArc(arc)
            for arc in self._native.timing_arcs(
                related_pin=related_pin,
                timing_type=timing_type,
                when=when,
            )
        ]


class Bundle:
    """A bundle of pins on a :class:`Cell` (a ``bundle`` group)."""

    def __init__(self, native: _native.Bundle):
        self._native = native

    @property
    def name(self) -> str:
        return self._native.name

    @property
    def direction(self) -> str | None:
        return self._native.direction

    @property
    def function(self) -> str | None:
        return self._native.function

    def function_expr(self) -> BooleanExpression | None:
        """The bundle ``function`` as a parsed :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.function_expr())

    @property
    def members(self) -> list[str]:
        return self._native.members

    def pins(self) -> list[str]:
        return self._native.pins()

    def pin(self, name: str) -> Pin:
        return Pin(self._native.pin(name))

    def timing_arcs(
        self,
        *,
        related_pin: str | None = None,
        timing_type: str | None = None,
        when: str | None = None,
    ) -> list[TimingArc]:
        return [
            TimingArc(arc)
            for arc in self._native.timing_arcs(
                related_pin=related_pin,
                timing_type=timing_type,
                when=when,
            )
        ]


class BusType:
    """A Liberty ``type`` bus definition."""

    def __init__(self, native: _native.BusType):
        self._native = native

    @property
    def name(self) -> str:
        return self._native.name

    def attributes(self) -> dict[str, str]:
        return self._native.attributes()

    def get(self, key: str) -> str | None:
        return self._native.get(key)


class TimingArc:
    """A timing arc between two pins.

    Exposes the arc's scalar attributes plus its timing tables
    (``cell_rise``, ``cell_fall``, constraints, …). CCS noise stages are
    available through :meth:`ccsn_stages`.
    """

    def __init__(self, native: _native.TimingArc):
        self._native = native

    @property
    def related_pin(self) -> str | None:
        return self._native.related_pin

    @property
    def timing_type(self) -> str | None:
        return self._native.timing_type

    @property
    def when(self) -> str | None:
        return self._native.when

    @property
    def sdf_cond(self) -> str | None:
        return self._native.sdf_cond

    @property
    def timing_sense(self) -> str | None:
        return self._native.timing_sense

    def when_expr(self) -> BooleanExpression | None:
        """The ``when`` condition as a parsed :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.when_expr())

    def sdf_cond_expr(self) -> BooleanExpression | None:
        """The ``sdf_cond`` as a parsed :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.sdf_cond_expr())

    def tables(self) -> list[str]:
        return self._native.tables()

    def table(self, name: str) -> TimingTable:
        return TimingTable(self._native.table(name))

    def ccsn_stages(self) -> list[CcsnStage]:
        return [CcsnStage(stage) for stage in self._native.ccsn_stages()]


class CcsnStage:
    """One CCS-noise stage (``ccsn_first_stage`` / ``ccsn_last_stage``)."""

    def __init__(self, native: _native.CcsnStage):
        self._native = native

    @property
    def name(self) -> str:
        return self._native.name

    @property
    def is_inverting(self) -> str | None:
        return self._native.is_inverting

    @property
    def is_needed(self) -> str | None:
        return self._native.is_needed

    @property
    def is_pass_gate(self) -> str | None:
        return self._native.is_pass_gate

    @property
    def stage_type(self) -> str | None:
        return self._native.stage_type

    @property
    def miller_cap_rise(self) -> float | None:
        return self._native.miller_cap_rise

    @property
    def miller_cap_fall(self) -> float | None:
        return self._native.miller_cap_fall

    @property
    def when(self) -> str | None:
        return self._native.when

    @property
    def mode(self) -> str | None:
        return self._native.mode

    def dc_current(self) -> TimingTable | None:
        native = self._native.dc_current()
        return TimingTable(native) if native is not None else None

    def output_voltage_rise(self) -> list[TimingTable]:
        return [TimingTable(table) for table in self._native.output_voltage_rise()]

    def output_voltage_fall(self) -> list[TimingTable]:
        return [TimingTable(table) for table in self._native.output_voltage_fall()]


class InternalPower:
    """An ``internal_power`` group (switching energy, not power)."""

    def __init__(self, native: _native.InternalPower):
        self._native = native

    @property
    def related_pin(self) -> str | None:
        return self._native.related_pin

    @property
    def related_pg_pin(self) -> str | None:
        return self._native.related_pg_pin

    @property
    def when(self) -> str | None:
        return self._native.when

    def when_expr(self) -> BooleanExpression | None:
        """The ``when`` condition as a parsed :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.when_expr())

    def tables(self) -> list[str]:
        return self._native.tables()

    def table(self, name: str) -> TimingTable:
        return TimingTable(self._native.table(name))


class DynamicCurrent:
    """A ``dynamic_current`` group (CCS power: per-condition PG current waves)."""

    def __init__(self, native: _native.DynamicCurrent):
        self._native = native

    @property
    def related_inputs(self) -> str | None:
        return self._native.related_inputs

    @property
    def related_outputs(self) -> str | None:
        return self._native.related_outputs

    @property
    def when(self) -> str | None:
        return self._native.when

    def when_expr(self) -> BooleanExpression | None:
        """The ``when`` condition as a parsed :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.when_expr())

    def switching_groups(self) -> list[SwitchingGroup]:
        return [SwitchingGroup(sg) for sg in self._native.switching_groups()]


class SwitchingGroup:
    """A CCS switching group inside ``dynamic_current``."""

    def __init__(self, native: _native.SwitchingGroup):
        self._native = native

    @property
    def input_switching_condition(self) -> str | None:
        return self._native.input_switching_condition

    @property
    def output_switching_condition(self) -> str | None:
        return self._native.output_switching_condition

    def input_switching_expr(self) -> BooleanExpression | None:
        """``input_switching_condition`` as a :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.input_switching_expr())

    def output_switching_expr(self) -> BooleanExpression | None:
        """``output_switching_condition`` as a :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.output_switching_expr())

    def pg_currents(self) -> list[PgCurrent]:
        return [PgCurrent(pg) for pg in self._native.pg_currents()]


class PgCurrent:
    """A ``pg_current`` group: current-vs-time waves for one PG pin."""

    def __init__(self, native: _native.PgCurrent):
        self._native = native

    @property
    def pg_pin(self) -> str | None:
        return self._native.pg_pin

    def vectors(self) -> list[TimingTable]:
        return [TimingTable(v) for v in self._native.vectors()]


class PgPin:
    """A ``pg_pin`` group (power/ground rail): a name and its simple attributes
    (``pg_type``, ``voltage_name``, …)."""

    def __init__(self, native: _native.PgPin):
        self._native = native

    @property
    def name(self) -> str:
        return self._native.name

    def attributes(self) -> list[tuple[str, str]]:
        """Ordered ``(name, value)`` simple attributes of the pg_pin group."""
        return self._native.attributes()


class LeakagePower:
    """A ``leakage_power`` group: a static leakage ``value`` (library power
    units), optionally gated by a ``when`` state and tied to a power rail."""

    def __init__(self, native: _native.LeakagePower):
        self._native = native

    @property
    def value(self) -> float | None:
        return self._native.value

    @property
    def when(self) -> str | None:
        return self._native.when

    @property
    def related_pg_pin(self) -> str | None:
        return self._native.related_pg_pin

    def when_expr(self) -> BooleanExpression | None:
        """The ``when`` condition as a parsed :class:`BooleanExpression` (None if absent)."""
        return _wrap_expr(self._native.when_expr())


class Ff:
    """A flip-flop (``ff`` / ``ff_bank``) group. The boolean attributes are
    available raw (strings) and as parsed expressions via ``*_expr``."""

    def __init__(self, native: _native.Ff):
        self._native = native

    @property
    def variable1(self) -> str | None:
        return self._native.variable1

    @property
    def variable2(self) -> str | None:
        return self._native.variable2

    @property
    def next_state(self) -> str | None:
        return self._native.next_state

    @property
    def clocked_on(self) -> str | None:
        return self._native.clocked_on

    @property
    def clocked_on_also(self) -> str | None:
        return self._native.clocked_on_also

    @property
    def clear(self) -> str | None:
        return self._native.clear

    @property
    def preset(self) -> str | None:
        return self._native.preset

    @property
    def power_down_function(self) -> str | None:
        return self._native.power_down_function

    def next_state_expr(self) -> BooleanExpression | None:
        return _wrap_expr(self._native.next_state_expr())

    def clocked_on_expr(self) -> BooleanExpression | None:
        return _wrap_expr(self._native.clocked_on_expr())

    def clocked_on_also_expr(self) -> BooleanExpression | None:
        return _wrap_expr(self._native.clocked_on_also_expr())

    def clear_expr(self) -> BooleanExpression | None:
        return _wrap_expr(self._native.clear_expr())

    def preset_expr(self) -> BooleanExpression | None:
        return _wrap_expr(self._native.preset_expr())

    def power_down_function_expr(self) -> BooleanExpression | None:
        return _wrap_expr(self._native.power_down_function_expr())


class Latch:
    """A latch (``latch`` / ``latch_bank``) group."""

    def __init__(self, native: _native.Latch):
        self._native = native

    @property
    def variable1(self) -> str | None:
        return self._native.variable1

    @property
    def variable2(self) -> str | None:
        return self._native.variable2

    @property
    def enable(self) -> str | None:
        return self._native.enable

    @property
    def data_in(self) -> str | None:
        return self._native.data_in

    @property
    def clear(self) -> str | None:
        return self._native.clear

    @property
    def preset(self) -> str | None:
        return self._native.preset

    def enable_expr(self) -> BooleanExpression | None:
        return _wrap_expr(self._native.enable_expr())

    def data_in_expr(self) -> BooleanExpression | None:
        return _wrap_expr(self._native.data_in_expr())

    def clear_expr(self) -> BooleanExpression | None:
        return _wrap_expr(self._native.clear_expr())

    def preset_expr(self) -> BooleanExpression | None:
        return _wrap_expr(self._native.preset_expr())


class TimingTable:
    """One Liberty lookup table.

    The ``values`` list is **flat** and **row-major** over the table's axes:
    ``index_1`` is the slowest axis and ``index_3`` the fastest. For a 2-D
    table::

        values[i * len(index_2) + j]     # i on index_1, j on index_2

    and for a 3-D table::

        values[i * len(index_2) * len(index_3) + j * len(index_3) + k]

    The long-form dataframes use the same convention (``row`` indexes
    ``index_1``, ``col`` indexes ``index_2``, ``depth`` indexes ``index_3``).
    Use :meth:`lookup` or :meth:`value_at` rather than deriving the flat
    index yourself.
    """

    def __init__(self, native: _native.TimingTable):
        self._native = native

    @property
    def name(self) -> str:
        """Table name, e.g. ``"cell_rise"`` or ``"rise_constraint"``."""
        return self._native.name

    @property
    def index_1(self) -> list[float]:
        """Values of the ``index_1`` axis, in file order."""
        return self._native.index_1

    @property
    def index_2(self) -> list[float]:
        """Values of the ``index_2`` axis; empty when the table has no such axis."""
        return self._native.index_2

    @property
    def index_3(self) -> list[float]:
        """Values of the ``index_3`` axis; empty when the table has no such axis."""
        return self._native.index_3

    @property
    def values(self) -> list[float]:
        """Flat table values, row-major with ``index_1`` slowest.

        Use :meth:`lookup` or :meth:`value_at` instead of indexing this list
        directly.
        """
        return self._native.values

    @property
    def template(self) -> str | None:
        """Lookup-table template name from the group header (axis variables)."""
        return self._native.template

    def axis_roles(self) -> list[str | None]:
        """The physical variable of each axis, in index order.

        ``["input_net_transition", "total_output_net_capacitance"]`` for a
        2-D NLDM table whose template says so. The list is truncated to the
        axes the table actually has; entries are ``None`` when the table has
        no template or the template does not name that axis.
        """
        return self._native.axis_roles()

    def axis_role(self, axis: int) -> str | None:
        """The variable of one axis: 1 = ``index_1``, 2 = ``index_2``,
        3 = ``index_3``."""
        return self._native.axis_role(axis)

    @property
    def index_1_variable(self) -> str | None:
        """The variable of the ``index_1`` axis, e.g. ``"input_net_transition"``."""
        return self._native.index_1_variable

    @property
    def index_2_variable(self) -> str | None:
        """The variable of the ``index_2`` axis."""
        return self._native.index_2_variable

    @property
    def index_3_variable(self) -> str | None:
        """The variable of the ``index_3`` axis."""
        return self._native.index_3_variable

    def lookup(
        self,
        index_1: float | None = None,
        index_2: float | None = None,
        index_3: float | None = None,
        **roles: float,
    ) -> float | None:
        """The value at a grid point, or ``None`` when the point is off-grid.

        Give the position on each axis either by index::

            table.lookup(index_1=0.108, index_2=0.0966)

        or by the axis's physical variable — the template's variable name, or
        the usual shorthands ``load`` and ``transition``::

            table.lookup(load=0.108, transition=0.0966)

        Every axis of the table needs a coordinate; a missing one is an error
        (``ValueError``). Coordinates are matched against the axis values with
        a small relative tolerance.
        """
        return self._native.lookup(
            index_1=index_1,
            index_2=index_2,
            index_3=index_3,
            **roles,
        )

    def value_at(self, row: int, col: int, depth: int | None = None) -> float:
        """The value at a grid position.

        ``row`` indexes ``index_1``, ``col`` indexes ``index_2`` and ``depth``
        indexes ``index_3`` — the same meaning the ``row`` / ``col`` / ``depth``
        columns of :meth:`LibertyDocument.to_polars` have. Raises
        ``IndexError`` when the position is outside the table.
        """
        if depth is None:
            return self._native.value_at(row, col)
        return self._native.value_at(row, col, depth)

    @property
    def reference_time(self) -> float | None:
        """CCS only: the ``reference_time`` of a ``vector`` group."""
        return self._native.reference_time

    def vectors(self) -> list[TimingTable]:
        """CCS only: nested ``vector`` sub-tables (current-vs-time waves).

        Empty for ordinary NLDM tables. Each vector has a single ``index_1``
        (input slew) and ``index_2`` (output cap), an ``index_3`` time axis, and
        ``values`` holding the output current samples.
        """
        return [TimingTable(v) for v in self._native.vectors()]

    def to_polars(self):
        """The table as a Polars ``DataFrame`` (one row per table point)."""
        import polars as pl

        return pl.DataFrame(self._native.rows())


class LibraryIndex:
    """Lazy, cell-indexed view of a Liberty file for fast open on large libs.

    Reads the (decompressed) file into memory once and records each cell's byte
    range; the library header (units/templates/attrs) is parsed up front, but
    individual cells are parsed only when :meth:`cell` is called. Use this
    instead of :func:`parse_file` when you need to list/inspect a few cells of a
    multi-GB library without paying for a full parse.
    """

    def __init__(self, native: _native.LibraryIndex):
        self._native = native

    @classmethod
    def open(cls, path: str | Path) -> LibraryIndex:
        return cls(_native.LibraryIndex.open(str(path)))

    @property
    def library_name(self) -> str:
        return self._native.library_name

    @property
    def voltage_unit(self) -> str | None:
        return self._native.voltage_unit

    @property
    def current_unit(self) -> str | None:
        return self._native.current_unit

    @property
    def time_unit(self) -> str | None:
        return self._native.time_unit

    @property
    def capacitive_load_unit(self) -> str | None:
        return self._native.capacitive_load_unit

    def energy_unit_joules(self) -> float | None:
        return self._native.energy_unit_joules()

    def templates(self) -> dict[str, list[str | None]]:
        return self._native.templates()

    def attributes(self) -> list[tuple[str, str]]:
        return self._native.attributes()

    def driver_waveforms(self) -> list[TimingTable]:
        return [TimingTable(w) for w in self._native.driver_waveforms()]

    def cell_names(self) -> list[str]:
        return self._native.cell_names()

    def num_cells(self) -> int:
        return self._native.num_cells()

    def cell(self, name: str) -> Cell:
        return Cell(self._native.cell(name))

    def cell_source(self, name: str) -> str:
        """Raw Liberty source text of one cell, sliced from the in-memory buffer
        (O(cell size), independent of total file size)."""
        return self._native.cell_source(name)


def parse_file(path: str | Path, **filters: Any) -> LibertyDocument:
    """Parse a Liberty file into a :class:`LibertyDocument`.

    Args:
        path: ``.lib`` or ``.lib.gz`` path.
        **filters: Parse-time filters. ``cells=[...]`` parses only the named
            cells and skips the rest of the file.

    Examples:
        >>> doc = parse_file("library.lib", cells=["target_cell"])
    """
    return LibertyDocument(_native.parse_file(str(path), **filters))


__all__ = [
    "ArcKey",
    "AxisRoleMismatch",
    "BooleanExpression",
    "Cell",
    "Bus",
    "Bundle",
    "BusType",
    "CcsnStage",
    "CompareEntry",
    "Comparison",
    "DynamicCurrent",
    "Ff",
    "InternalPower",
    "Latch",
    "LeakagePower",
    "LibertyDocument",
    "LibraryIndex",
    "MalformedTable",
    "MinPulseWidth",
    "Pin",
    "PgCurrent",
    "PgPin",
    "SwitchingGroup",
    "TimingArc",
    "TimingTable",
    "UnitMismatch",
    "compare",
    "parse_file",
]
