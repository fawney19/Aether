"""Vertex AI 认证处理。

支持两种认证方式：
- API Key: 通过 URL ?key= 查询参数认证，auth 层返回 None（由 transport hook 处理）
- Service Account: GCP SA JSON → JWT → Access Token → Bearer header
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, Any

from src.core.logger import logger
from src.core.provider_auth_types import ProviderAuthInfo

if TYPE_CHECKING:
    from src.models.database import ProviderAPIKey, ProviderEndpoint


async def get_vertex_ai_auth(
    endpoint: "ProviderEndpoint",
    key: "ProviderAPIKey",
) -> ProviderAuthInfo | None:
    """Vertex AI 认证统一入口。

    - auth_type = "api_key": 返回 None，API Key 认证通过 URL ?key= 参数处理
    - auth_type = "service_account": 调用 VertexAuthService 获取 Bearer token
    """
    auth_type = getattr(key, "auth_type", "api_key")

    if auth_type == "api_key":
        # API Key 认证不走 header，由 transport hook 在构建 URL 时附加 ?key=
        # 解密 auth_config 中可能的配置（如 model_format_mapping）
        decrypted_auth_config = _decrypt_auth_config(key)
        if decrypted_auth_config:
            return ProviderAuthInfo(
                auth_header="",
                auth_value="",
                decrypted_auth_config=decrypted_auth_config,
            )
        return None

    # service_account（以及向后兼容旧的 "vertex_ai" auth_type）
    return await _auth_service_account(key)


def _decrypt_auth_config(key: Any) -> dict[str, Any] | None:
    """解密 key.auth_config，返回 dict 或 None。"""
    from src.core.crypto import crypto_service

    encrypted = getattr(key, "auth_config", None)
    if not encrypted:
        return None
    try:
        if isinstance(encrypted, dict):
            return encrypted
        decrypted = crypto_service.decrypt(encrypted)
        result = json.loads(decrypted)
        return result if isinstance(result, dict) else None
    except Exception:
        return None


async def _auth_service_account(key: Any) -> ProviderAuthInfo:
    """Service Account 认证：SA JSON → JWT → Access Token。"""
    from src.core.crypto import crypto_service
    from src.core.exceptions import InvalidRequestException
    from src.core.vertex_auth import VertexAuthError, VertexAuthService

    try:
        # 优先从 auth_config 读取，兼容从 api_key 读取（过渡期）
        encrypted_auth_config = getattr(key, "auth_config", None)
        if encrypted_auth_config:
            if isinstance(encrypted_auth_config, dict):
                sa_json = encrypted_auth_config
            else:
                decrypted_config = crypto_service.decrypt(encrypted_auth_config)
                sa_json = json.loads(decrypted_config)
        else:
            # 兼容旧数据：从 api_key 读取
            decrypted_key = crypto_service.decrypt(key.api_key)
            if decrypted_key == "__placeholder__":
                raise InvalidRequestException("认证配置丢失，请重新添加该密钥。")
            sa_json = json.loads(decrypted_key)

        if not isinstance(sa_json, dict):
            raise InvalidRequestException("Service Account JSON 无效，请重新添加该密钥。")

        # 获取 Access Token（注入代理配置）
        from src.services.proxy_node.resolver import build_proxy_client_kwargs

        service = VertexAuthService(sa_json)
        access_token = await service.get_access_token(
            httpx_client_kwargs=build_proxy_client_kwargs(timeout=30),
        )

        return ProviderAuthInfo(
            auth_header="Authorization",
            auth_value=f"Bearer {access_token}",
            decrypted_auth_config=sa_json,
        )
    except InvalidRequestException:
        raise
    except VertexAuthError as e:
        raise InvalidRequestException(f"Vertex AI 认证失败：{e}")
    except json.JSONDecodeError:
        raise InvalidRequestException("Service Account JSON 格式无效，请重新添加该密钥。")
    except Exception:
        raise InvalidRequestException("Vertex AI 认证失败，请检查 Key 的 auth_config")
