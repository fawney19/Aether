"""
ProviderEndpoint 相关的 API 模型定义
"""

from __future__ import annotations

import re
from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator

from src.models.admin_requests import (
    ClaudeCodeAdvancedConfig,
    FailoverRulesConfig,
    PoolAdvancedConfig,
    ProxyConfig,
)

# ========== Header Rule 类型定义 ==========
# 请求头规则支持三种操作：
# - set: 设置/覆盖请求头 {"action": "set", "key": "X-Custom", "value": "val"}
# - drop: 删除请求头 {"action": "drop", "key": "X-Unwanted"}
# - rename: 重命名请求头 {"action": "rename", "from": "X-Old", "to": "X-New"}
# 实际验证在 headers.py 的 apply_rules 中处理
HeaderRule = dict[str, Any]


# ========== Body Rule 类型定义 ==========
# 请求体规则支持六种操作：
# - set: 设置/覆盖字段 {"action": "set", "path": "metadata", "value": {"custom": "val"}}
#     value 中的字符串 {{$original}} 会被替换为该路径的原值（完全匹配时保留类型）
# - drop: 删除字段 {"action": "drop", "path": "unwanted_field"}
# - rename: 重命名字段 {"action": "rename", "from": "old_key", "to": "new_key"}
# - append: 向数组追加元素 {"action": "append", "path": "messages", "value": {...}}
# - insert: 在数组指定位置插入 {"action": "insert", "path": "messages", "index": 0, "value": {...}}
# - regex_replace: 正则替换字符串值 {"action": "regex_replace", "path": "...", "pattern": "...", "replacement": "..."}
# - name_style: 转换命名风格 {"action": "name_style", "path": "tools[*].name", "style": "camelCase"}
# 路径语法支持数组索引：messages[0].content, data[-1], matrix[0][1]
# 路径语法支持通配符：tools[*].name（遍历所有元素）, tools[0-4].name（遍历范围）
# 运行时处理在 request_builder.py 的 apply_body_rules 中；结构校验见 _validate_body_rules
BodyRule = dict[str, Any]

# body_rules 允许的 action 集合
_BODY_RULE_ACTIONS: frozenset[str] = frozenset(
    {"set", "drop", "rename", "append", "insert", "regex_replace", "name_style"}
)

# name_style 允许的风格值
_NAME_STYLE_VALUES: frozenset[str] = frozenset(
    {"snake_case", "camelCase", "PascalCase", "kebab-case", "capitalize"}
)

# regex_replace 允许的 flags 字符
_REGEX_FLAG_CHARS: frozenset[str] = frozenset({"i", "m", "s"})

# condition 允许的操作符
_CONDITION_OPS: frozenset[str] = frozenset(
    {
        "eq",
        "neq",
        "gt",
        "lt",
        "gte",
        "lte",
        "starts_with",
        "ends_with",
        "contains",
        "matches",
        "exists",
        "not_exists",
        "in",
        "type_is",
    }
)

# type_is 允许的类型值
_TYPE_IS_VALUES: frozenset[str] = frozenset(
    {"string", "number", "boolean", "array", "object", "null"}
)


def parse_re_flags(flags_str: str) -> int:
    """将 flags 字符串（i/m/s）转换为 re 标志位。

    供 endpoint_models 校验和 request_builder 运行时共用。
    """
    result = 0
    for f in flags_str:
        if f == "i":
            result |= re.IGNORECASE
        elif f == "m":
            result |= re.MULTILINE
        elif f == "s":
            result |= re.DOTALL
    return result


def _validate_condition(condition: Any, rule_idx: int) -> None:
    """校验单条规则的 condition 结构"""
    if not isinstance(condition, dict):
        raise ValueError(f"body_rules[{rule_idx}]: condition 必须是 JSON 对象")

    op = condition.get("op")
    if not isinstance(op, str) or op not in _CONDITION_OPS:
        raise ValueError(
            f"body_rules[{rule_idx}]: condition.op 必须是 {sorted(_CONDITION_OPS)} 之一，"
            f"当前值: {op!r}"
        )

    path = condition.get("path")
    if not isinstance(path, str) or not path.strip():
        raise ValueError(f"body_rules[{rule_idx}]: condition 必须提供非空 path")

    # exists / not_exists 不需要 value
    if op in ("exists", "not_exists"):
        return

    value = condition.get("value")

    # 数值操作符校验
    if op in ("gt", "lt", "gte", "lte"):
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            raise ValueError(f"body_rules[{rule_idx}]: condition op={op!r} 的 value 必须为数值")

    # matches 正则校验
    if op == "matches":
        if not isinstance(value, str) or not value:
            raise ValueError(
                f"body_rules[{rule_idx}]: condition op=matches 的 value 必须为非空字符串"
            )
        try:
            re.compile(value)
        except re.error as e:
            raise ValueError(
                f"body_rules[{rule_idx}]: condition op=matches 的 value 不是合法正则: {e}"
            )

    # in 校验
    if op == "in":
        if not isinstance(value, list):
            raise ValueError(f"body_rules[{rule_idx}]: condition op=in 的 value 必须为数组")

    # type_is 校验
    if op == "type_is":
        if not isinstance(value, str) or value not in _TYPE_IS_VALUES:
            raise ValueError(
                f"body_rules[{rule_idx}]: condition op=type_is 的 value 必须是 "
                f"{sorted(_TYPE_IS_VALUES)} 之一"
            )

    # starts_with / ends_with / contains 对 value 做字符串校验
    if op in ("starts_with", "ends_with"):
        if not isinstance(value, str):
            raise ValueError(f"body_rules[{rule_idx}]: condition op={op!r} 的 value 必须为字符串")


