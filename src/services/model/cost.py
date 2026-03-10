"""
模型成本服务
负责统一的价格解析、缓存以及成本计算逻辑。
支持固定价格、按次计费和阶梯计费三种模式。

计费策略：
- 价格来源仍由 ModelCostService 解析
- 格式相关口径（计费模板、总输入上下文）统一由 core.api_format.capabilities 提供
"""

from __future__ import annotations

from dataclasses import dataclass

from sqlalchemy.orm import Session

from src.core.api_format.capabilities import (
    compute_total_input_context_for_api_format,
    resolve_billing_template_for_api_format,
)
from src.core.logger import logger
from src.models.database import GlobalModel, Model, Provider
from src.services.billing import calculate_request_cost

ProviderRef = str | Provider | None


@dataclass
class TieredPriceResult:
    """阶梯计费价格查询结果"""

    input_price_per_1m: float
    output_price_per_1m: float
    cache_creation_price_per_1m: float | None = None
    cache_read_price_per_1m: float | None = None
    tier_index: int = 0  # 命中的阶梯索引


@dataclass
class CostBreakdown:
    """成本明细"""

    input_cost: float
    output_cost: float
    cache_creation_cost: float
    cache_read_cost: float
    cache_cost: float
    request_cost: float
    total_cost: float


