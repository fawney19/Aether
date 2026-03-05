"""
JWT认证插件
支持JWT Bearer token认证
"""

from __future__ import annotations

import hashlib

from fastapi import Request
from sqlalchemy.orm import Session

from src.core.logger import logger
from src.models.database import User
from src.services.auth.service import AuthService
from src.services.wallet import WalletService

from .base import AuthContext, AuthPlugin


class JwtAuthPlugin(AuthPlugin):
    """
    JWT认证插件
    支持从Authorization Bearer header中提取JWT token进行认证
    """

    def __init__(self) -> None:
        super().__init__(name="jwt", priority=20)  # 高优先级，优先于API Key

    def get_credentials(self, request: Request) -> str | None:
        """
        从Authorization header中提取JWT token

        支持格式: Authorization: Bearer <token>
        """
        auth_header = request.headers.get("authorization")
        if auth_header and auth_header.startswith("Bearer "):
            return auth_header.replace("Bearer ", "")
        return None

    async def authenticate(self, request: Request, db: Session) -> AuthContext | None:
        """
        使用JWT token进行认证
        """
        # 提取JWT token
        token = self.get_credentials(request)
        if not token:
            logger.debug("未找到JWT token")
            return None

        token_fingerprint = hashlib.sha256(token.encode()).hexdigest()[:12]
        logger.info(f"JWT认证尝试 - 路径: {request.url.path}, token_fp={token_fingerprint}")

        try:
            # 验证JWT token
            payload = await AuthService.verify_token(token, token_type="access")
            logger.debug(f"JWT token验证成功, payload: {payload}")

            # 从payload中提取用户信息
            user_id = payload.get("user_id")
            if not user_id:
                logger.warning("JWT token中缺少用户ID")
                return None

            logger.debug(f"从JWT提取user_id: {user_id}, 类型: {type(user_id)}")

            # 从数据库获取用户信息
            user = db.query(User).filter(User.id == user_id).first()
            if not user:
                logger.warning(f"JWT认证失败 - 用户不存在: {user_id}")
                return None

            logger.debug(f"找到用户: {user.email}, is_active: {user.is_active}")

            if not user.is_active:
                logger.warning(f"JWT认证失败 - 用户已禁用: {user.email}")
                return None
            if user.is_deleted:
                logger.warning(f"JWT认证失败 - 用户已删除: {user.email}")
                return None

            if not AuthService.token_identity_matches_user(payload, user):
                logger.warning("JWT认证失败 - Token身份校验失败")
                return None

            wallet_access = WalletService.check_request_allowed(db, user=user, api_key=None)

            # 创建认证上下文
            auth_context = AuthContext(
                user_id=user.id,
                user_name=user.username,
                permissions={
                    "can_use_api": wallet_access.allowed,
                    "is_admin": user.role.value == "admin",
                },
                billing_info={
                    "billing": WalletService.serialize_wallet_summary(
                        WalletService.get_wallet(db, user_id=user.id)
                    ),
                    "balance_ok": wallet_access.allowed,
                    "message": wallet_access.message,
                },
                metadata={
                    "auth_method": "jwt",
                    "client_ip": request.client.host if request.client else "unknown",
                    "token_exp": payload.get("exp"),
                },
            )

            logger.info("JWT认证成功")

            return auth_context

        except Exception as e:
            logger.warning(f"JWT认证失败: {str(e)}")
            return None
