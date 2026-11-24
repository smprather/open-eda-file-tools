"""Open the text inputs that are read on the Python side."""

from collections.abc import Buffer, Iterator
from contextlib import ExitStack, contextmanager
from gzip import GzipFile
from io import BufferedReader, RawIOBase, TextIOWrapper
from pathlib import Path

from ._native import _has_gzip_magic


class _PrefixedInput(RawIOBase):
    """Replay probe bytes if a non-seekable stream gave a short peek."""

    def __init__(self, prefix: bytes, source: BufferedReader) -> None:
        super().__init__()
        self.prefix = prefix
        self.source = source

    def readable(self) -> bool:
        return True

    def readinto(self, buffer: Buffer) -> int | None:
        if not self.prefix:
            return self.source.readinto(buffer)
        view = memoryview(buffer).cast("B")
        count = min(len(view), len(self.prefix))
        view[:count] = self.prefix[:count]
        self.prefix = self.prefix[count:]
        return count


@contextmanager
def open_text(path: Path, *, errors: str = "strict") -> Iterator[tuple[TextIOWrapper, bool]]:
    """Open a UTF-8 input and report whether its bytes are gzip-compressed."""
    with ExitStack() as stack:
        source = stack.enter_context(path.open("rb"))
        prefix = source.peek(2)
        if len(prefix) == 1:
            prefix = source.read(2)
            source = stack.enter_context(BufferedReader(_PrefixedInput(prefix, source)))
        compressed = _has_gzip_magic(prefix)
        binary = stack.enter_context(GzipFile(fileobj=source)) if compressed else source
        yield stack.enter_context(TextIOWrapper(binary, encoding="utf-8", errors=errors)), compressed
