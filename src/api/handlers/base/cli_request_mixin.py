"""CLI Handler - 请求准备 Mixin"""

from __future__ import annotations

from dataclasses import dataclass
from typing import (
    TYPE_CHECKING,
    Any,
)

from src.api.handlers.base.request_builder import get_provider_auth
from src.api.handlers.base.utils import get_format_converter_registry
from src.core.api_format.headers import set_accept_if_absent
from src.core.logger import logger
from src.services.provider.behavior import get_provider_behavior
from src.services.provider.prompt_cache import maybe_patch_request_with_prompt_cache_key
from src.services.provider.stream_policy import (
    enforce_stream_mode_for_upstream,
    get_upstream_stream_policy,
    resolve_upstream_is_stream,
)
from src.services.provider.transport import build_provider_url

if TYPE_CHECKING:
    from src.api.handlers.base.cli_protocol import CliHandlerProtocol
    from src.core.api_format import EndpointDefinition


@dataclass(slots=True)
class CliUpstreamRequestResult:
    """Final outbound request artifacts for a selected Provider candidate."""

    payload: dict[str, Any]
    headers: dict[str, str]
    url: str
    url_model: str
    envelope: Any
    upstream_is_stream: bool
    tls_profile: str | None = None
    selected_base_url: str | None = None


