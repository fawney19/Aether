"""
认证服务测试

测试 AuthService 的核心功能：
- JWT Token 创建和验证
- 用户登录认证
- API Key 认证
"""

from datetime import datetime, timedelta, timezone
from unittest.mock import AsyncMock, MagicMock, patch

import jwt
import pytest

from src.core.enums import AuthSource
from src.models.database import UserRole
from src.services.auth.service import (
    JWT_ALGORITHM,
    JWT_EXPIRATION_HOURS,
    JWT_SECRET_KEY,
    AuthService,
)


class TestJWTTokenCreation:
    """测试 JWT Token 创建"""

    def test_create_access_token_contains_required_fields(self) -> None:
        """测试访问令牌包含必要字段"""
        data = {"sub": "user123", "email": "test@example.com"}
        token = AuthService.create_access_token(data)

        # 解码验证
        payload = jwt.decode(token, JWT_SECRET_KEY, algorithms=[JWT_ALGORITHM])

        assert payload["sub"] == "user123"
        assert payload["email"] == "test@example.com"
        assert payload["type"] == "access"
        assert "exp" in payload

    def test_create_access_token_expiration(self) -> None:
        """测试访问令牌过期时间正确"""
        data = {"sub": "user123"}
        token = AuthService.create_access_token(data)

        payload = jwt.decode(token, JWT_SECRET_KEY, algorithms=[JWT_ALGORITHM])

        # 验证过期时间在预期范围内（允许1分钟误差）
        exp_time = datetime.fromtimestamp(payload["exp"], tz=timezone.utc)
        expected_exp = datetime.now(timezone.utc) + timedelta(hours=JWT_EXPIRATION_HOURS)

        assert abs((exp_time - expected_exp).total_seconds()) < 60

    def test_create_refresh_token_type(self) -> None:
        """测试刷新令牌类型正确"""
        data = {"sub": "user123"}
        token = AuthService.create_refresh_token(data)

        payload = jwt.decode(token, JWT_SECRET_KEY, algorithms=[JWT_ALGORITHM])

        assert payload["type"] == "refresh"

    def test_create_refresh_token_longer_expiration(self) -> None:
        """测试刷新令牌过期时间更长"""
        data = {"sub": "user123"}
        access_token = AuthService.create_access_token(data)
        refresh_token = AuthService.create_refresh_token(data)

        access_payload = jwt.decode(access_token, JWT_SECRET_KEY, algorithms=[JWT_ALGORITHM])
        refresh_payload = jwt.decode(refresh_token, JWT_SECRET_KEY, algorithms=[JWT_ALGORITHM])

        # 刷新令牌应该比访问令牌过期时间更长
        assert refresh_payload["exp"] > access_payload["exp"]