def _validate_body_rules(rules: list[BodyRule]) -> list[BodyRule]:
    """校验 body_rules 列表的结构和正则合法性。

    校验项：
    - 每条规则必须是 dict 且包含合法 action
    - 需要 path 的 action（set/drop/append/insert/regex_replace）必须提供非空 path 字符串
    - rename 必须提供非空 from / to 字符串
    - insert 的 index 必须为整数
    - regex_replace 的 pattern 必须能通过 re.compile 编译，flags 仅允许 i/m/s
    """
    for idx, rule in enumerate(rules):
        if not isinstance(rule, dict):
            raise ValueError(f"body_rules[{idx}]: 规则必须是 JSON 对象")

        action = rule.get("action")
        if not isinstance(action, str) or action.strip().lower() not in _BODY_RULE_ACTIONS:
            raise ValueError(
                f"body_rules[{idx}]: action 必须是 {sorted(_BODY_RULE_ACTIONS)} 之一，"
                f"当前值: {action!r}"
            )
        action = action.strip().lower()

        # ---------- path 校验 ----------
        if action in {"set", "drop", "append", "insert", "regex_replace", "name_style"}:
            path = rule.get("path")
            if not isinstance(path, str) or not path.strip():
                raise ValueError(f"body_rules[{idx}]: action={action!r} 必须提供非空 path")

        # ---------- rename 校验 ----------
        if action == "rename":
            from_val = rule.get("from")
            to_val = rule.get("to")
            if not isinstance(from_val, str) or not from_val.strip():
                raise ValueError(f"body_rules[{idx}]: rename 必须提供非空 from")
            if not isinstance(to_val, str) or not to_val.strip():
                raise ValueError(f"body_rules[{idx}]: rename 必须提供非空 to")

        # ---------- insert 校验 ----------
        if action == "insert":
            index = rule.get("index")
            if not isinstance(index, int) or isinstance(index, bool):
                raise ValueError(f"body_rules[{idx}]: insert 的 index 必须为整数")

        # ---------- regex_replace 校验 ----------
        if action == "regex_replace":
            pattern = rule.get("pattern")
            if not isinstance(pattern, str) or not pattern:
                raise ValueError(f"body_rules[{idx}]: regex_replace 必须提供非空 pattern 字符串")

            replacement = rule.get("replacement", "")
            if not isinstance(replacement, str):
                raise ValueError(f"body_rules[{idx}]: regex_replace 的 replacement 必须为字符串")

            # 校验 flags
            flags_str = rule.get("flags", "")
            re_flags = 0
            if isinstance(flags_str, str) and flags_str:
                invalid_flags = set(flags_str) - _REGEX_FLAG_CHARS
                if invalid_flags:
                    raise ValueError(
                        f"body_rules[{idx}]: regex_replace 的 flags 仅允许 "
                        f"{''.join(sorted(_REGEX_FLAG_CHARS))}，"
                        f"非法字符: {''.join(sorted(invalid_flags))}"
                    )
                re_flags = parse_re_flags(flags_str)

            # 尝试编译正则，捕获语法错误
            try:
                re.compile(pattern, re_flags)
            except re.error as e:
                raise ValueError(
                    f"body_rules[{idx}]: regex_replace 的 pattern 不是合法正则表达式: {e}"
                )

            # 校验 count
            count = rule.get("count", 0)
            if not isinstance(count, int) or count < 0:
                raise ValueError(f"body_rules[{idx}]: regex_replace 的 count 必须为非负整数")

        # ---------- name_style 校验 ----------
        if action == "name_style":
            style = rule.get("style")
            if not isinstance(style, str) or style not in _NAME_STYLE_VALUES:
                raise ValueError(
                    f"body_rules[{idx}]: name_style 的 style 必须是 "
                    f"{sorted(_NAME_STYLE_VALUES)} 之一，当前值: {style!r}"
                )

        # ---------- condition 校验 ----------
        condition = rule.get("condition")
        if condition is not None:
            _validate_condition(condition, idx)

    return rules


# ========== ProviderEndpoint CRUD ==========


