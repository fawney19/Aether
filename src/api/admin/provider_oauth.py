"""管理员 Provider OAuth 管理 API。

用于固定类型 Provider 的 OAuth2 授权：
- start: 生成授权 URL（PKCE/state）
- complete: 粘贴 callback_url 完成换 token
- refresh: 手动强制刷新 token

注意：
- 该模块是“上游 Provider OAuth（用于反代调用）”，不是用户登录/绑定 OAuth。
- 不得在日志或响应中返回 access_token/refresh_token/client_secret。
"""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import secrets
import time
import uuid
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any, Literal
from urllib.parse import parse_qsl, urlencode, urlparse

import httpx
from fastapi import APIRouter, Depends, Request
from fastapi.concurrency import run_in_threadpool
from pydantic import BaseModel, Field
from redis.asyncio import Redis
from sqlalchemy.orm import Session

from src.clients.redis_client import get_redis_client
from src.core.crypto import crypto_service
from src.core.exceptions import InvalidRequestException, NotFoundException
from src.core.logger import logger
from src.core.provider_oauth_utils import enrich_auth_config, post_oauth_token
from src.core.provider_templates.fixed_providers import FIXED_PROVIDERS
from src.core.provider_templates.types import ProviderType
from src.database import get_db_context
from src.database.database import create_session, get_db
from src.models.database import Provider, ProviderAPIKey, User
from src.services.provider.pool.config import parse_pool_config
from src.services.provider_keys.auth_type import OAUTH_AUTH_TYPES
from src.services.scheduling.utils import release_db_connection_before_await
from src.utils.async_utils import safe_create_task
from src.utils.auth_utils import require_admin

router = APIRouter(prefix="/api/admin/provider-oauth", tags=["Provider OAuth"])


def _store_completed_oauth_sync(
    key_id: str,
    provider_type: str,
    access_token: str,
    auth_config: dict[str, Any],
) -> None:
    with get_db_context() as db:
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
        if not key:
            raise NotFoundException("Key 不存在", "key")
        key.api_key = crypto_service.encrypt(access_token)
        key.auth_config = crypto_service.encrypt(json.dumps(auth_config))


def _mark_refresh_failed_sync(key_id: str, reason: str) -> None:
    with get_db_context() as db:
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
        if not key:
            raise NotFoundException("Key 不存在", "key")
        key.oauth_invalid_at = datetime.now(timezone.utc)
        key.oauth_invalid_reason = reason


def _store_refreshed_oauth_sync(
    key_id: str,
    access_token: str,
    parsed_auth_config: dict[str, Any],
) -> None:
    with get_db_context() as db:
        key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
        if not key:
            raise NotFoundException("Key 不存在", "key")

        key.api_key = crypto_service.encrypt(access_token)
        key.auth_config = crypto_service.encrypt(json.dumps(parsed_auth_config))
        # 刷新成功 => 清除所有 oauth_invalid 标记（包括 [ACCOUNT_BLOCK]）。
        # Token 能成功刷新说明账号可用，之前的 block 标记应视为过时。
        if getattr(key, "oauth_invalid_at", None) is not None:
            key.oauth_invalid_at = None
            key.oauth_invalid_reason = None
        key.is_active = True


# ==============================================================================
# Redis state storage
# ==============================================================================

_PROVIDER_OAUTH_STATE_TTL_SECONDS = 600
_PROVIDER_OAUTH_STATE_PREFIX = "provider_oauth_state:"


_CONSUME_STATE_SCRIPT = r"""
local value = redis.call("GET", KEYS[1])
if value then
    redis.call("DEL", KEYS[1])
end
return value
"""


def _state_key(nonce: str) -> str:
    return f"{_PROVIDER_OAUTH_STATE_PREFIX}{nonce}"


@dataclass(frozen=True)
class ProviderOAuthStateData:
    nonce: str
    key_id: str  # 可能为空（新流程）
    provider_id: str  # 新增
    provider_type: str
    pkce_verifier: str | None
    created_at: int


async def _create_state(
    redis: Redis,
    *,
    key_id: str,
    provider_id: str,
    provider_type: str,
    pkce_verifier: str | None,
) -> str:
    nonce = secrets.token_urlsafe(24)
    data = {
        "nonce": nonce,
        "key_id": key_id,
        "provider_id": provider_id,
        "provider_type": provider_type,
        "pkce_verifier": pkce_verifier,
        "created_at": int(time.time()),
    }
    await redis.setex(_state_key(nonce), _PROVIDER_OAUTH_STATE_TTL_SECONDS, json.dumps(data))
    return nonce


async def _consume_state(redis: Redis, nonce: str) -> ProviderOAuthStateData | None:
    if not nonce:
        return None
    key = _state_key(nonce)
    raw = await redis.eval(_CONSUME_STATE_SCRIPT, 1, key)
    if not raw:
        return None
    try:
        parsed = json.loads(raw)
    except Exception:
        return None
    return ProviderOAuthStateData(
        nonce=str(parsed.get("nonce") or ""),
        key_id=str(parsed.get("key_id") or ""),
        provider_id=str(parsed.get("provider_id") or ""),
        provider_type=str(parsed.get("provider_type") or ""),
        pkce_verifier=parsed.get("pkce_verifier"),
        created_at=int(parsed.get("created_at") or 0),
    )


# ==============================================================================
# Batch import async task storage
# ==============================================================================

_PROVIDER_OAUTH_BATCH_TASK_PREFIX = "provider_oauth_batch_task:"
_PROVIDER_OAUTH_BATCH_TASK_TTL_SECONDS = 24 * 3600
_PROVIDER_OAUTH_BATCH_TASK_MAX_ERROR_SAMPLES = 20
_PROVIDER_OAUTH_DEFAULT_TIMEOUT_SECONDS = 30.0
_PROVIDER_OAUTH_BATCH_IMPORT_PROXY_TIMEOUT_SECONDS = 60.0
_PROVIDER_OAUTH_BATCH_IMPORT_COMMIT_BATCH_SIZE = 25
_PROVIDER_OAUTH_BATCH_TASK_ALLOWED_STATUSES = {
    "submitted",
    "processing",
    "completed",
    "failed",
}
_in_memory_batch_tasks: dict[str, tuple[int, dict[str, Any]]] = {}
_in_flight_batch_import_tasks: set[asyncio.Task[Any]] = set()


def _batch_task_key(task_id: str) -> str:
    return f"{_PROVIDER_OAUTH_BATCH_TASK_PREFIX}{task_id}"


def _cleanup_in_memory_batch_tasks(now_ts: int | None = None) -> None:
    ts = now_ts or int(time.time())
    expired = [k for k, (expire_at, _) in _in_memory_batch_tasks.items() if expire_at <= ts]
    for key in expired:
        _in_memory_batch_tasks.pop(key, None)


async def _save_batch_task_state(
    task_id: str,
    state: dict[str, Any],
    *,
    redis: Redis | None = None,
) -> None:
    now_ts = int(time.time())
    _cleanup_in_memory_batch_tasks(now_ts)
    state["updated_at"] = now_ts
    payload = json.dumps(state, ensure_ascii=False)

    redis_client = redis if redis is not None else await get_redis_client(require_redis=False)
    if redis_client is not None:
        try:
            await redis_client.setex(
                _batch_task_key(task_id),
                _PROVIDER_OAUTH_BATCH_TASK_TTL_SECONDS,
                payload,
            )
            return
        except Exception as exc:
            logger.debug("[BATCH_IMPORT_TASK] redis setex failed, fallback to memory: {}", exc)

    _in_memory_batch_tasks[task_id] = (
        now_ts + _PROVIDER_OAUTH_BATCH_TASK_TTL_SECONDS,
        json.loads(payload),
    )


async def _load_batch_task_state(
    task_id: str, *, redis: Redis | None = None
) -> dict[str, Any] | None:
    now_ts = int(time.time())
    _cleanup_in_memory_batch_tasks(now_ts)
    redis_client = redis if redis is not None else await get_redis_client(require_redis=False)
    if redis_client is not None:
        try:
            raw = await redis_client.get(_batch_task_key(task_id))
        except Exception as exc:
            logger.debug("[BATCH_IMPORT_TASK] redis get failed, fallback to memory: {}", exc)
            raw = None
        if raw:
            try:
                parsed = json.loads(raw)
                if isinstance(parsed, dict):
                    return parsed
            except Exception:
                return None

    record = _in_memory_batch_tasks.get(task_id)
    if not record:
        return None
    expire_at, state = record
    if expire_at <= now_ts:
        _in_memory_batch_tasks.pop(task_id, None)
        return None
    return dict(state)


# ==============================================================================
# Requests / responses
# ==============================================================================


class StartOAuthResponse(BaseModel):
    authorization_url: str
    redirect_uri: str
    provider_type: str
    instructions: str


class CompleteOAuthRequest(BaseModel):
    callback_url: str = Field(..., min_length=5, description="浏览器地址栏中的完整回调 URL")


class CompleteOAuthResponse(BaseModel):
    provider_type: str
    expires_at: int | None = None
    has_refresh_token: bool = False
    email: str | None = None


class ProviderCompleteOAuthRequest(BaseModel):
    callback_url: str = Field(..., min_length=5, description="浏览器地址栏中的完整回调 URL")
    name: str | None = Field(None, max_length=100, description="账号名称（可选）")
    proxy_node_id: str | None = Field(
        None,
        description="代理节点 ID（可选）。设置后 token 交换及后续所有操作（刷新、额度查询）均走该代理，避免 IP 污染",
    )


class ProviderCompleteOAuthResponse(BaseModel):
    key_id: str
    provider_type: str
    expires_at: int | None = None
    has_refresh_token: bool = False
    email: str | None = None
    replaced: bool = False


# ==============================================================================
# Helpers
# ==============================================================================


def _get_fixed_template(provider_type: str) -> Any | None:
    try:
        return FIXED_PROVIDERS.get(ProviderType(provider_type))
    except Exception:
        return None


def _supports_oauth(template: Any | None) -> bool:
    if not template:
        return False
    oauth = getattr(template, "oauth", None)
    if oauth is None:
        return False
    return bool(
        str(getattr(oauth, "authorize_url", "") or "").strip()
        and str(getattr(oauth, "token_url", "") or "").strip()
        and str(getattr(oauth, "client_id", "") or "").strip()
    )


def _require_fixed_provider(provider: Provider) -> str:
    provider_type = str(getattr(provider, "provider_type", "custom") or "custom").strip().lower()
    if not _get_fixed_template(provider_type):
        raise InvalidRequestException("该 Provider 不是固定类型，无法使用 provider-oauth")
    return provider_type


def _require_oauth_template(provider_type: str) -> Any:
    template = _get_fixed_template(provider_type)
    if not template:
        raise InvalidRequestException("不支持的 provider_type")
    if not _supports_oauth(template):
        raise InvalidRequestException("该 Provider 不支持 OAuth 授权")
    return template


def _resolve_proxy_for_oauth(
    provider_proxy: dict[str, Any] | None,
    proxy_node_id: str | None,
) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    """解析 OAuth 操作使用的代理配置。

    当前端指定了 proxy_node_id 时，优先使用该代理进行 token 交换等操作，
    并返回需要保存到 Key 上的代理配置。

    Args:
        provider_proxy: Provider 级别的代理配置
        proxy_node_id: 前端指定的代理节点 ID（可选）

    Returns:
        (effective_proxy, key_proxy):
            - effective_proxy: 本次操作实际使用的代理配置
            - key_proxy: 需要保存到 Key 上的代理配置（None 表示不设置 Key 级代理）
    """
    if proxy_node_id and proxy_node_id.strip():
        key_proxy: dict[str, Any] = {"node_id": proxy_node_id.strip(), "enabled": True}
        # 本次操作使用 Key 级代理
        return key_proxy, key_proxy
    # 无 Key 级代理，使用 Provider 级代理
    return provider_proxy, None


def _resolve_batch_import_timeout_seconds(proxy_config: dict[str, Any] | None) -> float:
    """返回 OAuth 批量导入 token 刷新超时。

    走代理链路时延更高，适当放宽超时，减少批量导入误超时。
    """
    if not proxy_config:
        return _PROVIDER_OAUTH_DEFAULT_TIMEOUT_SECONDS
    if isinstance(proxy_config, dict):
        if not proxy_config.get("enabled", True):
            return _PROVIDER_OAUTH_DEFAULT_TIMEOUT_SECONDS
        return _PROVIDER_OAUTH_BATCH_IMPORT_PROXY_TIMEOUT_SECONDS
    # 兼容历史数据：非 dict 但存在代理配置时同样使用放宽超时
    return _PROVIDER_OAUTH_BATCH_IMPORT_PROXY_TIMEOUT_SECONDS


def _pkce_s256(verifier: str) -> str:
    digest = hashlib.sha256(verifier.encode("utf-8")).digest()
    return base64.urlsafe_b64encode(digest).decode("utf-8").rstrip("=")