class TestJWTTokenVerification:
    """测试 JWT Token 验证"""

    @pytest.mark.asyncio
    async def test_verify_valid_access_token(self) -> None:
        """测试验证有效的访问令牌"""
        data = {"sub": "user123", "email": "test@example.com"}
        token = AuthService.create_access_token(data)

        with patch(
            "src.services.auth.service.JWTBlacklistService.is_blacklisted",
            new_callable=AsyncMock,
            return_value=False,
        ):
            payload = await AuthService.verify_token(token, token_type="access")

        assert payload["sub"] == "user123"
        assert payload["type"] == "access"

    @pytest.mark.asyncio
    async def test_verify_expired_token_raises_error(self) -> None:
        """测试验证过期令牌抛出异常"""
        # 创建一个已过期的 token
        data: dict[str, str | datetime] = {"sub": "user123", "type": "access"}
        expire = datetime.now(timezone.utc) - timedelta(hours=1)
        data["exp"] = expire
        expired_token = jwt.encode(data, JWT_SECRET_KEY, algorithm=JWT_ALGORITHM)

        from fastapi import HTTPException

        with pytest.raises(HTTPException) as exc_info:
            await AuthService.verify_token(expired_token)

        assert exc_info.value.status_code == 401
        assert "过期" in exc_info.value.detail

    @pytest.mark.asyncio
    async def test_verify_invalid_token_raises_error(self) -> None:
        """测试验证无效令牌抛出异常"""
        from fastapi import HTTPException

        with pytest.raises(HTTPException) as exc_info:
            await AuthService.verify_token("invalid.token.here")

        assert exc_info.value.status_code == 401

    @pytest.mark.asyncio
    async def test_verify_wrong_token_type_raises_error(self) -> None:
        """测试令牌类型不匹配抛出异常"""
        data = {"sub": "user123"}
        refresh_token = AuthService.create_refresh_token(data)

        from fastapi import HTTPException

        with patch(
            "src.services.auth.service.JWTBlacklistService.is_blacklisted",
            new_callable=AsyncMock,
            return_value=False,
        ):
            with pytest.raises(HTTPException) as exc_info:
                await AuthService.verify_token(refresh_token, token_type="access")

        assert exc_info.value.status_code == 401
        assert "类型错误" in exc_info.value.detail

    @pytest.mark.asyncio
    async def test_verify_blacklisted_token_raises_error(self) -> None:
        """测试已撤销的令牌抛出异常"""
        data = {"sub": "user123"}
        token = AuthService.create_access_token(data)

        from fastapi import HTTPException

        with patch(
            "src.services.auth.service.JWTBlacklistService.is_blacklisted",
            new_callable=AsyncMock,
            return_value=True,
        ):
            with pytest.raises(HTTPException) as exc_info:
                await AuthService.verify_token(token)

        assert exc_info.value.status_code == 401
        assert "撤销" in exc_info.value.detail


class TestUserAuthentication:
    """测试用户登录认证"""

    @pytest.mark.asyncio
    async def test_authenticate_user_success(self) -> None:
        """测试用户登录成功"""
        # Mock 数据库和用户对象
        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_user.email = "test@example.com"
        mock_user.is_deleted = False
        mock_user.is_active = True
        mock_user.auth_source = AuthSource.LOCAL
        mock_user.role = UserRole.USER
        mock_user.verify_password.return_value = True

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with patch(
            "src.services.auth.service.UserCacheService.invalidate_user_cache",
            new_callable=AsyncMock,
        ):
            result = await AuthService.authenticate_user(mock_db, "test@example.com", "password123")

        assert result == mock_user
        mock_user.verify_password.assert_called_once_with("password123")
        mock_db.commit.assert_called_once()

    @pytest.mark.asyncio
    async def test_authenticate_user_not_found(self) -> None:
        """测试用户不存在"""
        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = None

        result = await AuthService.authenticate_user(mock_db, "nonexistent@example.com", "password")

        assert result is None

    @pytest.mark.asyncio
    async def test_authenticate_user_wrong_password(self) -> None:
        """测试密码错误"""
        mock_user = MagicMock()
        mock_user.email = "test@example.com"
        mock_user.is_deleted = False
        mock_user.is_active = True
        mock_user.auth_source = AuthSource.LOCAL
        mock_user.role = UserRole.USER
        mock_user.verify_password.return_value = False

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        result = await AuthService.authenticate_user(mock_db, "test@example.com", "wrongpassword")

        assert result is None

    @pytest.mark.asyncio
    async def test_authenticate_user_inactive(self) -> None:
        """测试用户已禁用"""
        mock_user = MagicMock()
        mock_user.email = "test@example.com"
        mock_user.is_deleted = False
        mock_user.is_active = False
        mock_user.auth_source = AuthSource.LOCAL
        mock_user.role = UserRole.USER
        mock_user.verify_password.return_value = True

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        result = await AuthService.authenticate_user(mock_db, "test@example.com", "password123")

        assert result is None


