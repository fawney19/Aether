# mypy: disable-error-code="arg-type"
"""
ModelAvailabilityQuery 单元测试

说明：
- 本测试使用静态源码检查 + FakeSession/FakeQuery 风格，避免引入真实 DB 依赖
- 项目日志使用 loguru，pytest 的 caplog 默认无法直接捕获，因此用 loguru sink 验证 warning
"""

import inspect
from io import StringIO
from types import SimpleNamespace
from typing import Any

from loguru import logger

from src.services.model.availability import ModelAvailabilityQuery

# ============================================================================
# 测试辅助类
# ============================================================================


class FakeQuery:
    """通用 Fake Query，支持链式调用和自定义返回数据"""

    def __init__(self, data: list[Any]) -> None:
        self._data = data

    def filter(self, *_args: Any, **_kwargs: Any) -> "FakeQuery":
        return self

    def join(self, *_args: Any, **_kwargs: Any) -> "FakeQuery":
        return self

    def all(self) -> list[Any]:
        return self._data


class FakeSession:
    """
    通用 Fake Session

    注：FakeSession 仅实现了 Session 的 query 方法，用于测试场景。
    类型检查会报错，但运行时正常工作。
    """

    def __init__(self, query_data: list[Any]) -> None:
        self._query_data = query_data

    def query(self, *_entities: Any) -> FakeQuery:
        return FakeQuery(self._query_data)


class TestBaseActiveModelsSourceCode:
    """
    静态验证 base_active_models 的核心实现

    通过检查源码确保关键条件不会被遗漏。
    """

    def test_uses_inner_join_for_global_model(self) -> None:
        """应使用内连接 GlobalModel（排除 global_model_id=NULL）"""
        source = inspect.getsource(ModelAvailabilityQuery.base_active_models)

        assert (
            "join(Model.global_model)" in source or ".join(Model.global_model)" in source
        ), "base_active_models 应使用 join(Model.global_model) 内连接"
        assert (
            "outerjoin" not in source.lower()
        ), "base_active_models 不应使用 outerjoin（会返回 global_model_id=NULL 的记录）"

    def test_filters_is_available_true_or_null(self) -> None:
        """应过滤 is_available = True 或 NULL"""
        source = inspect.getsource(ModelAvailabilityQuery.base_active_models)

        assert "or_(" in source, "base_active_models 应使用 or_() 处理 is_available"
        assert "is_available" in source, "base_active_models 应包含 is_available 条件"
        assert (
            "is_(True)" in source and "is_(None)" in source
        ), "base_active_models 应同时检查 is_available = True 和 NULL"

    def test_filters_all_is_active_fields(self) -> None:
        """应过滤 Model/Provider/GlobalModel 的 is_active"""
        source = inspect.getsource(ModelAvailabilityQuery.base_active_models)

        assert "Model.is_active" in source, "base_active_models 应检查 Model.is_active"
        assert "Provider.is_active" in source, "base_active_models 应检查 Provider.is_active"
        assert "GlobalModel.is_active" in source, "base_active_models 应检查 GlobalModel.is_active"


