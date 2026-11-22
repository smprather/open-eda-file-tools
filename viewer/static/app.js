"use strict";

const PLOT_LAYOUT = {
  paper_bgcolor: "#26262b",
  plot_bgcolor: "#1e1e22",
  font: { color: "#d6d6dc", family: "ui-monospace, monospace", size: 11 },
  margin: { l: 55, r: 16, t: 18, b: 38 },
};
// Default 2D plot height — smaller than Plotly's 450 to cut vertical dead space.
const PLOT_H = 320;
// White grid/lines for the 3D scene axes.
const GRID_WHITE = { gridcolor: "#ffffff", zerolinecolor: "#ffffff", linecolor: "#ffffff" };

// Liberty groups the viewer actually RENDERS (tree node or table). The source
// pane strips only these out of a parent's text — they're shown elsewhere — so
// each source view shows only the data rendered for that selection. Everything
// the viewer does NOT display (ff/latch/statetable, pg_pin's own attrs, unknown
// groups, plain attributes) is left in, since the source is the only place to
// see it. pg_pin (shown in the cell table) and vector/switching_group/pg_current
// (part of a CCS table's own display) are intentionally NOT stripped.
const NAV_GROUPS = new Set([
  "pin", "bus", "bundle", "dynamic_current", "leakage_power",
  "timing", "internal_power",
  "cell_rise", "cell_fall", "rise_transition", "fall_transition",
  "rise_constraint", "fall_constraint", "retaining_rise", "retaining_fall",
  "retain_rise_slew", "retain_fall_slew", "cell_degradation",
  "rise_power", "fall_power", "output_current_rise", "output_current_fall",
  "receiver_capacitance", "receiver_capacitance1_rise", "receiver_capacitance1_fall",
  "receiver_capacitance2_rise", "receiver_capacitance2_fall",
]);

// Groups that hold raw float data the viewer does NOT yet display (so they
// belong in the source, but they're huge) — collapsed FIRST when over budget.
const UNDISPLAYED_DATA = new Set([]);
const SOURCE_MAX_LINES = 1000;

async function api(path) {
  // Retry only transport-level failures (fetch rejects with TypeError:
  // "NetworkError when attempting to fetch resource"). On first startup the
  // browser opens the instant the static shell is served, a hair before the
  // loopback worker reliably accepts API calls (WSLg makes this race worse), so
  // the very first /api/* can drop. A short backoff lets boot self-heal instead
  // of painting "NetworkError" into the UI. HTTP errors (404/500) are NOT
  // transport failures — they resolve with !res.ok and throw immediately.
  let res;
  for (let attempt = 0; ; attempt++) {
    try {
      res = await fetch(path);
      break;
    } catch (e) {
      if (attempt >= 5) throw e;
      await new Promise((r) => setTimeout(r, 150 * (attempt + 1)));
    }
  }
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json();
}

// Snapshot of what the client is currently showing; dumped to a file by the
// "Dump Debug" button (only present under `liberty-view --dev`).
const debugState = { meta: null, lastTable: null };

let selectionVersion = 0;
let plotlyPromise = null;
const plotRequests = new WeakMap();

function loadPlotly() {
  if (window.Plotly) return Promise.resolve(window.Plotly);
  if (!plotlyPromise) {
    plotlyPromise = new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = "/plotly.min.js";
      const failed = () => {
        script.remove();
        reject(new Error("could not load Plotly"));
      };
      script.onload = () => window.Plotly ? resolve(window.Plotly) : failed();
      script.onerror = failed;
      document.head.appendChild(script);
    }).catch((error) => {
      plotlyPromise = null;
      throw error;
    });
  }
  return plotlyPromise;
}

async function drawPlot(method, element, ...args) {
  const selection = selectionVersion;
  const request = {};
  plotRequests.set(element, request);
  const isCurrent = () => selection === selectionVersion &&
    plotRequests.get(element) === request && element.isConnected;
  try {
    const plotly = await loadPlotly();
    if (isCurrent()) await plotly[method](element, ...args);
  } catch (error) {
    if (isCurrent()) element.textContent = `plot error: ${error.message}`;
  }
}

// Full breadcrumb of the selection, "/"-joined (lib / cell / pin / arc / table),
// so a screenshot carries the complete location without the tree.
let LIB_NAME = "";
function setCrumb(parts) {
  document.getElementById("crumb").textContent = parts.filter(Boolean).join(" / ");
}

function setViewWide(wide) {
  document.getElementById("content-grid")?.classList.toggle("wide-view", !!wide);
}

// ---- library meta + cell list ---------------------------------------------
async function loadMeta() {
  const m = await api("/api/meta");
  const nameEl = document.getElementById("lib-name");
  debugState.meta = m;
  LIB_NAME = m.library_name;
  nameEl.textContent = m.library_name;
  nameEl.classList.add("clickable");
  nameEl.onclick = () => {
    document
      .querySelectorAll(".node.selected")
      .forEach((n) => n.classList.remove("selected"));
    beginSelection(null);
    currentSymCell = null;
    clearCellSymbol(document.getElementById("cell-symbol"));
    clearActiveSource();
    document.getElementById("source-section").classList.add("hidden");
    document.getElementById("source").textContent = "";
    renderLibrary(m);
    setCrumb([LIB_NAME]);
  };
  const energy = energyLabel(m.energy_unit_joules);
  // Drop the leading magnitude ("1ps" -> "ps", "1mA" -> "mA").
  const bare = (u) => (u ? u.replace(/^\s*[0-9.eE+-]+\s*/, "") : u);
  const units = [
    bare(m.voltage_unit),
    bare(m.current_unit),
    bare(m.time_unit),
    energy,
    bare(m.leakage_power_unit),
  ]
    .filter(Boolean)
    .join(" / ");
  document.getElementById("lib-units").textContent =
    `${units} · ${m.num_cells} cells`;
}

const _ENERGY_PREFIX = {
  "-3": "mJ", "-6": "uJ", "-9": "nJ",
  "-12": "pJ", "-15": "fJ", "-18": "aJ", "0": "J",
};
function energyLabel(joules) {
  if (!joules) return null;
  const exp = Math.round(Math.log10(joules));
  return _ENERGY_PREFIX[String(exp)] || `${joules} J`;
}

let filterTimer = null;
let filterRequest = 0;
let visibleCells = new Map();
let selectedCell = null;

async function loadCells(filter, request = ++filterRequest) {
  const q = filter ? `?filter=${encodeURIComponent(filter)}` : "";
  const data = await api(`/api/cells${q}`);
  if (request !== filterRequest) return;
  document.getElementById("cell-count").textContent =
    `${data.cells.length} / ${data.total} cells`;
  const next = new Map();
  for (const name of data.cells) {
    const existing = selectedCell?.name === name ? selectedCell : visibleCells.get(name);
    next.set(name, existing || cellNode(name));
  }
  visibleCells = next;
  document.getElementById("tree").replaceChildren(...[...next.values()].map((cell) => cell.li));
}

function beginSelection(cell) {
  const previous = selectedCell;
  selectedCell = cell;
  if (previous && previous !== cell) previous.prune();
  debugState.lastTable = null;
  hideWave();
  return ++selectionVersion;
}

// ---- tree rendering --------------------------------------------------------
function selectRow(li, cell) {
  document.querySelectorAll(".node.selected").forEach((n) => n.classList.remove("selected"));
  li.classList.add("selected");
  return beginSelection(cell);
}

function currentTrees() {
  const cells = [...visibleCells.values()];
  if (selectedCell && !visibleCells.has(selectedCell.name)) cells.push(selectedCell);
  return Object.fromEntries(cells.filter((cell) => cell.data).map((cell) => [cell.name, cell.data]));
}

function cellNode(name) {
  const li = document.createElement("li");
  li.className = "node cell";
  const row = document.createElement("div");
  row.className = "row";
  row.innerHTML = `<span class="toggle">▸</span>${name}`;
  const kids = document.createElement("ul");
  kids.className = "tree hidden";
  let pending = null;
  let built = false;
  const toggleEl = row.querySelector(".toggle");
  const cell = {name, li, data: null, prune};
  const isOpen = () => !kids.classList.contains("hidden");
  const retainTree = () => selectedCell === cell || (visibleCells.get(name) === cell && isOpen());

  function prune() {
    if (retainTree()) return;
    cell.data = null;
    kids.replaceChildren();
    built = false;
  }

  async function ensure() {
    if (cell.data) return cell.data;
    if (!pending) {
      pending = api(`/api/cells/${encodeURIComponent(name)}`).finally(() => { pending = null; });
    }
    const data = await pending;
    if (retainTree()) cell.data = data;
    return data;
  }

  // Arrow marker (single-click) or row (double-click) toggles expand/collapse.
  const toggle = async () => {
    const open = !kids.classList.toggle("hidden");
    toggleEl.textContent = open ? "▾" : "▸";
    if (!open) {
      prune();
      return;
    }
    if (built) return;
    try {
      const data = await ensure();
      if (isOpen() && retainTree() && !built) {
        for (const child of data.children || []) {
          kids.appendChild(treeNode(child, cell, [LIB_NAME, name]));
        }
        built = true;
      }
    } catch (error) {
      console.error(error);
      kids.classList.add("hidden");
      toggleEl.textContent = "▸";
    }
  };
  toggleEl.onclick = (e) => {
    e.stopPropagation();
    toggle();
  };
  row.ondblclick = toggle;
  row.onclick = async () => {
    const selection = selectRow(li, cell);
    setCrumb([LIB_NAME, name]);
    showSource(name, {path: []});
    try {
      const data = await ensure();
      if (selection !== selectionVersion) return;
      renderAttrs(data);
      if (data.pg_pins) renderPgPins(data.pg_pins);
      showCellSymbolFor(name);
    } catch (error) {
      if (selection === selectionVersion) showTableError("cell", error);
    }
  };
  li.appendChild(row);
  li.appendChild(kids);
  return cell;
}

