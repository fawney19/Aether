from __future__ import annotations

import asyncio
import json
import random
from typing import Any, Awaitable, Callable
from urllib.parse import quote, urlsplit, urlunsplit

import httpx
import jwt

from src.clients.http_client import HTTPClientPool
from src.core.logger import logger  # pyright: ignore
from src.core.provider_types import ProviderType

_ANTHROPIC_TOKEN_URL = "https://console.anthropic.com/v1/oauth/token"
_GOOGLE_USERINFO_URL = "https://www.googleapis.com/oauth2/v1/userinfo?alt=json"
_OPENAI_ACCOUNTS_CHECK_URL = (
    "https://chatgpt.com/backend-api/accounts/check/v4-2023-04-27"
)


def _coerce_proxy_url(proxy_config: dict[str, Any] | None) -> str | None:
    """为 tls-client 构建可用的代理 URL（best-effort）。

    说明：
    - core 层不解析 ProxyNode（node_id）模式，避免 core→services 反向依赖。
    - 仅支持手工 URL 模式：{url, username, password, enabled}。
    - node_id / tunnel 等复杂模式由 httpx 路径（HTTPClientPool）处理。
    """
    if not proxy_config or not proxy_config.get("enabled", True):
        return None

    raw_url = proxy_config.get("url")
    if not isinstance(raw_url, str) or not raw_url.strip():
        return None

    proxy_url = raw_url.strip()
    username = proxy_config.get("username")
    password = proxy_config.get("password")
    if isinstance(username, str) and username.strip():
        return _inject_auth_into_url(
            proxy_url, username.strip(), str(password) if password else None
        )
    return proxy_url


def _inject_auth_into_url(url: str, username: str, password: str | None = None) -> str:
    """将用户名密码注入 URL（仅用于 tls-client 同步请求）。"""
    try:
        parsed = urlsplit(url)
        if not parsed.scheme or not parsed.hostname:
            return url

        encoded_username = quote(username, safe="")
        encoded_password = quote(password, safe="") if password else ""
        host_part = parsed.hostname
        if parsed.port:
            host_part = f"{host_part}:{parsed.port}"
        auth_part = (
            f"{encoded_username}:{encoded_password}"
            if encoded_password
            else encoded_username
        )
        netloc = f"{auth_part}@{host_part}"

        return urlunsplit(
            (parsed.scheme, netloc, parsed.path, parsed.query, parsed.fragment)
        )
    except Exception:
        return url


def _proxy_display(proxy_config: dict[str, Any] | None) -> str | None:
    """生成用于日志输出的 proxy 摘要（不泄露认证信息）。"""
    if not proxy_config or not proxy_config.get("enabled", True):
        return None

    node_id = proxy_config.get("node_id")
    if isinstance(node_id, str) and node_id.strip():
        return f"node_id:{node_id.strip()}"

    proxy_url = _coerce_proxy_url(proxy_config)
    if not proxy_url:
        return None

    try:
        parts = urlsplit(proxy_url)
        host = parts.hostname or ""
        if parts.port:
            host = f"{host}:{parts.port}"
        # 仅保留 scheme + host + path，移除 userinfo/query/fragment
        return urlunsplit((parts.scheme, host, parts.path, "", ""))
    except Exception:
        return "<invalid_proxy>"


def _redact_url(url: str) -> str:
    """Remove query and fragment to avoid leaking secrets in logs."""
    try:
        parts = urlsplit(url)
        return urlunsplit((parts.scheme, parts.netloc, parts.path, "", ""))
    except Exception:
        return "<invalid_url>"


def _format_exc_chain(e: BaseException) -> str:
    parts: list[str] = []
    cur: BaseException | None = e
    while cur is not None:
        parts.append(f"{type(cur).__name__}: {cur}")
        nxt = getattr(cur, "__cause__", None) or getattr(cur, "__context__", None)
        cur = nxt if isinstance(nxt, BaseException) else None
    return " <- ".join(parts)