class TestGetProviderKeyRules:
    """测试 get_provider_key_rules"""

    def test_empty_provider_ids_returns_empty_dict(self) -> None:
        """空 provider_ids 应返回空字典"""
        result = ModelAvailabilityQuery.get_provider_key_rules(
            FakeSession([]),
            provider_ids=set(),
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={},
        )
        assert result == {}

    def test_skips_key_with_invalid_allowed_models_type(self) -> None:
        """allowed_models 类型异常时应跳过该 Key 并打日志"""
        log_output = StringIO()
        handler_id = logger.add(
            log_output,
            format="{message}",
            level="WARNING",
            filter=lambda record: "[ModelAvailability]" in record["message"],
        )

        try:
            # (key_id, provider_id, allowed_models, api_formats)
            data = [("key-1", "provider-1", "invalid-string-type", ["openai:chat"])]

            result = ModelAvailabilityQuery.get_provider_key_rules(
                FakeSession(data),
                provider_ids={"provider-1"},
                api_formats=["openai:chat"],
                provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
            )

            assert "provider-1" not in result or len(result.get("provider-1", [])) == 0

            log_content = log_output.getvalue()
            assert "allowed_models 类型异常" in log_content
            assert "key-1" in log_content
        finally:
            logger.remove(handler_id)

    def test_skips_key_with_invalid_api_formats_type(self) -> None:
        """api_formats 类型异常时应跳过该 Key 并打日志"""
        log_output = StringIO()
        handler_id = logger.add(
            log_output,
            format="{message}",
            level="WARNING",
            filter=lambda record: "[ModelAvailability]" in record["message"],
        )

        try:
            # api_formats 是字符串而非列表
            data = [("key-1", "provider-1", None, "openai:chat")]

            result = ModelAvailabilityQuery.get_provider_key_rules(
                FakeSession(data),
                provider_ids={"provider-1"},
                api_formats=["openai:chat"],
                provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
            )

            assert "provider-1" not in result or len(result.get("provider-1", [])) == 0

            log_content = log_output.getvalue()
            assert "api_formats 类型异常" in log_content
        finally:
            logger.remove(handler_id)

    def test_skips_key_with_none_api_formats(self) -> None:
        """api_formats 为 None 时应使用 endpoint 支持的格式（不打日志）"""
        data = [("key-1", "provider-1", None, None)]

        result = ModelAvailabilityQuery.get_provider_key_rules(
            FakeSession(data),
            provider_ids={"provider-1"},
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
        )

        # api_formats=None 表示使用 endpoint_formats
        assert "provider-1" in result
        rules = result["provider-1"]
        assert len(rules) == 1
        allowed_models, usable_formats = rules[0]
        assert "openai:chat" in usable_formats

    def test_allowed_models_none_means_no_restriction(self) -> None:
        """allowed_models = None 表示不限制模型"""
        data = [("key-1", "provider-1", None, ["openai:chat"])]

        result = ModelAvailabilityQuery.get_provider_key_rules(
            FakeSession(data),
            provider_ids={"provider-1"},
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
        )

        assert "provider-1" in result
        rules = result["provider-1"]
        assert len(rules) == 1
        allowed_models, usable_formats = rules[0]
        assert allowed_models is None  # None = 不限制
        assert "openai:chat" in usable_formats

    def test_allowed_models_list_is_preserved(self) -> None:
        """allowed_models 为有效列表时应正常返回"""
        data = [("key-1", "provider-1", ["claude-3-opus", "claude-3-sonnet"], ["openai:chat"])]

        result = ModelAvailabilityQuery.get_provider_key_rules(
            FakeSession(data),
            provider_ids={"provider-1"},
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
        )

        assert "provider-1" in result
        rules = result["provider-1"]
        assert len(rules) == 1
        allowed_models, _ = rules[0]
        assert allowed_models == ["claude-3-opus", "claude-3-sonnet"]

    def test_skips_key_when_format_intersection_empty(self) -> None:
        """格式交集为空时不包含该 Key"""
        # Key 支持 CLAUDE，但请求的是 OPENAI
        data = [("key-1", "provider-1", None, ["claude:chat"])]

        result = ModelAvailabilityQuery.get_provider_key_rules(
            FakeSession(data),
            provider_ids={"provider-1"},
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
        )

        assert "provider-1" not in result or len(result.get("provider-1", [])) == 0

    def test_multiple_keys_same_provider(self) -> None:
        """同一 Provider 下多个 Key 应合并规则"""
        data = [
            ("key-1", "provider-1", None, ["openai:chat"]),
            ("key-2", "provider-1", ["claude-3-opus"], ["openai:chat"]),
        ]

        result = ModelAvailabilityQuery.get_provider_key_rules(
            FakeSession(data),
            provider_ids={"provider-1"},
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
        )

        assert "provider-1" in result
        rules = result["provider-1"]
        assert len(rules) == 2


