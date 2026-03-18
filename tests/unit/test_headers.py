import json

from src.api.handlers.base.request_builder import (
    evaluate_condition,
    summarize_request_payload_shape,
)
from src.core.api_format import (
    CORE_REDACT_HEADERS,
    HeaderBuilder,
    build_upstream_headers_for_endpoint,
    detect_capabilities_for_endpoint,
    extract_client_api_key_for_endpoint,
    filter_response_headers,
    get_header_value,
    normalize_headers,
    redact_headers_for_log,
)
from src.services.capability.resolver import CapabilityResolver


class TestNormalizeHeaders:
    def test_lowercases_keys(self) -> None:
        result = normalize_headers({"Authorization": "Bearer x", "X-API-Key": "y"})
        assert result == {"authorization": "Bearer x", "x-api-key": "y"}

    def test_last_wins_on_case_collision(self) -> None:
        # 同一 header 不同大小写同时存在时，normalize 会按 dict 迭代顺序“后者覆盖前者”。
        result = normalize_headers({"A": "1", "a": "2"})
        assert result == {"a": "2"}


class TestGetHeaderValue:
    def test_case_insensitive_lookup(self) -> None:
        headers = {"X-Require-Capability": "context_1m"}
        assert get_header_value(headers, "x-require-capability") == "context_1m"

    def test_default_when_missing(self) -> None:
        assert get_header_value({"a": "1"}, "missing", default="d") == "d"


class TestExtractClientApiKey:
    def test_bearer_token(self) -> None:
        headers = {"authorization": "Bearer test-token"}
        assert extract_client_api_key_for_endpoint(headers, "openai:chat") == "test-token"

    def test_bearer_token_requires_prefix(self) -> None:
        headers = {"Authorization": "test-token"}
        assert extract_client_api_key_for_endpoint(headers, "openai:chat") is None

    def test_header_auth(self) -> None:
        headers = {"X-API-Key": "abc"}
        assert extract_client_api_key_for_endpoint(headers, "claude:chat") == "abc"


class TestDetectCapabilities:
    def test_claude_context_1m(self) -> None:
        headers = {"Anthropic-Beta": "context-1m,foo"}
        assert detect_capabilities_for_endpoint(headers, "claude:chat") == {"context_1m": True}

    def test_non_claude_noop(self) -> None:
        headers = {"Anthropic-Beta": "context-1m"}
        assert detect_capabilities_for_endpoint(headers, "openai:chat") == {}


