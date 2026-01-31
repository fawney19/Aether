"""
请求构建器 - 透传模式

透传模式 (Passthrough): CLI 和 Chat 等场景，原样转发请求体和头部
- 清理敏感头部：authorization, x-api-key, host, content-length 等
- 保留所有其他头部和请求体字段
- 适用于：Claude CLI、OpenAI CLI、Chat API 等场景

使用方式：
    builder = PassthroughRequestBuilder()
    payload, headers = builder.build(original_body, original_headers, endpoint, key)
"""

from __future__ import annotations

import json
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

from src.core.api_format import (
    UPSTREAM_DROP_HEADERS,
    HeaderBuilder,
    get_auth_config_for_endpoint,
    make_signature_key,
)
from src.core.crypto import crypto_service

if TYPE_CHECKING:
    from src.models.database import ProviderAPIKey, ProviderEndpoint


# ==============================================================================
# Service Account 认证结果类型
# ==============================================================================


@dataclass
class ProviderAuthInfo:
    """Provider 认证信息（用于 Service Account 等异步认证场景）"""

    auth_header: str
    auth_value: str
    # 解密后的认证配置（用于 URL 构建等场景，避免重复解密）
    decrypted_auth_config: dict[str, Any] | None = None

    def as_tuple(self) -> tuple[str, str]:
        """返回 (auth_header, auth_value) 元组"""
        return (self.auth_header, self.auth_value)


# ==============================================================================
# 统一的头部配置常量
# ==============================================================================

# 兼容别名：历史代码使用 SENSITIVE_HEADERS 命名
SENSITIVE_HEADERS: frozenset[str] = UPSTREAM_DROP_HEADERS

# 请求体中受保护的字段（不能被 body_rules 修改）
PROTECTED_BODY_FIELDS: frozenset[str] = frozenset(
    {
        "model",  # 模型名由系统管理
        "stream",  # 流式标志由系统管理
    }
)


# ==============================================================================
# 测试请求常量与辅助函数
# ==============================================================================

# 标准测试请求体（OpenAI 格式）
# 用于 check_endpoint 等测试场景，使用简单安全的消息内容避免触发安全过滤
DEFAULT_TEST_REQUEST: dict[str, Any] = {
    "messages": [{"role": "user", "content": "Hi"}],
    "max_tokens": 5,
    "temperature": 0,
}


def get_test_request_data(request_data: dict[str, Any] | None = None) -> dict[str, Any]:
    """获取测试请求数据

    如果传入 request_data，则合并到默认测试请求中；
    否则使用默认测试请求。

    Args:
        request_data: 用户提供的请求数据（会覆盖默认值）

    Returns:
        合并后的测试请求数据（OpenAI 格式）
    """
    if request_data:
        merged = DEFAULT_TEST_REQUEST.copy()
        merged.update(request_data)
        return merged
    return DEFAULT_TEST_REQUEST.copy()


