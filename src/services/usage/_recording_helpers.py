from __future__ import annotations

import json
from typing import Any

from sqlalchemy.orm import Session

from src.models.database import ApiKey, Usage, User
from src.services.billing.precision import to_money_decimal
from src.services.system.config import SystemConfigService
from src.services.usage._types import UsageCostInfo
from src.services.usage.error_classifier import classify_error


def _parse_format_dimensions(api_format: str | None) -> tuple[str | None, str | None]:
    """从 api_format (如 'claude:chat') 解析出 (api_family, endpoint_kind)"""
    if not api_format:
        return None, None
    parts = api_format.lower().split(":", 1)
    if len(parts) == 2:
        return parts[0], parts[1]
    return parts[0], None


# Metadata pruning configuration (ordered by priority - drop first to last)
METADATA_PRUNE_KEYS: tuple[str, ...] = (
    "raw_response_ref",
    "poll_raw_response",
    "trace",
    "debug",
    "dimensions",
    "provider_response_headers",
    "client_response_headers",
)

# Keys to preserve even under aggressive pruning
METADATA_KEEP_KEYS: frozenset[str] = frozenset(
    {
        "billing_snapshot",
        "billing_updated_at",
        "perf",
        "pool_summary",
        "scheduling_audit",
        "_metadata_truncated",
    }
)


def deserialize_body_if_json(value: Any) -> Any:
    """写库前按需反序列化 body JSON 字符串。

    仅对 JSON object/array 字符串做 json.loads，其他值保持原样。
    """
    if not isinstance(value, str):
        return value
    stripped = value.lstrip()
    if not stripped or stripped[0] not in "{[":
        return value
    try:
        parsed = json.loads(value)
    except (json.JSONDecodeError, TypeError):
        return value
    if isinstance(parsed, (dict, list)):
        return parsed
    return value


