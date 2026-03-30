# 支付通道接入指南

这份文档用于说明如何在当前项目里新增一个真实支付通道，例如微信支付、Crypto 支付，或者其他聚合支付。

当前支付网关抽象位于：

- `src/services/payment/gateway/base.py`
- `src/services/payment/gateway/__init__.py`
- `src/services/payment/gateway/registry.py`

现有示例实现：

- `src/services/payment/gateway/alipay.py`
- `src/services/payment/gateway/wechat.py`
- `src/services/payment/gateway/manual.py`

## 最小接入流程

如果你要新增一个支付通道，基本流程固定如下：

1. 新建一个 gateway 类
2. 声明 `capabilities`
3. 实现 `create_checkout()`
4. 如果支持主动查询，实现 `query_order_status()`，并尽量复用 `build_status_snapshot()`
5. 如果支持回调，解析时尽量复用 `build_parsed_callback_payload()`
6. 注册到 `src/services/payment/gateway/__init__.py`

## 第一步：新建 Gateway 类

在 `src/services/payment/gateway/` 下新增一个文件，例如：

- `src/services/payment/gateway/crypto.py`
- `src/services/payment/gateway/provider_x.py`

基础结构建议如下：

```python
from __future__ import annotations

from typing import Any

from src.services.payment.gateway.base import (
    CheckoutPayload,
    PaymentGateway,
    PaymentGatewayCapabilities,
)


class CryptoGateway(PaymentGateway):
    payment_method = "crypto"
    display_name = "Crypto"
    capabilities = PaymentGatewayCapabilities(
        supports_webhook=True,
        supports_active_query=True,
        supports_return_url=False,
        supports_refund=False,
        checkout_mode="redirect",
    )

    def create_checkout(self, *, order: Any) -> CheckoutPayload:
        ...
```

要求：

- `payment_method` 必须唯一，并且要和数据库/前端使用的支付方式编码一致
- `display_name` 是后台和前端展示用名称
- 类必须继承 `PaymentGateway`

## 第二步：声明 capabilities

每个通道都要明确自己的能力边界，统一写在 `capabilities` 里。

字段含义：

- `supports_webhook`：是否支持异步回调
- `supports_active_query`：是否支持主动查单
- `supports_return_url`：是否支持用户支付完成后的同步回跳
- `supports_refund`：是否支持原路退款
- `checkout_mode`：支付指引类型，当前常见值有 `redirect`、`qr_code`、`offline`

建议：

- 真实线上支付尽量让 `supports_active_query=True`
- 即使已经有 webhook，也建议支持主动查询，作为补偿机制
- 如果通道不支持同步回跳，`supports_return_url=False`

## 第三步：实现 create_checkout()

`create_checkout()` 的职责是生成统一的前端支付指引。

返回类型必须是 `CheckoutPayload`，核心字段包括：

- `gateway`
- `display_name`
- `gateway_order_id`
- `payment_url`
- `qr_code`
- `expires_at`
- `instructions`
- `raw`

推荐做法：

- 把第三方原始响应保留到 `raw`
- 如果通道是跳转支付，填充 `payment_url`
- 如果通道是扫码支付，填充 `qr_code`
- 如果是线下类支付，使用 `instructions`

可以参考：

- `src/services/payment/gateway/alipay.py`
- `src/services/payment/gateway/wechat.py`
- `src/services/payment/gateway/manual.py`

## 第四步：如果支持主动查询，实现 query_order_status()

如果通道支持查单，应该实现：

```python
def query_order_status(
    self,
    *,
    order_no: str,
    gateway_order_id: str | None = None,
) -> dict[str, Any]:
    ...
```

这个方法只负责：

- 调用第三方查单接口
- 返回原始响应或标准化前的响应

然后再通过 `normalize_query_response()` 把第三方响应转换成统一状态。

强烈建议在 `normalize_query_response()` 里复用：

- `build_status_snapshot()`

推荐模式：

```python
def normalize_query_response(
    self,
    *,
    order: Any,
    query_response: dict[str, Any],
) -> PaymentStatusSnapshot:
    channel_status = query_response.get("trade_status")
    return self.build_status_snapshot(
        channel_status=channel_status,
        gateway_order_id=query_response.get("trade_no"),
        pay_amount=query_response.get("total_amount"),
        pay_currency="CNY",
        exchange_rate="1",
        raw=query_response,
    )
```

`build_status_snapshot()` 会帮你统一处理：

- `channel_status`
- `order_status`
- `gateway_order_id`
- `pay_amount`
- `pay_currency`
- `exchange_rate`
- `error`
- `raw`

