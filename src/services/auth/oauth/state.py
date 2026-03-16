from __future__ import annotations

import json
import secrets
import time
from collections.abc import Awaitable
from dataclasses import dataclass
from typing import Any, cast

from redis.asyncio import Redis

OAUTH_STATE_TTL_SECONDS = 600
OAUTH_STATE_KEY_PREFIX = "oauth_state:"

# OAuth bind token: 用于安全地在浏览器跳转时传递用户身份
# 短期有效（5分钟），一次性使用
OAUTH_BIND_TOKEN_TTL_SECONDS = 300
OAUTH_BIND_TOKEN_KEY_PREFIX = "oauth_bind_token:"


CONSUME_STATE_SCRIPT = r"""
local value = redis.call("GET", KEYS[1])
if value then
    redis.call("DEL", KEYS[1])
end
return value
"""


@dataclass(frozen=True)
class OAuthStateData:
    nonce: str
    provider_type: str
    action: str  # "login" | "bind"
    user_id: str | None
    client_device_id: str | None
    created_at: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> OAuthStateData:
        return cls(
            nonce=str(data.get("nonce") or ""),
            provider_type=str(data.get("provider_type") or ""),
            action=str(data.get("action") or ""),
            user_id=data.get("user_id"),
            client_device_id=data.get("client_device_id"),
            created_at=int(data.get("created_at") or 0),
        )


def _state_key(nonce: str) -> str:
    return f"{OAUTH_STATE_KEY_PREFIX}{nonce}"


async def create_oauth_state(
    redis: Redis,
    *,
    provider_type: str,
    action: str,
    user_id: str | None = None,
    client_device_id: str | None = None,
) -> str:
    nonce = secrets.token_urlsafe(24)
    data = {
        "nonce": nonce,
        "provider_type": provider_type,
        "action": action,
        "user_id": user_id,
        "client_device_id": client_device_id,
        "created_at": int(time.time()),
    }
    await redis.setex(_state_key(nonce), OAUTH_STATE_TTL_SECONDS, json.dumps(data))
    return nonce


async def consume_oauth_state(redis: Redis, nonce: str) -> OAuthStateData | None:
    if not nonce:
        return None

    key = _state_key(nonce)
    # redis-py 的类型标注在 sync/async 之间会出现 Union；这里明确按 async 处理。
    raw = await cast(Awaitable[str | None], redis.eval(CONSUME_STATE_SCRIPT, 1, key))
    if not raw:
        return None

    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return None

    return OAuthStateData.from_dict(parsed)


@dataclass(frozen=True)
class OAuthBindTokenData:
    """OAuth 绑定临时令牌数据，用于浏览器跳转场景的安全认证"""

    token: str
    user_id: str
    provider_type: str
    created_at: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> OAuthBindTokenData:
        return cls(
            token=str(data.get("token") or ""),
            user_id=str(data.get("user_id") or ""),
            provider_type=str(data.get("provider_type") or ""),
            created_at=int(data.get("created_at") or 0),
        )


def _bind_token_key(token: str) -> str:
    return f"{OAUTH_BIND_TOKEN_KEY_PREFIX}{token}"


async def create_oauth_bind_token(redis: Redis, *, user_id: str, provider_type: str) -> str:
    """创建一次性 OAuth 绑定令牌，用于浏览器跳转场景"""
    token = secrets.token_urlsafe(32)
    data = {
        "token": token,
        "user_id": user_id,
        "provider_type": provider_type,
        "created_at": int(time.time()),
    }
    await redis.setex(_bind_token_key(token), OAUTH_BIND_TOKEN_TTL_SECONDS, json.dumps(data))
    return token


async def consume_oauth_bind_token(redis: Redis, token: str) -> OAuthBindTokenData | None:
    """消费（验证并删除）OAuth 绑定令牌，返回令牌数据或 None"""
    if not token:
        return None

    key = _bind_token_key(token)
    raw = await cast(Awaitable[str | None], redis.eval(CONSUME_STATE_SCRIPT, 1, key))
    if not raw:
        return None

    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return None

    return OAuthBindTokenData.from_dict(parsed)
