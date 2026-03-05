"""
UsageService 测试

测试用量统计服务的核心功能：
- 成本计算
- 钱包准入检查
- 用量统计查询
"""

from decimal import Decimal
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from src.services.usage.service import UsageService
from src.services.wallet import WalletAccessResult


class TestCostCalculation:
    """测试成本计算"""

    def test_calculate_cost_basic(self) -> None:
        """测试基础成本计算"""
        # 价格：输入 $3/1M, 输出 $15/1M
        result = UsageService.calculate_cost(
            input_tokens=1000,
            output_tokens=500,
            input_price_per_1m=3.0,
            output_price_per_1m=15.0,
        )

        (
            input_cost,
            output_cost,
            cache_creation_cost,
            cache_read_cost,
            cache_cost,
            request_cost,
            total_cost,
        ) = result

        # 1000 tokens * $3 / 1M = $0.003
        assert abs(input_cost - 0.003) < 0.0001
        # 500 tokens * $15 / 1M = $0.0075
        assert abs(output_cost - 0.0075) < 0.0001
        # Total = $0.003 + $0.0075 = $0.0105
        assert abs(total_cost - 0.0105) < 0.0001

    def test_calculate_cost_with_cache(self) -> None:
        """测试带缓存的成本计算"""
        result = UsageService.calculate_cost(
            input_tokens=1000,
            output_tokens=500,
            input_price_per_1m=3.0,
            output_price_per_1m=15.0,
            cache_creation_input_tokens=200,
            cache_read_input_tokens=300,
            cache_creation_price_per_1m=3.75,  # 1.25x input price
            cache_read_price_per_1m=0.3,  # 0.1x input price
        )

        (
            input_cost,
            output_cost,
            cache_creation_cost,
            cache_read_cost,
            cache_cost,
            request_cost,
            total_cost,
        ) = result

        # 验证缓存成本被计算
        assert cache_creation_cost > 0
        assert cache_read_cost > 0
        assert cache_cost == cache_creation_cost + cache_read_cost

    def test_calculate_cost_with_request_price(self) -> None:
        """测试按次计费"""
        result = UsageService.calculate_cost(
            input_tokens=1000,
            output_tokens=500,
            input_price_per_1m=3.0,
            output_price_per_1m=15.0,
            price_per_request=0.01,
        )

        (
            input_cost,
            output_cost,
            cache_creation_cost,
            cache_read_cost,
            cache_cost,
            request_cost,
            total_cost,
        ) = result

        assert request_cost == 0.01
        # Total 包含 request_cost
        assert total_cost == input_cost + output_cost + request_cost

    def test_calculate_cost_zero_tokens(self) -> None:
        """测试零 token 的成本计算"""
        result = UsageService.calculate_cost(
            input_tokens=0,
            output_tokens=0,
            input_price_per_1m=3.0,
            output_price_per_1m=15.0,
        )

        (
            input_cost,
            output_cost,
            cache_creation_cost,
            cache_read_cost,
            cache_cost,
            request_cost,
            total_cost,
        ) = result

        assert input_cost == 0
        assert output_cost == 0
        assert total_cost == 0


class TestBalanceCheck:
    """测试钱包准入检查"""

    def test_check_request_balance_sufficient(self) -> None:
        """测试余额充足"""
        mock_user = MagicMock()
        mock_user.role = MagicMock()
        mock_user.role.value = "user"

        mock_api_key = MagicMock()
        mock_api_key.is_standalone = False

        mock_db = MagicMock()

        with patch(
            "src.services.wallet.WalletService.check_request_allowed",
            return_value=WalletAccessResult(True, Decimal("70"), "OK"),
        ):
            is_ok, message = UsageService.check_request_balance(
                mock_db, mock_user, api_key=mock_api_key
            )

        assert is_ok is True

    def test_check_request_balance_exceeded(self) -> None:
        """测试余额耗尽时拦截新请求"""
        mock_user = MagicMock()
        mock_user.role = MagicMock()
        mock_user.role.value = "user"

        mock_api_key = MagicMock()
        mock_api_key.is_standalone = False

        mock_db = MagicMock()

        with patch(
            "src.services.wallet.WalletService.check_request_allowed",
            return_value=WalletAccessResult(False, Decimal("0"), "钱包余额不足"),
        ):
            is_ok, message = UsageService.check_request_balance(
                mock_db, mock_user, estimated_cost=5.0, api_key=mock_api_key
            )

        assert is_ok is False
        assert "余额" in message

    def test_check_request_balance_no_limit(self) -> None:
        """测试无配额限制（None）"""
        mock_user = MagicMock()
        mock_user.role = MagicMock()
        mock_user.role.value = "user"

        mock_api_key = MagicMock()
        mock_api_key.is_standalone = False

        mock_db = MagicMock()

        with patch(
            "src.services.wallet.WalletService.check_request_allowed",
            return_value=WalletAccessResult(True, None, "OK"),
        ):
            is_ok, message = UsageService.check_request_balance(
                mock_db, mock_user, api_key=mock_api_key
            )

        assert is_ok is True

    def test_check_request_balance_admin_bypass(self) -> None:
        """测试管理员绕过余额检查"""
        from src.models.database import UserRole

        mock_user = MagicMock()
        mock_user.role = UserRole.ADMIN

        mock_api_key = MagicMock()
        mock_api_key.is_standalone = False

        mock_db = MagicMock()

        with patch(
            "src.services.wallet.WalletService.check_request_allowed",
            return_value=WalletAccessResult(True, None, "OK"),
        ):
            is_ok, message = UsageService.check_request_balance(
                mock_db, mock_user, api_key=mock_api_key
            )

        assert is_ok is True

    def test_check_standalone_api_key_balance(self) -> None:
        """测试独立 API Key 余额充足"""
        mock_user = MagicMock()
        mock_user.role = MagicMock()
        mock_user.role.value = "user"

        mock_api_key = MagicMock()
        mock_api_key.is_standalone = True

        mock_db = MagicMock()

        with patch(
            "src.services.wallet.WalletService.check_request_allowed",
            return_value=WalletAccessResult(True, Decimal("40"), "OK"),
        ):
            is_ok, message = UsageService.check_request_balance(
                mock_db, mock_user, api_key=mock_api_key
            )

        assert is_ok is True

    def test_check_standalone_api_key_insufficient_balance(self) -> None:
        """测试独立 API Key 余额耗尽时拦截"""
        mock_user = MagicMock()
        mock_user.role = MagicMock()
        mock_user.role.value = "user"

        mock_api_key = MagicMock()
        mock_api_key.is_standalone = True

        mock_db = MagicMock()

        with patch(
            "src.services.wallet.WalletService.check_request_allowed",
            return_value=WalletAccessResult(False, Decimal("0"), "钱包余额不足"),
        ):
            is_ok, message = UsageService.check_request_balance(
                mock_db, mock_user, estimated_cost=5.0, api_key=mock_api_key
            )

        assert is_ok is False
        assert "Key余额不足" in message


