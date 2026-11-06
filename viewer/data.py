"""Data-access layer for the Liberty viewer.

Wraps a parsed ``LibertyDocument`` and exposes exactly what the HTTP API needs:
a lazily-built per-cell tree (metadata only, no table values) plus resolution of
a single table leaf to its axes/values. Keeping value extraction per-leaf is what
makes the design tolerant of very large libraries — the browser only ever pulls
one table at a time.
"""

from __future__ import annotations

import math
import re
from dataclasses import dataclass, field
from typing import Any, overload

import liberty_tools as lt

# Physical quantity ("kind") of each template axis variable and of each table's
# value, so axes/values can be labelled "<name> (<unit>)".
_VAR_KIND = {
    "input_net_transition": "time",
    "constrained_pin_transition": "time",
    "related_pin_transition": "time",
    "input_transition_time": "time",
    "time": "time",
    "input_noise_width": "time",
    "total_output_net_capacitance": "cap",
    "output_net_capacitance": "cap",
    "input_voltage": "voltage",
    "output_voltage": "voltage",
    "normalized_voltage": "voltage",
    "input_noise_height": "voltage",
}
_VALUE_KIND = {
    "cell_rise": "time",
    "cell_fall": "time",
    "rise_transition": "time",
    "fall_transition": "time",
    "rise_constraint": "time",
    "fall_constraint": "time",
    "retaining_rise": "time",
    "retaining_fall": "time",
    "retain_rise_slew": "time",
    "retain_fall_slew": "time",
    "cell_degradation": "time",
    "dc_current": "current",
    "output_current_rise": "current",
    "output_current_fall": "current",
    "rise_power": "energy",
    "fall_power": "energy",
    "receiver_capacitance1_rise": "cap",
    "receiver_capacitance1_fall": "cap",
    "receiver_capacitance2_rise": "cap",
    "receiver_capacitance2_fall": "cap",
}
_SI_PREFIX = {-18: "a", -15: "f", -12: "p", -9: "n", -6: "u", -3: "m", 0: ""}

# Short display aliases for axis variable names (labels only; raw names are kept
# in the parser/API).
_ALIAS = {
    "total_output_net_capacitance": "load",
    "input_net_transition": "slew",
    "input_transition_time": "slew",
}


# Liberty attributes whose value is a boolean expression — displayed in the
# canonical (minimized sum-of-products) format rather than verbatim.
_BOOL_ATTRS = frozenset(
    {
        "function",
        "three_state",
        "power_down_function",
        "when",
        "sdf_cond",
        "input_switching_condition",
        "output_switching_condition",
        "next_state",
        "clocked_on",
        "clocked_on_also",
        "clear",
        "preset",
        "enable",
        "data_in",
    }
)


@overload
def _fmt_bool(raw: str) -> str: ...


@overload
def _fmt_bool(raw: None) -> None: ...


def _fmt_bool(raw: str | None) -> str | None:
    """Render a Liberty boolean string in the canonical viewer format (minimized
    sum-of-products via :class:`liberty_tools.BooleanExpression`). Falls back to
    the verbatim text if it doesn't parse as a boolean expression."""
    if not raw:
        return raw
    try:
        return str(lt.BooleanExpression(raw))
    except ValueError:
        return raw


def _fmt_attr_rows(rows: list[tuple[str, str]]) -> list[list[str]]:
    """Format a raw ``(name, value)`` attribute list, canonicalizing the values
    of boolean-valued attributes (function, three_state, …)."""
    return [[k, _fmt_bool(v) if k in _BOOL_ATTRS else v] for k, v in rows]


def _step(group: str, name: str | None = None, indices: list[int] | None = None) -> dict[str, Any]:
    """One step of a source path: a `group (...)` block matched by paren `name`
    (e.g. `pin (Y)`) or by `indices`-th occurrence (e.g. the 0th `timing`)."""
    return {"group": group, "name": name} if name is not None else {"group": group, "indices": indices}


def _path_src(steps: list[dict[str, Any]], label: str) -> dict[str, Any]:
    """Source locator: a path of nested groups walked from the cell text down, so
    the source pane trims to exactly the selected line item (down to one table)."""
    return {"kind": "path", "path": steps, "label": label}


def _propagate_src(node: dict[str, Any], parent_src: dict[str, Any]) -> None:
    """Stamp every node with a source path: nodes that don't define their own
    inherit the nearest ancestor that does."""
    src = node.get("src") or parent_src
    node["src"] = src
    for child in node.get("children", []):
        _propagate_src(child, src)


def _meta_attrs(meta: dict[str, Any]) -> list[list[str]]:
    """Turn a node's scalar meta dict into ordered ``[name, value]`` rows,
    dropping empty entries — for nodes (arc/power/bus/bundle) that have no raw
    Liberty attribute list of their own. Boolean-valued entries are rendered in
    the canonical format."""
    out: list[list[str]] = []
    for k, v in meta.items():
        if v in (None, ""):
            continue
        out.append([k, _fmt_bool(str(v)) if k in _BOOL_ATTRS else str(v)])
    return out


def _bare_unit(raw: str | None) -> str | None:
    """`"1ps"` -> `"ps"`, `"1mA"` -> `"mA"`; strip a leading numeric magnitude."""
    if not raw:
        return None
    i = 0
    while i < len(raw) and (raw[i].isdigit() or raw[i] in ".eE+-"):
        i += 1
    return raw[i:].strip() or None


def _ndim(index_1: list[float], index_2: list[float], index_3: list[float]) -> int:
    if index_3:
        return 3
    if index_2:
        return 2
    if index_1:
        return 1
    return 0


