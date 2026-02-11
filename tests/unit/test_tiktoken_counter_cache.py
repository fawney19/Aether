import pytest


class _DummyEncoder:
    def encode(self, text: str) -> list[int]:
        return [0] * len(text)


class _DummyTiktoken:
    def __init__(self, calls: dict[str, int]) -> None:
        self._calls = calls

    def get_encoding(self, _name: str) -> _DummyEncoder:
        self._calls["get_encoding"] += 1
        return _DummyEncoder()

    def encoding_for_model(self, _model: str) -> _DummyEncoder:
        self._calls["encoding_for_model"] += 1
        return _DummyEncoder()


@pytest.mark.asyncio
async def test_encoder_is_globally_cached(monkeypatch: pytest.MonkeyPatch) -> None:
    import src.plugins.token.tiktoken_counter as tc

    # 清理缓存，避免受其他测试影响
    tc._get_encoder_cached.cache_clear()

    calls = {"get_encoding": 0, "encoding_for_model": 0}

    monkeypatch.setattr(tc, "TIKTOKEN_AVAILABLE", True)
    monkeypatch.setattr(tc, "tiktoken", _DummyTiktoken(calls))

    p1 = tc.TiktokenCounterPlugin()
    p2 = tc.TiktokenCounterPlugin()

    # 两个实例对同一 model 请求编码器，底层 get_encoding 应只触发一次
    await p1.count_tokens("hi", model="gpt-4")
    await p2.count_tokens("hi", model="gpt-4")

    assert calls["get_encoding"] == 1
