"""
YesCode 余额查询操作
"""

import asyncio
from datetime import datetime, timedelta
from typing import Any, cast

import httpx

from src.services.provider_ops.actions.balance import BalanceAction
from src.services.provider_ops.types import ActionResult, ActionStatus, BalanceInfo


async def fetch_yescode_combined_data(
    client: httpx.AsyncClient,
    base_url: str,
) -> dict[str, Any]:
    """
    获取 YesCode 合并数据（balance + profile）

    使用传入的 client 并发调用两个接口：
    - /api/v1/user/balance: 准确的 weekly_spent_balance
    - /api/v1/auth/profile: 用户信息、重置时间

    Args:
        client: 已配置好认证的 HTTP 客户端
        base_url: API 基础地址

    Returns:
        合并后的数据字典
    """
    base_url = base_url.rstrip("/")
    result: dict[str, Any] = {}

    # 并发调用两个接口
    balance_task = client.get(f"{base_url}/api/v1/user/balance")
    profile_task = client.get(f"{base_url}/api/v1/auth/profile")

    balance_resp, profile_resp = cast(
        tuple[httpx.Response | BaseException, httpx.Response | BaseException],
        await asyncio.gather(balance_task, profile_task, return_exceptions=True),
    )

    # 解析 balance 接口
    balance_data: dict[str, Any] = {}
    if isinstance(balance_resp, httpx.Response) and balance_resp.status_code == 200:
        try:
            balance_data = balance_resp.json()
            result["_balance_data"] = balance_data
        except Exception:
            pass

    # 解析 profile 接口
    profile_data: dict[str, Any] = {}
    if isinstance(profile_resp, httpx.Response) and profile_resp.status_code == 200:
        try:
            profile_data = profile_resp.json()
            result["_profile_data"] = profile_data
        except Exception:
            pass

    # 合并数据：balance 接口优先（更准确的余额数据）
    result["pay_as_you_go_balance"] = balance_data.get(
        "pay_as_you_go_balance", profile_data.get("pay_as_you_go_balance", 0)
    )
    result["subscription_balance"] = balance_data.get(
        "subscription_balance", profile_data.get("subscription_balance", 0)
    )
    result["weekly_limit"] = balance_data.get("weekly_limit") or (
        profile_data.get("subscription_plan") or {}
    ).get("weekly_limit")
    result["weekly_spent_balance"] = balance_data.get(
        "weekly_spent_balance", profile_data.get("current_week_spend", 0)
    )

    # 用户信息（仅 profile 有）
    result["username"] = profile_data.get("username")
    result["email"] = profile_data.get("email")

    # 重置时间（仅 profile 有）
    result["last_week_reset"] = profile_data.get("last_week_reset")
    result["last_daily_balance_add"] = profile_data.get("last_daily_balance_add")

    # subscription_plan（仅 profile 有）
    result["subscription_plan"] = profile_data.get("subscription_plan")

    return result


def parse_yescode_balance_extra(data: dict[str, Any]) -> dict[str, Any]:
    """
    解析 YesCode 余额额外信息

    Args:
        data: 合并后的数据（来自 fetch_yescode_combined_data 或单独接口）

    Returns:
        统一格式的 extra 字典
    """
    extra: dict[str, Any] = {}

    pay_as_you_go = data.get("pay_as_you_go_balance", 0)
    subscription = data.get("subscription_balance", 0)

    extra["pay_as_you_go_balance"] = pay_as_you_go

    # 每日额度上限
    plan = data.get("subscription_plan") or {}
    daily_balance = plan.get("daily_balance", subscription)

    # 周限额
    weekly_limit = data.get("weekly_limit") or plan.get("weekly_limit")
    weekly_spent = data.get("weekly_spent_balance", 0)

    # 映射为统一字段
    extra["daily_limit"] = daily_balance
    if weekly_limit is not None:
        extra["weekly_limit"] = weekly_limit
    extra["weekly_spent"] = weekly_spent

    # 计算重置时间
    last_week_reset = data.get("last_week_reset")
    if last_week_reset:
        try:
            if isinstance(last_week_reset, str):
                reset_dt = datetime.fromisoformat(last_week_reset.replace("Z", "+00:00"))
                next_reset = reset_dt + timedelta(days=7)
                extra["weekly_resets_at"] = int(next_reset.timestamp())
        except Exception:
            pass

    last_daily_add = data.get("last_daily_balance_add")
    if last_daily_add:
        try:
            if isinstance(last_daily_add, str):
                add_dt = datetime.fromisoformat(last_daily_add.replace("Z", "+00:00"))
                next_daily = add_dt + timedelta(days=1)
                extra["daily_resets_at"] = int(next_daily.timestamp())
        except Exception:
            pass

    # 计算实际可用余额
    if weekly_limit is not None:
        weekly_remaining = max(0, weekly_limit - weekly_spent)
        subscription_available = min(subscription, weekly_remaining)
        extra["daily_spent"] = daily_balance - min(daily_balance, subscription_available)
    else:
        subscription_available = subscription
        extra["daily_spent"] = max(0, daily_balance - subscription)

    extra["_subscription_available"] = subscription_available
    extra["_total_available"] = pay_as_you_go + subscription_available

    return extra


class YesCodeBalanceAction(BalanceAction):
    """
    YesCode 专用余额查询

    特点：
    - 余额单位直接是美元
    - 支持每周限额查询
    - 同时调用 balance 和 profile 接口获取完整数据
    - Cookie 失效时返回友好的错误提示
    """

    display_name = "查询余额（含每周限额）"
    description = "查询账户余额和每周限额信息"

    _cookie_auth = True

    def _parse_balance(self, data: Any) -> BalanceInfo:
        """YesCode 不使用基类的 _do_query_balance，此方法不会被调用"""
        raise NotImplementedError("YesCode 使用自定义 _do_query_balance")

    async def _do_query_balance(self, client: httpx.AsyncClient) -> ActionResult:
        """执行余额查询（实现抽象方法，复用 client 调用两个接口获取完整数据）"""
        import time

        start_time = time.time()
        base_url = str(client.base_url).rstrip("/")

        try:
            # 复用传入的 client 获取合并数据
            combined_data = await fetch_yescode_combined_data(client, base_url)
            response_time_ms = int((time.time() - start_time) * 1000)

            # 检查是否至少有一个接口成功
            if "_balance_data" not in combined_data and "_profile_data" not in combined_data:
                return self._make_error_result(
                    ActionStatus.AUTH_FAILED,
                    "Cookie 已失效，请重新配置",
                )

            # 使用公共函数解析余额
            extra = parse_yescode_balance_extra(combined_data)

            total_available = extra.pop("_total_available", 0)
            extra.pop("_subscription_available", None)

            balance = BalanceInfo(
                total_granted=None,
                total_used=None,
                total_available=total_available,
                currency=self.config.get("currency", "USD"),
                extra=extra if extra else {},
            )

            return self._make_success_result(
                data=balance,
                response_time_ms=response_time_ms,
                raw_response=combined_data,
            )

        except httpx.TimeoutException:
            return self._make_error_result(
                ActionStatus.NETWORK_ERROR,
                "请求超时",
                retry_after_seconds=30,
            )
        except httpx.RequestError as e:
            return self._make_error_result(
                ActionStatus.NETWORK_ERROR,
                f"网络错误: {str(e)}",
                retry_after_seconds=30,
            )
        except Exception as e:
            return self._make_error_result(
                ActionStatus.UNKNOWN_ERROR,
                f"未知错误: {str(e)}",
            )