async def _httpx_post(
    url: str,
    *,
    headers: dict[str, str] | None,
    data: Any,
    json_body: Any,
    proxy_config: dict[str, Any] | None,
    timeout_seconds: float,
) -> httpx.Response:
    client = await HTTPClientPool.get_proxy_client(proxy_config)
    proxy_url = _proxy_display(proxy_config)
    safe_url = _redact_url(url)

    last_exc: Exception | None = None
    # Only retry connection-level transient failures.
    for attempt in range(2):
        if attempt:
            await asyncio.sleep(0.25 * attempt)
        try:
            return await client.post(
                url,
                headers=headers,
                data=data,
                json=json_body,
                timeout=timeout_seconds,
            )
        except httpx.ConnectError as e:
            last_exc = e
            logger.warning(
                "OAuth token POST connect error (attempt={}/2) url={} host={} proxy={} err_chain={} err={!r}",
                attempt + 1,
                safe_url,
                urlsplit(url).netloc,
                proxy_url,
                _format_exc_chain(e),
                e,
            )
            if attempt == 0:
                continue
            raise
        except httpx.TimeoutException as e:
            last_exc = e
            logger.warning(
                "OAuth token POST timeout (attempt={}/2) url={} host={} proxy={} err_chain={} err={!r}",
                attempt + 1,
                safe_url,
                urlsplit(url).netloc,
                proxy_url,
                _format_exc_chain(e),
                e,
            )
            # Retry only the first time for timeouts.
            if attempt == 0:
                continue
            raise
        except httpx.RequestError as e:
            # Other request-level errors (proxy, TLS, etc). Log and re-raise.
            last_exc = e
            logger.error(
                "OAuth token POST request error url={} host={} proxy={} err_chain={} err={!r}",
                safe_url,
                urlsplit(url).netloc,
                proxy_url,
                _format_exc_chain(e),
                e,
            )
            raise

    # Should not be reachable.
    assert last_exc is not None
    raise last_exc


def _tls_client_post_sync(
    url: str,
    *,
    headers: dict[str, str] | None,
    data: Any,
    json_body: Any,
    proxy_url: str | None,
    timeout_seconds: float,
) -> tuple[int, dict[str, str], str]:
    # tls-client is optional at runtime; import only when needed.
    import tls_client  # pyright: ignore[reportMissingImports]

    session = tls_client.Session(
        client_identifier="firefox_120",
        random_tls_extension_order=True,
    )

    if proxy_url:
        session.proxies = {"http": proxy_url, "https": proxy_url}

    # tls-client uses a requests-like API.
    resp = session.post(
        url,
        headers=headers or {},
        data=data,
        json=json_body,
        timeout_seconds=timeout_seconds,
    )

    # Normalize output
    status_code = int(getattr(resp, "status_code", 0))
    text = str(getattr(resp, "text", ""))
    resp_headers = dict(getattr(resp, "headers", {}) or {})
    return status_code, resp_headers, text


async def post_oauth_token(
    *,
    provider_type: str,
    token_url: str,
    headers: dict[str, str] | None,
    data: Any = None,
    json_body: Any = None,
    proxy_config: dict[str, Any] | None = None,
    timeout_seconds: float = 30.0,
) -> httpx.Response:
    """POST to token endpoint.

    Claude Code + Anthropic token URL will try tls-client (Firefox TLS fingerprint) first.
    If tls-client is unavailable or fails, fall back to httpx.

    IMPORTANT: Never log secrets (tokens, secrets). This function only logs generic errors.
    """

    if provider_type == ProviderType.CLAUDE_CODE and token_url == _ANTHROPIC_TOKEN_URL:
        proxy_url = _coerce_proxy_url(proxy_config)
        try:
            status_code, resp_headers, text = await asyncio.to_thread(
                _tls_client_post_sync,
                token_url,
                headers=headers,
                data=data,
                json_body=json_body,
                proxy_url=proxy_url,
                timeout_seconds=timeout_seconds,
            )
            return httpx.Response(
                status_code=status_code,
                headers=resp_headers,
                content=text.encode("utf-8", errors="replace"),
                request=httpx.Request("POST", token_url),
            )
        except Exception as e:
            logger.warning(
                "Claude OAuth token request via tls-client failed; fallback to httpx. err={!r}",
                e,
            )

    return await _httpx_post(
        token_url,
        headers=headers,
        data=data,
        json_body=json_body,
        proxy_config=proxy_config,
        timeout_seconds=timeout_seconds,
    )


