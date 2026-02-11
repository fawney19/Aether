import pytest

from src.utils import json_helper as json


def test_dumps_and_loads_roundtrip() -> None:
    obj = {"a": 1, "b": "中文", "c": [1, 2, 3]}
    s = json.dumps(obj, ensure_ascii=False)
    assert isinstance(s, str)
    assert json.loads(s) == obj


def test_dumps_ensure_ascii_true_escapes_unicode() -> None:
    s = json.dumps({"x": "中文"})
    assert "\\u" in s


def test_loads_invalid_raises_jsondecodeerror() -> None:
    with pytest.raises(json.JSONDecodeError):
        json.loads("{bad")


def test_dumps_bytes_returns_bytes() -> None:
    b = json.dumps_bytes({"a": 1}, ensure_ascii=False)
    assert isinstance(b, (bytes,))
    assert json.loads(b) == {"a": 1}