function treeNode(node, cell, crumb) {
  const cellName = cell.name;
  const trail = crumb.concat(node.label);
  const li = document.createElement("li");
  li.className = `node ${node.type}`;
  const row = document.createElement("div");
  row.className = "row";
  const hasKids = node.children && node.children.length;
  if (!hasKids) li.classList.add("leaf");
  const tog = hasKids ? "▸" : "";
  let metaStr = "";
  if (node.meta) {
    if (node.meta.direction) metaStr = ` <span class="meta-inline">${node.meta.direction}${node.meta.function ? " = " + node.meta.function : ""}</span>`;
    else if (node.meta.area != null) metaStr = ` <span class="meta-inline">A=${node.meta.area}</span>`;
  }
  row.innerHTML = `<span class="toggle">${tog}</span>${node.label}${metaStr}`;

  if (node.multi) {
    // Collapsed branch: several 1-D line tables plotted together with a legend.
    row.onclick = () => {
      selectRow(li, cell);
      showCellSymbolFor(cellName);
      loadMultiTable(cellName, node.multi, node.ylabel);
      showSource(cellName, node.src, node.multi.map((m) => m.ref.table));
      setCrumb(trail);
    };
    li.appendChild(row);
    return li;
  }

  if (node.type === "table") {
    row.onclick = () => {
      selectRow(li, cell);
      showCellSymbolFor(cellName);
      loadTable(cellName, node.ref);
      showSource(cellName, node.src);
      setCrumb(trail);
    };
    li.appendChild(row);
    return li;
  }

  if (node.type === "leakage") {
    row.onclick = () => {
      selectRow(li, cell);
      showCellSymbolFor(cellName);
      renderLeakage(node.leakage);
      showSource(cellName, node.src);
      setCrumb(trail);
    };
    li.appendChild(row);
    return li;
  }

  let kids = null;
  let built = false;
  const toggleEl = row.querySelector(".toggle");
  if (hasKids) {
    kids = document.createElement("ul");
    kids.className = "tree hidden";
    // Arrow marker (single-click) or row (double-click) toggles expand/collapse;
    // a single row click only selects, so navigating long trees doesn't fold
    // things by accident. Leaves have no toggle, so double-click is ignored.
    const toggle = () => {
      const open = !kids.classList.toggle("hidden");
      toggleEl.textContent = open ? "▾" : "▸";
      if (open && !built) {
        built = true;
        for (const child of node.children) kids.appendChild(treeNode(child, cell, trail));
      }
    };
    toggleEl.onclick = (e) => {
      e.stopPropagation();
      toggle();
    };
    row.ondblclick = toggle;
  }
  row.onclick = () => {
    selectRow(li, cell);
    showCellSymbolFor(cellName);
    // A node carries its own scalar attributes (pin caps, arc meta, …) — show
    // them in the main view.
    if (node.attributes && node.attributes.length) renderAttrs(node);
    showSource(cellName, node.src);
    setCrumb(trail);
  };
  li.appendChild(row);
  if (kids) li.appendChild(kids);
  return li;
}

function renderAttrs(node) {
  setViewWide(false);
  const view = document.getElementById("view");
  view.innerHTML = "";
  hideWave();
  const attrs = node.attributes || [];
  if (!attrs.length) {
    view.innerHTML = '<div class="muted">no scalar attributes</div>';
    return;
  }
  const t = document.createElement("table");
  t.className = "attrs";
  t.innerHTML =
    "<tr><th>attribute</th><th>value</th></tr>" +
    attrs.map(([k, v]) => `<tr><th>${k}</th><td>${v}</td></tr>`).join("");
  view.appendChild(t);
}

// Cell symbol, rendered into the persistent #cell-symbol bar at the top so it
// stays visible while navigating the cell's pins/tables (not only when the cell
// line is selected). Sequential cells (flip-flop / latch) render a box symbol;
// combinational cells render each output pin's boolean function as a gate
// schematic.
const FIT_H = [90, 280];
const SYMBOL_THUMB_W = 260;
const SYMBOL_THUMB_H = 150;
const SYMBOL_ZOOM_FONT_PX = 16; // 12pt.
const SYMBOL_ZOOM_MARGIN = 24;
const SYMBOL_ZOOM_MS = 1000;
let currentSymCell = null;
let symbolZoom = null;

const escHtml = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

// Equation HTML where each minterm is an unbreakable span, so the line only
// wraps at the ` + ` separators (never inside a minterm — `!` is otherwise a
// soft-wrap opportunity).
function eqHtml(label, func) {
  const terms = String(func).split(" + ").map((m) => `<span class="mt">${escHtml(m)}</span>`);
  return `<span class="sym-lhs">${escHtml(label)} = </span>${terms.join(" + ")}`;
}

// Show the owning cell's symbol; cheap no-op if it's already displayed.
function showCellSymbolFor(cellName) {
  if (cellName === currentSymCell) return;
  if (selectedCell?.name !== cellName || !selectedCell.data) return;
  currentSymCell = cellName;
  renderCellSymbol(selectedCell.data);
}

function clearCellSymbol(host) {
  closeCellSymbolZoom(false);
  host.innerHTML = "";
  host.style.removeProperty("--cell-symbol-frame-w");
  host.style.removeProperty("--cell-symbol-frame-h");
  host.removeAttribute("role");
  host.removeAttribute("tabindex");
  host.removeAttribute("aria-label");
  host.onclick = null;
  host.onkeydown = null;
  document.getElementById("content-grid")?.classList.remove("has-cell-symbol");
}

function renderCellSymbol(cellData) {
  const host = document.getElementById("cell-symbol");
  if (!host) return;
  clearCellSymbol(host);

  let inner = "";
  if (cellData.seq && typeof seqSymbolSvg === "function") {
    const kind = cellData.seq.kind === "latch" ? "latch" : cellData.seq.scan ? "scan flop" : "flip-flop";
    let body;
    try {
      body = seqSymbolSvg(cellData.seq, { fitHeight: FIT_H });
    } catch (e) {
      body = `<div class="muted">${e.message}</div>`;
    }
    inner = `<div class="sym"><div class="sym-title">${kind}</div>${body}</div>`;
  } else if (typeof functionToSvg === "function") {
    const outs = (cellData.children || []).filter(
      (n) => n.type === "pin" && n.meta && /output/.test(n.meta.direction || "") && n.meta.function
    );
    if (!outs.length) return;
    inner = outs
      .map((n) => {
        let body;
        try {
          body = functionToSvg(n.meta.function, n.label, { minWidthRatio: 0.5, dots: false, demorgan: true, fitHeight: FIT_H });
        } catch (e) {
          body = `<div class="muted">${e.message}</div>`;
        }
        return `<div class="sym"><div class="sym-title eq">${eqHtml(n.label, n.meta.function)}</div>${body}</div>`;
      })
      .join("");
  } else {
    return;
  }

  const frame = document.createElement("div");
  frame.className = "cell-sym-frame";
  const box = document.createElement("div");
  box.className = "cell-sym";
  box.innerHTML = inner;
  frame.appendChild(box);
  host.appendChild(frame);
  document.getElementById("content-grid")?.classList.add("has-cell-symbol");
  host.setAttribute("role", "button");
  host.setAttribute("tabindex", "0");
  host.setAttribute("aria-label", "Zoom cell symbol");
  host.onclick = (e) => {
    e.stopPropagation();
    openCellSymbolZoom();
  };
  host.onkeydown = (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault();
    openCellSymbolZoom();
  };

  // Height: near natural size, clamped to FIT_H px. The label font is baked in
  // user units (fitHeight) so it lands at ~12px on screen. Width is driven only
  // by SVG geometry; equation text wraps/clips inside that width.
  const symMetrics = [];
  box.querySelectorAll(".sym").forEach((sym) => {
    const s = sym.querySelector("svg");
    if (!s) return;
    const vb = (s.getAttribute("viewBox") || "0 0 0 0").split(/\s+/).map(Number);
    const vbW = vb[2] || 1, vbH = vb[3] || 120;
    const hPx = Math.round(Math.min(FIT_H[1], Math.max(FIT_H[0], vbH)));
    s.setAttribute("height", hPx);
    s.removeAttribute("width");
    const title = sym.querySelector(".sym-title");
    if (title) {
      const svgW = Math.round((hPx * vbW) / vbH);
      const scale = hPx / vbH;
      title.title = title.textContent;
      const inputLabels = [...s.querySelectorAll(".lbl:not(.out)")];
      let labelRight = 0;
      if (inputLabels.length) {
        try {
          labelRight = Math.max(...inputLabels.map((label) => {
            const anchor = parseFloat(label.getAttribute("x"));
            if (label.getAttribute("text-anchor") === "end" && Number.isFinite(anchor)) return anchor;
            const box = label.getBBox();
            return box.x + box.width;
          }));
        } catch {
          const fontUser = parseFloat(s.style.fontSize || getComputedStyle(s).fontSize) || 12;
          labelRight = Math.max(...inputLabels.map((label) => {
            const x = parseFloat(label.getAttribute("x")) || 0;
            return label.getAttribute("text-anchor") === "end"
              ? x
              : x + label.textContent.length * fontUser * 0.62;
          }));
        }
      }
      const labelRightPx = Math.max(0, Math.round((labelRight - vb[0]) * scale));
      title.style.marginLeft = "0";
      title.style.width = "";
      title.style.maxWidth = "";
      symMetrics.push({
        sym,
        svg: s,
        title,
        svgW,
        labelRightPx,
      });
    }
  });
  if (symMetrics.length) {
    const labelCol = Math.max(...symMetrics.map((m) => m.labelRightPx));
    let stackW = 1;
    symMetrics.forEach((m) => {
      const svgOffset = Math.max(0, labelCol - m.labelRightPx);
      const symW = m.svgW + svgOffset;
      m.svg.style.marginLeft = `${svgOffset}px`;
      stackW = Math.max(stackW, symW);
    });
    stackW = Math.ceil(stackW);
    symMetrics.forEach((m) => {
      m.title.style.width = `${stackW}px`;
      m.title.style.maxWidth = `${stackW}px`;
      m.sym.style.width = `${stackW}px`;
    });
  }
  fitCellSymbolFrame(frame, box, SYMBOL_THUMB_W - 12, SYMBOL_THUMB_H - 12);
  syncCellSymbolHostSize(host, frame);
}

