"""Clock extraction from SDC (``create_clock`` / ``create_generated_clock``)."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from .tcl import evaluate

CREATE_VALUES = ("-name", "-period", "-waveform", "-comment")
CREATE_FLAGS = ("-add",)
GENERATED_VALUES = (
    "-name",
    "-source",
    "-master_clock",
    "-divide_by",
    "-multiply_by",
    "-edges",
    "-edge_shift",
    "-comment",
)
GENERATED_FLAGS = ("-invert", "-combinational", "-add")


@dataclass(frozen=True)
class Clock:
    """One clock definition.

    ``period`` and ``waveform`` are in the SDC's time unit (ns by convention).
    ``sources`` are symbolic endpoints: ``port:clk``, ``pin:div/Q``,
    ``net:clk``, ``cell:u1``.
    """

    name: str
    period: float | None
    waveform: tuple[float, ...] = ()
    sources: tuple[str, ...] = field(default_factory=tuple)
    generated_from: str | None = None
    divide_by: int | None = None
    multiply_by: int | None = None
    edges: tuple[float, ...] = ()
    invert: bool = False


def _split(
    words: list[str], values: tuple[str, ...], flags: tuple[str, ...]
) -> tuple[dict[str, str], set[str], list[str]]:
    """Split Tcl words into option values, boolean flags and positionals."""
    options: dict[str, str] = {}
    found_flags: set[str] = set()
    positional: list[str] = []
    index = 0
    while index < len(words):
        word = words[index]
        if word in values:
            if index + 1 >= len(words):
                raise ValueError(f"{word} requires a value")
            options[word] = words[index + 1]
            index += 2
        elif word in flags:
            found_flags.add(word)
            index += 1
        elif word.startswith("-"):
            # An option this extractor does not model; its value, if any,
            # is left alone rather than guessed.
            index += 1
        else:
            positional.append(word)
            index += 1
    return options, found_flags, positional


def _source_name(token: str) -> str:
    return token.split(":", 1)[1] if ":" in token else token


def _create_clock(words: list[str]) -> Clock:
    options, _, positional = _split(words, CREATE_VALUES, CREATE_FLAGS)
    sources = tuple(positional)
    name = options.get("-name") or (_source_name(sources[0]) if sources else "clock")
    period = float(options["-period"]) if "-period" in options else None
    waveform_text = options.get("-waveform")
    if waveform_text is not None:
        waveform = tuple(float(part) for part in waveform_text.split())
    elif period is not None:
        waveform = (0.0, period / 2.0)
    else:
        waveform = ()
    return Clock(name=name, period=period, waveform=waveform, sources=sources)


def _master_clock(options: dict[str, str], clocks: list[Clock]) -> Clock | None:
    named = options.get("-master_clock")
    if named:
        for clock in clocks:
            if clock.name == named:
                return clock
    source = options.get("-source")
    if source:
        tokens = set(source.split())
        matches = [clock for clock in clocks if tokens.intersection(clock.sources)]
        if len(matches) == 1:
            return matches[0]
    if len(clocks) == 1:
        return clocks[0]
    return None


def _create_generated_clock(words: list[str], clocks: list[Clock]) -> Clock:
    options, flags, positional = _split(words, GENERATED_VALUES, GENERATED_FLAGS)
    name = options.get("-name") or (_source_name(positional[0]) if positional else "generated")
    master = _master_clock(options, clocks)
    divide_by = int(float(options["-divide_by"])) if "-divide_by" in options else None
    multiply_by = int(float(options["-multiply_by"])) if "-multiply_by" in options else None
    period: float | None = None
    if master is not None and master.period is not None:
        if divide_by:
            period = master.period * divide_by
        elif multiply_by:
            period = master.period / multiply_by
    edges_text = options.get("-edges")
    edges = tuple(float(part) for part in edges_text.split()) if edges_text else ()
    return Clock(
        name=name,
        period=period,
        waveform=(),
        sources=tuple(positional),
        generated_from=master.name if master is not None else options.get("-master_clock"),
        divide_by=divide_by,
        multiply_by=multiply_by,
        edges=edges,
        invert="-invert" in flags,
    )


def clocks(path: Path | str) -> list[Clock]:
    """Return the clocks an SDC creates, in file order."""
    source = Path(path)
    records, error = evaluate(source)
    if error:
        raise ValueError(f"{source}: {error}")
    result: list[Clock] = []
    for words in records:
        if not words:
            continue
        command, args = words[0], words[1:]
        if command == "create_clock":
            result.append(_create_clock(args))
        elif command == "create_generated_clock":
            result.append(_create_generated_clock(args, result))
    return result