def build_test_request_body(
    format_id: str,
    request_data: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """构建测试请求体，自动处理格式转换

    使用格式转换注册表将 OpenAI 格式的测试请求转换为目标格式。

    Args:
        format_id: 目标 endpoint signature（如 "claude:chat", "gemini:chat", "openai:cli"）
        request_data: 可选的请求数据，会与默认测试请求合并

    Returns:
        转换为目标 API 格式的请求体
    """
    from src.core.api_format.conversion import (
        format_conversion_registry,
        register_default_normalizers,
    )
    from src.core.api_format.utils import get_base_format

    register_default_normalizers()

    # 获取测试请求数据（OpenAI 格式）
    source_data = get_test_request_data(request_data)

    # CLI 格式使用基础格式进行转换（claude:cli -> claude:chat）
    target_format = get_base_format(format_id) or format_id

    # 使用注册表进行格式转换 (openai:chat -> 目标基础格式)
    return format_conversion_registry.convert_request(
        source_data,
        make_signature_key("openai", "chat"),
        target_format,
    )


# ==============================================================================
# 请求体规则应用
# ==============================================================================


def apply_body_rules(
    body: dict[str, Any],
    rules: list[dict[str, Any]],
    protected_keys: frozenset[str] | None = None,
) -> dict[str, Any]:
    """
    应用请求体规则

    支持的规则类型：
    - set: 设置/覆盖字段 {"action": "set", "path": "metadata", "value": {"custom": "val"}}
    - drop: 删除字段 {"action": "drop", "path": "unwanted_field"}
    - rename: 重命名字段 {"action": "rename", "from": "old_key", "to": "new_key"}

    Args:
        body: 原始请求体
        rules: 规则列表
        protected_keys: 受保护的字段（不能被 set/drop/rename 修改）

    Returns:
        应用规则后的请求体
    """
    if not rules:
        return body

    # 复制一份，避免修改原始数据
    result = dict(body)
    protected = protected_keys or PROTECTED_BODY_FIELDS

    for rule in rules:
        action = rule.get("action")

        if action == "set":
            path = rule.get("path", "")
            value = rule.get("value")
            if path and path not in protected:
                result[path] = value

        elif action == "drop":
            path = rule.get("path", "")
            if path and path not in protected:
                result.pop(path, None)

        elif action == "rename":
            from_key = rule.get("from", "")
            to_key = rule.get("to", "")
            if from_key and to_key:
                # 两个 key 都不能是受保护的
                if from_key not in protected and to_key not in protected:
                    if from_key in result:
                        result[to_key] = result.pop(from_key)

    return result


# ==============================================================================
# 请求构建器
# ==============================================================================


class RequestBuilder(ABC):
    """请求构建器抽象基类"""

    @abstractmethod
    def build_payload(
        self,
        original_body: dict[str, Any],
        *,
        mapped_model: str | None = None,
        is_stream: bool = False,
    ) -> dict[str, Any]:
        """构建请求体"""
        pass

    @abstractmethod
    def build_headers(
        self,
        original_headers: dict[str, str],
        endpoint: Any,
        key: Any,
        *,
        extra_headers: dict[str, str] | None = None,
        pre_computed_auth: tuple[str, str] | None = None,
    ) -> dict[str, str]:
        """构建请求头"""
        pass

    def build(
        self,
        original_body: dict[str, Any],
        original_headers: dict[str, str],
        endpoint: Any,
        key: Any,
        *,
        mapped_model: str | None = None,
        is_stream: bool = False,
        extra_headers: dict[str, str] | None = None,
        pre_computed_auth: tuple[str, str] | None = None,
    ) -> tuple[dict[str, Any], dict[str, str]]:
        """
        构建完整的请求（请求体 + 请求头）

        Args:
            original_body: 原始请求体
            original_headers: 原始请求头
            endpoint: 端点配置
            key: Provider API Key
            mapped_model: 映射后的模型名
            is_stream: 是否为流式请求
            extra_headers: 额外请求头
            pre_computed_auth: 预先计算的认证信息 (auth_header, auth_value)

        Returns:
            Tuple[payload, headers]
        """
        payload = self.build_payload(
            original_body,
            mapped_model=mapped_model,
            is_stream=is_stream,
        )

        # 应用请求体规则（如果 endpoint 配置了 body_rules）
        body_rules = getattr(endpoint, "body_rules", None)
        if body_rules:
            payload = apply_body_rules(payload, body_rules)

        headers = self.build_headers(
            original_headers,
            endpoint,
            key,
            extra_headers=extra_headers,
            pre_computed_auth=pre_computed_auth,
        )
        return payload, headers


class PassthroughRequestBuilder(RequestBuilder):
    """
    透传模式请求构建器

    适用于 CLI 等场景，尽量保持请求原样：
    - 请求体：直接复制，只修改必要字段（model, stream）
    - 请求头：清理敏感头部（黑名单），透传其他所有头部
    """

    def build_payload(
        self,
        original_body: dict[str, Any],
        *,
        mapped_model: str | None = None,  # noqa: ARG002 - 由 apply_mapped_model 处理
        is_stream: bool = False,  # noqa: ARG002 - 保留原始值，不自动添加
    ) -> dict[str, Any]:
        """
        透传请求体 - 原样复制，不做任何修改

        透传模式下：
        - model: 由各 handler 的 apply_mapped_model 方法处理
        - stream: 保留客户端原始值（不同 API 处理方式不同）
        """
        return dict(original_body)

    def build_headers(
        self,
        original_headers: dict[str, str],
        endpoint: Any,
        key: Any,
        *,
        extra_headers: dict[str, str] | None = None,
        pre_computed_auth: tuple[str, str] | None = None,
    ) -> dict[str, str]:
        """
        透传请求头 - 清理敏感头部（黑名单），透传其他所有头部

        Args:
            original_headers: 原始请求头
            endpoint: 端点配置
            key: Provider API Key
            extra_headers: 额外请求头
            pre_computed_auth: 预先计算的认证信息 (auth_header, auth_value)，
                               用于 Service Account 等异步获取 token 的场景
        """
        # 1. 根据 API 格式自动设置认证头
        if pre_computed_auth:
            # 使用预先计算的认证信息（Service Account 等场景）
            auth_header, auth_value = pre_computed_auth
        else:
            # 标准 API Key 认证
            decrypted_key = crypto_service.decrypt(key.api_key)
            raw_family = getattr(endpoint, "api_family", None)
            raw_kind = getattr(endpoint, "endpoint_kind", None)
            endpoint_sig: str | None = None
            if (
                isinstance(raw_family, str)
                and isinstance(raw_kind, str)
                and raw_family
                and raw_kind
            ):
                endpoint_sig = make_signature_key(raw_family, raw_kind)
            else:
                # 兜底：允许 endpoint.api_format 已经是 signature key 的情况
                raw_format = getattr(endpoint, "api_format", None)
                if isinstance(raw_format, str) and ":" in raw_format:
                    endpoint_sig = raw_format

            auth_header, auth_type = get_auth_config_for_endpoint(endpoint_sig or "openai:chat")
            auth_value = f"Bearer {decrypted_key}" if auth_type == "bearer" else decrypted_key
        # 认证头始终受保护，防止 header_rules 覆盖
        protected_keys = {auth_header.lower(), "content-type"}

        builder = HeaderBuilder()

        # 2. 透传原始头部（排除默认敏感头部）
        if original_headers:
            for name, value in original_headers.items():
                if name.lower() in SENSITIVE_HEADERS:
                    continue
                builder.add(name, value)

        # 3. 应用 endpoint 的请求头规则（认证头受保护，无法通过 rules 设置）
        header_rules = getattr(endpoint, "header_rules", None)
        if header_rules:
            builder.apply_rules(header_rules, protected_keys)

        # 4. 添加额外头部
        if extra_headers:
            builder.add_many(extra_headers)

        # 5. 设置认证头（最高优先级，上游始终使用 header 认证）
        builder.add(auth_header, auth_value)

        # 6. 确保有 Content-Type
        headers = builder.build()
        if not any(k.lower() == "content-type" for k in headers):
            headers["Content-Type"] = "application/json"

        return headers


# ==============================================================================
# 便捷函数
# ==============================================================================


def build_passthrough_request(
    original_body: dict[str, Any],
    original_headers: dict[str, str],
    endpoint: Any,
    key: Any,
) -> tuple[dict[str, Any], dict[str, str]]:
    """
    构建透传模式的请求

    纯透传：原样复制请求体，只处理请求头（认证等）。
    model mapping 和 stream 由调用方自行处理（不同 API 格式处理方式不同）。
    """
    builder = PassthroughRequestBuilder()
    return builder.build(
        original_body,
        original_headers,
        endpoint,
        key,
    )


# ==============================================================================
# Service Account 认证支持
# ==============================================================================


async def get_provider_auth(
    endpoint: "ProviderEndpoint",
    key: "ProviderAPIKey",
) -> ProviderAuthInfo | None:
    """
    获取 Provider 的认证信息

    对于标准 API Key，返回 None（由 build_headers 自动处理）。
    对于 Service Account，异步获取 Access Token 并返回认证信息。

    Args:
        endpoint: 端点配置
        key: Provider API Key

    Returns:
        Service Account 场景: ProviderAuthInfo 对象（包含认证信息和解密后的配置）
        API Key 场景: None（由 build_headers 处理）

    Raises:
        InvalidRequestException: 认证配置无效或认证失败
    """
    from src.core.exceptions import InvalidRequestException

    auth_type = getattr(key, "auth_type", "api_key")

    if auth_type == "vertex_ai":
        from src.core.vertex_auth import VertexAuthError, VertexAuthService

        try:
            # 优先从 auth_config 读取，兼容从 api_key 读取（过渡期）
            encrypted_auth_config = getattr(key, "auth_config", None)
            if encrypted_auth_config:
                # auth_config 可能是加密字符串或未加密的 dict
                if isinstance(encrypted_auth_config, dict):
                    # 已经是 dict，直接使用（兼容未加密存储的情况）
                    sa_json = encrypted_auth_config
                else:
                    # 是加密字符串，需要解密
                    decrypted_config = crypto_service.decrypt(encrypted_auth_config)
                    sa_json = json.loads(decrypted_config)
            else:
                # 兼容旧数据：从 api_key 读取
                decrypted_key = crypto_service.decrypt(key.api_key)
                # 检查是否是占位符（表示 auth_config 丢失）
                if decrypted_key == "__placeholder__":
                    raise InvalidRequestException("认证配置丢失，请重新添加该密钥。")
                sa_json = json.loads(decrypted_key)

            if not isinstance(sa_json, dict):
                raise InvalidRequestException("Service Account JSON 无效，请重新添加该密钥。")

            # 获取 Access Token
            service = VertexAuthService(sa_json)
            access_token = await service.get_access_token()

            # Vertex AI 使用 Bearer token
            return ProviderAuthInfo(
                auth_header="Authorization",
                auth_value=f"Bearer {access_token}",
                decrypted_auth_config=sa_json,
            )
        except InvalidRequestException:
            raise
        except VertexAuthError as e:
            raise InvalidRequestException(f"Vertex AI 认证失败：{e}")
        except json.JSONDecodeError:
            raise InvalidRequestException("Service Account JSON 格式无效，请重新添加该密钥。")
        except Exception:
            raise InvalidRequestException("Vertex AI 认证失败，请检查 Key 的 auth_config")

    # 其他认证类型可在此扩展
    # elif auth_type == "oauth2":
    #     ...

    # 标准 API Key：返回 None，由 build_headers 处理
    return None