class ProviderEndpointCreate(BaseModel):
    """创建 Endpoint 请求"""

    provider_id: str = Field(..., description="Provider ID")
    api_format: str = Field(
        ...,
        description=(
            "Endpoint signature（例如: claude:chat/claude:cli, openai:chat/openai:cli/openai:compact/openai:video, gemini:chat/gemini:cli/gemini:video）"
        ),
    )
    base_url: str = Field(..., min_length=1, max_length=500, description="API 基础 URL")
    custom_path: str | None = Field(default=None, max_length=200, description="自定义请求路径")

    # 请求头配置
    header_rules: list[HeaderRule] | None = Field(
        default=None,
        description="请求头规则列表，支持 set/drop/rename 操作",
    )

    # 请求体配置
    body_rules: list[BodyRule] | None = Field(
        default=None,
        description="请求体规则列表，支持 set/drop/rename/append/insert/regex_replace 操作",
    )

    max_retries: int = Field(default=2, ge=0, le=999, description="最大重试次数")

    # 额外配置
    config: dict[str, Any] | None = Field(default=None, description="额外配置（JSON）")

    # 代理配置
    proxy: ProxyConfig | None = Field(default=None, description="代理配置")

    # 格式转换配置
    format_acceptance_config: dict[str, Any] | None = Field(
        default=None,
        description="格式接受策略配置（跨格式转换开关/白黑名单等）",
    )

    @field_validator("api_format")
    @classmethod
    def validate_api_format(cls, v: str) -> str:
        """验证 API 格式"""
        from src.core.api_format import list_endpoint_definitions, resolve_endpoint_definition
        from src.core.api_format.signature import normalize_signature_key

        normalized = normalize_signature_key(v)
        if resolve_endpoint_definition(normalized) is None:
            allowed = [d.signature_key for d in list_endpoint_definitions()]
            raise ValueError(f"api_format 必须是以下之一: {allowed}")
        return normalized

    @field_validator("base_url")
    @classmethod
    def validate_base_url(cls, v: str) -> str:
        if not re.match(r"^https?://", v, re.IGNORECASE):
            raise ValueError("URL 必须以 http:// 或 https:// 开头")

        return v.rstrip("/")  # 移除末尾斜杠

    @field_validator("body_rules")
    @classmethod
    def validate_body_rules(cls, v: list[BodyRule] | None) -> list[BodyRule] | None:
        """校验 body_rules 结构和正则合法性"""
        if v is None:
            return v
        return _validate_body_rules(v)


class ProviderEndpointUpdate(BaseModel):
    """更新 Endpoint 请求"""

    base_url: str | None = Field(
        default=None, min_length=1, max_length=500, description="API 基础 URL"
    )
    custom_path: str | None = Field(default=None, max_length=200, description="自定义请求路径")

    # 请求头配置
    header_rules: list[HeaderRule] | None = Field(
        default=None,
        description="请求头规则列表，支持 set/drop/rename 操作",
    )

    # 请求体配置
    body_rules: list[BodyRule] | None = Field(
        default=None,
        description="请求体规则列表，支持 set/drop/rename/append/insert/regex_replace 操作",
    )

    max_retries: int | None = Field(default=None, ge=0, le=999, description="最大重试次数")
    is_active: bool | None = Field(default=None, description="是否启用")
    config: dict[str, Any] | None = Field(default=None, description="额外配置")
    proxy: ProxyConfig | None = Field(default=None, description="代理配置")

    # 格式转换配置
    format_acceptance_config: dict[str, Any] | None = Field(
        default=None,
        description="格式接受策略配置（跨格式转换开关/白黑名单等）",
    )

    @field_validator("base_url")
    @classmethod
    def validate_base_url(cls, v: str | None) -> str | None:
        """验证 API URL"""
        if v is None:
            return v

        if not re.match(r"^https?://", v, re.IGNORECASE):
            raise ValueError("URL 必须以 http:// 或 https:// 开头")

        return v.rstrip("/")  # 移除末尾斜杠

    @field_validator("body_rules")
    @classmethod
    def validate_body_rules(cls, v: list[BodyRule] | None) -> list[BodyRule] | None:
        """校验 body_rules 结构和正则合法性"""
        if v is None:
            return v
        return _validate_body_rules(v)


class ProviderEndpointResponse(BaseModel):
    """Endpoint 响应"""

    id: str
    provider_id: str
    provider_name: str  # 冗余字段，方便前端显示

    # API 配置
    api_format: str
    base_url: str
    custom_path: str | None = None

    # 请求头配置
    header_rules: list[HeaderRule] | None = Field(default=None, description="请求头规则列表")

    # 请求体配置
    body_rules: list[BodyRule] | None = Field(default=None, description="请求体规则列表")

    max_retries: int

    # 状态
    is_active: bool

    # 额外配置
    config: dict[str, Any] | None = None

    # 代理配置（响应中密码已脱敏）
    proxy: dict[str, Any] | None = Field(default=None, description="代理配置（密码已脱敏）")

    # 格式转换配置
    format_acceptance_config: dict[str, Any] | None = Field(
        default=None,
        description="格式接受策略配置（跨格式转换开关/白黑名单等）",
    )

    # 统计（从 Keys 聚合）
    total_keys: int = Field(default=0, description="总 Key 数量")
    active_keys: int = Field(default=0, description="活跃 Key 数量")

    # 时间戳
    created_at: datetime
    updated_at: datetime

    model_config = ConfigDict(from_attributes=True)


# ========== ProviderAPIKey 相关 ==========


