"""Evaluate Synopsys Design Constraints in a real Tcl interpreter.

SDC is freeform Tcl. A hand-written subset parser always lags variables,
``[command]`` substitution, loops and flow-generated scripts. This module runs
the file in a sandboxed ``tclsh`` whose SDC commands are stubs that record
their arguments. Unknown commands are recorded and return an empty list, so a
full SDC never aborts on a constraint we do not model yet.

Tcl >= 8.6 is an environment prerequisite (``TCLSH`` overrides discovery);
there is deliberately no static-parser fallback.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

TCL_MINIMUM = "8.6"

PRELUDE = r"""
# sdc-tools prelude: evaluate an SDC in a safe interpreter with stubbed commands.
if {[catch {package require Tcl 8.6} ::tcl_version_found]} {
    puts stderr "sdc-tools requires Tcl >= 8.6; found [info patchlevel]"
    exit 3
}

set ::records {}
proc record {args} { lappend ::records $args }

proc json_escape {text} {
    set out ""
    foreach ch [split $text ""] {
        switch -- $ch {
            "\"" { append out "\\\"" }
            "\\" { append out "\\\\" }
            "\n" { append out "\\n" }
            "\r" { append out "\\r" }
            "\t" { append out "\\t" }
            default {
                scan $ch %c code
                if {$code < 32} {
                    append out [format "\\u%04x" $code]
                } else {
                    append out $ch
                }
            }
        }
    }
    return $out
}

set ::slave [interp create -safe]
interp alias $::slave record {} record
$::slave eval {
    proc unknown {args} { ::record unknown {*}$args; return {} }
    proc collect {kind args} {
        set out {}
        foreach arg $args {
            foreach word $arg { lappend out "$kind:$word" }
        }
        return $out
    }
    proc get_ports {args} { collect port {*}$args }
    proc get_pins {args} { collect pin {*}$args }
    proc get_nets {args} { collect net {*}$args }
    proc get_cells {args} { collect cell {*}$args }
    proc get_clocks {args} { collect clock {*}$args }
    proc create_clock {args} { ::record create_clock {*}$args; return {} }
    proc create_generated_clock {args} { ::record create_generated_clock {*}$args; return {} }
}

set ::failure ""
set ::path $::env(SDC_FILE)
if {[catch {
    set fh [open $::path r]
    fconfigure $fh -encoding utf-8
    set script [read $fh]
    close $fh
    $::slave eval $script
} ::failure]} {
    # Collected records survive a script error; the caller decides.
}

puts -nonewline "{\"records\":\["
set ::first 1
foreach rec $::records {
    if {!$::first} { puts -nonewline "," }
    set ::first 0
    puts -nonewline "\["
    set ::second 1
    foreach word $rec {
        if {!$::second} { puts -nonewline "," }
        set ::second 0
        puts -nonewline "\"[json_escape $word]\""
    }
    puts -nonewline "\]"
}
puts -nonewline "\],\"error\":\"[json_escape $::failure]\"}"
puts ""
exit 0
"""


class SdcError(ValueError):
    """An SDC could not be evaluated."""


def find_tclsh() -> str:
    """Locate a Tcl interpreter; ``TCLSH`` wins over PATH discovery."""
    override = os.environ.get("TCLSH")
    if override:
        return override
    for name in ("tclsh8.6", "tclsh"):
        found = shutil.which(name)
        if found:
            return found
    raise SdcError(
        f"sdc-tools requires Tcl >= {TCL_MINIMUM} (tclsh); "
        "install it or set TCLSH to its path"
    )


def evaluate(path: Path) -> tuple[list[list[str]], str | None]:
    """Run ``path`` and return ``(records, error)``.

    Each record is the command name followed by its Tcl words, with
    collections represented symbolically (``port:clk``, ``pin:u/CLK``, ...).
    """
    tclsh = find_tclsh()
    env = dict(os.environ, SDC_FILE=str(path))
    try:
        proc = subprocess.run(
            [tclsh, "-"],
            input=PRELUDE,
            text=True,
            encoding="utf-8",
            capture_output=True,
            env=env,
            check=False,
        )
    except OSError as exc:
        raise SdcError(f"cannot run {tclsh}: {exc}") from exc
    if proc.returncode == 3:
        raise SdcError(proc.stderr.strip() or f"Tcl >= {TCL_MINIMUM} required")
    if proc.returncode != 0:
        detail = proc.stderr.strip() or f"exit status {proc.returncode}"
        raise SdcError(f"{path}: tclsh failed: {detail}")
    try:
        payload = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise SdcError(f"{path}: unexpected tclsh output: {proc.stdout[:200]!r}") from exc
    return payload["records"], payload.get("error") or None
