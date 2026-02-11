"""
Vertex AI Service Account 认证服务

用于处理 Google Service Account 凭证的 JWT 签名和 Access Token 获取。
Access Token 会被缓存，直到过期前 60 秒才刷新。
"""

from __future__ import annotations

import time
from collections import OrderedDict

import httpx
import jwt

from src.core.logger import logger
from src.utils import json_helper as json


class VertexAuthError(Exception):
    """Vertex AI 认证错误"""

    pass


def _mask_email(email: str) -> str:
    """脱敏邮箱地址，如 foo@bar.iam.gserviceaccount.com -> foo@***.com"""
    if "@" not in email:
        return email[:8] + "***" if len(email) > 8 else "***"
    local, domain = email.rsplit("@", 1)
    # 保留 local 部分前几个字符和域名后缀
    masked_local = local[:6] + "***" if len(local) > 6 else local
    parts = domain.rsplit(".", 1)
    suffix = f".{parts[-1]}" if len(parts) > 1 else ""
    return f"{masked_local}@***{suffix}"


class VertexAuthService:
    """
    Vertex AI Service Account 认证服务

    用于将 Service Account JSON 凭证转换为 Access Token。

    使用方式：
        service = VertexAuthService(service_account_json)
        token = await service.get_access_token()
        project_id = service.project_id
        # 使用 token 和 project_id 构建请求
    """

    # Token 缓存：使用 OrderedDict 实现 LRU
    # key = client_email, value = (token, expires_at)
    _token_cache: OrderedDict[str, tuple[str, float]] = OrderedDict()
    _cache_max_size: int = 100  # 最多缓存 100 个 Service Account 的 Token

    # Token 请求端点
    TOKEN_URL = "https://oauth2.googleapis.com/token"

    # OAuth2 scope
    SCOPE = "https://www.googleapis.com/auth/cloud-platform"

    def __init__(self, service_account_json: str):
        """
        初始化认证服务

        Args:
            service_account_json: Service Account JSON 字符串或已解析的字典
        """
        if isinstance(service_account_json, str):
            try:
                self.sa_info = json.loads(service_account_json)
            except json.JSONDecodeError as e:
                raise VertexAuthError(f"Invalid Service Account JSON: {e}")
        else:
            self.sa_info = service_account_json

        # 验证必需字段
        required_fields = ["client_email", "private_key", "project_id"]
        missing = [f for f in required_fields if f not in self.sa_info]
        if missing:
            raise VertexAuthError(f"Service Account JSON missing required fields: {missing}")

        self.client_email = self.sa_info["client_email"]
        self.private_key = self.sa_info["private_key"]
        self.project_id = self.sa_info["project_id"]

    def _create_jwt(self) -> str:
        """
        创建签名的 JWT

        Returns:
            签名的 JWT 字符串
        """
        now = int(time.time())
        payload = {
            "iss": self.client_email,
            "sub": self.client_email,
            "aud": self.TOKEN_URL,
            "iat": now,
            "exp": now + 3600,  # 1 小时有效期
            "scope": self.SCOPE,
        }
        return jwt.encode(payload, self.private_key, algorithm="RS256")

    async def get_access_token(self) -> str:
        """
        获取 Access Token（带 LRU 缓存）

        如果缓存中有有效的 Token（距离过期超过 60 秒），直接返回。
        否则重新获取 Token。缓存采用 LRU 策略，超过 100 个条目时淘汰最旧的。

        Returns:
            Access Token 字符串

        Raises:
            VertexAuthError: 获取 Token 失败
        """
        # 检查缓存
        cache_key = self.client_email
        if cache_key in self._token_cache:
            token, expires_at = self._token_cache[cache_key]
            # 距离过期还有超过 60 秒，使用缓存
            if time.time() < expires_at - 60:
                # LRU: 移动到末尾（最近使用）
                self._token_cache.move_to_end(cache_key)
                return token

        # 获取新 Token
        try:
            signed_jwt = self._create_jwt()

            # 使用系统默认代理（Vertex AI token endpoint 是外部服务）
            from src.services.proxy_node.resolver import build_proxy_client_kwargs

            async with httpx.AsyncClient(**build_proxy_client_kwargs(timeout=30)) as client:
                resp = await client.post(
                    self.TOKEN_URL,
                    data={
                        "grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer",
                        "assertion": signed_jwt,
                    },
                )
                resp.raise_for_status()
                data = resp.json()

            access_token = data["access_token"]
            expires_in = data.get("expires_in", 3600)
            expires_at = time.time() + expires_in

            # 缓存 Token（LRU：新条目放在末尾）
            self._token_cache[cache_key] = (access_token, expires_at)
            self._token_cache.move_to_end(cache_key)

            # LRU 淘汰：超过最大缓存数时移除最旧的条目
            while len(self._token_cache) > self._cache_max_size:
                oldest_key = next(iter(self._token_cache))
                del self._token_cache[oldest_key]
                logger.debug(f"[VertexAuth] Evicted oldest cache entry: {_mask_email(oldest_key)}")

            logger.debug(
                f"[VertexAuth] Obtained access token for {_mask_email(self.client_email)}, "
                f"expires in {expires_in}s (cache size: {len(self._token_cache)})"
            )

            return access_token

        except httpx.HTTPStatusError as e:
            error_body = e.response.text[:500] if e.response.text else "(empty)"
            raise VertexAuthError(
                f"Failed to get access token: HTTP {e.response.status_code}: {error_body}"
            )
        except Exception as e:
            raise VertexAuthError(f"Failed to get access token: {e}")

    @classmethod
    def clear_cache(cls, client_email: str | None = None) -> None:
        """
        清除 Token 缓存

        Args:
            client_email: 指定要清除的账号，None 表示清除全部
        """
        if client_email:
            cls._token_cache.pop(client_email, None)
        else:
            cls._token_cache.clear()


async def get_vertex_access_token(service_account_json: str) -> tuple[str, str]:
    """
    便捷函数：获取 Vertex AI Access Token 和 Project ID

    Args:
        service_account_json: Service Account JSON 字符串

    Returns:
        (access_token, project_id) 元组
    """
    service = VertexAuthService(service_account_json)
    token = await service.get_access_token()
    return token, service.project_id