class EndpointAPIKeyCreate(BaseModel):
    """为 Provider 添加 API Key"""

    provider_id: str | None = Field(default=None, description="Provider ID（从 URL 获取）")
    api_formats: list[str] | None = Field(
        default=None, min_length=1, description="支持的 endpoint signature 列表（必填，路由层校验）"
    )

    api_key: str = Field(
        default="", max_length=10000, description="API Key（标准认证时必填，将自动加密）"
    )
    auth_type: Literal["api_key", "service_account", "oauth"] = Field(
        default="api_key",
        description="认证类型：api_key（标准 API Key）/ service_account（GCP Service Account）/ oauth（OAuth access_token）",
    )
    auth_config: dict[str, Any] | None = Field(
        default=None,
        description=(
            "认证配置（JSON）：service_account 时存储完整 Service Account JSON；"
            "oauth 时存储 token/refresh/expires_at 等（后端加密存储，不在响应中返回）"
        ),
    )
    name: str = Field(..., min_length=1, max_length=100, description="密钥名称（必填，用于识别）")

    # 成本计算
    rate_multipliers: dict[str, float] | None = Field(
        default=None,
        description="按 endpoint signature 的成本倍率，如 {'claude:cli': 1.0, 'openai:cli': 0.8}",
    )

    # 优先级和限制（数字越小越优先）
    internal_priority: int = Field(default=50, description="Key 内部优先级（提供商优先模式）")
    # rpm_limit: NULL=自适应模式（系统自动学习），数字=固定限制模式
    rpm_limit: int | None = Field(
        default=None, ge=1, le=10000, description="RPM 限制（NULL=自适应模式）"
    )
    allowed_models: list[str] | None = Field(
        default=None,
        description="允许使用的模型列表（null=不限制）",
    )

    # 能力标签
    capabilities: dict[str, bool] | None = Field(
        default=None, description="Key 能力标签，如 {'cache_1h': true, 'context_1m': true}"
    )

    # 缓存与熔断配置
    cache_ttl_minutes: int = Field(
        default=5, ge=0, le=60, description="缓存 TTL（分钟），0=禁用，默认5分钟"
    )
    max_probe_interval_minutes: int = Field(
        default=32, ge=2, le=32, description="熔断探测间隔（分钟），范围 2-32"
    )

    # 备注
    note: str | None = Field(default=None, max_length=500, description="备注说明（可选）")

    # 自动获取模型
    auto_fetch_models: bool = Field(
        default=False, description="是否启用自动获取模型（启用后系统定时从上游 API 获取可用模型）"
    )

    # 锁定的模型列表
    locked_models: list[str] | None = Field(
        default=None, description="被锁定的模型列表（刷新时不会被删除）"
    )

    # 模型过滤规则（仅当 auto_fetch_models=True 时生效）
    model_include_patterns: list[str] | None = Field(
        default=None, description="模型包含规则（支持 * 和 ? 通配符），空表示包含所有"
    )
    model_exclude_patterns: list[str] | None = Field(
        default=None, description="模型排除规则（支持 * 和 ? 通配符），空表示不排除"
    )

    @field_validator("api_formats")
    @classmethod
    def validate_api_formats(cls, v: list[str] | None) -> list[str] | None:
        """验证 API 格式列表"""
        if v is None:
            return v

        from src.core.api_format import list_endpoint_definitions, resolve_endpoint_definition
        from src.core.api_format.signature import normalize_signature_key

        allowed = [d.signature_key for d in list_endpoint_definitions()]
        validated: list[str] = []
        seen: set[str] = set()
        for fmt in v:
            normalized = normalize_signature_key(fmt)
            if resolve_endpoint_definition(normalized) is None:
                raise ValueError(f"api_formats 必须是以下之一: {allowed}，当前值: {fmt}")
            if normalized in seen:
                continue  # 静默去重
            seen.add(normalized)
            validated.append(normalized)
        return validated

    @field_validator("allowed_models")
    @classmethod
    def validate_allowed_models(cls, v: list[str] | None) -> list[str] | None:
        """
        规范化 allowed_models：去空、去重、保留顺序
        """
        if v is None:
            return v

        if not isinstance(v, list):
            raise ValueError("allowed_models 必须是列表")

        cleaned: list[str] = []
        seen: set[str] = set()
        for item in v:
            if not isinstance(item, str):
                raise ValueError("allowed_models 列表元素必须为字符串")
            name = item.strip()
            if not name or name in seen:
                continue
            seen.add(name)
            cleaned.append(name)
        return cleaned

    @field_validator("api_key")
    @classmethod
    def validate_api_key(cls, v: str) -> str:
        """验证 API Key 基本格式"""
        # 移除首尾空白（长度校验由 Field max_length 处理）
        return v.strip()

    @field_validator("name")
    @classmethod
    def validate_name(cls, v: str) -> str:
        """验证名称（防止 XSS）"""
        # 移除危险的 HTML 标签
        v = re.sub(r"<script.*?</script>", "", v, flags=re.IGNORECASE | re.DOTALL)
        v = re.sub(r"<iframe.*?</iframe>", "", v, flags=re.IGNORECASE | re.DOTALL)
        v = re.sub(r"javascript:", "", v, flags=re.IGNORECASE)
        v = re.sub(r"on\w+\s*=", "", v, flags=re.IGNORECASE)
        return v.strip()

    @field_validator("note")
    @classmethod
    def validate_note(cls, v: str | None) -> str | None:
        """验证备注（防止 XSS）"""
        if v is None:
            return v
        # 移除危险的 HTML 标签
        v = re.sub(r"<script.*?</script>", "", v, flags=re.IGNORECASE | re.DOTALL)
        v = re.sub(r"<iframe.*?</iframe>", "", v, flags=re.IGNORECASE | re.DOTALL)
        v = re.sub(r"javascript:", "", v, flags=re.IGNORECASE)
        v = re.sub(r"on\w+\s*=", "", v, flags=re.IGNORECASE)
        return v.strip()


