"""
计费计算器

配置驱动的计费计算，支持：
- 固定价格计费
- 阶梯计费
- 多种计费模板
- 自定义计费维度
"""

from __future__ import annotations

from typing import Any

from src.services.billing.models import (
    BillingDimension,
    CostBreakdown,
    StandardizedUsage,
)
from src.services.billing.templates import (
    BillingTemplates,
    get_template,
)


class BillingCalculator:
    """
    配置驱动的计费计算器

    支持多种计费模式：
    - 使用预定义模板（claude, openai, doubao 等）
    - 自定义计费维度
    - 阶梯计费

    示例:
        # 使用模板
        calculator = BillingCalculator(template="openai")

        # 自定义维度
        calculator = BillingCalculator(dimensions=[
            BillingDimension(name="input", usage_field="input_tokens", price_field="input_price_per_1m"),
            BillingDimension(name="output", usage_field="output_tokens", price_field="output_price_per_1m"),
        ])

        # 计算费用
        usage = StandardizedUsage(input_tokens=1000, output_tokens=500)
        prices = {"input_price_per_1m": 3.0, "output_price_per_1m": 15.0}
        result = calculator.calculate(usage, prices)
    """

    def __init__(
        self,
        dimensions: list[BillingDimension] | None = None,
        template: str | None = None,
    ):
        """
        初始化计费计算器

        Args:
            dimensions: 自定义计费维度列表（优先级高于模板）
            template: 使用预定义模板名称 ("claude", "openai", "doubao", "per_request" 等)
        """
        if dimensions:
            self.dimensions = dimensions
        elif template:
            self.dimensions = get_template(template)
        else:
            # 默认使用 Claude 模板（向后兼容）
            self.dimensions = BillingTemplates.CLAUDE_STANDARD

        self.template_name = template

    def calculate(
        self,
        usage: StandardizedUsage,
        prices: dict[str, float],
        tiered_pricing: dict[str, Any] | None = None,
        cache_ttl_minutes: int | None = None,
        total_input_context: int | None = None,
    ) -> CostBreakdown:
        """
        计算费用

        Args:
            usage: 标准化的 usage 数据
            prices: 价格配置 {"input_price_per_1m": 3.0, "output_price_per_1m": 15.0, ...}
            tiered_pricing: 阶梯计费配置（可选）
            cache_ttl_minutes: 缓存 TTL 分钟数（用于 TTL 差异化定价）
            total_input_context: 总输入上下文（用于阶梯判定，可选）
                如果提供，将使用该值进行阶梯判定；否则使用默认计算逻辑

        Returns:
            费用明细 (CostBreakdown)
        """
        result = CostBreakdown()

        # 处理阶梯计费
        effective_prices = prices.copy()
        if tiered_pricing and tiered_pricing.get("tiers"):
            tier, tier_index = self._get_tier(usage, tiered_pricing, total_input_context)
            if tier:
                result.tier_index = tier_index
                # 阶梯价格覆盖默认价格
                for key, value in tier.items():
                    if key not in ("up_to", "cache_ttl_pricing") and value is not None:
                        effective_prices[key] = value

                # 处理 TTL 差异化定价
                if cache_ttl_minutes is not None:
                    ttl_price = self._get_cache_read_price_for_ttl(tier, cache_ttl_minutes)
                    if ttl_price is not None:
                        effective_prices["cache_read_price_per_1m"] = ttl_price

        # 记录使用的价格
        result.effective_prices = effective_prices.copy()

        # 计算各维度费用
        total = 0.0
        for dim in self.dimensions:
            usage_value = usage.get(dim.usage_field, 0)
            price = effective_prices.get(dim.price_field, dim.default_price)

            if usage_value and price:
                cost = dim.calculate(usage_value, price)
                result.costs[dim.name] = cost
                total += cost

        result.total_cost = total
        return result

    def _get_tier(
        self,
        usage: StandardizedUsage,
        tiered_pricing: dict[str, Any],
        total_input_context: int | None = None,
    ) -> tuple[dict[str, Any] | None, int | None]:
        """
        确定价格阶梯

        Args:
            usage: usage 数据
            tiered_pricing: 阶梯配置 {"tiers": [...]}
            total_input_context: 预计算的总输入上下文（可选）

        Returns:
            (匹配的阶梯配置, 阶梯索引)
        """
        tiers = tiered_pricing.get("tiers", [])
        if not tiers:
            return None, None

        # 使用传入的 total_input_context，或者默认计算
        if total_input_context is None:
            total_input_context = self._compute_total_input_context(usage)

        for i, tier in enumerate(tiers):
            up_to = tier.get("up_to")
            # up_to 为 None 表示无上限（最后一个阶梯）
            if up_to is None or total_input_context <= up_to:
                return tier, i

        # 如果所有阶梯都有上限且都超过了，返回最后一个阶梯
        return tiers[-1], len(tiers) - 1

    def _compute_total_input_context(self, usage: StandardizedUsage) -> int:
        """
        计算总输入上下文（用于阶梯计费判定）

        默认: input_tokens + cache_read_tokens

        Args:
            usage: usage 数据

        Returns:
            总输入 token 数
        """
        return usage.input_tokens + usage.cache_read_tokens

    def _get_cache_read_price_for_ttl(
        self,
        tier: dict[str, Any],
        cache_ttl_minutes: int,
    ) -> float | None:
        """
        根据缓存 TTL 获取缓存读取价格

        某些厂商（如 Claude）对不同 TTL 的缓存有不同定价。

        Args:
            tier: 当前阶梯配置
            cache_ttl_minutes: 缓存时长（分钟）

        Returns:
            缓存读取价格，如果没有 TTL 差异化配置返回 None
        """
        ttl_pricing = tier.get("cache_ttl_pricing")
        if not ttl_pricing:
            return None

        sorted_pricing = sorted(
            (item for item in ttl_pricing if isinstance(item, dict)),
            key=lambda item: float(item.get("ttl_minutes", 0) or 0),
        )
        if not sorted_pricing:
            return None

        for ttl_config in sorted_pricing:
            ttl_value = int(ttl_config.get("ttl_minutes", 0) or 0)
            if cache_ttl_minutes == ttl_value:
                price = ttl_config.get("cache_read_price_per_1m")
                return float(price) if price is not None else None

        matched_price = None
        for ttl_config in sorted_pricing:
            ttl_value = int(ttl_config.get("ttl_minutes", 0) or 0)
            if ttl_value <= cache_ttl_minutes:
                matched_price = ttl_config.get("cache_read_price_per_1m")

        if matched_price is not None:
            return float(matched_price)

        return None

    @classmethod
    def from_config(cls, config: dict[str, Any]) -> BillingCalculator:
        """
        从配置创建计费计算器

        Config 格式:
        {
            "template": "claude",  # 或 "openai", "doubao", "per_request"
            # 或者自定义维度:
            "dimensions": [
                {"name": "input", "usage_field": "input_tokens", "price_field": "input_price_per_1m"},
                ...
            ]
        }

        Args:
            config: 配置字典

        Returns:
            BillingCalculator 实例
        """
        if "dimensions" in config:
            dimensions = [BillingDimension.from_dict(d) for d in config["dimensions"]]
            return cls(dimensions=dimensions)

        return cls(template=config.get("template", "claude"))

    def get_dimension_names(self) -> list[str]:
        """获取所有计费维度名称"""
        return [dim.name for dim in self.dimensions]

    def get_required_price_fields(self) -> list[str]:
        """获取所需的价格字段名称"""
        return [dim.price_field for dim in self.dimensions]

    def get_required_usage_fields(self) -> list[str]:
        """获取所需的 usage 字段名称"""
        return [dim.usage_field for dim in self.dimensions]


