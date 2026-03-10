import pytest

import src.core.api_format.metadata as metadata
from src.core.api_format.enums import ApiFamily, EndpointKind
from src.core.api_format.metadata import EndpointDefinition, get_default_body_rules_for_endpoint


def test_get_default_body_rules_for_endpoint_returns_empty_for_invalid() -> None:
    assert get_default_body_rules_for_endpoint("not-a-valid-signature") == []


def test_get_default_body_rules_for_endpoint_returns_empty_for_no_rules() -> None:
    """claude:chat 没有配置 default_body_rules，应返回空列表。"""
    assert get_default_body_rules_for_endpoint("claude:chat") == []


def test_get_default_body_rules_for_endpoint_returns_codex_cli_rules_only() -> None:
    """只有 codex + openai:cli 返回默认请求体规则。"""
    cli_rules = get_default_body_rules_for_endpoint("openai:cli", provider_type="codex")
    assert len(cli_rules) == 5
    actions = [r["action"] for r in cli_rules]
    assert actions == ["drop", "drop", "drop", "set", "set"]
    assert cli_rules[0]["path"] == "max_output_tokens"
    assert cli_rules[1]["path"] == "temperature"
    assert cli_rules[2]["path"] == "top_p"
    assert cli_rules[3] == {"action": "set", "path": "store", "value": False}
    assert cli_rules[4]["path"] == "instructions"
    assert cli_rules[4]["condition"]["op"] == "not_exists"

    # openai:compact also has the same default body rules (defined in EndpointDefinition)
    compact_rules = get_default_body_rules_for_endpoint("openai:compact", provider_type="codex")
    assert len(compact_rules) == 5


def test_get_default_body_rules_for_endpoint_returns_deep_copy(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    definition = EndpointDefinition(
        api_family=ApiFamily.OPENAI,
        endpoint_kind=EndpointKind.CLI,
        default_body_rules=(
            {
                "action": "set",
                "path": "metadata",
                "value": {"source": "default"},
            },
        ),
    )

    monkeypatch.setattr(metadata, "resolve_endpoint_definition", lambda _value: definition)

    rules = get_default_body_rules_for_endpoint("openai:cli")
    assert rules == [
        {
            "action": "set",
            "path": "metadata",
            "value": {"source": "default"},
        }
    ]

    rules[0]["value"]["source"] = "changed"
    assert definition.default_body_rules[0]["value"]["source"] == "default"