class EndpointAPIKeyUpdate(BaseModel):
    """更新 Endpoint API Key"""

    api_formats: list[str] | None = Field(
        default=None, min_length=1, description="支持的 API 格式列表"
    )

    api_key: str | None = Field(
        default=None,
        max_length=10000,
        description="API Key（标准认证时使用，将自动加密）",
    )
    auth_type: Literal["api_key", "service_account", "oauth"] | None = Field(
        default=None,
        description="认证类型：api_key（标准 API Key）/ service_account（GCP Service Account）/ oauth（OAuth access_token）",
    )
    auth_config: dict[str, Any] | None = Field(
        default=None,
        description=(
            "认证配置（JSON）：service_account 时存储完整 Service Account JSON；"
            "oauth 时存储 token/refresh/expires_at 等（后端加密存储，不在响应中返回）"
        ),
    )
    name: str | None = Field(default=None, min_length=1, max_length=100, description="密钥名称")
    rate_multipliers: dict[str, float] | None = Field(
        default=None,
        description="按 endpoint signature 的成本倍率，如 {'claude:cli': 1.0, 'openai:cli': 0.8}",
    )
    internal_priority: int | None = Field(
        default=None, description="Key 内部优先级（提供商优先模式，数字越小越优先）"
    )
    global_priority_by_format: dict[str, int] | None = Field(
        default=None,
        description="按 endpoint signature 的全局优先级，如 {'claude:chat': 1, 'claude:cli': 2}",
    )
    # rpm_limit: 使用特殊标记区分"未提供"和"设置为 null（自适应模式）"
    # - 不提供字段：不更新
    # - 提供 null：切换为自适应模式
    # - 提供数字：设置固定 RPM 限制
    rpm_limit: int | None = Field(
        default=None, ge=1, le=10000, description="RPM 限制（null=自适应模式）"
    )
    allowed_models: list[str] | None = Field(
        default=None,
        description="允许使用的模型列表（null=不限制）",
    )
    capabilities: dict[str, bool] | None = Field(
        default=None, description="Key 能力标签，如 {'cache_1h': true, 'context_1m': true}"
    )
    cache_ttl_minutes: int | None = Field(
        default=None, ge=0, le=60, description="缓存 TTL（分钟），0=禁用"
    )
    max_probe_interval_minutes: int | None = Field(
        default=None, ge=2, le=32, description="熔断探测间隔（分钟），范围 2-32"
    )
    is_active: bool | None = Field(default=None, description="是否启用")
    note: str | None = Field(default=None, max_length=500, description="备注说明")
    auto_fetch_models: bool | None = Field(default=None, description="是否启用自动获取模型")
    locked_models: list[str] | None = Field(
        default=None, description="被锁定的模型列表（刷新时不会被删除）"
    )
    # 模型过滤规则（仅当 auto_fetch_models=True 时生效）
    model_include_patterns: list[str] | None = Field(
        default=None, description="模型包含规则（支持 * 和 ? 通配符），空表示包含所有"
    )
    model_exclude_patterns: list[str] | None = Field(
        default=None, description="模型排除规则（支持 * 和 ? 通配符），空表示不排除"
    )
    # Key 级别代理配置（覆盖 Provider 级别代理）
    # - 不提供：不更新
    # - 提供 null：清除 Key 级别代理，回退到 Provider 级别代理
    # - 提供 ProxyConfig：设置 Key 级别代理
    proxy: ProxyConfig | None = Field(
        default=None,
        description="Key 级别代理配置（覆盖 Provider 级别代理），null=使用 Provider 级别代理",
    )
    fingerprint: dict[str, Any] | None = Field(
        default=None,
        description="请求指纹配置（TLS + HTTP 头部）",
    )

    @field_validator("api_formats")
    @classmethod
    def validate_api_formats(cls, v: list[str] | None) -> list[str] | None:
        """验证 API 格式列表"""
        if v is None:
            return v

        return EndpointAPIKeyCreate.validate_api_formats(v)

    @field_validator("allowed_models")
    @classmethod
    def validate_allowed_models(cls, v: list[str] | None) -> list[str] | None:
        # 与 EndpointAPIKeyCreate 保持一致
        return EndpointAPIKeyCreate.validate_allowed_models(v)

    @field_validator("api_key")
    @classmethod
    def validate_api_key(cls, v: str | None) -> str | None:
        """验证 API Key 基本格式"""
        if v is None:
            return v

        return v.strip()

    @field_validator("name")
    @classmethod
    def validate_name(cls, v: str | None) -> str | None:
        """验证名称（防止 XSS）"""
        if v is None:
            return v

        v = re.sub(r"<script.*?</script>", "", v, flags=re.IGNORECASE | re.DOTALL)
        v = re.sub(r"<iframe.*?</iframe>", "", v, flags=re.IGNORECASE | re.DOTALL)
        v = re.sub(r"javascript:", "", v, flags=re.IGNORECASE)
        v = re.sub(r"on\w+\s*=", "", v, flags=re.IGNORECASE)
        return v.strip()

    @field_validator("note")
    @classmethod
    def validate_note(cls, v: str | None) -> str | None:
        """验证备注（防止 XSS）"""
        if v is None:
            return v

        v = re.sub(r"<script.*?</script>", "", v, flags=re.IGNORECASE | re.DOTALL)
        v = re.sub(r"<iframe.*?</iframe>", "", v, flags=re.IGNORECASE | re.DOTALL)
        v = re.sub(r"javascript:", "", v, flags=re.IGNORECASE)
        v = re.sub(r"on\w+\s*=", "", v, flags=re.IGNORECASE)
        return v.strip()


