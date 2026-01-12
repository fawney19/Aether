"""
API Pipeline 测试

测试 ApiRequestPipeline 的核心功能：
- 认证流程（API Key、JWT Token）
- 配额计算
- 审计日志记录
"""

import pytest
from datetime import datetime, timezone
from unittest.mock import AsyncMock, MagicMock, patch

from fastapi import HTTPException

from src.api.base.pipeline import ApiRequestPipeline
from src.core.enums import UserRole


class TestPipelineQuotaCalculation:
    """测试 Pipeline 配额计算"""

    @pytest.fixture
    def pipeline(self) -> ApiRequestPipeline:
        return ApiRequestPipeline()

    def test_calculate_quota_remaining_with_quota(self, pipeline: ApiRequestPipeline) -> None:
        """测试有配额限制时计算剩余配额"""
        mock_user = MagicMock()
        mock_user.quota_usd = 100.0
        mock_user.used_usd = 30.0

        remaining = pipeline._calculate_quota_remaining(mock_user)

        assert remaining == 70.0

    def test_calculate_quota_remaining_no_quota(self, pipeline: ApiRequestPipeline) -> None:
        """测试无配额限制时返回 None"""
        mock_user = MagicMock()
        mock_user.quota_usd = None
        mock_user.used_usd = 30.0

        remaining = pipeline._calculate_quota_remaining(mock_user)

        assert remaining is None

    def test_calculate_quota_remaining_negative_quota(self, pipeline: ApiRequestPipeline) -> None:
        """测试负配额时返回 None"""
        mock_user = MagicMock()
        mock_user.quota_usd = -1
        mock_user.used_usd = 0.0

        remaining = pipeline._calculate_quota_remaining(mock_user)

        assert remaining is None

    def test_calculate_quota_remaining_exceeded(self, pipeline: ApiRequestPipeline) -> None:
        """测试配额已超时返回 0"""
        mock_user = MagicMock()
        mock_user.quota_usd = 100.0
        mock_user.used_usd = 150.0

        remaining = pipeline._calculate_quota_remaining(mock_user)

        assert remaining == 0.0

    def test_calculate_quota_remaining_none_user(self, pipeline: ApiRequestPipeline) -> None:
        """测试用户为 None 时返回 None"""
        remaining = pipeline._calculate_quota_remaining(None)

        assert remaining is None


