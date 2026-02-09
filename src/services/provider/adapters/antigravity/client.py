"""Antigravity API 客户端（与 Antigravity-Manager 对齐）。"""

from __future__ import annotations

import asyncio
import json
import random
import re
from typing import Any

from src.clients.http_client import HTTPClientPool
from src.core.logger import logger
from src.services.provider.adapters.antigravity.constants import (
    DAILY_BASE_URL,
    PROD_BASE_URL,
    SANDBOX_BASE_URL,
    VERSION_FETCH_URL,
    get_http_user_agent,
    parse_version_string,
    update_user_agent_version,
)
from src.services.provider.adapters.antigravity.url_availability import url_availability

# loadCodeAssist 请求体 metadata
_CODE_ASSIST_METADATA = {
    "ideType": "ANTIGRAVITY",
}

# Duration 解析正则（与 AM 的 retry.rs 对齐）
_DURATION_RE = re.compile(r"([\d.]+)\s*(ms|s|m|h)")


# ---------------------------------------------------------------------------
# Retry-After / Duration 解析工具（对齐 AM upstream/retry.rs）
# ---------------------------------------------------------------------------


def _parse_duration_ms(duration_str: str) -> int | None:
    """解析 Duration 字符串 (e.g. '1.5s', '200ms', '1h16m0.667s')，返回毫秒。"""
    total_ms = 0.0
    matched = False
    for m in _DURATION_RE.finditer(duration_str):
        matched = True
        value = float(m.group(1))
        unit = m.group(2)
        if unit == "ms":
            total_ms += value
        elif unit == "s":
            total_ms += value * 1000
        elif unit == "m":
            total_ms += value * 60_000
        elif unit == "h":
            total_ms += value * 3_600_000
    return round(total_ms) if matched else None


def parse_retry_delay(error_body: str | bytes | dict[str, Any]) -> float | None:
    """从 429 错误响应体中提取 retry delay（秒）。

    支持两种格式（与 AM 对齐）：
    1. error.details[].@type="...RetryInfo" → retryDelay
    2. error.details[].metadata.quotaResetDelay
    """
    try:
        data: dict[str, Any]
        if isinstance(error_body, (str, bytes)):
            data = json.loads(error_body)
        elif isinstance(error_body, dict):
            data = error_body
        else:
            return None

        details = data.get("error", {}).get("details", [])
        if not isinstance(details, list):
            return None

        # 方式 1: RetryInfo.retryDelay
        for detail in details:
            if not isinstance(detail, dict):
                continue
            type_str = detail.get("@type", "")
            if isinstance(type_str, str) and "RetryInfo" in type_str:
                retry_delay = detail.get("retryDelay")
                if isinstance(retry_delay, str):
                    ms = _parse_duration_ms(retry_delay)
                    if ms is not None:
                        return min((ms + 200) / 1000.0, 30.0)

        # 方式 2: metadata.quotaResetDelay
        for detail in details:
            if not isinstance(detail, dict):
                continue
            metadata = detail.get("metadata")
            if not isinstance(metadata, dict):
                continue
            quota_delay = metadata.get("quotaResetDelay")
            if isinstance(quota_delay, str):
                ms = _parse_duration_ms(quota_delay)
                if ms is not None:
                    return min((ms + 200) / 1000.0, 30.0)
    except Exception:
        pass
    return None


# ---------------------------------------------------------------------------
# Fallback 判断（对齐 AM upstream/client.rs: should_try_next_endpoint）
# ---------------------------------------------------------------------------


def _should_fallback_status(status_code: int) -> bool:
    """判断是否应该 fallback 到下一个端点。

    与 AM 对齐：429 + 408(超时) + 404 + 所有 5xx。
    4xx 客户端错误（400/401/403 等）不 fallback——换 URL 也不会成功。
    """
    if status_code == 429:
        return True
    if status_code in (404, 408):
        return True
    if 500 <= status_code < 600:  # noqa: PLR2004
        return True
    return False


# ---------------------------------------------------------------------------
# 账户封禁异常（对齐 AM: is_forbidden 标志）
# ---------------------------------------------------------------------------


class AntigravityAccountForbiddenException(Exception):
    """Antigravity 账户被封禁/禁止访问异常。

    当 API 返回 403 Forbidden 时抛出，表示账户权限被撤销。
    """

    def __init__(
        self,
        message: str = "账户访问被禁止",
        status_code: int = 403,
        reason: str | None = None,
    ):
        super().__init__(message)
        self.message = message
        self.status_code = status_code
        self.reason = reason