class TestGetProvidersWithActiveKeys:
    """测试 get_providers_with_active_keys"""

    def test_empty_provider_ids_returns_empty_set(self) -> None:
        """空 provider_ids 应返回空集合"""
        result = ModelAvailabilityQuery.get_providers_with_active_keys(
            FakeSession([]),
            provider_ids=set(),
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={},
        )
        assert result == set()

    def test_skips_key_with_invalid_api_formats_type(self) -> None:
        """api_formats 类型异常时应跳过该 Key 并打日志"""
        log_output = StringIO()
        handler_id = logger.add(
            log_output,
            format="{message}",
            level="WARNING",
            filter=lambda record: "[ModelAvailability]" in record["message"],
        )

        try:
            # (provider_id, api_formats) - api_formats 是字符串
            data = [("provider-1", "openai:chat")]

            result = ModelAvailabilityQuery.get_providers_with_active_keys(
                FakeSession(data),
                provider_ids={"provider-1"},
                api_formats=["openai:chat"],
                provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
            )

            assert "provider-1" not in result

            log_content = log_output.getvalue()
            assert "api_formats 类型异常" in log_content
        finally:
            logger.remove(handler_id)

    def test_skips_key_with_none_api_formats(self) -> None:
        """api_formats 为 None 时应使用 endpoint 支持的格式（返回该 Provider）"""
        data = [("provider-1", None)]

        result = ModelAvailabilityQuery.get_providers_with_active_keys(
            FakeSession(data),
            provider_ids={"provider-1"},
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
        )

        # api_formats=None 表示使用 endpoint_formats，所以应该返回
        assert "provider-1" in result

    def test_returns_provider_when_format_matches(self) -> None:
        """格式匹配时应返回该 Provider"""
        data = [("provider-1", ["openai:chat", "claude:chat"])]

        result = ModelAvailabilityQuery.get_providers_with_active_keys(
            FakeSession(data),
            provider_ids={"provider-1"},
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
        )

        assert "provider-1" in result

    def test_skips_provider_when_format_intersection_empty(self) -> None:
        """格式交集为空时不返回该 Provider"""
        # Key 支持 GEMINI，但请求的是 OPENAI，且端点支持 OPENAI
        data = [("provider-1", ["gemini:chat"])]

        result = ModelAvailabilityQuery.get_providers_with_active_keys(
            FakeSession(data),
            provider_ids={"provider-1"},
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
        )

        assert "provider-1" not in result

    def test_skips_provider_not_in_endpoint_formats(self) -> None:
        """Provider 不在 endpoint_formats 中时应跳过"""
        data = [("provider-1", ["openai:chat"])]

        result = ModelAvailabilityQuery.get_providers_with_active_keys(
            FakeSession(data),
            provider_ids={"provider-1"},
            api_formats=["openai:chat"],
            provider_to_endpoint_formats={},  # 空，无匹配端点
        )

        assert "provider-1" not in result

    def test_case_insensitive_format_matching(self) -> None:
        """格式匹配应忽略大小写"""
        data = [("provider-1", ["openai:chat"])]  # 小写

        result = ModelAvailabilityQuery.get_providers_with_active_keys(
            FakeSession(data),
            provider_ids={"provider-1"},
            api_formats=["OPENAI:CHAT"],  # 大写
            provider_to_endpoint_formats={"provider-1": {"openai:chat"}},
        )

        assert "provider-1" in result


class TestGetProvidersWithActiveEndpointsSourceCode:
    """静态验证 get_providers_with_active_endpoints 的核心实现"""

    def test_checks_provider_is_active(self) -> None:
        """应检查 Provider.is_active"""
        source = inspect.getsource(ModelAvailabilityQuery.get_providers_with_active_endpoints)
        assert "Provider.is_active" in source

    def test_checks_endpoint_is_active(self) -> None:
        """应检查 ProviderEndpoint.is_active"""
        source = inspect.getsource(ModelAvailabilityQuery.get_providers_with_active_endpoints)
        assert "ProviderEndpoint.is_active" in source

    def test_joins_provider_table(self) -> None:
        """应 JOIN Provider 表"""
        source = inspect.getsource(ModelAvailabilityQuery.get_providers_with_active_endpoints)
        assert ".join(" in source.lower() or "join(" in source.lower()


class TestFindByGlobalModelNameSourceCode:
    """静态验证 find_by_global_model_name 的核心实现"""

    def test_uses_base_active_models(self) -> None:
        """应复用 base_active_models"""
        source = inspect.getsource(ModelAvailabilityQuery.find_by_global_model_name)
        assert "base_active_models" in source

    def test_filters_by_global_model_name(self) -> None:
        """应按 GlobalModel.name 过滤"""
        source = inspect.getsource(ModelAvailabilityQuery.find_by_global_model_name)
        assert "GlobalModel.name" in source


class TestFindModelByIdNoFallback:
    """测试 find_model_by_id 不再回退到 provider_model_name"""

    def test_source_code_no_provider_model_name_in_function(self) -> None:
        """
        静态验证：find_model_by_id 函数中不应出现 provider_model_name

        直接检查字符串是否出现，覆盖所有可能的写法。
        """
        from src.api.base import models_service

        source = inspect.getsource(models_service.find_model_by_id)

        assert (
            "provider_model_name" not in source
        ), "find_model_by_id 中不应出现 provider_model_name（已删除回退逻辑）"