def _t_peak(time: list[float], values: list[float]) -> float | None:
    """Time at which ``abs(value)`` is largest — the scalar shown in each CCS
    grid cell."""
    if not values:
        return None
    ipk = max(range(len(values)), key=lambda k: abs(values[k]))
    return time[ipk] if ipk < len(time) else None


def _reshape(values: list[float], n1: int, n2: int, n3: int, ndim: int) -> Any:
    """Reshape the parser's row-major flat ``values`` into nested lists.

    Layout is row-major over (index_1 x index_2 x index_3); see CLAUDE.md.
    """
    if ndim <= 1:
        return list(values)
    if ndim == 2:
        return [values[i * n2 : (i + 1) * n2] for i in range(n1)]
    plane = n2 * n3
    return [
        [values[i * plane + j * n3 : i * plane + (j + 1) * n3] for j in range(n2)]
        for i in range(n1)
    ]


_SI_UNIT = {"f": 1e-15, "p": 1e-12, "n": 1e-9, "u": 1e-6, "µ": 1e-6, "m": 1e-3, "k": 1e3, "M": 1e6}


def _si_scale(unit: str | None) -> float | None:
    """'1mA' -> 1e-3, '1V' -> 1.0. None/unparseable -> None."""
    if not unit:
        return None
    s = unit.strip()
    j = 0
    while j < len(s) and (s[j].isdigit() or s[j] in ".eE+-"):
        j += 1
    mag = float(s[:j]) if j else 1.0
    rest = s[j:]
    prefix = _SI_UNIT.get(rest[0], 1.0) if len(rest) >= 2 else 1.0
    return mag * prefix


def _split_common_label(names: list[str]) -> tuple[str, list[str]]:
    """Split table names into a shared base (common leading/trailing `_` tokens)
    and the per-name distinguishing middle. `["rise_power", "fall_power"]` ->
    `("power", ["rise", "fall"])`."""
    toks = [n.split("_") for n in names]
    pre: list[str] = []
    for col in zip(*toks):
        if len(set(col)) == 1:
            pre.append(col[0])
        else:
            break
    suf: list[str] = []
    for col in zip(*[t[::-1] for t in toks]):
        if len(set(col)) == 1:
            suf.append(col[0])
        else:
            break
    suf.reverse()
    p, s = len(pre), len(suf)
    base = "_".join(pre + suf) or "value"
    legends = []
    for t in toks:
        mid = t[p: len(t) - s] if p + s < len(t) else t
        legends.append("_".join(mid))
    return base, legends