def calculate_request_cost(
    input_tokens: int,
    output_tokens: int,
    cache_creation_input_tokens: int,
    cache_read_input_tokens: int,
    input_price_per_1m: float,
    output_price_per_1m: float,
    cache_creation_price_per_1m: float | None,
    cache_read_price_per_1m: float | None,
    price_per_request: float | None,
    tiered_pricing: dict[str, Any] | None = None,
    cache_ttl_minutes: int | None = None,
    total_input_context: int | None = None,
    billing_template: str = "claude",
) -> dict[str, Any]:
    """
    计算请求成本的便捷函数

    封装了 BillingCalculator 的调用逻辑，返回兼容旧格式的字典。

    Args:
        input_tokens: 输入 token 数
        output_tokens: 输出 token 数
        cache_creation_input_tokens: 缓存创建 token 数
        cache_read_input_tokens: 缓存读取 token 数
        input_price_per_1m: 输入价格（每 1M tokens）
        output_price_per_1m: 输出价格（每 1M tokens）
        cache_creation_price_per_1m: 缓存创建价格（每 1M tokens）
        cache_read_price_per_1m: 缓存读取价格（每 1M tokens）
        price_per_request: 按次计费价格
        tiered_pricing: 阶梯计费配置
        cache_ttl_minutes: 缓存时长（分钟）
        total_input_context: 总输入上下文（用于阶梯判定）
        billing_template: 计费模板名称

    Returns:
        包含各项成本的字典：
        {
            "input_cost": float,
            "output_cost": float,
            "cache_creation_cost": float,
            "cache_read_cost": float,
            "cache_cost": float,
            "request_cost": float,
            "total_cost": float,
            "tier_index": int | None,
        }
    """
    # 构建标准化 usage
    usage = StandardizedUsage(
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        cache_creation_tokens=cache_creation_input_tokens,
        cache_read_tokens=cache_read_input_tokens,
        request_count=1,
    )

    # 构建价格配置
    prices: dict[str, float] = {
        "input_price_per_1m": input_price_per_1m,
        "output_price_per_1m": output_price_per_1m,
    }
    if cache_creation_price_per_1m is not None:
        prices["cache_creation_price_per_1m"] = cache_creation_price_per_1m
    if cache_read_price_per_1m is not None:
        prices["cache_read_price_per_1m"] = cache_read_price_per_1m
    if price_per_request is not None:
        prices["price_per_request"] = price_per_request

    # 使用 BillingCalculator 计算
    calculator = BillingCalculator(template=billing_template)
    result = calculator.calculate(
        usage, prices, tiered_pricing, cache_ttl_minutes, total_input_context
    )

    # 返回兼容旧格式的字典
    return {
        "input_cost": result.input_cost,
        "output_cost": result.output_cost,
        "cache_creation_cost": result.cache_creation_cost,
        "cache_read_cost": result.cache_read_cost,
        "cache_cost": result.cache_cost,
        "request_cost": result.request_cost,
        "total_cost": result.total_cost,
        "tier_index": result.tier_index,
    }