def _as_non_empty_str(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    text = value.strip()
    return text or None


def _first_non_empty_str(values: list[Any]) -> str | None:
    for value in values:
        text = _as_non_empty_str(value)
        if text:
            return text
    return None


def _decode_unverified_jwt_payload(token: str) -> dict[str, Any] | None:
    try:
        claims = jwt.decode(
            token,
            options={
                "verify_signature": False,
                "verify_aud": False,
            },
        )
    except Exception:
        return None
    return claims if isinstance(claims, dict) else None


def _extract_codex_fields_from_claims(claims: dict[str, Any]) -> dict[str, Any]:
    auth_info = claims.get("https://api.openai.com/auth")
    auth = auth_info if isinstance(auth_info, dict) else {}

    result: dict[str, Any] = {}

    email = _first_non_empty_str(
        [
            claims.get("email"),
            auth.get("email"),
        ]
    )
    if email:
        result["email"] = email

    account_id = _first_non_empty_str(
        [
            auth.get("chatgpt_account_id"),
            auth.get("chatgptAccountId"),
            auth.get("account_id"),
            auth.get("accountId"),
            claims.get("chatgpt_account_id"),
            claims.get("chatgptAccountId"),
            claims.get("account_id"),
            claims.get("accountId"),
        ]
    )
    if account_id:
        result["account_id"] = account_id

    account_user_id = _first_non_empty_str(
        [
            auth.get("chatgpt_account_user_id"),
            auth.get("chatgptAccountUserId"),
            auth.get("account_user_id"),
            auth.get("accountUserId"),
            claims.get("chatgpt_account_user_id"),
            claims.get("chatgptAccountUserId"),
            claims.get("account_user_id"),
            claims.get("accountUserId"),
        ]
    )
    if account_user_id:
        result["account_user_id"] = account_user_id

    plan_type = _first_non_empty_str(
        [
            auth.get("chatgpt_plan_type"),
            auth.get("chatgptPlanType"),
            auth.get("plan_type"),
            auth.get("planType"),
            claims.get("chatgpt_plan_type"),
            claims.get("chatgptPlanType"),
            claims.get("plan_type"),
            claims.get("planType"),
        ]
    )
    if plan_type:
        result["plan_type"] = plan_type

    user_id = _first_non_empty_str(
        [
            auth.get("chatgpt_user_id"),
            auth.get("chatgptUserId"),
            auth.get("user_id"),
            auth.get("userId"),
            claims.get("chatgpt_user_id"),
            claims.get("chatgptUserId"),
            claims.get("user_id"),
            claims.get("userId"),
            claims.get("sub"),
        ]
    )
    if user_id:
        result["user_id"] = user_id

    organizations = auth.get("organizations")
    if isinstance(organizations, list) and organizations:
        result["organizations"] = organizations

    return result


def parse_codex_id_token(id_token: Any) -> dict[str, Any]:
    """Parse Codex token payload without signature verification.

    Supports:
    - JWT string (typical id_token / sometimes access_token)
    - JSON string containing claims
    - Already-decoded dict payload
    """

    claims: dict[str, Any] | None = None
    if isinstance(id_token, dict):
        claims = id_token
    else:
        token_text = _as_non_empty_str(id_token)
        if not token_text:
            return {}

        if token_text.startswith("{"):
            try:
                payload = json.loads(token_text)
            except Exception:
                payload = None
            if isinstance(payload, dict):
                claims = payload

        if claims is None:
            claims = _decode_unverified_jwt_payload(token_text)

    if not isinstance(claims, dict):
        return {}
    return _extract_codex_fields_from_claims(claims)


async def fetch_google_email(
    access_token: str,
    *,
    proxy_config: dict[str, Any] | None = None,
    timeout_seconds: float = 10.0,
) -> str | None:
    if not access_token:
        return None

    client = await HTTPClientPool.get_proxy_client(proxy_config)
    try:
        resp = await client.get(
            _GOOGLE_USERINFO_URL,
            headers={
                "Authorization": f"Bearer {access_token}",
                "Accept": "application/json",
            },
            timeout=timeout_seconds,
        )
        if resp.status_code < 200 or resp.status_code >= 300:
            return None
        data = resp.json()
        email = data.get("email")
        if isinstance(email, str) and email:
            return email
        return None
    except Exception:
        return None


def _extract_openai_account_name(payload: Any, account_id: str) -> str | None:
    if not isinstance(payload, dict):
        return None

    accounts = payload.get("accounts")
    if isinstance(accounts, dict):
        direct = accounts.get(account_id)
        if isinstance(direct, dict):
            account = direct.get("account")
            account_info = account if isinstance(account, dict) else direct
            direct_name = _as_non_empty_str(account_info.get("name"))
            if direct_name:
                return direct_name

        for item in accounts.values():
            if not isinstance(item, dict):
                continue
            account = item.get("account")
            account_info = account if isinstance(account, dict) else item
            matched_account_id = _first_non_empty_str(
                [
                    account_info.get("id"),
                    account_info.get("account_id"),
                    account_info.get("accountId"),
                    item.get("id"),
                    item.get("account_id"),
                    item.get("accountId"),
                ]
            )
            if matched_account_id != account_id:
                continue
            matched_name = _as_non_empty_str(account_info.get("name"))
            if matched_name:
                return matched_name

    return None


async def fetch_openai_account_name(
    access_token: str,
    account_id: str,
    *,
    proxy_config: dict[str, Any] | None = None,
    timeout_seconds: float = 10.0,
) -> str | None:
    if not access_token or not account_id:
        return None

    proxy_url = _coerce_proxy_url(proxy_config)
    if not proxy_url and proxy_config:
        try:
            from src.services.proxy_node.resolver import build_proxy_url_async

            proxy_url = await build_proxy_url_async(proxy_config)
        except Exception:
            proxy_url = None

    try:
        from curl_cffi.requests import AsyncSession  # pyright: ignore[reportMissingImports]

        session = AsyncSession(
            impersonate="chrome110",
            proxies={"http": proxy_url, "https": proxy_url} if proxy_url else None,
            timeout=timeout_seconds,
            verify=False if proxy_url else True,
        )
        try:
            headers = {
                "Authorization": f"Bearer {access_token}",
                "Accept": "*/*",
                "Accept-Language": "en-US,en;q=0.9",
                "Referer": "https://chatgpt.com/",
                "Origin": "https://chatgpt.com",
                "Connection": "keep-alive",
            }
            for attempt in range(3):
                if attempt:
                    await asyncio.sleep(
                        [1.0, 2.0][attempt - 1] + random.uniform(0.5, 1.5)
                    )
                resp = await session.get(_OPENAI_ACCOUNTS_CHECK_URL, headers=headers)
                if 200 <= resp.status_code < 300:
                    return _extract_openai_account_name(resp.json(), account_id)
        finally:
            await session.close()
    except Exception:
        pass

    client = await HTTPClientPool.get_proxy_client(proxy_config)
    try:
        resp = await client.get(
            _OPENAI_ACCOUNTS_CHECK_URL,
            headers={
                "Authorization": f"Bearer {access_token}",
                "Accept": "application/json",
            },
            timeout=timeout_seconds,
        )
        if resp.status_code < 200 or resp.status_code >= 300:
            return None
        return _extract_openai_account_name(resp.json(), account_id)
    except Exception:
        return None


def extract_claude_email_from_token_response(token: dict[str, Any]) -> str | None:
    # CLIProxyAPI expects: { account: { email_address: ... } }
    try:
        account = token.get("account")
        if isinstance(account, dict):
            email = account.get("email_address")
            if isinstance(email, str) and email:
                return email
    except Exception:
        pass
    return None


# ---------------------------------------------------------------------------
# Auth Enricher Registry
# ---------------------------------------------------------------------------

AuthEnricherFn = Callable[
    [dict[str, Any], dict[str, Any], str, dict[str, Any] | None],
    Awaitable[dict[str, Any]],
]
_auth_enrichers: dict[str, AuthEnricherFn] = {}


def register_auth_enricher(provider_type: str, enricher: AuthEnricherFn) -> None:
    """注册 provider 特有的 auth_config enrichment hook。"""
    from src.core.provider_types import normalize_provider_type

    _auth_enrichers[normalize_provider_type(provider_type)] = enricher


async def _enrich_claude_code(
    auth_config: dict[str, Any],
    token_response: dict[str, Any],
    access_token: str,
    proxy_config: dict[str, Any] | None,
) -> dict[str, Any]:
    email = extract_claude_email_from_token_response(token_response)
    if email:
        auth_config["email"] = email
    return auth_config


async def _enrich_gemini_cli(
    auth_config: dict[str, Any],
    token_response: dict[str, Any],
    access_token: str,
    proxy_config: dict[str, Any] | None,
) -> dict[str, Any]:
    if not auth_config.get("email"):
        email = await fetch_google_email(
            access_token,
            proxy_config=proxy_config,
            timeout_seconds=10.0,
        )
        if email:
            auth_config["email"] = email
    return auth_config


def _bootstrap_auth_enrichers() -> None:
    # 简单的内置 enrichers 直接注册（兜底，会被 plugin.register_all() 覆盖）
    register_auth_enricher("claude_code", _enrich_claude_code)
    register_auth_enricher("gemini_cli", _enrich_gemini_cli)


_bootstrap_auth_enrichers()


async def enrich_auth_config(
    *,
    provider_type: str,
    auth_config: dict[str, Any],
    token_response: dict[str, Any],
    access_token: str,
    proxy_config: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Enrich auth_config with non-secret metadata (email/account_id).

    各 provider 的 enrichment 逻辑通过 register_auth_enricher 注册。
    为支持按需 bootstrap，这里会尝试按 provider_type 触发插件注册。
    """
    from src.core.provider_types import normalize_provider_type
    from src.services.provider.envelope import ensure_providers_bootstrapped

    pt = normalize_provider_type(provider_type)
    ensure_providers_bootstrapped(provider_types=[pt] if pt else None)
    enricher = _auth_enrichers.get(pt)
    if enricher:
        return await enricher(auth_config, token_response, access_token, proxy_config)
    return auth_config


def normalize_oauth_organizations(raw: Any) -> list[dict[str, Any]]:
    """Normalize raw organizations list from OAuth auth_config into a clean list of dicts."""
    if not isinstance(raw, list):
        return []

    result: list[dict[str, Any]] = []
    for item in raw:
        if not isinstance(item, dict):
            continue

        normalized: dict[str, Any] = {}
        org_id = item.get("id")
        if isinstance(org_id, str) and org_id.strip():
            normalized["id"] = org_id.strip()

        title = item.get("title")
        if isinstance(title, str) and title.strip():
            normalized["title"] = title.strip()

        role = item.get("role")
        if isinstance(role, str) and role.strip():
            normalized["role"] = role.strip()

        if "is_default" in item:
            normalized["is_default"] = bool(item.get("is_default"))

        if normalized:
            result.append(normalized)

    return result