class TestHeaderBuilder:
    def test_case_insensitive_uniqueness(self) -> None:
        builder = HeaderBuilder()
        builder.add("Authorization", "a")
        builder.add("authorization", "b")
        built = builder.build()
        assert len(built) == 1
        assert built["Authorization"] == "b"
        assert "authorization" not in built

    def test_add_protected_does_not_override(self) -> None:
        builder = HeaderBuilder()
        builder.add("Authorization", "base")
        builder.add_protected({"authorization": "override", "X-Test": "1"}, {"Authorization"})
        built = builder.build()
        assert built["Authorization"] == "base"
        assert built["X-Test"] == "1"

    def test_non_ascii_value_is_escaped_for_httpx(self) -> None:
        builder = HeaderBuilder()
        builder.add("X-Test", "桌面")
        built = builder.build()
        assert built["X-Test"] == r"\u684c\u9762"

    def test_codex_turn_metadata_is_normalized_to_ascii_json(self) -> None:
        builder = HeaderBuilder()
        raw = '{"turn_id":"t1","workspaces":{"d:\\\\桌面\\\\123\\\\Aether":{"has_changes":true}}}'
        builder.add("x-codex-turn-metadata", raw)
        built = builder.build()

        normalized = built["x-codex-turn-metadata"]
        assert normalized.isascii()
        parsed = json.loads(normalized)
        assert "d:\\桌面\\123\\Aether" in parsed["workspaces"]

    def test_apply_rules_supports_nested_conditions(self) -> None:
        builder = HeaderBuilder()
        builder.apply_rules(
            [
                {
                    "action": "set",
                    "key": "X-Flag",
                    "value": "1",
                    "condition": {
                        "all": [
                            {"path": "metadata.mode", "op": "eq", "value": "prod"},
                            {
                                "any": [
                                    {"path": "tier", "op": "eq", "value": "gold"},
                                    {"path": "tier", "op": "eq", "value": "silver"},
                                ]
                            },
                        ]
                    },
                }
            ],
            body={"metadata": {"mode": "prod"}, "tier": "silver"},
            condition_evaluator=evaluate_condition,
        )
        assert builder.build()["X-Flag"] == "1"

    def test_apply_rules_supports_original_source(self) -> None:
        builder = HeaderBuilder()
        builder.apply_rules(
            [
                {
                    "action": "set",
                    "key": "X-Original",
                    "value": "yes",
                    "condition": {
                        "path": "metadata.mode",
                        "op": "eq",
                        "value": "prod",
                        "source": "original",
                    },
                }
            ],
            body={"metadata": {"mode": "test"}},
            original_body={"metadata": {"mode": "prod"}},
            condition_evaluator=evaluate_condition,
        )
        assert builder.build()["X-Original"] == "yes"

    def test_apply_rules_fail_closed_without_body_or_evaluator(self) -> None:
        builder = HeaderBuilder()
        builder.apply_rules(
            [
                {
                    "action": "set",
                    "key": "X-Skip",
                    "value": "1",
                    "condition": {"path": "flag", "op": "eq", "value": True},
                }
            ]
        )
        assert "X-Skip" not in builder.build()


class TestBuildUpstreamHeaders:
    def test_priority_and_drop_headers(self) -> None:
        result = build_upstream_headers_for_endpoint(
            {
                "Host": "example.com",
                "X-Api-Key": "client",
                "User-Agent": "ua",
                "Content-Type": "text/plain",
                "Content-Encoding": "gzip",
            },
            "openai:chat",
            "provider",
            endpoint_headers={"Authorization": "bad", "X-Endpoint": "1", "Content-Type": "bad"},
            extra_headers={"User-Agent": "extra", "X-Extra": "1"},
        )
        assert "Host" not in result
        assert "X-Api-Key" not in result
        assert result["Authorization"] == "Bearer provider"
        assert result["User-Agent"] == "extra"
        assert result["Content-Type"] == "text/plain"
        assert "Content-Encoding" not in result
        assert result["X-Endpoint"] == "1"
        assert result["X-Extra"] == "1"

    def test_drop_headers_empty_does_not_fallback(self) -> None:
        result = build_upstream_headers_for_endpoint(
            {"Host": "example.com"},
            "openai:chat",
            "provider",
            drop_headers=frozenset(),
        )
        assert result["Host"] == "example.com"
        assert result["Authorization"] == "Bearer provider"

    def test_no_duplicate_on_case_variants(self) -> None:
        result = build_upstream_headers_for_endpoint(
            {"user-agent": "a"},
            "openai:chat",
            "provider",
            extra_headers={"User-Agent": "b"},
        )
        assert len([k for k in result if k.lower() == "user-agent"]) == 1
        assert result["user-agent"] == "b"
        assert "User-Agent" not in result

    def test_default_content_type(self) -> None:
        result = build_upstream_headers_for_endpoint({}, "openai:chat", "provider")
        assert result["Content-Type"] == "application/json"

    def test_header_rules_can_use_condition_against_body(self) -> None:
        result = build_upstream_headers_for_endpoint(
            {},
            "openai:chat",
            "provider",
            header_rules=[
                {
                    "action": "set",
                    "key": "X-Conditional",
                    "value": "1",
                    "condition": {"path": "mode", "op": "eq", "value": "prod"},
                }
            ],
            body={"mode": "prod"},
            condition_evaluator=evaluate_condition,
        )
        assert result["X-Conditional"] == "1"


