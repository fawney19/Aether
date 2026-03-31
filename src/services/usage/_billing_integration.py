from __future__ import annotations

from typing import Any

from src.core.api_format.signature import normalize_signature_key
from src.services.billing.token_normalization import normalize_input_tokens_for_billing
from src.services.usage._recording_helpers import (
    build_usage_params,
    deserialize_body_if_json,
    sanitize_request_metadata,
)
from src.services.usage._types import UsageCostInfo, UsageRecordParams


class UsageBillingIntegrationMixin:
    """计费集成方法 -- 准备用量记录的共享逻辑"""

    @classmethod
    async def _prepare_usage_record(
        cls,
        params: UsageRecordParams,
    ) -> tuple[dict[str, Any], float]:
        """准备用量记录的共享逻辑

        此方法提取了 record_usage 和 record_usage_async 的公共处理逻辑：
        - 获取费率倍数
        - 计算成本
        - 构建 Usage 参数

        Args:
            params: 用量记录参数数据类

        Returns:
            (usage_params 字典, total_cost 总成本)
        """
        # 计费口径以 Provider 为准（优先 endpoint_api_format）
        billing_api_format: str | None = None
        if params.endpoint_api_format:
            try:
                billing_api_format = normalize_signature_key(str(params.endpoint_api_format))
            except Exception:
                billing_api_format = None
        if billing_api_format is None and params.api_format:
            try:
                billing_api_format = normalize_signature_key(str(params.api_format))
            except Exception:
                billing_api_format = None

        input_tokens_for_billing = normalize_input_tokens_for_billing(
            billing_api_format,
            params.input_tokens,
            params.cache_read_input_tokens,
        )

        # 获取费率倍数和是否免费套餐（传递 api_format 支持按格式配置的倍率）
        actual_rate_multiplier, is_free_tier = await cls._get_rate_multiplier_and_free_tier(
            params.db, params.provider_api_key_id, params.provider_id, billing_api_format
        )

        metadata = dict(params.metadata or {})
        is_failed_request = params.status_code >= 400 or params.error_message is not None

        # Helper: compute billing task_type (billing domain)
        billing_task_type = (params.request_type or "").lower()
        if billing_task_type not in {"chat", "cli", "video", "image", "audio"}:
            billing_task_type = "chat"

        # 使用新计费系统计算费用
        from src.services.billing.service import BillingService

        request_count = 0 if is_failed_request else 1
        has_cache_tokens = bool(
            params.cache_creation_input_tokens > 0 or params.cache_read_input_tokens > 0
        )
        total_cache_creation_tokens = max(int(params.cache_creation_input_tokens or 0), 0)
        ttl_5m_tokens = max(
            min(int(params.cache_creation_input_tokens_5m or 0), total_cache_creation_tokens),
            0,
        )
        ttl_1h_tokens = max(
            min(
                int(params.cache_creation_input_tokens_1h or 0),
                total_cache_creation_tokens - ttl_5m_tokens,
            ),
            0,
        )
        remaining_cache_creation_tokens = max(
            total_cache_creation_tokens - ttl_5m_tokens - ttl_1h_tokens,
            0,
        )
        effective_cache_ttl_minutes = params.cache_ttl_minutes

        # 主链路很多场景不会显式传 cache_ttl_minutes，这里补全以确保 1h/5m TTL 差异化计价生效。
        # 优先使用上游返回的 5m/1h 明细；只有无法区分时再回退到 key 级 TTL。
        if effective_cache_ttl_minutes is None and has_cache_tokens:
            if ttl_1h_tokens > 0 and ttl_5m_tokens == 0:
                effective_cache_ttl_minutes = 60
            elif ttl_5m_tokens > 0 and ttl_1h_tokens == 0:
                effective_cache_ttl_minutes = 5
            elif ttl_1h_tokens > 0:
                # 混合场景优先按长 TTL 计，避免 1h 缓存被按 5m 误计。
                effective_cache_ttl_minutes = 60

        if effective_cache_ttl_minutes is None and has_cache_tokens and params.provider_api_key_id:
            try:
                from src.models.database import ProviderAPIKey

                key_ttl = (
                    params.db.query(ProviderAPIKey.cache_ttl_minutes)
                    .filter(ProviderAPIKey.id == params.provider_api_key_id)
                    .scalar()
                )
                if key_ttl is not None:
                    key_ttl_int = int(key_ttl)
                    if key_ttl_int >= 0:
                        effective_cache_ttl_minutes = key_ttl_int
            except Exception:
                # Best-effort fallback below.
                pass

        billing = BillingService(params.db)
        total_input_context = (
            0
            if not params.use_tiered_pricing
            else input_tokens_for_billing
            + total_cache_creation_tokens
            + int(params.cache_read_input_tokens or 0)
        )

        def _as_float(v: Any, d: float | None) -> float | None:
            try:
                if v is None:
                    return d
                return float(v)
            except Exception:
                return d

        def _make_billing_dims(
            *,
            input_tokens: int,
            output_tokens: int,
            cache_creation_tokens: int,
            cache_read_tokens: int,
            request_count_value: int,
            cache_ttl_minutes: int | None,
        ) -> dict[str, Any]:
            dims: dict[str, Any] = {
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "cache_creation_input_tokens": cache_creation_tokens,
                "cache_read_input_tokens": cache_read_tokens,
                "request_count": request_count_value,
                "total_input_context": total_input_context,
            }
            if cache_ttl_minutes is not None:
                dims["cache_ttl_minutes"] = cache_ttl_minutes
            return dims

        def _run_billing(dimensions: dict[str, Any]) -> Any:
            return billing.calculate(
                task_type=billing_task_type,
                model=params.model,
                provider_id=params.provider_id or "",
                dimensions=dimensions,
                strict_mode=None,
            ).snapshot

        def _snapshot_dict(snapshot: Any) -> dict[str, Any]:
            try:
                data = snapshot.to_dict()
            except Exception:
                return {}
            return data if isinstance(data, dict) else {}

        mixed_cache_creation_ttls = ttl_5m_tokens > 0 and ttl_1h_tokens > 0

        billing_snapshot_payload: dict[str, Any]
        cache_creation_cost_5m = 0.0
        cache_creation_cost_1h = 0.0
        cache_creation_price_5m = None
        cache_creation_price_1h = None

        if mixed_cache_creation_ttls:
            common_dims = _make_billing_dims(
                input_tokens=input_tokens_for_billing,
                output_tokens=params.output_tokens,
                cache_creation_tokens=0,
                cache_read_tokens=params.cache_read_input_tokens,
                request_count_value=request_count,
                cache_ttl_minutes=effective_cache_ttl_minutes,
            )
            common_snap = _run_billing(common_dims)
            common_breakdown = common_snap.cost_breakdown or {}
            common_rv = common_snap.resolved_variables or {}

            cache_5m_snap = _run_billing(
                _make_billing_dims(
                    input_tokens=0,
                    output_tokens=0,
                    cache_creation_tokens=ttl_5m_tokens,
                    cache_read_tokens=0,
                    request_count_value=0,
                    cache_ttl_minutes=5,
                )
            )
            cache_5m_breakdown = cache_5m_snap.cost_breakdown or {}
            cache_5m_rv = cache_5m_snap.resolved_variables or {}

            cache_1h_snap = _run_billing(
                _make_billing_dims(
                    input_tokens=0,
                    output_tokens=0,
                    cache_creation_tokens=ttl_1h_tokens,
                    cache_read_tokens=0,
                    request_count_value=0,
                    cache_ttl_minutes=60,
                )
            )
            cache_1h_breakdown = cache_1h_snap.cost_breakdown or {}
            cache_1h_rv = cache_1h_snap.resolved_variables or {}

            remaining_snap = None
            remaining_breakdown: dict[str, Any] = {}
            remaining_rv: dict[str, Any] = {}
            if remaining_cache_creation_tokens > 0:
                remaining_snap = _run_billing(
                    _make_billing_dims(
                        input_tokens=0,
                        output_tokens=0,
                        cache_creation_tokens=remaining_cache_creation_tokens,
                        cache_read_tokens=0,
                        request_count_value=0,
                        cache_ttl_minutes=effective_cache_ttl_minutes,
                    )
                )
                remaining_breakdown = remaining_snap.cost_breakdown or {}
                remaining_rv = remaining_snap.resolved_variables or {}

            input_cost = float(common_breakdown.get("input_cost", 0.0))
            output_cost = float(common_breakdown.get("output_cost", 0.0))
            cache_read_cost = float(common_breakdown.get("cache_read_cost", 0.0))
            request_cost = float(common_breakdown.get("request_cost", 0.0))
            cache_creation_cost_5m = float(cache_5m_breakdown.get("cache_creation_cost", 0.0))
            cache_creation_cost_1h = float(cache_1h_breakdown.get("cache_creation_cost", 0.0))
            remaining_cache_creation_cost = float(
                remaining_breakdown.get("cache_creation_cost", 0.0)
            )
            cache_creation_cost = (
                cache_creation_cost_5m + cache_creation_cost_1h + remaining_cache_creation_cost
            )
            cache_cost = cache_creation_cost + cache_read_cost
            total_cost = (
                input_cost
                + output_cost
                + cache_creation_cost
                + cache_read_cost
                + request_cost
            )

            input_price = _as_float(common_rv.get("input_price_per_1m"), 0.0) or 0.0
            output_price = _as_float(common_rv.get("output_price_per_1m"), 0.0) or 0.0
            cache_creation_price_5m = _as_float(
                cache_5m_rv.get("cache_creation_price_per_1m"),
                None,
            )
            cache_creation_price_1h = _as_float(
                cache_1h_rv.get("cache_creation_price_per_1m"),
                None,
            )
            remaining_cache_creation_price = _as_float(
                remaining_rv.get("cache_creation_price_per_1m"),
                None,
            )
            cache_read_price = _as_float(common_rv.get("cache_read_price_per_1m"), None)
            request_price = _as_float(common_rv.get("price_per_request"), None)

            cache_creation_price = remaining_cache_creation_price
            if (
                remaining_cache_creation_tokens == 0
                and cache_creation_price_5m is not None
                and cache_creation_price_1h is not None
                and cache_creation_price_5m == cache_creation_price_1h
            ):
                cache_creation_price = cache_creation_price_5m

            base_snapshot = _snapshot_dict(common_snap)
            if not base_snapshot:
                base_snapshot = _snapshot_dict(cache_5m_snap)
            if not base_snapshot:
                base_snapshot = _snapshot_dict(cache_1h_snap)

            resolved_dimensions = {
                "input_tokens": input_tokens_for_billing,
                "output_tokens": params.output_tokens,
                "cache_creation_input_tokens": params.cache_creation_input_tokens,
                "cache_read_input_tokens": params.cache_read_input_tokens,
                "request_count": request_count,
                "total_input_context": total_input_context,
            }
            if effective_cache_ttl_minutes is not None:
                resolved_dimensions["cache_ttl_minutes"] = effective_cache_ttl_minutes

            resolved_variables = dict(base_snapshot.get("resolved_variables") or {})
            resolved_variables["input_price_per_1m"] = input_price
            resolved_variables["output_price_per_1m"] = output_price
            resolved_variables["cache_read_price_per_1m"] = cache_read_price
            resolved_variables["price_per_request"] = request_price
            resolved_variables["cache_creation_price_per_1m_5m"] = cache_creation_price_5m
            resolved_variables["cache_creation_price_per_1m_1h"] = cache_creation_price_1h
            if cache_creation_price is None:
                resolved_variables.pop("cache_creation_price_per_1m", None)
            else:
                resolved_variables["cache_creation_price_per_1m"] = cache_creation_price

            billing_snapshot_payload = {
                **base_snapshot,
                "resolved_dimensions": resolved_dimensions,
                "dimensions_used": resolved_dimensions,
                "resolved_variables": resolved_variables,
                "cost_breakdown": {
                    "input_cost": input_cost,
                    "output_cost": output_cost,
                    "cache_creation_cost": cache_creation_cost,
                    "cache_read_cost": cache_read_cost,
                    "request_cost": request_cost,
                },
                "total_cost": total_cost,
                "cost": total_cost,
                "cache_creation_split": {
                    "5m": {
                        "tokens": ttl_5m_tokens,
                        "price_per_1m": cache_creation_price_5m,
                        "cost": cache_creation_cost_5m,
                    },
                    "1h": {
                        "tokens": ttl_1h_tokens,
                        "price_per_1m": cache_creation_price_1h,
                        "cost": cache_creation_cost_1h,
                    },
                },
            }
            if remaining_cache_creation_tokens > 0:
                billing_snapshot_payload["cache_creation_split"]["other"] = {
                    "tokens": remaining_cache_creation_tokens,
                    "price_per_1m": remaining_cache_creation_price,
                    "cost": remaining_cache_creation_cost,
                }
        else:
            dims = _make_billing_dims(
                input_tokens=input_tokens_for_billing,
                output_tokens=params.output_tokens,
                cache_creation_tokens=params.cache_creation_input_tokens,
                cache_read_tokens=params.cache_read_input_tokens,
                request_count_value=request_count,
                cache_ttl_minutes=effective_cache_ttl_minutes,
            )
            snap = _run_billing(dims)

            breakdown = snap.cost_breakdown or {}
            input_cost = float(breakdown.get("input_cost", 0.0))
            output_cost = float(breakdown.get("output_cost", 0.0))
            cache_creation_cost = float(breakdown.get("cache_creation_cost", 0.0))
            cache_read_cost = float(breakdown.get("cache_read_cost", 0.0))
            request_cost = float(breakdown.get("request_cost", 0.0))
            cache_cost = cache_creation_cost + cache_read_cost
            total_cost = float(snap.total_cost or 0.0)

            rv = snap.resolved_variables or {}

            input_price = _as_float(rv.get("input_price_per_1m"), 0.0) or 0.0
            output_price = _as_float(rv.get("output_price_per_1m"), 0.0) or 0.0
            cache_creation_price = _as_float(rv.get("cache_creation_price_per_1m"), None)
            cache_read_price = _as_float(rv.get("cache_read_price_per_1m"), None)
            request_price = _as_float(rv.get("price_per_request"), None)

            billing_snapshot_payload = _snapshot_dict(snap)

        # Audit snapshot (pruned later by sanitize_request_metadata)
        metadata["billing_snapshot"] = billing_snapshot_payload

        # Best-effort prune metadata to reduce DB/memory pressure.
        metadata = sanitize_request_metadata(metadata)

        # 构建 Usage 参数
        request_body = deserialize_body_if_json(params.request_body)
        provider_request_body = deserialize_body_if_json(params.provider_request_body)
        response_body = deserialize_body_if_json(params.response_body)
        client_response_body = deserialize_body_if_json(params.client_response_body)
        usage_params = build_usage_params(
            db=params.db,
            user=params.user,
            api_key=params.api_key,
            provider=params.provider,
            model=params.model,
            input_tokens=input_tokens_for_billing,
            output_tokens=params.output_tokens,
            cache_creation_input_tokens=params.cache_creation_input_tokens,
            cache_read_input_tokens=params.cache_read_input_tokens,
            cache_creation_input_tokens_5m=params.cache_creation_input_tokens_5m,
            cache_creation_input_tokens_1h=params.cache_creation_input_tokens_1h,
            request_type=params.request_type,
            api_format=params.api_format,
            api_family=params.api_family,
            endpoint_kind=params.endpoint_kind,
            endpoint_api_format=params.endpoint_api_format,
            has_format_conversion=params.has_format_conversion,
            is_stream=params.is_stream,
            response_time_ms=params.response_time_ms,
            first_byte_time_ms=params.first_byte_time_ms,
            status_code=params.status_code,
            error_message=params.error_message,
            metadata=metadata,
            request_headers=params.request_headers,
            request_body=request_body,
            provider_request_headers=params.provider_request_headers,
            provider_request_body=provider_request_body,
            response_headers=params.response_headers,
            client_response_headers=params.client_response_headers,
            response_body=response_body,
            client_response_body=client_response_body,
            request_id=params.request_id,
            provider_id=params.provider_id,
            provider_endpoint_id=params.provider_endpoint_id,
            provider_api_key_id=params.provider_api_key_id,
            status=params.status,
            cache_ttl_minutes=effective_cache_ttl_minutes,
            target_model=params.target_model,
            cost=UsageCostInfo(
                input_cost=input_cost,
                output_cost=output_cost,
                cache_creation_cost=cache_creation_cost,
                cache_read_cost=cache_read_cost,
                cache_cost=cache_cost,
                request_cost=request_cost,
                total_cost=total_cost,
                cache_creation_cost_5m=cache_creation_cost_5m,
                cache_creation_cost_1h=cache_creation_cost_1h,
                input_price=input_price,
                output_price=output_price,
                cache_creation_price=cache_creation_price,
                cache_creation_price_5m=cache_creation_price_5m,
                cache_creation_price_1h=cache_creation_price_1h,
                cache_read_price=cache_read_price,
                request_price=request_price,
                actual_rate_multiplier=actual_rate_multiplier,
                is_free_tier=is_free_tier,
            ),
        )

        return usage_params, total_cost

    @classmethod
    async def _prepare_usage_records_batch(
        cls,
        params_list: list[UsageRecordParams],
    ) -> list[tuple[dict[str, Any], float, Exception | None]]:
        """批量并行准备用量记录（性能优化）

        并行调用 _prepare_usage_record，提高批量处理效率。

        Args:
            params_list: 用量记录参数列表

        Returns:
            列表，每项为 (usage_params, total_cost, exception)
            如果处理成功，exception 为 None
        """
        import asyncio

        async def prepare_single(
            params: UsageRecordParams,
        ) -> tuple[dict[str, Any], float, Exception | None]:
            try:
                usage_params, total_cost = await cls._prepare_usage_record(params)
                return (usage_params, total_cost, None)
            except Exception as e:
                return ({}, 0.0, e)

        if not params_list:
            return []

        # 避免一次性创建过多 task（并且 _prepare_usage_record 内部也可能包含并行调用）
        # 这里采用分批 gather 来限制并发量。
        chunk_size = 50
        results: list[tuple[dict[str, Any], float, Exception | None]] = []
        for i in range(0, len(params_list), chunk_size):
            chunk = params_list[i : i + chunk_size]
            chunk_results = await asyncio.gather(*(prepare_single(p) for p in chunk))
            results.extend(chunk_results)
        return results
