"""Kiro usage/quota fetching utilities."""

from __future__ import annotations

import re
import time
import uuid
from typing import Any

import httpx

from src.clients.http_client import HTTPClientPool
from src.core.logger import logger
from src.services.provider.adapters.kiro.headers import (
    build_user_agent_usage,
    build_x_amz_user_agent_usage,
)
from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
from src.services.provider.adapters.kiro.models.usage_limits import (
    UsageLimitsResponse,
    calculate_current_usage,
    calculate_total_usage_limit,
)
from src.services.provider.adapters.kiro.token_manager import (
    generate_machine_id,
    is_token_expired,
    refresh_access_token,
)


class KiroAccountBannedException(Exception):
    """Kiro 账户被封禁异常"""

    def __init__(
        self,
        message: str = "账户已封禁",
        status_code: int = 403,
        reason: str | None = None,
    ):
        super().__init__(message)
        self.message = message
        self.status_code = status_code
        self.reason = reason


async def fetch_kiro_usage_limits(
    auth_config: dict[str, Any],
    proxy_config: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """
    调用 Kiro getUsageLimits API 获取使用额度信息

    Args:
        auth_config: 解密后的 KiroAuthConfig 数据
        proxy_config: 代理配置（可选）

    Returns:
        包含 usage_data 和 updated_auth_config 的字典

    Raises:
        RuntimeError: 请求失败时抛出
    """
    cfg = KiroAuthConfig.from_dict(auth_config)

    # 检查是否有缓存的 access_token 且未过期
    access_token: str | None = None
    updated_cfg: KiroAuthConfig | None = None

    if cfg.access_token and not is_token_expired(cfg.expires_at):
        # 使用缓存的 token
        access_token = cfg.access_token
        updated_cfg = cfg
        logger.debug("[KIRO_QUOTA] 使用缓存的 access_token")
    else:
        # token 过期或不存在，需要刷新
        logger.debug("[KIRO_QUOTA] Token 已过期或不存在，正在刷新...")
        access_token, updated_cfg = await refresh_access_token(cfg, proxy_config=proxy_config)

    if not access_token:
        raise RuntimeError("无法获取 Kiro access_token")

    # 构建请求
    from src.services.provider.adapters.kiro.constants import DEFAULT_REGION

    region = (updated_cfg.region if updated_cfg else cfg.region) or DEFAULT_REGION
    host = f"q.{region}.amazonaws.com"
    machine_id = generate_machine_id(updated_cfg or cfg)
    kiro_version = (updated_cfg.kiro_version if updated_cfg else cfg.kiro_version) or "0.8.0"

    # 构建 URL（添加 isEmailRequired=true 获取邮箱）
    url = f"https://{host}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST&isEmailRequired=true"

    profile_arn = updated_cfg.profile_arn if updated_cfg else cfg.profile_arn
    if profile_arn:
        from urllib.parse import quote

        url += f"&profileArn={quote(profile_arn, safe='')}"

    # 构建 headers
    headers = {
        "x-amz-user-agent": build_x_amz_user_agent_usage(
            kiro_version=kiro_version, machine_id=machine_id
        ),
        "User-Agent": build_user_agent_usage(kiro_version=kiro_version, machine_id=machine_id),
        "host": host,
        "amz-sdk-invocation-id": str(uuid.uuid4()),
        "amz-sdk-request": "attempt=1; max=1",
        "Authorization": f"Bearer {access_token}",
        "Connection": "close",
    }

    client = await HTTPClientPool.get_proxy_client(proxy_config=proxy_config)
    response = await client.get(url, headers=headers, timeout=httpx.Timeout(30.0))

    if response.status_code != 200:
        response_text = (response.text or "").strip()

        # 检测账户封禁（403/423 均视为封禁）
        # 对于 Kiro，403 表示账户权限被拒绝（无论是封禁、权限不足还是其他原因），
        # 都应该标记为异常状态，以便管理员及时处理
        is_banned = False
        ban_reason = None
        if response.status_code in (403, 423):
            is_banned = True
            # 检测封禁相关错误（用于提供更详细的原因说明）
            banned_keywords = [
                "AccountSuspendedException",
                "account.*suspend",
                "account.*banned",
                "account.*disabled",
                "account.*access.*denied",
            ]
            for keyword in banned_keywords:
                if re.search(keyword, response_text, re.IGNORECASE):
                    ban_reason = (
                        response_text[:200] if response_text else f"HTTP {response.status_code}"
                    )
                    break
            # 如果没有匹配到特定关键词，使用通用原因
            if not ban_reason:
                if response.status_code == 423:
                    ban_reason = response_text[:200] if response_text else "HTTP 423 Locked"
                else:
                    ban_reason = (
                        response_text[:200] if response_text else "HTTP 403 权限被拒绝"
                    )

        error_msg = {
            401: "认证失败，Token 无效或已过期",
            403: "账户异常，权限被拒绝",
            423: "账户已封禁",
            429: "请求过于频繁，已被限流",
        }.get(response.status_code, "获取使用额度失败")
        if 500 <= response.status_code < 600:
            error_msg = "服务器错误，AWS 服务暂时不可用"
        logger.debug(
            "kiro usage API error: HTTP {} | {}",
            response.status_code,
            response_text[:200],
        )

        # 如果检测到封禁，抛出带有封禁标记的异常
        if is_banned:
            raise KiroAccountBannedException(
                message=error_msg,
                status_code=response.status_code,
                reason=ban_reason,
            )

        raise RuntimeError(f"{error_msg}: HTTP {response.status_code}")

    try:
        data = response.json()
    except Exception as exc:
        raise RuntimeError(f"获取使用额度成功但响应解析失败: HTTP {response.status_code}") from exc

    # 返回刷新后的配置（用于更新 auth_config）
    return {
        "usage_data": data,
        "updated_auth_config": updated_cfg.to_dict() if updated_cfg else None,
    }


def parse_kiro_usage_response(data: dict) -> dict | None:
    """
    解析 Kiro getUsageLimits API 响应，提取限额信息和用户邮箱

    返回格式与 kiro.rs BalanceResponse 类似：
    - subscription_title: 订阅类型（如 "KIRO PRO+"）
    - current_usage: 当前使用量
    - usage_limit: 使用限额
    - remaining: 剩余额度
    - usage_percentage: 使用百分比
    - next_reset_at: 下次重置时间（Unix 时间戳）
    - email: 用户邮箱（通过 isEmailRequired=true 获取）
    """
    if not data:
        return None

    usage_resp = UsageLimitsResponse.from_dict(data)

    current_usage = calculate_current_usage(usage_resp)
    usage_limit = calculate_total_usage_limit(usage_resp)
    remaining = max(usage_limit - current_usage, 0.0)
    usage_percentage = (current_usage / usage_limit * 100.0) if usage_limit > 0 else 0.0
    usage_percentage = min(usage_percentage, 100.0)

    result: dict[str, Any] = {
        "current_usage": current_usage,
        "usage_limit": usage_limit,
        "remaining": remaining,
        "usage_percentage": usage_percentage,
    }

    # 订阅类型
    if usage_resp.subscription_info and usage_resp.subscription_info.subscription_title:
        result["subscription_title"] = usage_resp.subscription_info.subscription_title

    # 下次重置时间
    if usage_resp.next_date_reset is not None:
        result["next_reset_at"] = usage_resp.next_date_reset
    elif usage_resp.usage_breakdown_list and usage_resp.usage_breakdown_list[0].next_date_reset:
        result["next_reset_at"] = usage_resp.usage_breakdown_list[0].next_date_reset

    # 解析用户邮箱（从 desktopUserInfo 或 userInfo 中获取）
    user_info = data.get("desktopUserInfo") or data.get("userInfo") or {}
    if isinstance(user_info, dict):
        email = user_info.get("email")
        if isinstance(email, str) and email.strip():
            result["email"] = email.strip()

    # 添加更新时间戳
    result["updated_at"] = int(time.time())

    return result


__all__ = [
    "KiroAccountBannedException",
    "fetch_kiro_usage_limits",
    "parse_kiro_usage_response",
]
