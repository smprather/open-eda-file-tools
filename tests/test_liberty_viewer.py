"""Regression tests for viewer index and tree reuse."""

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from threading import Barrier, Event

import pytest

from viewer import server
from viewer.data import LibertyData

DEV = Path(__file__).resolve().parents[1] / "dev.lib"


def test_concurrent_cold_viewer_requests_share_index(monkeypatch):
    monkeypatch.setattr(server, "_data", None)
    monkeypatch.setenv("LIBERTY_FILE", str(DEV))
    ready = Barrier(5)
    release = Event()
    data = LibertyData.load(str(DEV))
    calls = []

    def load(_path):
        calls.append(_path)
        assert release.wait(5)
        return data

    def request():
        ready.wait()
        return server.get_data()

    monkeypatch.setattr(LibertyData, "load", load)
    with ThreadPoolExecutor(4) as pool:
        requests = [pool.submit(request) for _ in range(4)]
        ready.wait()
        # Give each request time to enter the cold initialization path.
        release.wait(0.05)
        release.set()
        assert all(request.result() is data for request in requests)
    assert len(calls) == 1


def test_failed_viewer_open_can_retry(monkeypatch):
    monkeypatch.setattr(server, "_data", None)
    monkeypatch.setenv("LIBERTY_FILE", str(DEV))
    load = LibertyData.load

    def fail(_path):
        raise ValueError("unreadable")

    monkeypatch.setattr(LibertyData, "load", fail)
    with pytest.raises(ValueError, match="unreadable"):
        server.get_data()
    monkeypatch.setattr(LibertyData, "load", load)
    assert server.get_data().doc.num_cells() == 2


@pytest.mark.parametrize("name", ["DevBuffer", "DevFlipFlop"])
def test_cell_tree_parses_once_without_fetching_table_payload(monkeypatch, name):
    data = LibertyData.load(str(DEV))
    cell = data.doc.cell
    calls = []

    def counted(name):
        calls.append(name)
        return cell(name)

    def unexpected(*args):
        pytest.fail("tree requested a full table payload")

    monkeypatch.setattr(data.doc, "cell", counted)
    monkeypatch.setattr(data, "table", unexpected)
    tree = data.cell_tree(name)
    assert calls == [name]
    assert tree["id"] == f"cell:{name}"
    assert tree["children"]


def test_bus_power_tree_collapses_only_compatible_siblings(tmp_path):
    source = """library (t) {
      voltage_unit : "1V"; current_unit : "1mA"; time_unit : "1ps";
      cell (c) {
        bus (B) {
          pin (Y) {
            internal_power () {
              related_pin : A; when : A';
              rise_power (p) { index_1 ("1, 2"); values ("1, 2"); }
              fall_power (p) { index_1 ("1, 2"); values ("2, 3"); }
            }
            internal_power () {
              related_pin : A;
              rise_power (p) { index_1 ("1, 2"); values ("1, 2"); }
              fall_power (p) { index_1 ("1"); index_3 ("1, 2"); values ("2, 3"); }
            }
          }
        }
      }
    }"""
    path = tmp_path / "bus.lib"
    path.write_text(source)
    data = LibertyData.load(str(path))
    tree = data.cell_tree("c")
    bus = tree["children"][0]
    pin = bus["children"][0]
    condition, default = pin["children"][0]["children"]
    assert condition["label"] == "when: !A"
    assert default["label"] == "when: [default]"
    collapsed = condition["children"][0]
    assert collapsed["ylabel"] == "power (fJ)"
    assert [item["name"] for item in collapsed["multi"]] == ["rise", "fall"]
    assert all(item["ref"]["container"] == "bus:B" for item in collapsed["multi"])
    ref = collapsed["multi"][0]["ref"]
    assert data.table("c", **ref)["values"] == [1, 2]
    refs = default["children"][0]["children"]
    assert [node["ref"]["table"] for node in refs] == ["rise_power", "fall_power"]
    assert data.table("c", **refs[1]["ref"])["ndim"] == 3


def test_cell_source_cap_counts_characters(tmp_path):
    source = "cell (c) { /*" + "µ" * 1_000_025 + "*/ }"
    path = tmp_path / "source.lib"
    path.write_text("library (t) {" + source + "}", encoding="utf-8")
    data = LibertyData.load(str(path))
    assert data.cell_source("c") == {
        "cell": "c",
        "text": source[:1_000_000],
        "length": len(source),
        "truncated": True,
    }


def test_ready_warms_index_without_building_metadata(monkeypatch):
    data = LibertyData.load(str(DEV))
    monkeypatch.setattr(server, "_data", None)
    monkeypatch.setenv("LIBERTY_FILE", str(DEV))
    monkeypatch.setattr(LibertyData, "load", lambda _path: data)

    def unexpected():
        pytest.fail("readiness built metadata")

    monkeypatch.setattr(data, "meta", unexpected)
    assert server.ready().status_code == 204
    assert server.get_data() is data
    assert server.ready().body == b""
