"""FastAPI backend for the Liberty viewer.

Run:
    LIBERTY_FILE=dev.lib uv run uvicorn viewer.server:app --reload

Endpoints are deliberately cell-scoped and leaf-scoped so the browser never
pulls more than one table at a time — the basis for tolerating multi-GB files.
"""

from __future__ import annotations

import asyncio
import json
import os
import signal
import sys
import time
from contextlib import asynccontextmanager
from pathlib import Path
from threading import Lock

from fastapi import FastAPI, HTTPException, Query, Request
from fastapi.responses import FileResponse, Response
from fastapi.staticfiles import StaticFiles

from .data import LibertyData

STATIC_DIR = Path(__file__).parent / "static"
# Fixed, predictable path so "check the debug dump" always means this file.
DEBUG_DUMP = Path("/tmp/liberty_view_debug.json")

# Exit-when-tab-closed watchdog. The client heartbeats `/api/ping`; on tab close
# it sends a `/api/bye` beacon. We exit once `_deadline` passes — heartbeats push
# it forward, `bye` pulls it in (but leaves a grace window so a refresh, which
# also fires the beacon, can re-ping and cancel the shutdown). No websockets.
# Background tabs throttle setInterval (often to <=1/min, sometimes paused), so
# the idle timeout must be comfortably longer than that or switching away from
# the tab for a bit looks like a "random" shutdown. `bye` still exits promptly
# on a real tab close.
_IDLE_TIMEOUT = 90.0  # no heartbeat for this long -> client assumed gone
_CLOSE_GRACE = 4.0  # after a `bye`, wait this long for a reload to re-ping
_deadline: float | None = None  # monotonic exit time; None = watchdog disabled
_last_ping: float | None = None  # monotonic of last heartbeat (for --dev logs)


def _dev_enabled() -> bool:
    return os.environ.get("LIBERTY_DEV") == "1"


def _log(msg: str) -> None:
    if _dev_enabled():
        print(f"[viewer] {msg}", flush=True)


def _bump(seconds: float) -> None:
    global _deadline
    if _deadline is not None:
        _deadline = time.monotonic() + seconds


async def _watchdog() -> None:
    while True:
        await asyncio.sleep(1.0)
        if _deadline is not None and time.monotonic() > _deadline:
            last = "never" if _last_ping is None else f"{time.monotonic() - _last_ping:.1f}s ago"
            _log(f"watchdog fired (last ping {last}, idle timeout {_IDLE_TIMEOUT}s) -> SIGINT")
            # SIGINT -> uvicorn graceful shutdown (same as Ctrl-C).
            signal.raise_signal(signal.SIGINT)
            return


@asynccontextmanager
async def lifespan(app: FastAPI):
    global _deadline
    task = None
    if os.environ.get("LIBERTY_EXIT_ON_CLOSE") == "1":
        _deadline = time.monotonic() + _IDLE_TIMEOUT  # grace for first page load
        task = asyncio.create_task(_watchdog())
        _log(f"exit-on-close watchdog armed (idle timeout {_IDLE_TIMEOUT}s)")
    yield
    if task:
        task.cancel()


app = FastAPI(title="Liberty Viewer", lifespan=lifespan)
_data: LibertyData | None = None
_data_lock = Lock()


# The app shell + our own JS/CSS, in addition to all /api/ JSON. `plotly.min.js`
# is deliberately omitted (large, immutable) so it stays cacheable.
_NO_STORE_PATHS = {"/", "/index.html", "/app.js", "/symbol.js", "/style.css"}


@app.middleware("http")
async def no_store(request: Request, call_next):
    """Never let the browser cache API JSON or the frontend assets — payloads and
    the JS/HTML change whenever the lib or viewer is rebuilt, and a stale cached
    copy silently hides edits (e.g. a new bottom pane that never appears)."""
    response = await call_next(request)
    path = request.url.path
    if path.startswith("/api/") or path in _NO_STORE_PATHS:
        response.headers["Cache-Control"] = "no-store"
    return response