class EndpointAPIKeyResponse(BaseModel):
    """Endpoint API Key 响应"""

    id: str

    provider_id: str = Field(..., description="Provider ID")
    api_formats: list[str] = Field(
        default=[], description="支持的 endpoint signature 列表（如 openai:chat, claude:cli）"
    )

    # Key 信息（脱敏）
    api_key_masked: str = Field(..., description="脱敏后的 Key")
    api_key_plain: str | None = Field(default=None, description="完整的 Key")
    auth_type: str = Field(
        default="api_key", description="认证类型：api_key / service_account / oauth"
    )
    # auth_config 不在响应中返回（包含敏感信息），前端通过 auth_type 判断类型
    name: str = Field(..., description="密钥名称")

    # 成本计算
    rate_multipliers: dict[str, float] | None = Field(
        default=None,
        description="按 endpoint signature 的成本倍率，如 {'claude:cli': 1.0, 'openai:cli': 0.8}",
    )

    # 优先级和限制
    internal_priority: int = Field(default=50, description="Endpoint 内部优先级")
    global_priority_by_format: dict[str, int] | None = Field(
        default=None, description="按 endpoint signature 的全局优先级"
    )
    rpm_limit: int | None = None
    allowed_models: list[str] | None = None
    capabilities: dict[str, bool] | None = Field(default=None, description="Key 能力标签")

    # OAuth 相关
    oauth_expires_at: int | None = Field(
        default=None, description="OAuth Token 过期时间（Unix 时间戳）"
    )
    oauth_email: str | None = Field(default=None, description="OAuth 账号邮箱")
    oauth_plan_type: str | None = Field(
        default=None, description="OAuth 账号套餐类型（如 free/plus/team/enterprise）"
    )
    oauth_account_id: str | None = Field(default=None, description="OAuth 账号 ID")
    oauth_invalid_at: int | None = Field(
        default=None, description="OAuth Token 失效时间（Unix 时间戳），如账号被封、授权撤销等"
    )
    oauth_invalid_reason: str | None = Field(default=None, description="OAuth Token 失效原因")

    # 缓存与熔断配置
    cache_ttl_minutes: int = Field(default=5, description="缓存 TTL（分钟），0=禁用")
    max_probe_interval_minutes: int = Field(default=32, description="熔断探测间隔（分钟）")

    # 按 endpoint signature 的健康度数据
    health_by_format: dict[str, Any] | None = Field(
        default=None, description="按 endpoint signature 存储的健康度数据"
    )
    circuit_breaker_by_format: dict[str, Any] | None = Field(
        default=None, description="按 endpoint signature 存储的熔断器状态"
    )

    # 聚合字段（从 health_by_format 计算，用于列表显示）
    health_score: float = Field(default=1.0, description="健康度（所有格式中的最低值）")
    consecutive_failures: int = Field(default=0, description="连续失败次数")
    last_failure_at: datetime | None = None

    # 聚合熔断器字段
    circuit_breaker_open: bool = Field(default=False, description="熔断器是否打开（任何格式）")
    circuit_breaker_open_at: datetime | None = Field(default=None, description="熔断器打开时间")
    next_probe_at: datetime | None = Field(default=None, description="下次进入半开状态时间")
    half_open_until: datetime | None = Field(default=None, description="半开状态结束时间")
    half_open_successes: int | None = Field(default=0, description="半开状态成功次数")
    half_open_failures: int | None = Field(default=0, description="半开状态失败次数")
    request_results_window: list[dict] | None = Field(None, description="请求结果滑动窗口")

    # 使用统计
    request_count: int
    success_count: int
    error_count: int
    success_rate: float = Field(default=0.0, description="成功率")
    avg_response_time_ms: float = Field(default=0.0, description="平均响应时间（毫秒）")

    # 状态
    is_active: bool

    # 自适应 RPM 信息
    is_adaptive: bool = Field(default=False, description="是否为自适应模式（rpm_limit=NULL）")
    learned_rpm_limit: int | None = Field(None, description="学习到的 RPM 限制")
    effective_limit: int | None = Field(None, description="当前有效限制")
    # 滑动窗口利用率采样
    utilization_samples: list[dict] | None = Field(None, description="利用率采样窗口")
    last_probe_increase_at: datetime | None = Field(None, description="上次探测性扩容时间")
    concurrent_429_count: int | None = None
    rpm_429_count: int | None = None
    last_429_at: datetime | None = None
    last_429_type: str | None = None

    # 备注
    note: str | None = None

    # 自动获取模型
    auto_fetch_models: bool = Field(default=False, description="是否启用自动获取模型")
    last_models_fetch_at: datetime | None = Field(None, description="最后获取模型时间")
    last_models_fetch_error: str | None = Field(None, description="最后获取模型错误信息")
    locked_models: list[str] | None = Field(None, description="被锁定的模型列表")
    # 模型过滤规则
    model_include_patterns: list[str] | None = Field(None, description="模型包含规则")
    model_exclude_patterns: list[str] | None = Field(None, description="模型排除规则")

    # 上游元数据（由响应头采集，如 Codex 额度信息）
    upstream_metadata: dict[str, Any] | None = Field(
        None, description="上游元数据（如 Codex 额度信息）"
    )

    # Key 级别代理配置
    proxy: dict[str, Any] | None = Field(
        None, description="Key 级别代理配置（覆盖 Provider 级别代理）"
    )
    fingerprint: dict[str, Any] | None = Field(
        None,
        description="请求指纹配置（TLS + HTTP 头部）",
    )

    # 时间戳
    last_used_at: datetime | None = None
    created_at: datetime
    updated_at: datetime

    model_config = ConfigDict(from_attributes=True)


# ========== 健康监控相关 ==========


class FormatHealthData(BaseModel):
    """单个 API 格式的健康度数据"""

    health_score: float = 1.0
    error_rate: float = 0.0
    window_size: int = 0
    consecutive_failures: int = 0
    last_failure_at: str | None = None
    circuit_breaker: dict[str, Any] = Field(default_factory=dict)


