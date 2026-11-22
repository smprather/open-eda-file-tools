"""Node regression tests for the Liberty viewer's source and plot handling."""

from pathlib import Path
import shutil
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[1]


def run_frontend(body: str) -> None:
    node = shutil.which("node")
    if node is None:
        pytest.skip("node is required for viewer frontend tests")
    script = r"""
    const fs = require("node:fs");
    const vm = require("node:vm");
    const assert = require("node:assert/strict");
    const source = fs.readFileSync("viewer/static/app.js", "utf8")
      .split('document.addEventListener("keydown", _treeKey);')[0];
    const context = { assert, console, setTimeout, URLSearchParams, window: {} };
    vm.runInNewContext(source + String.raw`
    (async () => {
    BODY
    })().catch((error) => {
      console.error(error);
      throw error;
    });`, context);
    """.replace("BODY", body)
    result = subprocess.run(
        [node, "-e", script], cwd=ROOT, text=True, capture_output=True, timeout=10
    )
    assert result.returncode == 0, result.stderr


def test_source_annotation_uses_innermost_closed_template():
    run_frontend(r"""
      const templates = {outer: ["slew", "cap"], inner: ["time"]};
      const input = [
        "output_current_rise (outer) {",
        '  index_2 ("1");',
        '  note : "escaped \\" and } {";',
        "  function : A';",
        "  /*",
        "  vector (inner) {",
        '    index_1 ("3");',
        "  }",
        "  */",
        "  vector (inner) {",
        '    index_1 ("4");',
        '    values ("6");',
        "  }",
        "  vector (outer) {",
        '    values ("7");',
        "  }",
        "  // }",
        "  vector (unrecognized) {",
        '    index_1 ("5");',
        "  }",
        "}",
        "vector (inner) {",
        '  index_1 ("8");',
      ];
      const expected = [...input];
      expected[1] += "  // cap";
      expected[10] += "  // time";
      expected[11] += "  // ------> time ----->";
      expected[14] += "  // ------> slew ----->";
      expected[18] += "  // slew";
      assert.equal(_annotateIndices(input.join("\n"), templates), expected.join("\n"));
    """)


def test_source_cache_reuses_recent_cells_and_evicts_by_text_size():
    run_frontend(r"""
      let requests = [];
      globalThis.fetch = async (path) => {
        requests.push(path);
        return {ok: true, json: async () => ({text: "x".repeat(1_000_000), length: 1_000_025, truncated: true})};
      };
      const data = await cellSource("one");
      assert.equal(data.text.length, 1_000_000);
      assert.equal(data.length, 1_000_025);
      assert.equal(await cellSource("one"), data);
      await cellSource("two");
      await cellSource("one");
      assert.equal(requests.length, 2);
      await cellSource("three");
      await cellSource("four");
      assert.equal(await cellSource("one"), data);
      await cellSource("two");
      assert.equal(requests.length, 5);
    """)


def test_plotly_is_lazy_shared_retryable_and_skips_obsolete_render():
    assert "/plotly.min.js" not in (ROOT / "viewer/static/index.html").read_text()
    run_frontend(r"""
      const scripts = [];
      const drawn = [];
      const target = {isConnected: true, textContent: ""};
      globalThis.document = {
        head: {appendChild(script) { scripts.push(script); }},
        createElement() { return {remove() { this.removed = true; }}; },
      };
      assert.equal(scripts.length, 0);
      const first = drawPlot("newPlot", target, [1]);
      const obsolete = drawPlot("newPlot", target, [2]);
      assert.equal(scripts.length, 1);
      scripts[0].onerror();
      await Promise.all([first, obsolete]);
      assert.equal(scripts[0].removed, true);
      assert.match(target.textContent, /Plotly/);
      const priorSelection = drawPlot("newPlot", target, [3]);
      selectionVersion++;
      const current = drawPlot("newPlot", target, [4]);
      const other = {isConnected: true};
      const detached = drawPlot("react", other, [5]);
      other.isConnected = false;
      assert.equal(scripts.length, 2);
      assert.equal(scripts[1].src, "/plotly.min.js");
      window.Plotly = {
        newPlot(element, traces) { drawn.push(traces[0]); },
        react() { assert.fail("obsolete chart drew into a detached view"); },
      };
      scripts[1].onload();
      await Promise.all([priorSelection, current, detached]);
      assert.equal(String(drawn), "4");
    """)


def test_obsolete_source_and_filter_responses_leave_current_view_intact():
    run_frontend(r"""
      const requests = new Map();
      globalThis.fetch = (path) => new Promise((resolve) => {
        requests.set(path, (data) => resolve({ok: true, json: async () => data}));
      });
      const fields = {
        "source-section": {classList: {remove() {}}},
        "source-title": {},
        source: {},
        "cell-count": {},
        tree: {replaceChildren() {}},
      };
      globalThis.document = {getElementById(id) { return fields[id]; }};

      const oldestFilter = loadCells("old");
      const latestFilter = loadCells("new");
      requests.get("/api/cells?filter=new")({cells: [], total: 1});
      await latestFilter;
      requests.get("/api/cells?filter=old")({cells: [], total: 17});
      await oldestFilter;
      assert.equal(fields["cell-count"].textContent, "0 / 1 cells");

      const oldestSource = showSource("old", {path: []});
      selectionVersion++;
      const latestSource = showSource("new", {path: []});
      requests.get("/api/cells/new/source")({text: "cell (new) { area : 2; }", length: 26});
      await latestSource;
      const current = fields.source.innerHTML;
      assert.match(current, /new/);
      requests.get("/api/cells/old/source")({text: "cell (old) { area : 1; }", length: 26});
      await oldestSource;
      assert.equal(fields.source.innerHTML, current);
    """)