def build_usage_params(
    *,
    db: Session,
    user: User | None,
    api_key: ApiKey | None,
    provider: str,
    model: str,
    input_tokens: int,
    output_tokens: int,
    cache_creation_input_tokens: int,
    cache_read_input_tokens: int,
    cache_creation_input_tokens_5m: int = 0,
    cache_creation_input_tokens_1h: int = 0,
    request_type: str,
    api_format: str | None,
    api_family: str | None = None,
    endpoint_kind: str | None = None,
    endpoint_api_format: str | None,
    provider_api_family: str | None = None,
    provider_endpoint_kind: str | None = None,
    has_format_conversion: bool,
    is_stream: bool,
    response_time_ms: int | None,
    first_byte_time_ms: int | None,
    status_code: int,
    error_message: str | None,
    metadata: dict[str, Any] | None,
    request_headers: dict[str, Any] | None,
    request_body: Any | None,
    provider_request_headers: dict[str, Any] | None,
    provider_request_body: Any | None,
    response_headers: dict[str, Any] | None,
    client_response_headers: dict[str, Any] | None,
    response_body: Any | None,
    client_response_body: Any | None,
    request_id: str,
    provider_id: str | None,
    provider_endpoint_id: str | None,
    provider_api_key_id: str | None,
    status: str,
    cache_ttl_minutes: int | None = None,
    target_model: str | None,
    cost: UsageCostInfo,
) -> dict[str, Any]:
    """构建 Usage 记录的参数字典（内部方法，避免代码重复）"""

    # 展开成本信息
    input_cost = cost.input_cost
    output_cost = cost.output_cost
    cache_creation_cost = cost.cache_creation_cost
    cache_read_cost = cost.cache_read_cost
    cache_cost = cost.cache_cost
    request_cost = cost.request_cost
    total_cost = cost.total_cost
    input_price = cost.input_price
    output_price = cost.output_price
    cache_creation_price = cost.cache_creation_price
    cache_read_price = cost.cache_read_price
    request_price = cost.request_price
    actual_rate_multiplier = cost.actual_rate_multiplier
    is_free_tier = cost.is_free_tier

    # 根据配置决定是否记录请求详情
    should_log_headers = SystemConfigService.should_log_headers(db)
    should_log_body = SystemConfigService.should_log_body(db)

    # 处理请求头（可能需要脱敏）
    processed_request_headers = None
    if should_log_headers and request_headers is not None:
        processed_request_headers = SystemConfigService.mask_sensitive_headers(db, request_headers)

    # 处理提供商请求头（可能需要脱敏）
    processed_provider_request_headers = None
    if should_log_headers and provider_request_headers is not None:
        processed_provider_request_headers = SystemConfigService.mask_sensitive_headers(
            db, provider_request_headers
        )

    # 处理请求体和响应体（可能需要截断）
    processed_request_body = None
    processed_provider_request_body = None
    processed_response_body = None
    processed_client_response_body = None
    if should_log_body:
        if request_body is not None:
            processed_request_body = SystemConfigService.truncate_body(
                db, request_body, is_request=True
            )
        if provider_request_body is not None:
            processed_provider_request_body = SystemConfigService.truncate_body(
                db, provider_request_body, is_request=True
            )
        if response_body is not None:
            processed_response_body = SystemConfigService.truncate_body(
                db, response_body, is_request=False
            )
        if client_response_body is not None:
            processed_client_response_body = SystemConfigService.truncate_body(
                db, client_response_body, is_request=False
            )

    # 处理响应头
    processed_response_headers = None
    if should_log_headers and response_headers is not None:
        processed_response_headers = SystemConfigService.mask_sensitive_headers(
            db, response_headers
        )

    # 处理返回给客户端的响应头
    processed_client_response_headers = None
    if should_log_headers and client_response_headers is not None:
        processed_client_response_headers = SystemConfigService.mask_sensitive_headers(
            db, client_response_headers
        )

    # 计算真实成本（表面成本 * 倍率），免费套餐实际费用为 0
    if is_free_tier:
        actual_input_cost = 0.0
        actual_output_cost = 0.0
        actual_cache_creation_cost = 0.0
        actual_cache_read_cost = 0.0
        actual_request_cost = 0.0
        actual_total_cost = 0.0
    else:
        actual_input_cost = input_cost * actual_rate_multiplier
        actual_output_cost = output_cost * actual_rate_multiplier
        actual_cache_creation_cost = cache_creation_cost * actual_rate_multiplier
        actual_cache_read_cost = cache_read_cost * actual_rate_multiplier
        actual_request_cost = request_cost * actual_rate_multiplier
        actual_total_cost = total_cost * actual_rate_multiplier
    actual_cache_cost = actual_cache_creation_cost + actual_cache_read_cost

    error_category = None
    if status_code >= 400 or error_message or status in {"failed", "cancelled"}:
        error_category = classify_error(status_code, error_message, status).value

    # 从 api_format / endpoint_api_format 解析 api_family + endpoint_kind
    # 优先使用透传值，fallback 到字符串解析
    parsed_family, parsed_kind = _parse_format_dimensions(api_format)
    client_family = api_family or parsed_family
    client_kind = endpoint_kind or parsed_kind

    parsed_ep_family, parsed_ep_kind = _parse_format_dimensions(endpoint_api_format)
    ep_family = provider_api_family or parsed_ep_family
    ep_kind = provider_endpoint_kind or parsed_ep_kind

    input_output_total_tokens = input_tokens + output_tokens
    input_context_tokens = input_tokens + cache_read_input_tokens
    tracked_total_tokens = (
        input_tokens
        + output_tokens
        + cache_creation_input_tokens
        + cache_read_input_tokens
    )

    cache_creation_cost_5m = 0.0
    cache_creation_cost_1h = 0.0
    actual_cache_creation_cost_5m = 0.0
    actual_cache_creation_cost_1h = 0.0
    cache_creation_price_5m = None
    cache_creation_price_1h = None
    total_cache_creation_tokens = int(cache_creation_input_tokens or 0)
    ttl_5m_tokens = int(cache_creation_input_tokens_5m or 0)
    ttl_1h_tokens = int(cache_creation_input_tokens_1h or 0)

    if total_cache_creation_tokens > 0:
        if ttl_5m_tokens > 0 and ttl_1h_tokens > 0:
            ttl_5m_ratio = ttl_5m_tokens / total_cache_creation_tokens
            cache_creation_cost_5m = cache_creation_cost * ttl_5m_ratio
            cache_creation_cost_1h = cache_creation_cost - cache_creation_cost_5m
            actual_cache_creation_cost_5m = actual_cache_creation_cost * ttl_5m_ratio
            actual_cache_creation_cost_1h = actual_cache_creation_cost - actual_cache_creation_cost_5m
        elif ttl_5m_tokens > 0:
            cache_creation_cost_5m = cache_creation_cost
            actual_cache_creation_cost_5m = actual_cache_creation_cost
            cache_creation_price_5m = cache_creation_price
        elif ttl_1h_tokens > 0:
            cache_creation_cost_1h = cache_creation_cost
            actual_cache_creation_cost_1h = actual_cache_creation_cost
            cache_creation_price_1h = cache_creation_price
        elif cache_ttl_minutes is not None and cache_ttl_minutes <= 5:
            cache_creation_cost_5m = cache_creation_cost
            actual_cache_creation_cost_5m = actual_cache_creation_cost
            cache_creation_price_5m = cache_creation_price
        else:
            cache_creation_cost_1h = cache_creation_cost
            actual_cache_creation_cost_1h = actual_cache_creation_cost
            cache_creation_price_1h = cache_creation_price

    return {
        "user_id": user.id if user else None,
        "api_key_id": api_key.id if api_key else None,
        "username": user.username if user else None,
        "api_key_name": api_key.name if api_key else None,
        "request_id": request_id,
        "provider_name": provider,
        "model": model,
        "target_model": target_model,
        "provider_id": provider_id,
        "provider_endpoint_id": provider_endpoint_id,
        "provider_api_key_id": provider_api_key_id,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "input_output_total_tokens": input_output_total_tokens,
        "input_context_tokens": input_context_tokens,
        "total_tokens": tracked_total_tokens,
        "cache_creation_input_tokens": cache_creation_input_tokens,
        "cache_read_input_tokens": cache_read_input_tokens,
        "cache_creation_input_tokens_5m": cache_creation_input_tokens_5m,
        "cache_creation_input_tokens_1h": cache_creation_input_tokens_1h,
        "input_cost_usd": to_money_decimal(input_cost),
        "output_cost_usd": to_money_decimal(output_cost),
        "cache_cost_usd": to_money_decimal(cache_cost),
        "cache_creation_cost_usd": to_money_decimal(cache_creation_cost),
        "cache_creation_cost_usd_5m": to_money_decimal(cache_creation_cost_5m),
        "cache_creation_cost_usd_1h": to_money_decimal(cache_creation_cost_1h),
        "cache_read_cost_usd": to_money_decimal(cache_read_cost),
        "request_cost_usd": to_money_decimal(request_cost),
        "total_cost_usd": to_money_decimal(total_cost),
        "actual_input_cost_usd": to_money_decimal(actual_input_cost),
        "actual_output_cost_usd": to_money_decimal(actual_output_cost),
        "actual_cache_creation_cost_usd": to_money_decimal(actual_cache_creation_cost),
        "actual_cache_creation_cost_usd_5m": to_money_decimal(actual_cache_creation_cost_5m),
        "actual_cache_creation_cost_usd_1h": to_money_decimal(actual_cache_creation_cost_1h),
        "actual_cache_read_cost_usd": to_money_decimal(actual_cache_read_cost),
        "actual_cache_cost_usd": to_money_decimal(actual_cache_cost),
        "actual_request_cost_usd": to_money_decimal(actual_request_cost),
        "actual_total_cost_usd": to_money_decimal(actual_total_cost),
        "rate_multiplier": actual_rate_multiplier,
        "input_price_per_1m": input_price,
        "output_price_per_1m": output_price,
        "cache_creation_price_per_1m": cache_creation_price,
        "cache_creation_price_per_1m_5m": cache_creation_price_5m,
        "cache_creation_price_per_1m_1h": cache_creation_price_1h,
        "cache_read_price_per_1m": cache_read_price,
        "price_per_request": request_price,
        "request_type": request_type,
        "api_format": api_format,
        "api_family": client_family,
        "endpoint_kind": client_kind,
        "endpoint_api_format": endpoint_api_format,
        "provider_api_family": ep_family,
        "provider_endpoint_kind": ep_kind,
        "has_format_conversion": has_format_conversion,
        "is_stream": is_stream,
        "status_code": status_code,
        "error_message": error_message,
        "error_category": error_category,
        "response_time_ms": response_time_ms,
        "first_byte_time_ms": first_byte_time_ms,
        "status": status,
        "request_metadata": metadata,
        "request_headers": processed_request_headers,
        "request_body": processed_request_body,
        "provider_request_headers": processed_provider_request_headers,
        "provider_request_body": processed_provider_request_body,
        "response_headers": processed_response_headers,
        "client_response_headers": processed_client_response_headers,
        "response_body": processed_response_body,
        "client_response_body": processed_client_response_body,
    }