class ModelCostService:
    """集中负责模型价格与成本计算，避免在 mapper/usage 中重复实现。"""

    _price_cache: dict[str, dict[str, float]] = {}
    _cache_price_cache: dict[str, dict[str, float]] = {}
    _tiered_pricing_cache: dict[str, dict | None] = {}
    _MAX_CACHE_SIZE: int = 500  # 每层缓存上限，防止无界增长

    def __init__(self, db: Session):
        self.db = db

    # ------------------------------------------------------------------
    # 阶梯计费相关方法
    # ------------------------------------------------------------------

    @staticmethod
    def get_tier_for_tokens(tiered_pricing: dict, total_input_tokens: int) -> dict | None:
        """
        根据总输入 token 数确定价格阶梯。

        Args:
            tiered_pricing: 阶梯计费配置 {"tiers": [...]}
            total_input_tokens: 总输入 token 数（input_tokens + cache_read_tokens）

        Returns:
            匹配的阶梯配置，如果未找到返回 None
        """
        if not tiered_pricing or "tiers" not in tiered_pricing:
            return None

        tiers = tiered_pricing.get("tiers", [])
        if not tiers:
            return None

        for tier in tiers:
            up_to = tier.get("up_to")
            if up_to is None or total_input_tokens <= up_to:
                return tier

        # 如果所有阶梯都有上限且都超过了，返回最后一个阶梯
        return tiers[-1] if tiers else None

    @staticmethod
    def get_cache_read_price_for_ttl(
        tier: dict, cache_ttl_minutes: int | None = None
    ) -> float | None:
        """
        根据缓存 TTL 获取缓存读取价格。

        Args:
            tier: 当前阶梯配置
            cache_ttl_minutes: 缓存时长（分钟），如果为 None 使用默认价格

        Returns:
            缓存读取价格
        """
        # 首先检查是否有 TTL 差异化定价
        ttl_pricing = tier.get("cache_ttl_pricing")
        if ttl_pricing and cache_ttl_minutes is not None:
            # 找到匹配或最接近的 TTL 价格
            matched_price = None
            for ttl_config in ttl_pricing:
                ttl_limit = ttl_config.get("ttl_minutes", 0)
                if cache_ttl_minutes <= ttl_limit:
                    matched_price = ttl_config.get("cache_read_price_per_1m")
                    break
            if matched_price is not None:
                return matched_price
            # 如果超过所有配置的 TTL，使用最后一个
            if ttl_pricing:
                return ttl_pricing[-1].get("cache_read_price_per_1m")

        # 使用默认的缓存读取价格
        return tier.get("cache_read_price_per_1m")

    async def get_tiered_pricing_async(self, provider: ProviderRef, model: str) -> dict | None:
        """
        异步获取模型的阶梯计费配置。

        Args:
            provider: Provider 对象或提供商名称
            model: 模型名称

        Returns:
            阶梯计费配置，如果未配置返回 None
        """
        result = await self.get_tiered_pricing_with_source_async(provider, model)
        return result.get("pricing") if result else None

    async def get_tiered_pricing_with_source_async(
        self, provider: ProviderRef, model: str
    ) -> dict | None:
        """
        异步获取模型的阶梯计费配置及来源信息。

        Args:
            provider: Provider 对象或提供商名称
            model: 模型名称

        Returns:
            包含 pricing 和 source 的字典:
            - pricing: 阶梯计费配置
            - source: 'provider' 或 'global'
        """
        provider_name = self._provider_name(provider)
        cache_key = f"{provider_name}:{model}:tiered_with_source"

        if cache_key in self._tiered_pricing_cache:
            return self._tiered_pricing_cache[cache_key]

        provider_obj = self._resolve_provider(provider)
        result = None

        if provider_obj:
            # 直接通过 GlobalModel.name 查找
            global_model = (
                self.db.query(GlobalModel)
                .filter(
                    GlobalModel.name == model,
                    GlobalModel.is_active == True,
                )
                .first()
            )

            if global_model:
                model_obj = (
                    self.db.query(Model)
                    .filter(
                        Model.provider_id == provider_obj.id,
                        Model.global_model_id == global_model.id,
                        Model.is_active == True,
                    )
                    .first()
                )

                if model_obj:
                    # 判断定价来源
                    if model_obj.tiered_pricing is not None:
                        result = {"pricing": model_obj.tiered_pricing, "source": "provider"}
                    elif global_model.default_tiered_pricing is not None:
                        result = {
                            "pricing": global_model.default_tiered_pricing,
                            "source": "global",
                        }
                else:
                    # Provider 没有实现该模型，直接使用 GlobalModel 的默认阶梯配置
                    if global_model.default_tiered_pricing is not None:
                        result = {
                            "pricing": global_model.default_tiered_pricing,
                            "source": "global",
                        }

        self._evict_if_full(self._tiered_pricing_cache)
        self._tiered_pricing_cache[cache_key] = result
        return result

    def get_tiered_pricing(self, provider: ProviderRef, model: str) -> dict | None:
        """同步获取模型的阶梯计费配置（直接查缓存和数据库，避免事件循环开销）。

        Returns:
            阶梯计费配置字典，如果未找到配置则返回 None。
            注意：与 get_tiered_pricing_async 不同，此方法仅返回 pricing 部分，不含 source 字段。
        """
        provider_name = self._provider_name(provider)
        cache_key = f"{provider_name}:{model}:tiered_with_source"

        # 优先从缓存获取
        if cache_key in self._tiered_pricing_cache:
            result = self._tiered_pricing_cache[cache_key]
            return result.get("pricing") if result else None

        # 缓存未命中，同步查询数据库
        provider_obj = self._resolve_provider(provider)
        result = None

        if provider_obj:
            global_model = (
                self.db.query(GlobalModel)
                .filter(
                    GlobalModel.name == model,
                    GlobalModel.is_active == True,
                )
                .first()
            )

            if global_model:
                model_obj = (
                    self.db.query(Model)
                    .filter(
                        Model.provider_id == provider_obj.id,
                        Model.global_model_id == global_model.id,
                        Model.is_active == True,
                    )
                    .first()
                )

                if model_obj:
                    if model_obj.tiered_pricing is not None:
                        result = {"pricing": model_obj.tiered_pricing, "source": "provider"}
                    elif global_model.default_tiered_pricing is not None:
                        result = {
                            "pricing": global_model.default_tiered_pricing,
                            "source": "global",
                        }
                else:
                    if global_model.default_tiered_pricing is not None:
                        result = {
                            "pricing": global_model.default_tiered_pricing,
                            "source": "global",
                        }

        self._evict_if_full(self._tiered_pricing_cache)
        self._tiered_pricing_cache[cache_key] = result
        return result.get("pricing") if result else None

    # ------------------------------------------------------------------
    # 公共方法
    # ------------------------------------------------------------------

    async def get_model_price_async(self, provider: ProviderRef, model: str) -> tuple[float, float]:
        """
        异步版本: 返回给定 provider/model 的 (input_price, output_price)。

        注意：如果模型配置了阶梯计费，此方法返回第一个阶梯的价格作为默认值。
        实际计费时应使用 compute_cost_with_tiered_pricing 方法。

        计费逻辑:
        1. 直接通过 GlobalModel.name 匹配
        2. 查找该 Provider 的 Model 实现
        3. 获取价格配置

        Args:
            provider: Provider 对象或提供商名称
            model: 用户请求的模型名（必须是 GlobalModel.name）

        Returns:
            (input_price, output_price) 元组
        """
        provider_name = self._provider_name(provider)
        cache_key = f"{provider_name}:{model}"

        if cache_key in self._price_cache:
            prices = self._price_cache[cache_key]
            return prices["input"], prices["output"]

        provider_obj = self._resolve_provider(provider)
        input_price = None
        output_price = None

        if provider_obj:
            # 直接通过 GlobalModel.name 查找（用户必须使用标准模型名称）
            global_model = (
                self.db.query(GlobalModel)
                .filter(
                    GlobalModel.name == model,
                    GlobalModel.is_active == True,
                )
                .first()
            )
            if global_model:
                model_obj = (
                    self.db.query(Model)
                    .filter(
                        Model.provider_id == provider_obj.id,
                        Model.global_model_id == global_model.id,
                        Model.is_active == True,
                    )
                    .first()
                )
                if model_obj:
                    # 检查是否有阶梯计费
                    tiered = model_obj.get_effective_tiered_pricing()
                    if tiered and tiered.get("tiers"):
                        first_tier = tiered["tiers"][0]
                        input_price = first_tier.get("input_price_per_1m", 0)
                        output_price = first_tier.get("output_price_per_1m", 0)
                    else:
                        input_price = model_obj.get_effective_input_price()
                        output_price = model_obj.get_effective_output_price()
                    logger.debug(
                        f"找到模型价格配置: {provider_name}/{model} "
                        f"(输入: ${input_price}/M, 输出: ${output_price}/M)"
                    )
                else:
                    # Provider 没有实现该模型，直接使用 GlobalModel 的默认价格
                    tiered = global_model.default_tiered_pricing
                    if tiered and tiered.get("tiers"):
                        first_tier = tiered["tiers"][0]
                        input_price = first_tier.get("input_price_per_1m", 0)
                        output_price = first_tier.get("output_price_per_1m", 0)
                    else:
                        input_price = 0.0
                        output_price = 0.0
                    logger.debug(
                        f"使用 GlobalModel 默认价格: {provider_name}/{model} "
                        f"(输入: ${input_price}/M, 输出: ${output_price}/M)"
                    )

        # 如果没有找到价格配置，使用 0.0 并记录警告
        if input_price is None:
            input_price = 0.0
        if output_price is None:
            output_price = 0.0

        # 检查是否有按次计费配置（按次计费模型的 token 价格可以为 0）
        if input_price == 0.0 and output_price == 0.0:
            # 异步检查按次计费价格
            price_per_request = await self.get_request_price_async(provider, model)
            if price_per_request is None or price_per_request == 0.0:
                logger.warning(
                    f"未找到模型价格配置: {provider_name}/{model}，请在 GlobalModel 中配置价格"
                )

        self._evict_if_full(self._price_cache)
        self._price_cache[cache_key] = {"input": input_price, "output": output_price}
        return input_price, output_price

    def get_model_price(self, provider: ProviderRef, model: str) -> tuple[float, float]:
        """
        返回给定 provider/model 的 (input_price, output_price)。
        直接查缓存和数据库，避免事件循环开销。

        Args:
            provider: Provider 对象或提供商名称
            model: 用户请求的模型名（必须是 GlobalModel.name）

        Returns:
            (input_price, output_price) 元组
        """
        provider_name = self._provider_name(provider)
        cache_key = f"{provider_name}:{model}"

        # 优先从缓存获取
        if cache_key in self._price_cache:
            prices = self._price_cache[cache_key]
            return prices["input"], prices["output"]

        # 缓存未命中，同步查询数据库
        provider_obj = self._resolve_provider(provider)
        input_price = None
        output_price = None

        if provider_obj:
            global_model = (
                self.db.query(GlobalModel)
                .filter(
                    GlobalModel.name == model,
                    GlobalModel.is_active == True,
                )
                .first()
            )
            if global_model:
                model_obj = (
                    self.db.query(Model)
                    .filter(
                        Model.provider_id == provider_obj.id,
                        Model.global_model_id == global_model.id,
                        Model.is_active == True,
                    )
                    .first()
                )
                if model_obj:
                    tiered = model_obj.get_effective_tiered_pricing()
                    if tiered and tiered.get("tiers"):
                        first_tier = tiered["tiers"][0]
                        input_price = first_tier.get("input_price_per_1m", 0)
                        output_price = first_tier.get("output_price_per_1m", 0)
                    else:
                        input_price = model_obj.get_effective_input_price()
                        output_price = model_obj.get_effective_output_price()
                else:
                    tiered = global_model.default_tiered_pricing
                    if tiered and tiered.get("tiers"):
                        first_tier = tiered["tiers"][0]
                        input_price = first_tier.get("input_price_per_1m", 0)
                        output_price = first_tier.get("output_price_per_1m", 0)
                    else:
                        input_price = 0.0
                        output_price = 0.0

        if input_price is None:
            input_price = 0.0
        if output_price is None:
            output_price = 0.0

        # 与异步版本保持一致：当 token 价格为 0 时，仅在没有按次计费配置时告警
        if input_price == 0.0 and output_price == 0.0:
            price_per_request = self.get_request_price(provider, model)
            if price_per_request is None or price_per_request == 0.0:
                logger.warning(
                    "未找到模型价格配置: {}/{}，请在 GlobalModel 中配置价格",
                    provider_name,
                    model,
                )

        self._evict_if_full(self._price_cache)
        self._price_cache[cache_key] = {"input": input_price, "output": output_price}
        return input_price, output_price

    async def get_cache_prices_async(
        self, provider: ProviderRef, model: str, input_price: float
    ) -> tuple[float | None, float | None]:
        """
        异步版本: 返回缓存创建/读取价格（每 1M tokens）。

        Args:
            provider: Provider 对象或提供商名称
            model: 用户请求的模型名（必须是 GlobalModel.name）
            input_price: 基础输入价格（用于 Claude 模型的默认估算）

        Returns:
            (cache_creation_price, cache_read_price) 元组
        """
        provider_name = self._provider_name(provider)
        cache_key = f"{provider_name}:{model}"

        if cache_key in self._cache_price_cache:
            prices = self._cache_price_cache[cache_key]
            return prices["creation"], prices["read"]

        provider_obj = self._resolve_provider(provider)
        cache_creation_price = None
        cache_read_price = None

        if provider_obj:
            # 直接通过 GlobalModel.name 查找（用户必须使用标准模型名称）
            global_model = (
                self.db.query(GlobalModel)
                .filter(
                    GlobalModel.name == model,
                    GlobalModel.is_active == True,
                )
                .first()
            )

            # 查找该 Provider 的 Model 实现
            if global_model:
                model_obj = (
                    self.db.query(Model)
                    .filter(
                        Model.provider_id == provider_obj.id,
                        Model.global_model_id == global_model.id,
                        Model.is_active == True,
                    )
                    .first()
                )

                if model_obj:
                    # 检查是否有阶梯计费配置
                    tiered = model_obj.get_effective_tiered_pricing()
                    if tiered and tiered.get("tiers"):
                        # 使用第一个阶梯的缓存价格作为默认值
                        first_tier = tiered["tiers"][0]
                        cache_creation_price = first_tier.get("cache_creation_price_per_1m")
                        cache_read_price = first_tier.get("cache_read_price_per_1m")
                    else:
                        # 使用 get_effective_* 方法，会自动回退到 GlobalModel 的默认值
                        cache_creation_price = model_obj.get_effective_cache_creation_price()
                        cache_read_price = model_obj.get_effective_cache_read_price()
                else:
                    # Provider 没有实现该模型，直接使用 GlobalModel 的默认价格
                    tiered = global_model.default_tiered_pricing
                    if tiered and tiered.get("tiers"):
                        first_tier = tiered["tiers"][0]
                        cache_creation_price = first_tier.get("cache_creation_price_per_1m")
                        cache_read_price = first_tier.get("cache_read_price_per_1m")
                    else:
                        cache_creation_price = None
                        cache_read_price = None

        # 默认缓存价格估算（如果没有配置）- 基于输入价格计算
        if cache_creation_price is None or cache_read_price is None:
            if cache_creation_price is None:
                cache_creation_price = input_price * 1.25
            if cache_read_price is None:
                cache_read_price = input_price * 0.1

        self._evict_if_full(self._cache_price_cache)
        self._cache_price_cache[cache_key] = {
            "creation": cache_creation_price,
            "read": cache_read_price,
        }
        return cache_creation_price, cache_read_price

    async def get_request_price_async(self, provider: ProviderRef, model: str) -> float | None:
        """
        异步版本: 返回按次计费价格（每次请求的固定费用）。

        Args:
            provider: Provider 对象或提供商名称
            model: 用户请求的模型名（必须是 GlobalModel.name）

        Returns:
            按次计费价格，如果没有配置则返回 None
        """
        provider_obj = self._resolve_provider(provider)
        price_per_request = None

        if provider_obj:
            # 直接通过 GlobalModel.name 查找（用户必须使用标准模型名称）
            global_model = (
                self.db.query(GlobalModel)
                .filter(
                    GlobalModel.name == model,
                    GlobalModel.is_active == True,
                )
                .first()
            )

            # 查找该 Provider 的 Model 实现
            if global_model:
                model_obj = (
                    self.db.query(Model)
                    .filter(
                        Model.provider_id == provider_obj.id,
                        Model.global_model_id == global_model.id,
                        Model.is_active == True,
                    )
                    .first()
                )

                if model_obj:
                    # 使用 get_effective_* 方法，会自动回退到 GlobalModel 的默认值
                    price_per_request = model_obj.get_effective_price_per_request()
                else:
                    # Provider 没有实现该模型，直接使用 GlobalModel 的默认价格
                    price_per_request = global_model.default_price_per_request

        return price_per_request

    def get_request_price(self, provider: ProviderRef, model: str) -> float | None:
        """
        返回按次计费价格（每次请求的固定费用）。
        直接查数据库，避免事件循环开销。

        Args:
            provider: Provider 对象或提供商名称
            model: 用户请求的模型名

        Returns:
            按次计费价格，如果没有配置则返回 None
        """
        provider_obj = self._resolve_provider(provider)
        price_per_request = None

        if provider_obj:
            global_model = (
                self.db.query(GlobalModel)
                .filter(
                    GlobalModel.name == model,
                    GlobalModel.is_active == True,
                )
                .first()
            )

            if global_model:
                model_obj = (
                    self.db.query(Model)
                    .filter(
                        Model.provider_id == provider_obj.id,
                        Model.global_model_id == global_model.id,
                        Model.is_active == True,
                    )
                    .first()
                )

                if model_obj:
                    price_per_request = model_obj.get_effective_price_per_request()
                else:
                    price_per_request = global_model.default_price_per_request

        return price_per_request

    def get_cache_prices(
        self, provider: ProviderRef, model: str, input_price: float
    ) -> tuple[float | None, float | None]:
        """
        返回缓存创建/读取价格（每 1M tokens）。
        直接查缓存和数据库，避免事件循环开销。

        Args:
            provider: Provider 对象或提供商名称
            model: 用户请求的模型名（必须是 GlobalModel.name）
            input_price: 基础输入价格（用于默认估算）

        Returns:
            (cache_creation_price, cache_read_price) 元组
        """
        provider_name = self._provider_name(provider)
        cache_key = f"{provider_name}:{model}"

        # 优先从缓存获取
        if cache_key in self._cache_price_cache:
            prices = self._cache_price_cache[cache_key]
            return prices["creation"], prices["read"]

        # 缓存未命中，同步查询数据库
        provider_obj = self._resolve_provider(provider)
        cache_creation_price = None
        cache_read_price = None

        if provider_obj:
            global_model = (
                self.db.query(GlobalModel)
                .filter(
                    GlobalModel.name == model,
                    GlobalModel.is_active == True,
                )
                .first()
            )

            if global_model:
                model_obj = (
                    self.db.query(Model)
                    .filter(
                        Model.provider_id == provider_obj.id,
                        Model.global_model_id == global_model.id,
                        Model.is_active == True,
                    )
                    .first()
                )

                if model_obj:
                    tiered = model_obj.get_effective_tiered_pricing()
                    if tiered and tiered.get("tiers"):
                        first_tier = tiered["tiers"][0]
                        cache_creation_price = first_tier.get("cache_creation_price_per_1m")
                        cache_read_price = first_tier.get("cache_read_price_per_1m")
                    else:
                        cache_creation_price = model_obj.get_effective_cache_creation_price()
                        cache_read_price = model_obj.get_effective_cache_read_price()
                else:
                    tiered = global_model.default_tiered_pricing
                    if tiered and tiered.get("tiers"):
                        first_tier = tiered["tiers"][0]
                        cache_creation_price = first_tier.get("cache_creation_price_per_1m")
                        cache_read_price = first_tier.get("cache_read_price_per_1m")

        # 默认缓存价格估算
        if cache_creation_price is None:
            cache_creation_price = input_price * 1.25
        if cache_read_price is None:
            cache_read_price = input_price * 0.1

        self._evict_if_full(self._cache_price_cache)
        self._cache_price_cache[cache_key] = {
            "creation": cache_creation_price,
            "read": cache_read_price,
        }
        return cache_creation_price, cache_read_price

    def calculate_cost(
        self,
        provider: Provider,
        model: str,
        input_tokens: int,
        output_tokens: int,
    ) -> dict[str, float]:
        """返回与旧 ModelMapper.calculate_cost 相同结构的费用信息。"""
        input_price, output_price = self.get_model_price(provider, model)
        input_cost, output_cost, _, _, _, _, total_cost = self.compute_cost(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            input_price_per_1m=input_price,
            output_price_per_1m=output_price,
        )
        return {
            "input_cost": round(input_cost, 6),
            "output_cost": round(output_cost, 6),
            "total_cost": round(total_cost, 6),
            "input_price_per_1m": input_price,
            "output_price_per_1m": output_price,
        }

    @staticmethod
    def compute_cost(
        *,
        input_tokens: int,
        output_tokens: int,
        input_price_per_1m: float,
        output_price_per_1m: float,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        cache_creation_price_per_1m: float | None = None,
        cache_read_price_per_1m: float | None = None,
        price_per_request: float | None = None,
    ) -> tuple[float, float, float, float, float, float, float]:
        """成本计算核心逻辑（固定价格模式），供 UsageService 等复用。

        Returns:
            Tuple of (input_cost, output_cost, cache_creation_cost,
                     cache_read_cost, cache_cost, request_cost, total_cost)
        """
        input_cost = (input_tokens / 1_000_000) * input_price_per_1m
        output_cost = (output_tokens / 1_000_000) * output_price_per_1m

        cache_creation_cost = 0.0
        cache_read_cost = 0.0
        if cache_creation_input_tokens > 0 and cache_creation_price_per_1m is not None:
            cache_creation_cost = (
                cache_creation_input_tokens / 1_000_000
            ) * cache_creation_price_per_1m
        if cache_read_input_tokens > 0 and cache_read_price_per_1m is not None:
            cache_read_cost = (cache_read_input_tokens / 1_000_000) * cache_read_price_per_1m

        cache_cost = cache_creation_cost + cache_read_cost

        # 按次计费成本
        request_cost = price_per_request if price_per_request is not None else 0.0

        total_cost = input_cost + output_cost + cache_cost + request_cost
        return (
            input_cost,
            output_cost,
            cache_creation_cost,
            cache_read_cost,
            cache_cost,
            request_cost,
            total_cost,
        )

    @staticmethod
    def compute_cost_with_tiered_pricing(
        *,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        tiered_pricing: dict | None = None,
        cache_ttl_minutes: int | None = None,
        price_per_request: float | None = None,
        # 回退价格（当没有阶梯配置时使用）
        fallback_input_price_per_1m: float = 0.0,
        fallback_output_price_per_1m: float = 0.0,
        fallback_cache_creation_price_per_1m: float | None = None,
        fallback_cache_read_price_per_1m: float | None = None,
    ) -> tuple[float, float, float, float, float, float, float, int | None]:
        """
        支持阶梯计费的成本计算核心逻辑。

        阶梯判定：使用 input_tokens + cache_read_input_tokens（总输入上下文）

        Args:
            input_tokens: 输入 token 数
            output_tokens: 输出 token 数
            cache_creation_input_tokens: 缓存创建 token 数
            cache_read_input_tokens: 缓存读取 token 数
            tiered_pricing: 阶梯计费配置
            cache_ttl_minutes: 缓存时长（分钟），用于 TTL 差异化定价
            price_per_request: 按次计费价格
            fallback_*: 回退价格配置

        Returns:
            Tuple of (input_cost, output_cost, cache_creation_cost,
                     cache_read_cost, cache_cost, request_cost, total_cost, tier_index)
            tier_index: 命中的阶梯索引（0-based），如果未使用阶梯计费则为 None
        """
        # 计算总输入上下文（用于阶梯判定）
        total_input_context = input_tokens + cache_read_input_tokens

        tier_index = None
        input_price_per_1m = fallback_input_price_per_1m
        output_price_per_1m = fallback_output_price_per_1m
        cache_creation_price_per_1m = fallback_cache_creation_price_per_1m
        cache_read_price_per_1m = fallback_cache_read_price_per_1m

        # 如果有阶梯配置，查找匹配的阶梯
        if tiered_pricing and tiered_pricing.get("tiers"):
            tier = ModelCostService.get_tier_for_tokens(tiered_pricing, total_input_context)
            if tier:
                # 找到阶梯索引
                tier_index = tiered_pricing["tiers"].index(tier)

                input_price_per_1m = tier.get("input_price_per_1m", fallback_input_price_per_1m)
                output_price_per_1m = tier.get("output_price_per_1m", fallback_output_price_per_1m)
                cache_creation_price_per_1m = tier.get(
                    "cache_creation_price_per_1m", fallback_cache_creation_price_per_1m
                )

                # 获取缓存读取价格（考虑 TTL 差异化）
                cache_read_price_per_1m = ModelCostService.get_cache_read_price_for_ttl(
                    tier, cache_ttl_minutes
                )
                if cache_read_price_per_1m is None:
                    cache_read_price_per_1m = fallback_cache_read_price_per_1m

                logger.debug(
                    f"[阶梯计费] 总输入上下文: {total_input_context}, "
                    f"命中阶梯: {tier_index + 1}, "
                    f"输入价格: ${input_price_per_1m}/M, "
                    f"输出价格: ${output_price_per_1m}/M"
                )

        # 计算成本
        input_cost = (input_tokens / 1_000_000) * input_price_per_1m
        output_cost = (output_tokens / 1_000_000) * output_price_per_1m

        cache_creation_cost = 0.0
        cache_read_cost = 0.0
        if cache_creation_input_tokens > 0 and cache_creation_price_per_1m is not None:
            cache_creation_cost = (
                cache_creation_input_tokens / 1_000_000
            ) * cache_creation_price_per_1m
        if cache_read_input_tokens > 0 and cache_read_price_per_1m is not None:
            cache_read_cost = (cache_read_input_tokens / 1_000_000) * cache_read_price_per_1m

        cache_cost = cache_creation_cost + cache_read_cost

        # 按次计费成本
        request_cost = price_per_request if price_per_request is not None else 0.0

        total_cost = input_cost + output_cost + cache_cost + request_cost

        return (
            input_cost,
            output_cost,
            cache_creation_cost,
            cache_read_cost,
            cache_cost,
            request_cost,
            total_cost,
            tier_index,
        )

    @classmethod
    def clear_cache(cls) -> None:
        """清理价格相关缓存。"""
        cls._price_cache.clear()
        cls._cache_price_cache.clear()
        cls._tiered_pricing_cache.clear()

    @classmethod
    def _evict_if_full(cls, cache: dict) -> None:
        """缓存超出上限时清空，防止无界增长。"""
        if len(cache) >= cls._MAX_CACHE_SIZE:
            cache.clear()

    # ------------------------------------------------------------------
    # 内部辅助
    # ------------------------------------------------------------------

    def _provider_name(self, provider: ProviderRef) -> str:
        if isinstance(provider, Provider):
            return provider.name
        return provider or "unknown"

    def _resolve_provider(self, provider: ProviderRef) -> Provider | None:
        if isinstance(provider, Provider):
            return provider
        if not provider or provider == "unknown":
            return None
        return self.db.query(Provider).filter(Provider.name == provider).first()

    # ------------------------------------------------------------------
    # 基于策略模式的计费方法
    # ------------------------------------------------------------------

    async def compute_cost_with_strategy_async(
        self,
        provider: ProviderRef,
        model: str,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        api_format: str | None = None,
        cache_ttl_minutes: int | None = None,
    ) -> tuple[float, float, float, float, float, float, float, int | None]:
        """
        使用计费策略计算成本（异步版本）

        根据 core.api_format.capabilities 解析计费模板与总输入上下文，支持阶梯计费和 TTL 差异化。

        Args:
            provider: Provider 对象或提供商名称
            model: 模型名称
            input_tokens: 输入 token 数
            output_tokens: 输出 token 数
            cache_creation_input_tokens: 缓存创建 token 数
            cache_read_input_tokens: 缓存读取 token 数
            api_format: API 格式（用于解析格式相关计费口径）
            cache_ttl_minutes: 缓存时长（分钟），用于 TTL 差异化定价

        Returns:
            Tuple of (input_cost, output_cost, cache_creation_cost,
                     cache_read_cost, cache_cost, request_cost, total_cost, tier_index)
        """
        # 获取价格配置
        input_price, output_price = await self.get_model_price_async(provider, model)
        cache_creation_price, cache_read_price = await self.get_cache_prices_async(
            provider, model, input_price
        )
        request_price = await self.get_request_price_async(provider, model)
        tiered_pricing = await self.get_tiered_pricing_async(provider, model)

        billing_template = resolve_billing_template_for_api_format(api_format) or ""

        if billing_template:
            total_input_context = compute_total_input_context_for_api_format(
                api_format,
                input_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            )

            result = calculate_request_cost(
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cache_creation_input_tokens=cache_creation_input_tokens,
                cache_read_input_tokens=cache_read_input_tokens,
                input_price_per_1m=input_price,
                output_price_per_1m=output_price,
                cache_creation_price_per_1m=cache_creation_price,
                cache_read_price_per_1m=cache_read_price,
                price_per_request=request_price,
                tiered_pricing=tiered_pricing,
                cache_ttl_minutes=cache_ttl_minutes,
                total_input_context=total_input_context,
                billing_template=billing_template,
            )
            return (
                result["input_cost"],
                result["output_cost"],
                result["cache_creation_cost"],
                result["cache_read_cost"],
                result["cache_cost"],
                result["request_cost"],
                result["total_cost"],
                result["tier_index"],
            )
        else:
            # 回退到默认计算逻辑（无显式格式能力时使用静态方法）
            return self.compute_cost_with_tiered_pricing(
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cache_creation_input_tokens=cache_creation_input_tokens,
                cache_read_input_tokens=cache_read_input_tokens,
                tiered_pricing=tiered_pricing,
                cache_ttl_minutes=cache_ttl_minutes,
                price_per_request=request_price,
                fallback_input_price_per_1m=input_price,
                fallback_output_price_per_1m=output_price,
                fallback_cache_creation_price_per_1m=cache_creation_price,
                fallback_cache_read_price_per_1m=cache_read_price,
            )

    def compute_cost_with_strategy(
        self,
        provider: ProviderRef,
        model: str,
        input_tokens: int,
        output_tokens: int,
        cache_creation_input_tokens: int = 0,
        cache_read_input_tokens: int = 0,
        api_format: str | None = None,
        cache_ttl_minutes: int | None = None,
    ) -> tuple[float, float, float, float, float, float, float, int | None]:
        """
        使用计费策略计算成本（同步版本）
        """
        import asyncio

        return asyncio.run(
            self.compute_cost_with_strategy_async(
                provider=provider,
                model=model,
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cache_creation_input_tokens=cache_creation_input_tokens,
                cache_read_input_tokens=cache_read_input_tokens,
                api_format=api_format,
                cache_ttl_minutes=cache_ttl_minutes,
            )
        )
