# 支付（/api/admin/payments）

> 家族：`payments_manage` · 权限域：`admin:payments`
> 说明：订单 / 回调 / 兑换码批次 / 网关
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/payments/callbacks` | `list_callbacks` |
| GET | `/api/admin/payments/gateways/epay` | `get_epay_gateway` |
| PUT | `/api/admin/payments/gateways/epay` | `update_epay_gateway` |
| POST | `/api/admin/payments/gateways/epay/test` | `test_epay_gateway` |
| GET | `/api/admin/payments/gateways/{provider}` | `get_payment_gateway` |
| PUT | `/api/admin/payments/gateways/{provider}` | `update_payment_gateway` |
| POST | `/api/admin/payments/gateways/{provider}/test` | `test_payment_gateway` |
| GET | `/api/admin/payments/orders` | `list_orders` |
| GET | `/api/admin/payments/orders/{gateway_provider}` | `get_order` |
| POST | `/api/admin/payments/orders/{gateway_provider}/credit` | `credit_order` |
| POST | `/api/admin/payments/orders/{gateway_provider}/expire` | `expire_order` |
| POST | `/api/admin/payments/orders/{gateway_provider}/fail` | `fail_order` |
| GET | `/api/admin/payments/redeem-codes/batches` | `list_redeem_code_batches` |
| POST | `/api/admin/payments/redeem-codes/batches` | `create_redeem_code_batch` |
| GET | `/api/admin/payments/redeem-codes/batches/{gateway_provider}` | `get_redeem_code_batch` |
| GET | `/api/admin/payments/redeem-codes/batches/{gateway_provider}/codes` | `list_redeem_codes` |
| POST | `/api/admin/payments/redeem-codes/batches/{gateway_provider}/delete` | `delete_redeem_code_batch` |
| POST | `/api/admin/payments/redeem-codes/batches/{gateway_provider}/disable` | `disable_redeem_code_batch` |
| POST | `/api/admin/payments/redeem-codes/codes/{gateway_provider}/disable` | `disable_redeem_code` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/payments/orders?status=paid -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/payments/redeem-codes/batches \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"name":"batch-1","amount_usd":5.0,"total_count":100}'
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/payments/gateways/epay -H "Authorization: Bearer ae-xxx"
```
