"""Antigravity v1internal request/response envelope helpers.

Antigravity reuses the `gemini:chat` endpoint signature but wraps the actual
wire format:
- Request: V1InternalRequest (top-level metadata + nested GeminiRequest)
- Response: V1InternalResponse (top-level responseId + nested GeminiResponse)

对齐 Antigravity-Manager wrapper.rs 的处理逻辑：
- Claude model tool ID 注入（request + response）
- Thinking budget capping (Auto cap 24576)
- [undefined] 字符串深度清理
- parametersJsonSchema → parameters 重命名
- JSON Schema 禁止字段清洗
- Antigravity System Instruction 注入
- Signature 错误检测
"""

from __future__ import annotations

import uuid
from typing import Any

from src.services.provider.adapters.antigravity.constants import (
    ANTIGRAVITY_SYSTEM_INSTRUCTION,
)
from src.services.provider.adapters.antigravity.constants import (
    REQUEST_USER_AGENT as ANTIGRAVITY_REQUEST_USER_AGENT,
)
from src.services.provider.adapters.antigravity.constants import (
    SIGNATURE_ERROR_KEYWORDS,
    THINKING_BUDGET_AUTO_CAP,
    THINKING_BUDGET_DEFAULT_INJECT,
    THINKING_MODELS_AUTO_INJECT_KEYWORDS,
    get_http_user_agent,
)
from src.services.provider.adapters.antigravity.url_availability import url_availability
from src.services.provider.request_context import get_selected_base_url

# ---------------------------------------------------------------------------
# Request body 预处理工具函数（对齐 AM wrapper.rs / common_utils.rs）
# ---------------------------------------------------------------------------


def _deep_clean_undefined(obj: Any) -> None:
    """In-place 递归清理 '[undefined]' 字符串值。

    Cherry Studio 等客户端会在请求中注入 '[undefined]' 字符串，
    可能导致上游 API 解析错误。
    """
    if isinstance(obj, dict):
        keys_to_remove = [k for k, v in obj.items() if v == "[undefined]"]
        for k in keys_to_remove:
            del obj[k]
        for v in obj.values():
            if isinstance(v, (dict, list)):
                _deep_clean_undefined(v)
    elif isinstance(obj, list):
        i = 0
        while i < len(obj):
            if obj[i] == "[undefined]":
                obj.pop(i)
            else:
                if isinstance(obj[i], (dict, list)):
                    _deep_clean_undefined(obj[i])
                i += 1


def _inject_claude_tool_ids_request(inner_request: dict[str, Any], model: str) -> None:
    """为 Claude 模型注入 functionCall/functionResponse 的 id 字段。

    Google v1internal 在目标模型为 Claude 时要求 functionCall 带有 id 字段，
    但标准 Gemini 协议不包含此字段。对齐 AM wrapper.rs #1522。
    """
    if "claude" not in model.lower():
        return

    contents = inner_request.get("contents")
    if not isinstance(contents, list):
        return

    for content in contents:
        if not isinstance(content, dict):
            continue
        parts = content.get("parts")
        if not isinstance(parts, list):
            continue

        # 每条消息维护独立的计数器（确保 Call 和 Response 生成匹配的 ID）
        name_counters: dict[str, int] = {}

        for part in parts:
            if not isinstance(part, dict):
                continue

            # 1. functionCall（Assistant 请求调用工具）
            fc = part.get("functionCall")
            if isinstance(fc, dict) and fc.get("id") is None:
                name = fc.get("name", "unknown")
                if not isinstance(name, str):
                    name = "unknown"
                count = name_counters.get(name, 0)
                fc["id"] = f"call_{name}_{count}"
                name_counters[name] = count + 1

            # 2. functionResponse（User 回复工具结果）
            fr = part.get("functionResponse")
            if isinstance(fr, dict) and fr.get("id") is None:
                name = fr.get("name", "unknown")
                if not isinstance(name, str):
                    name = "unknown"
                count = name_counters.get(name, 0)
                fr["id"] = f"call_{name}_{count}"
                name_counters[name] = count + 1