function fitCellSymbolFrame(frame, box, maxW, maxH) {
  box.style.transform = "none";
  const r = box.getBoundingClientRect();
  const rawW = Math.max(1, Math.ceil(box.scrollWidth || r.width));
  const rawH = Math.max(1, Math.ceil(box.scrollHeight || r.height));
  const scale = Math.min(1, maxW / rawW, maxH / rawH);
  frame.style.width = `${Math.ceil(rawW * scale)}px`;
  frame.style.height = `${Math.ceil(rawH * scale)}px`;
  box.style.transform = `scale(${scale})`;
  box.dataset.symRawWidth = String(rawW);
  box.dataset.symRawHeight = String(rawH);
  box.dataset.symThumbScale = String(scale);
}

function syncCellSymbolHostSize(host, frame) {
  const r = frame.getBoundingClientRect();
  const w = Math.max(1, Math.ceil(parseFloat(frame.style.width) || r.width));
  const h = Math.max(1, Math.ceil(parseFloat(frame.style.height) || r.height));
  host.style.setProperty("--cell-symbol-frame-w", `${w}px`);
  host.style.setProperty("--cell-symbol-frame-h", `${h}px`);
}

function svgFontPxAtScaleOne(box) {
  let maxPx = 12;
  box.querySelectorAll("svg").forEach((svg) => {
    const vb = (svg.getAttribute("viewBox") || "0 0 0 0").split(/\s+/).map(Number);
    const vbH = vb[3] || 1;
    const hPx = parseFloat(svg.getAttribute("height")) || svg.getBoundingClientRect().height || vbH;
    const fontUser = parseFloat(svg.style.fontSize || getComputedStyle(svg).fontSize) || 12;
    maxPx = Math.max(maxPx, fontUser * hPx / vbH);
  });
  return maxPx;
}

function symbolZoomTarget(box, rawW, rawH) {
  const targetByFont = SYMBOL_ZOOM_FONT_PX / svgFontPxAtScaleOne(box);
  const capByViewport = Math.min(
    (window.innerWidth - SYMBOL_ZOOM_MARGIN * 2) / rawW,
    (window.innerHeight - SYMBOL_ZOOM_MARGIN * 2) / rawH
  );
  const startScale = parseFloat(box.dataset.symThumbScale) || 1;
  const scale = Math.max(0.1, Math.min(Math.max(startScale, targetByFont), capByViewport));
  const w = rawW * scale;
  const h = rawH * scale;
  return {
    scale,
    left: Math.max(SYMBOL_ZOOM_MARGIN, (window.innerWidth - w) / 2),
    top: Math.max(SYMBOL_ZOOM_MARGIN, (window.innerHeight - h) / 2),
  };
}

function openCellSymbolZoom() {
  const host = document.getElementById("cell-symbol");
  const frame = host?.querySelector(".cell-sym-frame");
  const box = frame?.querySelector(".cell-sym");
  if (!host || !frame || !box || symbolZoom) return;

  const rawW = parseFloat(box.dataset.symRawWidth) || box.scrollWidth;
  const rawH = parseFloat(box.dataset.symRawHeight) || box.scrollHeight;
  if (!rawW || !rawH) return;

  const from = frame.getBoundingClientRect();
  const startScale = from.width / rawW;
  const to = symbolZoomTarget(box, rawW, rawH);
  const backdrop = document.createElement("div");
  backdrop.className = "symbol-zoom-backdrop";
  const panel = document.createElement("div");
  panel.className = "symbol-zoom-panel";
  panel.style.width = `${rawW}px`;
  panel.style.height = `${rawH}px`;
  panel.style.transform = `translate(${from.left}px, ${from.top}px) scale(${startScale})`;

  const clone = box.cloneNode(true);
  clone.style.transform = "none";
  clone.removeAttribute("data-sym-raw-width");
  clone.removeAttribute("data-sym-raw-height");
  clone.removeAttribute("data-sym-thumb-scale");
  panel.appendChild(clone);
  document.body.append(backdrop, panel);

  const close = () => closeCellSymbolZoom(true);
  symbolZoom = { backdrop, panel, rawW, rawH, close };
  backdrop.addEventListener("click", close);
  panel.addEventListener("click", close);
  document.addEventListener("keydown", onCellSymbolZoomKey, true);

  // Commit the thumbnail transform before applying the zoom target; otherwise
  // some browsers batch both writes into the first paint and the open looks
  // instant while close animates normally.
  panel.getBoundingClientRect();
  backdrop.getBoundingClientRect();
  requestAnimationFrame(() => {
    backdrop.classList.add("open");
    panel.style.transform = `translate(${to.left}px, ${to.top}px) scale(${to.scale})`;
  });
}

function onCellSymbolZoomKey(e) {
  if (e.key === "Escape") closeCellSymbolZoom(true);
}

function closeCellSymbolZoom(animate) {
  if (!symbolZoom) return;
  const z = symbolZoom;
  symbolZoom = null;
  document.removeEventListener("keydown", onCellSymbolZoomKey, true);
  const finish = () => {
    z.backdrop.remove();
    z.panel.remove();
  };
  if (!animate) {
    finish();
    return;
  }
  z.backdrop.classList.remove("open");
  const frame = document.querySelector("#cell-symbol .cell-sym-frame");
  if (frame) {
    const to = frame.getBoundingClientRect();
    z.panel.style.transform = `translate(${to.left}px, ${to.top}px) scale(${to.width / z.rawW})`;
  } else {
    z.panel.style.opacity = "0";
  }
  setTimeout(finish, SYMBOL_ZOOM_MS);
}

// pg_pin groups as a table (appended below the cell attributes).
function renderPgPins(d) {
  const view = document.getElementById("view");
  const t = document.createElement("table");
  t.className = "attrs pg";
  t.innerHTML =
    "<tr><th>pg_pin</th>" +
    d.cols.map((c) => `<th>${c}</th>`).join("") +
    "</tr>" +
    d.rows
      .map(
        (r) =>
          `<tr><th>${r.name}</th>` +
          d.cols.map((c) => `<td>${r.values[c] ?? ""}</td>`).join("") +
          "</tr>"
      )
      .join("");
  view.appendChild(t);
}

// leakage_power: a when-condition × power-rail table of static leakage values.
function renderLeakage(d) {
  setViewWide(false);
  const view = document.getElementById("view");
  view.innerHTML = "";
  hideWave();
  const cols = d.pg_pins;
  const t = document.createElement("table");
  t.className = "attrs";
  t.innerHTML =
    "<tr><th>when</th>" +
    cols.map((c) => `<th>${c}</th>`).join("") +
    "</tr>" +
    d.rows
      .map(
        (r) =>
          `<tr><th>${r.when}</th>` +
          cols
            .map((c) => `<td class="num">${r.values[c] ?? ""}</td>`)
            .join("") +
          "</tr>"
      )
      .join("");
  view.appendChild(t);
}

// ---- library-level view ----------------------------------------------------
function renderLibrary(m) {
  renderAttrs({ label: m.library_name, attributes: m.attributes });
  const waves = m.driver_waveforms || [];
  if (!waves.length) return;
  const view = document.getElementById("view");

  const h = document.createElement("h3");
  h.className = "section";
  h.textContent = "normalized_driver_waveform — click a waveform to plot it";
  view.appendChild(h);

  const bar = document.createElement("div");
  bar.className = "dw-buttons";
  view.appendChild(bar);

  // Build the detail (caption + table + plot) once and reuse it, so clicking a
  // different waveform updates content in place instead of tearing the subtree
  // down and rebuilding it — that collapse/regrow is what jumped scroll to top.
  const cap = document.createElement("h3");
  cap.className = "section";
  const scroll = document.createElement("div");
  scroll.className = "dw-table-scroll";
  const tbl = document.createElement("table");
  tbl.className = "grid";
  scroll.appendChild(tbl);
  const plotTitle = document.createElement("h3");
  plotTitle.className = "section";
  plotTitle.textContent = "waveform (one curve per slew)";
  const plot = document.createElement("div");
  const refs = { cap, tbl, plot };
  view.append(cap, scroll, plotTitle, plot);

  waves.forEach((w) => {
    const btn = document.createElement("button");
    btn.className = "dw-btn";
    btn.textContent = w.name;
    btn.onclick = () => {
      bar.querySelectorAll(".dw-btn.active").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
      updateDriverWaveform(w, refs);
    };
    bar.appendChild(btn);
  });
}