def _parse_callback_params(callback_url: str) -> dict[str, str]:
    parsed = urlparse(callback_url.strip())
    query = dict(parse_qsl(parsed.query, keep_blank_values=True))
    fragment = dict(parse_qsl((parsed.fragment or "").lstrip("#"), keep_blank_values=True))
    merged = {**query, **fragment}

    # Claude 参考实现里：code 参数可能包含 "<code>#<state>" 的拼接形式
    code = merged.get("code")
    if code and "#" in code:
        code_part, state_part = code.split("#", 1)
        merged["code"] = code_part
        if "state" not in merged and state_part:
            merged["state"] = state_part

    return {str(k): str(v) for k, v in merged.items()}


# ==============================================================================
# Shared helpers
# ==============================================================================


def _get_provider_api_formats(provider: Provider) -> list[str]:
    """从 Provider 的活跃 endpoints 中提取所有 api_format。"""
    return [
        ep.api_format
        for ep in provider.endpoints
        if getattr(ep, "api_format", None) and getattr(ep, "is_active", False)
    ]


def _create_oauth_key(
    db: Session,
    *,
    provider_id: str,
    name: str,
    access_token: str,
    auth_config: dict[str, Any],
    api_formats: list[str],
    flush_only: bool = False,
    proxy: dict[str, Any] | None = None,
    auto_fetch_models: bool = False,
) -> "ProviderAPIKey":
    """创建 OAuth Key 记录并持久化。

    Args:
        flush_only: True 时仅 flush（批量导入场景），False 时 commit + refresh。
        proxy: Key 级别代理配置（如 {"node_id": "xxx", "enabled": True}），
               创建时设置后，后续 token 刷新、额度刷新等操作立即走代理，避免 IP 污染。
        auto_fetch_models: 是否启用自动获取上游模型，默认关闭。
    """
    from src.models.database import ProviderAPIKey as ProviderAPIKeyModel

    new_key = ProviderAPIKeyModel(
        provider_id=provider_id,
        name=name,
        api_key=crypto_service.encrypt(access_token),
        auth_type="oauth",
        auth_config=crypto_service.encrypt(json.dumps(auth_config)),
        api_formats=api_formats,
        is_active=True,
        auto_fetch_models=auto_fetch_models,
    )
    if proxy:
        new_key.proxy = proxy
    db.add(new_key)
    if flush_only:
        db.flush()
    else:
        db.commit()
        db.refresh(new_key)
    return new_key


def _update_existing_oauth_key(
    db: Session,
    existing_key: "ProviderAPIKey",
    access_token: str,
    auth_config: dict[str, Any],
    flush_only: bool = False,
    proxy: dict[str, Any] | None = None,
) -> "ProviderAPIKey":
    """覆盖更新已失效的 OAuth Key，恢复为活跃状态。"""
    existing_key.api_key = crypto_service.encrypt(access_token)
    existing_key.auth_config = crypto_service.encrypt(json.dumps(auth_config))
    existing_key.is_active = True
    existing_key.oauth_invalid_at = None
    existing_key.oauth_invalid_reason = None
    existing_key.health_by_format = {}  # type: ignore[assignment]
    existing_key.circuit_breaker_by_format = {}  # type: ignore[assignment]
    existing_key.error_count = 0
    existing_key.last_error_at = None
    existing_key.last_error_msg = None
    if proxy:
        existing_key.proxy = proxy
    if flush_only:
        db.flush()
    else:
        db.commit()
        db.refresh(existing_key)
    return existing_key


async def _fetch_kiro_email(
    auth_config: dict[str, Any],
    proxy_config: dict[str, Any] | None = None,
) -> str | None:
    """通过 getUsageLimits API 获取 Kiro 用户邮箱。"""
    from src.services.provider.adapters.kiro.usage import (
        fetch_kiro_usage_limits,
        parse_kiro_usage_response,
    )

    try:
        result = await fetch_kiro_usage_limits(auth_config, proxy_config=proxy_config)
        usage_data = result.get("usage_data")
        if usage_data:
            parsed = parse_kiro_usage_response(usage_data)
            if parsed and parsed.get("email"):
                return parsed["email"]
    except Exception as e:
        logger.warning("[KIRO] 获取用户邮箱失败: {} | {}", type(e).__name__, e, exc_info=True)

    return None


def _build_kiro_key_name(
    email: str | None,
    auth_method: str | None,
    refresh_token: str | None,
) -> str:
    """根据 email / auth_method / refresh_token 生成 Kiro Key 名称。"""
    method = auth_method or "social"
    if not email:
        token_hash = hashlib.sha256((refresh_token or "").encode()).hexdigest()[:6]
        base = f"kiro_{token_hash}"
    else:
        base = email
    return f"{base} ({method})"


def _normalize_codex_plan_group(plan_type: Any) -> str | None:
    """将 Codex plan_type 归一化到判重分组。

    分组规则：
    - free
    - team/plus/enterprise（同组）
    """
    if not isinstance(plan_type, str):
        return None
    normalized = plan_type.strip().lower()
    if not normalized:
        return None
    if normalized == "free":
        return "free"
    if normalized in {"team", "plus", "enterprise"}:
        return "team_plus_enterprise"
    return None