def _process_thinking_budget(inner_request: dict[str, Any], model: str) -> None:
    """处理 Thinking Budget：自动注入 + Auto Cap。

    对齐 AM wrapper.rs：
    - 对 flash/pro/thinking 模型处理 thinkingConfig
    - 自动注入 thinkingConfig（对已知需要 thinking 的模型）
    - Auto Cap：budget 超过 24576 时裁剪
    """
    lower_model = model.lower()
    if not any(kw in lower_model for kw in ("flash", "pro", "thinking")):
        return

    # 确保 generationConfig 存在
    gen_config = inner_request.setdefault("generationConfig", {})
    if not isinstance(gen_config, dict):
        return

    # 自动注入 thinkingConfig（对已知需要 thinking 的模型）
    if gen_config.get("thinkingConfig") is None:
        should_inject = any(kw in lower_model for kw in THINKING_MODELS_AUTO_INJECT_KEYWORDS)
        if should_inject:
            gen_config["thinkingConfig"] = {
                "includeThoughts": True,
                "thinkingBudget": THINKING_BUDGET_DEFAULT_INJECT,
            }

    # Auto Cap
    thinking_config = gen_config.get("thinkingConfig")
    if not isinstance(thinking_config, dict):
        return

    budget = thinking_config.get("thinkingBudget")
    if isinstance(budget, int) and budget > THINKING_BUDGET_AUTO_CAP:
        thinking_config["thinkingBudget"] = THINKING_BUDGET_AUTO_CAP


def _clean_tool_declarations(inner_request: dict[str, Any]) -> None:
    """清洗工具声明：重命名字段 + 移除禁止的 Schema 字段 + 过滤搜索声明。

    对齐 AM wrapper.rs：
    - parametersJsonSchema → parameters（Gemini CLI 兼容）
    - 移除 Gemini 不支持的 Schema 字段（multipleOf 等）
    - 过滤 web_search / google_search 工具声明
    """
    tools = inner_request.get("tools")
    if not isinstance(tools, list):
        return

    for tool in tools:
        if not isinstance(tool, dict):
            continue
        decls = tool.get("functionDeclarations")
        if not isinstance(decls, list):
            continue

        # 1. 过滤搜索关键字函数
        decls[:] = [
            d
            for d in decls
            if not (
                isinstance(d, dict)
                and isinstance(d.get("name"), str)
                and d["name"] in ("web_search", "google_search")
            )
        ]

        # 2. 重命名 + 清洗
        for decl in decls:
            if not isinstance(decl, dict):
                continue

            # parametersJsonSchema → parameters
            if "parametersJsonSchema" in decl:
                params = decl.pop("parametersJsonSchema")
                if isinstance(params, dict):
                    _clean_json_schema(params)
                decl["parameters"] = params
            elif "parameters" in decl:
                params = decl["parameters"]
                if isinstance(params, dict):
                    _clean_json_schema(params)


def _clean_json_schema(schema: dict[str, Any]) -> None:
    """递归移除 Gemini 不支持的 JSON Schema 字段。"""
    from src.core.api_format.schema_utils import clean_gemini_schema

    clean_gemini_schema(schema)


def _compact_contents(inner_request: dict[str, Any]) -> None:
    """Strip invalid parts, drop empty contents, merge consecutive same-role.

    Delegates to :func:`compact_gemini_contents` from the Gemini normalizer to
    avoid duplicating the validation logic.

    This function modifies *inner_request* in-place.
    """
    from src.core.api_format.conversion.normalizers.gemini import compact_gemini_contents

    contents = inner_request.get("contents")
    if not isinstance(contents, list):
        return

    result = compact_gemini_contents(contents)
    inner_request["contents"] = result


