from unittest.mock import MagicMock

from src.models.database import GlobalModel, Model
from src.services.billing.default_rules import DefaultBillingRuleGenerator
from src.services.billing.formula_engine import FormulaEngine
from src.services.billing.rule_service import BillingRuleService


class TestDefaultBillingRuleGenerator:
    def test_default_rule_basic_chat_cost(self) -> None:
        global_model = GlobalModel(
            name="test-model",
            display_name="Test Model",
            is_active=True,
            default_price_per_request=0.01,
            default_tiered_pricing={
                "tiers": [
                    {
                        "up_to": None,
                        "input_price_per_1m": 3.0,
                        "output_price_per_1m": 15.0,
                        # cache prices intentionally omitted (legacy derives from input price)
                    }
                ]
            },
        )

        rule = DefaultBillingRuleGenerator.generate_for_model(
            global_model=global_model,
            model=None,
            task_type="chat",
        )

        engine = FormulaEngine()
        result = engine.evaluate(
            expression=rule.expression,
            variables=rule.variables,
            dimensions={
                "input_tokens": 1000,
                "output_tokens": 500,
                "cache_creation_tokens": 200,
                "cache_read_tokens": 300,
                "request_count": 1,
                # tier key
                "total_input_context": 1000 + 300,
            },
            dimension_mappings=rule.dimension_mappings,
            strict_mode=True,
        )

        assert result.status == "complete"
        assert abs(float(result.cost) - 0.02134) < 1e-9

    def test_default_rule_tiered_pricing_uses_total_input_context(self) -> None:
        global_model = GlobalModel(
            name="tiered-model",
            display_name="Tiered Model",
            is_active=True,
            default_price_per_request=None,
            default_tiered_pricing={
                "tiers": [
                    {"up_to": 200000, "input_price_per_1m": 3.0, "output_price_per_1m": 15.0},
                    {"up_to": None, "input_price_per_1m": 1.5, "output_price_per_1m": 7.5},
                ]
            },
        )

        rule = DefaultBillingRuleGenerator.generate_for_model(
            global_model=global_model,
            model=None,
            task_type="chat",
        )

        engine = FormulaEngine()
        result = engine.evaluate(
            expression=rule.expression,
            variables=rule.variables,
            dimensions={
                "input_tokens": 250000,
                "output_tokens": 10000,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 0,
                "request_count": 1,
                "total_input_context": 250000,
            },
            dimension_mappings=rule.dimension_mappings,
            strict_mode=True,
        )

        # 250000 * 1.5 / 1M = 0.375
        # 10000 * 7.5 / 1M = 0.075
        assert result.status == "complete"
        assert abs(float(result.cost) - 0.45) < 1e-9

    def test_default_rule_cache_ttl_pricing_overrides_cache_read_price(self) -> None:
        global_model = GlobalModel(
            name="ttl-model",
            display_name="TTL Model",
            is_active=True,
            default_price_per_request=0.0,
            default_tiered_pricing={
                "tiers": [
                    {
                        "up_to": None,
                        "input_price_per_1m": 3.0,
                        "output_price_per_1m": 15.0,
                        "cache_read_price_per_1m": 0.3,
                        "cache_ttl_pricing": [
                            {"ttl_minutes": 5, "cache_read_price_per_1m": 0.3},
                            {"ttl_minutes": 60, "cache_read_price_per_1m": 0.5},
                        ],
                    }
                ]
            },
        )

        rule = DefaultBillingRuleGenerator.generate_for_model(
            global_model=global_model,
            model=None,
            task_type="chat",
        )

        engine = FormulaEngine()
        result = engine.evaluate(
            expression=rule.expression,
            variables=rule.variables,
            dimensions={
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 1000,
                "cache_ttl_minutes": 60,
                "request_count": 1,
                "total_input_context": 0 + 1000,
            },
            dimension_mappings=rule.dimension_mappings,
            strict_mode=True,
        )

        # TTL=60 should use cache_read_price_per_1m=0.5
        # 1000 * 0.5 / 1M = 0.0005
        assert result.status == "complete"
        assert abs(float(result.cost) - 0.0005) < 1e-9

    def test_default_rule_cache_ttl_pricing_overrides_cache_creation_price(self) -> None:
        global_model = GlobalModel(
            name="ttl-creation-model",
            display_name="TTL Creation Model",
            is_active=True,
            default_price_per_request=0.0,
            default_tiered_pricing={
                "tiers": [
                    {
                        "up_to": None,
                        "input_price_per_1m": 3.0,
                        "output_price_per_1m": 15.0,
                        "cache_creation_price_per_1m": 3.75,
                        "cache_ttl_pricing": [
                            {"ttl_minutes": 5, "cache_creation_price_per_1m": 3.75},
                            {"ttl_minutes": 60, "cache_creation_price_per_1m": 6.0},
                        ],
                    }
                ]
            },
        )

        rule = DefaultBillingRuleGenerator.generate_for_model(
            global_model=global_model,
            model=None,
            task_type="chat",
        )

        engine = FormulaEngine()

        # TTL=5: cache_creation_price_per_1m=3.75
        # 1_000_000 * 3.75 / 1M = 3.75
        result_5m = engine.evaluate(
            expression=rule.expression,
            variables=rule.variables,
            dimensions={
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 1_000_000,
                "cache_read_tokens": 0,
                "cache_ttl_minutes": 5,
                "request_count": 1,
                "total_input_context": 0,
            },
            dimension_mappings=rule.dimension_mappings,
            strict_mode=True,
        )
        assert result_5m.status == "complete"
        assert abs(float(result_5m.cost) - 3.75) < 1e-9

        # TTL=60: cache_creation_price_per_1m=6.0
        # 1_000_000 * 6.0 / 1M = 6.0
        result_1h = engine.evaluate(
            expression=rule.expression,
            variables=rule.variables,
            dimensions={
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 1_000_000,
                "cache_read_tokens": 0,
                "cache_ttl_minutes": 60,
                "request_count": 1,
                "total_input_context": 0,
            },
            dimension_mappings=rule.dimension_mappings,
            strict_mode=True,
        )
        assert result_1h.status == "complete"
        assert abs(float(result_1h.cost) - 6.0) < 1e-9

    def test_default_rule_cache_ttl_pricing_uses_base_price_for_5m_when_only_1h_override_exists(
        self,
    ) -> None:
        global_model = GlobalModel(
            name="ttl-creation-1h-only-model",
            display_name="TTL Creation 1h Only Model",
            is_active=True,
            default_price_per_request=0.0,
            default_tiered_pricing={
                "tiers": [
                    {
                        "up_to": None,
                        "input_price_per_1m": 3.0,
                        "output_price_per_1m": 15.0,
                        "cache_creation_price_per_1m": 3.75,
                        "cache_ttl_pricing": [
                            {"ttl_minutes": 60, "cache_creation_price_per_1m": 6.0},
                        ],
                    }
                ]
            },
        )

        rule = DefaultBillingRuleGenerator.generate_for_model(
            global_model=global_model,
            model=None,
            task_type="chat",
        )

        engine = FormulaEngine()
        result = engine.evaluate(
            expression=rule.expression,
            variables=rule.variables,
            dimensions={
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 1_000_000,
                "cache_read_tokens": 0,
                "cache_ttl_minutes": 5,
                "request_count": 1,
                "total_input_context": 0,
            },
            dimension_mappings=rule.dimension_mappings,
            strict_mode=True,
        )

        assert result.status == "complete"
        assert abs(float(result.cost) - 3.75) < 1e-9

    def test_default_rule_cache_ttl_pricing_uses_base_read_price_for_5m_when_only_1h_override_exists(
        self,
    ) -> None:
        global_model = GlobalModel(
            name="ttl-read-1h-only-model",
            display_name="TTL Read 1h Only Model",
            is_active=True,
            default_price_per_request=0.0,
            default_tiered_pricing={
                "tiers": [
                    {
                        "up_to": None,
                        "input_price_per_1m": 3.0,
                        "output_price_per_1m": 15.0,
                        "cache_read_price_per_1m": 0.3,
                        "cache_ttl_pricing": [
                            {"ttl_minutes": 60, "cache_read_price_per_1m": 0.5},
                        ],
                    }
                ]
            },
        )

        rule = DefaultBillingRuleGenerator.generate_for_model(
            global_model=global_model,
            model=None,
            task_type="chat",
        )

        engine = FormulaEngine()
        result = engine.evaluate(
            expression=rule.expression,
            variables=rule.variables,
            dimensions={
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 1_000,
                "cache_ttl_minutes": 5,
                "request_count": 1,
                "total_input_context": 1_000,
            },
            dimension_mappings=rule.dimension_mappings,
            strict_mode=True,
        )

        assert result.status == "complete"
        assert abs(float(result.cost) - 0.0003) < 1e-9


