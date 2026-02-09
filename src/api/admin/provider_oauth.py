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

import base64
import hashlib
import json
import secrets
import time
from dataclasses import dataclass
from typing import Any
from urllib.parse import parse_qsl, urlencode, urlparse

import httpx
from fastapi import APIRouter, Depends, Request
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
from src.database.database import get_db
from src.models.database import Provider, ProviderAPIKey, User
from src.utils.auth_utils import require_admin

router = APIRouter(prefix="/api/admin/provider-oauth", tags=["Provider OAuth"])


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


# ==============================================================================
# Helpers
# ==============================================================================


def _require_fixed_provider(provider: Provider) -> str:
    provider_type = (getattr(provider, "provider_type", "custom") or "custom").strip()
    if provider_type == ProviderType.CUSTOM:
        raise InvalidRequestException("该 Provider 不是固定类型，无法使用 provider-oauth")
    return provider_type


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
    auto_fetch_models: bool = True,
) -> "ProviderAPIKey":
    """创建 OAuth Key 记录并持久化。

    Args:
        flush_only: True 时仅 flush（批量导入场景），False 时 commit + refresh。
        proxy: Key 级别代理配置（如 {"node_id": "xxx", "enabled": True}），
               创建时设置后，后续 token 刷新、额度刷新等操作立即走代理，避免 IP 污染。
        auto_fetch_models: 是否启用自动获取上游模型，非 custom 提供商默认开启。
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


async def _trigger_auto_fetch_models(key_ids: list[str]) -> None:
    """为启用了 auto_fetch_models 的新建 Key 触发模型获取。"""
    if not key_ids:
        return
    try:
        from src.services.model.fetch_scheduler import get_model_fetch_scheduler

        scheduler = get_model_fetch_scheduler()
        for key_id in key_ids:
            logger.info("[AUTO_FETCH] OAuth Key {} 默认开启自动获取模型，触发模型获取", key_id)
            try:
                await scheduler._fetch_models_for_key_by_id(key_id)
            except Exception as e:
                logger.error(f"[AUTO_FETCH] Key {key_id} 触发模型获取失败: {e}")
    except Exception as e:
        logger.error(f"[AUTO_FETCH] 获取 ModelFetchScheduler 失败: {e}")


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
        logger.warning("[KIRO] 获取用户邮箱失败: {}", e)

    return None


def _check_duplicate_oauth_account(
    db: Session,
    provider_id: str,
    auth_config: dict[str, Any],
    exclude_key_id: str | None = None,
) -> None:
    """
    检查是否存在重复的 OAuth 账号

    通过以下字段判断重复：
    - account_id: Codex 等使用的账号 ID
    - email + auth_method: Kiro 使用 email + auth_method 组合判断
      （同一邮箱可能通过 Social 和 IdC 两种方式登录，视为不同账号）
    - email: 其他 OAuth Provider 使用邮箱判断

    Args:
        db: 数据库 session
        provider_id: Provider ID
        auth_config: 新账号的 auth_config
        exclude_key_id: 排除的 Key ID（用于更新场景）

    Raises:
        InvalidRequestException: 如果发现重复账号
    """
    new_email = auth_config.get("email")
    new_account_id = auth_config.get("account_id")
    new_auth_method = auth_config.get("auth_method")  # Kiro: social / idc
    new_provider_type = auth_config.get("provider_type")

    # 如果没有可用于识别的字段，跳过检查
    if not new_email and not new_account_id:
        return

    # 查询该 Provider 下所有 OAuth 类型的 Keys
    query = db.query(ProviderAPIKey).filter(
        ProviderAPIKey.provider_id == provider_id,
        ProviderAPIKey.auth_type.in_(["oauth", "kiro"]),  # kiro 也是 OAuth 类型
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
            existing_account_id = decrypted_config.get("account_id")
            existing_auth_method = decrypted_config.get("auth_method")
            existing_provider_type = decrypted_config.get("provider_type")

            # account_id 相同即重复（Codex 等）
            if new_account_id and existing_account_id and new_account_id == existing_account_id:
                raise InvalidRequestException(
                    f"该 OAuth 账号已存在于当前 Provider 中（名称: {existing_key.name}）"
                )

            # email 判断
            if new_email and existing_email and new_email == existing_email:
                is_kiro = new_provider_type == "kiro" or existing_provider_type == "kiro"
                if is_kiro:
                    # Kiro: 只有 email + auth_method 都相同才视为重复
                    # 同一邮箱可能通过 Social 和 IdC 两种方式登录，视为不同账号
                    if new_auth_method and existing_auth_method:
                        if new_auth_method.lower() == existing_auth_method.lower():
                            auth_method_display = (
                                "Social" if new_auth_method.lower() == "social" else "IdC"
                            )
                            raise InvalidRequestException(
                                f"该 Kiro 账号 ({new_email}, {auth_method_display}) "
                                f"已存在于当前 Provider 中（名称: {existing_key.name}）"
                            )
                    # auth_method 不同，不视为重复
                else:
                    # 非 Kiro Provider: 仅 email 相同即视为重复
                    raise InvalidRequestException(
                        f"该 OAuth 账号 ({new_email}) 已存在于当前 Provider 中"
                        f"（名称: {existing_key.name}）"
                    )
        except InvalidRequestException:
            raise
        except Exception:
            # 解密失败时跳过该 Key
            continue


# ==============================================================================
# Routes
# ==============================================================================


@router.get("/supported-types")
async def supported_types(_: User = Depends(require_admin)) -> list[dict[str, Any]]:
    # 不返回 client_secret
    result: list[dict[str, Any]] = []
    for provider_type, template in FIXED_PROVIDERS.items():
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

    try:
        template = FIXED_PROVIDERS.get(ProviderType(provider_type))
    except Exception:
        template = None
    if not template:
        raise InvalidRequestException("不支持的 provider_type")

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

    try:
        template = FIXED_PROVIDERS.get(ProviderType(provider_type))
    except Exception:
        template = None
    if not template:
        raise InvalidRequestException("不支持的 provider_type")

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

    # store
    key.api_key = crypto_service.encrypt(access_token)
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

    key.auth_config = crypto_service.encrypt(json.dumps(auth_config))
    db.commit()

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
    key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == key_id).first()
    if not key:
        raise NotFoundException("Key 不存在", "key")
    if (getattr(key, "auth_type", "api_key") or "api_key") != "oauth":
        raise InvalidRequestException("该 Key 不是 oauth 认证类型")

    provider = db.query(Provider).filter(Provider.id == key.provider_id).first()
    if not provider:
        raise NotFoundException("Provider 不存在", "provider")
    provider_type = _require_fixed_provider(provider)

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
            # 标记为失效
            key.oauth_invalid_at = datetime.now(timezone.utc)
            key.oauth_invalid_reason = str(e)
            db.commit()
            logger.warning("Kiro Key {} token 刷新失败，已标记为失效: {}", key_id, e)
            raise InvalidRequestException("Kiro token refresh 失败，请检查凭据是否有效")

        # 更新 key
        key.api_key = crypto_service.encrypt(access_token)
        key.auth_config = crypto_service.encrypt(json.dumps(new_cfg.to_dict()))
        key.oauth_invalid_at = None
        key.oauth_invalid_reason = None
        db.commit()

        return CompleteOAuthResponse(
            provider_type=provider_type,
            expires_at=new_cfg.expires_at or None,
            has_refresh_token=bool(new_cfg.refresh_token),
            email=None,
        )

    try:
        template = FIXED_PROVIDERS.get(ProviderType(provider_type))
    except Exception:
        template = None
    if not template:
        raise InvalidRequestException("不支持的 provider_type")

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
        # 解析错误原因
        error_reason = f"HTTP {resp.status_code}"
        try:
            error_body = resp.json()
            if "error" in error_body:
                error_reason = str(error_body.get("error_description") or error_body.get("error"))
        except Exception:
            error_reason = resp.text[:100] if resp.text else f"HTTP {resp.status_code}"

        # 标记为失效（400/401/403 通常表示永久性错误）
        if resp.status_code in (400, 401, 403):
            from datetime import datetime, timezone

            key.oauth_invalid_at = datetime.now(timezone.utc)
            key.oauth_invalid_reason = error_reason
            db.commit()
            logger.warning("Key {} OAuth token 刷新失败，已标记为失效: {}", key_id, error_reason)

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

    # store
    key.api_key = crypto_service.encrypt(access_token)
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

    # Antigravity：enrich_auth_config 会自动尝试补 project_id，
    # 即使本次仍未获取到也不阻断刷新（token 已成功更新），下次刷新会继续重试
    if provider_type == ProviderType.ANTIGRAVITY and not parsed.get("project_id"):
        logger.warning(
            "[OAUTH_REFRESH] Antigravity key {} 刷新成功但 project_id 仍缺失，"
            "下次刷新将继续尝试获取",
            key_id,
        )

    key.auth_config = crypto_service.encrypt(json.dumps(parsed))
    # 刷新成功，清除 token 级别的失效标记
    # 但保留账号级别的失效标记（以 OAUTH_ACCOUNT_BLOCK_PREFIX 开头），
    # 这种不是 token 问题，刷新 token 解决不了
    from src.services.provider.oauth_token import is_account_level_block

    if not is_account_level_block(getattr(key, "oauth_invalid_reason", None)):
        key.oauth_invalid_at = None
        key.oauth_invalid_reason = None
    db.commit()

    return CompleteOAuthResponse(
        provider_type=provider_type,
        expires_at=expires_at,
        has_refresh_token=bool(parsed.get("refresh_token")),
        email=parsed.get("email"),
    )


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

    try:
        template = FIXED_PROVIDERS.get(ProviderType(provider_type))
    except Exception:
        template = None
    if not template:
        raise InvalidRequestException("不支持的 provider_type")

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

    try:
        template = FIXED_PROVIDERS.get(ProviderType(provider_type))
    except Exception:
        template = None
    if not template:
        raise InvalidRequestException("不支持的 provider_type")

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

    # 检查是否存在重复的 OAuth 账号
    _check_duplicate_oauth_account(db, provider_id, auth_config)

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

    # 默认开启了 auto_fetch_models，触发模型获取
    await _trigger_auto_fetch_models([str(new_key.id)])

    return ProviderCompleteOAuthResponse(
        key_id=str(new_key.id),
        provider_type=provider_type,
        expires_at=expires_at,
        has_refresh_token=bool(refresh_token),
        email=auth_config.get("email"),
    )


# ==============================================================================
# Import Refresh Token (从导出文件导入)
# ==============================================================================


def _parse_tokens_input(raw_input: str) -> list[str]:
    """
    解析通用 Token 导入输入，支持多种格式。

    支持的格式：
    1. 单个 Token 字符串
    2. JSON 数组: ["token1", "token2", ...]
    3. 纯 Token 导入（一行一个）: "token1\\ntoken2\\ntoken3"

    返回: Token 字符串列表
    """
    raw = raw_input.strip()
    if not raw:
        return []

    result: list[str] = []

    # 尝试解析为 JSON 数组
    if raw.startswith("["):
        try:
            parsed = json.loads(raw)
            if isinstance(parsed, list):
                for item in parsed:
                    if isinstance(item, str) and item.strip():
                        result.append(item.strip())
                return result
        except json.JSONDecodeError:
            pass  # 不是有效 JSON，继续尝试其他格式

    # 纯 Token 导入（一行一个）
    lines = raw.splitlines()
    for line in lines:
        token = line.strip()
        if token and not token.startswith("#"):  # 忽略空行和注释行
            result.append(token)

    return result


def _parse_kiro_import_input(raw_input: str) -> list[dict[str, Any]]:
    """
    解析 Kiro 凭据导入输入。

    支持的格式：
    1. 扁平 JSON 对象: {"refresh_token": "...", "auth_method": "social", ...}
    2. JSON 数组（批量）: [{...}, {...}]
    3. 纯 Token（一行一个）: "token1\\ntoken2"

    返回: 凭据字典列表
    """
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
                    if isinstance(item, dict):
                        result.append(item)
                    elif isinstance(item, str) and item.strip():
                        result.append({"refreshToken": item.strip()})
                return result

            if isinstance(parsed, dict):
                # 兼容嵌套格式: {"auth_config": {...}} / {"authConfig": {...}}
                nested = parsed.get("auth_config") or parsed.get("authConfig")
                if isinstance(nested, dict):
                    return [nested]
                return [parsed]

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
        max_length=500_000,
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


class BatchImportResponse(BaseModel):
    """批量导入响应"""

    total: int = Field(..., description="总凭据数")
    success: int = Field(..., description="成功导入数")
    failed: int = Field(..., description="失败数")
    results: list[BatchImportResultItem] = Field(..., description="每个凭据的导入结果")


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
            logger.warning("Kiro Refresh Token 验证失败: {}", e)
            raise InvalidRequestException("Kiro Refresh Token 验证失败，请检查凭据是否有效")

        # 检查是否存在重复的 Kiro 账号
        _check_duplicate_oauth_account(db, provider_id, new_cfg.to_dict())

        # Kiro 确定账号名称（与 Codex/Antigravity 保持一致，使用 email）
        name = (payload.name or "").strip()
        email: str | None = None
        if not name:
            email = await _fetch_kiro_email(new_cfg.to_dict(), proxy_config=proxy_config)
            name = email or f"账号_{int(time.time())}"

        # 将获取到的 email 写回 auth_config，确保持久化
        if email and not new_cfg.email:
            new_cfg.email = email

        new_key = _create_oauth_key(
            db,
            provider_id=provider_id,
            name=name,
            access_token=access_token,
            auth_config=new_cfg.to_dict(),
            api_formats=_get_provider_api_formats(provider),
            proxy=key_proxy,
        )

        # 默认开启了 auto_fetch_models，触发模型获取
        await _trigger_auto_fetch_models([str(new_key.id)])

        return ProviderCompleteOAuthResponse(
            key_id=str(new_key.id),
            provider_type=provider_type,
            expires_at=new_cfg.expires_at or None,
            has_refresh_token=bool(new_cfg.refresh_token),
            email=email,
        )

    try:
        template = FIXED_PROVIDERS.get(ProviderType(provider_type))
    except Exception:
        template = None
    if not template:
        raise InvalidRequestException("不支持的 provider_type")

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

    # 检查是否存在重复的 OAuth 账号
    _check_duplicate_oauth_account(db, provider_id, auth_config)

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

    # 默认开启了 auto_fetch_models，触发模型获取
    await _trigger_auto_fetch_models([str(new_key.id)])

    return ProviderCompleteOAuthResponse(
        key_id=str(new_key.id),
        provider_type=provider_type,
        expires_at=expires_at,
        has_refresh_token=bool(new_refresh_token),
        email=auth_config.get("email"),
    )


# ==============================================================================
# 通用批量导入（支持所有 OAuth Provider）
# ==============================================================================


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

    # Kiro 使用专用逻辑
    if provider_type == ProviderType.KIRO.value:
        return await _batch_import_kiro_internal(
            provider_id=provider_id,
            provider=provider,
            raw_credentials=payload.credentials,
            db=db,
            proxy_config=proxy_config,
            key_proxy=key_proxy,
        )

    # 标准 OAuth Provider（Codex、Antigravity、GeminiCli、ClaudeCode）
    try:
        template = FIXED_PROVIDERS.get(ProviderType(provider_type))
    except Exception:
        template = None
    if not template:
        raise InvalidRequestException(f"不支持的 provider_type: {provider_type}")

    # 解析 Token 列表
    tokens = _parse_tokens_input(payload.credentials)
    if not tokens:
        raise InvalidRequestException("未找到有效的 Token 数据")

    api_formats = _get_provider_api_formats(provider)
    token_url = template.oauth.token_url
    is_json = "anthropic.com" in token_url
    scope_str = " ".join(template.oauth.scopes) if template.oauth.scopes else ""

    results: list[BatchImportResultItem] = []
    success_count = 0
    failed_count = 0

    for idx, refresh_token in enumerate(tokens):
        try:
            # 验证 Token 非空
            if not refresh_token or len(refresh_token) < 10:
                results.append(
                    BatchImportResultItem(
                        index=idx,
                        status="error",
                        error="Token 无效或过短",
                    )
                )
                failed_count += 1
                continue

            # 使用 refresh_token 换取 access_token
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

            try:
                resp = await post_oauth_token(
                    provider_type=provider_type,
                    token_url=token_url,
                    headers=headers,
                    data=data,
                    json_body=json_body,
                    proxy_config=proxy_config,
                    timeout_seconds=30.0,
                )
            except Exception as e:
                results.append(
                    BatchImportResultItem(
                        index=idx,
                        status="error",
                        error=f"Token 刷新请求失败: {e}",
                    )
                )
                failed_count += 1
                continue

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

                results.append(
                    BatchImportResultItem(
                        index=idx,
                        status="error",
                        error=f"Token 验证失败: {error_reason}",
                    )
                )
                failed_count += 1
                continue

            token_data = resp.json()
            access_token = str(token_data.get("access_token") or "")
            new_refresh_token = str(token_data.get("refresh_token") or "") or refresh_token

            if not access_token:
                results.append(
                    BatchImportResultItem(
                        index=idx,
                        status="error",
                        error="Token 刷新返回缺少 access_token",
                    )
                )
                failed_count += 1
                continue

            expires_in = token_data.get("expires_in")
            expires_at: int | None = None
            try:
                if expires_in is not None:
                    expires_at = int(time.time()) + int(expires_in)
            except Exception:
                expires_at = None

            # 构建 auth_config
            auth_config: dict[str, Any] = {
                "provider_type": provider_type,
                "token_type": token_data.get("token_type"),
                "refresh_token": new_refresh_token or None,
                "expires_at": expires_at,
                "scope": token_data.get("scope"),
                "updated_at": int(time.time()),
            }

            # 获取额外信息（email 等）
            try:
                auth_config = await enrich_auth_config(
                    provider_type=provider_type,
                    auth_config=auth_config,
                    token_response=token_data,
                    access_token=access_token,
                    proxy_config=proxy_config,
                )
            except Exception as e:
                logger.warning("批量导入: enrich_auth_config 失败 (index={}): {}", idx, e)
                # 不中断，继续使用基本 auth_config

            # 检查是否存在重复
            try:
                _check_duplicate_oauth_account(db, provider_id, auth_config)
            except InvalidRequestException as e:
                results.append(
                    BatchImportResultItem(
                        index=idx,
                        status="error",
                        error=str(e),
                    )
                )
                failed_count += 1
                continue

            # 生成名称
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

            results.append(
                BatchImportResultItem(
                    index=idx,
                    status="success",
                    key_id=str(new_key.id),
                    key_name=name,
                )
            )
            success_count += 1

        except Exception as e:
            logger.error("批量导入 OAuth 凭据失败 (index={}): {}", idx, e)
            results.append(
                BatchImportResultItem(
                    index=idx,
                    status="error",
                    error=f"导入失败: {e}",
                )
            )
            failed_count += 1

    # 提交所有成功的记录
    if success_count > 0:
        db.commit()

    # 批量导入完成后，触发所有成功 Key 的模型获取
    success_key_ids = [r.key_id for r in results if r.status == "success" and r.key_id]
    if success_key_ids:
        await _trigger_auto_fetch_models(success_key_ids)

    logger.info(
        "[BATCH_IMPORT] Provider {} ({}): 成功 {}/{}, 失败 {}",
        provider_id,
        provider_type,
        success_count,
        len(tokens),
        failed_count,
    )

    return BatchImportResponse(
        total=len(tokens),
        success=success_count,
        failed=failed_count,
        results=results,
    )


async def _batch_import_kiro_internal(
    provider_id: str,
    provider: Provider,
    raw_credentials: str,
    db: Session,
    proxy_config: dict[str, Any] | None = None,
    key_proxy: dict[str, Any] | None = None,
) -> BatchImportResponse:
    """Kiro 批量导入内部实现（供通用端点调用）。

    Args:
        proxy_config: 本次操作使用的代理配置（已由调用方解析）
        key_proxy: 需要保存到 Key 上的代理配置
    """
    from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
    from src.services.provider.adapters.kiro.token_manager import refresh_access_token

    # 解析输入
    credentials = _parse_kiro_import_input(raw_credentials)
    if not credentials:
        raise InvalidRequestException("未找到有效的凭据数据")

    api_formats = _get_provider_api_formats(provider)

    results: list[BatchImportResultItem] = []
    success_count = 0
    failed_count = 0

    for idx, cred in enumerate(credentials):
        try:
            # 验证必需字段
            is_valid, error_msg = KiroAuthConfig.validate_required_fields(cred)
            if not is_valid:
                results.append(
                    BatchImportResultItem(
                        index=idx,
                        status="error",
                        error=error_msg,
                    )
                )
                failed_count += 1
                continue

            # 解析凭据配置
            cfg = KiroAuthConfig.from_dict(cred)
            cfg.provider_type = ProviderType.KIRO.value

            # 刷新 Token 以验证有效性
            try:
                access_token, new_cfg = await refresh_access_token(cfg, proxy_config=proxy_config)
            except Exception as e:
                results.append(
                    BatchImportResultItem(
                        index=idx,
                        status="error",
                        error=f"Token 验证失败: {e}",
                    )
                )
                failed_count += 1
                continue

            # 检查是否存在重复
            try:
                _check_duplicate_oauth_account(db, provider_id, new_cfg.to_dict())
            except InvalidRequestException as e:
                results.append(
                    BatchImportResultItem(
                        index=idx,
                        status="error",
                        error=str(e),
                    )
                )
                failed_count += 1
                continue

            # Kiro 确定账号名称（与 Codex/Antigravity 保持一致，使用 email）
            email = await _fetch_kiro_email(new_cfg.to_dict(), proxy_config=proxy_config)
            name = email or f"账号_{int(time.time())}"

            # 将获取到的 email 写回 auth_config，确保持久化
            if email and not new_cfg.email:
                new_cfg.email = email

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

            results.append(
                BatchImportResultItem(
                    index=idx,
                    status="success",
                    key_id=str(new_key.id),
                    key_name=name,
                    auth_method=new_cfg.auth_method or "social",
                )
            )
            success_count += 1

        except Exception as e:
            logger.error("批量导入 Kiro 凭据失败 (index={}): {}", idx, e)
            results.append(
                BatchImportResultItem(
                    index=idx,
                    status="error",
                    error=f"导入失败: {e}",
                )
            )
            failed_count += 1

    # 提交所有成功的记录
    if success_count > 0:
        db.commit()

    # 批量导入完成后，触发所有成功 Key 的模型获取
    success_key_ids = [r.key_id for r in results if r.status == "success" and r.key_id]
    if success_key_ids:
        await _trigger_auto_fetch_models(success_key_ids)

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
        results=results,
    )