class HealthStatusResponse(BaseModel):
    """健康状态响应（支持按格式查询）"""

    # 基础信息
    key_id: str
    key_is_active: bool
    key_statistics: dict[str, Any] | None = None

    # 整体健康度（取所有格式中的最低值）
    key_health_score: float = 1.0
    any_circuit_open: bool = False

    # 按格式的健康度数据
    health_by_format: dict[str, FormatHealthData] | None = None

    # 单格式查询时的字段
    api_format: str | None = None
    key_consecutive_failures: int | None = None
    key_last_failure_at: str | None = None

    # 单格式查询时的熔断器状态
    circuit_breaker_open: bool = False
    circuit_breaker_open_at: str | None = None
    next_probe_at: str | None = None
    half_open_until: str | None = None
    half_open_successes: int = 0
    half_open_failures: int = 0


class HealthSummaryResponse(BaseModel):
    """健康状态摘要"""

    endpoints: dict[str, int] = Field(..., description="Endpoint 统计 (total, active, unhealthy)")
    keys: dict[str, int] = Field(..., description="Key 统计 (total, active, unhealthy)")


# ========== RPM 控制相关 ==========


class KeyRpmStatusResponse(BaseModel):
    """Key RPM 状态响应"""

    key_id: str = Field(..., description="Key ID")
    current_rpm: int = Field(default=0, description="当前 RPM 计数")
    rpm_limit: int | None = Field(default=None, description="RPM 限制")


class KeyPriorityItem(BaseModel):
    """单个 Key 优先级项"""

    key_id: str = Field(..., description="Key ID")
    internal_priority: int = Field(..., ge=0, description="Key 内部优先级（数字越小越优先）")


class BatchUpdateKeyPriorityRequest(BaseModel):
    """批量更新 Key 优先级请求"""

    priorities: list[KeyPriorityItem] = Field(..., min_length=1, description="Key 优先级列表")


# ========== 提供商摘要（增强版） ==========


class ProviderUpdateRequest(BaseModel):
    """Provider 基础配置更新请求"""

    name: str | None = Field(None, min_length=1, max_length=100)
    description: str | None = None
    website: str | None = Field(None, max_length=500, description="主站网站")
    provider_priority: int | None = Field(None, description="提供商优先级(数字越小越优先)")
    keep_priority_on_conversion: bool | None = Field(
        None,
        description="格式转换时是否保持优先级（True=保持原优先级，False=需要转换时降级）",
    )
    enable_format_conversion: bool | None = Field(
        None,
        description="是否允许格式转换（提供商级别开关）",
    )
    is_active: bool | None = None
    billing_type: str | None = Field(
        None, description="计费类型：monthly_quota/pay_as_you_go/free_tier"
    )
    monthly_quota_usd: float | None = Field(None, ge=0, description="订阅配额（美元）")
    quota_reset_day: int | None = Field(None, ge=1, le=31, description="配额重置日（1-31）")
    quota_expires_at: datetime | None = Field(None, description="配额过期时间")
    # 请求配置（从 Endpoint 迁移）
    max_retries: int | None = Field(None, ge=0, le=10, description="最大重试次数")
    proxy: dict[str, Any] | None = Field(None, description="代理配置")
    # 超时配置（秒），为空时使用全局配置
    stream_first_byte_timeout: float | None = Field(
        None, ge=1, le=300, description="流式请求首字节超时（秒）"
    )
    request_timeout: float | None = Field(
        None, ge=1, le=600, description="非流式请求整体超时（秒）"
    )
    claude_code_advanced: ClaudeCodeAdvancedConfig | None = Field(
        None, description="Claude Code 高级配置"
    )
    pool_advanced: PoolAdvancedConfig | None = Field(None, description="通用号池配置")
    failover_rules: FailoverRulesConfig | None = Field(None, description="故障转移规则配置")


