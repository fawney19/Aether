"""
API Pipeline 测试

测试 ApiRequestPipeline 的核心功能：
- 认证流程（API Key、JWT Token）
- 余额计算
- 审计日志记录
"""

from datetime import datetime, timezone
from types import SimpleNamespace
from typing import Any
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from fastapi import HTTPException

from src.api.base.adapter import ApiMode
from src.api.base.pipeline import ApiRequestPipeline
from src.core.enums import UserRole
from src.core.modules.hooks import AUTH_TOKEN_PREFIX_AUTHENTICATORS
from src.services.rate_limit.user_rpm_limiter import RpmCheckResult


class TestPipelineBalanceCalculation:
    """Balance calculation tests for Pipeline."""

    @pytest.fixture
    def pipeline(self) -> ApiRequestPipeline:
        return ApiRequestPipeline()

    @pytest.mark.asyncio
    async def test_calculate_balance_remaining_with_balance(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        """Returns remaining balance for limited wallets."""
        mock_user = MagicMock()
        mock_user.id = "user-123"

        thread_db = MagicMock()
        db_user = MagicMock()
        thread_db.query.return_value.filter.return_value.first.return_value = db_user

        with patch("src.api.base.pipeline.create_session", return_value=thread_db):
            with patch(
                "src.api.base.pipeline.WalletService.get_balance_snapshot",
                return_value=70.0,
            ):
                remaining = await pipeline._calculate_balance_remaining_async(mock_user)

        assert remaining == 70.0
        thread_db.close.assert_called_once()

    @pytest.mark.asyncio
    async def test_calculate_balance_remaining_unlimited(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        """Returns None for unlimited wallets."""
        mock_user = MagicMock()
        mock_user.id = "user-123"

        thread_db = MagicMock()
        db_user = MagicMock()
        thread_db.query.return_value.filter.return_value.first.return_value = db_user

        with patch("src.api.base.pipeline.create_session", return_value=thread_db):
            with patch(
                "src.api.base.pipeline.WalletService.get_balance_snapshot",
                return_value=None,
            ):
                remaining = await pipeline._calculate_balance_remaining_async(mock_user)

        assert remaining is None
        thread_db.close.assert_called_once()

    @pytest.mark.asyncio
    async def test_calculate_balance_remaining_none_user(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        """Returns None when user is missing."""
        remaining = await pipeline._calculate_balance_remaining_async(None)

        assert remaining is None


class TestPipelineRunModes:
    """Returns remaining balance for limited wallets."""

    @pytest.fixture
    def pipeline(self) -> ApiRequestPipeline:
        return ApiRequestPipeline()

    @pytest.mark.asyncio
    async def test_run_management_mode_skips_balance_calculation(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        """Management mode skips balance calculation."""
        mock_request = MagicMock()
        mock_request.method = "GET"
        mock_request.url.path = "/api/admin/tokens"
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        mock_user = MagicMock()
        mock_user.id = "admin-123"
        mock_token = MagicMock()
        mock_token.id = "mt-123"

        mock_adapter = MagicMock()
        mock_adapter.name = "test-adapter"
        mock_adapter.authorize = MagicMock(return_value=None)
        mock_response = MagicMock()
        mock_response.status_code = 200
        mock_adapter.handle = AsyncMock(return_value=mock_response)

        mock_context = MagicMock()
        mock_context.db = mock_db
        mock_context.request = mock_request

        with patch.object(
            pipeline,
            "_authenticate_management",
            new_callable=AsyncMock,
            return_value=(mock_user, mock_token),
        ):
            with patch(
                "src.api.base.pipeline.ApiRequestContext.build",
                return_value=mock_context,
            ):
                with patch.object(
                    pipeline,
                    "_calculate_balance_remaining_async",
                    new_callable=AsyncMock,
                ) as mock_balance:
                    with patch.object(pipeline, "_record_audit_event"):
                        response = await pipeline.run(
                            mock_adapter,
                            mock_request,
                            mock_db,
                            mode=ApiMode.MANAGEMENT,
                        )

        assert response == mock_response
        assert mock_context.management_token == mock_token
        mock_balance.assert_not_called()


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
                    mock_context,
                    mock_adapter,
                    success=False,
                    status_code=500,
                    error="Internal error",
                )

            mock_log.assert_called_once()
            call_kwargs = mock_log.call_args[1]
            assert call_kwargs["status_code"] == 500
            assert call_kwargs["error_message"] == "Internal error"

    def test_record_audit_event_commits_when_route_already_committed(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        mock_context = MagicMock()
        mock_context.db = MagicMock()
        mock_context.user = MagicMock()
        mock_context.user.id = "user-123"
        mock_context.api_key = None
        mock_context.request_id = "req-123"
        mock_context.client_ip = "127.0.0.1"
        mock_context.user_agent = "test-agent"
        mock_context.request = MagicMock()
        mock_context.request.method = "POST"
        mock_context.request.url.path = "/api/auth/refresh"
        mock_context.request.state = SimpleNamespace(tx_committed_by_route=True)
        mock_context.start_time = 1000.0

        mock_adapter = MagicMock()
        mock_adapter.name = "test-adapter"
        mock_adapter.audit_log_enabled = True
        mock_adapter.audit_success_event = None
        mock_adapter.audit_failure_event = None

        with patch.object(pipeline.audit_service, "log_event") as mock_log:
            pipeline._record_audit_event(mock_context, mock_adapter, success=True, status_code=200)

        mock_log.assert_called_once()
        mock_context.db.commit.assert_called_once()

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

    @pytest.mark.asyncio
    async def test_authenticate_client_missing_key(self, pipeline: ApiRequestPipeline) -> None:
        """测试缺少 API Key 时抛出异常"""
        mock_request = MagicMock()
        mock_request.headers = {}
        mock_request.url.path = "/v1/messages"
        mock_request.state = MagicMock()

        mock_db = MagicMock()

        mock_adapter = MagicMock()
        mock_adapter.extract_api_key = MagicMock(return_value=None)

        with pytest.raises(HTTPException) as exc_info:
            await pipeline._authenticate_client(mock_request, mock_db, mock_adapter)

        assert exc_info.value.status_code == 401
        assert "API密钥" in exc_info.value.detail

    @pytest.mark.asyncio
    async def test_authenticate_client_invalid_key(self, pipeline: ApiRequestPipeline) -> None:
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
            "authenticate_api_key_threadsafe",
            new_callable=AsyncMock,
            return_value=None,
        ):
            with pytest.raises(HTTPException) as exc_info:
                await pipeline._authenticate_client(mock_request, mock_db, mock_adapter)

        assert exc_info.value.status_code == 401

    @pytest.mark.asyncio
    async def test_authenticate_client_balance_exceeded(self, pipeline: ApiRequestPipeline) -> None:
        """测试余额不足时抛出异常"""
        mock_user = MagicMock()
        mock_user.id = "user-123"

        mock_api_key = MagicMock()
        mock_api_key.id = "key-123"
        mock_api_key.is_standalone = False

        mock_request = MagicMock()
        mock_request.headers = {"Authorization": "Bearer sk-test"}
        mock_request.url.path = "/v1/messages"
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        db_user = MagicMock()
        db_user.id = "user-123"
        db_user.is_active = True
        db_user.is_deleted = False
        db_api_key = MagicMock()
        db_api_key.id = "key-123"
        db_api_key.user_id = "user-123"
        db_api_key.is_active = True
        db_api_key.is_locked = False
        db_api_key.is_standalone = False
        db_api_key.expires_at = None
        user_query = MagicMock()
        user_query.filter.return_value.first.return_value = db_user
        api_key_query = MagicMock()
        api_key_query.filter.return_value.first.return_value = db_api_key
        mock_db.query.side_effect = [user_query, api_key_query]

        mock_adapter = MagicMock()
        mock_adapter.extract_api_key = MagicMock(return_value="sk-test")

        with patch.object(
            pipeline.auth_service,
            "authenticate_api_key_threadsafe",
            new_callable=AsyncMock,
            return_value=MagicMock(
                user=mock_user,
                api_key=mock_api_key,
                access_allowed=False,
                balance_remaining=0.0,
            ),
        ):
            from src.core.exceptions import BalanceInsufficientException

            with pytest.raises(BalanceInsufficientException):
                await pipeline._authenticate_client(mock_request, mock_db, mock_adapter)

    @pytest.mark.asyncio
    async def test_authenticate_client_requery_detects_inactive_user(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_api_key = MagicMock()
        mock_api_key.id = "key-123"

        mock_request = MagicMock()
        mock_request.headers = {"Authorization": "Bearer sk-test"}
        mock_request.url.path = "/v1/messages"
        mock_request.state = MagicMock()

        mock_adapter = MagicMock()
        mock_adapter.extract_api_key = MagicMock(return_value="sk-test")

        db_user = MagicMock()
        db_user.id = "user-123"
        db_user.is_active = False
        db_user.is_deleted = False
        db_api_key = MagicMock()
        db_api_key.id = "key-123"
        db_api_key.user_id = "user-123"
        db_api_key.is_active = True
        db_api_key.is_locked = False
        db_api_key.is_standalone = False
        db_api_key.expires_at = None

        mock_db = MagicMock()
        user_query = MagicMock()
        user_query.filter.return_value.first.return_value = db_user
        api_key_query = MagicMock()
        api_key_query.filter.return_value.first.return_value = db_api_key
        mock_db.query.side_effect = [user_query, api_key_query]

        with patch.object(
            pipeline.auth_service,
            "authenticate_api_key_threadsafe",
            new_callable=AsyncMock,
            return_value=MagicMock(
                user=mock_user,
                api_key=mock_api_key,
                access_allowed=True,
                balance_remaining=10.0,
            ),
        ):
            with pytest.raises(HTTPException) as exc_info:
                await pipeline._authenticate_client(mock_request, mock_db, mock_adapter)

        assert exc_info.value.status_code == 401

    @pytest.mark.asyncio
    async def test_authenticate_client_requery_detects_locked_key(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_api_key = MagicMock()
        mock_api_key.id = "key-123"

        mock_request = MagicMock()
        mock_request.headers = {"Authorization": "Bearer sk-test"}
        mock_request.url.path = "/v1/messages"
        mock_request.state = MagicMock()

        mock_adapter = MagicMock()
        mock_adapter.extract_api_key = MagicMock(return_value="sk-test")

        db_user = MagicMock()
        db_user.id = "user-123"
        db_user.is_active = True
        db_user.is_deleted = False
        db_api_key = MagicMock()
        db_api_key.id = "key-123"
        db_api_key.user_id = "user-123"
        db_api_key.is_active = True
        db_api_key.is_locked = True
        db_api_key.is_standalone = False
        db_api_key.expires_at = None

        mock_db = MagicMock()
        user_query = MagicMock()
        user_query.filter.return_value.first.return_value = db_user
        api_key_query = MagicMock()
        api_key_query.filter.return_value.first.return_value = db_api_key
        mock_db.query.side_effect = [user_query, api_key_query]

        with patch.object(
            pipeline.auth_service,
            "authenticate_api_key_threadsafe",
            new_callable=AsyncMock,
            return_value=MagicMock(
                user=mock_user,
                api_key=mock_api_key,
                access_allowed=True,
                balance_remaining=10.0,
            ),
        ):
            with pytest.raises(HTTPException) as exc_info:
                await pipeline._authenticate_client(mock_request, mock_db, mock_adapter)

        assert exc_info.value.status_code == 403
        assert "锁定" in str(exc_info.value.detail)


class TestPipelineUserRateLimit:
    @pytest.fixture
    def pipeline(self) -> ApiRequestPipeline:
        return ApiRequestPipeline()

    @pytest.mark.asyncio
    async def test_check_user_rate_limit_uses_system_default_for_user_scope(
        self, pipeline: ApiRequestPipeline, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        request = MagicMock()
        request.state = MagicMock()
        db = MagicMock()
        user = MagicMock(id="user-1", rate_limit=None)
        api_key = MagicMock(id="key-1", is_standalone=False, rate_limit=0)

        limiter = MagicMock()
        limiter.get_user_rpm_key.return_value = "rpm:user:user-1:1"
        limiter.get_key_rpm_key.return_value = "rpm:key:key-1:1"
        limiter.check_and_consume = AsyncMock(return_value=RpmCheckResult(allowed=True))

        monkeypatch.setattr(
            "src.api.base.pipeline.get_user_rpm_limiter",
            AsyncMock(return_value=limiter),
        )
        monkeypatch.setattr(
            "src.api.base.pipeline.SystemConfigService.get_config",
            lambda *_a, **_k: 60,
        )

        await pipeline._check_user_rate_limit(request, db, user, api_key)

        limiter.check_and_consume.assert_awaited_once_with(
            user_rpm_key="rpm:user:user-1:1",
            user_rpm_limit=60,
            key_rpm_key="rpm:key:key-1:1",
            key_rpm_limit=0,
        )

    @pytest.mark.asyncio
    async def test_check_user_rate_limit_returns_429_with_scope_header(
        self, pipeline: ApiRequestPipeline, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        request = MagicMock()
        request.state = MagicMock()
        db = MagicMock()
        user = MagicMock(id="user-1", rate_limit=100)
        api_key = MagicMock(id="key-1", is_standalone=False, rate_limit=10)

        limiter = MagicMock()
        limiter.get_user_rpm_key.return_value = "rpm:user:user-1:1"
        limiter.get_key_rpm_key.return_value = "rpm:key:key-1:1"
        limiter.get_retry_after.return_value = 17
        limiter.check_and_consume = AsyncMock(
            return_value=RpmCheckResult(
                allowed=False,
                scope="key",
                limit=10,
                remaining=0,
                retry_after=17,
            )
        )

        monkeypatch.setattr(
            "src.api.base.pipeline.get_user_rpm_limiter",
            AsyncMock(return_value=limiter),
        )
        monkeypatch.setattr(
            "src.api.base.pipeline.SystemConfigService.get_config",
            lambda *_a, **_k: 60,
        )

        with pytest.raises(HTTPException) as exc_info:
            await pipeline._check_user_rate_limit(request, db, user, api_key)

        assert exc_info.value.status_code == 429
        assert exc_info.value.headers == {
            "Retry-After": "17",
            "X-RateLimit-Limit": "10",
            "X-RateLimit-Remaining": "0",
            "X-RateLimit-Scope": "key",
        }

    @pytest.mark.asyncio
    async def test_check_user_rate_limit_uses_system_default_for_standalone_key(
        self, pipeline: ApiRequestPipeline, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        request = MagicMock()
        request.state = MagicMock()
        db = MagicMock()
        user = MagicMock(id="user-1", rate_limit=999)
        api_key = MagicMock(id="standalone-1", is_standalone=True, rate_limit=None)

        limiter = MagicMock()
        limiter.get_standalone_rpm_key.return_value = "rpm:ukey:standalone-1:1"
        limiter.get_key_rpm_key.return_value = "rpm:key:standalone-1:1"
        limiter.check_and_consume = AsyncMock(return_value=RpmCheckResult(allowed=True))

        monkeypatch.setattr(
            "src.api.base.pipeline.get_user_rpm_limiter",
            AsyncMock(return_value=limiter),
        )
        monkeypatch.setattr(
            "src.api.base.pipeline.SystemConfigService.get_config",
            lambda *_a, **_k: 60,
        )

        await pipeline._check_user_rate_limit(request, db, user, api_key)

        limiter.check_and_consume.assert_awaited_once_with(
            user_rpm_key="rpm:ukey:standalone-1:1",
            user_rpm_limit=60,
            key_rpm_key="rpm:key:standalone-1:1",
            key_rpm_limit=0,
        )

    @pytest.mark.asyncio
    async def test_check_user_rate_limit_returns_429_with_user_scope_header(
        self, pipeline: ApiRequestPipeline, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        request = MagicMock()
        request.state = MagicMock()
        db = MagicMock()
        user = MagicMock(id="user-1", rate_limit=3)
        api_key = MagicMock(id="key-1", is_standalone=False, rate_limit=10)

        limiter = MagicMock()
        limiter.get_user_rpm_key.return_value = "rpm:user:user-1:1"
        limiter.get_key_rpm_key.return_value = "rpm:key:key-1:1"
        limiter.get_retry_after.return_value = 23
        limiter.check_and_consume = AsyncMock(
            return_value=RpmCheckResult(
                allowed=False,
                scope="user",
                limit=3,
                remaining=0,
                retry_after=23,
            )
        )

        monkeypatch.setattr(
            "src.api.base.pipeline.get_user_rpm_limiter",
            AsyncMock(return_value=limiter),
        )
        monkeypatch.setattr(
            "src.api.base.pipeline.SystemConfigService.get_config",
            lambda *_a, **_k: 60,
        )

        with pytest.raises(HTTPException) as exc_info:
            await pipeline._check_user_rate_limit(request, db, user, api_key)

        assert exc_info.value.status_code == 429
        assert exc_info.value.headers == {
            "Retry-After": "23",
            "X-RateLimit-Limit": "3",
            "X-RateLimit-Remaining": "0",
            "X-RateLimit-Scope": "user",
        }


class TestPipelineTokenPrefixAuth:
    """Tests token-prefix auth isolation."""

    @pytest.fixture
    def pipeline(self) -> ApiRequestPipeline:
        return ApiRequestPipeline()

    @pytest.mark.asyncio
    async def test_try_token_prefix_auth_uses_isolated_session(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        mock_request = MagicMock()
        mock_request.headers = {}
        mock_request.client = MagicMock(host="127.0.0.1")

        route_db = MagicMock()
        auth_db = MagicMock()
        mock_user = MagicMock()
        mock_token = MagicMock()

        async def authenticate(db: Any, token: str, client_ip: str) -> tuple[Any, Any]:
            assert db is auth_db
            assert token == "ae_test"
            assert client_ip == "127.0.0.1"
            return mock_user, mock_token

        with patch("src.api.base.pipeline.create_session", return_value=auth_db):
            with patch("src.utils.request_utils.get_client_ip", return_value="127.0.0.1"):
                with patch("src.core.modules.hooks.get_hook_dispatcher") as mock_get_dispatcher:
                    dispatcher = MagicMock()
                    dispatcher.dispatch = AsyncMock(
                        return_value=[
                            {
                                "prefix": "ae_",
                                "module": "management_tokens",
                                "authenticate": authenticate,
                            }
                        ]
                    )
                    mock_get_dispatcher.return_value = dispatcher

                    result = await pipeline._try_token_prefix_auth(
                        "ae_test", mock_request, route_db
                    )

        assert result == (mock_user, mock_token)
        dispatcher.dispatch.assert_awaited_once_with(AUTH_TOKEN_PREFIX_AUTHENTICATORS)
        auth_db.expunge.assert_any_call(mock_user)
        auth_db.expunge.assert_any_call(mock_token)
        auth_db.close.assert_called_once()


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
        created_at = datetime.now(timezone.utc)
        mock_session = MagicMock()
        mock_session.id = "session-123"

        mock_user = MagicMock()
        mock_user.id = "admin-123"
        mock_user.is_active = True
        mock_user.is_deleted = False
        mock_user.role = UserRole.ADMIN
        mock_user.email = "admin@example.com"
        mock_user.created_at = created_at

        mock_request = MagicMock()
        mock_request.headers = {
            "authorization": "Bearer valid-token",
            "X-Client-Device-Id": "device-admin-123",
        }
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with (
            patch.object(
                pipeline.auth_service,
                "verify_token",
                new_callable=AsyncMock,
                return_value={
                    "user_id": "admin-123",
                    "created_at": created_at.isoformat(),
                    "session_id": "session-123",
                },
            ),
            patch(
                "src.api.base.pipeline.SessionService.get_active_session",
                return_value=mock_session,
            ),
            patch("src.api.base.pipeline.SessionService.touch_session", return_value=True),
            patch("src.api.base.pipeline.SessionService.assert_session_device_matches"),
        ):
            user, management_token = await pipeline._authenticate_admin(mock_request, mock_db)

        assert user == mock_user
        assert management_token is None
        assert mock_request.state.user_id == "admin-123"
        assert mock_request.state.user_session_id == "session-123"
        mock_db.commit.assert_called_once()

    @pytest.mark.asyncio
    async def test_authenticate_admin_lowercase_bearer(self, pipeline: ApiRequestPipeline) -> None:
        """测试 bearer (小写) 前缀也能正确解析"""
        created_at = datetime.now(timezone.utc)
        mock_session = MagicMock()
        mock_session.id = "session-123"

        mock_user = MagicMock()
        mock_user.id = "admin-123"
        mock_user.is_active = True
        mock_user.is_deleted = False
        mock_user.role = UserRole.ADMIN
        mock_user.email = "admin@example.com"
        mock_user.created_at = created_at

        mock_request = MagicMock()
        mock_request.headers = {
            "authorization": "bearer valid-token",
            "X-Client-Device-Id": "device-admin-123",
        }
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with (
            patch.object(
                pipeline.auth_service,
                "verify_token",
                new_callable=AsyncMock,
                return_value={
                    "user_id": "admin-123",
                    "created_at": created_at.isoformat(),
                    "session_id": "session-123",
                },
            ) as mock_verify,
            patch(
                "src.api.base.pipeline.SessionService.get_active_session",
                return_value=mock_session,
            ),
            patch("src.api.base.pipeline.SessionService.touch_session", return_value=True),
            patch("src.api.base.pipeline.SessionService.assert_session_device_matches"),
        ):
            user, management_token = await pipeline._authenticate_admin(mock_request, mock_db)

        mock_verify.assert_awaited_once_with("valid-token", token_type="access")
        assert user == mock_user
        assert management_token is None

    @pytest.mark.asyncio
    async def test_authenticate_admin_rejects_legacy_token_without_session_id(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        created_at = datetime.now(timezone.utc)

        mock_request = MagicMock()
        mock_request.headers = {
            "authorization": "Bearer valid-token",
            "X-Client-Device-Id": "device-admin-legacy",
        }
        mock_request.state = MagicMock()

        mock_user = MagicMock()
        mock_user.id = "admin-123"
        mock_user.is_active = True
        mock_user.is_deleted = False
        mock_user.role = UserRole.ADMIN
        mock_user.email = "admin@example.com"
        mock_user.created_at = created_at

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with (
            patch.object(
                pipeline.auth_service,
                "verify_token",
                new_callable=AsyncMock,
                return_value={"user_id": "admin-123", "created_at": created_at.isoformat()},
            ),
            patch.object(
                pipeline.auth_service,
                "token_identity_matches_user",
                return_value=True,
            ),
        ):
            with pytest.raises(HTTPException, match="登录会话已失效，请重新登录"):
                await pipeline._authenticate_admin(mock_request, mock_db)

    @pytest.mark.asyncio
    async def test_authenticate_admin_rollback_on_session_touch_commit_failure(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        created_at = datetime.now(timezone.utc)
        mock_session = MagicMock()
        mock_session.id = "session-123"

        mock_user = MagicMock()
        mock_user.id = "admin-123"
        mock_user.is_active = True
        mock_user.is_deleted = False
        mock_user.role = UserRole.ADMIN
        mock_user.email = "admin@example.com"
        mock_user.created_at = created_at

        mock_request = MagicMock()
        mock_request.headers = {
            "authorization": "Bearer valid-token",
            "X-Client-Device-Id": "device-admin-123",
        }
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user
        mock_db.commit.side_effect = RuntimeError("lock timeout")

        with (
            patch.object(
                pipeline.auth_service,
                "verify_token",
                new_callable=AsyncMock,
                return_value={
                    "user_id": "admin-123",
                    "created_at": created_at.isoformat(),
                    "session_id": "session-123",
                },
            ),
            patch(
                "src.api.base.pipeline.SessionService.get_active_session",
                return_value=mock_session,
            ),
            patch("src.api.base.pipeline.SessionService.touch_session", return_value=True),
            patch("src.api.base.pipeline.SessionService.assert_session_device_matches"),
        ):
            user, management_token = await pipeline._authenticate_admin(mock_request, mock_db)

        assert user == mock_user
        assert management_token is None
        mock_db.rollback.assert_called_once()

    @pytest.mark.asyncio
    async def test_authenticate_admin_skips_commit_when_session_touch_not_needed(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        created_at = datetime.now(timezone.utc)
        mock_session = MagicMock()
        mock_session.id = "session-123"

        mock_user = MagicMock()
        mock_user.id = "admin-123"
        mock_user.is_active = True
        mock_user.is_deleted = False
        mock_user.role = UserRole.ADMIN
        mock_user.email = "admin@example.com"
        mock_user.created_at = created_at

        mock_request = MagicMock()
        mock_request.headers = {
            "authorization": "Bearer valid-token",
            "X-Client-Device-Id": "device-admin-123",
        }
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with (
            patch.object(
                pipeline.auth_service,
                "verify_token",
                new_callable=AsyncMock,
                return_value={
                    "user_id": "admin-123",
                    "created_at": created_at.isoformat(),
                    "session_id": "session-123",
                },
            ),
            patch(
                "src.api.base.pipeline.SessionService.get_active_session",
                return_value=mock_session,
            ),
            patch(
                "src.api.base.pipeline.SessionService.touch_session",
                return_value=False,
            ),
            patch("src.api.base.pipeline.SessionService.assert_session_device_matches"),
        ):
            user, management_token = await pipeline._authenticate_admin(mock_request, mock_db)

        assert user == mock_user
        assert management_token is None
        mock_db.commit.assert_not_called()


class TestPipelineUserAuth:
    """测试普通用户 JWT 认证"""

    @pytest.fixture
    def pipeline(self) -> ApiRequestPipeline:
        return ApiRequestPipeline()

    @pytest.mark.asyncio
    async def test_authenticate_user_lowercase_bearer(self, pipeline: ApiRequestPipeline) -> None:
        """测试 bearer (小写) 前缀也能正确解析"""
        created_at = datetime.now(timezone.utc)
        mock_session = MagicMock()
        mock_session.id = "session-456"

        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_user.is_active = True
        mock_user.is_deleted = False
        mock_user.email = "user@example.com"
        mock_user.created_at = created_at

        mock_request = MagicMock()
        mock_request.headers = {
            "authorization": "bearer valid-token",
            "X-Client-Device-Id": "device-user-456",
        }
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with (
            patch.object(
                pipeline.auth_service,
                "verify_token",
                new_callable=AsyncMock,
                return_value={
                    "user_id": "user-123",
                    "created_at": created_at.isoformat(),
                    "session_id": "session-456",
                },
            ) as mock_verify,
            patch(
                "src.api.base.pipeline.SessionService.get_active_session",
                return_value=mock_session,
            ),
            patch("src.api.base.pipeline.SessionService.touch_session", return_value=True),
            patch("src.api.base.pipeline.SessionService.assert_session_device_matches"),
        ):
            user, management_token = await pipeline._authenticate_user(mock_request, mock_db)

        mock_verify.assert_awaited_once_with("valid-token", token_type="access")
        assert user == mock_user
        assert management_token is None
        assert mock_request.state.user_session_id == "session-456"
        mock_db.commit.assert_called_once()

    @pytest.mark.asyncio
    async def test_authenticate_user_rejects_legacy_token_without_session_id(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        """历史 JWT 无 session_id 时应拒绝。"""
        created_at = datetime.now(timezone.utc)

        mock_request = MagicMock()
        mock_request.headers = {
            "authorization": "Bearer valid-token",
            "X-Client-Device-Id": "device-user-legacy",
        }
        mock_request.state = MagicMock()

        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_user.is_active = True
        mock_user.is_deleted = False

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with (
            patch.object(
                pipeline.auth_service,
                "verify_token",
                new_callable=AsyncMock,
                return_value={"user_id": "user-123", "created_at": created_at.isoformat()},
            ),
            patch.object(
                pipeline.auth_service,
                "token_identity_matches_user",
                return_value=True,
            ),
        ):
            with pytest.raises(HTTPException, match="登录会话已失效，请重新登录"):
                await pipeline._authenticate_user(mock_request, mock_db)

    @pytest.mark.asyncio
    async def test_authenticate_user_requires_device_id(self, pipeline: ApiRequestPipeline) -> None:
        created_at = datetime.now(timezone.utc)

        mock_request = MagicMock()
        mock_request.headers = {"authorization": "Bearer valid-token"}
        mock_request.query_params = {}
        mock_request.state = MagicMock()

        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_user.is_active = True
        mock_user.is_deleted = False
        mock_user.created_at = created_at

        mock_session = MagicMock()
        mock_session.id = "session-456"

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with (
            patch.object(
                pipeline.auth_service,
                "verify_token",
                new_callable=AsyncMock,
                return_value={
                    "user_id": "user-123",
                    "created_at": created_at.isoformat(),
                    "session_id": "session-456",
                },
            ),
            patch(
                "src.api.base.pipeline.SessionService.get_active_session",
                return_value=mock_session,
            ),
            patch.object(
                pipeline.auth_service,
                "token_identity_matches_user",
                return_value=True,
            ),
        ):
            with pytest.raises(HTTPException, match="缺少或无效的设备标识"):
                await pipeline._authenticate_user(mock_request, mock_db)

    @pytest.mark.asyncio
    async def test_authenticate_user_skips_commit_when_session_touch_not_needed(
        self, pipeline: ApiRequestPipeline
    ) -> None:
        created_at = datetime.now(timezone.utc)
        mock_session = MagicMock()
        mock_session.id = "session-456"

        mock_user = MagicMock()
        mock_user.id = "user-123"
        mock_user.is_active = True
        mock_user.is_deleted = False
        mock_user.email = "user@example.com"
        mock_user.created_at = created_at

        mock_request = MagicMock()
        mock_request.headers = {
            "authorization": "Bearer valid-token",
            "X-Client-Device-Id": "device-user-456",
        }
        mock_request.state = MagicMock()

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = mock_user

        with (
            patch.object(
                pipeline.auth_service,
                "verify_token",
                new_callable=AsyncMock,
                return_value={
                    "user_id": "user-123",
                    "created_at": created_at.isoformat(),
                    "session_id": "session-456",
                },
            ),
            patch(
                "src.api.base.pipeline.SessionService.get_active_session",
                return_value=mock_session,
            ),
            patch(
                "src.api.base.pipeline.SessionService.touch_session",
                return_value=False,
            ),
            patch("src.api.base.pipeline.SessionService.assert_session_device_matches"),
        ):
            user, management_token = await pipeline._authenticate_user(mock_request, mock_db)

        assert user == mock_user
        assert management_token is None
        mock_db.commit.assert_not_called()
