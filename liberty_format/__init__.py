"""Liberty source formatter — strict brace-depth indentation, guaranteed to be
functionally transparent (the formatted text lexes to the exact same token
stream as the input; otherwise the formatter refuses to emit).

The reindent is line-based: each line is re-indented to its brace nesting depth,
trailing whitespace is trimmed, and runs of blank lines are collapsed. Content
inside quoted strings and comments is never touched. Whether a change is
*functionally* safe is not trusted to this logic — it is proven afterward by
comparing the Rust lexer's token stream (see :func:`format_text`).
"""

from __future__ import annotations

from collections.abc import Iterable, Iterator
from dataclasses import dataclass
from io import StringIO

from liberty_tools import _native


class TransparencyError(RuntimeError):
    """Raised when formatting would change the lexed token stream."""


@dataclass
class _LineScan:
    depth_delta: int  # net change in brace depth across the line
    first_is_close: bool  # first significant (code) char is '}' (start state normal)
    end_in_block: bool  # line ends inside a /* ... */ comment
    end_in_str: str | None  # quote char if line ends inside a string, else None


def _scan_line(s: str, in_block: bool, in_str: str | None) -> _LineScan:
    """Scan one physical line for brace depth, carrying block-comment and string
    state across the newline.

    Liberty strings may span physical lines: ``\\``+newline drops the newline,
    an unescaped newline is a literal byte. The escape (if pending at end of
    line) consumes the newline, so per-line ``escaped`` state never crosses the
    boundary — only ``in_block`` and ``in_str`` do.
    """
    depth_delta = 0
    first_is_close = False
    seen_sig = in_block or in_str is not None
    escaped = False
    i, n = 0, len(s)
    while i < n:
        c = s[i]
        if in_block:
            if c == "*" and i + 1 < n and s[i + 1] == "/":
                in_block = False
                i += 2
                continue
            i += 1
            continue
        if in_str is not None:
            if escaped:
                escaped = False
            elif c == "\\":
                escaped = True
            elif c == in_str:
                in_str = None
            i += 1
            continue
        # normal (code) state
        if c == "/" and i + 1 < n and s[i + 1] == "*":
            in_block = True
            i += 2
            continue
        if (c == "/" and i + 1 < n and s[i + 1] == "/") or c == "#":
            break  # line comment runs to end of line
        if c in '"\'':
            seen_sig = True
            in_str = c
            i += 1
            continue
        if not c.isspace():
            if not seen_sig:
                seen_sig = True
                first_is_close = c == "}"
            if c == "{":
                depth_delta += 1
            elif c == "}":
                depth_delta -= 1
        i += 1
    return _LineScan(depth_delta, first_is_close, in_block, in_str)


def _reindent_lines(lines: Iterable[str], indent: str) -> Iterator[str]:
    """Yield reindented lines, carrying the scan state across physical lines."""
    depth = 0
    in_block = False
    in_str: str | None = None
    blank_run = 0
    pending_empty = 0
    emitted = False
    for raw in lines:
        raw = raw.removesuffix("\n")
        scan = _scan_line(raw, in_block, in_str)
        if in_block or in_str is not None:
            # line begins inside a block comment or a multi-line string: every
            # byte (including leading/trailing whitespace) is significant — emit
            # it verbatim.
            line = raw
            is_blank = False
        else:
            body = raw.lstrip()
            # If the line ends inside a string, its trailing whitespace is part
            # of that string; don't trim it.
            if scan.end_in_str is None:
                body = body.rstrip()
            if body == "":
                line = ""
                is_blank = True
            else:
                eff = depth - (1 if scan.first_is_close else 0)
                line = indent * max(eff, 0) + body
                is_blank = False
        depth += scan.depth_delta
        in_block, in_str = scan.end_in_block, scan.end_in_str
        blank_run = blank_run + 1 if is_blank else 0
        if blank_run > 1:
            continue
        if not line:
            pending_empty += 1
            continue
        # Defer empty lines so the output always ends in a single newline.
        for _ in range(pending_empty):
            yield "\n"
        pending_empty = 0
        yield line + "\n"
        emitted = True
    if not emitted:
        yield "\n"


def reindent(text: str, indent: str = "  ") -> str:
    """Reindent ``text`` by brace depth. Pure whitespace/blank-line normalization."""
    return "".join(_reindent_lines(StringIO(text), indent))


def _verify_tokens(comparison: tuple[bool, int, int]) -> None:
    equal, before, after = comparison
    if not equal:
        raise TransparencyError(
            "formatting changed the token stream "
            f"({before} -> {after} tokens); refusing to emit"
        )


def format_text(text: str, indent: str = "  ") -> str:
    """Reindent ``text`` and prove the result is functionally transparent.

    Raises :class:`TransparencyError` if the formatted output does not lex to the
    same token stream as the input — the caller should then leave the file
    untouched.
    """
    formatted = reindent(text, indent)
    _verify_tokens(_native._compare_token_strings(text, formatted))
    return formatted
