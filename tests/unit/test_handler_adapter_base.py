import pytest

from src.api.handlers.claude.adapter import ClaudeChatAdapter


@pytest.mark.asyncio
async def test_check_endpoint_rejects_base_url_dict() -> None:
    with pytest.raises(TypeError, match="base_url must be a non-empty string"):
        await ClaudeChatAdapter.check_endpoint(
            client=None,  # type: ignore[arg-type]
            base_url={"base_url": "https://api.anthropic.com"},  # type: ignore[arg-type]
            api_key="test-key",
            request_data={
                "model": "claude-sonnet-4-5-20250929",
                "messages": [{"role": "user", "content": "hello"}],
                "max_tokens": 32,
                "stream": False,
            },
        )


def test_validate_test_base_url_trims_whitespace() -> None:
    assert ClaudeChatAdapter._validate_test_base_url("  https://api.anthropic.com/v1  ") == (
        "https://api.anthropic.com/v1"
    )