def _normalize_codex_identity_value(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    normalized = value.strip()
    return normalized or None


def _is_codex_provider(provider_type: Any) -> bool:
    return str(provider_type or "").strip().lower() == ProviderType.CODEX.value


def _match_codex_identity(
    *,
    new_auth_config: dict[str, Any],
    existing_auth_config: dict[str, Any],
) -> bool | None:
    """Codex 判重优先按 account/team 维度进行。

    Returns:
        True: 明确重复
        False: 明确不是重复（例如同用户不同 account/team）
        None: 信息不足，调用方应继续使用兜底规则
    """
    new_provider_type = new_auth_config.get("provider_type")
    existing_provider_type = existing_auth_config.get("provider_type")
    if not (_is_codex_provider(new_provider_type) or _is_codex_provider(existing_provider_type)):
        return None

    new_account_user_id = _normalize_codex_identity_value(new_auth_config.get("account_user_id"))
    existing_account_user_id = _normalize_codex_identity_value(
        existing_auth_config.get("account_user_id")
    )
    if new_account_user_id and existing_account_user_id:
        return new_account_user_id == existing_account_user_id

    new_account_id = _normalize_codex_identity_value(new_auth_config.get("account_id"))
    existing_account_id = _normalize_codex_identity_value(existing_auth_config.get("account_id"))
    new_user_id = _normalize_codex_identity_value(new_auth_config.get("user_id"))
    existing_user_id = _normalize_codex_identity_value(existing_auth_config.get("user_id"))
    new_email = _normalize_codex_identity_value(new_auth_config.get("email"))
    existing_email = _normalize_codex_identity_value(existing_auth_config.get("email"))

    if new_account_id and existing_account_id and new_account_id != existing_account_id:
        return False

    if new_account_id and existing_account_id and new_user_id and existing_user_id:
        return new_account_id == existing_account_id and new_user_id == existing_user_id

    if new_account_id and existing_account_id and new_email and existing_email:
        return new_account_id == existing_account_id and new_email == existing_email

    return None


def _is_codex_cross_plan_group_non_duplicate(
    *,
    new_provider_type: Any,
    existing_provider_type: Any,
    new_plan_type: Any,
    existing_plan_type: Any,
) -> bool:
    """Codex 账号在 free 与 Team/Plus/Enterprise 之间不判重。"""
    new_pt = str(new_provider_type or "").strip().lower()
    existing_pt = str(existing_provider_type or "").strip().lower()
    if new_pt != ProviderType.CODEX.value and existing_pt != ProviderType.CODEX.value:
        return False

    new_group = _normalize_codex_plan_group(new_plan_type)
    existing_group = _normalize_codex_plan_group(existing_plan_type)
    return bool(new_group and existing_group and new_group != existing_group)


def _check_duplicate_oauth_account(
    db: Session,
    provider_id: str,
    auth_config: dict[str, Any],
    exclude_key_id: str | None = None,
) -> ProviderAPIKey | None:
    """
    检查是否存在重复的 OAuth 账号。

    通过以下字段判断重复：
    - Codex: 优先 account_user_id，其次 (user_id, account_id) / (email, account_id)
      同一用户切换不同 Team/account_id 时不判重；free 与 Team/Plus/Enterprise 互不判重
    - user_id: Codex 之外优先使用用户级别 ID
    - email + auth_method: Kiro 使用 email + auth_method 组合判断
      （同一邮箱可能通过 Social 和 IdC 两种方式登录，视为不同账号）
    - email: 其他 OAuth Provider 使用邮箱判断

    Returns:
        None: 无重复，可以新建
        ProviderAPIKey: 找到已失效的重复账号，调用方应覆盖此 key

    Raises:
        InvalidRequestException: 如果发现活跃的重复账号
    """
    new_email = auth_config.get("email")
    new_user_id = auth_config.get("user_id")
    new_auth_method = auth_config.get("auth_method")  # Kiro: social / idc
    new_provider_type = auth_config.get("provider_type")
    new_plan_type = auth_config.get("plan_type")

    # 如果没有可用于识别的字段，跳过检查
    if not new_email and not new_user_id:
        return None

    # 查询该 Provider 下所有 OAuth 类型的 Keys
    query = db.query(ProviderAPIKey).filter(
        ProviderAPIKey.provider_id == provider_id,
        ProviderAPIKey.auth_type.in_(OAUTH_AUTH_TYPES),
    )
    if exclude_key_id:
        query = query.filter(ProviderAPIKey.id != exclude_key_id)

    existing_keys = query.all()

    for existing_key in existing_keys:
        if not existing_key.auth_config:
            continue
        try:
            decrypted_config = json.loads(
                crypto_service.decrypt(existing_key.auth_config, silent=True)
            )
            existing_email = decrypted_config.get("email")
            existing_user_id = decrypted_config.get("user_id")
            existing_auth_method = decrypted_config.get("auth_method")
            existing_provider_type = decrypted_config.get("provider_type")
            existing_plan_type = decrypted_config.get("plan_type")

            is_duplicate = False

            codex_identity_match = _match_codex_identity(
                new_auth_config=auth_config,
                existing_auth_config=decrypted_config,
            )
            if codex_identity_match is True:
                is_duplicate = True
            elif codex_identity_match is False:
                is_duplicate = False

            # user_id 相同即重复（Codex 等，同一 team 下不同成员共享 account_id 但 user_id 不同）
            if (
                codex_identity_match is None
                and new_user_id
                and existing_user_id
                and new_user_id == existing_user_id
            ):
                if not _is_codex_cross_plan_group_non_duplicate(
                    new_provider_type=new_provider_type,
                    existing_provider_type=existing_provider_type,
                    new_plan_type=new_plan_type,
                    existing_plan_type=existing_plan_type,
                ):
                    is_duplicate = True

            # email 判断
            if (
                codex_identity_match is None
                and not is_duplicate
                and new_email
                and existing_email
                and new_email == existing_email
            ):
                is_kiro = new_provider_type == "kiro" or existing_provider_type == "kiro"
                if is_kiro:
                    # Kiro: 只有 email + auth_method 都相同才视为重复
                    if (
                        new_auth_method
                        and existing_auth_method
                        and new_auth_method.lower() == existing_auth_method.lower()
                    ):
                        is_duplicate = True
                else:
                    if not _is_codex_cross_plan_group_non_duplicate(
                        new_provider_type=new_provider_type,
                        existing_provider_type=existing_provider_type,
                        new_plan_type=new_plan_type,
                        existing_plan_type=existing_plan_type,
                    ):
                        is_duplicate = True

            if is_duplicate:
                # 失效账号允许覆盖
                if not existing_key.is_active:
                    logger.info(
                        "重复 OAuth 账号已失效，将覆盖更新（key_id={}, name={}）",
                        existing_key.id,
                        existing_key.name,
                    )
                    return existing_key

                # 活跃的重复账号，拒绝添加
                identifier = (
                    auth_config.get("account_user_id")
                    or auth_config.get("account_id")
                    or new_email
                    or new_user_id
                    or ""
                )
                raise InvalidRequestException(
                    f"该 OAuth 账号 ({identifier}) 已存在于当前 Provider 中"
                    f"（名称: {existing_key.name}）"
                )
        except InvalidRequestException:
            raise
        except Exception:
            # 解密失败时跳过该 Key
            continue

    return None


# ==============================================================================
# Routes
# ==============================================================================


@router.get("/supported-types")
async def supported_types(_: User = Depends(require_admin)) -> list[dict[str, Any]]:
    # 不返回 client_secret
    result: list[dict[str, Any]] = []
    for provider_type, template in FIXED_PROVIDERS.items():
        if not _supports_oauth(template):
            continue
        result.append(
            {
                "provider_type": (
                    str(provider_type.value)
                    if hasattr(provider_type, "value")
                    else str(provider_type)
                ),
                "display_name": template.display_name,
                "scopes": list(template.oauth.scopes),
                "redirect_uri": template.oauth.redirect_uri,
                "authorize_url": template.oauth.authorize_url,
                "token_url": template.oauth.token_url,
                "use_pkce": bool(template.oauth.use_pkce),
            }
        )
    return result


@router.post("/keys/{key_id}/start", response_model=StartOAuthResponse)
async def start_oauth(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> StartOAuthResponse:
    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
    if not key:
        raise NotFoundException("Key 不存在", "key")
    if (getattr(key, "auth_type", "api_key") or "api_key") != "oauth":
        raise InvalidRequestException("该 Key 不是 oauth 认证类型")

    provider = db.query(Provider).filter(Provider.id == key.provider_id).first()
    if not provider:
        raise NotFoundException("Provider 不存在", "provider")
    provider_type = _require_fixed_provider(provider)

    template = _require_oauth_template(provider_type)

    redis = await get_redis_client(require_redis=True)
    assert redis is not None

    pkce_verifier: str | None = None
    code_challenge: str | None = None

    if template.oauth.use_pkce:
        pkce_verifier = secrets.token_urlsafe(32)
        code_challenge = _pkce_s256(pkce_verifier)

    state = await _create_state(
        redis,
        key_id=key_id,
        provider_id=str(provider.id),
        provider_type=provider_type,
        pkce_verifier=pkce_verifier,
    )

    params: dict[str, Any] = {
        "client_id": template.oauth.client_id,
        "response_type": "code",
        "redirect_uri": template.oauth.redirect_uri,
        "scope": " ".join(template.oauth.scopes),
        "state": state,
    }

    # Codex 参考实现额外参数
    if provider_type == ProviderType.CODEX.value:
        params.update(
            {
                "prompt": "login",
                "id_token_add_organizations": "true",
                "codex_cli_simplified_flow": "true",
            }
        )

    if template.oauth.use_pkce and code_challenge:
        params["code_challenge"] = code_challenge
        params["code_challenge_method"] = "S256"

    authorization_url = f"{template.oauth.authorize_url}?{urlencode(params)}"

    return StartOAuthResponse(
        authorization_url=authorization_url,
        redirect_uri=template.oauth.redirect_uri,
        provider_type=provider_type,
        instructions=(
            "1) 打开 authorization_url 完成授权\n"
            "2) 授权后会跳转到 redirect_uri（localhost）\n"
            "3) 复制浏览器地址栏完整 URL，调用 complete 接口粘贴 callback_url"
        ),
    )


@router.post("/keys/{key_id}/complete", response_model=CompleteOAuthResponse)
async def complete_oauth(
    key_id: str,
    payload: CompleteOAuthRequest,
    request: Request,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> CompleteOAuthResponse:
    redis = await get_redis_client(require_redis=True)
    assert redis is not None

    params = _parse_callback_params(payload.callback_url)
    code = params.get("code")
    state = params.get("state")
    if not code or not state:
        raise InvalidRequestException("callback_url 缺少 code/state")

    state_data = await _consume_state(redis, state)
    if not state_data or state_data.key_id != key_id:
        raise InvalidRequestException("state 无效或已过期")

    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
    if not key:
        raise NotFoundException("Key 不存在", "key")
    if (getattr(key, "auth_type", "api_key") or "api_key") != "oauth":
        raise InvalidRequestException("该 Key 不是 oauth 认证类型")

    provider = db.query(Provider).filter(Provider.id == key.provider_id).first()
    if not provider:
        raise NotFoundException("Provider 不存在", "provider")
    provider_type = _require_fixed_provider(provider)

    template = _require_oauth_template(provider_type)

    # exchange token
    token_url = template.oauth.token_url

    # Claude token endpoint 是 JSON；Codex/Google 是 form。这里先做最小实现：按 URL 判断。
    is_json = "anthropic.com" in token_url

    if is_json:
        body: dict[str, Any] = {
            "grant_type": "authorization_code",
            "client_id": template.oauth.client_id,
            "redirect_uri": template.oauth.redirect_uri,
            "code": code,
            "state": state,
        }
        if state_data.pkce_verifier:
            body["code_verifier"] = state_data.pkce_verifier
        headers = {"Content-Type": "application/json", "Accept": "application/json"}
        data = None
        json_body = body
    else:
        form: dict[str, str] = {
            "grant_type": "authorization_code",
            "client_id": template.oauth.client_id,
            "redirect_uri": template.oauth.redirect_uri,
            "code": code,
        }
        if template.oauth.client_secret:
            form["client_secret"] = template.oauth.client_secret
        if state_data.pkce_verifier:
            form["code_verifier"] = state_data.pkce_verifier
        headers = {
            "Content-Type": "application/x-www-form-urlencoded",
            "Accept": "application/json",
        }
        data = form
        json_body = None

    proxy_config = getattr(provider, "proxy", None)

    resp = await post_oauth_token(
        provider_type=provider_type,
        token_url=token_url,
        headers=headers,
        data=data,
        json_body=json_body,
        proxy_config=proxy_config,
        timeout_seconds=30.0,
    )

    if resp.status_code < 200 or resp.status_code >= 300:
        raise InvalidRequestException("token exchange 失败")

    token = resp.json()
    access_token = str(token.get("access_token") or "")
    refresh_token = str(token.get("refresh_token") or "")
    expires_in = token.get("expires_in")
    expires_at: int | None = None
    try:
        if expires_in is not None:
            expires_at = int(time.time()) + int(expires_in)
    except Exception:
        expires_at = None

    if not access_token:
        raise InvalidRequestException("token exchange 返回缺少 access_token")

    auth_config: dict[str, Any] = {
        "provider_type": provider_type,
        "token_type": token.get("token_type"),
        "refresh_token": refresh_token or None,
        "expires_at": expires_at,
        "scope": token.get("scope"),
        "updated_at": int(time.time()),
    }

    auth_config = await enrich_auth_config(
        provider_type=provider_type,
        auth_config=auth_config,
        token_response=token,
        access_token=access_token,
        proxy_config=proxy_config,
    )

    await run_in_threadpool(
        _store_completed_oauth_sync,
        key_id,
        provider_type,
        access_token,
        auth_config,
    )

    return CompleteOAuthResponse(
        provider_type=provider_type,
        expires_at=expires_at,
        has_refresh_token=bool(refresh_token),
        email=auth_config.get("email"),
    )


@router.post("/keys/{key_id}/refresh", response_model=CompleteOAuthResponse)
async def refresh_oauth(
    key_id: str,
    request: Request,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> CompleteOAuthResponse:
    from src.services.provider.auth import _acquire_refresh_lock, _release_refresh_lock

    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
    if not key:
        raise NotFoundException("Key 不存在", "key")
    if (getattr(key, "auth_type", "api_key") or "api_key") != "oauth":
        raise InvalidRequestException("该 Key 不是 oauth 认证类型")

    provider = db.query(Provider).filter(Provider.id == key.provider_id).first()
    if not provider:
        raise NotFoundException("Provider 不存在", "provider")
    provider_type = _require_fixed_provider(provider)

    redis, got_lock = await _acquire_refresh_lock(key_id)
    if redis is not None and not got_lock:
        raise InvalidRequestException("该 Key 正在续期，请稍后重试")

    try:
        # Kiro 使用自定义 token refresh 机制
        if provider_type == ProviderType.KIRO.value:
            from datetime import datetime, timezone

            from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
            from src.services.provider.adapters.kiro.token_manager import refresh_access_token

            encrypted_auth_config = getattr(key, "auth_config", None)
            if not encrypted_auth_config:
                raise InvalidRequestException("缺少 auth_config，无法 refresh")

            decrypted = crypto_service.decrypt(encrypted_auth_config)
            parsed = json.loads(decrypted)

            cfg = KiroAuthConfig.from_dict(parsed)
            cfg.provider_type = ProviderType.KIRO.value

            from src.services.proxy_node.resolver import resolve_effective_proxy

            proxy_config = resolve_effective_proxy(
                getattr(provider, "proxy", None), getattr(key, "proxy", None)
            )
            try:
                access_token, new_cfg = await refresh_access_token(cfg, proxy_config=proxy_config)
            except Exception as e:
                await run_in_threadpool(
                    _mark_refresh_failed_sync,
                    key_id,
                    f"[REFRESH_FAILED] Token 续期失败: {e}",
                )
                logger.warning("Kiro Key {} token 刷新失败，已标记为刷新失效: {}", key_id, e)
                raise InvalidRequestException("Kiro token refresh 失败，请检查凭据是否有效")

            await run_in_threadpool(
                _store_refreshed_oauth_sync,
                key_id,
                access_token,
                new_cfg.to_dict(),
            )

            return CompleteOAuthResponse(
                provider_type=provider_type,
                expires_at=new_cfg.expires_at or None,
                has_refresh_token=bool(new_cfg.refresh_token),
                email=None,
            )

        template = _require_oauth_template(provider_type)

        encrypted_auth_config = getattr(key, "auth_config", None)
        if not encrypted_auth_config:
            raise InvalidRequestException("缺少 auth_config，无法 refresh")

        decrypted = crypto_service.decrypt(encrypted_auth_config)
        parsed = json.loads(decrypted)
        refresh_token = str(parsed.get("refresh_token") or "")
        if not refresh_token:
            raise InvalidRequestException("缺少 refresh_token，需要重新授权")

        token_url = template.oauth.token_url
        is_json = "anthropic.com" in token_url
        scope_str = " ".join(template.oauth.scopes) if template.oauth.scopes else ""

        if is_json:
            body: dict[str, Any] = {
                "grant_type": "refresh_token",
                "client_id": template.oauth.client_id,
                "refresh_token": refresh_token,
            }
            if scope_str:
                body["scope"] = scope_str
            headers = {"Content-Type": "application/json", "Accept": "application/json"}
            data = None
            json_body = body
        else:
            form: dict[str, str] = {
                "grant_type": "refresh_token",
                "client_id": template.oauth.client_id,
                "refresh_token": refresh_token,
            }
            if scope_str:
                form["scope"] = scope_str
            if template.oauth.client_secret:
                form["client_secret"] = template.oauth.client_secret
            headers = {
                "Content-Type": "application/x-www-form-urlencoded",
                "Accept": "application/json",
            }
            data = form
            json_body = None

        from src.services.proxy_node.resolver import resolve_effective_proxy

        proxy_config = resolve_effective_proxy(
            getattr(provider, "proxy", None), getattr(key, "proxy", None)
        )

        resp = await post_oauth_token(
            provider_type=provider_type,
            token_url=token_url,
            headers=headers,
            data=data,
            json_body=json_body,
            proxy_config=proxy_config,
            timeout_seconds=30.0,
        )

        if resp.status_code < 200 or resp.status_code >= 300:
            error_reason = f"HTTP {resp.status_code}"
            try:
                error_body = resp.json()
                if "error" in error_body:
                    error_reason = str(
                        error_body.get("error_description") or error_body.get("error")
                    )
            except Exception:
                error_reason = resp.text[:100] if resp.text else f"HTTP {resp.status_code}"

            if resp.status_code in (400, 401, 403):
                await run_in_threadpool(
                    _mark_refresh_failed_sync,
                    key_id,
                    f"[REFRESH_FAILED] Token 续期失败 ({resp.status_code}): {error_reason}",
                )
                logger.warning(
                    "Key {} OAuth token 刷新失败，已标记为刷新失效: {}", key_id, error_reason
                )

            raise InvalidRequestException(f"token refresh 失败: {error_reason}")

        token = resp.json()
        access_token = str(token.get("access_token") or "")
        new_refresh_token = str(token.get("refresh_token") or "")
        expires_in = token.get("expires_in")
        expires_at: int | None = None
        try:
            if expires_in is not None:
                expires_at = int(time.time()) + int(expires_in)
        except Exception:
            expires_at = None

        if not access_token:
            raise InvalidRequestException("token refresh 返回缺少 access_token")

        parsed["token_type"] = token.get("token_type")
        if new_refresh_token:
            parsed["refresh_token"] = new_refresh_token
        parsed["expires_at"] = expires_at
        parsed["scope"] = token.get("scope")
        parsed["updated_at"] = int(time.time())

        parsed = await enrich_auth_config(
            provider_type=provider_type,
            auth_config=parsed,
            token_response=token,
            access_token=access_token,
            proxy_config=proxy_config,
        )

        if provider_type == ProviderType.ANTIGRAVITY and not parsed.get("project_id"):
            logger.warning(
                "[OAUTH_REFRESH] Antigravity key {} 刷新成功但 project_id 仍缺失，"
                "下次刷新将继续尝试获取",
                key_id,
            )

        await run_in_threadpool(_store_refreshed_oauth_sync, key_id, access_token, parsed)

        return CompleteOAuthResponse(
            provider_type=provider_type,
            expires_at=expires_at,
            has_refresh_token=bool(parsed.get("refresh_token")),
            email=parsed.get("email"),
        )
    finally:
        if got_lock:
            await _release_refresh_lock(redis, key_id)


# ==============================================================================
# Provider-level OAuth (不需要预先创建 key)
# ==============================================================================


@router.post("/providers/{provider_id}/start", response_model=StartOAuthResponse)
async def start_provider_oauth(
    provider_id: str,
    request: Request,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> StartOAuthResponse:
    """基于 Provider 启动 OAuth（不需要预先创建 key）。"""
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException("Provider 不存在", "provider")
    provider_type = _require_fixed_provider(provider)

    if provider_type == ProviderType.KIRO.value:
        raise InvalidRequestException("Kiro 不支持 OAuth 授权，请使用导入授权。")

    template = _require_oauth_template(provider_type)

    redis = await get_redis_client(require_redis=True)
    assert redis is not None

    pkce_verifier: str | None = None
    code_challenge: str | None = None

    if template.oauth.use_pkce:
        pkce_verifier = secrets.token_urlsafe(32)
        code_challenge = _pkce_s256(pkce_verifier)

    state = await _create_state(
        redis,
        key_id="",  # 空，complete 时创建
        provider_id=provider_id,
        provider_type=provider_type,
        pkce_verifier=pkce_verifier,
    )

    params: dict[str, Any] = {
        "client_id": template.oauth.client_id,
        "response_type": "code",
        "redirect_uri": template.oauth.redirect_uri,
        "scope": " ".join(template.oauth.scopes),
        "state": state,
    }

    if provider_type == ProviderType.CODEX.value:
        params.update(
            {
                "prompt": "login",
                "id_token_add_organizations": "true",
                "codex_cli_simplified_flow": "true",
            }
        )

    if template.oauth.use_pkce and code_challenge:
        params["code_challenge"] = code_challenge
        params["code_challenge_method"] = "S256"

    authorization_url = f"{template.oauth.authorize_url}?{urlencode(params)}"

    return StartOAuthResponse(
        authorization_url=authorization_url,
        redirect_uri=template.oauth.redirect_uri,
        provider_type=provider_type,
        instructions=(
            "1) 打开 authorization_url 完成授权\n"
            "2) 授权后会跳转到 redirect_uri（localhost）\n"
            "3) 复制浏览器地址栏完整 URL，调用 complete 接口粘贴 callback_url"
        ),
    )


@router.post("/providers/{provider_id}/complete", response_model=ProviderCompleteOAuthResponse)
async def complete_provider_oauth(
    provider_id: str,
    payload: ProviderCompleteOAuthRequest,
    request: Request,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> ProviderCompleteOAuthResponse:
    """完成 Provider OAuth 并创建 key。"""
    redis = await get_redis_client(require_redis=True)
    assert redis is not None

    params = _parse_callback_params(payload.callback_url)
    code = params.get("code")
    state = params.get("state")
    if not code or not state:
        raise InvalidRequestException("callback_url 缺少 code/state")

    state_data = await _consume_state(redis, state)
    if not state_data or state_data.provider_id != provider_id:
        raise InvalidRequestException("state 无效或已过期")

    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException("Provider 不存在", "provider")
    provider_type = _require_fixed_provider(provider)

    if provider_type == ProviderType.KIRO.value:
        raise InvalidRequestException("Kiro 不支持 OAuth 授权，请使用导入授权。")

    template = _require_oauth_template(provider_type)

    # exchange token
    token_url = template.oauth.token_url
    is_json = "anthropic.com" in token_url

    if is_json:
        body: dict[str, Any] = {
            "grant_type": "authorization_code",
            "client_id": template.oauth.client_id,
            "redirect_uri": template.oauth.redirect_uri,
            "code": code,
            "state": state,
        }
        if state_data.pkce_verifier:
            body["code_verifier"] = state_data.pkce_verifier
        headers = {"Content-Type": "application/json", "Accept": "application/json"}
        data = None
        json_body = body
    else:
        form: dict[str, str] = {
            "grant_type": "authorization_code",
            "client_id": template.oauth.client_id,
            "redirect_uri": template.oauth.redirect_uri,
            "code": code,
        }
        if template.oauth.client_secret:
            form["client_secret"] = template.oauth.client_secret
        if state_data.pkce_verifier:
            form["code_verifier"] = state_data.pkce_verifier
        headers = {
            "Content-Type": "application/x-www-form-urlencoded",
            "Accept": "application/json",
        }
        data = form
        json_body = None

    # 解析代理：前端指定 proxy_node_id 时优先使用，否则回退到 Provider 级代理
    proxy_config, key_proxy = _resolve_proxy_for_oauth(
        getattr(provider, "proxy", None), payload.proxy_node_id
    )

    resp = await post_oauth_token(
        provider_type=provider_type,
        token_url=token_url,
        headers=headers,
        data=data,
        json_body=json_body,
        proxy_config=proxy_config,
        timeout_seconds=30.0,
    )

    if resp.status_code < 200 or resp.status_code >= 300:
        raise InvalidRequestException("token exchange 失败")

    token = resp.json()
    access_token = str(token.get("access_token") or "")
    refresh_token = str(token.get("refresh_token") or "")
    expires_in = token.get("expires_in")
    expires_at: int | None = None
    try:
        if expires_in is not None:
            expires_at = int(time.time()) + int(expires_in)
    except Exception:
        expires_at = None

    if not access_token:
        raise InvalidRequestException("token exchange 返回缺少 access_token")

    # 构建 auth_config
    auth_config: dict[str, Any] = {
        "provider_type": provider_type,
        "token_type": token.get("token_type"),
        "refresh_token": refresh_token or None,
        "expires_at": expires_at,
        "scope": token.get("scope"),
        "updated_at": int(time.time()),
    }

    auth_config = await enrich_auth_config(
        provider_type=provider_type,
        auth_config=auth_config,
        token_response=token,
        access_token=access_token,
        proxy_config=proxy_config,
    )

    # 检查是否存在重复的 OAuth 账号（失效账号允许覆盖）
    existing_key = _check_duplicate_oauth_account(db, provider_id, auth_config)
    replaced = False

    if existing_key:
        new_key = _update_existing_oauth_key(
            db, existing_key, access_token, auth_config, proxy=key_proxy
        )
        replaced = True
    else:
        # 确定账号名称
        name = (payload.name or "").strip()
        if not name:
            name = auth_config.get("email") or f"账号_{int(time.time())}"

        new_key = _create_oauth_key(
            db,
            provider_id=provider_id,
            name=name,
            access_token=access_token,
            auth_config=auth_config,
            api_formats=_get_provider_api_formats(provider),
            proxy=key_proxy,
        )

    response = ProviderCompleteOAuthResponse(
        key_id=str(new_key.id),
        provider_type=provider_type,
        expires_at=expires_at,
        has_refresh_token=bool(refresh_token),
        email=auth_config.get("email"),
        replaced=replaced,
    )

    # 单个导入完成后，后台触发一次配额刷新
    safe_create_task(_refresh_quota_after_import(provider_id, provider_type, [str(new_key.id)]))

    return response


# ==============================================================================
# Import Refresh Token (从导出文件导入)
# ==============================================================================


def _coerce_import_str(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    normalized = value.strip()
    return normalized or None


def _extract_standard_oauth_import_entry(item: Any) -> dict[str, Any] | None:
    if isinstance(item, str):
        token = _coerce_import_str(item)
        if token:
            return {"refresh_token": token}
        return None
    if not isinstance(item, dict):
        return None

    refresh_token = _coerce_import_str(item.get("refresh_token")) or _coerce_import_str(
        item.get("refreshToken")
    )
    if not refresh_token:
        return None

    entry: dict[str, Any] = {"refresh_token": refresh_token}

    account_id = (
        _coerce_import_str(item.get("account_id"))
        or _coerce_import_str(item.get("accountId"))
        or _coerce_import_str(item.get("chatgpt_account_id"))
        or _coerce_import_str(item.get("chatgptAccountId"))
    )
    if account_id:
        entry["account_id"] = account_id

    account_user_id = (
        _coerce_import_str(item.get("account_user_id"))
        or _coerce_import_str(item.get("accountUserId"))
        or _coerce_import_str(item.get("chatgpt_account_user_id"))
        or _coerce_import_str(item.get("chatgptAccountUserId"))
    )
    if account_user_id:
        entry["account_user_id"] = account_user_id

    plan_type = (
        _coerce_import_str(item.get("plan_type"))
        or _coerce_import_str(item.get("planType"))
        or _coerce_import_str(item.get("chatgpt_plan_type"))
        or _coerce_import_str(item.get("chatgptPlanType"))
    )
    if plan_type:
        entry["plan_type"] = plan_type.lower()

    user_id = (
        _coerce_import_str(item.get("user_id"))
        or _coerce_import_str(item.get("userId"))
        or _coerce_import_str(item.get("chatgpt_user_id"))
        or _coerce_import_str(item.get("chatgptUserId"))
    )
    if user_id:
        entry["user_id"] = user_id

    email = _coerce_import_str(item.get("email"))
    if email:
        entry["email"] = email

    return entry


def _parse_standard_oauth_import_entries(raw_input: str) -> list[dict[str, Any]]:
    """
    解析标准 OAuth 导入输入，保留 refresh_token 及可用账号提示字段。

    支持的格式：
    1. 单个 Token 字符串
    2. JSON 字符串数组: ["token1", "token2", ...]
    3. JSON 对象数组: [{"refresh_token": "token1", "account_id": "...", ...}, ...]
    4. 单个 JSON 对象: {"refresh_token": "token1", ...}
    5. 纯 Token 导入（一行一个）: "token1\\ntoken2\\ntoken3"
    """
    raw = raw_input.strip()
    if not raw:
        return []

    result: list[dict[str, Any]] = []

    if raw.startswith("["):
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            parsed = None
        if isinstance(parsed, list):
            for item in parsed:
                entry = _extract_standard_oauth_import_entry(item)
                if entry:
                    result.append(entry)
            return result

    if raw.startswith("{"):
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            parsed = None
        if isinstance(parsed, dict):
            entry = _extract_standard_oauth_import_entry(parsed)
            return [entry] if entry else []

    for line in raw.splitlines():
        token = line.strip()
        if token and not token.startswith("#"):
            result.append({"refresh_token": token})

    return result


def _parse_tokens_input(raw_input: str) -> list[str]:
    """兼容旧逻辑：仅返回 refresh_token 列表。"""
    return [entry["refresh_token"] for entry in _parse_standard_oauth_import_entries(raw_input)]


def _parse_kiro_import_input(raw_input: str) -> list[dict[str, Any]]:
    """
    解析 Kiro 凭据导入输入。

    支持的格式：
    1. 扁平 JSON 对象: {"refresh_token": "...", "auth_method": "social", ...}
    2. JSON 数组（批量）: [{...}, {...}]
    3. 纯 Token（一行一个）: "token1\\ntoken2"

    返回: 凭据字典列表
    """

    def _normalize_item(item: Any) -> dict[str, Any] | None:
        """规范化单条 Kiro 导入项，兼容导出结构。"""
        if isinstance(item, str) and item.strip():
            return {"refreshToken": item.strip()}
        if not isinstance(item, dict):
            return None

        nested = item.get("auth_config") or item.get("authConfig")
        if isinstance(nested, dict):
            # 优先使用 auth_config，兼容导出对象形态：
            # {"name": "...", "auth_config": {...}, ...}
            merged = dict(nested)
            # 若顶层也包含关键字段，允许覆盖 nested（便于手工修正）
            for key in (
                "provider_type",
                "providerType",
                "auth_method",
                "authMethod",
                "auth_type",
                "authType",
                "refresh_token",
                "refreshToken",
                "expires_at",
                "expiresAt",
                "profile_arn",
                "profileArn",
                "region",
                "auth_region",
                "authRegion",
                "api_region",
                "apiRegion",
                "client_id",
                "clientId",
                "client_secret",
                "clientSecret",
                "machine_id",
                "machineId",
                "kiro_version",
                "kiroVersion",
                "system_version",
                "systemVersion",
                "node_version",
                "nodeVersion",
                "email",
                "access_token",
                "accessToken",
            ):
                value = item.get(key)
                if value is not None and value != "":
                    merged[key] = value
            return merged

        return item

    raw = raw_input.strip()
    if not raw:
        return []

    # 尝试解析为 JSON
    if raw.startswith("{") or raw.startswith("["):
        try:
            parsed = json.loads(raw)

            if isinstance(parsed, list):
                result: list[dict[str, Any]] = []
                for item in parsed:
                    normalized = _normalize_item(item)
                    if normalized:
                        result.append(normalized)
                return result

            if isinstance(parsed, dict):
                normalized = _normalize_item(parsed)
                if normalized:
                    return [normalized]

        except json.JSONDecodeError:
            pass

    # 纯 Token（一行一个）
    return [
        {"refreshToken": line.strip()}
        for line in raw.splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]


class ImportRefreshTokenRequest(BaseModel):
    refresh_token: str = Field(..., min_length=1, description="Refresh Token")
    name: str | None = Field(None, max_length=100, description="账号名称（可选）")
    proxy_node_id: str | None = Field(
        None,
        description="代理节点 ID（可选）。设置后导入验证及后续所有操作均走该代理",
    )


class BatchImportRequest(BaseModel):
    """批量导入 Kiro 凭据请求"""

    credentials: str = Field(
        ...,
        min_length=1,
        description="凭据数据，支持多种格式：JSON 对象、JSON 数组、纯 Token（一行一个）",
    )
    proxy_node_id: str | None = Field(
        None,
        description="代理节点 ID（可选）。设置后批量导入验证及后续所有操作均走该代理",
    )


class BatchImportResultItem(BaseModel):
    """单个凭据导入结果"""

    index: int = Field(..., description="凭据在输入中的索引（从 0 开始）")
    status: str = Field(..., description="状态：success / error")
    key_id: str | None = Field(None, description="创建的 Key ID（成功时）")
    key_name: str | None = Field(None, description="创建的 Key 名称（成功时）")
    auth_method: str | None = Field(None, description="认证类型（成功时）")
    error: str | None = Field(None, description="错误信息（失败时）")
    replaced: bool = Field(False, description="是否覆盖了已失效的重复账号")


class BatchImportResponse(BaseModel):
    """批量导入响应"""

    total: int = Field(..., description="总凭据数")
    success: int = Field(..., description="成功导入数")
    failed: int = Field(..., description="失败数")
    results: list[BatchImportResultItem] = Field(..., description="每个凭据的导入结果")


BatchImportTaskStatus = Literal["submitted", "processing", "completed", "failed"]


class BatchImportTaskStartResponse(BaseModel):
    """异步批量导入任务创建响应"""

    task_id: str = Field(..., description="任务 ID")
    status: BatchImportTaskStatus = Field(..., description="任务状态")
    total: int = Field(..., description="待处理总数")
    processed: int = Field(0, description="已处理数")
    success: int = Field(0, description="成功数")
    failed: int = Field(0, description="失败数")
    progress_percent: int = Field(0, description="进度百分比（0-100）")
    message: str | None = Field(None, description="状态描述")


class BatchImportTaskStatusResponse(BaseModel):
    """异步批量导入任务状态"""

    task_id: str = Field(..., description="任务 ID")
    provider_id: str = Field(..., description="Provider ID")
    provider_type: str = Field(..., description="Provider 类型")
    status: BatchImportTaskStatus = Field(..., description="任务状态")
    total: int = Field(..., description="待处理总数")
    processed: int = Field(0, description="已处理数")
    success: int = Field(0, description="成功数")
    failed: int = Field(0, description="失败数")
    progress_percent: int = Field(0, description="进度百分比（0-100）")
    message: str | None = Field(None, description="状态描述")
    error: str | None = Field(None, description="任务级错误信息")
    error_samples: list[BatchImportResultItem] = Field(
        default_factory=list,
        description="错误样例（最多保留部分）",
    )
    created_at: int = Field(..., description="创建时间戳（秒）")
    started_at: int | None = Field(None, description="开始时间戳（秒）")
    finished_at: int | None = Field(None, description="结束时间戳（秒）")
    updated_at: int = Field(..., description="更新时间戳（秒）")


BatchImportProgressHook = Callable[
    [int, int, int, int, BatchImportResultItem],
    Awaitable[None],
]


def _task_state_to_response(state: dict[str, Any]) -> BatchImportTaskStatusResponse:
    raw_status = str(state.get("status") or "failed")
    status: BatchImportTaskStatus = (
        raw_status if raw_status in _PROVIDER_OAUTH_BATCH_TASK_ALLOWED_STATUSES else "failed"
    )
    error_samples: list[BatchImportResultItem] = []
    for item in state.get("error_samples") or []:
        try:
            error_samples.append(BatchImportResultItem.model_validate(item))
        except Exception:
            continue

    return BatchImportTaskStatusResponse(
        task_id=str(state.get("task_id") or ""),
        provider_id=str(state.get("provider_id") or ""),
        provider_type=str(state.get("provider_type") or ""),
        status=status,
        total=int(state.get("total") or 0),
        processed=int(state.get("processed") or 0),
        success=int(state.get("success") or 0),
        failed=int(state.get("failed") or 0),
        progress_percent=max(0, min(100, int(state.get("progress_percent") or 0))),
        message=(str(state["message"]) if state.get("message") is not None else None),
        error=(str(state["error"]) if state.get("error") is not None else None),
        error_samples=error_samples,
        created_at=int(state.get("created_at") or int(time.time())),
        started_at=(int(state["started_at"]) if state.get("started_at") is not None else None),
        finished_at=(int(state["finished_at"]) if state.get("finished_at") is not None else None),
        updated_at=int(state.get("updated_at") or int(time.time())),
    )


def _estimate_batch_import_total(provider_type: str, raw_credentials: str) -> int:
    if provider_type == ProviderType.KIRO.value:
        return len(_parse_kiro_import_input(raw_credentials))
    return len(_parse_standard_oauth_import_entries(raw_credentials))


def _release_batch_import_db_connection_before_await(db: Session) -> None:
    """Best-effort 释放批量导入任务的只读 DB 连接。

    批量导入会在单个后台任务里执行大量 await（上游 token 校验、邮箱探测、Redis 进度更新）。
    如果前面做过 Provider 查询而 Session 一直保持事务打开，连接会长时间占着不放，
    在大批量导入且多数条目最终失败时尤其容易把连接池拖满。

    这里复用调度器已有的 helper：仅在 Session 没有挂起写入时才提前结束事务，
    避免影响 flush 后尚未提交的数据。
    """
    release_db_connection_before_await(db)


def _commit_batch_import_writes_if_needed(db: Session, pending_writes: int) -> int:
    """按固定批次提交导入写入，避免长事务持续占用连接。"""
    if pending_writes < _PROVIDER_OAUTH_BATCH_IMPORT_COMMIT_BATCH_SIZE:
        return pending_writes
    db.commit()
    return 0


def _apply_codex_import_hints(auth_config: dict[str, Any], import_entry: dict[str, Any]) -> None:
    """将导入文件中可用的 Codex 账号信息作为兜底补全（不覆盖已有值）。"""
    for field in ("account_user_id", "account_id", "plan_type", "user_id", "email"):
        value = import_entry.get(field)
        if value and not auth_config.get(field):
            auth_config[field] = value


@router.post(
    "/providers/{provider_id}/import-refresh-token",
    response_model=ProviderCompleteOAuthResponse,
)
async def import_refresh_token(
    provider_id: str,
    payload: ImportRefreshTokenRequest,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> ProviderCompleteOAuthResponse:
    """通过 Refresh Token 导入 OAuth 账号。

    使用导出的 Refresh Token 换取 Access Token 并创建新的 OAuth Key。
    """
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException("Provider 不存在", "provider")
    provider_type = _require_fixed_provider(provider)

    # 解析代理：前端指定 proxy_node_id 时优先使用，否则回退到 Provider 级代理
    proxy_config, key_proxy = _resolve_proxy_for_oauth(
        getattr(provider, "proxy", None), payload.proxy_node_id
    )

    if provider_type == ProviderType.KIRO.value:
        raw_import = payload.refresh_token.strip()
        if not raw_import:
            raise InvalidRequestException("Refresh Token 不能为空")

        # 使用统一的解析函数
        credentials = _parse_kiro_import_input(raw_import)
        if not credentials:
            raise InvalidRequestException("无法解析凭据数据")

        # 单条导入只取第一个
        raw_cfg = credentials[0]

        from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
        from src.services.provider.adapters.kiro.token_manager import refresh_access_token

        # 验证必需字段
        is_valid, error_msg = KiroAuthConfig.validate_required_fields(raw_cfg)
        if not is_valid:
            raise InvalidRequestException(error_msg)

        # 解析配置（自动推断 auth_method）
        cfg = KiroAuthConfig.from_dict(raw_cfg)
        cfg.provider_type = ProviderType.KIRO.value

        try:
            access_token, new_cfg = await refresh_access_token(cfg, proxy_config=proxy_config)
        except Exception as e:
            logger.warning("Kiro Refresh Token 验证失败: {} | {}", type(e).__name__, e)
            raise InvalidRequestException(f"Kiro Refresh Token 验证失败: {type(e).__name__}")

        # 先获取 email，确保重复检查时有 email 可用
        email = await _fetch_kiro_email(new_cfg.to_dict(), proxy_config=proxy_config)
        if email and not new_cfg.email:
            new_cfg.email = email

        # 检查是否存在重复的 Kiro 账号（失效账号允许覆盖）
        existing_key = _check_duplicate_oauth_account(db, provider_id, new_cfg.to_dict())
        replaced = False

        # Kiro 确定账号名称：email + auth_method 区分不同来源
        name = (payload.name or "").strip()
        if not name:
            name = _build_kiro_key_name(email, new_cfg.auth_method, new_cfg.refresh_token)

        if existing_key:
            new_key = _update_existing_oauth_key(
                db, existing_key, access_token, new_cfg.to_dict(), proxy=key_proxy
            )
            replaced = True
        else:
            new_key = _create_oauth_key(
                db,
                provider_id=provider_id,
                name=name,
                access_token=access_token,
                auth_config=new_cfg.to_dict(),
                api_formats=_get_provider_api_formats(provider),
                proxy=key_proxy,
            )

        response = ProviderCompleteOAuthResponse(
            key_id=str(new_key.id),
            provider_type=provider_type,
            expires_at=new_cfg.expires_at or None,
            has_refresh_token=bool(new_cfg.refresh_token),
            email=email,
            replaced=replaced,
        )

        # 单个导入完成后，后台触发一次配额刷新
        safe_create_task(_refresh_quota_after_import(provider_id, provider_type, [str(new_key.id)]))

        return response

    template = _require_oauth_template(provider_type)

    # 用 refresh_token 换取 access_token
    refresh_token = payload.refresh_token.strip()
    token_url = template.oauth.token_url
    is_json = "anthropic.com" in token_url
    scope_str = " ".join(template.oauth.scopes) if template.oauth.scopes else ""

    if is_json:
        body: dict[str, Any] = {
            "grant_type": "refresh_token",
            "client_id": template.oauth.client_id,
            "refresh_token": refresh_token,
        }
        if scope_str:
            body["scope"] = scope_str
        headers = {"Content-Type": "application/json", "Accept": "application/json"}
        data = None
        json_body = body
    else:
        form: dict[str, str] = {
            "grant_type": "refresh_token",
            "client_id": template.oauth.client_id,
            "refresh_token": refresh_token,
        }
        if scope_str:
            form["scope"] = scope_str
        if template.oauth.client_secret:
            form["client_secret"] = template.oauth.client_secret
        headers = {
            "Content-Type": "application/x-www-form-urlencoded",
            "Accept": "application/json",
        }
        data = form
        json_body = None

    # proxy_config 和 key_proxy 已在上方 Kiro 分支之前统一解析

    resp = await post_oauth_token(
        provider_type=provider_type,
        token_url=token_url,
        headers=headers,
        data=data,
        json_body=json_body,
        proxy_config=proxy_config,
        timeout_seconds=30.0,
    )

    if resp.status_code < 200 or resp.status_code >= 300:
        error_reason = f"HTTP {resp.status_code}"
        try:
            error_body = resp.json()
            if "error" in error_body:
                error_reason = str(error_body.get("error_description") or error_body.get("error"))
        except Exception:
            error_reason = resp.text[:100] if resp.text else f"HTTP {resp.status_code}"
        raise InvalidRequestException(f"Refresh Token 验证失败: {error_reason}")

    token = resp.json()
    access_token = str(token.get("access_token") or "")
    new_refresh_token = str(token.get("refresh_token") or "") or refresh_token
    expires_in = token.get("expires_in")
    expires_at: int | None = None
    try:
        if expires_in is not None:
            expires_at = int(time.time()) + int(expires_in)
    except Exception:
        expires_at = None

    if not access_token:
        raise InvalidRequestException("token refresh 返回缺少 access_token")

    # 构建 auth_config
    auth_config: dict[str, Any] = {
        "provider_type": provider_type,
        "token_type": token.get("token_type"),
        "refresh_token": new_refresh_token or None,
        "expires_at": expires_at,
        "scope": token.get("scope"),
        "updated_at": int(time.time()),
    }

    auth_config = await enrich_auth_config(
        provider_type=provider_type,
        auth_config=auth_config,
        token_response=token,
        access_token=access_token,
        proxy_config=proxy_config,
    )

    # 检查是否存在重复的 OAuth 账号（失效账号允许覆盖）
    existing_key = _check_duplicate_oauth_account(db, provider_id, auth_config)
    replaced = False

    if existing_key:
        new_key = _update_existing_oauth_key(
            db, existing_key, access_token, auth_config, proxy=key_proxy
        )
        replaced = True
    else:
        # 确定账号名称
        name = (payload.name or "").strip()
        if not name:
            name = auth_config.get("email") or f"账号_{int(time.time())}"

        new_key = _create_oauth_key(
            db,
            provider_id=provider_id,
            name=name,
            access_token=access_token,
            auth_config=auth_config,
            api_formats=_get_provider_api_formats(provider),
            proxy=key_proxy,
        )

    response = ProviderCompleteOAuthResponse(
        key_id=str(new_key.id),
        provider_type=provider_type,
        expires_at=expires_at,
        has_refresh_token=bool(new_refresh_token),
        email=auth_config.get("email"),
        replaced=replaced,
    )

    # 单个导入完成后，后台触发一次配额刷新
    safe_create_task(_refresh_quota_after_import(provider_id, provider_type, [str(new_key.id)]))

    return response


# ==============================================================================
# 导入后配额刷新
# ==============================================================================


def _extract_success_key_ids(result: BatchImportResponse) -> list[str]:
    """从批量导入结果中提取成功导入的 key_id 列表。"""
    return [r.key_id for r in result.results if r.status == "success" and r.key_id]


async def _refresh_quota_after_import(
    provider_id: str,
    provider_type: str,
    key_ids: list[str],
) -> None:
    """导入完成后触发一次配额刷新（使用独立 db session）。"""
    from src.services.provider_keys.key_quota_service import (
        CODEX_WHAM_USAGE_URL,
        QUOTA_REFRESH_PROVIDER_TYPES,
        refresh_provider_quota_for_provider,
    )

    if not key_ids or provider_type not in QUOTA_REFRESH_PROVIDER_TYPES:
        return
    try:
        db = create_session()
        try:
            await refresh_provider_quota_for_provider(
                db=db,
                provider_id=provider_id,
                codex_wham_usage_url=CODEX_WHAM_USAGE_URL,
                key_ids=key_ids,
            )
        finally:
            db.close()
    except Exception as exc:
        logger.warning("[BATCH_IMPORT] 导入后配额刷新失败 (provider={}): {}", provider_id, exc)


# ==============================================================================
# 通用批量导入（支持所有 OAuth Provider）
# ==============================================================================


async def _batch_import_standard_oauth_internal(
    *,
    provider_id: str,
    provider_type: str,
    provider: Provider,
    raw_credentials: str,
    db: Session,
    proxy_config: dict[str, Any] | None = None,
    key_proxy: dict[str, Any] | None = None,
    progress_hook: BatchImportProgressHook | None = None,
    concurrency: int = 1,
) -> BatchImportResponse:
    """标准 OAuth Provider 批量导入（不含 Kiro）。"""
    template = _require_oauth_template(provider_type)
    timeout_seconds = _resolve_batch_import_timeout_seconds(proxy_config)

    import_entries = _parse_standard_oauth_import_entries(raw_credentials)
    if not import_entries:
        raise InvalidRequestException("未找到有效的 Token 数据")

    api_formats = _get_provider_api_formats(provider)
    token_url = template.oauth.token_url
    is_json = "anthropic.com" in token_url
    scope_str = " ".join(template.oauth.scopes) if template.oauth.scopes else ""

    total = len(import_entries)
    results: list[BatchImportResultItem] = [None] * total  # type: ignore[list-item]
    success_count = 0
    failed_count = 0
    processed_count = 0
    pending_success_writes = 0
    db_lock = asyncio.Lock()
    sem = asyncio.Semaphore(max(concurrency, 1))

    _release_batch_import_db_connection_before_await(db)

    async def _process_entry(idx: int, import_entry: dict[str, Any]) -> None:
        nonlocal success_count, failed_count, processed_count, pending_success_writes
        result_item: BatchImportResultItem

        async with sem:
            try:
                refresh_token = import_entry.get("refresh_token", "")
                if not refresh_token or len(refresh_token) < 10:
                    result_item = BatchImportResultItem(
                        index=idx,
                        status="error",
                        error="Token 无效或过短",
                    )
                    failed_count += 1
                else:
                    if is_json:
                        body: dict[str, Any] = {
                            "grant_type": "refresh_token",
                            "client_id": template.oauth.client_id,
                            "refresh_token": refresh_token,
                        }
                        if scope_str:
                            body["scope"] = scope_str
                        headers = {
                            "Content-Type": "application/json",
                            "Accept": "application/json",
                        }
                        data = None
                        json_body = body
                    else:
                        form: dict[str, str] = {
                            "grant_type": "refresh_token",
                            "client_id": template.oauth.client_id,
                            "refresh_token": refresh_token,
                        }
                        if scope_str:
                            form["scope"] = scope_str
                        if template.oauth.client_secret:
                            form["client_secret"] = template.oauth.client_secret
                        headers = {
                            "Content-Type": "application/x-www-form-urlencoded",
                            "Accept": "application/json",
                        }
                        data = form
                        json_body = None

                    try:
                        _release_batch_import_db_connection_before_await(db)
                        resp = await post_oauth_token(
                            provider_type=provider_type,
                            token_url=token_url,
                            headers=headers,
                            data=data,
                            json_body=json_body,
                            proxy_config=proxy_config,
                            timeout_seconds=timeout_seconds,
                        )
                    except Exception as exc:
                        result_item = BatchImportResultItem(
                            index=idx,
                            status="error",
                            error=f"Token 刷新请求失败: {exc}",
                        )
                        failed_count += 1
                        processed_count += 1
                        results[idx] = result_item
                        if progress_hook is not None:
                            _release_batch_import_db_connection_before_await(db)
                            await progress_hook(
                                total, processed_count, success_count, failed_count, result_item
                            )
                        return

                    if resp.status_code < 200 or resp.status_code >= 300:
                        error_reason = f"HTTP {resp.status_code}"
                        try:
                            error_body = resp.json()
                            if "error" in error_body:
                                error_reason = str(
                                    error_body.get("error_description") or error_body.get("error")
                                )
                        except Exception:
                            error_reason = (
                                resp.text[:100] if resp.text else f"HTTP {resp.status_code}"
                            )

                        result_item = BatchImportResultItem(
                            index=idx,
                            status="error",
                            error=f"Token 验证失败: {error_reason}",
                        )
                        failed_count += 1
                        processed_count += 1
                        results[idx] = result_item
                        if progress_hook is not None:
                            _release_batch_import_db_connection_before_await(db)
                            await progress_hook(
                                total, processed_count, success_count, failed_count, result_item
                            )
                        return

                    token_data = resp.json()
                    access_token = str(token_data.get("access_token") or "")
                    new_refresh_token = str(token_data.get("refresh_token") or "") or refresh_token

                    if not access_token:
                        result_item = BatchImportResultItem(
                            index=idx,
                            status="error",
                            error="Token 刷新返回缺少 access_token",
                        )
                        failed_count += 1
                        processed_count += 1
                        results[idx] = result_item
                        if progress_hook is not None:
                            _release_batch_import_db_connection_before_await(db)
                            await progress_hook(
                                total, processed_count, success_count, failed_count, result_item
                            )
                        return

                    expires_in = token_data.get("expires_in")
                    expires_at: int | None = None
                    try:
                        if expires_in is not None:
                            expires_at = int(time.time()) + int(expires_in)
                    except Exception:
                        expires_at = None

                    auth_config: dict[str, Any] = {
                        "provider_type": provider_type,
                        "token_type": token_data.get("token_type"),
                        "refresh_token": new_refresh_token or None,
                        "expires_at": expires_at,
                        "scope": token_data.get("scope"),
                        "updated_at": int(time.time()),
                    }

                    try:
                        _release_batch_import_db_connection_before_await(db)
                        auth_config = await enrich_auth_config(
                            provider_type=provider_type,
                            auth_config=auth_config,
                            token_response=token_data,
                            access_token=access_token,
                            proxy_config=proxy_config,
                        )
                    except Exception as exc:
                        logger.warning("批量导入: enrich_auth_config 失败 (index={}): {}", idx, exc)

                    if provider_type == ProviderType.CODEX.value:
                        _apply_codex_import_hints(auth_config, import_entry)

                    async with db_lock:
                        try:
                            existing_key = _check_duplicate_oauth_account(
                                db, provider_id, auth_config
                            )
                        except InvalidRequestException as exc:
                            result_item = BatchImportResultItem(
                                index=idx,
                                status="error",
                                error=str(exc),
                            )
                            failed_count += 1
                            processed_count += 1
                            results[idx] = result_item
                            if progress_hook is not None:
                                _release_batch_import_db_connection_before_await(db)
                                await progress_hook(
                                    total,
                                    processed_count,
                                    success_count,
                                    failed_count,
                                    result_item,
                                )
                            return

                        replaced = False
                        if existing_key:
                            new_key = _update_existing_oauth_key(
                                db,
                                existing_key,
                                access_token,
                                auth_config,
                                flush_only=True,
                                proxy=key_proxy,
                            )
                            name = existing_key.name
                            replaced = True
                        else:
                            email = auth_config.get("email")
                            if email:
                                name = f"{provider_type}_{email}"
                            else:
                                name = f"{provider_type}_{int(time.time())}_{idx}"
                            if len(name) > 100:
                                name = name[:100]

                            new_key = _create_oauth_key(
                                db,
                                provider_id=provider_id,
                                name=name,
                                access_token=access_token,
                                auth_config=auth_config,
                                api_formats=api_formats,
                                flush_only=True,
                                proxy=key_proxy,
                            )

                        pending_success_writes += 1
                        pending_success_writes = _commit_batch_import_writes_if_needed(
                            db, pending_success_writes
                        )

                    result_item = BatchImportResultItem(
                        index=idx,
                        status="success",
                        key_id=str(new_key.id),
                        key_name=name,
                        replaced=replaced,
                    )
                    success_count += 1

            except Exception as exc:
                logger.error("批量导入 OAuth 凭据失败 (index={}): {}", idx, exc)
                result_item = BatchImportResultItem(
                    index=idx,
                    status="error",
                    error=f"导入失败: {exc}",
                )
                failed_count += 1

        processed_count += 1
        results[idx] = result_item
        if progress_hook is not None:
            _release_batch_import_db_connection_before_await(db)
            await progress_hook(total, processed_count, success_count, failed_count, result_item)

    await asyncio.gather(
        *[_process_entry(i, e) for i, e in enumerate(import_entries)],
        return_exceptions=True,
    )

    if success_count > 0 and pending_success_writes > 0:
        db.commit()

    final_results = [r for r in results if r is not None]
    if len(final_results) != total:
        logger.warning("[BATCH_IMPORT] 结果不完整: expected={}, got={}", total, len(final_results))

    logger.info(
        "[BATCH_IMPORT] Provider {} ({}): 成功 {}/{}, 失败 {}",
        provider_id,
        provider_type,
        success_count,
        len(import_entries),
        failed_count,
    )

    return BatchImportResponse(
        total=len(import_entries),
        success=success_count,
        failed=failed_count,
        results=final_results,
    )


@router.post(
    "/providers/{provider_id}/batch-import",
    response_model=BatchImportResponse,
)
async def batch_import_oauth(
    provider_id: str,
    payload: BatchImportRequest,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> BatchImportResponse:
    """批量导入 OAuth 凭据（通用）。

    支持的 Provider 类型：Codex、Antigravity、GeminiCli、ClaudeCode、Kiro

    支持多种格式：
    1. JSON 数组: ["token1", "token2", ...]
    2. 纯 Token 导入（一行一个）
    3. Kiro 专用：JSON 对象或对象数组（含 refreshToken/clientId 等字段）

    批量导入时自动跳过错误，不中断导入。
    """
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException("Provider 不存在", "provider")

    provider_type = _require_fixed_provider(provider)

    # 解析代理：前端指定 proxy_node_id 时优先使用，否则回退到 Provider 级代理
    proxy_config, key_proxy = _resolve_proxy_for_oauth(
        getattr(provider, "proxy", None), payload.proxy_node_id
    )

    # 从 pool_advanced 读取批量并发数
    _pool_cfg = parse_pool_config(getattr(provider, "config", None))
    batch_concurrency = (_pool_cfg.batch_concurrency or 8) if _pool_cfg else 8

    if provider_type == ProviderType.KIRO.value:
        result = await _batch_import_kiro_internal(
            provider_id=provider_id,
            provider=provider,
            raw_credentials=payload.credentials,
            db=db,
            proxy_config=proxy_config,
            key_proxy=key_proxy,
            concurrency=batch_concurrency,
        )
    else:
        result = await _batch_import_standard_oauth_internal(
            provider_id=provider_id,
            provider_type=provider_type,
            provider=provider,
            raw_credentials=payload.credentials,
            db=db,
            proxy_config=proxy_config,
            key_proxy=key_proxy,
            concurrency=batch_concurrency,
        )

    # 导入完成后，后台触发一次配额刷新
    success_key_ids = _extract_success_key_ids(result)
    if success_key_ids:
        safe_create_task(_refresh_quota_after_import(provider_id, provider_type, success_key_ids))

    return result


async def _run_batch_import_task(
    *,
    task_id: str,
    provider_id: str,
    payload: BatchImportRequest,
) -> None:
    redis = await get_redis_client(require_redis=False)
    state = await _load_batch_task_state(task_id, redis=redis)
    if state is None:
        return

    state["status"] = "processing"
    state["started_at"] = int(time.time())
    state["message"] = "任务开始执行"
    await _save_batch_task_state(task_id, state, redis=redis)

    db = create_session()
    try:
        provider = db.query(Provider).filter(Provider.id == provider_id).first()
        if not provider:
            raise NotFoundException("Provider 不存在", "provider")

        provider_type = _require_fixed_provider(provider)
        state["provider_type"] = provider_type

        proxy_config, key_proxy = _resolve_proxy_for_oauth(
            getattr(provider, "proxy", None), payload.proxy_node_id
        )

        # 从 pool_advanced 读取批量并发数
        _pool_cfg = parse_pool_config(getattr(provider, "config", None))
        batch_concurrency = (_pool_cfg.batch_concurrency or 8) if _pool_cfg else 8

        async def progress_hook(
            total: int,
            processed: int,
            success_count: int,
            failed_count: int,
            result_item: BatchImportResultItem,
        ) -> None:
            state["total"] = total
            state["processed"] = processed
            state["success"] = success_count
            state["failed"] = failed_count
            state["progress_percent"] = int((processed * 100) / total) if total > 0 else 0
            state["message"] = f"处理中 {processed}/{total}"
            if result_item.status == "error":
                state["error"] = result_item.error
                error_samples = state.get("error_samples")
                if not isinstance(error_samples, list):
                    error_samples = []
                    state["error_samples"] = error_samples
                if len(error_samples) < _PROVIDER_OAUTH_BATCH_TASK_MAX_ERROR_SAMPLES:
                    error_samples.append(result_item.model_dump())
            try:
                await _save_batch_task_state(task_id, state, redis=redis)
            except Exception as exc:
                logger.debug(
                    "[BATCH_IMPORT_TASK] save progress failed (task_id={}): {}", task_id, exc
                )

        if provider_type == ProviderType.KIRO.value:
            result = await _batch_import_kiro_internal(
                provider_id=provider_id,
                provider=provider,
                raw_credentials=payload.credentials,
                db=db,
                proxy_config=proxy_config,
                key_proxy=key_proxy,
                progress_hook=progress_hook,
                concurrency=batch_concurrency,
            )
        else:
            result = await _batch_import_standard_oauth_internal(
                provider_id=provider_id,
                provider_type=provider_type,
                provider=provider,
                raw_credentials=payload.credentials,
                db=db,
                proxy_config=proxy_config,
                key_proxy=key_proxy,
                progress_hook=progress_hook,
                concurrency=batch_concurrency,
            )

        state["status"] = "completed"
        state["processed"] = result.total
        state["success"] = result.success
        state["failed"] = result.failed
        state["progress_percent"] = 100
        state["finished_at"] = int(time.time())
        state["message"] = f"导入完成：成功 {result.success}，失败 {result.failed}"
        await _save_batch_task_state(task_id, state, redis=redis)

        # 导入完成后触发一次配额刷新
        success_key_ids = _extract_success_key_ids(result)
        if success_key_ids:
            await _refresh_quota_after_import(provider_id, provider_type, success_key_ids)

    except Exception as exc:
        try:
            db.rollback()
        except Exception:
            pass
        state["status"] = "failed"
        state["finished_at"] = int(time.time())
        state["message"] = "导入任务执行失败"
        state["error"] = str(exc)
        await _save_batch_task_state(task_id, state, redis=redis)
        logger.error("[BATCH_IMPORT_TASK] task_id={} failed: {}", task_id, exc)
    finally:
        db.close()


@router.post(
    "/providers/{provider_id}/batch-import/tasks",
    response_model=BatchImportTaskStartResponse,
)
async def start_batch_import_oauth_task(
    provider_id: str,
    payload: BatchImportRequest,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> BatchImportTaskStartResponse:
    """创建 OAuth 批量导入异步任务。"""
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException("Provider 不存在", "provider")

    provider_type = _require_fixed_provider(provider)
    if provider_type != ProviderType.KIRO.value:
        _require_oauth_template(provider_type)

    total = _estimate_batch_import_total(provider_type, payload.credentials)
    if total <= 0:
        raise InvalidRequestException("未找到有效的 Token 数据")

    task_id = str(uuid.uuid4())
    state: dict[str, Any] = {
        "task_id": task_id,
        "provider_id": provider_id,
        "provider_type": provider_type,
        "status": "submitted",
        "total": total,
        "processed": 0,
        "success": 0,
        "failed": 0,
        "progress_percent": 0,
        "message": "任务已提交，等待执行",
        "error": None,
        "error_samples": [],
        "created_at": int(time.time()),
        "started_at": None,
        "finished_at": None,
    }
    await _save_batch_task_state(task_id, state)

    task = asyncio.create_task(
        _run_batch_import_task(
            task_id=task_id,
            provider_id=provider_id,
            payload=payload,
        )
    )
    _in_flight_batch_import_tasks.add(task)
    task.add_done_callback(_in_flight_batch_import_tasks.discard)

    return BatchImportTaskStartResponse(
        task_id=task_id,
        status="submitted",
        total=total,
        processed=0,
        success=0,
        failed=0,
        progress_percent=0,
        message="任务已提交，正在后台导入",
    )


@router.get(
    "/providers/{provider_id}/batch-import/tasks/{task_id}",
    response_model=BatchImportTaskStatusResponse,
)
async def get_batch_import_oauth_task_status(
    provider_id: str,
    task_id: str,
    _: User = Depends(require_admin),
) -> BatchImportTaskStatusResponse:
    """获取 OAuth 批量导入异步任务状态。"""
    state = await _load_batch_task_state(task_id)
    if not state:
        raise NotFoundException("批量导入任务不存在或已过期", "task")

    if str(state.get("provider_id") or "") != provider_id:
        raise NotFoundException("批量导入任务不存在", "task")

    return _task_state_to_response(state)


async def _batch_import_kiro_internal(
    provider_id: str,
    provider: Provider,
    raw_credentials: str,
    db: Session,
    proxy_config: dict[str, Any] | None = None,
    key_proxy: dict[str, Any] | None = None,
    progress_hook: BatchImportProgressHook | None = None,
    concurrency: int = 1,
) -> BatchImportResponse:
    """Kiro 批量导入内部实现（供通用端点调用）。

    Args:
        proxy_config: 本次操作使用的代理配置（已由调用方解析）
        key_proxy: 需要保存到 Key 上的代理配置
        concurrency: 并发数（从 pool_advanced.batch_concurrency 读取）
    """
    from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
    from src.services.provider.adapters.kiro.token_manager import refresh_access_token

    # 解析输入
    credentials = _parse_kiro_import_input(raw_credentials)
    if not credentials:
        raise InvalidRequestException("未找到有效的凭据数据")
    timeout_seconds = _resolve_batch_import_timeout_seconds(proxy_config)

    api_formats = _get_provider_api_formats(provider)

    total = len(credentials)
    results: list[BatchImportResultItem] = [None] * total  # type: ignore[list-item]
    success_count = 0
    failed_count = 0
    processed_count = 0
    pending_success_writes = 0
    db_lock = asyncio.Lock()
    sem = asyncio.Semaphore(max(concurrency, 1))

    _release_batch_import_db_connection_before_await(db)

    async def _process_entry(idx: int, cred: dict[str, Any]) -> None:
        nonlocal success_count, failed_count, processed_count, pending_success_writes
        result_item: BatchImportResultItem

        async with sem:
            try:
                is_valid, error_msg = KiroAuthConfig.validate_required_fields(cred)
                if not is_valid:
                    result_item = BatchImportResultItem(
                        index=idx,
                        status="error",
                        error=error_msg,
                    )
                    failed_count += 1
                    processed_count += 1
                    results[idx] = result_item
                    if progress_hook is not None:
                        _release_batch_import_db_connection_before_await(db)
                        await progress_hook(
                            total, processed_count, success_count, failed_count, result_item
                        )
                    return

                cfg = KiroAuthConfig.from_dict(cred)
                cfg.provider_type = ProviderType.KIRO.value

                try:
                    _release_batch_import_db_connection_before_await(db)
                    access_token, new_cfg = await refresh_access_token(
                        cfg,
                        proxy_config=proxy_config,
                        timeout_seconds=timeout_seconds,
                    )
                except Exception as exc:
                    result_item = BatchImportResultItem(
                        index=idx,
                        status="error",
                        error=f"Token 验证失败: {exc}",
                    )
                    failed_count += 1
                    processed_count += 1
                    results[idx] = result_item
                    if progress_hook is not None:
                        _release_batch_import_db_connection_before_await(db)
                        await progress_hook(
                            total, processed_count, success_count, failed_count, result_item
                        )
                    return

                _release_batch_import_db_connection_before_await(db)
                email = await _fetch_kiro_email(new_cfg.to_dict(), proxy_config=proxy_config)
                if email and not new_cfg.email:
                    new_cfg.email = email

                async with db_lock:
                    try:
                        existing_key = _check_duplicate_oauth_account(
                            db, provider_id, new_cfg.to_dict()
                        )
                    except InvalidRequestException as exc:
                        result_item = BatchImportResultItem(
                            index=idx,
                            status="error",
                            error=str(exc),
                        )
                        failed_count += 1
                        processed_count += 1
                        results[idx] = result_item
                        if progress_hook is not None:
                            _release_batch_import_db_connection_before_await(db)
                            await progress_hook(
                                total,
                                processed_count,
                                success_count,
                                failed_count,
                                result_item,
                            )
                        return

                    replaced = False
                    if existing_key:
                        new_key = _update_existing_oauth_key(
                            db,
                            existing_key,
                            access_token,
                            new_cfg.to_dict(),
                            flush_only=True,
                            proxy=key_proxy,
                        )
                        name = existing_key.name
                        replaced = True
                    else:
                        name = _build_kiro_key_name(
                            email, new_cfg.auth_method, new_cfg.refresh_token
                        )
                        new_key = _create_oauth_key(
                            db,
                            provider_id=provider_id,
                            name=name,
                            access_token=access_token,
                            auth_config=new_cfg.to_dict(),
                            api_formats=api_formats,
                            flush_only=True,
                            proxy=key_proxy,
                        )

                    pending_success_writes += 1
                    pending_success_writes = _commit_batch_import_writes_if_needed(
                        db, pending_success_writes
                    )

                result_item = BatchImportResultItem(
                    index=idx,
                    status="success",
                    key_id=str(new_key.id),
                    key_name=name,
                    auth_method=new_cfg.auth_method or "social",
                    replaced=replaced,
                )
                success_count += 1

            except Exception as exc:
                logger.error("批量导入 Kiro 凭据失败 (index={}): {}", idx, exc)
                result_item = BatchImportResultItem(
                    index=idx,
                    status="error",
                    error=f"导入失败: {exc}",
                )
                failed_count += 1

        processed_count += 1
        results[idx] = result_item
        if progress_hook is not None:
            _release_batch_import_db_connection_before_await(db)
            await progress_hook(total, processed_count, success_count, failed_count, result_item)

    await asyncio.gather(
        *[_process_entry(i, c) for i, c in enumerate(credentials)],
        return_exceptions=True,
    )

    # 提交所有成功的记录
    if success_count > 0 and pending_success_writes > 0:
        db.commit()

    final_results = [r for r in results if r is not None]
    if len(final_results) != total:
        logger.warning(
            "[KIRO_BATCH_IMPORT] 结果不完整: expected={}, got={}", total, len(final_results)
        )

    logger.info(
        "[KIRO_BATCH_IMPORT] Provider {}: 成功 {}/{}, 失败 {}",
        provider_id,
        success_count,
        len(credentials),
        failed_count,
    )

    return BatchImportResponse(
        total=len(credentials),
        success=success_count,
        failed=failed_count,
        results=final_results,
    )


# ==============================================================================
# Device Authorization (AWS SSO OIDC - RFC 8628)
# ==============================================================================

_DEVICE_AUTH_SESSION_PREFIX = "device_auth_session:"
_DEVICE_AUTH_SESSION_TTL_BUFFER = 60  # Redis TTL = expires_in + buffer

_KIRO_SSO_SCOPES = [
    "codewhisperer:completions",
    "codewhisperer:analysis",
    "codewhisperer:conversations",
    "codewhisperer:transformations",
    "codewhisperer:taskassist",
]
_KIRO_SSO_DEFAULT_START_URL = "https://view.awsapps.com/start"
_KIRO_SSO_DEFAULT_REGION = "us-east-1"


class DeviceAuthorizeRequest(BaseModel):
    start_url: str = Field(
        _KIRO_SSO_DEFAULT_START_URL,
        description="IAM Identity Center Start URL（如 https://your-org.awsapps.com/start）",
    )
    region: str = Field(
        _KIRO_SSO_DEFAULT_REGION,
        pattern=r"^[a-z0-9-]+$",
        description="IAM Identity Center 部署 region",
    )
    proxy_node_id: str | None = Field(None, description="代理节点 ID")


class DeviceAuthorizeResponse(BaseModel):
    session_id: str
    user_code: str
    verification_uri: str
    verification_uri_complete: str
    expires_in: int
    interval: int


class DevicePollRequest(BaseModel):
    session_id: str = Field(..., description="设备授权会话 ID")


class DevicePollResponse(BaseModel):
    status: str  # pending / authorized / slow_down / expired / error
    key_id: str | None = None
    email: str | None = None
    error: str | None = None
    replaced: bool = False


async def _sso_oidc_post(
    url: str,
    body: dict[str, Any],
    proxy_config: dict[str, Any] | None = None,
    timeout: float = 30.0,
) -> dict[str, Any]:
    """POST to AWS SSO OIDC endpoint, return parsed JSON or raise."""
    from src.clients.http_client import HTTPClientPool

    client = await HTTPClientPool.get_proxy_client(proxy_config=proxy_config)
    resp = await client.post(
        url,
        json=body,
        timeout=timeout,
    )
    if resp.status_code >= 400:
        # 返回原始 JSON 让调用方处理错误码
        try:
            err_data = resp.json()
            err_code = err_data.get("error", "")
            if err_code in ("authorization_pending", "slow_down"):
                logger.debug(
                    "SSO OIDC polling: {} returned {} ({})",
                    url,
                    resp.status_code,
                    err_code,
                )
            else:
                logger.warning(
                    "SSO OIDC request failed: {} returned {} | body={}",
                    url,
                    resp.status_code,
                    err_data,
                )
            return {"_error": True, "_status": resp.status_code, **err_data}
        except Exception:
            logger.warning(
                "SSO OIDC request failed: {} returned {} | text={}",
                url,
                resp.status_code,
                resp.text[:200],
            )
            raise InvalidRequestException(f"AWS SSO OIDC 请求失败: HTTP {resp.status_code}")
    return resp.json()


async def _register_sso_oidc_client(
    region: str,
    *,
    start_url: str,
    proxy_config: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """注册 AWS SSO OIDC 客户端 (public, device_code grant)。"""
    url = f"https://oidc.{region}.amazonaws.com/client/register"
    body = {
        "clientName": "Aether Gateway",
        "clientType": "public",
        "scopes": _KIRO_SSO_SCOPES,
        "grantTypes": [
            "urn:ietf:params:oauth:grant-type:device_code",
            "refresh_token",
        ],
        "issuerUrl": start_url,
    }
    result = await _sso_oidc_post(url, body, proxy_config=proxy_config)
    if result.get("_error"):
        error_desc = result.get("error_description") or result.get("error") or "unknown"
        raise InvalidRequestException(f"注册 OIDC 客户端失败: {error_desc}")
    return result


async def _start_device_authorization(
    region: str,
    client_id: str,
    client_secret: str,
    *,
    start_url: str,
    proxy_config: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """发起设备授权，返回 device_code / user_code / verification_uri 等。"""
    url = f"https://oidc.{region}.amazonaws.com/device_authorization"
    body = {
        "clientId": client_id,
        "clientSecret": client_secret,
        "startUrl": start_url,
    }
    result = await _sso_oidc_post(url, body, proxy_config=proxy_config)
    if result.get("_error"):
        error_desc = result.get("error_description") or result.get("error") or "unknown"
        raise InvalidRequestException(f"发起设备授权失败: {error_desc}")
    return result


async def _poll_device_token(
    region: str,
    client_id: str,
    client_secret: str,
    device_code: str,
    proxy_config: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """轮询设备授权 token 端点，返回原始 JSON（含成功或错误信息）。"""
    url = f"https://oidc.{region}.amazonaws.com/token"
    body = {
        "clientId": client_id,
        "clientSecret": client_secret,
        "grantType": "urn:ietf:params:oauth:grant-type:device_code",
        "deviceCode": device_code,
    }
    return await _sso_oidc_post(url, body, proxy_config=proxy_config)


@router.post(
    "/providers/{provider_id}/device-authorize",
    response_model=DeviceAuthorizeResponse,
)
async def device_authorize(
    provider_id: str,
    payload: DeviceAuthorizeRequest,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> DeviceAuthorizeResponse:
    """发起 AWS SSO OIDC 设备授权流程（仅限 Kiro provider）。"""
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    if not provider:
        raise NotFoundException("Provider 不存在", "provider")
    provider_type = _require_fixed_provider(provider)
    if provider_type != ProviderType.KIRO.value:
        raise InvalidRequestException("设备授权仅支持 Kiro provider")

    proxy_config, key_proxy = _resolve_proxy_for_oauth(
        getattr(provider, "proxy", None), payload.proxy_node_id
    )

    region = (payload.region or _KIRO_SSO_DEFAULT_REGION).strip()
    start_url = (payload.start_url or _KIRO_SSO_DEFAULT_START_URL).strip()

    # 1. 注册 OIDC 客户端
    client_reg = await _register_sso_oidc_client(
        region, start_url=start_url, proxy_config=proxy_config
    )
    client_id = client_reg["clientId"]
    client_secret = client_reg["clientSecret"]

    # 2. 发起设备授权
    device_auth = await _start_device_authorization(
        region, client_id, client_secret, start_url=start_url, proxy_config=proxy_config
    )
    device_code = device_auth.get("deviceCode") or device_auth.get("device_code") or ""
    user_code = device_auth.get("userCode") or device_auth.get("user_code") or ""
    verification_uri = (
        device_auth.get("verificationUri")
        or device_auth.get("verification_uri")
        or device_auth.get("verificationUrl")
        or ""
    )
    verification_uri_complete = (
        device_auth.get("verificationUriComplete")
        or device_auth.get("verification_uri_complete")
        or device_auth.get("verificationUrlComplete")
        or verification_uri
    )
    expires_in = int(device_auth.get("expiresIn") or device_auth.get("expires_in") or 600)
    interval = int(device_auth.get("interval") or 5)

    # 3. 存入 Redis
    redis = await get_redis_client(require_redis=True)
    assert redis is not None

    session_id = secrets.token_urlsafe(24)
    session_data = {
        "provider_id": provider_id,
        "region": region,
        "client_id": client_id,
        "client_secret": client_secret,
        "device_code": device_code,
        "interval": interval,
        "expires_at": int(time.time()) + expires_in,
        "status": "pending",
        "proxy_node_id": (payload.proxy_node_id or "").strip() or None,
        "created_at": int(time.time()),
    }
    redis_key = f"{_DEVICE_AUTH_SESSION_PREFIX}{session_id}"
    await redis.setex(
        redis_key,
        expires_in + _DEVICE_AUTH_SESSION_TTL_BUFFER,
        json.dumps(session_data),
    )

    return DeviceAuthorizeResponse(
        session_id=session_id,
        user_code=user_code,
        verification_uri=verification_uri,
        verification_uri_complete=verification_uri_complete,
        expires_in=expires_in,
        interval=interval,
    )


@router.post(
    "/providers/{provider_id}/device-poll",
    response_model=DevicePollResponse,
)
async def device_poll(
    provider_id: str,
    payload: DevicePollRequest,
    db: Session = Depends(get_db),
    _: User = Depends(require_admin),
) -> DevicePollResponse:
    """轮询设备授权状态，授权成功时自动创建 Key。"""
    redis = await get_redis_client(require_redis=True)
    assert redis is not None

    redis_key = f"{_DEVICE_AUTH_SESSION_PREFIX}{payload.session_id}"
    raw = await redis.get(redis_key)
    if not raw:
        return DevicePollResponse(status="expired", error="会话不存在或已过期")

    session = json.loads(raw)
    if session.get("provider_id") != provider_id:
        return DevicePollResponse(status="error", error="会话与 Provider 不匹配")

    # 已完成的会话直接返回缓存结果
    cached_status = session.get("status")
    if cached_status == "authorized":
        return DevicePollResponse(
            status="authorized",
            key_id=session.get("key_id"),
            email=session.get("email"),
            replaced=session.get("replaced", False),
        )
    if cached_status in ("expired", "error"):
        return DevicePollResponse(status=cached_status, error=session.get("error_msg"))

    # 检查是否已过期
    if int(time.time()) > session.get("expires_at", 0):
        session["status"] = "expired"
        await redis.setex(redis_key, 30, json.dumps(session))
        return DevicePollResponse(status="expired", error="设备码已过期")

    # 解析代理
    provider = db.query(Provider).filter(Provider.id == provider_id).first()
    proxy_config, key_proxy = _resolve_proxy_for_oauth(
        getattr(provider, "proxy", None) if provider else None,
        session.get("proxy_node_id"),
    )

    # 轮询 token 端点
    region = session["region"]
    token_result = await _poll_device_token(
        region=region,
        client_id=session["client_id"],
        client_secret=session["client_secret"],
        device_code=session["device_code"],
        proxy_config=proxy_config,
    )

    # 处理错误响应
    if token_result.get("_error"):
        error_code = token_result.get("error", "")
        if error_code == "authorization_pending":
            return DevicePollResponse(status="pending")
        if error_code == "slow_down":
            return DevicePollResponse(status="slow_down")
        if error_code == "expired_token":
            session["status"] = "expired"
            await redis.setex(redis_key, 30, json.dumps(session))
            return DevicePollResponse(status="expired", error="设备码已过期")
        if error_code == "access_denied":
            session["status"] = "error"
            session["error_msg"] = "用户拒绝授权"
            await redis.setex(redis_key, 30, json.dumps(session))
            return DevicePollResponse(status="error", error="用户拒绝授权")
        # 其他错误
        err_msg = token_result.get("error_description") or error_code or "未知错误"
        return DevicePollResponse(status="error", error=err_msg)

    # 成功拿到 token，执行导入流程
    access_token_raw = token_result.get("accessToken") or ""
    refresh_token_raw = token_result.get("refreshToken") or ""
    expires_in = token_result.get("expiresIn")

    if not access_token_raw or not refresh_token_raw:
        return DevicePollResponse(
            status="error", error="token 响应缺少 accessToken 或 refreshToken"
        )

    # 构建 KiroAuthConfig 并验证
    from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
    from src.services.provider.adapters.kiro.token_manager import refresh_access_token

    cfg = KiroAuthConfig(
        auth_method="idc",
        refresh_token=refresh_token_raw,
        client_id=session["client_id"],
        client_secret=session["client_secret"],
        region=region,
        access_token=access_token_raw,
        expires_at=(int(time.time()) + int(expires_in) if expires_in else 0),
    )
    cfg.provider_type = ProviderType.KIRO.value

    # 用 refresh_token 验证有效性并获取最新 token
    try:
        verified_access_token, new_cfg = await refresh_access_token(cfg, proxy_config=proxy_config)
    except Exception as e:
        logger.warning("设备授权 token 验证失败: {}", e)
        return DevicePollResponse(status="error", error=f"token 验证失败: {type(e).__name__}")

    # 获取邮箱
    email = await _fetch_kiro_email(new_cfg.to_dict(), proxy_config=proxy_config)
    if email and not new_cfg.email:
        new_cfg.email = email

    # 检查重复
    try:
        existing_key = _check_duplicate_oauth_account(db, provider_id, new_cfg.to_dict())
    except InvalidRequestException as e:
        return DevicePollResponse(status="error", error=str(e))

    # 创建/更新 Key
    replaced = False
    name = _build_kiro_key_name(email, new_cfg.auth_method, new_cfg.refresh_token)
    api_formats = _get_provider_api_formats(provider) if provider else []

    if existing_key:
        new_key = _update_existing_oauth_key(
            db, existing_key, verified_access_token, new_cfg.to_dict(), proxy=key_proxy
        )
        replaced = True
    else:
        new_key = _create_oauth_key(
            db,
            provider_id=provider_id,
            name=name,
            access_token=verified_access_token,
            auth_config=new_cfg.to_dict(),
            api_formats=api_formats,
            proxy=key_proxy,
        )

    # 更新 Redis session 为已完成（短 TTL 让前端最后一次轮询能拿到结果）
    session["status"] = "authorized"
    session["key_id"] = str(new_key.id)
    session["email"] = email
    session["replaced"] = replaced
    await redis.setex(redis_key, 60, json.dumps(session))

    # 单个导入完成后，后台触发一次配额刷新
    provider_type = ProviderType.KIRO.value
    safe_create_task(_refresh_quota_after_import(provider_id, provider_type, [str(new_key.id)]))

    return DevicePollResponse(
        status="authorized",
        key_id=str(new_key.id),
        email=email,
        replaced=replaced,
    )