def _extract_forbidden_reason(response_text: str) -> str | None:
    """从 403 响应体中提取封禁原因。

    尝试解析 JSON 响应中的 error.message 字段。
    """
    if not response_text:
        return None
    try:
        data = json.loads(response_text)
        if isinstance(data, dict):
            error = data.get("error")
            if isinstance(error, dict):
                message = error.get("message")
                if isinstance(message, str) and message.strip():
                    return message.strip()
            # 直接在顶层查找 message
            message = data.get("message")
            if isinstance(message, str) and message.strip():
                return message.strip()
    except Exception:
        pass
    # 如果无法解析，返回原始文本的前 100 个字符
    if len(response_text) > 100:
        return response_text[:100] + "..."
    return response_text if response_text.strip() else None


# ---------------------------------------------------------------------------
# 从 loadCodeAssist 响应中提取信息
# ---------------------------------------------------------------------------


def extract_tier_id(data: dict[str, Any]) -> str:
    """从 loadCodeAssist 响应中提取 tier ID。

    优先选 allowedTiers 中 isDefault=true 的，fallback 到第一个，最终 fallback 到 "LEGACY"。
    """
    allowed_tiers = data.get("allowedTiers")
    if not isinstance(allowed_tiers, list):
        return "LEGACY"

    # 第一轮：找 isDefault
    for tier in allowed_tiers:
        if isinstance(tier, dict) and tier.get("isDefault") is True:
            tier_id = tier.get("id", "")
            if isinstance(tier_id, str) and tier_id.strip():
                return tier_id.strip()

    # 第二轮：取第一个有 id 的
    for tier in allowed_tiers:
        if isinstance(tier, dict):
            tier_id = tier.get("id", "")
            if isinstance(tier_id, str) and tier_id.strip():
                return tier_id.strip()

    return "LEGACY"


def extract_project_id(data: dict[str, Any]) -> str:
    """从响应中提取 project_id，兼容 string 和 {"id": "..."} 两种格式。"""
    raw = data.get("cloudaicompanionProject")
    if isinstance(raw, str) and raw.strip():
        return raw.strip()
    if isinstance(raw, dict):
        pid = raw.get("id", "")
        if isinstance(pid, str) and pid.strip():
            return pid.strip()
    return ""


# ---------------------------------------------------------------------------
# 核心 API 客户端函数
# ---------------------------------------------------------------------------


async def load_code_assist(
    access_token: str,
    proxy_config: dict[str, Any] | None = None,
    *,
    timeout_seconds: float = 10.0,
) -> dict[str, Any]:
    """调用 /v1internal:loadCodeAssist 获取账户信息。

    对齐 AM project_resolver.rs：Sandbox 优先，避免 Prod 429。

    注意：
    - email 需通过 Google userinfo API 获取（由 enrich_auth_config 复用已有逻辑）
    - 这里仅负责 project_id / tier 等信息
    """
    if not access_token:
        raise ValueError("missing access_token")

    client = await HTTPClientPool.get_proxy_client(proxy_config)

    headers = {
        "Authorization": f"Bearer {access_token}",
        "User-Agent": get_http_user_agent(),
        "Content-Type": "application/json",
    }
    body = {"metadata": _CODE_ASSIST_METADATA}

    # Sandbox 优先（与 AM 对齐：避免 Prod 429）
    urls = url_availability.get_ordered_urls(prefer_daily=True)
    if not urls:
        urls = [SANDBOX_BASE_URL, DAILY_BASE_URL, PROD_BASE_URL]
    last_exc: Exception | None = None

    for base_url in urls:
        try:
            resp = await client.post(
                f"{base_url}/v1internal:loadCodeAssist",
                json=body,
                headers=headers,
                timeout=timeout_seconds,
            )
            if 200 <= resp.status_code < 300:
                url_availability.mark_success(base_url)
                data = resp.json()
                return data if isinstance(data, dict) else {}

            # 可 fallback 的错误：标记不可用，尝试下一个 URL
            if _should_fallback_status(resp.status_code):
                url_availability.mark_unavailable(base_url)

                # 429：尝试解析 Retry-After 并等待
                if resp.status_code == 429:
                    delay = parse_retry_delay(resp.text)
                    if delay and delay > 0:
                        logger.debug(
                            "[antigravity] loadCodeAssist 429, waiting {:.1f}s before fallback",
                            delay,
                        )
                        await asyncio.sleep(min(delay, 5.0))

                last_exc = RuntimeError(
                    f"loadCodeAssist failed: status={resp.status_code} base_url={base_url}"
                )
                continue

            # 不可 fallback 的 4xx（400/401/403 等）：直接抛出，换 URL 也不会成功
            raise RuntimeError(
                f"loadCodeAssist failed: status={resp.status_code} base_url={base_url} "
                f"body={resp.text[:200] if resp.text else ''}"
            )
        except RuntimeError:
            raise
        except Exception as e:
            url_availability.mark_unavailable(base_url)
            last_exc = e
            continue

    raise last_exc or RuntimeError("loadCodeAssist failed: all endpoints exhausted")


