"""
API endpoint metadata (new mode).

新模式下，系统以 (ApiFamily, EndpointKind) 作为结构化标识；
在需要用 string 做 key（DB / JSON dict / metrics label / logs）时，统一使用
`family:kind` 的 endpoint signature key（全小写）。
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from copy import deepcopy
from dataclasses import dataclass, field
from types import MappingProxyType
from typing import Any

from src.core.api_format.enums import ApiFamily, AuthMethod, EndpointKind
from src.core.api_format.signature import EndpointSignature, make_signature_key, parse_signature_key


@dataclass(frozen=True, slots=True)
class EndpointDefinition:
    """
    端点定义（ApiFamily + EndpointKind）。

    - aliases: 用于调试/展示/配置的别名（不用于“接受 legacy APIFormat”）
    - default_path: 上游默认路径，可被 ProviderEndpoint.custom_path 覆盖
    - auth_method/auth_header/auth_type: 认证信息（header/bearer 等）
    - extra_headers/protected_keys: 格式固定头与保护头
    - model_in_body/stream_in_body: 结构差异标记（用于 request/response 构造/规范化）
    - data_format_id: 数据格式标识（相同即可透传；不同需格式转换）
    """

    api_family: ApiFamily
    endpoint_kind: EndpointKind

    aliases: Sequence[str] = field(default_factory=tuple)
    default_path: str = "/"
    path_prefix: str = ""

    auth_method: AuthMethod = AuthMethod.BEARER
    auth_header: str = "Authorization"
    auth_type: str = "bearer"  # "bearer" | "header"

    extra_headers: Mapping[str, str] = field(default_factory=dict)
    protected_keys: frozenset[str] = field(default_factory=frozenset)

    model_in_body: bool = True
    stream_in_body: bool = True

    data_format_id: str = ""
    default_body_rules: Sequence[dict[str, Any]] = field(default_factory=tuple)

    @property
    def signature(self) -> EndpointSignature:
        return EndpointSignature(api_family=self.api_family, endpoint_kind=self.endpoint_kind)

    @property
    def signature_key(self) -> str:
        return self.signature.key

    def iter_aliases(self) -> Iterable[str]:
        # 统一包含 signature key（便于配置/展示）
        yield self.signature_key
        for alias in self.aliases:
            value = str(alias or "").strip()
            if value:
                yield value


CODEX_DEFAULT_BODY_RULES: tuple[dict[str, Any], ...] = (
    {"action": "drop", "path": "max_output_tokens"},
    {"action": "drop", "path": "temperature"},
    {"action": "drop", "path": "top_p"},
    {"action": "set", "path": "store", "value": False},
    {
        "action": "set",
        "path": "instructions",
        "value": "You are GPT-5.",
        "condition": {"path": "instructions", "op": "not_exists"},
    },
)


_ENDPOINT_DEFINITIONS: dict[tuple[ApiFamily, EndpointKind], EndpointDefinition] = {
    # Claude
    (ApiFamily.CLAUDE, EndpointKind.CHAT): EndpointDefinition(
        api_family=ApiFamily.CLAUDE,
        endpoint_kind=EndpointKind.CHAT,
        aliases=("claude", "anthropic", "claude_compatible"),
        default_path="/v1/messages",
        auth_method=AuthMethod.API_KEY,
        auth_header="x-api-key",
        auth_type="header",
        extra_headers={"anthropic-version": "2023-06-01"},
        protected_keys=frozenset({"x-api-key", "content-type", "anthropic-version"}),
        data_format_id="claude",
    ),
    (ApiFamily.CLAUDE, EndpointKind.CLI): EndpointDefinition(
        api_family=ApiFamily.CLAUDE,
        endpoint_kind=EndpointKind.CLI,
        aliases=("claude_cli", "claude-cli"),
        default_path="/v1/messages",
        auth_method=AuthMethod.BEARER,
        auth_header="Authorization",
        auth_type="bearer",
        protected_keys=frozenset({"authorization", "content-type"}),
        data_format_id="claude",
    ),
    # OpenAI
    (ApiFamily.OPENAI, EndpointKind.CHAT): EndpointDefinition(
        api_family=ApiFamily.OPENAI,
        endpoint_kind=EndpointKind.CHAT,
        aliases=(
            "openai",
            "openai_compatible",
            "deepseek",
            "grok",
            "moonshot",
            "zhipu",
            "qwen",
            "baichuan",
            "minimax",
        ),
        default_path="/v1/chat/completions",
        auth_method=AuthMethod.BEARER,
        auth_header="Authorization",
        auth_type="bearer",
        protected_keys=frozenset({"authorization", "content-type"}),
        data_format_id="openai_chat",
    ),
    (ApiFamily.OPENAI, EndpointKind.CLI): EndpointDefinition(
        api_family=ApiFamily.OPENAI,
        endpoint_kind=EndpointKind.CLI,
        aliases=("openai_cli", "responses"),
        default_path="/v1/responses",
        auth_method=AuthMethod.BEARER,
        auth_header="Authorization",
        auth_type="bearer",
        protected_keys=frozenset({"authorization", "content-type"}),
        data_format_id="openai_responses",
    ),
    (ApiFamily.OPENAI, EndpointKind.COMPACT): EndpointDefinition(
        api_family=ApiFamily.OPENAI,
        endpoint_kind=EndpointKind.COMPACT,
        aliases=("openai_compact", "responses_compact"),
        default_path="/v1/responses/compact",
        auth_method=AuthMethod.BEARER,
        auth_header="Authorization",
        auth_type="bearer",
        protected_keys=frozenset({"authorization", "content-type"}),
        # compact endpoint is non-streaming by design.
        stream_in_body=False,
        data_format_id="openai_responses",
        default_body_rules=CODEX_DEFAULT_BODY_RULES,
    ),
    (ApiFamily.OPENAI, EndpointKind.VIDEO): EndpointDefinition(
        api_family=ApiFamily.OPENAI,
        endpoint_kind=EndpointKind.VIDEO,
        aliases=("openai_video", "sora"),
        default_path="/v1/videos",
        auth_method=AuthMethod.BEARER,
        auth_header="Authorization",
        auth_type="bearer",
        protected_keys=frozenset({"authorization", "content-type"}),
        model_in_body=True,
        stream_in_body=False,
        data_format_id="openai_video",
    ),
    (ApiFamily.OPENAI, EndpointKind.IMAGE): EndpointDefinition(
        api_family=ApiFamily.OPENAI,
        endpoint_kind=EndpointKind.IMAGE,
        aliases=("openai_image", "gpt_image", "dalle"),
        default_path="/v1/images/generations",
        auth_method=AuthMethod.BEARER,
        auth_header="Authorization",
        auth_type="bearer",
        protected_keys=frozenset({"authorization", "content-type"}),
        model_in_body=True,
        stream_in_body=True,
        data_format_id="openai_image",
    ),
    # Gemini
    (ApiFamily.GEMINI, EndpointKind.CHAT): EndpointDefinition(
        api_family=ApiFamily.GEMINI,
        endpoint_kind=EndpointKind.CHAT,
        aliases=("gemini", "google", "vertex"),
        default_path="/v1beta/models/{model}:{action}",
        auth_method=AuthMethod.GOOG_API_KEY,
        auth_header="x-goog-api-key",
        auth_type="header",
        protected_keys=frozenset({"x-goog-api-key", "content-type"}),
        model_in_body=False,
        stream_in_body=False,
        data_format_id="gemini",
    ),
    (ApiFamily.GEMINI, EndpointKind.CLI): EndpointDefinition(
        api_family=ApiFamily.GEMINI,
        endpoint_kind=EndpointKind.CLI,
        aliases=("gemini_cli", "gemini-cli"),
        default_path="/v1beta/models/{model}:{action}",
        auth_method=AuthMethod.GOOG_API_KEY,
        auth_header="x-goog-api-key",
        auth_type="header",
        protected_keys=frozenset({"x-goog-api-key", "content-type"}),
        model_in_body=False,
        stream_in_body=False,
        data_format_id="gemini",
    ),
    (ApiFamily.GEMINI, EndpointKind.VIDEO): EndpointDefinition(
        api_family=ApiFamily.GEMINI,
        endpoint_kind=EndpointKind.VIDEO,
        aliases=("gemini_video", "veo"),
        default_path="/v1beta/models/{model}:predictLongRunning",
        auth_method=AuthMethod.GOOG_API_KEY,
        auth_header="x-goog-api-key",
        auth_type="header",
        protected_keys=frozenset({"x-goog-api-key", "content-type"}),
        model_in_body=False,
        stream_in_body=False,
        data_format_id="gemini_video",
    ),
}

# 对外只暴露只读视图，避免被随意修改
ENDPOINT_DEFINITIONS: Mapping[tuple[ApiFamily, EndpointKind], EndpointDefinition] = (
    MappingProxyType(_ENDPOINT_DEFINITIONS)
)


def list_endpoint_definitions() -> list[EndpointDefinition]:
    return list(ENDPOINT_DEFINITIONS.values())


def get_endpoint_definition(
    api_family: ApiFamily, endpoint_kind: EndpointKind
) -> EndpointDefinition:
    return ENDPOINT_DEFINITIONS[(api_family, endpoint_kind)]


def resolve_endpoint_definition(
    value: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
) -> EndpointDefinition | None:
    """
    Resolve an endpoint definition from a signature-like input.

    Accepted inputs:
    - EndpointSignature
    - (ApiFamily, EndpointKind)
    - "family:kind" signature string
    """
    try:
        if isinstance(value, EndpointSignature):
            return ENDPOINT_DEFINITIONS.get((value.api_family, value.endpoint_kind))
        if isinstance(value, tuple) and len(value) == 2:
            fam, kind = value
            if isinstance(fam, ApiFamily) and isinstance(kind, EndpointKind):
                return ENDPOINT_DEFINITIONS.get((fam, kind))
        if isinstance(value, str):
            sig = parse_signature_key(value)
            return ENDPOINT_DEFINITIONS.get((sig.api_family, sig.endpoint_kind))
    except Exception:
        return None
    return None


def get_default_path_for_endpoint(
    value: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
) -> str:
    definition = resolve_endpoint_definition(value)
    return definition.default_path if definition else "/"


def get_local_path_for_endpoint(
    value: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
) -> str:
    definition = resolve_endpoint_definition(value)
    if not definition:
        return "/"
    prefix = definition.path_prefix or ""
    return prefix + definition.default_path


def get_auth_config_for_endpoint(
    value: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
) -> tuple[str, str]:
    definition = resolve_endpoint_definition(value)
    if not definition:
        return "Authorization", "bearer"
    return definition.auth_header, definition.auth_type


def get_extra_headers_for_endpoint(
    value: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
) -> Mapping[str, str]:
    definition = resolve_endpoint_definition(value)
    return definition.extra_headers if definition else {}


def get_protected_keys_for_endpoint(
    value: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
) -> frozenset[str]:
    definition = resolve_endpoint_definition(value)
    return (
        definition.protected_keys
        if definition and definition.protected_keys
        else frozenset({"authorization", "content-type"})
    )


def get_data_format_id_for_endpoint(
    value: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
) -> str:
    """
    获取端点的数据格式标识。

    - 相同 data_format_id 可透传（不需要数据转换）
    - 不同 data_format_id 需要走 format conversion
    """
    definition = resolve_endpoint_definition(value)
    if definition and definition.data_format_id:
        return definition.data_format_id
    return ""


def get_default_body_rules_for_endpoint(
    value: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
    *,
    provider_type: str | None = None,
) -> list[dict[str, Any]]:
    """获取端点的默认 body_rules。

    优先查找 unified api_format capability registry 中的 provider 维度规则（如 Codex 对 openai:cli 的定制规则），
    找不到时回退到 EndpointDefinition 上的通用默认规则。
    """
    # 确保 provider plugins 已注册（填充 capabilities 中的 provider registry）
    # ensure_providers_bootstrapped 是幂等的，重复调用无副作用
    if provider_type:
        try:
            import importlib

            envelope = importlib.import_module("src.services.provider.envelope")
            getattr(envelope, "ensure_providers_bootstrapped")()
        except Exception:
            pass

    # 1) provider_type 维度的注册规则优先
    if provider_type:
        pt = provider_type.strip().lower()
        sig = _normalize_sig_key(value)
        from src.core.api_format.capabilities import get_provider_default_body_rules

        provider_rules = get_provider_default_body_rules(pt, sig)
        if provider_rules is not None:
            return provider_rules

    # 2) 回退到 EndpointDefinition 上的通用默认规则
    definition = resolve_endpoint_definition(value)
    if not definition or not definition.default_body_rules:
        return []
    return deepcopy(list(definition.default_body_rules))


# ---------------------------------------------------------------------------
# Provider-scoped default body rules compatibility wrappers
# ---------------------------------------------------------------------------


def register_provider_default_body_rules(
    provider_type: str,
    endpoint_sig: str,
    rules: Sequence[dict[str, Any]],
) -> None:
    """兼容入口：注册特定 provider_type + endpoint_sig 的默认 body_rules，真实存储位于 core registry。"""
    pt = provider_type.strip().lower()
    sig = _normalize_sig_key(endpoint_sig)
    from src.core.api_format.capabilities import (
        register_provider_default_body_rules as register_provider_default_body_rules_in_registry,
    )

    register_provider_default_body_rules_in_registry(pt, sig, rules)


def _normalize_sig_key(
    value: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
) -> str:
    """将各种端点标识形式归一化为 signature key 字符串。"""
    if isinstance(value, str):
        try:
            return parse_signature_key(value).key
        except Exception:
            return value.strip().lower()
    if isinstance(value, EndpointSignature):
        return value.key
    if isinstance(value, tuple) and len(value) == 2:
        return make_signature_key(value[0], value[1])
    return str(value).strip().lower()


def can_passthrough_endpoint(
    client: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
    provider: str | EndpointSignature | tuple[ApiFamily, EndpointKind],
) -> bool:
    """
    判断两个 endpoint signature 是否可以透传（无需数据转换）。

    透传条件：
    1) signature 完全相同
    2) data_format_id 相同（如 claude:chat / claude:cli）
    """
    try:
        if isinstance(client, str) and isinstance(provider, str):
            if parse_signature_key(client).key == parse_signature_key(provider).key:
                return True
    except Exception:
        pass

    client_id = get_data_format_id_for_endpoint(client)
    provider_id = get_data_format_id_for_endpoint(provider)
    return bool(client_id) and client_id == provider_id


def make_endpoint_signature(api_family: str, endpoint_kind: str) -> str:
    """
    Helper: build canonical signature key from raw strings (lowercased/trimmed).

    This is used in places that store family/kind separately in DB.
    """
    return make_signature_key(api_family, endpoint_kind)


__all__ = [
    "CODEX_DEFAULT_BODY_RULES",
    "EndpointDefinition",
    "ENDPOINT_DEFINITIONS",
    "list_endpoint_definitions",
    "get_endpoint_definition",
    "resolve_endpoint_definition",
    "get_default_path_for_endpoint",
    "get_local_path_for_endpoint",
    "get_auth_config_for_endpoint",
    "get_extra_headers_for_endpoint",
    "get_protected_keys_for_endpoint",
    "get_data_format_id_for_endpoint",
    "get_default_body_rules_for_endpoint",
    "can_passthrough_endpoint",
    "make_endpoint_signature",
]