// A normalized_driver_waveform is a 2-D table (slew x normalized voltage -> time).
// Show the raw table, then plot one voltage-vs-time curve per input slew. Updates
// the persistent refs in place (Plotly.react keeps the same div -> no scroll jump).
function updateDriverWaveform(w, refs) {
  const L = w.labels;
  refs.cap.textContent = `${w.name}: ${L.time} for each (${L.slew}, ${L.voltage})`;

  refs.tbl.innerHTML =
    `<tr><th>${L.slew} \\ ${L.voltage}</th>` +
    w.voltage.map((v) => `<th>${v}</th>`).join("") +
    "</tr>" +
    w.slew
      .map(
        (s, i) =>
          `<tr><th>${s}</th>` +
          w.time[i].map((t) => `<td class="num">${t}</td>`).join("") +
          "</tr>"
      )
      .join("");

  const traces = w.slew.map((s, i) => ({
    x: w.time[i],
    y: w.voltage,
    mode: "lines+markers",
    name: `${L.slew}=${s}`,
  }));
  drawPlot("react", refs.plot, traces, {
    ...PLOT_LAYOUT,
    xaxis: { title: L.time },
    yaxis: { title: L.voltage, range: [0, 1] },
    legend: { font: { size: 10 } },
  }, { responsive: true });
}

// ---- leaf table rendering --------------------------------------------------
async function loadTable(cell, ref) {
  const selection = selectionVersion;
  const params = new URLSearchParams({
    cell,
    pin: ref.pin,
    group: ref.group,
    arc_index: ref.arc_index,
    table: ref.table,
    container: ref.container || "",
  });
  try {
    const data = await api(`/api/table?${params}`);
    if (selection !== selectionVersion) return;
    debugState.lastTable = { cell, ref, data };
    renderTable(data, ref);
  } catch (error) {
    if (selection === selectionVersion) showTableError("table", error);
  }
}

function showTableError(kind, error) {
  const message = document.createElement("div");
  message.className = "muted";
  message.textContent = `${kind} error: ${error.message}`;
  document.getElementById("view").replaceChildren(message);
}

function renderTable(data, ref) {
  const view = document.getElementById("view");
  view.innerHTML = "";
  hideWave();
  if (data.kind === "ccs") return renderCcsGrid(view, data);
  if (data.ndim === 0) return renderScalar(view, data);
  if (data.ndim === 1) return renderLine(view, data, ref);
  if (data.ndim === 2) {
    // A 2-index table with one index pinned to a single value is really a 1-D
    // curve — draw it as a line (not a 1-row/col heatmap) and note the fixed
    // index in the title.
    const flat = collapse2d(data);
    if (flat) return renderLine(view, flat, ref);
    return renderHeatmap(view, data, ref);
  }
  return renderGrid(view, data, ref);
}

function fmtVal(v) {
  return typeof v === "number" ? Number(v.toPrecision(4)).toString() : String(v);
}

// If a 2-D table has a single-valued index, return a pseudo-1-D payload over the
// multi-valued index (with `_fixed` describing the pinned index); else null.
function collapse2d(data) {
  const n1 = (data.index_1 || []).length;
  const n2 = (data.index_2 || []).length;
  const L = labels(data);
  if (n2 === 1 && n1 > 1) {
    return {
      ...data, ndim: 1, index_1: data.index_1, values: data.values.map((r) => r[0]),
      labels: { ...L, index_1: L.index_1 }, _fixed: `${L.index_2} = ${fmtVal(data.index_2[0])}`,
    };
  }
  if (n1 === 1) {
    return {
      ...data, ndim: 1, index_1: data.index_2, values: data.values[0],
      labels: { ...L, index_1: L.index_2 }, _fixed: `${L.index_1} = ${fmtVal(data.index_1[0])}`,
    };
  }
  return null;
}

// CCS: clickable (slew x cap) grid; each cell holds a current-vs-time wave.
// Grid label = t95 (time current decays to 95% of peak). Click -> wave plot.
function renderCcsGrid(view, data) {
  setViewWide(false);
  const L = data.labels || { index_1: "slew", index_2: "cap", time: "time", current: "current" };
  const note = document.createElement("h3");
  note.className = "section";
  note.textContent =
    `${data.table}: ${L.index_1} × ${L.index_2} — cell = time of peak |${L.current}| (${L.time}); click to plot the current wave`;
  view.appendChild(note);

  const tbl = document.createElement("table");
  tbl.className = "grid";
  const head = document.createElement("tr");
  head.innerHTML = `<th>${L.index_1} \\ ${L.index_2}</th>` + data.index_2.map((c) => `<th>${c}</th>`).join("");
  tbl.appendChild(head);

  const populated = [];
  data.index_1.forEach((slew, i) => {
    const tr = document.createElement("tr");
    tr.innerHTML = `<th>${slew}</th>`;
    data.index_2.forEach((cap, j) => {
      const td = document.createElement("td");
      const cellData = data.grid[i][j];
      if (cellData) {
        const btn = document.createElement("button");
        btn.textContent = cellData.t_peak != null ? cellData.t_peak.toPrecision(4) : "·";
        btn.title = `slew=${slew}, cap=${cap}, ref_time=${cellData.reference_time}`;
        btn.onclick = () => {
          tbl.querySelectorAll("button.active").forEach((b) => b.classList.remove("active"));
          btn.classList.add("active");
          showCcsWave(data.table, slew, cap, cellData, data.labels);
        };
        populated.push({ button: btn, i, j });
        td.appendChild(btn);
      }
      tr.appendChild(td);
    });
    tbl.appendChild(tr);
  });
  view.appendChild(tbl);
  if (populated.length) {
    const ci = Math.floor((data.index_1.length - 1) / 2);
    const cj = Math.floor((data.index_2.length - 1) / 2);
    const nearest = populated.reduce((best, cell) => {
      const bestDist = (best.i - ci) ** 2 + (best.j - cj) ** 2;
      const cellDist = (cell.i - ci) ** 2 + (cell.j - cj) ** 2;
      return cellDist < bestDist ? cell : best;
    });
    nearest.button.click();
  }
}

// CCS/CCSP waves carry a long settling tail (current ~0 out to ~1 ns); without
// clamping, the x-autoscale squishes the real edge into a sliver that reads as
// "nothing". Return the time at which the signal has decayed to <1% of peak.
function activeXMax(time, current) {
  const full = time.length ? time[time.length - 1] : 1;
  let peak = 0;
  for (const v of current) {
    const a = Math.abs(v);
    if (a > peak) peak = a;
  }
  if (peak <= 0) return full;
  const thr = peak * 0.01;
  let last = 0;
  for (let k = 0; k < current.length; k++) {
    if (Math.abs(current[k]) > thr) last = k;
  }
  const x = (time[last] || full) * 1.15;
  return x > 0 ? Math.min(x, full) : full;
}

function showCcsWave(table, slew, cap, cell, L) {
  L = L || { time: "time", current: "current" };
  document.getElementById("wave-section").classList.remove("hidden");
  document.getElementById("wave-title").textContent =
    `${table} · slew=${slew} · cap=${cap} · ref_time=${cell.reference_time} · t_peak=${cell.t_peak != null ? cell.t_peak.toPrecision(4) : "n/a"}`;
  const traces = [{ x: cell.time, y: cell.current, mode: "lines+markers", line: { color: "#7bd88f" }, name: "current" }];
  const shapes = [];
  if (cell.t_peak != null) {
    shapes.push({ type: "line", x0: cell.t_peak, x1: cell.t_peak, yref: "paper", y0: 0, y1: 1, line: { color: "#e6c07b", dash: "dot", width: 1 } });
  }
  drawPlot("newPlot", document.getElementById("wave"), traces, {
    ...PLOT_LAYOUT,
    height: PLOT_H,
    // Default view to the active region; double-click autoscales to the full tail.
    xaxis: { title: L.time, range: [0, activeXMax(cell.time, cell.current)] },
    yaxis: { title: L.current },
    shapes,
  }, { responsive: true });
}

function labels(data) {
  return data.labels || { index_1: "index_1", index_2: "index_2", index_3: "index_3", value: "value" };
}

function renderScalar(view, data) {
  setViewWide(false);
  const L = labels(data);
  const t = document.createElement("table");
  t.className = "scalar";
  t.innerHTML = `<tr><th>${L.value}</th><td>${data.scalar}</td></tr>`;
  view.appendChild(t);
}

function renderLine(view, data, ref) {
  setViewWide(true);
  const L = labels(data);
  const div = document.createElement("div");
  view.appendChild(div);
  // Power is energy (fJ); always keep 0 on the y-axis so magnitudes read true.
  const isPower = /power/.test(data.table || "");
  drawPlot("newPlot", div, [{ x: data.index_1, y: data.values, mode: "lines+markers", line: { color: "#5aa9e6" } }], {
    ...PLOT_LAYOUT,
    height: PLOT_H,
    margin: { ...PLOT_LAYOUT.margin, t: 32 },
    title: data._fixed ? `${L.value} @ ${data._fixed}` : L.value,
    xaxis: { title: { text: L.index_1, standoff: 6 }, automargin: true },
    yaxis: { title: { text: L.value, standoff: 12 }, automargin: true, rangemode: isPower ? "tozero" : "normal" },
  }, { responsive: true });
}

