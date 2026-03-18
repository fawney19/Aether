"""
Provider Key 响应对象构建器。
"""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any

from src.core.crypto import crypto_service
from src.core.logger import logger
from src.core.provider_oauth_utils import normalize_oauth_organizations
from src.models.database import ProviderAPIKey
from src.models.endpoint_models import EndpointAPIKeyResponse
from src.services.provider_keys.auth_type import normalize_auth_type


def build_key_response(
    key: ProviderAPIKey, api_key_plain: str | None = None
) -> EndpointAPIKeyResponse:
    """构建 Key 响应对象。"""
    auth_type = normalize_auth_type(getattr(key, "auth_type", "api_key"))
    encrypted_api_key = str(getattr(key, "api_key", "") or "")
    request_count = int(getattr(key, "request_count", 0) or 0)
    success_count = int(getattr(key, "success_count", 0) or 0)
    total_response_time_ms = float(getattr(key, "total_response_time_ms", 0) or 0.0)
    rpm_limit = getattr(key, "rpm_limit", None)

    if auth_type in ("service_account", "vertex_ai"):
        # Service Account 不显示占位符
        masked_key = "[Service Account]"
    elif auth_type == "oauth":
        masked_key = "[OAuth Token]"
    else:
        try:
            decrypted_key = crypto_service.decrypt(encrypted_api_key)
            masked_key = f"{decrypted_key[:8]}***{decrypted_key[-4:]}"
        except Exception:
            masked_key = "***ERROR***"

    success_rate = success_count / request_count if request_count > 0 else 0.0
    avg_response_time_ms = (
        total_response_time_ms / success_count if success_count > 0 else 0.0
    )

    is_adaptive = rpm_limit is None
    key_dict: dict[str, Any] = dict(getattr(key, "__dict__", {}))
    key_dict.pop("_sa_instance_state", None)
    key_dict.pop("api_key", None)  # 移除敏感字段，避免泄露
    key_dict["auth_type"] = auth_type

    # 提取 OAuth 元数据（如果是 OAuth 类型）
    oauth_expires_at = None
    oauth_email = None
    oauth_plan_type = None
    oauth_account_id = None
    oauth_account_name = None
    oauth_account_user_id = None
    oauth_organizations: list[dict[str, object]] = []
    encrypted_auth_config = key_dict.pop("auth_config", None)  # 移除敏感字段，避免泄露
    if (
        auth_type == "oauth"
        and isinstance(encrypted_auth_config, str)
        and encrypted_auth_config
    ):
        try:
            decrypted_config = crypto_service.decrypt(encrypted_auth_config)
            auth_config = json.loads(decrypted_config)
            oauth_expires_at = auth_config.get("expires_at")
            oauth_email = auth_config.get("email")
            oauth_plan_type = auth_config.get(
                "plan_type"
            )  # Codex: plus/free/team/enterprise
            # Antigravity 使用 "tier" 字段（如 "PAID"/"FREE"），做小写化 fallback
            if not oauth_plan_type:
                ag_tier = auth_config.get("tier")
                if ag_tier and isinstance(ag_tier, str):
                    oauth_plan_type = ag_tier.lower()
            oauth_account_id = auth_config.get(
                "account_id"
            )  # Codex: chatgpt_account_id
            oauth_account_name = auth_config.get("account_name")
            oauth_account_user_id = auth_config.get("account_user_id")
            oauth_organizations = normalize_oauth_organizations(
                auth_config.get("organizations")
            )
        except Exception as e:
            logger.error("Failed to decrypt auth_config for key {}: {}", key.id, e)

    # 从 health_by_format 计算汇总字段（便于列表展示）
    raw_health_by_format = getattr(key, "health_by_format", None)
    health_by_format = (
        raw_health_by_format if isinstance(raw_health_by_format, dict) else {}
    )
    raw_circuit_by_format = getattr(key, "circuit_breaker_by_format", None)
    circuit_by_format = (
        raw_circuit_by_format if isinstance(raw_circuit_by_format, dict) else {}
    )

    # 计算整体健康度（取所有格式中的最低值）
    if health_by_format:
        health_scores = [
            float(h.get("health_score") or 1.0) for h in health_by_format.values()
        ]
        min_health_score = min(health_scores) if health_scores else 1.0
        # 取最大的连续失败次数
        max_consecutive = max(
            (
                int(h.get("consecutive_failures") or 0)
                for h in health_by_format.values()
            ),
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
                getattr(
                    key, "learned_rpm_limit", None
                )  # 自适应模式：使用学习值，未学习时为 None（不限制）
                if is_adaptive
                else rpm_limit
            ),
            # 汇总字段
            "health_score": min_health_score,
            "consecutive_failures": max_consecutive,
            "last_failure_at": last_failure,
            "circuit_breaker_open": any_circuit_open,
            # OAuth 相关
            "oauth_expires_at": oauth_expires_at,
            "oauth_email": oauth_email,
            "oauth_plan_type": oauth_plan_type,
            "oauth_account_id": oauth_account_id,
            "oauth_account_name": oauth_account_name,
            "oauth_account_user_id": oauth_account_user_id,
            "oauth_organizations": oauth_organizations,
            "oauth_invalid_at": (
                int(oauth_invalid_at.timestamp())
                if isinstance(
                    (oauth_invalid_at := getattr(key, "oauth_invalid_at", None)),
                    datetime,
                )
                else None
            ),
            "oauth_invalid_reason": getattr(key, "oauth_invalid_reason", None),
        }
    )

    # 防御性：确保 api_formats 存在（历史数据可能为空/缺失）
    if "api_formats" not in key_dict or key_dict["api_formats"] is None:
        key_dict["api_formats"] = []

    return EndpointAPIKeyResponse(**key_dict)