async def onboard_user(
    access_token: str,
    tier_id: str = "LEGACY",
    proxy_config: dict[str, Any] | None = None,
    *,
    max_attempts: int = 5,
    poll_interval: float = 2.0,
    timeout_seconds: float = 30.0,
) -> str:
    """调用 /v1internal:onboardUser 激活账号并获取 project_id。

    当 loadCodeAssist 返回 allowedTiers 但没有 cloudaicompanionProject 时，
    需要先通过 onboardUser 选择 tier 来分配 project。

    改进（对齐 AM）：
    - 使用 url_availability 做多 URL fallback
    - 轮询期间的临时网络错误会 continue 而非直接终止

    Args:
        access_token: OAuth access token
        tier_id: 要选择的 tier ID（从 allowedTiers 中提取）
        proxy_config: 代理配置
        max_attempts: 最大轮询次数（onboardUser 是异步操作）
        poll_interval: 轮询间隔（秒）
        timeout_seconds: 单次请求超时

    Returns:
        project_id

    Raises:
        RuntimeError: 激活失败
    """
    if not access_token:
        raise ValueError("missing access_token")

    client = await HTTPClientPool.get_proxy_client(proxy_config)

    headers = {
        "Authorization": f"Bearer {access_token}",
        "User-Agent": get_http_user_agent(),
        "Content-Type": "application/json",
    }
    body = {
        "tierId": tier_id,
        "metadata": _CODE_ASSIST_METADATA,
    }

    # 使用 url_availability 选择端点（与其他函数一致）
    urls = url_availability.get_ordered_urls(prefer_daily=True)
    if not urls:
        urls = [SANDBOX_BASE_URL, DAILY_BASE_URL, PROD_BASE_URL]
    base_url = urls[0]

    logger.info("[antigravity] onboardUser: 开始激活账号, tier={}, endpoint={}", tier_id, base_url)

    for attempt in range(1, max_attempts + 1):
        try:
            resp = await client.post(
                f"{base_url}/v1internal:onboardUser",
                json=body,
                headers=headers,
                timeout=timeout_seconds,
            )
            if resp.status_code < 200 or resp.status_code >= 300:
                # 标记可用性
                if _should_fallback_status(resp.status_code):
                    url_availability.mark_unavailable(base_url)
                raise RuntimeError(
                    f"onboardUser failed: status={resp.status_code}, "
                    f"body={resp.text[:200] if resp.text else ''}"
                )

            url_availability.mark_success(base_url)
            data = resp.json()
            if not isinstance(data, dict):
                raise RuntimeError(f"onboardUser: unexpected response type: {type(data)}")

            done = data.get("done")
            if done is True:
                # 从 response.cloudaicompanionProject 提取 project_id
                response_data = data.get("response")
                if isinstance(response_data, dict):
                    project_id = extract_project_id(response_data)
                    if project_id:
                        logger.info(
                            "[antigravity] onboardUser: 激活成功, project_id={}",
                            project_id[:8] + "...",
                        )
                        return project_id

                # done=true 但 project_id 为空（常见：cloudaicompanionProject: {}）
                # 返回空串，由调用方 fallback 到随机 project_id
                logger.debug("[antigravity] onboardUser: done=true 但 project_id 为空")
                return ""

            # done != true，继续轮询
            logger.debug(
                "[antigravity] onboardUser: 轮询 {}/{}, 等待完成...",
                attempt,
                max_attempts,
            )
            if attempt < max_attempts:
                await asyncio.sleep(poll_interval)

        except RuntimeError:
            raise
        except Exception as e:
            # 临时网络错误：记录并继续轮询（而非直接终止）
            logger.warning(
                "[antigravity] onboardUser: 轮询 {}/{} 网络错误: {}, 继续重试...",
                attempt,
                max_attempts,
                e,
            )
            if attempt < max_attempts:
                await asyncio.sleep(poll_interval)
                continue
            raise RuntimeError(
                f"onboardUser request failed after {max_attempts} attempts: {e}"
            ) from e

    raise RuntimeError(f"onboardUser: 超时，已轮询 {max_attempts} 次仍未完成")


