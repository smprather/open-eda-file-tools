"""Tests for the first-class BooleanExpression object and the `*_expr`
accessors that surface parsed Liberty boolean fields.

`str(expr)` is a minimized sum-of-products; `==`/`hash` use the canonical
sum-of-minterms over the support, so equality means logical equivalence.
"""

from pathlib import Path

import pytest

import liberty_tools as lt

B = lt.BooleanExpression


# ---- canonical string + minimized str -------------------------------------
def test_minimized_str():
    assert str(B("A | B")) == "A + B"
    assert str(B("A & !B")) == "A*!B"
    assert str(B("(A & B) | (A & C) | (B & C)")) == "A*B + A*C + B*C"
    assert str(B("A & (B | !B)")) == "A"
    assert str(B("A | !A")) == "1"
    assert str(B("A & !A")) == "0"


def test_canonical_minterms():
    assert B("A | B").canonical() == "!A*B + A*!B + A*B"
    assert B("A | B").minterms() == ["!A*B", "A*!B", "A*B"]
    assert B("A & !A").minterms() == []
    assert B("A | !A").minterms() == []


def test_deterministic():
    # Repeated construction yields identical strings (no run-to-run wobble in
    # the minimizer's choice among equally-minimal covers).
    expr = "(A & B) | (B & C) | (A & C)"
    assert str(B(expr)) == str(B(expr))


# ---- logical-equivalence equality / hashing -------------------------------
def test_equality_is_logical_equivalence():
    assert B("A | B") == B("B | A")  # commutativity
    assert B("A & (B | !B)") == B("A")  # vacuous variable dropped
    assert B("!(A & B)") == B("!A | !B")  # De Morgan
    assert B("A ^ B") == B("A & !B | !A & B")  # XOR expansion
    assert B("A & B") != B("A | B")
    assert B("A") != B("B")


def test_hash_and_set():
    assert hash(B("A | B")) == hash(B("B | A"))
    assert len({B("A | B"), B("B | A"), B("B | A | A")}) == 1
    assert len({B("A & B"), B("A | B")}) == 2


def test_eq_with_foreign_type():
    assert (B("A") == "A") is False
    assert (B("A") != 5) is True


# ---- support / variables --------------------------------------------------
def test_variables_are_sorted_support():
    assert B("B | A").variables == ["A", "B"]
    assert B("A & (B | !B)").variables == ["A"]  # vacuous B eliminated
    assert B("1").variables == []


# ---- round-trip -----------------------------------------------------------
@pytest.mark.parametrize(
    "src",
    ["A | B", "A & !B", "A ^ B", "!(A & B)", "A & (B | !B)", "A | !A", "A & !A"],
)
def test_roundtrip_through_parser(src):
    expr = B(src)
    # The canonical and minimized strings both re-parse to the same function.
    assert B(expr.canonical()) == expr
    assert B(str(expr)) == expr


# ---- eval / implies -------------------------------------------------------
def test_eval():
    assert B("A & B").eval({"A": True, "B": True}) is True
    assert B("A & B").eval({"A": True, "B": False}) is False
    assert B("A ^ B").eval(A=True, B=False) is True
    assert B("A").eval({}) is False  # missing var defaults false
    assert B("A | B").eval({"A": False, "B": True, "Z": True}) is True  # extra ignored


def test_implies():
    assert B("A & B").implies(B("A")) is True
    assert B("A").implies(B("A & B")) is False
    assert B("A").implies(B("A | B")) is True


# ---- space / juxtaposition AND (Liberty Table 7-4) ------------------------
def test_space_is_and():
    assert B("A B") == B("A & B")
    assert B("A B C") == B("A & B & C")
    assert B("A B + C") == B("(A & B) | C")  # implicit AND binds tighter than OR
    assert B("A' B") == B("!A & B")  # suffix-NOT then juxtaposition


# ---- repr -----------------------------------------------------------------
def test_repr():
    assert repr(B("A & B")) == "BooleanExpression('A & B')"


# ---- caps / parse errors --------------------------------------------------
def test_variable_cap():
    too_many = " | ".join(f"V{i:02d}" for i in range(13))  # 13 > cap of 12
    with pytest.raises(ValueError):
        B(too_many)


@pytest.mark.parametrize("bad", ["A &", "(A", "A |", "!"])
def test_parse_errors(bad):
    with pytest.raises(ValueError):
        B(bad)


