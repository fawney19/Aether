# 推荐奖励（/api/admin/referrals）

> 家族：`referrals_manage` · 权限域：`admin:referrals`
> 说明：推荐 / 奖励列表与处理
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/referral-rewards` | `list_referral_rewards` |
| POST | `/api/admin/referral-rewards/{id}/retry` | `retry_referral_reward` |
| POST | `/api/admin/referral-rewards/{id}/void` | `void_referral_reward` |
| GET | `/api/admin/referrals` | `list_referrals` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/referrals -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/referral-rewards?status=pending -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/referral-rewards/{id}/retry -H "Authorization: Bearer ae-xxx"
```