另外请正确配置这些状态映射：

- `success_channel_statuses`
- `expired_channel_statuses`
- `failed_channel_statuses`

这样 `map_channel_status_to_order_status()` 就可以自动把第三方状态映射为项目内部状态，例如：

- `paid`
- `expired`
- `failed`
- `pending`

## 第五步：如果支持回调，解析时尽量复用 build_parsed_callback_payload()

如果通道支持 webhook，通常需要实现：

- `verify_callback_payload()`
- `parse_callback_payload()`

其中：

- `verify_callback_payload()` 负责验签
- `parse_callback_payload()` 负责把第三方回调结构映射成项目统一结构

推荐在 `parse_callback_payload()` 中复用：

- `build_parsed_callback_payload()`

推荐模式：

```python
def parse_callback_payload(self, *, payload: dict[str, Any] | None) -> ParsedCallbackPayload:
    data = dict(payload or {})
    return self.build_parsed_callback_payload(
        callback_key=f"crypto_{data.get('event_id') or data.get('id')}",
        order_no=data.get("merchant_order_no"),
        gateway_order_id=data.get("transaction_id"),
        pay_amount=str(data.get("amount")) if data.get("amount") is not None else None,
        pay_currency=data.get("currency"),
        exchange_rate=data.get("exchange_rate"),
        channel_status=data.get("status"),
        raw=data,
    )
```

`build_parsed_callback_payload()` 会统一处理：

- `callback_key`
- `order_no`
- `gateway_order_id`
- `pay_amount`
- `pay_currency`
- `exchange_rate`
- `order_status`
- `channel_status`
- `raw`

如果你已经正确设置了状态映射，它还能自动推导 `order_status`。

## 第六步：注册到 __init__.py

新增 gateway 后，必须注册到全局注册表，否则业务层无法按 `payment_method` 找到它。

当前注册入口：

- `src/services/payment/gateway/__init__.py`

示例：

```python
from src.services.payment.gateway.crypto import CryptoGateway

_REGISTRY = PaymentGatewayRegistry()
for _gateway in (AlipayGateway(), WeChatGateway(), ManualGateway(), CryptoGateway()):
    _REGISTRY.register(_gateway)
```

## 建议同时补齐的内容

除了 gateway 类本身，真实接入时通常还需要同步检查下面这些地方：

### 1. 配置项

如果新通道需要密钥、商户号、回调地址、网关域名等配置，需要补到配置系统中，例如：

- `src/config/settings.py`
- `.env`
- 部署环境变量

### 2. 支付方式枚举与展示

如果前端或后台要展示这个支付方式，通常还要补：

- `frontend/src/utils/walletDisplay.ts`
- 管理端支付配置页面
- 钱包充值页支付方式下拉框

### 3. 管理端开关

如果系统支持“按通道启用/关闭”，要确认新通道已经纳入：

- 充值全局开关
- 单通道开关
- 最小/最大金额校验

### 4. 定时主动查询

如果通道支持主动查询，建议确认它已经被定时补偿任务覆盖。

重点是：

- 只查询待支付订单
- 采用退避策略，不要无限频繁查询
- 到达过期时间后停止无意义查单

### 5. 回跳体验

如果通道支持 return URL，前端钱包页最好能：

- 识别支付回跳参数
- 刷新订单状态
- 清理 URL 参数

## 最佳实践

- 优先复用 `build_status_snapshot()` 和 `build_parsed_callback_payload()`，不要在每个通道里重复拼统一结构
- 第三方原始响应尽量保存在 `raw`，便于排障
- 主动查询和 webhook 最好同时支持，不要只依赖单一链路
- `payment_method` 一旦上线，尽量不要随意改名，避免历史订单无法识别
- 对回调验签失败、查单异常、空响应等情况要保留清晰日志
- 新通道至少补一组单元测试和一组接口测试

## 推荐验收清单

新增一个真实支付通道后，至少确认以下链路：

1. 可以成功创建充值订单
2. 前端能拿到正确的支付链接或二维码
3. 用户支付后，webhook 能正确更新订单
4. webhook 不可达时，主动查询可以补偿成功
5. 钱包页和管理端能正确显示订单状态
6. 订单过期、失败、重复回调场景不会写坏状态

## 可直接参考的实现

- 支付宝真实接入：`src/services/payment/gateway/alipay.py`
- 微信占位实现：`src/services/payment/gateway/wechat.py`
- 人工通道占位实现：`src/services/payment/gateway/manual.py`
- 网关抽象基类：`src/services/payment/gateway/base.py`
- 网关注册表：`src/services/payment/gateway/registry.py`