// Several 1-D line tables on one plot, differentiated by legend (e.g. a power
// pg node's rise_power + fall_power -> y-axis "power", legend "rise"/"fall").
async function loadMultiTable(cell, items, ylabel) {
  const selection = selectionVersion;
  try {
    const datas = await Promise.all(
      items.map((it) => {
        const r = it.ref;
        const params = new URLSearchParams({
          cell, pin: r.pin, group: r.group, arc_index: r.arc_index,
          table: r.table, container: r.container || "",
        });
        return api(`/api/table?${params}`);
      })
    );
    if (selection !== selectionVersion) return;
    debugState.lastTable = { cell, multi: items, ylabel, datas };
    renderMultiLine(datas, items.map((i) => i.name), ylabel);
  } catch (error) {
    if (selection === selectionVersion) showTableError("table", error);
  }
}

function renderMultiLine(datas, names, ylabel) {
  setViewWide(true);
  const view = document.getElementById("view");
  view.innerHTML = "";
  hideWave();
  const div = document.createElement("div");
  view.appendChild(div);
  const isPower = datas.some((d) => /power/.test(d.table || ""));
  const L0 = labels(datas[0]);
  const traces = datas.map((d, i) => ({
    x: d.index_1, y: d.values, mode: "lines+markers", name: names[i],
  }));
  drawPlot("newPlot", div, traces, {
    ...PLOT_LAYOUT,
    height: PLOT_H,
    margin: { ...PLOT_LAYOUT.margin, t: 32 },
    title: ylabel,
    xaxis: { title: { text: L0.index_1, standoff: 6 }, automargin: true },
    yaxis: { title: { text: ylabel, standoff: 12 }, automargin: true, rangemode: isPower ? "tozero" : "normal" },
    legend: { font: { size: 11 } },
  }, { responsive: true });
}

function renderHeatmap(view, data, ref) {
  setViewWide(true);
  const L = labels(data);
  const hover = `${L.index_1}=%{y}<br>${L.index_2}=%{x}<br>${L.value}=%{z}<extra></extra>`;
  const heat = document.createElement("div");
  view.appendChild(heat);
  drawPlot("newPlot", heat, [{
    z: data.values, x: data.index_2, y: data.index_1,
    type: "heatmap", colorscale: "Viridis", hovertemplate: hover,
  }], {
    ...PLOT_LAYOUT,
    height: 300,
    margin: { ...PLOT_LAYOUT.margin, t: 32 },
    title: L.value,
    xaxis: { title: L.index_2 },
    yaxis: { title: L.index_1 },
  }, { responsive: true });

  const controls = document.createElement("div");
  controls.className = "plot-controls";

  const modeControls = document.createElement("div");
  modeControls.className = "plot-segment";
  const waveControls = document.createElement("div");
  waveControls.className = "plot-segment hidden";

  const state = { mode: "surface", groupBy: "index_1" };
  const surf = document.createElement("div");
  view.append(controls, surf);

  const renderLower = () => {
    const isWaves = state.mode === "waves";
    waveControls.classList.toggle("hidden", !isWaves);
    setPlotControlActive(modeButtons, state.mode);
    setPlotControlActive(groupButtons, state.groupBy);
    if (isWaves) renderWaveFamilies(surf, data, L, state.groupBy);
    else renderSurfacePlot(surf, data, L, hover);
  };

  const modeButtons = {
    surface: plotControlButton("3D surface", () => {
      state.mode = "surface";
      renderLower();
    }),
    waves: plotControlButton("2D waves", () => {
      state.mode = "waves";
      renderLower();
    }),
  };
  modeControls.append(modeButtons.surface, modeButtons.waves);

  const groupButtons = {
    index_1: plotControlButton(`Group by ${L.index_1}`, () => {
      state.groupBy = "index_1";
      renderLower();
    }),
    index_2: plotControlButton(`Group by ${L.index_2}`, () => {
      state.groupBy = "index_2";
      renderLower();
    }),
  };
  waveControls.append(groupButtons.index_1, groupButtons.index_2);
  controls.append(modeControls, waveControls);
  renderLower();
}

function plotControlButton(text, onClick) {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.textContent = text;
  btn.onclick = onClick;
  return btn;
}

function setPlotControlActive(buttons, activeKey) {
  Object.entries(buttons).forEach(([key, btn]) => {
    const active = key === activeKey;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-pressed", active ? "true" : "false");
  });
}

function renderSurfacePlot(surf, data, L, hover) {
  // z range spans the actual data (values can be negative, e.g. hold
  // constraints); anchor at 0 only when the data is one-sided.
  const flat = data.values.flat();
  let zlo = Math.min(...flat);
  let zhi = Math.max(...flat);
  if (zlo > 0) zlo = 0;
  if (zhi < 0) zhi = 0;
  if (zlo === zhi) { zlo -= 1; zhi += 1; }
  // Markers at the actual table grid points overlaid on the interpolated surface.
  const px = [];
  const py = [];
  const pz = [];
  data.index_1.forEach((yv, i) =>
    data.index_2.forEach((xv, j) => {
      px.push(xv);
      py.push(yv);
      pz.push(data.values[i][j]);
    })
  );
  // Equal x/y screen extent, z capped at 50% of it (steep z ranges don't render
  // as a cliff); viewed from the -x/-y side so the (0,0,0) corner faces front.
  const sceneBase = {
    aspectmode: "manual",
    aspectratio: { x: 1, y: 1, z: 0.5 },
    camera: { eye: { x: -1.2, y: -1.2, z: 0.85 } },
  };
  // All three axes start at 0 so x/y share the same origin corner; white grid.
  const axisXY = {
    xaxis: { title: L.index_2, range: [0, Math.max(...data.index_2)], ...GRID_WHITE },
    yaxis: { title: L.index_1, range: [0, Math.max(...data.index_1)], ...GRID_WHITE },
  };
  const points = {
    x: px, y: py, type: "scatter3d", mode: "markers", showlegend: false,
    marker: { size: 1.65, color: "#ff8c00", opacity: 0.95 }, name: "points",
  };

  // CCSN dc_current: show the current surface beside a resistance surface,
  // resistance = |input_voltage - output_voltage| / dc_current. Two 3D scenes
  // side by side in one div; cells where I==0 become gaps (null).
  if (data.table === "dc_current") {
    // Rail-relative on-resistance of the conducting pull path: the rail the
    // current flows FROM is Vdd when I>0 (charging up) else Vss, so
    // R = (Vrail - Vout) / I is naturally >= 0 and reads as pulling strength.
    // Drop cells where the input/output swing |Vin-Vout| is < 10% of the rail
    // span (ill-conditioned near the operating point). Falls back to |dV|/|I|
    // when the rails are unknown.
    const rails = data.rails;
    const kscale = data.r_scale_kohm || 1; // raw (V / current_unit) -> kohm
    const dvMin = rails ? 0.1 * (rails.vdd - rails.vss) : 0;
    const R_MAX_KOHM = 10; // drop implausibly large resistances (near I->0)
    const R = data.index_1.map((vin, i) =>
      data.index_2.map((vout, j) => {
        const I = data.values[i][j];
        if (!I) return null;
        let r;
        if (!rails) {
          r = (Math.abs(vin - vout) / Math.abs(I)) * kscale;
        } else {
          if (Math.abs(vin - vout) < dvMin) return null;
          const vrail = I > 0 ? rails.vdd : rails.vss;
          r = ((vrail - vout) / I) * kscale;
        }
        return r > R_MAX_KOHM ? null : r;
      })
    );
    const rz = [];
    data.index_1.forEach((_, i) => data.index_2.forEach((__, j) => rz.push(R[i][j])));
    // Resistance explodes where dc_current -> 0 (heavy-tailed), which flattens
    // the "transistors-on" range. A robust percentile cap (90th pct + 10%) keeps
    // the on-range variation visible; outlier spikes clip out of the view cube.
    // The same bounds drive the colorscale (cmin/cmax) so colour isn't washed out.
    const pctRange = (arr) => {
      const f = arr.filter((v) => Number.isFinite(v)).sort((a, b) => a - b);
      if (!f.length) return [0, 1];
      const q = (p) => {
        const pos = (f.length - 1) * p;
        const i = Math.floor(pos);
        return f[i + 1] !== undefined ? f[i] + (pos - i) * (f[i + 1] - f[i]) : f[i];
      };
      let lo = f[0] >= 0 ? 0 : q(0.1);
      let hi = q(0.9) * 1.1;
      if (!(hi > lo)) { lo = f[0]; hi = f[f.length - 1]; }
      if (lo === hi) { lo -= 1; hi += 1; }
      return [lo, hi];
    };
    const [rlo, rhi] = pctRange(rz);
    const hoverR = `${L.index_1}=%{y}<br>${L.index_2}=%{x}<br>resistance=%{z:.4g} kΩ<extra></extra>`;
    const valBare = (L.value || "").replace(/\s*\([^)]*\)\s*$/, ""); // drop unit suffix
    drawPlot("newPlot", surf, [
      { z: data.values, x: data.index_2, y: data.index_1, scene: "scene", type: "surface", colorscale: "Viridis", showscale: false, opacity: 0.8, hovertemplate: hover },
      { ...points, z: pz, scene: "scene", hovertemplate: hover },
      { z: R, x: data.index_2, y: data.index_1, scene: "scene2", type: "surface", colorscale: "Cividis", showscale: false, opacity: 0.8, cmin: rlo, cmax: rhi, hovertemplate: hoverR },
      { ...points, z: rz, scene: "scene2", hovertemplate: hoverR },
    ], {
      ...PLOT_LAYOUT,
      height: 340,
      margin: { l: 0, r: 0, t: 18, b: 0 },
      scene: { ...sceneBase, domain: { x: [0, 0.48], y: [0, 1] }, ...axisXY, zaxis: { title: valBare, range: [zlo, zhi], ...GRID_WHITE } },
      scene2: { ...sceneBase, domain: { x: [0.52, 1], y: [0, 1] }, ...axisXY, zaxis: { title: "resistance", range: [rlo, rhi], ...GRID_WHITE } },
      annotations: [
        { text: L.value, x: 0.24, y: 1, xref: "paper", yref: "paper", showarrow: false, font: { color: "#cbd5e1" } },
        { text: "resistance (kΩ)", x: 0.76, y: 1, xref: "paper", yref: "paper", showarrow: false, font: { color: "#cbd5e1" } },
      ],
    }, { responsive: true });
    return;
  }

  drawPlot("newPlot", surf, [{
    z: data.values, x: data.index_2, y: data.index_1,
    type: "surface", colorscale: "Viridis", showscale: false, opacity: 0.8, hovertemplate: hover,
  }, {
    ...points, z: pz, hovertemplate: hover,
  }], {
    ...PLOT_LAYOUT,
    height: 340,
    margin: { l: 0, r: 0, t: 0, b: 0 },
    scene: {
      ...sceneBase,
      domain: { x: [0, 1], y: [0, 1] },
      ...axisXY,
      zaxis: { title: L.value, range: [zlo, zhi], ...GRID_WHITE },
    },
  }, { responsive: true });
}