class TestAPIKeyAuthentication:
    """测试 API Key 认证"""

    def test_authenticate_api_key_success(self) -> None:
        """测试 API Key 认证成功"""
        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_user.email = "test@example.com"
        mock_user.is_active = True
        mock_user.is_deleted = False

        mock_api_key = MagicMock()
        mock_api_key.is_active = True
        mock_api_key.is_locked = False
        mock_api_key.expires_at = None
        mock_api_key.user = mock_user
        mock_api_key.balance_used_usd = 0.0

        mock_db = MagicMock()
        mock_db.query.return_value.options.return_value.filter.return_value.first.return_value = (
            mock_api_key
        )

        with patch("src.services.auth.service.ApiKey.hash_key", return_value="hashed_key"):
            with patch(
                "src.services.auth.service.ApiKeyService.check_balance",
                return_value=(True, 100.0),
            ):
                result = AuthService.authenticate_api_key(mock_db, "sk-test-key")

        assert result is not None
        assert result[0] == mock_user
        assert result[1] == mock_api_key

    def test_authenticate_api_key_last_used_commit_disables_expire_on_commit(self) -> None:
        """当需要更新 last_used_at 时，应临时关闭 expire_on_commit 以避免重复查询。"""
        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_user.email = "test@example.com"
        mock_user.is_active = True
        mock_user.is_deleted = False

        mock_api_key = MagicMock()
        mock_api_key.id = "key-123"
        mock_api_key.is_active = True
        mock_api_key.is_locked = False
        mock_api_key.expires_at = None
        mock_api_key.user = mock_user
        mock_api_key.balance_used_usd = 0.0

        mock_db = MagicMock()
        mock_db.expire_on_commit = True

        def _commit_side_effect() -> None:
            assert mock_db.expire_on_commit is False

        mock_db.commit.side_effect = _commit_side_effect
        mock_db.query.return_value.options.return_value.filter.return_value.first.return_value = (
            mock_api_key
        )

        with patch("src.services.auth.service._should_update_last_used", return_value=True):
            with patch("src.services.auth.service.ApiKey.hash_key", return_value="hashed_key"):
                with patch(
                    "src.services.auth.service.ApiKeyService.check_balance",
                    return_value=(True, 100.0),
                ):
                    result = AuthService.authenticate_api_key(mock_db, "sk-test-key")

        assert result is not None
        assert mock_db.expire_on_commit is True
        mock_db.commit.assert_called_once()

    def test_authenticate_api_key_not_found(self) -> None:
        """测试 API Key 不存在"""
        mock_db = MagicMock()
        mock_db.query.return_value.options.return_value.filter.return_value.first.return_value = (
            None
        )

        with patch("src.services.auth.service.ApiKey.hash_key", return_value="hashed_key"):
            result = AuthService.authenticate_api_key(mock_db, "sk-invalid-key")

        assert result is None

    def test_authenticate_api_key_inactive(self) -> None:
        """测试 API Key 已禁用"""
        mock_api_key = MagicMock()
        mock_api_key.is_active = False
        mock_api_key.is_locked = False

        mock_db = MagicMock()
        mock_db.query.return_value.options.return_value.filter.return_value.first.return_value = (
            mock_api_key
        )

        with patch("src.services.auth.service.ApiKey.hash_key", return_value="hashed_key"):
            result = AuthService.authenticate_api_key(mock_db, "sk-inactive-key")

        assert result is None

    def test_authenticate_api_key_expired(self) -> None:
        """测试 API Key 已过期"""
        mock_api_key = MagicMock()
        mock_api_key.is_active = True
        mock_api_key.is_locked = False
        mock_api_key.expires_at = datetime.now(timezone.utc) - timedelta(days=1)

        mock_db = MagicMock()
        mock_db.query.return_value.options.return_value.filter.return_value.first.return_value = (
            mock_api_key
        )

        with patch("src.services.auth.service.ApiKey.hash_key", return_value="hashed_key"):
            result = AuthService.authenticate_api_key(mock_db, "sk-expired-key")

        assert result is None
