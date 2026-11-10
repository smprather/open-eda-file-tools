"""``liberty-view`` CLI — serve the browser-based Liberty data viewer.

Run: ``uv run liberty-view [LIBERTY_FILE] [--port N]`` (or, once installed with
``uv tool install .``, just ``liberty-view``).
"""

from __future__ import annotations

from importlib.metadata import PackageNotFoundError, version
import os
import socket
import sys
import threading
import time
import urllib.error
import urllib.request
import webbrowser

import click
import uvicorn


def _package_version() -> str:
    try:
        return version("open-eda-file-tools")
    except PackageNotFoundError:
        return "0.0.0+unknown"


def _pick_port(host: str, start: int, span: int = 64) -> int:
    """Return the first free TCP port at or after ``start`` (scanning ``span``).

    The requested port is often already taken (a previous viewer still running),
    so walk forward instead of failing with ``address already in use``. Detect a
    free port by probing with ``connect_ex``: a refused connection (non-zero)
    means nothing is listening there.
    """
    for port in range(start, start + span):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            if s.connect_ex((host, port)) != 0:
                return port
    raise click.ClickException(
        f"no free port in {start}..{start + span - 1} on {host}"
    )


def _open_when_ready(url: str, timeout: float = 30.0) -> None:
    """Wait for the data API to answer, then open the browser.

    ``uvicorn.run`` blocks, so this runs in a daemon thread. We probe
    ``/api/meta`` rather than ``/`` so the browser opens only once the *data*
    layer is live — the static shell is served before ``LibraryIndex.open`` has
    run, so opening on ``/`` lets the page fire its first ``/api/*`` calls into a
    not-quite-ready server and surface a transport "NetworkError". Probing
    ``/api/meta`` both confirms the API works and warms ``_data`` so the browser's
    own first request is instant. On Linux/WSL the default ``webbrowser.open`` can
    resolve to a non-GUI handler, so force ``xdg-open`` via a BackgroundBrowser.
    """
    deadline = time.monotonic() + timeout
    probe = url.rstrip("/") + "/api/meta"
    while time.monotonic() < deadline:
        try:
            urllib.request.urlopen(probe, timeout=1.0)  # noqa: S310 (localhost)
            break
        except urllib.error.HTTPError:
            break  # any HTTP status means the server is up and answering
        except OSError:
            time.sleep(0.15)
    else:
        return  # never came up; nothing to open
    if sys.platform.startswith("linux"):
        webbrowser.BackgroundBrowser("xdg-open").open(url)
    else:
        webbrowser.open(url)


@click.command()
@click.argument(
    "liberty_file",
    default="dev.lib",
    type=click.Path(dir_okay=False),
)
@click.option("--host", default="127.0.0.1", show_default=True, help="Bind address.")
@click.option("--port", default=8000, show_default=True, type=int, help="TCP port.")
@click.option(
    "--reload",
    is_flag=True,
    help="Auto-reload on source changes (development).",
)
@click.option(
    "--open-browser/--no-open-browser",
    default=True,
    help="Open the viewer in the default browser once the server is up (default: true).",
)
@click.option(
    "--dev",
    is_flag=True,
    help="Add a Dump Debug button that writes the page state to "
    "/tmp/liberty_view_debug.json for inspection.",
)
@click.option(
    "--access-log/--no-access-log",
    default=False,
    help="Print the web server's per-request activity log (default: off).",
)
@click.option(
    "--exit-on-close/--no-exit-on-close",
    default=True,
    help="Shut the server down shortly after the browser tab closes (default: true).",
)
@click.version_option(
    version=_package_version(),
    prog_name="liberty-view",
    message="%(prog)s %(version)s",
)
def main(
    liberty_file: str,
    host: str,
    port: int,
    reload: bool,
    open_browser: bool,
    dev: bool,
    exit_on_close: bool,
    access_log: bool,
) -> None:
    """Browse a Liberty (.lib or .lib.gz) library in the browser.

    Sidebar tree of cells → pins → timing/power arcs → tables; scalars render
    as tables, 1-D as line plots, 2-D as heatmap + surface, 3-D/CCS as a
    clickable grid that opens a wave plot. The server is cell/leaf-scoped, so
    the browser never loads the whole file at once.
    """
    os.environ["LIBERTY_FILE"] = liberty_file
    if dev:
        os.environ["LIBERTY_DEV"] = "1"
    if exit_on_close:
        os.environ["LIBERTY_EXIT_ON_CLOSE"] = "1"
    # 0.0.0.0 / "" are bind-all; point the browser and the probe at loopback.
    view_host = "127.0.0.1" if host in ("0.0.0.0", "") else host
    requested = port
    port = _pick_port(view_host, port)
    if port != requested:
        click.secho(f"port {requested} busy; using {port}", fg="yellow")
    url = f"http://{view_host}:{port}"

    click.secho(f"Serving {liberty_file} at ", nl=False)
    click.secho(url, fg="cyan", bold=True)

    if open_browser:
        threading.Thread(target=_open_when_ready, args=(url,), daemon=True).start()

    uvicorn.run(
        "viewer.server:app", host=host, port=port, reload=reload, access_log=access_log
    )


if __name__ == "__main__":
    main()