class TestGetAvailableModelIdsWithMappings:
    """测试 _get_available_model_ids_for_format 支持 model_mappings"""

    def test_source_code_uses_check_model_allowed_with_mappings(self) -> None:
        """静态验证：应使用 check_model_allowed_with_mappings 而非 check_model_allowed"""
        from src.api.base import models_service

        source = inspect.getsource(models_service._get_available_model_ids_for_format)

        assert (
            "check_model_allowed_with_mappings" in source
        ), "_get_available_model_ids_for_format 应使用 check_model_allowed_with_mappings"

    def test_source_code_extracts_model_mappings_from_config(self) -> None:
        """静态验证：应从 global_model.config 提取 model_mappings"""
        from src.api.base import models_service

        source = inspect.getsource(models_service._get_available_model_ids_for_format)

        assert (
            "model_mappings" in source
        ), "_get_available_model_ids_for_format 应提取 model_mappings"
        assert (
            ".config" in source or "config" in source
        ), "_get_available_model_ids_for_format 应从 config 获取 model_mappings"


class TestModelMappingsIntegration:
    """
    测试 model_mappings 正则映射的集成场景

    场景：
    - GlobalModel.name = "claude-haiku-4-5-20251001"
    - Model.provider_model_name = "claude-haiku-4-5-20251001"
    - GlobalModel.config.model_mappings = ["claude-3-5-haiku-.*"]
    - Key.allowed_models = ["claude-3-5-haiku-20251001"]

    预期：通过 model_mappings 正则匹配，模型应出现在可用列表中
    """

    def test_mapping_pattern_matches_allowed_model(self) -> None:
        """model_mappings 正则应能匹配 Key.allowed_models 中的模型"""
        from src.core.model_permissions import check_model_allowed_with_mappings

        # 模拟你描述的场景
        is_allowed, matched = check_model_allowed_with_mappings(
            model_name="claude-haiku-4-5-20251001",  # GlobalModel.name
            allowed_models=["claude-3-5-haiku-20251001"],  # Key.allowed_models
            model_mappings=["claude-3-5-haiku-.*"],  # GlobalModel.config.model_mappings
        )

        assert is_allowed is True
        assert matched == "claude-3-5-haiku-20251001"

    def test_exact_match_takes_priority_over_mapping(self) -> None:
        """精确匹配优先于映射匹配"""
        from src.core.model_permissions import check_model_allowed_with_mappings

        is_allowed, matched = check_model_allowed_with_mappings(
            model_name="claude-haiku-4-5-20251001",
            allowed_models=["claude-haiku-4-5-20251001"],  # 精确匹配
            model_mappings=["claude-3-5-haiku-.*"],
        )

        assert is_allowed is True
        assert matched is None  # 精确匹配时 matched 为 None

    def test_no_mapping_match_returns_false(self) -> None:
        """映射不匹配时返回 False"""
        from src.core.model_permissions import check_model_allowed_with_mappings

        is_allowed, matched = check_model_allowed_with_mappings(
            model_name="claude-haiku-4-5-20251001",
            allowed_models=["gpt-4o"],  # 与映射模式不匹配
            model_mappings=["claude-3-5-haiku-.*"],
        )

        assert is_allowed is False
        assert matched is None

    def test_multiple_mappings_first_match_wins(self) -> None:
        """多个映射时，按 allowed_models 排序后第一个匹配的生效"""
        from src.core.model_permissions import check_model_allowed_with_mappings

        is_allowed, matched = check_model_allowed_with_mappings(
            model_name="target-model",
            allowed_models=["b-model-1", "a-model-1"],
            model_mappings=[".*-model-1"],  # 匹配两个
        )

        assert is_allowed is True
        # 按字母排序，a-model-1 先匹配
        assert matched == "a-model-1"


# ============================================================================
# AccessRestrictions 测试
# ============================================================================


