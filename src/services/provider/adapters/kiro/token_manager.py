"""Kiro token refresh helpers."""

from __future__ import annotations

import base64
import hashlib
import re
import time
from typing import Any

import httpx

from src.clients.http_client import HTTPClientPool
from src.core.logger import logger
from src.services.provider.adapters.kiro.headers import build_kiro_ide_tag
from src.services.provider.adapters.kiro.models.credentials import KiroAuthConfig
from src.utils import json_helper as json

IDC_AMZ_USER_AGENT = (
    "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown "
    "api/sso-oidc#3.738.0 m/E KiroIDE"
)

_REGION_RE = re.compile(r"^[a-z]{2}-[a-z0-9-]+-\d+$")
_HEX64_RE = re.compile(r"^[0-9a-fA-F]{64}$")
_UUID_RE = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
)


def validate_refresh_token(refresh_token: str) -> None:
    token = str(refresh_token or "").strip()
    if not token:
        raise ValueError("missing refresh_token")

    # kiro.rs: length < 100 or contains "..." is considered truncated.
    if len(token) < 100 or token.endswith("...") or "..." in token:
        raise ValueError(
            "refresh_token appears truncated; please export the full token from Kiro IDE"
        )


def normalize_machine_id(machine_id: str) -> str | None:
    raw = str(machine_id or "").strip()
    if not raw:
        return None

    if _HEX64_RE.fullmatch(raw):
        return raw.lower()

    if _UUID_RE.fullmatch(raw):
        without = raw.replace("-", "").lower()
        return without + without

    return None


def generate_machine_id(cfg: KiroAuthConfig) -> str:
    normalized = normalize_machine_id(cfg.machine_id or "")
    if normalized:
        return normalized

    validate_refresh_token(cfg.refresh_token)
    seed = f"KotlinNativeAPI/{cfg.refresh_token}".encode("utf-8")
    return hashlib.sha256(seed).hexdigest()


def is_token_expired(expires_at: int | None, *, skew_seconds: int = 120) -> bool:
    try:
        ts = int(expires_at or 0)
    except Exception:
        ts = 0
    if ts <= 0:
        return True
    return int(time.time()) >= ts - int(skew_seconds)


def _resolve_region(cfg: KiroAuthConfig) -> str:
    region = str(cfg.region or "").strip()
    if region and _REGION_RE.fullmatch(region):
        return region
    # Keep best-effort fallback; actual host parsing happens in transport hook.
    from src.services.provider.adapters.kiro.constants import DEFAULT_REGION

    return region or DEFAULT_REGION


def _try_extract_email_from_jwt(token: str) -> str | None:
    """尝试从 JWT access_token 中提取 email。

    Kiro Social / IdC 返回的 accessToken 可能是 JWT 格式，
    payload 中可能包含 email 字段。仅做 base64 解码，不验证签名。
    失败时静默返回 None。
    """
    try:
        parts = token.split(".")
        if len(parts) != 3:
            return None
        # base64url decode the payload (second segment)
        payload_b64 = parts[1]
        # Add padding
        padding = 4 - len(payload_b64) % 4
        if padding != 4:
            payload_b64 += "=" * padding
        payload_bytes = base64.urlsafe_b64decode(payload_b64)
        claims = json.loads(payload_bytes)
        # Try common email claim keys
        for key in ("email", "Email", "mail", "upn"):
            val = claims.get(key)
            if isinstance(val, str) and "@" in val:
                return val.strip()
    except Exception:
        pass
    return None


async def refresh_social_token(
    cfg: KiroAuthConfig,
    *,
    proxy_config: dict[str, Any] | None,
    timeout_seconds: float = 30.0,
) -> tuple[str, KiroAuthConfig]:
    """Refresh access token via Kiro Social refresh endpoint."""
    validate_refresh_token(cfg.refresh_token)

    region = _resolve_region(cfg)
    url = f"https://prod.{region}.auth.desktop.kiro.dev/refreshToken"
    host = f"prod.{region}.auth.desktop.kiro.dev"

    machine_id = generate_machine_id(cfg)
    kiro_version = (cfg.kiro_version or "").strip() or "0.8.0"
    ua = build_kiro_ide_tag(kiro_version=kiro_version, machine_id=machine_id)

    body = {"refreshToken": cfg.refresh_token}
    headers = {
        "User-Agent": ua,
        "Host": host,
        "Accept": "application/json, text/plain, */*",
        "Content-Type": "application/json",
        "Connection": "close",
        "Accept-Encoding": "gzip, compress, deflate, br",
    }

    client = await HTTPClientPool.get_proxy_client(proxy_config=proxy_config)
    resp = await client.post(
        url,
        headers=headers,
        json=body,
        timeout=httpx.Timeout(timeout_seconds),
    )

    if resp.status_code < 200 or resp.status_code >= 300:
        logger.debug(
            "kiro social refresh error: HTTP {} | {}",
            resp.status_code,
            (resp.text or "").strip()[:200],
        )
        raise RuntimeError(f"kiro social refresh failed: HTTP {resp.status_code}")

    data: dict[str, Any]
    try:
        data = resp.json()
    except Exception as e:
        raise RuntimeError("kiro social refresh: invalid json response") from e

    access_token = str(data.get("accessToken") or "").strip()
    if not access_token:
        raise RuntimeError("kiro social refresh returned empty accessToken")

    new_cfg = KiroAuthConfig.from_dict(cfg.to_dict())

    # refreshToken/profileArn may rotate
    rt = data.get("refreshToken")
    if isinstance(rt, str) and rt.strip():
        new_cfg.refresh_token = rt.strip()

    profile_arn = data.get("profileArn")
    if isinstance(profile_arn, str) and profile_arn.strip():
        new_cfg.profile_arn = profile_arn.strip()

    expires_in = data.get("expiresIn")
    try:
        if expires_in is not None:
            new_cfg.expires_at = int(time.time()) + int(expires_in)
    except Exception:
        new_cfg.expires_at = int(time.time()) + 3600

    # Persist computed machine_id if user didn't provide one.
    if not (cfg.machine_id or "").strip():
        new_cfg.machine_id = machine_id

    # 尝试从 accessToken 中提取 email（如果尚未设置）
    if not (new_cfg.email or "").strip():
        extracted_email = _try_extract_email_from_jwt(access_token)
        if extracted_email:
            new_cfg.email = extracted_email
            logger.debug("kiro social: extracted email from accessToken: {}", extracted_email)

    # 缓存 access_token
    new_cfg.access_token = access_token

    return access_token, new_cfg