class CliRequestMixin:
    """请求准备相关方法的 Mixin"""

    async def _get_mapped_model(
        self,
        source_model: str,
        provider_id: str,
    ) -> str | None:
        """
        获取模型映射后的实际模型名

        查找逻辑：
        1. 直接通过 GlobalModel.name 匹配
        2. 查找该 Provider 的 Model 实现
        3. 使用 provider_model_name / provider_model_mappings 选择最终名称

        Args:
            source_model: 用户请求的模型名（必须是 GlobalModel.name）
            provider_id: Provider ID

        Returns:
            映射后的 Provider 模型名，如果没有找到映射则返回 None
        """
        from src.services.model.mapper import ModelMapperMiddleware

        mapper = ModelMapperMiddleware(self.db)
        mapping = await mapper.get_mapping(source_model, provider_id)

        logger.debug(
            f"[CLI] _get_mapped_model: source={source_model}, provider={provider_id[:8]}..., mapping={mapping}"
        )

        if mapping and mapping.model:
            # 使用 select_provider_model_name 支持模型映射功能
            # 传入 api_key.id 作为 affinity_key，实现相同用户稳定选择同一映射
            # 传入 api_format 用于过滤适用的映射作用域
            affinity_key = self.api_key.id if self.api_key else None
            mapped_name = mapping.model.select_provider_model_name(
                affinity_key, api_format=self.FORMAT_ID
            )
            logger.debug(
                f"[CLI] 模型映射: {source_model} -> {mapped_name} (provider={provider_id[:8]}...)"
            )
            return mapped_name

        logger.debug("[CLI] 无模型映射，使用原始名称: {}", source_model)
        return None

    def extract_model_from_request(
        self: CliHandlerProtocol,
        request_body: dict[str, Any],
        path_params: dict[str, Any] | None = None,  # noqa: ARG002 - 子类使用
    ) -> str:
        """
        从请求中提取模型名 - 子类可覆盖

        不同 API 格式的 model 位置不同：
        - OpenAI/Claude: 在请求体中 request_body["model"]
        - Gemini: 在 URL 路径中 path_params["model"]

        子类应覆盖此方法实现各自的提取逻辑。

        Args:
            request_body: 请求体
            path_params: URL 路径参数

        Returns:
            模型名，如果无法提取则返回 "unknown"
        """
        # 默认实现：从请求体获取
        model = request_body.get("model")
        return str(model) if model else "unknown"

    def apply_mapped_model(
        self,
        request_body: dict[str, Any],
        mapped_model: str,  # noqa: ARG002 - 子类使用
    ) -> dict[str, Any]:
        """
        将映射后的模型名应用到请求体

        基类默认实现：不修改请求体，保持原样透传。
        子类应覆盖此方法实现各自的模型名替换逻辑。

        Args:
            request_body: 原始请求体
            mapped_model: 映射后的模型名（子类使用）

        Returns:
            请求体（默认不修改）
        """
        # 基类不修改请求体，子类覆盖此方法实现特定格式的处理
        return request_body

    def prepare_provider_request_body(
        self,
        request_body: dict[str, Any],
    ) -> dict[str, Any]:
        """
        准备发送给 Provider 的请求体 - 子类可覆盖

        在模型映射之后、发送请求之前调用，用于移除不需要发送给上游的字段。
        例如 Gemini API 需要移除请求体中的 model 字段（因为 model 在 URL 路径中）。

        Args:
            request_body: 经过模型映射处理后的请求体

        Returns:
            准备好的请求体
        """
        return request_body

    def finalize_provider_request(
        self,
        request_body: dict[str, Any],
        *,
        mapped_model: str | None,
        provider_api_format: str | None,
    ) -> dict[str, Any]:
        """
        格式转换完成后、envelope 之前的模型感知后处理钩子 - 子类可覆盖

        用于根据目标模型的特性对请求体做最终调整，例如：
        - 图像生成模型需要移除不兼容的 tools/system_instruction 并注入 imageConfig
        - 特定模型需要注入/移除某些字段
        - Gemini 格式：清理无效 parts 和合并连续同角色 contents

        此方法在流式和非流式路径中均会被调用，且 mapped_model 已确定。

        Args:
            request_body: 已完成格式转换的请求体
            mapped_model: 映射后的目标模型名
            provider_api_format: Provider 侧 API 格式标识

        Returns:
            调整后的请求体
        """
        # Gemini 格式请求：清理无效 parts 和合并连续同角色 contents
        # 跨格式转换（如 Claude -> Gemini）可能产生 thinking 等无法表示的块，
        # 导致 parts 为空或缺少有效 data-oneof 字段，被 Google API 拒绝。
        if provider_api_format and "gemini" in str(provider_api_format).lower():
            contents = request_body.get("contents")
            if isinstance(contents, list):
                from src.core.api_format.conversion.normalizers.gemini import (
                    compact_gemini_contents,
                )

                request_body["contents"] = compact_gemini_contents(contents)

        return request_body

    async def _build_upstream_request(
        self: CliHandlerProtocol,
        *,
        provider: Any,
        endpoint: Any,
        key: Any,
        request_body: dict[str, Any],
        original_headers: dict[str, str],
        query_params: dict[str, str] | None,
        client_api_format: str,
        provider_api_format: str,
        fallback_model: str,
        mapped_model: str | None,
        client_is_stream: bool,
        needs_conversion: bool = False,
        output_limit: int | None = None,
    ) -> CliUpstreamRequestResult:
        """Build the final outbound URL/body/headers for the selected upstream."""

        provider_type = str(getattr(provider, "provider_type", "") or "").lower()
        behavior = get_provider_behavior(
            provider_type=provider_type,
            endpoint_sig=provider_api_format,
        )
        envelope = behavior.envelope
        target_variant = behavior.same_format_variant
        conversion_variant = behavior.cross_format_variant

        upstream_policy = get_upstream_stream_policy(
            endpoint,
            provider_type=provider_type,
            endpoint_sig=provider_api_format,
        )
        upstream_is_stream = resolve_upstream_is_stream(
            client_is_stream=client_is_stream,
            policy=upstream_policy,
        )

        envelope_tls_profile: str | None = None
        if envelope and hasattr(envelope, "prepare_context"):
            envelope_tls_profile = envelope.prepare_context(
                provider_config=getattr(provider, "config", None),
                key_id=str(getattr(key, "id", "") or ""),
                user_api_key_id=str(getattr(self.api_key, "id", "") or ""),
                is_stream=upstream_is_stream,
                provider_id=str(getattr(provider, "id", "") or ""),
                key=key,
            )

        if needs_conversion and provider_api_format:
            request_body, url_model = await self._convert_request_for_cross_format(
                request_body,
                client_api_format,
                provider_api_format,
                mapped_model,
                fallback_model,
                is_stream=upstream_is_stream,
                target_variant=conversion_variant,
                output_limit=output_limit,
            )
        else:
            request_body = self.prepare_provider_request_body(request_body)
            url_model = (
                self.get_model_for_url(request_body, mapped_model) or mapped_model or fallback_model
            )
            if target_variant and provider_api_format:
                registry = get_format_converter_registry()
                request_body = registry.convert_request(
                    request_body,
                    provider_api_format,
                    provider_api_format,
                    target_variant=target_variant,
                )

        request_body = self.finalize_provider_request(
            request_body,
            mapped_model=mapped_model,
            provider_api_format=provider_api_format,
        )

        if provider_api_format:
            enforce_stream_mode_for_upstream(
                request_body,
                provider_api_format=provider_api_format,
                upstream_is_stream=upstream_is_stream,
            )

        request_body = maybe_patch_request_with_prompt_cache_key(
            request_body,
            provider_api_format=provider_api_format,
            provider_type=provider_type,
            base_url=getattr(endpoint, "base_url", None),
            user_api_key_id=str(getattr(self.api_key, "id", "") or ""),
            request_headers=original_headers,
        )

        auth_info = await get_provider_auth(endpoint, key)
        if envelope:
            request_body, url_model = envelope.wrap_request(
                request_body,
                model=url_model or fallback_model or "",
                url_model=url_model,
                decrypted_auth_config=auth_info.decrypted_auth_config if auth_info else None,
            )
            if hasattr(envelope, "post_wrap_request"):
                await envelope.post_wrap_request(request_body)

        extra_headers: dict[str, str] = {}
        if envelope:
            extra_headers.update(envelope.extra_headers() or {})

        provider_payload, provider_headers = self._request_builder.build(
            request_body,
            original_headers,
            endpoint,
            key,
            is_stream=upstream_is_stream,
            extra_headers=extra_headers if extra_headers else None,
            pre_computed_auth=auth_info.as_tuple() if auth_info else None,
            envelope=envelope,
            provider_api_format=provider_api_format,
        )
        if upstream_is_stream:
            set_accept_if_absent(provider_headers)

        url = build_provider_url(
            endpoint,
            query_params=query_params,
            path_params={"model": url_model},
            is_stream=upstream_is_stream,
            key=key,
            decrypted_auth_config=auth_info.decrypted_auth_config if auth_info else None,
        )
        selected_base_url = envelope.capture_selected_base_url() if envelope else None

        return CliUpstreamRequestResult(
            payload=provider_payload,
            headers=provider_headers,
            url=str(url),
            url_model=str(url_model or fallback_model or ""),
            envelope=envelope,
            upstream_is_stream=upstream_is_stream,
            tls_profile=envelope_tls_profile,
            selected_base_url=selected_base_url,
        )

    @staticmethod
    def _get_format_metadata(format_id: str) -> "EndpointDefinition | None":
        """获取 endpoint 元数据（解析失败返回 None）"""
        from src.core.api_format.metadata import resolve_endpoint_definition

        return resolve_endpoint_definition(format_id)

    def _finalize_converted_request(
        self,
        request_body: dict[str, Any],
        client_api_format: str,
        provider_api_format: str,
        mapped_model: str | None,
        fallback_model: str,
        is_stream: bool,
    ) -> None:
        """
        跨格式转换后统一设置并清理 model/stream 字段（原地修改）

        处理逻辑：
        1. 根据目标格式决定是否在 body 中设置 model
        2. 若客户端格式不含 stream 字段但 Provider 需要，则显式设置
        3. 移除目标格式不允许在 body 中携带的字段（如 Gemini 的 model/stream）

        Args:
            request_body: 转换后的请求体（会被原地修改）
            client_api_format: 客户端 API 格式
            provider_api_format: Provider API 格式
            mapped_model: 映射后的模型名
            fallback_model: 备用模型名
            is_stream: 是否流式请求
        """
        client_meta = self._get_format_metadata(client_api_format)
        provider_meta = self._get_format_metadata(provider_api_format)

        # 默认：model_in_body=True, stream_in_body=True（如 OpenAI/Claude）
        client_uses_stream = client_meta.stream_in_body if client_meta else True
        provider_model_in_body = provider_meta.model_in_body if provider_meta else True
        provider_stream_in_body = provider_meta.stream_in_body if provider_meta else True

        # 设置 model（仅当 Provider 允许且 body 中需要）
        if provider_model_in_body:
            request_body["model"] = mapped_model or fallback_model
        else:
            request_body.pop("model", None)

        # 设置 stream（客户端不带但 Provider 需要时显式设置；Provider 不需要时移除）
        if provider_stream_in_body:
            if not client_uses_stream:
                request_body["stream"] = is_stream
        else:
            request_body.pop("stream", None)

        # OpenAI Chat Completions: request usage in streaming mode.
        provider_fmt = str(provider_api_format or "").strip().lower()
        if is_stream and provider_fmt == "openai:chat":
            stream_options = request_body.get("stream_options")
            if not isinstance(stream_options, dict):
                stream_options = {}
            stream_options["include_usage"] = True
            request_body["stream_options"] = stream_options

    async def _convert_request_for_cross_format(
        self,
        request_body: dict[str, Any],
        client_api_format: str,
        provider_api_format: str,
        mapped_model: str | None,
        fallback_model: str,
        is_stream: bool,
        *,
        target_variant: str | None = None,
        output_limit: int | None = None,
    ) -> tuple[dict[str, Any], str]:
        """
        跨格式请求转换的公共逻辑

        将客户端格式的请求体转换为 Provider 格式，并处理 model/stream 字段的补齐和清理。

        Args:
            request_body: 原始请求体（会被修改）
            client_api_format: 客户端 API 格式
            provider_api_format: Provider API 格式
            mapped_model: 映射后的模型名
            fallback_model: 备用模型名（通常是原始请求的 model）
            is_stream: 是否流式请求
            target_variant: 目标变体（如 "codex"），用于同格式但有细微差异的上游
            output_limit: GlobalModel 配置的模型输出上限

        Returns:
            (转换后的请求体, 用于 URL 的模型名)
        """
        registry = get_format_converter_registry()
        converted_body = await registry.convert_request_async(
            request_body,
            str(client_api_format),
            str(provider_api_format),
            target_variant=target_variant,
            output_limit=output_limit,
        )

        # 先计算 URL 模型（在清理 body 中的 model 字段之前）
        url_model = (
            self.get_model_for_url(converted_body, mapped_model) or mapped_model or fallback_model
        )

        # 统一设置并清理 model/stream 字段
        self._finalize_converted_request(
            converted_body,
            str(client_api_format),
            str(provider_api_format),
            mapped_model,
            fallback_model,
            is_stream,
        )

        return converted_body, url_model

    def get_model_for_url(
        self,
        request_body: dict[str, Any],
        mapped_model: str | None,
    ) -> str | None:
        """
        获取用于 URL 路径的模型名

        某些 API 格式（如 Gemini）需要将 model 放入 URL 路径中。
        子类应覆盖此方法返回正确的值。

        Args:
            request_body: 请求体
            mapped_model: 映射后的模型名（如果有）

        Returns:
            用于 URL 路径的模型名，默认优先使用映射后的名称
        """
        return mapped_model or request_body.get("model")

    def _extract_response_metadata(
        self,
        response: dict[str, Any],
    ) -> dict[str, Any]:
        """
        从响应中提取 Provider 特有的元数据 - 子类可覆盖

        例如 Gemini 返回的 modelVersion 字段。
        这些元数据会存储到 Usage.request_metadata 中。

        Args:
            response: Provider 返回的响应

        Returns:
            元数据字典，默认为空
        """
        return {}