# ---- accessor wiring on an inline fixture ---------------------------------
SEQ_FIXTURE = """
library (seq_lib) {
  cell (DFF) {
    ff (IQ, IQN) {
      next_state : "D";
      clocked_on : "CP";
      clear : "RN'";
      preset : "SN'";
    }
    leakage_power () {
      when : "D & CP";
      related_pg_pin : VDD;
      value : 12.5;
    }
    leakage_power () {
      related_pg_pin : VSS;
      value : 0;
    }
    pin (D) {
      direction : input;
    }
    pin (CP) {
      direction : input;
    }
    pin (Q) {
      direction : output;
      function : "IQ";
      timing () {
        related_pin : "CP";
        timing_type : rising_edge;
        when : "D & CP";
        sdf_cond : "D";
      }
    }
    pin (TRI) {
      direction : output;
      function : "IQ";
      three_state : "!OE";
    }
  }
  cell (LAT) {
    latch (IQ, IQN) {
      enable : "G";
      data_in : "D";
    }
    pin (Q) {
      direction : output;
      function : "IQ";
    }
  }
}
"""


@pytest.fixture
def seq_doc(tmp_path):
    path = tmp_path / "seq.lib"
    path.write_text(SEQ_FIXTURE)
    return lt.parse_file(str(path))


def test_function_expr(seq_doc):
    q = seq_doc.cell("DFF").pin("Q")
    assert q.function == "IQ"
    assert q.function_expr() == B("IQ")
    # A pin without a function returns None.
    assert seq_doc.cell("DFF").pin("D").function_expr() is None


def test_three_state_expr(seq_doc):
    tri = seq_doc.cell("DFF").pin("TRI")
    assert tri.three_state == "!OE"
    assert tri.three_state_expr() == B("!OE")
    assert seq_doc.cell("DFF").pin("Q").three_state_expr() is None


def test_timing_when_and_sdf_cond_expr(seq_doc):
    arc = seq_doc.cell("DFF").pin("Q").timing_arcs()[0]
    assert arc.when_expr() == B("D & CP")
    assert arc.sdf_cond == "D"
    assert arc.sdf_cond_expr() == B("D")


def test_ff_accessors(seq_doc):
    ff = seq_doc.cell("DFF").ff()
    assert ff is not None
    assert (ff.variable1, ff.variable2) == ("IQ", "IQN")
    assert ff.next_state_expr() == B("D")
    assert ff.clocked_on_expr() == B("CP")
    assert ff.clear_expr() == B("!RN")  # apostrophe NOT
    assert ff.preset_expr() == B("!SN")
    assert seq_doc.cell("LAT").ff() is None


def test_leakage_power(seq_doc):
    lps = seq_doc.cell("DFF").leakage_powers()
    assert len(lps) == 2
    gated = next(lp for lp in lps if lp.when)
    assert gated.value == 12.5
    assert gated.related_pg_pin == "VDD"
    assert gated.when_expr() == B("D & CP")
    default = next(lp for lp in lps if lp.when is None)
    assert default.value == 0.0 and default.related_pg_pin == "VSS"
    assert seq_doc.cell("LAT").leakage_powers() == []


def test_latch_accessors(seq_doc):
    latch = seq_doc.cell("LAT").latch()
    assert latch is not None
    assert (latch.variable1, latch.variable2) == ("IQ", "IQN")
    assert latch.enable_expr() == B("G")
    assert latch.data_in_expr() == B("D")
    assert seq_doc.cell("DFF").latch() is None


# ---- real ASAP7 lib (skip if absent) --------------------------------------
@pytest.mark.skipif(not Path("dev.lib").exists(), reason="dev.lib not present")
def test_dev_lib_accessors():
    idx = lt.LibraryIndex.open("dev.lib")

    y = idx.cell("BUFx2_ASAP7_6t_R").pin("Y")
    function_expr = y.function_expr()
    assert function_expr is not None
    assert str(function_expr) == "A"
    assert function_expr.variables == ["A"]

    dff = idx.cell("DFFHQNx1_ASAP7_6t_R")
    ff = dff.ff()
    assert ff is not None
    assert ff.clocked_on_expr() == B("CLK")
    # power_down_function uses bracketed PG-pin names; ensure `[ ]`-free names
    # round-trip and minimize.
    pdf = dff.pin("QN").power_down_function_expr()
    assert pdf == B("(!VDD) + (VSS)")