class TestAccessRestrictionsFromApiKeyAndUser:
    """测试 AccessRestrictions.from_api_key_and_user 合并逻辑"""

    def test_both_none_returns_no_restrictions(self) -> None:
        """API Key 和 User 都为 None 时返回无限制"""
        from src.api.base.models_service import AccessRestrictions

        result = AccessRestrictions.from_api_key_and_user(None, None)

        assert result.allowed_providers is None
        assert result.allowed_models is None
        assert result.allowed_api_formats is None

    def test_api_key_restrictions_take_priority(self) -> None:
        """API Key 的限制优先于 User 的限制"""
        from unittest.mock import MagicMock

        from src.api.base.models_service import AccessRestrictions

        api_key = MagicMock()
        api_key.allowed_providers = ["provider-a"]
        api_key.allowed_models = ["model-a"]
        api_key.allowed_api_formats = ["openai:chat"]

        user = MagicMock()
        user.group = SimpleNamespace(
            allowed_providers=["provider-b"],
            allowed_models=["model-b"],
            allowed_api_formats=["claude:chat"],
        )

        result = AccessRestrictions.from_api_key_and_user(api_key, user)

        assert result.allowed_providers == ["provider-a"]
        assert result.allowed_models == ["model-a"]
        assert result.allowed_api_formats == ["openai:chat"]

    def test_user_restrictions_used_when_api_key_has_none(self) -> None:
        """API Key 无限制时使用 User 的限制"""
        from unittest.mock import MagicMock

        from src.api.base.models_service import AccessRestrictions

        api_key = MagicMock()
        api_key.allowed_providers = None
        api_key.allowed_models = None
        api_key.allowed_api_formats = None

        user = MagicMock()
        user.group = SimpleNamespace(
            allowed_providers=["provider-b"],
            allowed_models=["model-b"],
            allowed_api_formats=["claude:chat"],
        )

        result = AccessRestrictions.from_api_key_and_user(api_key, user)

        assert result.allowed_providers == ["provider-b"]
        assert result.allowed_models == ["model-b"]
        assert result.allowed_api_formats == ["claude:chat"]

    def test_partial_api_key_restrictions(self) -> None:
        """API Key 部分限制时，其余字段从 User 获取"""
        from unittest.mock import MagicMock

        from src.api.base.models_service import AccessRestrictions

        api_key = MagicMock()
        api_key.allowed_providers = ["provider-a"]
        api_key.allowed_models = None  # 无限制
        api_key.allowed_api_formats = None  # 无限制

        user = MagicMock()
        user.group = SimpleNamespace(
            allowed_providers=["provider-b"],
            allowed_models=["model-b"],
            allowed_api_formats=["claude:chat"],
        )

        result = AccessRestrictions.from_api_key_and_user(api_key, user)

        assert result.allowed_providers == ["provider-a"]  # 来自 API Key
        assert result.allowed_models == ["model-b"]  # 来自 User
        assert result.allowed_api_formats == ["claude:chat"]  # 来自 User

    def test_only_api_key_provided(self) -> None:
        """只提供 API Key 时使用其限制"""
        from unittest.mock import MagicMock

        from src.api.base.models_service import AccessRestrictions

        api_key = MagicMock()
        api_key.allowed_providers = ["provider-a"]
        api_key.allowed_models = ["model-a"]
        api_key.allowed_api_formats = ["openai:chat"]

        result = AccessRestrictions.from_api_key_and_user(api_key, None)

        assert result.allowed_providers == ["provider-a"]
        assert result.allowed_models == ["model-a"]
        assert result.allowed_api_formats == ["openai:chat"]

    def test_only_user_provided(self) -> None:
        """只提供 User 时使用其限制"""
        from unittest.mock import MagicMock

        from src.api.base.models_service import AccessRestrictions

        user = MagicMock()
        user.group = SimpleNamespace(
            allowed_providers=["provider-b"],
            allowed_models=["model-b"],
            allowed_api_formats=["claude:chat"],
        )

        result = AccessRestrictions.from_api_key_and_user(None, user)

        assert result.allowed_providers == ["provider-b"]
        assert result.allowed_models == ["model-b"]
        assert result.allowed_api_formats == ["claude:chat"]

    def test_user_group_defaults_are_used(self) -> None:
        """用户限制统一从分组默认值读取。"""
        from src.api.base.models_service import AccessRestrictions

        user = SimpleNamespace(
            group=SimpleNamespace(
                allowed_providers=["provider-group"],
                allowed_models=["model-group"],
                allowed_api_formats=["openai:chat"],
            ),
        )

        result = AccessRestrictions.from_api_key_and_user(None, user)

        assert result.allowed_providers == ["provider-group"]
        assert result.allowed_models == ["model-group"]
        assert result.allowed_api_formats == ["openai:chat"]