def _inject_system_instruction(inner_request: dict[str, Any]) -> None:
    """注入 Antigravity 身份系统指令。

    对齐 AM wrapper.rs：
    - 如果已有 systemInstruction：在前面插入（避免重复）
    - 如果没有：创建新的
    - 补全 role: user（Gemini API 要求）

    注意：Gemini API 使用 protobuf oneof，`system_instruction`（snake_case）和
    `systemInstruction`（camelCase）是同一个字段，只能设置其中一个。
    需要先统一到 camelCase 再处理。
    """
    # 统一 snake_case 到 camelCase（避免 oneof 冲突）
    if "system_instruction" in inner_request:
        snake_value = inner_request.pop("system_instruction")
        # 仅当 camelCase 不存在时才迁移
        if "systemInstruction" not in inner_request:
            inner_request["systemInstruction"] = snake_value

    system_instruction = inner_request.get("systemInstruction")

    if isinstance(system_instruction, dict):
        # 补全 role
        if "role" not in system_instruction:
            system_instruction["role"] = "user"

        parts = system_instruction.get("parts")
        if isinstance(parts, list):
            # 检查是否已包含 Antigravity 身份（避免重复注入）
            has_antigravity = False
            if parts and isinstance(parts[0], dict):
                text = parts[0].get("text", "")
                if isinstance(text, str) and "You are Antigravity" in text:
                    has_antigravity = True

            if not has_antigravity:
                parts.insert(0, {"text": ANTIGRAVITY_SYSTEM_INSTRUCTION})
    else:
        # 没有 systemInstruction，创建新的
        inner_request["systemInstruction"] = {
            "role": "user",
            "parts": [{"text": ANTIGRAVITY_SYSTEM_INSTRUCTION}],
        }


# ---------------------------------------------------------------------------
# Response 后处理工具函数
# ---------------------------------------------------------------------------


def _inject_claude_tool_ids_response(response: dict[str, Any], model: str) -> None:
    """为 Claude 模型的响应注入 functionCall 的 id 字段。

    对齐 AM wrapper.rs inject_ids_to_response：
    让下游客户端（如 OpenCode/Vercel AI SDK）能感知 tool call ID，
    并在下一轮对话中原样带回。
    """
    if "claude" not in model.lower():
        return

    candidates = response.get("candidates")
    if not isinstance(candidates, list):
        return

    for candidate in candidates:
        if not isinstance(candidate, dict):
            continue
        content = candidate.get("content")
        if not isinstance(content, dict):
            continue
        parts = content.get("parts")
        if not isinstance(parts, list):
            continue

        name_counters: dict[str, int] = {}
        for part in parts:
            if not isinstance(part, dict):
                continue
            fc = part.get("functionCall")
            if isinstance(fc, dict) and fc.get("id") is None:
                name = fc.get("name", "unknown")
                if not isinstance(name, str):
                    name = "unknown"
                count = name_counters.get(name, 0)
                fc["id"] = f"call_{name}_{count}"
                name_counters[name] = count + 1


# ---------------------------------------------------------------------------
# Core wrap / unwrap 函数
# ---------------------------------------------------------------------------


def _generate_stable_session_id(inner_request: dict[str, Any]) -> str:
    """生成稳定的 sessionId（对齐 CLIProxyAPI generateStableSessionID）。

    基于请求内容的哈希生成确定性 sessionId，相同内容的请求会得到相同的 sessionId，
    有助于上游维持会话上下文。
    """
    import hashlib
    import json

    try:
        # 使用 contents 字段生成稳定哈希（与 CLIProxyAPI 对齐）
        contents = inner_request.get("contents")
        if contents:
            raw = json.dumps(contents, sort_keys=True, separators=(",", ":"))
        else:
            raw = json.dumps(inner_request, sort_keys=True, separators=(",", ":"))
        digest = hashlib.sha256(raw.encode()).hexdigest()[:32]
        return f"session-{digest}"
    except Exception:
        return f"session-{uuid.uuid4().hex[:32]}"