class TestPipelineAuditLogging:
    """测试 Pipeline 审计日志"""

    @pytest.fixture
    def pipeline(self) -> ApiRequestPipeline:
        return ApiRequestPipeline()

    def test_record_audit_event_success(self, pipeline: ApiRequestPipeline) -> None:
        """测试记录成功的审计事件"""
        mock_context = MagicMock()
        mock_context.db = MagicMock()
        mock_context.user = MagicMock()
        mock_context.user.id = "user-123"
        mock_context.api_key = MagicMock()
        mock_context.api_key.id = "key-123"
        mock_context.request_id = "req-123"
        mock_context.client_ip = "127.0.0.1"
        mock_context.user_agent = "test-agent"
        mock_context.request = MagicMock()
        mock_context.request.method = "POST"
        mock_context.request.url.path = "/v1/messages"
        mock_context.start_time = 1000.0

        mock_adapter = MagicMock()
        mock_adapter.name = "test-adapter"
        mock_adapter.audit_log_enabled = True
        mock_adapter.audit_success_event = None
        mock_adapter.audit_failure_event = None

        with patch.object(
            pipeline.audit_service,
            "log_event",
        ) as mock_log:
            with patch("time.time", return_value=1001.0):
                pipeline._record_audit_event(
                    mock_context, mock_adapter, success=True, status_code=200
                )

            mock_log.assert_called_once()
            call_kwargs = mock_log.call_args[1]
            assert call_kwargs["user_id"] == "user-123"
            assert call_kwargs["status_code"] == 200

    def test_record_audit_event_failure(self, pipeline: ApiRequestPipeline) -> None:
        """测试记录失败的审计事件"""
        mock_context = MagicMock()
        mock_context.db = MagicMock()
        mock_context.user = MagicMock()
        mock_context.user.id = "user-123"
        mock_context.api_key = MagicMock()
        mock_context.api_key.id = "key-123"
        mock_context.request_id = "req-123"
        mock_context.client_ip = "127.0.0.1"
        mock_context.user_agent = "test-agent"
        mock_context.request = MagicMock()
        mock_context.request.method = "POST"
        mock_context.request.url.path = "/v1/messages"
        mock_context.start_time = 1000.0

        mock_adapter = MagicMock()
        mock_adapter.name = "test-adapter"
        mock_adapter.audit_log_enabled = True
        mock_adapter.audit_success_event = None
        mock_adapter.audit_failure_event = None

        with patch.object(
            pipeline.audit_service,
            "log_event",
        ) as mock_log:
            with patch("time.time", return_value=1001.0):
                pipeline._record_audit_event(
                    mock_context, mock_adapter, success=False, status_code=500, error="Internal error"
                )

            mock_log.assert_called_once()
            call_kwargs = mock_log.call_args[1]
            assert call_kwargs["status_code"] == 500
            assert call_kwargs["error_message"] == "Internal error"

    def test_record_audit_event_no_db(self, pipeline: ApiRequestPipeline) -> None:
        """测试没有数据库会话时跳过审计"""
        mock_context = MagicMock()
        mock_context.db = None

        mock_adapter = MagicMock()
        mock_adapter.audit_log_enabled = True

        with patch.object(
            pipeline.audit_service,
            "log_event",
        ) as mock_log:
            # 不应该抛出异常
            pipeline._record_audit_event(mock_context, mock_adapter, success=True)

            # 不应该调用 log_event
            mock_log.assert_not_called()

    def test_record_audit_event_disabled(self, pipeline: ApiRequestPipeline) -> None:
        """测试审计日志被禁用时跳过"""
        mock_context = MagicMock()
        mock_context.db = MagicMock()

        mock_adapter = MagicMock()
        mock_adapter.audit_log_enabled = False

        with patch.object(
            pipeline.audit_service,
            "log_event",
        ) as mock_log:
            pipeline._record_audit_event(mock_context, mock_adapter, success=True)

            mock_log.assert_not_called()

    def test_record_audit_event_exception_handling(self, pipeline: ApiRequestPipeline) -> None:
        """测试审计日志异常不影响主流程"""
        mock_context = MagicMock()
        mock_context.db = MagicMock()
        mock_context.user = MagicMock()
        mock_context.user.id = "user-123"
        mock_context.api_key = MagicMock()
        mock_context.api_key.id = "key-123"
        mock_context.request_id = "req-123"
        mock_context.client_ip = "127.0.0.1"
        mock_context.user_agent = "test-agent"
        mock_context.request = MagicMock()
        mock_context.request.method = "POST"
        mock_context.request.url.path = "/v1/messages"
        mock_context.start_time = 1000.0

        mock_adapter = MagicMock()
        mock_adapter.name = "test-adapter"
        mock_adapter.audit_log_enabled = True
        mock_adapter.audit_success_event = None

        with patch.object(
            pipeline.audit_service,
            "log_event",
            side_effect=Exception("DB error"),
        ):
            with patch("time.time", return_value=1001.0):
                # 不应该抛出异常
                pipeline._record_audit_event(mock_context, mock_adapter, success=True)


class TestPipelineAuthentication:
    """测试 Pipeline 认证相关逻辑"""

    @pytest.fixture
    def pipeline(self) -> ApiRequestPipeline:
        return ApiRequestPipeline()

    def test_authenticate_client_missing_key(self, pipeline: ApiRequestPipeline) -> None:
        """测试缺少 API Key 时抛出异常"""
        mock_request = MagicMock()
        mock_request.headers = {}
        mock_request.url.path = "/v1/messages"
        mock_request.state = MagicMock()

        mock_db = MagicMock()

        mock_adapter = MagicMock()
        mock_adapter.extract_api_key = MagicMock(return_value=None)

        with pytest.raises(HTTPException) as exc_info:
            pipeline._authenticate_client(mock_request, mock_db, mock_adapter)

        assert exc_info.value.status_code == 401
        assert "API密钥" in exc_info.value.detail

    def test_authenticate_client_invalid_key(self, pipeline: ApiRequestPipeline) -> None:
        """测试无效的 API Key"""
        mock_request = MagicMock()
        mock_request.headers = {"Authorization": "Bearer sk-invalid"}
        mock_request.url.path = "/v1/messages"
        mock_request.state = MagicMock()

        mock_db = MagicMock()

        mock_adapter = MagicMock()
        mock_adapter.extract_api_key = MagicMock(return_value="sk-invalid")

        with patch.object(
            pipeline.auth_service,
            "authenticate_api_key",
            return_value=None,
        ):
            with pytest.raises(HTTPException) as exc_info:
                pipeline._authenticate_client(mock_request, mock_db, mock_adapter)

        assert exc_info.value.status_code == 401

    def test_authenticate_client_quota_exceeded(self, pipeline: ApiRequestPipeline) -> None:
        """测试配额超限时抛出异常"""
        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_user.quota_usd = 100.0
        mock_user.used_usd = 100.0

        mock_api_key = MagicMock()
        mock_api_key.id = "key-123"
        mock_api_key.is_standalone = False

        mock_request = MagicMock()
        mock_request.headers = {"Authorization": "Bearer sk-test"}
        mock_request.url.path = "/v1/messages"
        mock_request.state = MagicMock()

        mock_db = MagicMock()

        mock_adapter = MagicMock()
        mock_adapter.extract_api_key = MagicMock(return_value="sk-test")

        with patch.object(
            pipeline.auth_service,
            "authenticate_api_key",
            return_value=(mock_user, mock_api_key),
        ):
            with patch.object(
                pipeline.usage_service,
                "check_user_quota",
                return_value=(False, "配额不足"),
            ):
                from src.core.exceptions import QuotaExceededException

                with pytest.raises(QuotaExceededException):
                    pipeline._authenticate_client(mock_request, mock_db, mock_adapter)