class TestAccessRestrictionsIsApiFormatAllowed:
    """测试 AccessRestrictions.is_api_format_allowed"""

    def test_none_allowed_formats_means_all_allowed(self) -> None:
        """allowed_api_formats = None 表示所有格式都允许"""
        from src.api.base.models_service import AccessRestrictions

        restrictions = AccessRestrictions(allowed_api_formats=None)

        assert restrictions.is_api_format_allowed("openai:chat") is True
        assert restrictions.is_api_format_allowed("claude:chat") is True
        assert restrictions.is_api_format_allowed("gemini:chat") is True

    def test_format_in_allowed_list(self) -> None:
        """格式在允许列表中时返回 True"""
        from src.api.base.models_service import AccessRestrictions

        restrictions = AccessRestrictions(allowed_api_formats=["openai:chat", "claude:chat"])

        assert restrictions.is_api_format_allowed("openai:chat") is True
        assert restrictions.is_api_format_allowed("claude:chat") is True

    def test_format_not_in_allowed_list(self) -> None:
        """格式不在允许列表中时返回 False"""
        from src.api.base.models_service import AccessRestrictions

        restrictions = AccessRestrictions(allowed_api_formats=["openai:chat"])

        assert restrictions.is_api_format_allowed("claude:chat") is False
        assert restrictions.is_api_format_allowed("gemini:chat") is False

    def test_empty_allowed_list_blocks_all(self) -> None:
        """空允许列表阻止所有格式"""
        from src.api.base.models_service import AccessRestrictions

        restrictions = AccessRestrictions(allowed_api_formats=[])

        assert restrictions.is_api_format_allowed("openai:chat") is False
        assert restrictions.is_api_format_allowed("claude:chat") is False


class TestAccessRestrictionsIsModelAllowed:
    """测试 AccessRestrictions.is_model_allowed"""

    def test_no_restrictions_allows_all(self) -> None:
        """无限制时允许所有模型"""
        from src.api.base.models_service import AccessRestrictions

        restrictions = AccessRestrictions(allowed_providers=None, allowed_models=None)

        assert restrictions.is_model_allowed("claude-3-opus", "provider-a") is True
        assert restrictions.is_model_allowed("gpt-4", "provider-b") is True

    def test_provider_restriction_blocks_unallowed_provider(self) -> None:
        """Provider 限制阻止不在列表中的 Provider"""
        from src.api.base.models_service import AccessRestrictions

        restrictions = AccessRestrictions(allowed_providers=["provider-a"])

        assert restrictions.is_model_allowed("claude-3-opus", "provider-a") is True
        assert restrictions.is_model_allowed("claude-3-opus", "provider-b") is False

    def test_model_restriction_blocks_unallowed_model(self) -> None:
        """模型限制阻止不在列表中的模型"""
        from src.api.base.models_service import AccessRestrictions

        restrictions = AccessRestrictions(allowed_models=["claude-3-opus"])

        assert restrictions.is_model_allowed("claude-3-opus", "provider-a") is True
        assert restrictions.is_model_allowed("gpt-4", "provider-a") is False

    def test_both_restrictions_must_pass(self) -> None:
        """Provider 和模型限制都必须通过"""
        from src.api.base.models_service import AccessRestrictions

        restrictions = AccessRestrictions(
            allowed_providers=["provider-a"],
            allowed_models=["claude-3-opus"],
        )

        # 两者都满足
        assert restrictions.is_model_allowed("claude-3-opus", "provider-a") is True

        # Provider 不满足
        assert restrictions.is_model_allowed("claude-3-opus", "provider-b") is False

        # 模型不满足
        assert restrictions.is_model_allowed("gpt-4", "provider-a") is False

        # 两者都不满足
        assert restrictions.is_model_allowed("gpt-4", "provider-b") is False

    def test_empty_provider_list_blocks_all(self) -> None:
        """空 Provider 列表阻止所有"""
        from src.api.base.models_service import AccessRestrictions

        restrictions = AccessRestrictions(allowed_providers=[])

        assert restrictions.is_model_allowed("claude-3-opus", "provider-a") is False

    def test_empty_model_list_blocks_all(self) -> None:
        """空模型列表阻止所有"""
        from src.api.base.models_service import AccessRestrictions

        restrictions = AccessRestrictions(allowed_models=[])

        assert restrictions.is_model_allowed("claude-3-opus", "provider-a") is False
