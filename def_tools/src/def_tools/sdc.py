from __future__ import annotations

from pathlib import Path

ClockRoot = tuple[str, str, str]


def clock_roots_from_sdc(path: Path) -> list[ClockRoot]:
    """Extract create_clock/create_generated_clock roots from SDC Tcl stubs."""
    roots: list[ClockRoot] = []
    for command in _split_commands(path.read_text(encoding="utf-8")):
        tokens = _tokenize(command)
        if not tokens:
            continue
        cmd = tokens[0]
        if cmd not in {"create_clock", "create_generated_clock"}:
            continue
        name = _option_value(tokens, "-name")
        endpoints: list[tuple[str, str]] = []
        for token in tokens[1:]:
            endpoints.extend(_eval_collection(token))
        if not endpoints:
            continue
        for endpoint_type, endpoint_name in endpoints:
            roots.append((name or endpoint_name, endpoint_type, endpoint_name))
    return roots


def _split_commands(text: str) -> list[str]:
    commands: list[str] = []
    current: list[str] = []
    brace_depth = 0
    bracket_depth = 0
    in_quote = False
    escaped = False
    in_comment = False

    for char in text:
        if in_comment:
            if char == "\n":
                in_comment = False
                command = "".join(current).strip()
                if command:
                    commands.append(command)
                current = []
            continue
        if escaped:
            current.append(char)
            escaped = False
            continue
        if char == "\\":
            escaped = True
            current.append(char)
            continue
        if char == '"' and brace_depth == 0:
            in_quote = not in_quote
            current.append(char)
            continue
        if not in_quote:
            if char == "{":
                brace_depth += 1
            elif char == "}" and brace_depth > 0:
                brace_depth -= 1
            elif char == "[":
                bracket_depth += 1
            elif char == "]" and bracket_depth > 0:
                bracket_depth -= 1
            elif char == "#" and brace_depth == 0 and bracket_depth == 0:
                in_comment = True
                continue
            elif char in {"\n", ";"} and brace_depth == 0 and bracket_depth == 0:
                command = "".join(current).strip()
                if command:
                    commands.append(command)
                current = []
                continue
        current.append(char)

    command = "".join(current).strip()
    if command:
        commands.append(command)
    return commands


def _tokenize(command: str) -> list[str]:
    tokens: list[str] = []
    current: list[str] = []
    brace_depth = 0
    bracket_depth = 0
    in_quote = False

    for char in command:
        if char == '"' and brace_depth == 0 and bracket_depth == 0:
            in_quote = not in_quote
            continue
        if not in_quote:
            if char == "{":
                brace_depth += 1
                if brace_depth == 1:
                    continue
            elif char == "}" and brace_depth > 0:
                brace_depth -= 1
                if brace_depth == 0:
                    continue
            elif char == "[":
                bracket_depth += 1
            elif char == "]" and bracket_depth > 0:
                bracket_depth -= 1
            elif char.isspace() and brace_depth == 0 and bracket_depth == 0:
                if current:
                    tokens.append("".join(current))
                    current = []
                continue
        current.append(char)

    if current:
        tokens.append("".join(current))
    return tokens


def _option_value(tokens: list[str], option: str) -> str | None:
    try:
        index = tokens.index(option)
    except ValueError:
        return None
    if index + 1 >= len(tokens):
        return None
    return tokens[index + 1]


def _eval_collection(token: str) -> list[tuple[str, str]]:
    if not (token.startswith("[") and token.endswith("]")):
        return []
    inner = token[1:-1].strip()
    parts = _tokenize(inner)
    if len(parts) < 2:
        return []
    command, values = parts[0], parts[1:]
    if command == "get_ports":
        return [("port", value) for value in values]
    if command == "get_pins":
        return [("pin", value) for value in values]
    return []