class TestPipelineAdminAuth:
    """测试管理员认证"""

    @pytest.fixture
    def pipeline(self) -> ApiRequestPipeline:
        return ApiRequestPipeline()

    @pytest.mark.asyncio
    async def test_authenticate_admin_missing_token(self, pipeline: ApiRequestPipeline) -> None:
        """测试缺少管理员令牌"""
        mock_request = MagicMock()
        mock_request.headers = {}

        mock_db = MagicMock()

        with pytest.raises(HTTPException) as exc_info:
            await pipeline._authenticate_admin(mock_request, mock_db)

        assert exc_info.value.status_code == 401
        assert "管理员凭证" in exc_info.value.detail

    @pytest.mark.asyncio
    async def test_authenticate_admin_invalid_token(self, pipeline: ApiRequestPipeline) -> None:
        """测试无效的管理员令牌"""
        mock_request = MagicMock()
        mock_request.headers = {"authorization": "Bearer invalid-token"}

        mock_db = MagicMock()

        with patch.object(
            pipeline.auth_service,
            "verify_token",
            side_effect=HTTPException(status_code=401, detail="Invalid token"),
        ):
            with pytest.raises(HTTPException) as exc_info:
                await pipeline._authenticate_admin(mock_request, mock_db)

            assert exc_info.value.status_code == 401

    @pytest.mark.asyncio
    async def test_authenticate_admin_success(self, pipeline: ApiRequestPipeline) -> None:
        """测试管理员认证成功"""
        mock_user = MagicMock()
        mock_user.id = "admin-123"
        mock_user.is_active = True
        mock_user.role = UserRole.ADMIN
        mock_user.email = "admin@example.com"

        mock_request = MagicMock()
        mock_request.headers = {"authorization": "Bearer valid-token"}
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with patch.object(
            pipeline.auth_service,
            "verify_token",
            new_callable=AsyncMock,
            return_value={"user_id": "admin-123"},
        ):
            user, management_token = await pipeline._authenticate_admin(mock_request, mock_db)

        assert user == mock_user
        assert management_token is None
        assert mock_request.state.user_id == "admin-123"

    @pytest.mark.asyncio
    async def test_authenticate_admin_lowercase_bearer(self, pipeline: ApiRequestPipeline) -> None:
        """测试 bearer (小写) 前缀也能正确解析"""
        mock_user = MagicMock()
        mock_user.id = "admin-123"
        mock_user.is_active = True
        mock_user.role = UserRole.ADMIN
        mock_user.email = "admin@example.com"

        mock_request = MagicMock()
        mock_request.headers = {"authorization": "bearer valid-token"}
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with patch.object(
            pipeline.auth_service,
            "verify_token",
            new_callable=AsyncMock,
            return_value={"user_id": "admin-123"},
        ) as mock_verify:
            user, management_token = await pipeline._authenticate_admin(mock_request, mock_db)

        mock_verify.assert_awaited_once_with("valid-token", token_type="access")
        assert user == mock_user
        assert management_token is None


class TestPipelineUserAuth:
    """测试普通用户 JWT 认证"""

    @pytest.fixture
    def pipeline(self) -> ApiRequestPipeline:
        return ApiRequestPipeline()

    @pytest.mark.asyncio
    async def test_authenticate_user_lowercase_bearer(self, pipeline: ApiRequestPipeline) -> None:
        """测试 bearer (小写) 前缀也能正确解析"""
        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_user.is_active = True
        mock_user.email = "user@example.com"

        mock_request = MagicMock()
        mock_request.headers = {"authorization": "bearer valid-token"}
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with patch.object(
            pipeline.auth_service,
            "verify_token",
            new_callable=AsyncMock,
            return_value={"user_id": "user-123"},
        ) as mock_verify:
            user, management_token = await pipeline._authenticate_user(mock_request, mock_db)

        mock_verify.assert_awaited_once_with("valid-token", token_type="access")
        assert user == mock_user
        assert management_token is None