def update_existing_usage(
    existing_usage: Usage,
    usage_params: dict[str, Any],
    target_model: str | None,
) -> None:
    """更新已存在的 Usage 记录（内部方法）"""
    # 更新关键字段
    existing_usage.provider_name = usage_params["provider_name"]
    existing_usage.model = usage_params["model"]
    existing_usage.request_type = usage_params["request_type"]
    existing_usage.api_format = usage_params["api_format"]
    existing_usage.api_family = usage_params.get("api_family")
    existing_usage.endpoint_kind = usage_params.get("endpoint_kind")
    existing_usage.endpoint_api_format = usage_params["endpoint_api_format"]
    existing_usage.provider_api_family = usage_params.get("provider_api_family")
    existing_usage.provider_endpoint_kind = usage_params.get("provider_endpoint_kind")
    existing_usage.has_format_conversion = usage_params["has_format_conversion"]
    existing_usage.is_stream = usage_params["is_stream"]
    existing_usage.status = usage_params["status"]
    existing_usage.status_code = usage_params["status_code"]
    existing_usage.error_message = usage_params["error_message"]
    existing_usage.error_category = usage_params.get("error_category")
    existing_usage.response_time_ms = usage_params["response_time_ms"]
    existing_usage.first_byte_time_ms = usage_params["first_byte_time_ms"]

    # 更新请求头和请求体（如果有新值）
    if usage_params["request_headers"] is not None:
        existing_usage.request_headers = usage_params["request_headers"]
    if usage_params["request_body"] is not None:
        existing_usage.request_body = usage_params["request_body"]
    if usage_params["provider_request_headers"] is not None:
        existing_usage.provider_request_headers = usage_params["provider_request_headers"]
    if usage_params["provider_request_body"] is not None:
        existing_usage.provider_request_body = usage_params["provider_request_body"]
    existing_usage.response_body = usage_params["response_body"]
    existing_usage.response_headers = usage_params["response_headers"]
    existing_usage.client_response_headers = usage_params["client_response_headers"]
    existing_usage.client_response_body = usage_params["client_response_body"]

    # 更新 token 和费用信息
    existing_usage.input_tokens = usage_params["input_tokens"]
    existing_usage.output_tokens = usage_params["output_tokens"]
    existing_usage.input_output_total_tokens = usage_params["input_output_total_tokens"]
    existing_usage.input_context_tokens = usage_params["input_context_tokens"]
    existing_usage.total_tokens = usage_params["total_tokens"]
    existing_usage.cache_creation_input_tokens = usage_params["cache_creation_input_tokens"]
    existing_usage.cache_read_input_tokens = usage_params["cache_read_input_tokens"]
    existing_usage.cache_creation_input_tokens_5m = usage_params.get(
        "cache_creation_input_tokens_5m", 0
    )
    existing_usage.cache_creation_input_tokens_1h = usage_params.get(
        "cache_creation_input_tokens_1h", 0
    )
    existing_usage.input_cost_usd = usage_params["input_cost_usd"]
    existing_usage.output_cost_usd = usage_params["output_cost_usd"]
    existing_usage.cache_cost_usd = usage_params["cache_cost_usd"]
    existing_usage.cache_creation_cost_usd = usage_params["cache_creation_cost_usd"]
    existing_usage.cache_creation_cost_usd_5m = usage_params["cache_creation_cost_usd_5m"]
    existing_usage.cache_creation_cost_usd_1h = usage_params["cache_creation_cost_usd_1h"]
    existing_usage.cache_read_cost_usd = usage_params["cache_read_cost_usd"]
    existing_usage.request_cost_usd = usage_params["request_cost_usd"]
    existing_usage.total_cost_usd = usage_params["total_cost_usd"]
    existing_usage.actual_input_cost_usd = usage_params["actual_input_cost_usd"]
    existing_usage.actual_output_cost_usd = usage_params["actual_output_cost_usd"]
    existing_usage.actual_cache_creation_cost_usd = usage_params["actual_cache_creation_cost_usd"]
    existing_usage.actual_cache_creation_cost_usd_5m = usage_params[
        "actual_cache_creation_cost_usd_5m"
    ]
    existing_usage.actual_cache_creation_cost_usd_1h = usage_params[
        "actual_cache_creation_cost_usd_1h"
    ]
    existing_usage.actual_cache_read_cost_usd = usage_params["actual_cache_read_cost_usd"]
    existing_usage.actual_cache_cost_usd = usage_params["actual_cache_cost_usd"]
    existing_usage.actual_request_cost_usd = usage_params["actual_request_cost_usd"]
    existing_usage.actual_total_cost_usd = usage_params["actual_total_cost_usd"]
    existing_usage.rate_multiplier = usage_params["rate_multiplier"]
    existing_usage.cache_creation_price_per_1m_5m = usage_params.get("cache_creation_price_per_1m_5m")
    existing_usage.cache_creation_price_per_1m_1h = usage_params.get("cache_creation_price_per_1m_1h")

    # 更新 Provider 侧追踪信息（仅在有新值时更新，避免覆盖已有数据）
    if usage_params.get("provider_id"):
        existing_usage.provider_id = usage_params["provider_id"]
    if usage_params.get("provider_endpoint_id"):
        existing_usage.provider_endpoint_id = usage_params["provider_endpoint_id"]
    if usage_params.get("provider_api_key_id"):
        existing_usage.provider_api_key_id = usage_params["provider_api_key_id"]

    # 更新元数据（如 billing_snapshot/dimensions 等）
    if usage_params.get("request_metadata") is not None:
        existing_usage.request_metadata = usage_params["request_metadata"]

    # 更新模型映射信息
    if target_model is not None:
        existing_usage.target_model = target_model


def sanitize_request_metadata(metadata: dict[str, Any]) -> dict[str, Any]:
    """
    Best-effort metadata pruning to reduce DB/CPU/memory pressure.

    This is called right before persisting Usage rows (or updating request_metadata).
    Pruning order is defined by `METADATA_PRUNE_KEYS` (first key is dropped first).
    """
    if not isinstance(metadata, dict) or not metadata:
        return {}

    from src.config.settings import config

    # Enforce global metadata size limit (best-effort)
    max_bytes = int(getattr(config, "usage_metadata_max_bytes", 0) or 0)
    if max_bytes <= 0:
        return metadata

    def _size(d: dict[str, Any]) -> int:
        try:
            return len(json.dumps(d, ensure_ascii=False, default=str))
        except Exception:
            return len(str(d))

    if _size(metadata) <= max_bytes:
        return metadata

    # Progressive pruning (configurable order)
    metadata["_metadata_truncated"] = True

    for k in METADATA_PRUNE_KEYS:
        if k in metadata:
            metadata.pop(k, None)
            if _size(metadata) <= max_bytes:
                return metadata

    # Fallback: keep only billing-related metadata
    reduced = {k: metadata.get(k) for k in METADATA_KEEP_KEYS if k in metadata}
    return reduced
