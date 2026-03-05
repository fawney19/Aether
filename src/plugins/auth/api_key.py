"""
API Key认证插件
支持从header中提取API Key进行认证
"""

from __future__ import annotations

from fastapi import Request
from sqlalchemy.orm import Session

from src.core.logger import logger
from src.services.auth.service import AuthService
from src.services.usage.service import UsageService
from src.services.wallet import WalletService

from .base import AuthContext, AuthPlugin


class ApiKeyAuthPlugin(AuthPlugin):
    """
    API Key认证插件
    支持从x-api-key header或Authorization Bearer token中提取API Key
    """

    def __init__(self) -> None:
        super().__init__(name="api_key", priority=10)

    def get_credentials(self, request: Request) -> str | None:
        """
        从请求头中提取API Key

        支持两种方式：
        1. x-api-key: <key>
        2. Authorization: Bearer <key>
        """
        # 尝试从x-api-key header获取
        api_key = request.headers.get("x-api-key")
        if api_key:
            return api_key

        # 尝试从Authorization header获取
        auth_header = request.headers.get("authorization")
        if auth_header and auth_header.startswith("Bearer "):
            return auth_header.replace("Bearer ", "")

        return None

    async def authenticate(self, request: Request, db: Session) -> AuthContext | None:
        """
        使用API Key进行认证
        """
        # 提取API Key
        api_key = self.get_credentials(request)
        if not api_key:
            logger.debug("未找到API Key凭据")
            return None

        # 认证API Key
        auth_result = AuthService.authenticate_api_key(db, api_key)
        if not auth_result:
            logger.warning("API Key认证失败")
            return None

        user, api_key_obj = auth_result

        # 检查用户或独立 Key 的钱包余额可用性
        access_ok, message = UsageService.check_request_balance(db, user, api_key=api_key_obj)
        billing_wallet = (
            WalletService.get_wallet(db, api_key_id=api_key_obj.id)
            if api_key_obj.is_standalone
            else WalletService.get_wallet(db, user_id=user.id)
        )

        # 创建认证上下文
        auth_context = AuthContext(
            user_id=user.id,
            user_name=user.username,
            api_key_id=api_key_obj.id,
            api_key_name=api_key_obj.name if hasattr(api_key_obj, "name") else None,
            permissions={
                "can_use_api": access_ok,
                "is_admin": user.is_admin if hasattr(user, "is_admin") else False,
                "is_standalone_key": api_key_obj.is_standalone,  # 标记是否为独立余额Key
            },
            billing_info={
                "billing": WalletService.serialize_wallet_summary(billing_wallet),
                "balance_ok": access_ok,
                "message": message,
            },
            metadata={
                "auth_method": "api_key",
                "client_ip": request.client.host if request.client else "unknown",
                "is_standalone": api_key_obj.is_standalone,  # 在metadata中也保存一份
            },
        )

        logger.info("API Key认证成功")

        return auth_context