function renderWaveFamilies(plot, data, L, groupBy) {
  const traces = groupBy === "index_2"
    ? data.index_2.map((xv, j) => ({
      x: data.index_1,
      y: data.index_1.map((_, i) => data.values[i][j]),
      name: `${L.index_2}=${fmtVal(xv)}`,
      mode: "lines+markers",
      type: "scatter",
    }))
    : data.index_1.map((yv, i) => ({
      x: data.index_2,
      y: data.values[i],
      name: `${L.index_1}=${fmtVal(yv)}`,
      mode: "lines+markers",
      type: "scatter",
    }));
  const xTitle = groupBy === "index_2" ? L.index_1 : L.index_2;
  const isPower = /power/.test(data.table || "");
  drawPlot("newPlot", plot, traces, {
    ...PLOT_LAYOUT,
    height: PLOT_H,
    margin: { ...PLOT_LAYOUT.margin, t: 18 },
    xaxis: { title: { text: xTitle, standoff: 6 }, automargin: true },
    yaxis: { title: { text: L.value, standoff: 12 }, automargin: true, rangemode: isPower ? "tozero" : "normal" },
    legend: { font: { size: 11 } },
  }, { responsive: true });
}

// 3D table -> clickable (index_1 x index_2) grid; cell click -> wave over index_3.
function renderGrid(view, data, ref) {
  setViewWide(false);
  const L = labels(data);
  const note = document.createElement("h3");
  note.className = "section";
  note.textContent = `${data.table}: ${L.index_1} × ${L.index_2} grid — click a cell to plot the ${L.index_3} wave`;
  view.appendChild(note);

  const tbl = document.createElement("table");
  tbl.className = "grid";
  const head = document.createElement("tr");
  head.innerHTML = `<th></th>` + data.index_2.map((v) => `<th>${v}</th>`).join("");
  tbl.appendChild(head);

  data.index_1.forEach((y, i) => {
    const tr = document.createElement("tr");
    tr.innerHTML = `<th>${y}</th>`;
    data.index_2.forEach((x, j) => {
      const td = document.createElement("td");
      const wave = data.values[i][j];
      const summary = Math.max(...wave.map(Math.abs)).toPrecision(3);
      const btn = document.createElement("button");
      btn.textContent = summary;
      btn.title = `index_1=${y}, index_2=${x}`;
      btn.onclick = () => {
        tbl.querySelectorAll("button.active").forEach((b) => b.classList.remove("active"));
        btn.classList.add("active");
        showWave(data, i, j);
      };
      td.appendChild(btn);
      tr.appendChild(td);
    });
    tbl.appendChild(tr);
  });
  view.appendChild(tbl);
}

function showWave(data, i, j) {
  const L = labels(data);
  document.getElementById("wave-section").classList.remove("hidden");
  document.getElementById("wave-title").textContent =
    `${data.table} wave @ ${L.index_1}=${data.index_1[i]}, ${L.index_2}=${data.index_2[j]}`;
  drawPlot("newPlot", document.getElementById("wave"), [{
    x: data.index_3, y: data.values[i][j], mode: "lines", line: { color: "#7bd88f" },
  }], {
    ...PLOT_LAYOUT,
    height: PLOT_H,
    xaxis: { title: L.index_3 },
    yaxis: { title: L.value },
  }, { responsive: true });
}

function hideWave() {
  plotRequests.delete(document.getElementById("wave"));
  document.getElementById("wave-section").classList.add("hidden");
}

// ---- raw cell source (bottom pane) -----------------------------------------
// Keep the active source and at most two million characters of recent sources.
const RECENT_SOURCE_CHARS = 2_000_000;
const recentSources = new Map();
const pendingSources = new Map();
let recentSourceChars = 0;
let activeSource = null;

function rememberSource(source) {
  if (!source?.data) return;
  const {cell, data} = source;
  recentSources.set(cell, data);
  recentSourceChars += data.text.length;
  while (recentSourceChars > RECENT_SOURCE_CHARS) {
    const [name, oldest] = recentSources.entries().next().value;
    recentSources.delete(name);
    recentSourceChars -= oldest.text.length;
  }
}

function clearActiveSource() {
  rememberSource(activeSource);
  activeSource = null;
}

async function cellSource(cell) {
  if (activeSource?.cell !== cell) {
    const data = recentSources.get(cell);
    if (data) {
      recentSources.delete(cell);
      recentSourceChars -= data.text.length;
    }
    rememberSource(activeSource);
    activeSource = {cell, data};
  }
  const source = activeSource;
  if (source.data) return source.data;
  let pending = pendingSources.get(cell);
  if (!pending) {
    pending = api(`/api/cells/${encodeURIComponent(cell)}/source`)
      .finally(() => pendingSources.delete(cell));
    pendingSources.set(cell, pending);
  }
  const data = await pending;
  if (activeSource === source) source.data = data;
  return data;
}

