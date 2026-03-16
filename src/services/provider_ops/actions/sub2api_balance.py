"""
Sub2API 余额查询操作
"""

import asyncio
import time
from typing import Any, cast

import httpx

from src.services.provider_ops.actions.balance import BalanceAction
from src.services.provider_ops.types import ActionResult, ActionStatus, BalanceInfo


class Sub2ApiBalanceAction(BalanceAction):
    """
    Sub2API 余额查询

    特点：
    - 并发调用 /api/v1/auth/me 和 /api/v1/subscriptions/summary
    - auth/me 提供基础余额（balance + points）
    - subscriptions/summary 提供订阅详情（各订阅的日/周/月用量和额度）
    - 响应格式: {"code": 0, "message": "success", "data": {...}}
    """

    display_name = "查询余额"
    description = "查询 Sub2API 账户余额和订阅信息"

    def _parse_balance(self, data: Any) -> BalanceInfo:
        """本类完全重写了 _do_query_balance，绕过基类默认流程，故此方法不会被调用"""
        raise NotImplementedError(
            "Sub2API 重写了 _do_query_balance，不走基类的 _parse_balance 路径"
        )

    async def _do_query_balance(self, client: httpx.AsyncClient) -> ActionResult:
        """并发查询 auth/me 和 subscriptions/summary"""
        start_time = time.time()

        me_endpoint = self.config.get("endpoint", "/api/v1/auth/me?timezone=Asia/Shanghai")
        sub_endpoint = self.config.get("subscription_endpoint", "/api/v1/subscriptions/summary")

        try:
            me_resp, sub_resp = cast(
                tuple[httpx.Response | BaseException, httpx.Response | BaseException],
                await asyncio.gather(
                    client.get(me_endpoint),
                    client.get(sub_endpoint),
                    return_exceptions=True,
                ),
            )
            response_time_ms = int((time.time() - start_time) * 1000)

            # 解析 auth/me
            me_data: dict[str, Any] = {}
            me_ok = False
            if isinstance(me_resp, httpx.Response):
                if me_resp.status_code in (401, 403):
                    return self._make_error_result(
                        ActionStatus.AUTH_FAILED, "认证失败，请检查凭据配置"
                    )
                if me_resp.status_code == 200:
                    try:
                        me_json = me_resp.json()
                        if me_json.get("code") == 0:
                            me_data = me_json.get("data", {})
                            me_ok = True
                    except Exception:
                        pass

            if not me_ok:
                return self._make_error_result(ActionStatus.UNKNOWN_ERROR, "查询用户信息失败")

            # 基础余额
            balance = self._to_float(me_data.get("balance")) or 0.0
            points = self._to_float(me_data.get("points")) or 0.0
            total_available = balance + points

            extra: dict[str, Any] = {
                "balance": balance,
                "points": points,
            }

            # 解析 subscriptions/summary（可选，失败不影响主流程）
            if isinstance(sub_resp, httpx.Response) and sub_resp.status_code == 200:
                try:
                    sub_json = sub_resp.json()
                    if sub_json.get("code") == 0:
                        summary = sub_json.get("data", {})
                        extra["active_subscriptions"] = summary.get("active_count", 0)
                        extra["total_used_usd"] = summary.get("total_used_usd", 0)
                        extra["subscriptions"] = self._parse_subscriptions(
                            summary.get("subscriptions", [])
                        )
                except Exception:
                    pass

            balance_info = self._create_balance_info(
                total_available=total_available,
                currency="USD",
                extra=extra,
            )

            return self._make_success_result(
                data=balance_info,
                response_time_ms=response_time_ms,
                raw_response={"me": me_data},
            )

        except httpx.TimeoutException:
            return self._make_error_result(
                ActionStatus.NETWORK_ERROR, "请求超时", retry_after_seconds=30
            )
        except httpx.RequestError as e:
            return self._make_error_result(
                ActionStatus.NETWORK_ERROR, f"网络错误: {e}", retry_after_seconds=30
            )
        except Exception as e:
            return self._make_error_result(ActionStatus.UNKNOWN_ERROR, f"未知错误: {e}")

    @staticmethod
    def _parse_subscriptions(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
        """将订阅列表精简为前端需要的字段"""
        result = []
        for item in items:
            sub: dict[str, Any] = {
                "group_name": item.get("group_name", ""),
                "status": item.get("status", ""),
            }
            # 只保留非 None 的限额字段（0 是有效值，表示未使用/无限额）
            for field in (
                "daily_used_usd",
                "daily_limit_usd",
                "weekly_used_usd",
                "weekly_limit_usd",
                "monthly_used_usd",
                "monthly_limit_usd",
            ):
                val = item.get(field)
                if val is not None:
                    sub[field] = val
            expires_at = item.get("expires_at")
            if expires_at:
                sub["expires_at"] = expires_at
            result.append(sub)
        return result