def wrap_v1internal_request(
    gemini_request: dict[str, Any],
    *,
    project_id: str,
    model: str,
    request_type: str = "agent",
) -> dict[str, Any]:
    """Wrap a GeminiRequest into Antigravity V1InternalRequest.

    处理流程（对齐 AM wrapper.rs）：
    1. 移除 model（移到顶层）
    2. 移除 safetySettings（v1internal 不支持）
    3. 深度清理 [undefined] 字符串
    4. Claude model tool ID 注入（图像生成模型跳过）
    5. Thinking budget 处理
    6. 工具声明清洗（图像生成模型跳过）
    7. System Instruction 注入（图像生成模型跳过）
    8. 清理空 parts 的 contents 并合并连续同角色条目
    9. 注入 sessionId（对齐 CLIProxyAPI）
    10. 构建 v1internal 信封
    """
    from src.api.handlers.gemini.image_gen import is_image_gen_model

    inner_request = dict(gemini_request)
    inner_request.pop("model", None)
    inner_request.pop("safetySettings", None)

    is_image_gen = is_image_gen_model(model)

    # 1. 深度清理 [undefined]
    _deep_clean_undefined(inner_request)

    if not is_image_gen:
        # 2. Claude tool ID 注入
        _inject_claude_tool_ids_request(inner_request, model)

    # 3. Thinking budget 处理
    _process_thinking_budget(inner_request, model)

    if not is_image_gen:
        # 4. 工具声明清洗
        _clean_tool_declarations(inner_request)

        # 5. System Instruction 注入
        _inject_system_instruction(inner_request)
    else:
        # 图像生成模型：对齐 AM wrapper.rs，移除不兼容字段
        inner_request.pop("tools", None)
        inner_request.pop("toolConfig", None)
        inner_request.pop("tool_config", None)
        inner_request.pop("systemInstruction", None)
        inner_request.pop("system_instruction", None)
        request_type = "image_gen"

    # 6. 清理空 parts 的 contents 并合并连续同角色条目
    #    跨格式转换（如 Responses API reasoning 块）可能产生空 parts 的 content，
    #    Gemini API 要求每个 content 至少有一个有效 part，并且严格交替 user/model 角色。
    _compact_contents(inner_request)

    # 7. 注入 sessionId（对齐 CLIProxyAPI/sub2api）
    if "sessionId" not in inner_request:
        inner_request["sessionId"] = _generate_stable_session_id(inner_request)

    return {
        "project": project_id,
        "requestId": f"agent-{uuid.uuid4()}",
        "request": inner_request,
        "model": model,
        "userAgent": ANTIGRAVITY_REQUEST_USER_AGENT,
        "requestType": request_type,
    }


def unwrap_v1internal_response(response: dict[str, Any]) -> dict[str, Any]:
    """Unwrap Antigravity V1InternalResponse into a GeminiResponse-like dict."""
    inner = response.get("response")
    if isinstance(inner, dict):
        unwrapped = dict(inner)
        resp_id = response.get("responseId")
        if resp_id is not None:
            unwrapped["_v1internal_response_id"] = resp_id
        return unwrapped
    return response


def cache_thought_signatures(model: str, response: dict[str, Any]) -> None:
    """Best-effort cache for Antigravity thought signatures.

    同时缓存到 legacy (text) 层和 tool (Layer 1) 层。
    """
    try:
        from src.services.provider.adapters.antigravity.signature_cache import signature_cache
    except Exception:
        return

    try:
        candidates = response.get("candidates")
        if not isinstance(candidates, list):
            return

        for cand in candidates:
            if not isinstance(cand, dict):
                continue
            content = cand.get("content")
            if not isinstance(content, dict):
                continue
            parts = content.get("parts")
            if not isinstance(parts, list):
                continue

            for part in parts:
                if not isinstance(part, dict):
                    continue

                # 缓存 thinking signature（legacy text 层）
                text = part.get("text")
                sig = (
                    part.get("thoughtSignature")
                    or part.get("thought_signature")
                    or part.get("signature")
                )
                if isinstance(text, str) and text and isinstance(sig, str) and sig:
                    signature_cache.cache(model, text, sig)

                # 缓存 tool call signature（Layer 1）
                fc = part.get("functionCall")
                if isinstance(fc, dict) and isinstance(sig, str) and sig:
                    tool_id = fc.get("id")
                    if isinstance(tool_id, str) and tool_id:
                        signature_cache.cache_tool_signature(tool_id, sig)
    except Exception:
        # Never fail request path due to cache issues.
        return