function _escRe(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// Return the index of the `}` matching the `{` at `open`, ignoring braces inside
// quoted strings and #/// /* */ comments.
function _matchBrace(text, open) {
  let depth = 0;
  let line = false;
  let block = false;
  let str = false;
  for (let i = open; i < text.length; i++) {
    const c = text[i];
    const n = text[i + 1];
    if (line) { if (c === "\n") line = false; continue; }
    if (block) { if (c === "*" && n === "/") { block = false; i++; } continue; }
    if (str) { if (c === "\\") i++; else if (c === '"') str = false; continue; }
    if (c === '"') { str = true; continue; }
    if (c === "#") { line = true; continue; }
    if (c === "/" && n === "/") { line = true; i++; continue; }
    if (c === "/" && n === "*") { block = true; i++; continue; }
    if (c === "{") depth++;
    else if (c === "}") { depth--; if (depth === 0) return i; }
  }
  return -1;
}

// Slice the `kind (name) { ... }` group (including its header line) out of text.
function _sliceGroup(text, kind, name) {
  const hdr = new RegExp(
    `(?:^|\\n)([ \\t]*)${_escRe(kind)}[ \\t]*\\([ \\t]*${_escRe(name)}[ \\t]*\\)[ \\t]*\\{`
  );
  const m = hdr.exec(text);
  if (!m) return null;
  const start = m.index + (text[m.index] === "\n" ? 1 : 0);
  const end = _matchBrace(text, text.indexOf("{", m.index));
  return end < 0 ? null : text.slice(start, end + 1);
}

// Slice the `indices`-th anonymous `kind (...) { ... }` group(s) out of text, in
// file order — used to trim to one exact timing()/internal_power() group.
function _sliceGroupOccurrences(text, kind, indices) {
  const re = new RegExp(`(?:^|\\n)[ \\t]*${_escRe(kind)}[ \\t]*\\([^)]*\\)[ \\t]*\\{`, "g");
  const heads = [];
  let m;
  while ((m = re.exec(text))) {
    heads.push(m.index + (text[m.index] === "\n" ? 1 : 0));
  }
  const want = new Set(indices);
  const out = [];
  for (let k = 0; k < heads.length; k++) {
    if (!want.has(k)) continue;
    const end = _matchBrace(text, text.indexOf("{", heads[k]));
    if (end >= 0) out.push(text.slice(heads[k], end + 1));
  }
  return out.length ? out.join("\n") : null;
}

// Walk a source path (steps from the cell down) and return the deepest text we
// can reach: each step slices a named `group (name)` or the indexed occurrences
// of `group (...)`. Stops at the last level that resolves (best effort).
function _walkPath(cellText, path) {
  let cur = cellText;
  for (const step of path) {
    const nxt =
      step.name != null
        ? _sliceGroup(cur, step.group, step.name)
        : _sliceGroupOccurrences(cur, step.group, step.indices);
    if (nxt == null) break;
    cur = nxt;
  }
  return cur;
}

// Keep the lib text verbatim but annotate each `index_N (...)` line with a
// trailing `// <variable>` comment dereferenced from its table template. Each
// `<group> (template) {…}` block has its own template, so the lookup is scoped
// to the innermost enclosing block (CCS `vector` groups deref correctly).
function _annotateIndices(text, templates) {
  const frames = [];
  const annotations = [];
  let active = null;
  let header = false;
  let argsStart = null;
  let vars = null;
  let lineStart = true;
  const whitespace = /\s+/y;
  const word = /(?:\\[\s\S]|\/(?![/*])|[^\\/\s,{}()[\];:])+/y;
  for (let i = 0; i < text.length;) {
    whitespace.lastIndex = i;
    const space = whitespace.exec(text);
    if (space) {
      const lastBreak = space[0].lastIndexOf("\n");
      lineStart = (lastBreak >= 0 || lineStart) && /^[ \t]*$/.test(space[0].slice(lastBreak + 1));
      i = whitespace.lastIndex;
      continue;
    }

    const c = text[i], next = text[i + 1];
    if (c === "#" || (c === "/" && next === "/")) {
      const end = text.indexOf("\n", i + 1);
      i = end < 0 ? text.length : end;
      continue;
    }
    if (c === "/" && next === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end < 0 ? text.length : end + 2;
      lineStart = false;
      continue;
    }
    if (c === "\\" && (next === "\n" || next === "\r")) {
      i++;
      continue;
    }

    let end = i + 1;
    let candidate = false;
    if (c === '"' || c === "'") {
      while (end < text.length) {
        const char = text[end++];
        if (char === "\\") end++;
        else if (char === c) break;
      }
      vars = null;
    } else if (c === "(") {
      argsStart = header ? i + 1 : null;
      vars = null;
    } else if (c === ")") {
      const template = argsStart === null ? "" : text.slice(argsStart, i).trim();
      vars = Object.hasOwn(templates, template) ? templates[template] : null;
      argsStart = null;
    } else if (c === "{") {
      frames.push(active);
      if (vars) active = {vars, closed: false};
      vars = null;
      argsStart = null;
    } else if (c === "}") {
      if (frames.length) {
        const parent = frames.pop();
        if (active !== parent) active.closed = true;
        active = parent;
      }
      vars = null;
      argsStart = null;
    } else {
      word.lastIndex = i;
      const match = word.exec(text);
      if (match) {
        end = word.lastIndex;
        const name = match[0];
        if (lineStart) {
          candidate = /^[A-Za-z_]\w*$/.test(name);
          const index = /^index_([123])$/.exec(name);
          if ((index || name === "values") && active) {
            const variable = active.vars[index ? Number(index[1]) - 1 : 0];
            if (variable) {
              const newline = text.indexOf("\n", end);
              const suffix = index ? `  // ${variable}` : `  // ------> ${variable} ----->`;
              annotations.push({frame: active, end: newline < 0 ? text.length : newline, suffix});
            }
          }
        }
      }
      vars = null;
    }
    header = candidate;
    lineStart = false;
    i = end;
  }

  // An unclosed template can occur when the source hit the server's cap.
  // Emit annotations only for complete groups, leaving every original byte.
  let start = 0;
  const parts = [];
  for (const annotation of annotations) {
    if (!annotation.frame.closed) continue;
    parts.push(text.slice(start, annotation.end), annotation.suffix);
    start = annotation.end;
  }
  parts.push(text.slice(start));
  return parts.join("");
}

function _escHtml(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// Minimal Liberty syntax highlighter -> HTML. Char-walk with string/comment
// state so braces/commas inside quotes or comments aren't mis-tokenized.
function _highlight(text) {
  const n = text.length;
  let out = "";
  let i = 0;
  const span = (cls, s) => `<span class="${cls}">${_escHtml(s)}</span>`;
  while (i < n) {
    const c = text[i];
    const d = text[i + 1];
    if (c === "#" || (c === "/" && d === "/")) {
      let j = text.indexOf("\n", i);
      if (j < 0) j = n;
      out += span("tok-com", text.slice(i, j));
      i = j;
    } else if (c === "/" && d === "*") {
      let j = text.indexOf("*/", i);
      j = j < 0 ? n : j + 2;
      out += span("tok-com", text.slice(i, j));
      i = j;
    } else if (c === '"') {
      let j = i + 1;
      while (j < n && text[j] !== '"') j += text[j] === "\\" ? 2 : 1;
      j = Math.min(j + 1, n);
      out += span("tok-str", text.slice(i, j));
      i = j;
    } else if (/[A-Za-z_]/.test(c)) {
      let j = i + 1;
      while (j < n && /[\w[\].]/.test(text[j])) j++;
      const word = text.slice(i, j);
      let k = j;
      while (k < n && (text[k] === " " || text[k] === "\t")) k++;
      out += text[k] === "(" || text[k] === ":" ? span("tok-key", word) : _escHtml(word);
      i = j;
    } else if (/[0-9]/.test(c) || (c === "-" && /[0-9.]/.test(d))) {
      let j = i + 1;
      while (j < n && /[0-9.eE+-]/.test(text[j])) j++;
      out += span("tok-num", text.slice(i, j));
      i = j;
    } else {
      out += _escHtml(c);
      i++;
    }
  }
  return out;
}

// Strip child groups (one brace level into each top-level block) whose keyword
// is in `nav` — they're shown elsewhere in the display. Single pass, brace-/
// string-/comment-aware.
function _stripNavGroups(text, nav) {
  const n = text.length;
  const parts = [];
  let copyFrom = 0;
  let i = 0;
  let depth = 0;
  while (i < n) {
    const c = text[i];
    const d = text[i + 1];
    if (c === '"') { i++; while (i < n && text[i] !== '"') i += text[i] === "\\" ? 2 : 1; i++; continue; }
    if (c === "#") { while (i < n && text[i] !== "\n") i++; continue; }
    if (c === "/" && d === "/") { while (i < n && text[i] !== "\n") i++; continue; }
    if (c === "/" && d === "*") { const j = text.indexOf("*/", i); i = j < 0 ? n : j + 2; continue; }
    if (depth === 1 && /[A-Za-z_]/.test(c)) {
      let j = i + 1;
      while (j < n && /\w/.test(text[j])) j++;
      const word = text.slice(i, j);
      let k = j;
      while (k < n && (text[k] === " " || text[k] === "\t" || text[k] === "\n")) k++;
      if (text[k] === "(" && nav.has(word)) {
        let p = k + 1;
        while (p < n && text[p] !== ")") p++;
        let q = p + 1;
        while (q < n && (text[q] === " " || text[q] === "\t" || text[q] === "\n")) q++;
        if (text[q] === "{") {
          const gEnd = _matchBrace(text, q);
          let e = gEnd < 0 ? n : gEnd + 1;
          while (e < n && (text[e] === ";" || text[e] === " " || text[e] === "\t")) e++;
          let lineStart = i;
          while (lineStart > copyFrom && (text[lineStart - 1] === " " || text[lineStart - 1] === "\t")) lineStart--;
          parts.push(text.slice(copyFrom, lineStart));
          if (text[e] === "\n") e++;
          copyFrom = e;
          i = e;
          continue;
        }
      }
      i = j;
      continue;
    }
    if (c === "{") depth++;
    else if (c === "}") depth--;
    i++;
  }
  parts.push(text.slice(copyFrom));
  return parts.join("");
}

// Parse every `{}` group: {open, close, depth (0 = top-level block), word
// (header keyword)}. Brace-/string-/comment-aware.
function _parseGroups(text) {
  const n = text.length;
  const groups = [];
  const stack = [];
  let i = 0;
  let depth = 0;
  while (i < n) {
    const c = text[i];
    const d = text[i + 1];
    if (c === '"') { i++; while (i < n && text[i] !== '"') i += text[i] === "\\" ? 2 : 1; i++; continue; }
    if (c === "#") { while (i < n && text[i] !== "\n") i++; continue; }
    if (c === "/" && d === "/") { while (i < n && text[i] !== "\n") i++; continue; }
    if (c === "/" && d === "*") { const j = text.indexOf("*/", i); i = j < 0 ? n : j + 2; continue; }
    if (c === "{") { stack.push({ open: i, level: depth }); depth++; i++; continue; }
    if (c === "}") {
      const g = stack.pop();
      depth--;
      if (g) {
        const head = text.slice(Math.max(0, g.open - 400), g.open);
        const m = /([A-Za-z_]\w*)[ \t]*\([^)]*\)[ \t\n]*$/.exec(head);
        groups.push({ open: g.open, close: i, depth: g.level, word: m ? m[1] : "" });
      }
      i++;
      continue;
    }
    i++;
  }
  return groups;
}

function _newlines(text, a, b) {
  let c = 0;
  for (let i = a; i <= b && i < text.length; i++) if (text[i] === "\n") c++;
  return c;
}

// Replace each group's `{ … }` body with a one-line placeholder. `groups` must
// be non-overlapping (outermost). Header line is preserved.
function _spliceCollapse(text, groups) {
  const gs = groups.slice().sort((a, b) => a.open - b.open);
  let out = "";
  let pos = 0;
  for (const g of gs) {
    if (g.open < pos) continue;
    out += text.slice(pos, g.open);
    out += `{ /* ${_newlines(text, g.open, g.close)} lines collapsed */ }`;
    pos = g.close + 1;
  }
  return out + text.slice(pos);
}

function _lineCount(text) {
  return text.split("\n").length;
}

// Keep the source pane under `budget` lines. First collapse undisplayed raw-data
// groups (CCSN); if still over, collapse general groups deepest-first.
function _collapseToBudget(text, budget) {
  if (_lineCount(text) <= budget) return text;

  // Pass 1: collapse undisplayed-data groups (outermost matches).
  let groups = _parseGroups(text);
  let data = groups.filter((g) => UNDISPLAYED_DATA.has(g.word));
  data = data.filter((g) => !data.some((o) => o !== g && o.open < g.open && o.close > g.close));
  if (data.length) {
    text = _spliceCollapse(text, data);
    if (_lineCount(text) <= budget) return text;
    groups = _parseGroups(text);
  }

  // Pass 2: deepest-first. Pick the largest depth T whose collapse fits.
  const total = _lineCount(text);
  const maxDepth = groups.reduce((m, g) => Math.max(m, g.depth), 0);
  for (let t = maxDepth; t >= 1; t--) {
    const cand = groups.filter((g) => g.depth === t);
    const saved = cand.reduce((s, g) => s + _newlines(text, g.open, g.close), 0);
    if (total - saved <= budget) return _spliceCollapse(text, cand);
  }

  // Fallback: collapse depth-1, then hard-truncate any remainder.
  const d1 = groups.filter((g) => g.depth === 1);
  if (d1.length) text = _spliceCollapse(text, d1);
  const lines = text.split("\n");
  if (lines.length > budget) {
    text = lines.slice(0, budget).join("\n") + `\n// … ${lines.length - budget} more lines …`;
  }
  return text;
}

// Break statements that share a line: put content after a `{` (and after a `;`)
// on its own line. The original whitespace becomes the indent. String-/comment-
// aware so `{`/`;` inside quotes or comments are left alone.
function _breakLines(text) {
  const n = text.length;
  let out = "";
  let i = 0;
  const contentAfter = (k, stopBrace) => {
    while (k < n && (text[k] === " " || text[k] === "\t")) k++;
    return k < n && text[k] !== "\n" && (!stopBrace || text[k] !== "}");
  };
  while (i < n) {
    const c = text[i];
    const d = text[i + 1];
    if (c === '"') {
      out += c; i++;
      while (i < n && text[i] !== '"') { if (text[i] === "\\") { out += text[i]; i++; } if (i < n) { out += text[i]; i++; } }
      if (i < n) { out += text[i]; i++; }
      continue;
    }
    if (c === "#" || (c === "/" && d === "/")) { while (i < n && text[i] !== "\n") { out += text[i]; i++; } continue; }
    if (c === "/" && d === "*") { const j = text.indexOf("*/", i); const e = j < 0 ? n : j + 2; out += text.slice(i, e); i = e; continue; }
    out += c;
    i++;
    if (c === "{" && contentAfter(i, false)) out += "\n";
    else if (c === ";" && contentAfter(i, true)) out += "\n";
  }
  return out;
}

async function showSource(cell, src, keep) {
  const selection = selectionVersion;
  const sec = document.getElementById("source-section");
  const title = document.getElementById("source-title");
  const pre = document.getElementById("source");
  sec.classList.remove("hidden");
  let d;
  try {
    d = await cellSource(cell);
  } catch (error) {
    if (selection !== selectionVersion) return;
    title.textContent = `source: ${cell} — error: ${error.message}`;
    pre.textContent = "";
    return;
  }
  if (selection !== selectionVersion) return;
  const path = src?.path || [];
  title.textContent = d.truncated
    ? `Source truncated (${d.length.toLocaleString()} characters in cell)`
    : "";
  const templates = (debugState.meta && debugState.meta.templates) || {};
  // Strip child groups shown elsewhere; break run-on statements; drop blanks.
  // `keep` (e.g. a collapsed multi-line node's own tables) stays in the source.
  const nav = keep && keep.length ? new Set([...NAV_GROUPS].filter((g) => !keep.includes(g))) : NAV_GROUPS;
  let text = _stripNavGroups(_walkPath(d.text, path), nav);
  text = _breakLines(text);
  text = text.split("\n").filter((l) => l.trim() !== "").join("\n");
  text = _annotateIndices(text, templates);
  // Enforce a space after every comma, except before a line-continuation "\".
  text = text.replace(/,(?![\s\\])/g, ", ");
  // Keep the pane bounded (CCSN etc. collapse first, then deepest-first).
  text = _collapseToBudget(text, SOURCE_MAX_LINES);
  pre.innerHTML = _highlight(text);
}

// ---- debug dump (only when the server runs with --dev) ---------------------
async function dumpDebug() {
  const status = document.getElementById("dump-status");
  const payload = {
    timestamp: new Date().toISOString(),
    url: location.href,
    crumb: document.getElementById("crumb").textContent,
    selected:
      document.querySelector(".node.selected > .row")?.textContent?.trim() || null,
    meta: debugState.meta,
    openCells: currentTrees(),
    lastTable: debugState.lastTable,
  };
  try {
    const res = await fetch("/api/debug", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
    const out = await res.json();
    status.textContent = `wrote ${out.path}`;
  } catch (e) {
    status.textContent = `dump failed: ${e.message}`;
  }
}

// Re-exec the server, wait for it to come back, then reload the page — picks up
// rebuilt native code / edited Python without a manual restart.
async function devRestart() {
  const status = document.getElementById("dump-status");
  status.textContent = "restarting…";
  try {
    await fetch("/api/restart", { method: "POST" });
  } catch {
    /* connection drops as the process re-execs — expected */
  }
  for (let i = 0; i < 100; i++) {
    await new Promise((r) => setTimeout(r, 300));
    try {
      const res = await fetch("/api/config", { cache: "no-store" });
      if (res.ok) {
        location.reload();
        return;
      }
    } catch {
      /* not back yet */
    }
  }
  status.textContent = "restart: server did not come back";
}

async function initDevBar() {
  try {
    const cfg = await api("/api/config");
    if (!cfg.dev) return;
    document.getElementById("devbar").classList.remove("hidden");
    document.getElementById("dump-debug").addEventListener("click", dumpDebug);
    document.getElementById("dev-reload").addEventListener("click", devRestart);
  } catch (e) {
    console.error(e);
  }
}

// ---- keyboard navigation ---------------------------------------------------
// Up/Down move the selection over visible rows; Right expands (only), Left
// collapses (only), Enter toggles. Leaves ignore expand/collapse/Enter.
function _treeKey(e) {
  if (e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA") return;
  if (!["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Enter"].includes(e.key)) return;
  const rows = [...document.querySelectorAll("#tree li.node")].filter((li) => li.offsetParent !== null);
  if (!rows.length) return;
  e.preventDefault();
  const cur = document.querySelector("#tree li.node.selected");
  const idx = cur ? rows.indexOf(cur) : -1;
  const kids = (li) => li && li.querySelector(":scope > ul.tree");
  const expanded = (li) => { const u = kids(li); return u && !u.classList.contains("hidden"); };
  const tog = (li) => li.querySelector(":scope > .row > .toggle");
  const select = (li) => {
    li.querySelector(":scope > .row").click();
    li.scrollIntoView({ block: "nearest" });
  };

  if (e.key === "ArrowDown") return select(rows[Math.min(idx + 1, rows.length - 1)] || rows[0]);
  if (e.key === "ArrowUp") return select(rows[idx <= 0 ? 0 : idx - 1]);
  if (!cur) return select(rows[0]);
  if (e.key === "ArrowRight") { if (kids(cur) && !expanded(cur)) tog(cur).click(); return; }
  if (e.key === "ArrowLeft") { if (kids(cur) && expanded(cur)) tog(cur).click(); return; }
  if (e.key === "Enter") { if (kids(cur)) tog(cur).click(); }
}
document.addEventListener("keydown", _treeKey);

// ---- copy source -----------------------------------------------------------
(function () {
  const btn = document.getElementById("source-copy");
  const copyIcon = btn.innerHTML;
  const checkIcon =
    '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>';
  btn.onclick = async () => {
    try {
      await navigator.clipboard.writeText(document.getElementById("source").textContent);
      btn.classList.add("copied");
      btn.innerHTML = checkIcon;
      setTimeout(() => { btn.classList.remove("copied"); btn.innerHTML = copyIcon; }, 1000);
    } catch (e) {
      console.error(e);
    }
  };
})();

// ---- boot ------------------------------------------------------------------
document.getElementById("filter").addEventListener("input", (e) => {
  clearTimeout(filterTimer);
  const v = e.target.value;
  const request = ++filterRequest;
  filterTimer = setTimeout(() => loadCells(v, request).catch(console.error), 200);
});

loadMeta().catch((e) => (document.getElementById("lib-name").textContent = "error: " + e.message));
loadCells("").catch((e) => console.error(e));
initDevBar();

// Let the server exit when this tab goes away: heartbeat while open, beacon on
// close. A refresh fires the beacon too, but the reloaded page re-pings inside
// the server's grace window, so it survives. (Disabled by --no-exit-on-close.)
function heartbeat() {
  // sendBeacon (not fetch) so the periodic ping doesn't spin the tab throbber,
  // which WSLg surfaces as a taskbar "attention" blip. Falls back to fetch.
  if (navigator.sendBeacon) navigator.sendBeacon("/api/ping");
  else fetch("/api/ping", { method: "POST" }).catch(() => {});
}
heartbeat();
setInterval(heartbeat, 5000);
addEventListener("pageshow", heartbeat);
// Background tabs throttle/pause the interval; re-ping the moment the tab is
// visible/focused again so the server's idle watchdog doesn't fire on return.
addEventListener("visibilitychange", () => { if (!document.hidden) heartbeat(); });
addEventListener("focus", heartbeat);
addEventListener("pagehide", () => navigator.sendBeacon("/api/bye"));
