"""OpenAI handler package (lazy exports)."""

from __future__ import annotations

from importlib import import_module
from typing import Any

_LAZY_EXPORTS: dict[str, tuple[str, str]] = {
    "OpenAIChatAdapter": (".adapter", "OpenAIChatAdapter"),
    "OpenAIChatHandler": (".handler", "OpenAIChatHandler"),
    "OpenAIVideoAdapter": (".video_adapter", "OpenAIVideoAdapter"),
    "OpenAIVideoHandler": (".video_handler", "OpenAIVideoHandler"),
    "OpenAIImageAdapter": (".image_adapter", "OpenAIImageAdapter"),
    "OpenAIImageHandler": (".image_handler", "OpenAIImageHandler"),
}

__all__ = list(_LAZY_EXPORTS.keys())


def __getattr__(name: str) -> Any:
    if name not in _LAZY_EXPORTS:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")

    module_name, attr_name = _LAZY_EXPORTS[name]
    module = import_module(module_name, __name__)
    value = getattr(module, attr_name)
    globals()[name] = value
    return value