async def fetch_available_models(
    access_token: str,
    *,
    project_id: str,
    proxy_config: dict[str, Any] | None = None,
    timeout_seconds: float = 10.0,
) -> dict[str, Any]:
    """调用 /v1internal:fetchAvailableModels 获取可用模型（包含配额信息）。

    Antigravity 的此接口会返回类似：
        {"models": {"claude-sonnet-4": {"displayName": "...", "quotaInfo": {...}}, ...}}

    对齐 AM：Sandbox 优先 + 正确的 fallback 逻辑。
    """
    if not access_token:
        raise ValueError("missing access_token")
    if not project_id:
        raise ValueError("missing project_id")

    client = await HTTPClientPool.get_proxy_client(proxy_config)

    headers = {
        "Authorization": f"Bearer {access_token}",
        "User-Agent": get_http_user_agent(),
        "Content-Type": "application/json",
        "Accept": "application/json",
    }
    body = {"project": project_id}

    # Sandbox 优先
    urls = url_availability.get_ordered_urls(prefer_daily=True)
    if not urls:
        urls = [SANDBOX_BASE_URL, DAILY_BASE_URL, PROD_BASE_URL]
    last_exc: Exception | None = None

    for base_url in urls:
        try:
            resp = await client.post(
                f"{base_url}/v1internal:fetchAvailableModels",
                json=body,
                headers=headers,
                timeout=timeout_seconds,
            )
            if 200 <= resp.status_code < 300:
                url_availability.mark_success(base_url)
                data = resp.json()
                return data if isinstance(data, dict) else {}

            # 可 fallback 的错误
            if _should_fallback_status(resp.status_code):
                url_availability.mark_unavailable(base_url)

                # 429：尝试等待
                if resp.status_code == 429:
                    delay = parse_retry_delay(resp.text)
                    if delay and delay > 0:
                        logger.debug(
                            "[antigravity] fetchAvailableModels 429, waiting {:.1f}s",
                            delay,
                        )
                        await asyncio.sleep(min(delay, 5.0))

                last_exc = RuntimeError(
                    f"fetchAvailableModels failed: status={resp.status_code} base_url={base_url}"
                )
                continue

            # 403 Forbidden：账户权限被禁止（对齐 AM is_forbidden 标志）
            if resp.status_code == 403:
                reason = _extract_forbidden_reason(resp.text)
                logger.warning(
                    "[antigravity] fetchAvailableModels 403 Forbidden: {}",
                    reason or "unknown",
                )
                raise AntigravityAccountForbiddenException(
                    message="账户访问被禁止",
                    status_code=403,
                    reason=reason,
                )

            # 不可 fallback 的 4xx：直接抛出
            raise RuntimeError(
                f"fetchAvailableModels failed: status={resp.status_code} base_url={base_url} "
                f"body={resp.text[:200] if resp.text else ''}"
            )
        except (RuntimeError, AntigravityAccountForbiddenException):
            raise
        except Exception as e:
            url_availability.mark_unavailable(base_url)
            last_exc = e
            continue

    raise last_exc or RuntimeError("fetchAvailableModels failed: all endpoints exhausted")


# ---------------------------------------------------------------------------
# User-Agent 版本号动态更新
# ---------------------------------------------------------------------------


async def refresh_user_agent(
    proxy_config: dict[str, Any] | None = None,
    *,
    timeout_seconds: float = 5.0,
) -> str | None:
    """从远程 API 获取最新 Antigravity 版本号并更新 User-Agent。

    对齐 AM constants.rs：
    1. 尝试 VERSION_FETCH_URL
    2. Fallback 到 changelog 页面
    3. 最终 fallback 到 _FALLBACK_VERSION

    Returns:
        更新后的版本号，或 None（如果获取失败但 fallback 到默认版本）。
    """
    try:
        client = await HTTPClientPool.get_proxy_client(proxy_config)
        resp = await client.get(VERSION_FETCH_URL, timeout=timeout_seconds)
        if 200 <= resp.status_code < 300:
            version = parse_version_string(resp.text)
            if version:
                update_user_agent_version(version)
                logger.info("[antigravity] User-Agent 版本已更新: {}", version)
                return version
    except Exception as e:
        logger.debug("[antigravity] 获取远程版本失败: {}", e)

    return None


# ---------------------------------------------------------------------------
# Fallback Project ID 生成
# ---------------------------------------------------------------------------

_ADJECTIVES = ("useful", "bright", "swift", "calm", "bold")
_NOUNS = ("fuze", "wave", "spark", "flow", "core")


def generate_fallback_project_id() -> str:
    """生成随机 project_id（与 AM project_resolver.rs 的 generateProjectID 一致）。

    格式: {adjective}-{noun}-{5位随机字符(base36)}
    Antigravity API 不严格校验 project 字段，当所有正常获取途径都失败时用作 fallback。
    """
    adj = random.choice(_ADJECTIVES)  # noqa: S311
    noun = random.choice(_NOUNS)  # noqa: S311
    chars = "abcdefghijklmnopqrstuvwxyz0123456789"
    rand_part = "".join(random.choice(chars) for _ in range(5))  # noqa: S311
    return f"{adj}-{noun}-{rand_part}"


__all__ = [
    "AntigravityAccountForbiddenException",
    "extract_project_id",
    "extract_tier_id",
    "fetch_available_models",
    "generate_fallback_project_id",
    "load_code_assist",
    "onboard_user",
    "parse_retry_delay",
    "refresh_user_agent",
]
