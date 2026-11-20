# 钱包（/api/admin/wallets）

> 家族：`wallets_manage` · 权限域：`admin:wallets`
> 说明：钱包列表/详情/台账/充值/调账/退款
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/wallets` | `list_wallets` |
| GET | `/api/admin/wallets/ledger` | `ledger` |
| GET | `/api/admin/wallets/refund-requests` | `list_refund_requests` |
| GET | `/api/admin/wallets/{id}` | `wallet_detail` |
| POST | `/api/admin/wallets/{id}/adjust` | `adjust_balance` |
| POST | `/api/admin/wallets/{id}/recharge` | `recharge_balance` |
| GET | `/api/admin/wallets/{id}/transactions` | `list_wallet_transactions` |
| GET | `/api/admin/wallets/{wallet_id}/refunds` | `list_wallet_refunds` |
| POST | `/api/admin/wallets/{wallet_id}/refunds/{refund_id}/complete` | `complete_refund` |
| POST | `/api/admin/wallets/{wallet_id}/refunds/{refund_id}/fail` | `fail_refund` |
| POST | `/api/admin/wallets/{wallet_id}/refunds/{refund_id}/process` | `process_refund` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/wallets?limit=20 -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/wallets/{wallet_id}/recharge \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"amount_usd":10.0,"payment_method":"manual","description":"充值"}'
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/wallets/{wallet_id}/refunds/{refund_id}/process \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{}'
```