async def refresh_idc_token(
    cfg: KiroAuthConfig,
    *,
    proxy_config: dict[str, Any] | None,
    timeout_seconds: float = 30.0,
) -> tuple[str, KiroAuthConfig]:
    """Refresh access token via AWS SSO OIDC endpoint (IdC)."""
    validate_refresh_token(cfg.refresh_token)

    if not (cfg.client_id or "").strip() or not (cfg.client_secret or "").strip():
        raise ValueError("idc refresh requires client_id and client_secret")

    region = _resolve_region(cfg)
    url = f"https://oidc.{region}.amazonaws.com/token"
    host = f"oidc.{region}.amazonaws.com"

    body = {
        "clientId": cfg.client_id,
        "clientSecret": cfg.client_secret,
        "refreshToken": cfg.refresh_token,
        "grantType": "refresh_token",
    }

    headers = {
        "Content-Type": "application/json",
        "Host": host,
        "x-amz-user-agent": IDC_AMZ_USER_AGENT,
        "User-Agent": "node",
        "Accept": "*/*",
    }

    client = await HTTPClientPool.get_proxy_client(proxy_config=proxy_config)
    resp = await client.post(
        url,
        headers=headers,
        json=body,
        timeout=httpx.Timeout(timeout_seconds),
    )

    if resp.status_code < 200 or resp.status_code >= 300:
        logger.debug(
            "kiro idc refresh error: HTTP {} | {}",
            resp.status_code,
            (resp.text or "").strip()[:200],
        )
        raise RuntimeError(f"kiro idc refresh failed: HTTP {resp.status_code}")

    data: dict[str, Any]
    try:
        data = resp.json()
    except Exception as e:
        raise RuntimeError("kiro idc refresh: invalid json response") from e

    access_token = str(data.get("accessToken") or "").strip()
    if not access_token:
        raise RuntimeError("kiro idc refresh returned empty accessToken")

    new_cfg = KiroAuthConfig.from_dict(cfg.to_dict())

    rt = data.get("refreshToken")
    if isinstance(rt, str) and rt.strip():
        new_cfg.refresh_token = rt.strip()

    expires_in = data.get("expiresIn")
    try:
        if expires_in is not None:
            new_cfg.expires_at = int(time.time()) + int(expires_in)
    except Exception:
        new_cfg.expires_at = int(time.time()) + 3600

    # Persist computed machine_id if user didn't provide one.
    if not (cfg.machine_id or "").strip():
        new_cfg.machine_id = generate_machine_id(cfg)

    # 尝试从 accessToken 中提取 email（如果尚未设置）
    if not (new_cfg.email or "").strip():
        extracted_email = _try_extract_email_from_jwt(access_token)
        if extracted_email:
            new_cfg.email = extracted_email
            logger.debug("kiro idc: extracted email from accessToken: {}", extracted_email)

    # 缓存 access_token
    new_cfg.access_token = access_token

    return access_token, new_cfg


async def refresh_access_token(
    cfg: KiroAuthConfig,
    *,
    proxy_config: dict[str, Any] | None,
) -> tuple[str, KiroAuthConfig]:
    method = (cfg.auth_method or "social").strip().lower()
    if method == "idc":
        return await refresh_idc_token(cfg, proxy_config=proxy_config)
    return await refresh_social_token(cfg, proxy_config=proxy_config)


__all__ = [
    "IDC_AMZ_USER_AGENT",
    "generate_machine_id",
    "is_token_expired",
    "normalize_machine_id",
    "refresh_access_token",
    "refresh_idc_token",
    "refresh_social_token",
    "validate_refresh_token",
]
