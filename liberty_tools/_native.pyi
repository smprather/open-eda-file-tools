"""Type stubs for the PyO3 extension module.

The extension ships no generated stubs, so members are exposed as ``Any``
through ``__getattr__``. The typed wrapper classes in ``liberty_tools`` (and the
public ``__init__.pyi``) are the supported surface.
"""

from typing import Any

def __getattr__(name: str) -> Any: ...