class TestBillingRuleServiceDefaultFallback:
    def test_find_rule_returns_default_for_chat_when_no_db_rule(self) -> None:
        from src.services.billing.cache import BillingCache

        BillingCache.invalidate_all()

        global_model = GlobalModel(
            id="gm-1",
            name="test-model",
            display_name="Test Model",
            is_active=True,
            default_price_per_request=0.0,
            default_tiered_pricing={
                "tiers": [{"up_to": None, "input_price_per_1m": 3.0, "output_price_per_1m": 15.0}]
            },
        )

        model_obj = Model(
            id="m-1",
            provider_id="p-1",
            global_model_id="gm-1",
            provider_model_name="provider-test-model",
            is_active=True,
            tiered_pricing=None,
            price_per_request=None,
        )
        model_obj.global_model = global_model

        # Build a mock Session with deterministic query().filter().first() chain.
        q_global = MagicMock()
        q_global.filter.return_value.first.return_value = global_model

        q_model = MagicMock()
        q_model.filter.return_value.first.return_value = model_obj

        q_rule_model = MagicMock()
        q_rule_model.filter.return_value.first.return_value = None

        q_rule_global = MagicMock()
        q_rule_global.filter.return_value.first.return_value = None

        db = MagicMock()
        db.query.side_effect = [q_global, q_model, q_rule_model, q_rule_global]

        lookup = BillingRuleService.find_rule(
            db,
            provider_id="p-1",
            model_name="test-model",
            task_type="chat",
        )
        assert lookup is not None
        assert lookup.scope == "default"
        assert lookup.rule.id == "__default__"
        assert lookup.effective_task_type == "chat"

        # Cached: second call should not touch db.query again.
        db.query.reset_mock()
        lookup2 = BillingRuleService.find_rule(
            db,
            provider_id="p-1",
            model_name="test-model",
            task_type="chat",
        )
        assert lookup2 is not None
        assert lookup2.scope == "default"
        assert db.query.call_count == 0

    def test_find_rule_returns_default_for_video_when_require_rule_false(self) -> None:
        from src.config.settings import config
        from src.services.billing.cache import BillingCache

        BillingCache.invalidate_all()
        old_require = config.billing_require_rule
        config.billing_require_rule = False

        try:
            global_model = GlobalModel(
                id="gm-2",
                name="video-model",
                display_name="Video Model",
                is_active=True,
                default_price_per_request=0.0,
                default_tiered_pricing={
                    "tiers": [
                        {"up_to": None, "input_price_per_1m": 3.0, "output_price_per_1m": 15.0}
                    ]
                },
            )

            q_global = MagicMock()
            q_global.filter.return_value.first.return_value = global_model

            q_rule_global = MagicMock()
            q_rule_global.filter.return_value.first.return_value = None

            db = MagicMock()
            # provider_id omitted -> only GlobalModel query + global BillingRule query
            db.query.side_effect = [q_global, q_rule_global]

            lookup = BillingRuleService.find_rule(
                db,
                provider_id=None,
                model_name="video-model",
                task_type="video",
            )
            assert lookup is not None
            assert lookup.scope == "default"
            assert lookup.rule.id == "__default__"
            assert lookup.effective_task_type == "video"
        finally:
            config.billing_require_rule = old_require

    def test_find_rule_returns_template_for_video_when_require_rule_true(self) -> None:
        from src.config.settings import config
        from src.services.billing.cache import BillingCache

        BillingCache.invalidate_all()
        old_require = config.billing_require_rule
        config.billing_require_rule = True

        try:
            global_model = GlobalModel(
                id="gm-3",
                name="video-model-2",
                display_name="Video Model 2",
                is_active=True,
                default_price_per_request=0.0,
                default_tiered_pricing={
                    "tiers": [
                        {"up_to": None, "input_price_per_1m": 3.0, "output_price_per_1m": 15.0}
                    ]
                },
            )

            q_global = MagicMock()
            q_global.filter.return_value.first.return_value = global_model

            q_rule_global = MagicMock()
            q_rule_global.filter.return_value.first.return_value = None

            db = MagicMock()
            db.query.side_effect = [q_global, q_rule_global]

            lookup = BillingRuleService.find_rule(
                db,
                provider_id=None,
                model_name="video-model-2",
                task_type="video",
            )
            assert lookup is not None
            assert lookup.scope == "default"
            assert lookup.rule.id == "__default__"
            # Universal Billing Rule is now used for all task types
            assert lookup.rule.name == "Universal Billing Rule"
        finally:
            config.billing_require_rule = old_require