class TestUsageStatistics:
    """测试用量统计查询

    注意：get_usage_summary 方法内部使用了数据库方言特定的日期函数，
    需要真实数据库或更复杂的 mock。这里只测试方法存在性。
    """

    def test_get_usage_summary_exists(self) -> None:
        """测试 get_usage_summary 方法存在"""
        assert hasattr(UsageService, "get_usage_summary")
        assert callable(getattr(UsageService, "get_usage_summary"))


class TestHelperMethods:
    """测试辅助方法"""

    @pytest.mark.asyncio
    async def test_get_rate_multiplier_and_free_tier_default(self) -> None:
        """测试默认费率倍数"""
        mock_db = MagicMock()
        # 模拟未找到 provider_api_key
        mock_db.query.return_value.filter.return_value.first.return_value = None

        rate_multiplier, is_free_tier = await UsageService._get_rate_multiplier_and_free_tier(
            mock_db, provider_api_key_id=None, provider_id=None
        )

        assert rate_multiplier == 1.0
        assert is_free_tier is False

    @pytest.mark.asyncio
    async def test_get_rate_multiplier_from_provider_api_key(self) -> None:
        """测试从 ProviderAPIKey 获取费率倍数"""
        mock_provider_api_key = MagicMock()
        mock_provider_api_key.rate_multipliers = {"claude:chat": 0.8}

        mock_endpoint = MagicMock()
        mock_endpoint.provider_id = "provider-123"

        mock_provider = MagicMock()
        mock_provider.billing_type = "standard"

        mock_db = MagicMock()
        # 第一次查询返回 provider_api_key
        mock_db.query.return_value.filter.return_value.first.side_effect = [
            mock_provider_api_key,
            mock_endpoint,
            mock_provider,
        ]

        rate_multiplier, is_free_tier = await UsageService._get_rate_multiplier_and_free_tier(
            mock_db, provider_api_key_id="pak-123", provider_id=None, api_format="claude:chat"
        )

        assert rate_multiplier == 0.8
        assert is_free_tier is False


class TestUsageStatusUpdate:
    """测试进行中状态更新对请求头/体的补写能力"""

    def test_update_usage_status_can_persist_request_and_provider_payloads(self) -> None:
        usage = MagicMock()
        usage.status = "pending"
        usage.provider_name = "pending"
        usage.billing_status = "pending"
        usage.finalized_at = None
        usage.request_headers = None
        usage.request_body = None
        usage.provider_request_headers = None
        usage.provider_request_body = None

        mock_db = MagicMock()
        mock_db.query.return_value.filter.return_value.first.return_value = usage

        client_headers = {"authorization": "Bearer abc", "x-trace-id": "trace-1"}
        provider_headers = {"authorization": "Bearer upstream", "x-provider": "demo"}
        client_body = {"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "hello"}]}
        provider_body = {
            "model": "upstream-model",
            "messages": [{"role": "user", "content": "hello"}],
        }

        with (
            patch(
                "src.services.system.config.SystemConfigService.should_log_headers",
                return_value=True,
            ),
            patch(
                "src.services.system.config.SystemConfigService.should_log_body",
                return_value=True,
            ),
            patch(
                "src.services.system.config.SystemConfigService.mask_sensitive_headers",
                side_effect=lambda _db, h: {"masked": h},
            ),
            patch(
                "src.services.system.config.SystemConfigService.truncate_body",
                side_effect=lambda _db, b, is_request=True: {
                    "truncated": b,
                    "is_request": is_request,
                },
            ),
        ):
            updated = UsageService.update_usage_status(
                db=mock_db,
                request_id="req-streaming-1",
                status="streaming",
                provider="demo-provider",
                request_headers=client_headers,
                request_body=client_body,
                provider_request_headers=provider_headers,
                provider_request_body=provider_body,
            )

        assert updated is usage
        assert usage.request_headers == {"masked": client_headers}
        assert usage.provider_request_headers == {"masked": provider_headers}
        assert usage.request_body == {"truncated": client_body, "is_request": True}
        assert usage.provider_request_body == {"truncated": provider_body, "is_request": True}
        mock_db.commit.assert_called_once()