def get_data() -> LibertyData:
    global _data
    data = _data
    if data is not None:
        return data
    with _data_lock:
        if _data is None:
            path = os.environ.get("LIBERTY_FILE", "dev.lib")
            if not Path(path).exists():
                raise HTTPException(500, f"LIBERTY_FILE not found: {path}")
            _data = LibertyData.load(path)
        return _data


@app.get("/api/ready")
def ready():
    get_data()
    return Response(status_code=204)


@app.get("/api/meta")
def meta():
    return get_data().meta()


@app.get("/api/cells")
def cells(
    filter: str | None = None,
    offset: int = 0,
    limit: int = Query(500, le=5000),
):
    return get_data().cell_names(filter, offset, limit)


@app.get("/api/cells/{cell}")
def cell_tree(cell: str):
    try:
        return get_data().cell_tree(cell)
    except KeyError:
        raise HTTPException(404, f"unknown cell {cell!r}")


@app.get("/api/cells/{cell}/source")
def cell_source(cell: str):
    try:
        return get_data().cell_source(cell)
    except (KeyError, ValueError):
        raise HTTPException(404, f"unknown cell {cell!r}")


@app.get("/api/table")
def table(
    cell: str,
    pin: str,
    group: str,
    arc_index: int,
    table: str,
    container: str = "",
):
    try:
        return get_data().table(cell, pin, group, arc_index, table, container)
    except (KeyError, IndexError) as exc:
        raise HTTPException(404, f"table not found: {exc}")


@app.post("/api/ping")
def ping():
    """Client heartbeat — keeps the exit-on-close watchdog from firing. POST so
    the client can use navigator.sendBeacon (no fetch throbber that blips the
    WSLg taskbar)."""
    global _last_ping
    now = time.monotonic()
    if _dev_enabled():
        _log(f"ping ({'first' if _last_ping is None else f'+{now - _last_ping:.1f}s'})")
    _last_ping = now
    _bump(_IDLE_TIMEOUT)
    return {"ok": True}


@app.post("/api/bye")
async def bye():
    """Tab-close beacon — pull the exit deadline in to a short grace window (a
    refresh fires this too, but its reload re-pings before the grace elapses)."""
    _log(f"bye beacon -> grace {_CLOSE_GRACE}s")
    _bump(_CLOSE_GRACE)
    return {"ok": True}


@app.get("/api/config")
def config():
    """Client boot config. ``dev`` toggles the in-page Dump Debug button."""
    return {"dev": _dev_enabled()}


def _reexec() -> None:
    # Replace this process with a fresh `python -m viewer <same args>`, so a
    # rebuilt native extension / edited Python is picked up. Listening sockets
    # are non-inheritable (PEP 446) and close here, freeing the port to re-bind.
    os.execv(sys.executable, [sys.executable, "-m", "viewer", *sys.argv[1:]])


@app.post("/api/restart")
async def restart():
    """Dev-only: re-exec the server so updated code takes effect. The client
    polls until it's back, then reloads the page."""
    if not _dev_enabled():
        raise HTTPException(404, "restart disabled (run with --dev)")
    # Re-exec after the response is flushed (replacing the process never returns).
    asyncio.get_running_loop().call_later(0.25, _reexec)
    return {"ok": True}


@app.post("/api/debug")
async def debug_dump(request: Request):
    """Persist the client's current state to ``/tmp/liberty_view_debug.json`` so
    it can be inspected out-of-band (overwriting any prior dump). Only enabled
    under ``--dev`` to keep a file-writing endpoint off by default."""
    if not _dev_enabled():
        raise HTTPException(404, "debug dump disabled (run with --dev)")
    state = await request.json()
    DEBUG_DUMP.write_text(json.dumps(state, indent=2))
    return {"ok": True, "path": str(DEBUG_DUMP)}


@app.get("/")
def index():
    return FileResponse(STATIC_DIR / "index.html")


app.mount("/", StaticFiles(directory=STATIC_DIR), name="static")