class TestFilterResponseHeaders:
    def test_drops_hop_by_hop_and_body_dependent_headers(self) -> None:
        headers = {
            "Content-Length": "1",
            "Transfer-Encoding": "chunked",
            "Connection": "keep-alive",
            "Content-Type": "application/json",
            "X-Test": "1",
        }
        assert filter_response_headers(headers) == {"X-Test": "1"}


class TestRedactHeadersForLog:
    def test_redacts_core_sensitive_headers_case_insensitively(self) -> None:
        headers = {"Authorization": "secret", "X-Api-Key": "secret2", "X-Test": "1"}
        redacted = redact_headers_for_log(headers, CORE_REDACT_HEADERS)
        assert redacted["Authorization"] == "***"
        assert redacted["X-Api-Key"] == "***"
        assert redacted["X-Test"] == "1"


class TestCapabilityResolverHeaderParsing:
    def test_x_require_capability_case_insensitive(self) -> None:
        reqs = CapabilityResolver.resolve_requirements(
            request_headers={"x-require-capability": "context_1m"}
        )
        assert reqs == {"context_1m": True}


class TestAuthHeaderCasePreservation:
    def test_build_upstream_headers_preserves_lowercase_authorization_key(self) -> None:
        result = build_upstream_headers_for_endpoint(
            {"authorization": "Bearer client-token", "X-Test": "1"},
            "openai:chat",
            "provider",
        )
        assert "authorization" in result
        assert "Authorization" not in result
        assert result["authorization"] == "Bearer provider"

    def test_passthrough_request_builder_preserves_lowercase_authorization_key(self) -> None:
        from types import SimpleNamespace

        from src.api.handlers.base.request_builder import PassthroughRequestBuilder

        builder = PassthroughRequestBuilder()
        endpoint = SimpleNamespace(api_family="openai", endpoint_kind="cli", header_rules=None)
        key = SimpleNamespace(api_key="unused")

        headers = builder.build_headers(
            original_headers={"authorization": "Bearer client-token"},
            endpoint=endpoint,
            key=key,
            pre_computed_auth=("Authorization", "Bearer provider-token"),
        )

        assert "authorization" in headers
        assert "Authorization" not in headers
        assert headers["authorization"] == "Bearer provider-token"


class TestRequestPayloadSummary:
    def test_prefers_runtime_provider_api_format_over_endpoint_static_format(self) -> None:
        payload = {"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}

        summary = summarize_request_payload_shape(
            payload,
            provider_api_format="gemini:chat",
            body_rules=[{"action": "drop", "path": "toolConfig"}],
        )

        assert summary["format"] == "gemini:chat"
        assert summary["contents_count"] == 1
        assert summary["message_count"] is None
        assert summary["body_rule_count"] == 1
        assert summary["top_level_keys"] == ["contents"]

    def test_detects_gemini_camelcase_prompt_bearing_fields(self) -> None:
        payload = {
            "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
            "systemInstruction": {"parts": [{"text": "system"}]},
            "toolConfig": {"functionCallingConfig": {"mode": "AUTO"}},
            "generationConfig": {"temperature": 0.1},
        }

        summary = summarize_request_payload_shape(
            payload,
            provider_api_format="gemini:chat",
            body_rules=None,
        )

        assert summary["has_system"] is True
        assert summary["has_tool_choice"] is True
        assert summary["has_generation_config"] is True
        assert summary["function_declaration_count"] == 0
        assert summary["tool_count"] is None
        assert summary["format"] == "gemini:chat"
        assert summary["contents_count"] == 1
        assert summary["body_rule_count"] == 0
        assert summary["top_level_keys"] == [
            "contents",
            "generationConfig",
            "systemInstruction",
            "toolConfig",
        ]