@dataclass
class LibertyData:
    path: str
    doc: lt.LibraryIndex
    templates: dict[str, list[str | None]] = field(default_factory=dict)
    unit_by_kind: dict[str, str | None] = field(default_factory=dict)
    _vmap: dict[str, float] | None = None

    @classmethod
    def load(cls, path: str) -> "LibertyData":
        # LibraryIndex parses only the header up front and indexes cell byte
        # ranges; cells are parsed on demand (one at a time) when the tree/table
        # endpoints ask for them. This keeps open time ~seconds on multi-GB libs
        # instead of a full parse into RAM.
        doc = lt.LibraryIndex.open(path)
        energy = doc.energy_unit_joules()
        energy_unit = None
        if energy:
            energy_unit = _SI_PREFIX.get(round(math.log10(energy)), "") + "J"
        unit_by_kind = {
            "time": _bare_unit(doc.time_unit),
            "cap": doc.capacitive_load_unit,
            "current": _bare_unit(doc.current_unit),
            "voltage": _bare_unit(doc.voltage_unit),
            "energy": energy_unit,
        }
        return cls(path=path, doc=doc, templates=doc.templates(), unit_by_kind=unit_by_kind)

    def _with_unit(self, name: str, kind: str | None) -> str:
        unit = self.unit_by_kind.get(kind) if kind else None
        return f"{name} ({unit})" if unit else name

    def _axis_labels(self, template: str | None, table: str) -> dict[str, str]:
        """Per-axis ("<variable> (<unit>)") and value labels for a table."""
        vars_ = self.templates.get(template or "", [None, None, None])
        labels = {}
        for k, axis in enumerate(("index_1", "index_2", "index_3")):
            var = vars_[k] if k < len(vars_) else None
            labels[axis] = (
                self._with_unit(_ALIAS.get(var, var), _VAR_KIND.get(var or "")) if var else axis
            )
        labels["value"] = self._with_unit(table, _VALUE_KIND.get(table))
        return labels

    # -- library-level metadata ------------------------------------------------
    def meta(self) -> dict[str, Any]:
        return {
            "library_name": self.doc.library_name,
            "path": self.path,
            "voltage_unit": self.doc.voltage_unit,
            "current_unit": self.doc.current_unit,
            "time_unit": self.doc.time_unit,
            "energy_unit_joules": self.doc.energy_unit_joules(),
            "leakage_power_unit": next(
                (v for k, v in self.doc.attributes() if k == "leakage_power_unit"), None
            ),
            "templates": self.templates,  # name -> [var_1, var_2, var_3]
            "num_cells": self.doc.num_cells(),
            "attributes": [[k, str(v)] for k, v in self.doc.attributes()],
            "driver_waveforms": self._driver_waveforms(),
        }

    def _driver_waveforms(self) -> list[dict[str, Any]]:
        """``normalized_driver_waveform`` tables for the library view.

        Each is a 2-D table: index_1 = input slew, index_2 = normalized voltage
        (0..1), values = time. Reshaped to ``time[slew][voltage]`` so the client
        can draw one voltage-vs-time curve per slew.
        """
        t = self.unit_by_kind.get("time")
        tu = f" ({t})" if t else ""
        out = []
        for i, w in enumerate(self.doc.driver_waveforms()):
            slew = list(w.index_1)
            volt = list(w.index_2)
            out.append(
                {
                    "name": w.name or f"waveform {i + 1}",
                    "slew": slew,
                    "voltage": volt,
                    "time": _reshape(list(w.values), len(slew), len(volt), 0, 2),
                    "labels": {
                        "slew": f"slew{tu}",
                        "voltage": "normalized voltage",
                        "time": f"time{tu}",
                    },
                }
            )
        return out

    def cell_names(
        self, filter_: str | None = None, offset: int = 0, limit: int = 500
    ) -> dict[str, Any]:
        names = self.doc.cell_names()
        if filter_:
            f = filter_.lower()
            names = [n for n in names if f in n.lower()]
        return {"total": len(names), "cells": names[offset : offset + limit]}

    # -- raw cell source (byte-slice from the in-memory buffer) ----------------
    def cell_source(self, cell_name: str) -> dict[str, Any]:
        """Raw Liberty text of one cell. Capped so the largest CCS cells can't
        bog down the browser; cost is O(cell size), not file size."""
        text = self.doc.cell_source(cell_name)
        limit = 1_000_000
        return {
            "cell": cell_name,
            "text": text[:limit],
            "length": len(text),
            "truncated": len(text) > limit,
        }

    # -- per-cell tree (metadata only, no values) ------------------------------
    def cell_tree(self, cell_name: str) -> dict[str, Any]:
        cell = self.doc.cell(cell_name)
        node: dict[str, Any] = {
            "id": f"cell:{cell_name}",
            "label": cell_name,
            "type": "cell",
            "meta": {"area": cell.area},
            "attributes": _fmt_attr_rows(cell.attributes()),
            "pg_pins": self._pg_pin_table(cell),
            "seq": self._seq_spec(cell),
            "src": _path_src([], f"{cell_name} · cell"),
            "children": [],
        }
        for pin_name in cell.pins():
            node["children"].append(self._pin_node(cell.pin(pin_name), container=""))
        for bus_name in cell.buses():
            bus = cell.bus(bus_name)
            bnode: dict[str, Any] = {
                "id": f"bus:{bus_name}",
                "label": f"{bus_name} (bus)",
                "type": "bus",
                "meta": {"direction": bus.direction, "bus_type": bus.bus_type},
                "attributes": _meta_attrs({"direction": bus.direction, "bus_type": bus.bus_type}),
                "src": _path_src([_step("bus", name=bus_name)], f"{bus_name} · bus"),
                "children": self._arc_nodes(list(bus.timing_arcs()), "bus", bus_name, ""),
            }
            for pin_name in bus.pins():
                bnode["children"].append(self._pin_node(bus.pin(pin_name), container=f"bus:{bus_name}"))
            node["children"].append(bnode)
        for bundle_name in cell.bundles():
            bundle = cell.bundle(bundle_name)
            unode: dict[str, Any] = {
                "id": f"bundle:{bundle_name}",
                "label": f"{bundle_name} (bundle)",
                "type": "bundle",
                "meta": {"direction": bundle.direction, "members": bundle.members},
                "attributes": _meta_attrs(
                    {"direction": bundle.direction, "members": ", ".join(bundle.members)}
                ),
                "src": _path_src([_step("bundle", name=bundle_name)], f"{bundle_name} · bundle"),
                "children": self._arc_nodes(list(bundle.timing_arcs()), "bundle", bundle_name, ""),
            }
            for pin_name in bundle.pins():
                unode["children"].append(self._pin_node(bundle.pin(pin_name), container=f"bundle:{bundle_name}"))
            node["children"].append(unode)
        leak = self._leakage_node(cell, cell_name)
        if leak:
            node["children"].append(leak)
        node["children"].extend(self._ccsp_nodes(cell))
        # Each node carries the source scope (cell/pin/bus/bundle group) to show
        # in the bottom pane; descendants inherit their nearest named ancestor.
        self._collapse_line_groups(node, cell_name)
        _propagate_src(node, node["src"])
        return node

    def _collapse_line_groups(self, node: dict[str, Any], cell_name: str) -> None:
        """Collapse a branch whose children are all 1-D `power` line tables into a
        single multi-curve leaf (e.g. `pg=VDD` -> rise_power + fall_power on one
        plot, legend `rise`/`fall`). The children are removed; the branch becomes
        the leaf. `timing` group is skipped (its tables are 2-D/CCS and the probe
        would clone large CCS data)."""
        kids = node.get("children")
        if not kids:
            return
        for k in kids:
            self._collapse_line_groups(k, cell_name)
        if len(kids) < 2 or not all(
            k.get("type") == "table" and (k.get("ref") or {}).get("group") == "power"
            for k in kids
        ):
            return
        try:
            payloads = [
                self.table(
                    cell_name, r["pin"], r["group"], r["arc_index"], r["table"], r.get("container", "")
                )
                for r in (k["ref"] for k in kids)
            ]
        except Exception:
            return
        if not all(p.get("ndim") == 1 for p in payloads):
            return
        base, legends = _split_common_label([k["ref"]["table"] for k in kids])
        m = re.search(r"\(([^)]*)\)", (payloads[0].get("labels") or {}).get("value", ""))
        node["multi"] = [{"ref": k["ref"], "name": lg} for k, lg in zip(kids, legends)]
        node["ylabel"] = base + (f" ({m.group(1)})" if m else "")
        node["leaf"] = True
        node.pop("children", None)

    @staticmethod
    def _lit(expr: str | None) -> dict[str, Any] | None:
        """A single-literal boolean (`CLK`, `!SETN`, `D'`) -> {name, inv}. Complex
        expressions are returned as-is with inv=False."""
        if not expr:
            return None
        e = expr.strip()
        inv = False
        if e.startswith("!") or e.startswith("~"):
            inv, e = True, e[1:].strip()
        elif e.endswith("'"):
            inv, e = True, e[:-1].strip()
        while e.startswith("(") and e.endswith(")"):
            e = e[1:-1].strip()
        return {"name": e, "inv": inv}

    def _seq_spec(self, cell: lt.Cell) -> dict[str, Any] | None:
        """Sequential-cell symbol spec from the `ff`/`latch` group: clock/enable,
        D, Q/QN output pins, async set/clear, and scan (SE/SI) detection."""
        ff, la = cell.ff(), cell.latch()
        g = ff or la
        if g is None:
            return None
        pins = list(cell.pins())
        obj = {p: cell.pin(p) for p in pins}
        ins = [p for p in pins if (obj[p].direction or "") == "input"]
        outs = [(p, obj[p].function) for p in pins if (obj[p].direction or "").startswith("output")]
        v1, v2 = g.variable1, g.variable2
        qpin = next((p for p, f in outs if f == v1), None)
        qnpin = next((p for p, f in outs if f == v2), None)
        # Fallback: assign any unmatched outputs in order to Q then QN.
        rest = [p for p, _ in outs if p not in (qpin, qnpin)]
        if qpin is None and rest:
            qpin = rest.pop(0)
        if qnpin is None and rest:
            qnpin = rest.pop(0)
        spec: dict[str, Any] = {
            "kind": "ff" if ff else "latch",
            "d": "D" if "D" in ins else ((la.data_in if la else None) or "D"),
            "q": qpin,
            "qn": qnpin,
            "set": self._lit(g.preset),
            "clr": self._lit(g.clear),
            "scan": {"si": "SI", "se": "SE"} if ("SE" in ins and "SI" in ins) else None,
        }
        spec["clock" if ff else "enable"] = self._lit(
            ff.clocked_on if ff else (la.enable if la else None)
        )
        return spec

    def _pg_pin_table(self, cell: lt.Cell) -> dict[str, Any] | None:
        """The cell's `pg_pin` groups as a table: rows are pg pins, columns the
        ordered union of their attributes (pg_type, voltage_name, …)."""
        pgs = cell.pg_pins()
        if not pgs:
            return None
        cols: list[str] = []
        rows = []
        for pg in pgs:
            attrs = dict(pg.attributes())
            for k in attrs:
                if k not in cols:
                    cols.append(k)
            rows.append({"name": pg.name, "values": attrs})
        return {"cols": cols, "rows": rows}

    def _leakage_node(self, cell: lt.Cell, cell_name: str) -> dict[str, Any] | None:
        """`leakage_power` as a single when-condition × power-rail table node.

        Rows are when-conditions (canonical form; the no-`when` default is
        `(default)`); columns are the `related_pg_pin` rails (or one `value`
        column when no rail is given). Cells are the leakage values."""
        lps = cell.leakage_powers()
        if not lps:
            return None
        has_pg = any(lp.related_pg_pin for lp in lps)
        cols = (
            sorted({lp.related_pg_pin for lp in lps if lp.related_pg_pin})
            if has_pg
            else ["value"]
        )
        rows: dict[str, dict[str, float | None]] = {}
        order: list[str] = []
        for lp in lps:
            key = _fmt_bool(lp.when) if lp.when else "(default)"
            if key not in rows:
                rows[key] = {}
                order.append(key)
            rows[key][(lp.related_pg_pin or "value") if has_pg else "value"] = lp.value
        return {
            "id": f"cell:{cell_name}|leakage",
            "label": "leakage_power",
            "type": "leakage",
            "leakage": {
                "pg_pins": cols,
                "rows": [{"when": k, "values": rows[k]} for k in order],
            },
            "src": _path_src(
                [_step("leakage_power", indices=list(range(len(lps))))], "leakage_power"
            ),
            "children": [],
        }

    def _ccsp_nodes(self, cell: lt.Cell) -> list[dict[str, Any]]:
        """CCS-power (`dynamic_current`) subtree: dynamic_current -> switching_group
        -> pg_current leaf (a slew x cap grid of current-vs-time waves)."""
        out: list[dict[str, Any]] = []
        for dci, dc in enumerate(cell.dynamic_currents()):
            io = " -> ".join(x for x in (dc.related_inputs, dc.related_outputs) if x)
            dnode: dict[str, Any] = {
                "id": f"ccsp:{dci}",
                "label": f"dynamic_current (CCSP){' [' + io + ']' if io else ''}",
                "type": "dynamic",
                "attributes": _meta_attrs(
                    {
                        "related_inputs": dc.related_inputs,
                        "related_outputs": dc.related_outputs,
                        "when": dc.when,
                    }
                ),
                "src": _path_src(
                    [_step("dynamic_current", indices=[dci])], "dynamic_current"
                ),
                "children": [],
            }
            for sgi, sg in enumerate(dc.switching_groups()):
                ic, oc = sg.input_switching_condition, sg.output_switching_condition
                snode: dict[str, Any] = {
                    "id": f"ccsp:{dci}:{sgi}",
                    "label": f"in {_fmt_bool(ic) or '?'} / out {_fmt_bool(oc) or '?'}",
                    "type": "switchgroup",
                    "attributes": _meta_attrs(
                        {
                            "input_switching_condition": ic,
                            "output_switching_condition": oc,
                        }
                    ),
                    "children": [],
                }
                for pgi, pg in enumerate(sg.pg_currents()):
                    pin = pg.pg_pin or f"pg{pgi}"
                    leaf = self._table_leaf("ccsp", pin, f"{dci}:{sgi}", pgi, pin)
                    leaf["label"] = f"pg_current {pin}"
                    snode["children"].append(leaf)
                dnode["children"].append(snode)
            out.append(dnode)
        return out

    def _wrap_when(
        self, when: str | None, node_id: str, children: list[dict[str, Any]]
    ) -> list[dict[str, Any]]:
        """If a ``when`` condition exists, nest ``children`` under an extra
        ``when`` tree level; otherwise return them unchanged."""
        if not when:
            return children
        return [
            {
                "id": f"{node_id}|when",
                "label": f"when: {_fmt_bool(when)}",
                "type": "when",
                "attributes": [["when", when]],
                "children": children,
            }
        ]

    def _pin_node(self, pin: lt.Pin, container: str) -> dict[str, Any]:
        children: list[dict[str, Any]] = []
        children.extend(self._arc_nodes(list(pin.timing_arcs()), "pin", pin.name, container))
        children.extend(self._power_nodes(pin, container))
        return {
            "id": f"{container}|pin:{pin.name}",
            "label": f"{pin.name}",
            "type": "pin",
            "meta": {"direction": pin.direction, "function": _fmt_bool(pin.function)},
            "attributes": _fmt_attr_rows(pin.attributes()),
            "src": _path_src([_step("pin", name=pin.name)], f"{pin.name} · pin"),
            "children": children,
        }

    def _power_nodes(self, pin: lt.Pin, container: str) -> list[dict[str, Any]]:
        """Build `internal_power` tree nodes, grouped by `related_pin`. Within a
        related_pin, groups differing only by `related_pg_pin` collapse under a
        pg-pin sub-level. When a related_pin has more than one `when` (e.g. a
        conditional group plus its unconditional **default arc**), each `when`
        becomes a sub-level — the unconditional one shown as `when: [default]`,
        the fallback used when side-inputs don't match an exact condition."""
        by_pin: dict[str, list[tuple[int, lt.InternalPower]]] = {}
        order: list[str] = []
        for i, grp in enumerate(pin.internal_power()):
            rp = grp.related_pin or ""
            if rp not in by_pin:
                by_pin[rp] = []
                order.append(rp)
            by_pin[rp].append((i, grp))
        out = []
        for rp in order:
            members = by_pin[rp]
            if len({g.when or "" for _, g in members}) > 1:
                out.append(self._power_default_node(pin, container, rp, members))
            else:
                out.append(self._power_group_node(pin, container, members))
        return out

    def _power_pg_node(
        self, pin: lt.Pin, container: str, idx: int, grp: lt.InternalPower
    ) -> dict[str, Any]:
        """One `internal_power` group as a `pg=<rail>` node holding its tables."""
        prefix = [_step("pin", name=pin.name), _step("internal_power", indices=[idx])]
        pg = grp.related_pg_pin or f"pg{idx}"
        return {
            "id": f"{container}|pin:{pin.name}|power:{idx}",
            "label": f"pg={pg}",
            "type": "pgpin",
            "meta": {"related_pg_pin": grp.related_pg_pin},
            "attributes": _meta_attrs({"related_pg_pin": grp.related_pg_pin}),
            "src": _path_src(prefix, f"{pin.name} · internal_power"),
            "children": [
                self._table_leaf(container, pin.name, "power", idx, t, prefix)
                for t in grp.tables()
            ],
        }

    def _power_default_node(
        self,
        pin: lt.Pin,
        container: str,
        related_pin: str,
        members: list[tuple[int, lt.InternalPower]],
    ) -> dict[str, Any]:
        """One related_pin with several `when` variants: a `when:` sub-level per
        condition, the unconditional fallback as `when: [default]`."""
        by_when: dict[str, list[tuple[int, lt.InternalPower]]] = {}
        worder: list[str] = []
        for i, grp in members:
            w = grp.when or ""
            if w not in by_when:
                by_when[w] = []
                worder.append(w)
            by_when[w].append((i, grp))
        gid = f"{container}|pin:{pin.name}|powerpin:{related_pin}"
        when_children = [
            {
                "id": f"{gid}|when:{w}",
                "label": f"when: {_fmt_bool(w)}" if w else "when: [default]",
                "type": "when",
                "attributes": _meta_attrs({"when": w or None, "related_pin": related_pin}),
                "children": [self._power_pg_node(pin, container, i, g) for i, g in by_when[w]],
            }
            for w in worder
        ]
        arrow = f"{related_pin}→{pin.name}" if related_pin else pin.name
        meta = {"related_pin": related_pin}
        return {
            "id": gid,
            "label": f"internal_power {arrow}",
            "type": "powergrp",
            "meta": meta,
            "attributes": _meta_attrs(meta),
            "src": _path_src(
                [_step("pin", name=pin.name), _step("internal_power", indices=[i for i, _ in members])],
                f"{pin.name} · internal_power",
            ),
            "children": when_children,
        }

    def _power_group_node(
        self,
        pin: lt.Pin,
        container: str,
        members: list[tuple[int, lt.InternalPower]],
    ) -> dict[str, Any]:
        first = members[0][1]
        related_pin, when = first.related_pin, first.when
        pin_step = _step("pin", name=pin.name)

        def tables_for(idx: int, grp: lt.InternalPower) -> list[dict[str, Any]]:
            prefix = [pin_step, _step("internal_power", indices=[idx])]
            return [
                self._table_leaf(container, pin.name, "power", idx, t, prefix)
                for t in grp.tables()
            ]

        # Lone group: flat node with the pg pin in the label (unchanged layout).
        if len(members) == 1:
            idx, grp = members[0]
            arrow = f"{grp.related_pin}→{pin.name}" if grp.related_pin else pin.name
            label = f"internal_power {arrow}"
            if grp.related_pg_pin:
                label += f" pg={grp.related_pg_pin}"
            pid = f"{container}|pin:{pin.name}|power:{idx}"
            meta = {
                "related_pin": grp.related_pin,
                "related_pg_pin": grp.related_pg_pin,
                "when": grp.when,
            }
            return {
                "id": pid,
                "label": label,
                "type": "powergrp",
                "meta": meta,
                "attributes": _meta_attrs(meta),
                "src": _path_src(
                    [pin_step, _step("internal_power", indices=[idx])],
                    f"{pin.name} · internal_power",
                ),
                "children": self._wrap_when(grp.when, pid, tables_for(idx, grp)),
            }

        # Several groups differing only by pg pin: one parent, a pg-pin level
        # (nested under the `when` level when a condition is present).
        gid = f"{container}|pin:{pin.name}|powergrp:{related_pin or ''}:{when or ''}"
        pg_nodes = [self._power_pg_node(pin, container, idx, grp) for idx, grp in members]
        arrow = f"{related_pin}→{pin.name}" if related_pin else pin.name
        label = f"internal_power {arrow}"
        meta = {"related_pin": related_pin, "when": when}
        return {
            "id": gid,
            "label": label,
            "type": "powergrp",
            "meta": meta,
            "attributes": _meta_attrs(meta),
            "src": _path_src(
                [pin_step, _step("internal_power", indices=[i for i, _ in members])],
                f"{pin.name} · internal_power",
            ),
            "children": self._wrap_when(when, gid, pg_nodes),
        }

    def _arc_nodes(
        self, arcs: list[lt.TimingArc], scope: str, owner: str, container: str
    ) -> list[dict[str, Any]]:
        """Build timing-arc tree nodes, collapsing arcs that share the same
        identity (``related_pin`` + ``timing_type``) under one node with each
        ``when`` as a sub-level. Arcs that differ only by ``when`` are the same
        physical arc split by state, so they read better grouped together."""
        groups: dict[tuple[str, str], list[tuple[int, lt.TimingArc]]] = {}
        order: list[tuple[str, str]] = []
        for i, arc in enumerate(arcs):
            key = (arc.related_pin or "", arc.timing_type or "")
            if key not in groups:
                groups[key] = []
                order.append(key)
            groups[key].append((i, arc))
        return [self._arc_group_node(groups[key], scope, owner, container) for key in order]

    def _arc_group_node(
        self,
        members: list[tuple[int, lt.TimingArc]],
        scope: str,
        owner: str,
        container: str,
    ) -> dict[str, Any]:
        first = members[0][1]
        # `timing <related>→<owner> <type>`, e.g. `timing a→y combinational`.
        arrow = f"{first.related_pin}→{owner}" if first.related_pin else owner
        parts = [arrow]
        if first.timing_type:
            parts.append(first.timing_type)
        label = "timing " + " ".join(parts)
        # For bus/bundle direct arcs, the "pin" used by /api/table is the owner name.
        base = container if scope == "pin" else f"{scope}:{owner}"
        owner_step = _step(scope, name=owner)

        def tables_for(idx: int, arc: lt.TimingArc) -> list[dict[str, Any]]:
            prefix = [owner_step, _step("timing", indices=[idx])]
            out = [self._table_leaf(base, owner, "timing", idx, t, prefix) for t in arc.tables()]
            stage_counts: dict[str, int] = {}
            for stage_idx, stage in enumerate(arc.ccsn_stages()):
                stage_occurrence = stage_counts.get(stage.name, 0)
                stage_counts[stage.name] = stage_occurrence + 1
                stage_prefix = prefix + [_step(stage.name, indices=[stage_occurrence])]
                children: list[dict[str, Any]] = []
                if stage.dc_current() is not None:
                    leaf = self._table_leaf(base, owner, "ccsn", idx, f"{stage_idx}:dc_current")
                    leaf["label"] = "dc_current"
                    leaf["src"] = _path_src(
                        stage_prefix + [_step("dc_current", indices=[0])], "dc_current · table"
                    )
                    children.append(leaf)
                for table in ("output_voltage_rise", "output_voltage_fall"):
                    vectors = getattr(stage, table)()
                    if vectors:
                        leaf = self._table_leaf(base, owner, "ccsn", idx, f"{stage_idx}:{table}")
                        leaf["label"] = table
                        leaf["src"] = _path_src(
                            stage_prefix + [_step(table, indices=[0])], f"{table} · table"
                        )
                        children.append(leaf)
                meta = {
                    "is_inverting": stage.is_inverting,
                    "is_needed": stage.is_needed,
                    "is_pass_gate": stage.is_pass_gate,
                    "stage_type": stage.stage_type,
                    "miller_cap_rise": stage.miller_cap_rise,
                    "miller_cap_fall": stage.miller_cap_fall,
                    "when": stage.when,
                }
                out.append(
                    {
                        "id": f"{base}|pin:{owner}|ccsn:{idx}:{stage_idx}",
                        "label": stage.name,
                        "type": "ccsn",
                        "attributes": _meta_attrs(meta),
                        "src": _path_src(stage_prefix, f"{stage.name} · CCSN"),
                        "children": children,
                    }
                )
            return out

        # Lone unconditional arc: tables hang straight off the arc node.
        if len(members) == 1 and not first.when:
            idx, arc = members[0]
            arc_id = f"{container}|{scope}:{owner}|timing:{idx}"
            return {
                "id": arc_id,
                "label": label,
                "type": "arc",
                "meta": {"related_pin": first.related_pin, "timing_type": first.timing_type},
                "attributes": _meta_attrs(
                    {
                        "related_pin": first.related_pin,
                        "timing_type": first.timing_type,
                        "timing_sense": first.timing_sense,
                    }
                ),
                "src": _path_src([owner_step, _step("timing", indices=[idx])], f"{owner} · timing"),
                "children": tables_for(idx, arc),
            }

        # Otherwise one arc node, each member's `when` a sub-level beneath it.
        group_id = f"{container}|{scope}:{owner}|timinggrp:{first.related_pin or ''}:{first.timing_type or ''}"
        when_children = []
        for idx, arc in members:
            arc_id = f"{container}|{scope}:{owner}|timing:{idx}"
            when_children.append(
                {
                    "id": f"{arc_id}|when",
                    "label": f"when: {_fmt_bool(arc.when)}" if arc.when else "when: [default]",
                    "type": "when",
                    "attributes": _meta_attrs(
                        {
                            "when": arc.when,
                            "related_pin": arc.related_pin,
                            "timing_type": arc.timing_type,
                            "timing_sense": arc.timing_sense,
                        }
                    ),
                    "src": _path_src(
                        [owner_step, _step("timing", indices=[idx])], f"{owner} · timing"
                    ),
                    "children": tables_for(idx, arc),
                }
            )
        return {
            "id": group_id,
            "label": label,
            "type": "arc",
            "meta": {"related_pin": first.related_pin, "timing_type": first.timing_type},
            "attributes": _meta_attrs(
                {
                    "related_pin": first.related_pin,
                    "timing_type": first.timing_type,
                    "timing_sense": first.timing_sense,
                }
            ),
            "src": _path_src(
                [owner_step, _step("timing", indices=[i for i, _ in members])],
                f"{owner} · timing",
            ),
            "children": when_children,
        }

    def _table_leaf(
        self,
        container: str,
        pin: str,
        group: str,
        arc_index: int,
        table: str,
        src_prefix: list[dict[str, Any]] | None = None,
    ) -> dict[str, Any]:
        leaf: dict[str, Any] = {
            "id": f"{container}|pin:{pin}|{group}:{arc_index}|table:{table}",
            "label": table,
            "type": "table",
            "leaf": True,
            "ref": {
                "container": container,
                "pin": pin,
                "group": group,
                "arc_index": arc_index,
                "table": table,
            },
        }
        # `src_prefix` locates the enclosing timing()/internal_power() group; the
        # table itself is that group's `<table> (...)` block. None -> inherit.
        if src_prefix is not None:
            leaf["src"] = _path_src(src_prefix + [_step(table, indices=[0])], f"{table} · table")
        return leaf

    # -- resolve one table leaf to axes + values -------------------------------
    def table(
        self,
        cell: str,
        pin: str,
        group: str,
        arc_index: int,
        table: str,
        container: str = "",
    ) -> dict[str, Any]:
        cell_obj = self.doc.cell(cell)
        if container == "ccsp":
            # group encodes "<dynamic_current idx>:<switching_group idx>",
            # arc_index = pg_current idx, table/pin = pg pin name.
            dci, sgi = (int(x) for x in group.split(":"))
            pg = (
                cell_obj.dynamic_currents()[dci]
                .switching_groups()[sgi]
                .pg_currents()[arc_index]
            )
            return self._ccs_payload(table, pg.vectors())
        pin_obj = self._resolve_pin_owner(cell_obj, container, pin)
        if group == "timing":
            arcs = pin_obj.timing_arcs()
            tbl = arcs[arc_index].table(table)
        elif group == "ccsn":
            stage_idx_raw, table_name = table.split(":", 1)
            stage = pin_obj.timing_arcs()[arc_index].ccsn_stages()[int(stage_idx_raw)]
            if table_name == "dc_current":
                return self._dc_current_payload(cell_obj, stage)
            if table_name == "output_voltage_rise":
                return self._ccs_payload(table_name, stage.output_voltage_rise(), "voltage")
            if table_name == "output_voltage_fall":
                return self._ccs_payload(table_name, stage.output_voltage_fall(), "voltage")
            raise ValueError(f"unknown ccsn table {table_name!r}")
        elif group == "power":
            grps = pin_obj.internal_power()
            tbl = grps[arc_index].table(table)
        else:
            raise ValueError(f"unknown group {group!r}")

        vectors = tbl.vectors()
        if vectors:
            return self._ccs_payload(table, vectors)

        i1, i2, i3 = tbl.index_1, tbl.index_2, tbl.index_3
        ndim = _ndim(i1, i2, i3)
        n1, n2, n3 = len(i1) or 1, len(i2) or 1, len(i3) or 1
        return {
            "table": table,
            "kind": "table",
            "ndim": ndim,
            "index_1": i1,
            "index_2": i2,
            "index_3": i3,
            "values": _reshape(tbl.values, n1, n2, n3, ndim),
            "scalar": tbl.values[0] if ndim == 0 and tbl.values else None,
            "labels": self._axis_labels(tbl.template, table),
        }

    def _voltage_map(self) -> dict[str, float]:
        """Library-level ``voltage_map`` (rail name -> volts), parsed once."""
        if self._vmap is None:
            m: dict[str, float] = {}
            for key, val in self.doc.attributes():
                if key == "voltage_map":
                    parts = [p.strip() for p in val.split(",")]
                    if len(parts) == 2:
                        try:
                            m[parts[0]] = float(parts[1])
                        except ValueError:
                            pass
            self._vmap = m
        return self._vmap

    def _rail_window(self, cell_obj: lt.Cell) -> tuple[float, float] | tuple[None, None]:
        """``(Vss, Vdd)`` from the cell's primary_ground/primary_power pg_pins via
        ``voltage_map``; ``(None, None)`` if the rails can't be resolved."""
        vmap = self._voltage_map()
        vdd = vss = None
        for pg in cell_obj.pg_pins():
            attrs = dict(pg.attributes())
            name = attrs.get("voltage_name")
            if name is None:
                continue
            if attrs.get("pg_type") == "primary_power":
                vdd = vmap.get(name)
            elif attrs.get("pg_type") == "primary_ground":
                vss = vmap.get(name)
        if vdd is None or vss is None:
            return (None, None)
        return (min(vss, vdd), max(vss, vdd))

    def _dc_current_payload(self, cell_obj: lt.Cell, stage: lt.CcsnStage) -> dict[str, Any]:
        """CCSN ``dc_current`` as a 2-D table with the input/output voltage axes
        clipped to the supply rails ``[Vss, Vdd]`` — the channel-connecting
        block's operating window, where pulling-strength resistance is meaningful.
        Outside the rails current flows into the power pin / out of ground."""
        tbl = stage.dc_current()
        if tbl is None:
            raise ValueError("ccsn stage has no dc_current table")
        i1, i2, vals = tbl.index_1, tbl.index_2, tbl.values
        n2 = len(i2)
        lo, hi = self._rail_window(cell_obj)
        keep = (lambda v: True) if lo is None else (lambda v: lo <= v <= hi)
        ri = [a for a, v in enumerate(i1) if keep(v)]
        cj = [b for b, v in enumerate(i2) if keep(v)]
        grid = [[vals[a * n2 + b] for b in cj] for a in ri]
        return {
            "table": "dc_current",
            "kind": "table",
            "ndim": 2,
            "index_1": [i1[a] for a in ri],
            "index_2": [i2[b] for b in cj],
            "index_3": [],
            "values": grid,
            "scalar": None,
            "labels": self._axis_labels(tbl.template, "dc_current"),
            # Supply rails (volts) for the viewer's rail-relative resistance and
            # the delta-v cutoff; null when the cell's rails can't be resolved.
            "rails": None if lo is None else {"vss": lo, "vdd": hi},
            # raw resistance (volts / current_unit) -> kilo-ohms
            "r_scale_kohm": ((_si_scale(self.doc.voltage_unit) or 1.0)
                             / (_si_scale(self.doc.current_unit) or 1.0)) / 1e3,
        }

    def _ccs_payload(
        self, table: str, vectors: list, value_label: str = "current"
    ) -> dict[str, Any]:
        """CCS group -> slew x cap grid of value-vs-time waves.

        Each `vector` is one point in (input slew, output cap) space holding a
        full value(time) wave. The grid cell summary is the 95%-decay time.
        """
        slews = sorted({v.index_1[0] for v in vectors if v.index_1})
        caps = sorted({v.index_2[0] for v in vectors if v.index_2})
        si = {s: i for i, s in enumerate(slews)}
        ci = {c: j for j, c in enumerate(caps)}
        grid: list[list[Any]] = [[None] * len(caps) for _ in slews]
        for v in vectors:
            if not (v.index_1 and v.index_2):
                continue
            time, current = v.index_3, v.values
            grid[si[v.index_1[0]]][ci[v.index_2[0]]] = {
                "time": time,
                "current": current,
                "reference_time": v.reference_time,
                "t_peak": _t_peak(time, current),
            }
        template = vectors[0].template if vectors else None
        vars_ = self.templates.get(template or "", [None, None, None])
        labels = {
            "index_1": self._with_unit(_ALIAS.get(vars_[0], vars_[0]), _VAR_KIND.get(vars_[0] or "")) if vars_[0] else "slew",
            "index_2": self._with_unit(_ALIAS.get(vars_[1], vars_[1]), _VAR_KIND.get(vars_[1] or "")) if len(vars_) > 1 and vars_[1] else "load",
            "time": self._with_unit(vars_[2] if len(vars_) > 2 and vars_[2] else "time", "time"),
            "current": self._with_unit(value_label, value_label),
        }
        return {
            "table": table,
            "kind": "ccs",
            "index_1": slews,
            "index_2": caps,
            "grid": grid,
            "labels": labels,
        }

    def _resolve_pin_owner(self, cell_obj: lt.Cell, container: str, pin: str):
        """Return the object whose timing_arcs/internal_power we read.

        ``container`` is ""/"bus:NAME"/"bundle:NAME". For a bus/bundle *direct*
        arc the leaf encodes container="bus:NAME" and pin=NAME, so we return the
        bus/bundle itself (it exposes timing_arcs)."""
        if not container:
            return cell_obj.pin(pin)
        kind, name = container.split(":", 1)
        owner = cell_obj.bus(name) if kind == "bus" else cell_obj.bundle(name)
        if pin == name:
            return owner  # direct bus/bundle arc
        return owner.pin(pin)
