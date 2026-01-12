"""
Provider API Keys 管理
"""

import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Dict, List

from fastapi import APIRouter, Depends, Query, Request
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.pipeline import ApiRequestPipeline
from src.config.constants import RPMDefaults
from src.core.crypto import crypto_service
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.key_capabilities import get_capability
from src.core.logger import logger
from src.database import get_db
from src.models.database import Provider, ProviderAPIKey, ProviderEndpoint
from src.services.cache.provider_cache import ProviderCacheService
from src.models.endpoint_models import (
    EndpointAPIKeyCreate,
    EndpointAPIKeyResponse,
    EndpointAPIKeyUpdate,
)

router = APIRouter(tags=["Provider Keys"])
pipeline = ApiRequestPipeline()

@router.put("/keys/{key_id}", response_model=EndpointAPIKeyResponse)
async def update_endpoint_key(
    key_id: str,
    key_data: EndpointAPIKeyUpdate,
    request: Request,
    db: Session = Depends(get_db),
) -> EndpointAPIKeyResponse:
    """
    更新 Provider Key

    更新指定 Key 的配置，支持修改并发限制、速率倍数、优先级、
    配额限制、能力限制等。支持部分更新。

    **路径参数**:
    - `key_id`: Key ID

    **请求体字段**（均为可选）:
    - `api_key`: 新的 API Key 原文
    - `name`: Key 名称
    - `note`: 备注
    - `rate_multiplier`: 速率倍数
    - `internal_priority`: 内部优先级
    - `rpm_limit`: RPM 限制（设置为 null 可切换到自适应模式）
    - `allowed_models`: 允许的模型列表
    - `capabilities`: 能力配置
    - `is_active`: 是否活跃

    **返回字段**:
    - 包含更新后的完整 Key 信息
    """
    adapter = AdminUpdateEndpointKeyAdapter(key_id=key_id, key_data=key_data)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/keys/grouped-by-format")
async def get_keys_grouped_by_format(
    request: Request,
    db: Session = Depends(get_db),
) -> dict:
    """
    获取按 API 格式分组的所有 Keys

    获取所有活跃的 Key，按 API 格式分组返回，用于全局优先级管理。
    每个 Key 包含基本信息、健康度指标、能力标签等。

    **返回字段**:
    - 返回一个字典，键为 API 格式，值为该格式下的 Key 列表
    - 每个 Key 包含：
      - `id`: Key ID
      - `name`: Key 名称
      - `api_key_masked`: 脱敏后的 API Key
      - `internal_priority`: 内部优先级
      - `global_priority`: 全局优先级
      - `rate_multiplier`: 速率倍数
      - `is_active`: 是否活跃
      - `circuit_breaker_open`: 熔断器状态
      - `provider_name`: Provider 名称
      - `endpoint_base_url`: Endpoint 基础 URL
      - `api_format`: API 格式
      - `capabilities`: 能力简称列表
      - `success_rate`: 成功率
      - `avg_response_time_ms`: 平均响应时间
      - `request_count`: 请求总数
    """
    adapter = AdminGetKeysGroupedByFormatAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/keys/{key_id}/reveal")