class ProviderWithEndpointsSummary(BaseModel):
    """Provider 和 Endpoints 摘要"""

    # Provider 基本信息
    id: str
    name: str
    provider_type: str | None = Field(
        default=None, description="Provider 类型（custom/claude_code/codex/gemini_cli/antigravity）"
    )
    description: str | None = None
    website: str | None = None
    provider_priority: int = Field(default=100, description="提供商优先级(数字越小越优先)")
    keep_priority_on_conversion: bool = Field(
        default=False,
        description="格式转换时是否保持优先级（True=保持原优先级，False=需要转换时降级）",
    )
    enable_format_conversion: bool = Field(
        default=True,
        description="是否允许格式转换（提供商级别开关）",
    )
    is_active: bool

    # 计费相关字段
    billing_type: str | None = None
    monthly_quota_usd: float | None = None
    monthly_used_usd: float | None = None
    quota_reset_day: int | None = Field(default=None, description="配额重置周期（天数）")
    quota_last_reset_at: datetime | None = Field(default=None, description="当前周期开始时间")
    quota_expires_at: datetime | None = Field(default=None, description="配额过期时间")

    # 请求配置（从 Endpoint 迁移）
    max_retries: int | None = Field(default=2, description="最大重试次数")
    proxy: dict[str, Any] | None = Field(default=None, description="代理配置")
    # 超时配置（秒），为空时使用全局配置
    stream_first_byte_timeout: float | None = Field(
        default=None, description="流式请求首字节超时（秒）"
    )
    request_timeout: float | None = Field(default=None, description="非流式请求整体超时（秒）")
    claude_code_advanced: ClaudeCodeAdvancedConfig | None = Field(
        default=None, description="Claude Code 高级配置"
    )
    pool_advanced: PoolAdvancedConfig | None = Field(default=None, description="通用号池配置")
    failover_rules: FailoverRulesConfig | None = Field(default=None, description="故障转移规则配置")

    # Endpoint 统计
    total_endpoints: int = Field(default=0, description="总 Endpoint 数量")
    active_endpoints: int = Field(default=0, description="活跃 Endpoint 数量")

    # Key 统计（所有 Endpoints 的 Keys）
    total_keys: int = Field(default=0, description="总 Key 数量")
    active_keys: int = Field(default=0, description="活跃 Key 数量")

    # Model 统计
    total_models: int = Field(default=0, description="总模型数量")
    active_models: int = Field(default=0, description="活跃模型数量")
    global_model_ids: list[str] = Field(default=[], description="活跃模型关联的全局模型 ID 列表")

    # API 格式列表
    api_formats: list[str] = Field(default=[], description="支持的 API 格式列表")

    # Endpoint 健康度详情
    endpoint_health_details: list[dict[str, Any]] = Field(
        default=[],
        description="每个 Endpoint 的健康度详情 [{api_format: str, health_score: float, is_active: bool}]",
    )

    # 健康度统计
    avg_health_score: float = Field(default=1.0, description="平均健康度")
    unhealthy_endpoints: int = Field(
        default=0, description="不健康的端点数量（health_score < 0.5）"
    )

    # Provider Ops 配置状态
    ops_configured: bool = Field(default=False, description="是否配置了扩展操作（余额监控等）")
    ops_architecture_id: str | None = Field(
        default=None, description="扩展操作使用的架构 ID（如 cubence, anyrouter）"
    )

    # 时间戳
    created_at: datetime
    updated_at: datetime

    model_config = ConfigDict(from_attributes=True)


# ========== 健康监控可视化模型 ==========


class EndpointHealthEvent(BaseModel):
    """单个端点的请求事件"""

    timestamp: datetime
    status: str
    status_code: int | None = None
    latency_ms: int | None = None
    error_type: str | None = None
    error_message: str | None = None


class EndpointHealthMonitor(BaseModel):
    """端点健康监控信息"""

    endpoint_id: str
    api_format: str
    is_active: bool
    total_attempts: int
    success_count: int
    failed_count: int
    skipped_count: int
    success_rate: float = Field(default=1.0, description="最近事件窗口的成功率")
    last_event_at: datetime | None = None
    events: list[EndpointHealthEvent] = Field(default_factory=list)


class ProviderEndpointHealthMonitorResponse(BaseModel):
    """Provider 下所有端点的健康监控"""

    provider_id: str
    provider_name: str
    generated_at: datetime
    endpoints: list[EndpointHealthMonitor] = Field(default_factory=list)


class ApiFormatHealthMonitor(BaseModel):
    """按 API 格式聚合的健康监控信息"""

    api_format: str
    total_attempts: int
    success_count: int
    failed_count: int
    skipped_count: int
    success_rate: float = Field(default=1.0, description="最近事件窗口的成功率")
    provider_count: int = Field(default=0, description="参与统计的 Provider 数量")
    key_count: int = Field(default=0, description="参与统计的 API Key 数量")
    last_event_at: datetime | None = None
    events: list[EndpointHealthEvent] = Field(default_factory=list)
    timeline: list[str] = Field(
        default_factory=list,
        description="Usage 表生成的健康时间线（healthy/warning/unhealthy/unknown）",
    )
    time_range_start: datetime | None = Field(
        default=None, description="时间线所覆盖区间的开始时间"
    )
    time_range_end: datetime | None = Field(default=None, description="时间线所覆盖区间的结束时间")


class ApiFormatHealthMonitorResponse(BaseModel):
    """所有 API 格式的健康监控汇总"""

    generated_at: datetime
    formats: list[ApiFormatHealthMonitor] = Field(default_factory=list)


# ========== 公开健康监控模型（不含敏感信息） ==========


class PublicHealthEvent(BaseModel):
    """公开版单个请求事件（不含敏感信息如 provider_id、key_id）"""

    timestamp: datetime
    status: str
    status_code: int | None = None
    latency_ms: int | None = None
    error_type: str | None = None


class PublicApiFormatHealthMonitor(BaseModel):
    """公开版 API 格式健康监控信息（不含敏感信息）"""

    api_format: str
    api_path: str = Field(default="/", description="该 API 格式的本站请求路径")
    total_attempts: int = Field(default=0, description="总请求次数")
    success_count: int = Field(default=0, description="成功次数")
    failed_count: int = Field(default=0, description="失败次数")
    skipped_count: int = Field(default=0, description="跳过次数")
    success_rate: float = Field(default=1.0, description="成功率")
    last_event_at: datetime | None = None
    events: list[PublicHealthEvent] = Field(default_factory=list, description="事件列表")
    timeline: list[str] = Field(
        default_factory=list,
        description="Usage 表生成的健康时间线（healthy/warning/unhealthy/unknown）",
    )
    time_range_start: datetime | None = Field(default=None, description="时间线覆盖区间开始时间")
    time_range_end: datetime | None = Field(default=None, description="时间线覆盖区间结束时间")


class PublicApiFormatHealthMonitorResponse(BaseModel):
    """公开版健康监控汇总（不含敏感信息）"""

    generated_at: datetime
    formats: list[PublicApiFormatHealthMonitor] = Field(default_factory=list)