def is_signature_error(status_code: int, error_body: str) -> bool:
    """检测 400 错误是否为 thinking signature 相关错误。

    对齐 AM handlers/common.rs：用于判断是否应该移除 thinking 配置后重试。
    """
    if status_code != 400:
        return False
    return any(kw in error_body for kw in SIGNATURE_ERROR_KEYWORDS)


# ---------------------------------------------------------------------------
# Envelope 类（Provider Hook 接口）
# ---------------------------------------------------------------------------


class AntigravityV1InternalEnvelope:
    """Provider envelope hooks for Antigravity v1internal wrapper."""

    name = "antigravity:v1internal"

    def extra_headers(self) -> dict[str, str] | None:
        return {"User-Agent": get_http_user_agent()}

    def wrap_request(
        self,
        request_body: dict[str, Any],
        *,
        model: str,
        url_model: str | None,
        decrypted_auth_config: dict[str, Any] | None,
    ) -> tuple[dict[str, Any], str | None]:
        from src.core.logger import logger as _envelope_logger

        project_id = (decrypted_auth_config or {}).get("project_id")
        if not isinstance(project_id, str) or not project_id:
            from src.core.exceptions import ProviderNotAvailableException

            raise ProviderNotAvailableException(
                "Antigravity OAuth 配置缺少 project_id，请重新授权",
                provider_name="antigravity",
                upstream_response="missing auth_config.project_id",
            )

        wrapped = wrap_v1internal_request(
            request_body,
            project_id=project_id,
            model=model,
        )

        # Debug: 打印 v1internal 请求的关键字段（不打印完整 body 避免日志过大）
        _envelope_logger.debug(
            "[Antigravity Envelope] model={}, project_id={}, requestType={}, "
            "userAgent={}, has_sessionId={}, has_systemInstruction={}, "
            "has_contents={}, has_generationConfig={}",
            wrapped.get("model"),
            str(wrapped.get("project", ""))[:8] + "...",
            wrapped.get("requestType"),
            wrapped.get("userAgent"),
            "sessionId" in (wrapped.get("request") or {}),
            "systemInstruction" in (wrapped.get("request") or {}),
            "contents" in (wrapped.get("request") or {}),
            "generationConfig" in (wrapped.get("request") or {}),
        )

        # Antigravity's model lives in the request body, not the URL path.
        return wrapped, None

    def unwrap_response(self, data: Any) -> Any:
        if isinstance(data, dict):
            return unwrap_v1internal_response(data)
        return data

    def postprocess_unwrapped_response(self, *, model: str, data: Any) -> None:
        if isinstance(data, dict):
            # Claude model: 注入 tool call ID（对齐 AM wrapper.rs inject_ids_to_response）
            _inject_claude_tool_ids_response(data, model)
            cache_thought_signatures(model, data)

    def capture_selected_base_url(self) -> str | None:
        return get_selected_base_url()

    def on_http_status(self, *, base_url: str | None, status_code: int) -> None:
        if not base_url:
            return
        if status_code == 200:
            url_availability.mark_success(base_url)
        elif status_code in (429, 500, 502, 503, 504):
            url_availability.mark_unavailable(base_url)

    def on_connection_error(self, *, base_url: str | None, exc: Exception) -> None:  # noqa: ARG002
        if not base_url:
            return
        url_availability.mark_unavailable(base_url)

    def force_stream_rewrite(self) -> bool:
        # Streaming must be rewritten even when endpoint signature matches, because
        # Antigravity wraps chunks in v1internal envelope.
        return True


antigravity_v1internal_envelope = AntigravityV1InternalEnvelope()


__all__ = [
    "AntigravityV1InternalEnvelope",
    "antigravity_v1internal_envelope",
    "cache_thought_signatures",
    "is_signature_error",
    "unwrap_v1internal_response",
    "wrap_v1internal_request",
]