async def reveal_endpoint_key(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> dict:
    """
    获取完整的 API Key

    解密并返回指定 Key 的完整原文，用于查看和复制。
    此操作会被记录到审计日志。

    **路径参数**:
    - `key_id`: Key ID

    **返回字段**:
    - `api_key`: 完整的 API Key 原文
    """
    adapter = AdminRevealEndpointKeyAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/keys/{key_id}")
async def delete_endpoint_key(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
) -> dict:
    """
    删除 Provider Key

    删除指定的 API Key。此操作不可逆，请谨慎使用。

    **路径参数**:
    - `key_id`: Key ID

    **返回字段**:
    - `message`: 操作结果消息
    """
    adapter = AdminDeleteEndpointKeyAdapter(key_id=key_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ========== Provider Keys API ==========


@router.get("/providers/{provider_id}/keys", response_model=List[EndpointAPIKeyResponse])
async def list_provider_keys(
    provider_id: str,
    request: Request,
    skip: int = Query(0, ge=0, description="跳过的记录数"),
    limit: int = Query(100, ge=1, le=1000, description="返回的最大记录数"),
    db: Session = Depends(get_db),
) -> List[EndpointAPIKeyResponse]:
    """
    获取 Provider 的所有 Keys

    获取指定 Provider 下的所有 API Key 列表，支持多 API 格式。
    结果按优先级和创建时间排序。

    **路径参数**:
    - `provider_id`: Provider ID

    **查询参数**:
    - `skip`: 跳过的记录数，用于分页（默认 0）
    - `limit`: 返回的最大记录数（1-1000，默认 100）
    """
    adapter = AdminListProviderKeysAdapter(
        provider_id=provider_id,
        skip=skip,
        limit=limit,
    )
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/providers/{provider_id}/keys", response_model=EndpointAPIKeyResponse)
async def add_provider_key(
    provider_id: str,
    key_data: EndpointAPIKeyCreate,
    request: Request,
    db: Session = Depends(get_db),
) -> EndpointAPIKeyResponse:
    """
    为 Provider 添加 Key

    为指定 Provider 添加新的 API Key，支持配置多个 API 格式。

    **路径参数**:
    - `provider_id`: Provider ID

    **请求体字段**:
    - `api_formats`: 支持的 API 格式列表（必填）
    - `api_key`: API Key 原文（将被加密存储）
    - `name`: Key 名称
    - 其他配置字段同 Key
    """
    adapter = AdminCreateProviderKeyAdapter(provider_id=provider_id, key_data=key_data)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# -------- Adapters --------


@dataclass
class AdminUpdateEndpointKeyAdapter(AdminApiAdapter):
    key_id: str
    key_data: EndpointAPIKeyUpdate

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise NotFoundException(f"Key {self.key_id} 不存在")

        update_data = self.key_data.model_dump(exclude_unset=True)
        if "api_key" in update_data:
            update_data["api_key"] = crypto_service.encrypt(update_data["api_key"])

        # 特殊处理 rpm_limit：需要区分"未提供"和"显式设置为 null"
        if "rpm_limit" in self.key_data.model_fields_set:
            update_data["rpm_limit"] = self.key_data.rpm_limit
            if self.key_data.rpm_limit is None:
                update_data["learned_rpm_limit"] = None
                logger.info("Key %s 切换为自适应 RPM 模式", self.key_id)

        # 统一处理 allowed_models：空列表/空字典 -> None（表示不限制）
        if "allowed_models" in update_data:
            am = update_data["allowed_models"]
            if am is not None and (
                (isinstance(am, list) and len(am) == 0)
                or (isinstance(am, dict) and len(am) == 0)
            ):
                update_data["allowed_models"] = None

        for field, value in update_data.items():
            setattr(key, field, value)
        key.updated_at = datetime.now(timezone.utc)

        db.commit()
        db.refresh(key)

        # 任何字段更新都清除缓存，确保缓存一致性
        # 包括 is_active、allowed_models、capabilities 等影响权限和行为的字段
        await ProviderCacheService.invalidate_provider_api_key_cache(self.key_id)

        logger.info("[OK] 更新 Key: ID=%s, Updates=%s", self.key_id, list(update_data.keys()))

        return _build_key_response(key)


@dataclass
class AdminRevealEndpointKeyAdapter(AdminApiAdapter):
    """获取完整的 API Key（用于查看和复制）"""

    key_id: str

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise NotFoundException(f"Key {self.key_id} 不存在")

        try:
            decrypted_key = crypto_service.decrypt(key.api_key)
        except Exception as e:
            logger.error(f"解密 Key 失败: ID={self.key_id}, Error={e}")
            raise InvalidRequestException(
                "无法解密 API Key，可能是加密密钥已更改。请重新添加该密钥。"
            )

        logger.info(f"[REVEAL] 查看完整 Key: ID={self.key_id}, Name={key.name}")
        return {"api_key": decrypted_key}


@dataclass
class AdminDeleteEndpointKeyAdapter(AdminApiAdapter):
    key_id: str

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == self.key_id).first()
        if not key:
            raise NotFoundException(f"Key {self.key_id} 不存在")

        provider_id = key.provider_id
        try:
            db.delete(key)
            db.commit()
        except Exception as exc:
            db.rollback()
            logger.error(f"删除 Key 失败: ID={self.key_id}, Error={exc}")
            raise

        logger.warning(f"[DELETE] 删除 Key: ID={self.key_id}, Provider={provider_id}")
        return {"message": f"Key {self.key_id} 已删除"}


class AdminGetKeysGroupedByFormatAdapter(AdminApiAdapter):
    async def handle(self, context):  # type: ignore[override]
        db = context.db

        # Key 属于 Provider：按 key.api_formats 分组展示
        keys = (
            db.query(ProviderAPIKey, Provider)
            .join(Provider, ProviderAPIKey.provider_id == Provider.id)
            .filter(
                ProviderAPIKey.is_active.is_(True),
                Provider.is_active.is_(True),
            )
            .order_by(
                ProviderAPIKey.global_priority.asc().nullslast(),
                ProviderAPIKey.internal_priority.asc(),
            )
            .all()
        )

        provider_ids = {str(provider.id) for _key, provider in keys}
        endpoints = (
            db.query(
                ProviderEndpoint.provider_id,
                ProviderEndpoint.api_format,
                ProviderEndpoint.base_url,
            )
            .filter(
                ProviderEndpoint.provider_id.in_(provider_ids),
                ProviderEndpoint.is_active.is_(True),
            )
            .all()
        )
        endpoint_base_url_map: Dict[tuple[str, str], str] = {}
        for provider_id, api_format, base_url in endpoints:
            fmt = api_format.value if hasattr(api_format, "value") else str(api_format)
            endpoint_base_url_map[(str(provider_id), fmt)] = base_url

        grouped: Dict[str, List[dict]] = {}
        for key, provider in keys:
            api_formats = key.api_formats or []

            if not api_formats:
                continue  # 跳过没有 API 格式的 Key

            try:
                decrypted_key = crypto_service.decrypt(key.api_key)
                masked_key = f"{decrypted_key[:8]}***{decrypted_key[-4:]}"
            except Exception as e:
                logger.error(f"解密 Key 失败: key_id={key.id}, error={e}")
                masked_key = "***ERROR***"

            # 计算健康度指标
            success_rate = key.success_count / key.request_count if key.request_count > 0 else None
            avg_response_time_ms = (
                round(key.total_response_time_ms / key.success_count, 2)
                if key.success_count > 0
                else None
            )

            # 将 capabilities dict 转换为启用的能力简短名称列表
            caps_list = []
            if key.capabilities:
                for cap_name, enabled in key.capabilities.items():
                    if enabled:
                        cap_def = get_capability(cap_name)
                        caps_list.append(cap_def.short_name if cap_def else cap_name)

            # 构建 Key 信息（基础数据）
            key_info = {
                "id": key.id,
                "name": key.name,
                "api_key_masked": masked_key,
                "internal_priority": key.internal_priority,
                "global_priority": key.global_priority,
                "rate_multiplier": key.rate_multiplier,
                "is_active": key.is_active,
                "provider_name": provider.name,
                "api_formats": api_formats,
                "capabilities": caps_list,
                "success_rate": success_rate,
                "avg_response_time_ms": avg_response_time_ms,
                "request_count": key.request_count,
            }

            # 将 Key 添加到每个支持的格式分组中，并附加格式特定的健康度数据
            health_by_format = key.health_by_format or {}
            circuit_by_format = key.circuit_breaker_by_format or {}
            provider_id = str(provider.id)
            for api_format in api_formats:
                if api_format not in grouped:
                    grouped[api_format] = []
                # 为每个格式创建副本，设置当前格式
                format_key_info = key_info.copy()
                format_key_info["api_format"] = api_format
                format_key_info["endpoint_base_url"] = endpoint_base_url_map.get(
                    (provider_id, api_format)
                )
                # 添加格式特定的健康度数据
                format_health = health_by_format.get(api_format, {})
                format_circuit = circuit_by_format.get(api_format, {})
                format_key_info["health_score"] = float(format_health.get("health_score") or 1.0)
                format_key_info["circuit_breaker_open"] = bool(format_circuit.get("open", False))
                grouped[api_format].append(format_key_info)

        # 直接返回分组对象，供前端使用
        return grouped


# ========== Adapters ==========


def _build_key_response(
    key: ProviderAPIKey, api_key_plain: str | None = None
) -> EndpointAPIKeyResponse:
    """构建 Key 响应对象的辅助函数"""
    try:
        decrypted_key = crypto_service.decrypt(key.api_key)
        masked_key = f"{decrypted_key[:8]}***{decrypted_key[-4:]}"
    except Exception:
        masked_key = "***ERROR***"

    success_rate = key.success_count / key.request_count if key.request_count > 0 else 0.0
    avg_response_time_ms = (
        key.total_response_time_ms / key.success_count if key.success_count > 0 else 0.0
    )

    is_adaptive = key.rpm_limit is None
    key_dict = key.__dict__.copy()
    key_dict.pop("_sa_instance_state", None)

    # 从 health_by_format 计算汇总字段（便于列表展示）
    health_by_format = key.health_by_format or {}
    circuit_by_format = key.circuit_breaker_by_format or {}

    # 计算整体健康度（取所有格式中的最低值）
    if health_by_format:
        health_scores = [
            float(h.get("health_score") or 1.0) for h in health_by_format.values()
        ]
        min_health_score = min(health_scores) if health_scores else 1.0
        # 取最大的连续失败次数
        max_consecutive = max(
            (int(h.get("consecutive_failures") or 0) for h in health_by_format.values()),
            default=0,
        )
        # 取最近的失败时间
        failure_times = [
            h.get("last_failure_at")
            for h in health_by_format.values()
            if h.get("last_failure_at")
        ]
        last_failure = max(failure_times) if failure_times else None
    else:
        min_health_score = 1.0
        max_consecutive = 0
        last_failure = None

    # 检查是否有任何格式的熔断器打开
    any_circuit_open = any(c.get("open", False) for c in circuit_by_format.values())

    key_dict.update(
        {
            "api_key_masked": masked_key,
            "api_key_plain": api_key_plain,
            "success_rate": success_rate,
            "avg_response_time_ms": round(avg_response_time_ms, 2),
            "is_adaptive": is_adaptive,
            "effective_limit": (
                (key.learned_rpm_limit if key.learned_rpm_limit is not None else RPMDefaults.INITIAL_LIMIT)
                if is_adaptive
                else key.rpm_limit
            ),
            # 汇总字段
            "health_score": min_health_score,
            "consecutive_failures": max_consecutive,
            "last_failure_at": last_failure,
            "circuit_breaker_open": any_circuit_open,
        }
    )

    # 防御性：确保 api_formats 存在（历史数据可能为空/缺失）
    if "api_formats" not in key_dict or key_dict["api_formats"] is None:
        key_dict["api_formats"] = []

    return EndpointAPIKeyResponse(**key_dict)


@dataclass
class AdminListProviderKeysAdapter(AdminApiAdapter):
    """获取 Provider 的所有 Keys"""

    provider_id: str
    skip: int
    limit: int

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException(f"Provider {self.provider_id} 不存在")

        keys = (
            db.query(ProviderAPIKey)
            .filter(ProviderAPIKey.provider_id == self.provider_id)
            .order_by(ProviderAPIKey.internal_priority.asc(), ProviderAPIKey.created_at.asc())
            .offset(self.skip)
            .limit(self.limit)
            .all()
        )

        return [_build_key_response(key) for key in keys]


@dataclass
class AdminCreateProviderKeyAdapter(AdminApiAdapter):
    """为 Provider 添加 Key"""

    provider_id: str
    key_data: EndpointAPIKeyCreate

    async def handle(self, context):  # type: ignore[override]
        db = context.db
        provider = db.query(Provider).filter(Provider.id == self.provider_id).first()
        if not provider:
            raise NotFoundException(f"Provider {self.provider_id} 不存在")

        # 验证 api_formats 必填
        if not self.key_data.api_formats:
            raise InvalidRequestException("api_formats 为必填字段")

        # 允许同一个 API Key 在同一 Provider 下添加多次
        # 用户可以为不同的 API 格式创建独立的配置记录，便于分开管理

        encrypted_key = crypto_service.encrypt(self.key_data.api_key)
        now = datetime.now(timezone.utc)

        new_key = ProviderAPIKey(
            id=str(uuid.uuid4()),
            provider_id=self.provider_id,
            api_formats=self.key_data.api_formats,
            api_key=encrypted_key,
            name=self.key_data.name,
            note=self.key_data.note,
            rate_multiplier=self.key_data.rate_multiplier,
            rate_multipliers=self.key_data.rate_multipliers,  # 按 API 格式的成本倍率
            internal_priority=self.key_data.internal_priority,
            rpm_limit=self.key_data.rpm_limit,
            allowed_models=self.key_data.allowed_models if self.key_data.allowed_models else None,
            capabilities=self.key_data.capabilities if self.key_data.capabilities else None,
            cache_ttl_minutes=self.key_data.cache_ttl_minutes,
            max_probe_interval_minutes=self.key_data.max_probe_interval_minutes,
            request_count=0,
            success_count=0,
            error_count=0,
            total_response_time_ms=0,
            health_by_format={},  # 按格式存储健康度
            circuit_breaker_by_format={},  # 按格式存储熔断器状态
            is_active=True,
            last_used_at=None,
            created_at=now,
            updated_at=now,
        )

        db.add(new_key)
        db.commit()
        db.refresh(new_key)

        logger.info(
            f"[OK] 添加 Key: Provider={self.provider_id}, "
            f"Formats={self.key_data.api_formats}, Key=***{self.key_data.api_key[-4:]}, ID={new_key.id}"
        )

        return _build_key_response(new_key, api_key_plain=self.key_data.api_key)
