"""
管理接口的 Pydantic 请求模型

提供完整的输入验证和安全过滤
"""

from __future__ import annotations

import re
from datetime import datetime
from typing import Any

from pydantic import BaseModel, Field, field_validator, model_validator

from src.core.enums import ProviderBillingType
from src.core.validators import PasswordValidator


class ProxyConfig(BaseModel):
    """代理配置"""

    # 模式 1: 手动配置代理 URL（原有）
    url: str | None = Field(None, description="代理 URL (http://, https://, socks5://)")
    username: str | None = Field(None, max_length=255, description="代理用户名")
    password: str | None = Field(None, max_length=500, description="代理密码")
    # 模式 2: ProxyNode（aether-proxy 注册的节点）
    node_id: str | None = Field(None, description="代理节点 ID")
    enabled: bool = Field(True, description="是否启用代理（false 时保留配置但不使用）")

    @field_validator("url")
    @classmethod
    def validate_proxy_url(cls, v: str | None) -> str | None:
        """验证代理 URL 格式"""
        if v is None:
            return None

        from urllib.parse import urlparse

        v = v.strip()
        if not v:
            return None

        # 检查禁止的字符（防止注入）
        if "\n" in v or "\r" in v:
            raise ValueError("代理 URL 包含非法字符")

        # 验证协议（不支持 SOCKS4）
        if not re.match(r"^(http|https|socks5)://", v, re.IGNORECASE):
            raise ValueError("代理 URL 必须以 http://, https:// 或 socks5:// 开头")

        # 验证 URL 结构
        parsed = urlparse(v)
        if not parsed.netloc:
            raise ValueError("代理 URL 必须包含有效的 host")

        # 禁止 URL 中内嵌认证信息，强制使用独立字段
        if parsed.username or parsed.password:
            raise ValueError("请勿在 URL 中包含用户名和密码，请使用独立的认证字段")

        return v

    @field_validator("node_id")
    @classmethod
    def validate_node_id(cls, v: str | None) -> str | None:
        if v is None:
            return None
        v = v.strip()
        return v or None

    @model_validator(mode="after")
    def validate_proxy_mode(self) -> "ProxyConfig":
        if not self.enabled:
            return self

        if not self.url and not self.node_id:
            raise ValueError("启用代理时，必须提供 url 或 node_id")

        if self.url and self.node_id:
            raise ValueError("url 和 node_id 不能同时设置")

        return self


class FailoverRuleItem(BaseModel):
    """故障转移规则条目"""

    pattern: str = Field(..., min_length=1, max_length=500, description="正则表达式")
    description: str = Field("", max_length=200, description="规则描述")
    status_codes: list[int] | None = Field(
        default=None,
        description="HTTP 状态码列表（可选，为空时匹配所有状态码）",
    )

    @field_validator("pattern")
    @classmethod
    def validate_pattern(cls, v: str) -> str:
        """验证正则表达式语法"""
        import re as _re

        try:
            _re.compile(v)
        except _re.error as e:
            raise ValueError(f"无效的正则表达式: {e}")
        return v

    @field_validator("status_codes")
    @classmethod
    def validate_status_codes(cls, v: list[int] | None) -> list[int] | None:
        """验证 HTTP 状态码"""
        if v is None:
            return v
        for code in v:
            if not (100 <= code <= 599):
                raise ValueError(f"无效的 HTTP 状态码: {code}")
        return v


class FailoverRulesConfig(BaseModel):
    """故障转移规则配置"""

    success_failover_patterns: list[FailoverRuleItem] = Field(
        default_factory=list,
        description="成功响应转移规则: HTTP 200 但响应体匹配正则时触发转移",
    )
    error_stop_patterns: list[FailoverRuleItem] = Field(
        default_factory=list,
        description="错误终止规则: HTTP 非 200 且响应体匹配正则时停止转移",
    )


class ScoringWeightsConfig(BaseModel):
    """多维评分权重配置。"""

    lru: float = Field(0.3, ge=0.0, le=1.0)
    latency: float = Field(0.25, ge=0.0, le=1.0)
    health: float = Field(0.2, ge=0.0, le=1.0)
    cost_remaining: float = Field(0.25, ge=0.0, le=1.0)


def _allowed_pool_preset_names() -> set[str]:
    from src.services.provider.pool.dimensions import get_preset_names

    return get_preset_names() | {"lru"}


def _preset_mode_meta(name: str) -> tuple[set[str], str | None]:
    from src.services.provider.pool.dimensions import get_preset_dimension

    dim = get_preset_dimension(name)
    if dim is None or not dim.modes:
        return set(), None

    ordered_modes = [str(mode).strip().lower() for mode in dim.modes if str(mode).strip()]
    if not ordered_modes:
        return set(), None
    modes = set(ordered_modes)
    default_mode = str(dim.default_mode or "").strip().lower()
    if not default_mode or default_mode not in modes:
        default_mode = ordered_modes[0]
    return modes, default_mode


class SchedulingPresetItem(BaseModel):
    """调度预设条目（新格式：有序对象列表）。"""

    preset: str
    enabled: bool = True
    mode: str | None = None

    @field_validator("preset")
    @classmethod
    def validate_preset(cls, v: str) -> str:
        normalized = v.strip().lower()
        allowed = _allowed_pool_preset_names()
        if normalized not in allowed:
            raise ValueError(f"无效的 preset: {normalized}")
        return normalized

    @field_validator("mode")
    @classmethod
    def normalize_mode(cls, v: str | None) -> str | None:
        if v is None:
            return None
        normalized = v.strip().lower()
        return normalized or None

    @model_validator(mode="after")
    def validate_mode(self) -> "SchedulingPresetItem":
        allowed_modes, default_mode = _preset_mode_meta(self.preset)
        if not allowed_modes:
            self.mode = None
            return self

        if self.mode is None:
            self.mode = default_mode
            return self

        if self.mode not in allowed_modes:
            raise ValueError(
                f"preset={self.preset} 的 mode 必须是: {', '.join(sorted(allowed_modes))}"
            )
        return self


class PoolAdvancedConfig(BaseModel):
    """通用号池配置（适用于所有 Provider 类型）。"""

    global_priority: int | None = Field(
        None,
        ge=0,
        le=999999,
        description="global_key 模式下号池整体优先级（数字越小越优先）",
    )
    sticky_session_ttl_seconds: int | None = Field(
        None,
        ge=60,
        le=86400,
        description="粘性会话 TTL（秒），同一对话始终路由到同一 Key。None = 禁用",
    )
    load_threshold_percent: int | None = Field(
        None,
        ge=10,
        le=100,
        description="负载率阈值（%），超过时该 Key 被降权。默认 80",
    )
    # 保留旧字段供向后兼容（新客户端不再发送）
    lru_enabled: bool = Field(True, description="LRU 调度（优先选择最久未用的 Key）")
    scheduling_mode: str | None = Field(
        None,
        pattern="^(lru|multi_score)$",
        description="号池调度模式：lru 或 multi_score",
    )
    scheduling_presets: list[SchedulingPresetItem] | list[str] | None = Field(
        None,
        description=(
            "调度预设列表（新格式：对象列表 [{preset, enabled, mode}]；"
            "旧格式：字符串列表 ['quota_balanced', ...]）"
        ),
    )
    scoring_weights: ScoringWeightsConfig | None = Field(None, description="多维评分权重")
    latency_window_seconds: int | None = Field(
        None,
        ge=300,
        le=86400,
        description="延迟窗口（秒），仅 multi_score 生效",
    )
    latency_sample_limit: int | None = Field(
        None,
        ge=10,
        le=200,
        description="每个 Key 的延迟样本上限，仅 multi_score 生效",
    )
    cost_window_seconds: int | None = Field(
        None,
        ge=3600,
        le=86400,
        description="滚动成本窗口（秒）。默认 18000（5 小时）",
    )
    cost_limit_per_key_tokens: int | None = Field(
        None, ge=0, description="每个 Key 在窗口内的最大 token 用量。None = 不限"
    )
    cost_soft_threshold_percent: int | None = Field(
        None,
        ge=0,
        le=100,
        description="成本软阈值（%），超过时优先选用其他 Key。默认 80",
    )
    rate_limit_cooldown_seconds: int | None = Field(
        None, ge=10, le=3600, description="429 冷却时间（秒）。默认 300"
    )
    overload_cooldown_seconds: int | None = Field(
        None, ge=5, le=600, description="529 冷却时间（秒）。默认 30"
    )
    proactive_refresh_seconds: int | None = Field(
        None,
        ge=60,
        le=600,
        description="OAuth Token 提前刷新秒数。默认 180（3 分钟）",
    )
    health_policy_enabled: bool = Field(
        True, description="启用号池健康策略（按上游错误码自动冷却/禁用 Key）"
    )
    unschedulable_rules: list[dict] | None = Field(
        None,
        description="关键词临时不可调度规则: [{'keyword': '...', 'duration_minutes': 5}]",
    )
    batch_concurrency: int | None = Field(
        None,
        ge=1,
        le=32,
        description="批量操作并发数（前端批量刷新 OAuth/额度等）。默认 8",
    )
    probing_enabled: bool = Field(False, description="启用主动探测（定期检查 Key 可用性）")
    probing_interval_minutes: int | None = Field(
        None,
        ge=1,
        le=1440,
        description="主动探测间隔（分钟）。默认 10",
    )
    auto_remove_banned_keys: bool = Field(False, description="检测到封号时自动清除账号")


class ClaudeCodeAdvancedConfig(BaseModel):
    """Claude Code 特有配置。"""

    max_sessions: int | None = Field(
        None, ge=1, le=1000, description="最大活跃会话数（为空表示不限制）"
    )
    session_idle_timeout_minutes: int | None = Field(
        None, ge=1, le=1440, description="会话空闲超时（分钟）"
    )
    session_id_masking_enabled: bool = Field(
        False, description="是否启用会话 ID 伪装（固定 metadata.user_id 中 session 片段）"
    )
    cache_ttl_override_enabled: bool = Field(
        False, description="是否启用 Cache TTL 强制替换（统一所有请求的 cache_control 类型）"
    )
    cache_ttl_override_target: str = Field(
        "ephemeral",
        description="Cache TTL 目标类型: ephemeral (5min) 或 1h",
        pattern="^(ephemeral|1h)$",
    )
    cli_only_enabled: bool = Field(
        False,
        description="是否仅允许 Claude Code CLI 客户端访问（非 CLI 流量返回 403）",
    )

    @model_validator(mode="after")
    def normalize_session_control(self) -> "ClaudeCodeAdvancedConfig":
        # 未启用会话限制时，不保留超时配置，避免产生误导。
        if self.max_sessions is None:
            self.session_idle_timeout_minutes = None
            return self

        # 启用会话限制但未设置超时时，回落到 5 分钟默认值。
        if self.session_idle_timeout_minutes is None:
            self.session_idle_timeout_minutes = 5
        return self


class CreateProviderRequest(BaseModel):
    """创建 Provider 请求"""

    name: str = Field(..., min_length=1, max_length=100, description="提供商名称（唯一）")
    provider_type: str | None = Field(
        default="custom",
        max_length=20,
        description="Provider 类型：custom/claude_code/codex/gemini_cli/antigravity",
    )
    description: str | None = Field(None, max_length=1000, description="描述")
    website: str | None = Field(None, max_length=500, description="官网地址")

    @field_validator("name")
    @classmethod
    def validate_name(cls, v: str) -> str:
        """验证名称格式，防止注入攻击"""
        v = v.strip()

        # 只允许安全的字符：字母、数字、下划线、连字符、空格、中文
        if not re.match(r"^[\w\s\u4e00-\u9fff-]+$", v):
            raise ValueError("名称只能包含字母、数字、下划线、连字符、空格和中文")

        # 检查 SQL 注入关键字（不区分大小写）
        sql_keywords = [
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "DROP",
            "CREATE",
            "ALTER",
            "TRUNCATE",
            "UNION",
            "EXEC",
            "EXECUTE",
            "--",
            "/*",
            "*/",
        ]
        v_upper = v.upper()
        for keyword in sql_keywords:
            if keyword in v_upper:
                raise ValueError(f"名称包含非法关键字: {keyword}")

        return v

    billing_type: str | None = Field(
        ProviderBillingType.PAY_AS_YOU_GO.value, description="计费类型"
    )
    monthly_quota_usd: float | None = Field(None, ge=0, description="周期配额（美元）")
    quota_reset_day: int | None = Field(30, ge=1, le=365, description="配额重置周期（天数）")
    quota_last_reset_at: datetime | None = Field(None, description="当前周期开始时间")
    quota_expires_at: datetime | None = Field(None, description="配额过期时间")
    provider_priority: int | None = Field(
        None, ge=0, le=10000, description="提供商优先级（数字越小越优先，留空时新建自动置顶）"
    )
    keep_priority_on_conversion: bool = Field(
        False,
        description="格式转换时是否保持优先级（True=保持原优先级，False=需要转换时降级）",
    )
    is_active: bool | None = Field(True, description="是否启用")
    concurrent_limit: int | None = Field(None, ge=0, description="并发限制")
    # 请求配置（从 Endpoint 迁移）
    max_retries: int | None = Field(2, ge=0, le=999, description="最大重试次数")
    proxy: ProxyConfig | None = Field(None, description="代理配置")
    # 超时配置（秒），为空时使用全局配置
    stream_first_byte_timeout: float | None = Field(
        None, ge=1, le=300, description="流式请求首字节超时（秒）"
    )
    request_timeout: float | None = Field(
        None, ge=1, le=600, description="非流式请求整体超时（秒）"
    )
    pool_advanced: PoolAdvancedConfig | None = Field(
        None, description="号池高级配置（适用于所有 Provider 类型）"
    )
    claude_code_advanced: ClaudeCodeAdvancedConfig | None = Field(
        None, description="Claude Code 特有配置"
    )
    failover_rules: FailoverRulesConfig | None = Field(None, description="故障转移规则配置")
    config: dict[str, Any] | None = Field(None, description="其他配置")

    @field_validator("provider_type")
    @classmethod
    def validate_provider_type(cls, v: str | None) -> str | None:
        if v is None:
            return "custom"
        v = v.strip()
        from src.core.provider_types import VALID_PROVIDER_TYPES

        if v not in VALID_PROVIDER_TYPES:
            raise ValueError(
                f"无效的 provider_type，有效值为: {', '.join(sorted(VALID_PROVIDER_TYPES))}"
            )
        return v

    @field_validator("name", "description")
    @classmethod
    def sanitize_text(cls, v: str | None) -> str | None:
        """清理文本输入，防止 XSS"""
        if v is None:
            return v

        # 移除潜在的脚本标签
        v = re.sub(r"<script.*?</script>", "", v, flags=re.IGNORECASE | re.DOTALL)
        v = re.sub(r"<iframe.*?</iframe>", "", v, flags=re.IGNORECASE | re.DOTALL)
        v = re.sub(r"javascript:", "", v, flags=re.IGNORECASE)
        v = re.sub(r"on\w+\s*=", "", v, flags=re.IGNORECASE)  # 移除事件处理器

        # 移除危险的 HTML 标签
        dangerous_tags = ["script", "iframe", "object", "embed", "link", "style"]
        for tag in dangerous_tags:
            v = re.sub(rf"<{tag}[^>]*>", "", v, flags=re.IGNORECASE)
            v = re.sub(rf"</{tag}>", "", v, flags=re.IGNORECASE)

        return v.strip()

    @field_validator("website")
    @classmethod
    def validate_website(cls, v: str | None) -> str | None:
        """验证网站地址"""
        if v is None or v.strip() == "":
            return None

        v = v.strip()

        # 自动补全 https:// 前缀
        if not re.match(r"^https?://", v, re.IGNORECASE):
            v = f"https://{v}"

        return v

    @field_validator("billing_type")
    @classmethod
    def validate_billing_type(cls, v: str | None) -> str | None:
        """验证计费类型"""
        if v is None:
            return ProviderBillingType.PAY_AS_YOU_GO.value

        try:
            ProviderBillingType(v)
            return v
        except ValueError:
            valid_types = [t.value for t in ProviderBillingType]
            raise ValueError(f"无效的计费类型，有效值为: {', '.join(valid_types)}")

    @model_validator(mode="after")
    def validate_claude_code_advanced_scope(self) -> "CreateProviderRequest":
        provider_type = (self.provider_type or "custom").strip()
        if self.claude_code_advanced is not None and provider_type != "claude_code":
            raise ValueError("claude_code_advanced 仅适用于 provider_type=claude_code")
        return self


class UpdateProviderRequest(BaseModel):
    """更新 Provider 请求"""

    name: str | None = Field(None, min_length=1, max_length=100)
    provider_type: str | None = Field(
        None,
        max_length=20,
        description="Provider 类型：custom/claude_code/codex/gemini_cli/antigravity",
    )
    description: str | None = Field(None, max_length=1000)
    website: str | None = Field(None, max_length=500)
    billing_type: str | None = None
    monthly_quota_usd: float | None = Field(None, ge=0)
    quota_reset_day: int | None = Field(None, ge=1, le=365)
    quota_last_reset_at: datetime | None = None
    quota_expires_at: datetime | None = None
    provider_priority: int | None = Field(None, ge=0, le=10000)
    keep_priority_on_conversion: bool | None = Field(
        None,
        description="格式转换时是否保持优先级（True=保持原优先级，False=需要转换时降级）",
    )
    is_active: bool | None = None
    concurrent_limit: int | None = Field(None, ge=0)
    # 请求配置（从 Endpoint 迁移）
    max_retries: int | None = Field(None, ge=0, le=999, description="最大重试次数")
    proxy: ProxyConfig | None = Field(None, description="代理配置")
    # 超时配置（秒），为空时使用全局配置
    stream_first_byte_timeout: float | None = Field(
        None, ge=1, le=300, description="流式请求首字节超时（秒）"
    )
    request_timeout: float | None = Field(
        None, ge=1, le=600, description="非流式请求整体超时（秒）"
    )
    pool_advanced: PoolAdvancedConfig | None = Field(
        None, description="号池高级配置（适用于所有 Provider 类型）"
    )
    claude_code_advanced: ClaudeCodeAdvancedConfig | None = Field(
        None, description="Claude Code 特有配置"
    )
    failover_rules: FailoverRulesConfig | None = Field(None, description="故障转移规则配置")
    enable_format_conversion: bool | None = Field(
        None, description="是否允许格式转换（提供商级别开关）"
    )
    config: dict[str, Any] | None = None

    # 复用相同的验证器
    _sanitize_text = field_validator("name", "description")(
        CreateProviderRequest.sanitize_text.__func__
    )
    _validate_website = field_validator("website")(CreateProviderRequest.validate_website.__func__)
    _validate_billing_type = field_validator("billing_type")(
        CreateProviderRequest.validate_billing_type.__func__
    )
    _validate_provider_type = field_validator("provider_type")(
        CreateProviderRequest.validate_provider_type.__func__
    )

    @model_validator(mode="after")
    def validate_claude_code_advanced_scope(self) -> "UpdateProviderRequest":
        # 更新场景下 provider_type 可能不在 payload 中，最终校验由路由层结合数据库值完成。
        if self.claude_code_advanced is not None and self.provider_type is not None:
            provider_type = (self.provider_type or "custom").strip()
            if provider_type != "claude_code":
                raise ValueError("claude_code_advanced 仅适用于 provider_type=claude_code")
        return self


class CreateEndpointRequest(BaseModel):
    """创建 Endpoint 请求"""

    provider_id: str = Field(..., description="Provider ID")
    name: str = Field(..., min_length=1, max_length=100, description="Endpoint 名称")
    base_url: str = Field(..., min_length=1, max_length=500, description="API 基础 URL")
    api_format: str = Field(
        ..., description="Endpoint signature（如 openai:chat, claude:cli, gemini:video）"
    )
    custom_path: str | None = Field(None, max_length=200, description="自定义路径")
    priority: int | None = Field(100, ge=0, le=1000, description="优先级")
    is_active: bool | None = Field(True, description="是否启用")
    concurrent_limit: int | None = Field(None, ge=0, description="并发限制")
    config: dict[str, Any] | None = Field(None, description="其他配置")
    proxy: ProxyConfig | None = Field(None, description="代理配置")

    @field_validator("name")
    @classmethod
    def validate_name(cls, v: str) -> str:
        """验证名称"""
        if not re.match(r"^[a-zA-Z0-9_-]+$", v):
            raise ValueError("名称只能包含英文字母、数字、下划线和连字符")
        return v

    @field_validator("base_url")
    @classmethod
    def validate_base_url(cls, v: str) -> str:
        """验证 API URL"""
        if not re.match(r"^https?://", v, re.IGNORECASE):
            raise ValueError("URL 必须以 http:// 或 https:// 开头")

        return v.rstrip("/")  # 移除末尾斜杠

    @field_validator("api_format")
    @classmethod
    def validate_api_format(cls, v: str) -> str:
        """验证 API 格式"""
        from src.core.api_format import list_endpoint_definitions, resolve_endpoint_definition
        from src.core.api_format.signature import normalize_signature_key

        normalized = normalize_signature_key(v)
        if resolve_endpoint_definition(normalized) is None:
            valid_formats = [d.signature_key for d in list_endpoint_definitions()]
            raise ValueError(f"无效的 api_format，有效值为: {', '.join(valid_formats)}")
        return normalized

    @field_validator("custom_path")
    @classmethod
    def validate_custom_path(cls, v: str | None) -> str | None:
        """验证自定义路径"""
        if v is None:
            return v

        # 确保路径不包含危险字符
        if not re.match(r"^[/a-zA-Z0-9_-]+$", v):
            raise ValueError("路径只能包含字母、数字、斜杠、下划线和连字符")

        return v


class UpdateEndpointRequest(BaseModel):
    """更新 Endpoint 请求"""

    name: str | None = Field(None, min_length=1, max_length=100)
    base_url: str | None = Field(None, min_length=1, max_length=500)
    api_format: str | None = None
    custom_path: str | None = Field(None, max_length=200)
    priority: int | None = Field(None, ge=0, le=1000)
    is_active: bool | None = None
    concurrent_limit: int | None = Field(None, ge=0)
    config: dict[str, Any] | None = None
    proxy: ProxyConfig | None = Field(None, description="代理配置")

    # 复用验证器
    _validate_name = field_validator("name")(CreateEndpointRequest.validate_name.__func__)
    _validate_base_url = field_validator("base_url")(
        CreateEndpointRequest.validate_base_url.__func__
    )
    _validate_api_format = field_validator("api_format")(
        CreateEndpointRequest.validate_api_format.__func__
    )
    _validate_custom_path = field_validator("custom_path")(
        CreateEndpointRequest.validate_custom_path.__func__
    )


class CreateAPIKeyRequest(BaseModel):
    """创建 API Key 请求"""

    endpoint_id: str = Field(..., description="Endpoint ID")
    api_key: str = Field(..., min_length=1, max_length=10000, description="API Key")
    priority: int | None = Field(100, ge=0, le=1000, description="优先级")
    is_active: bool | None = Field(True, description="是否启用")
    rpm_limit: int | None = Field(None, ge=0, description="RPM 限制（NULL=自适应）")
    notes: str | None = Field(None, max_length=500, description="备注")

    @field_validator("api_key")
    @classmethod
    def validate_api_key(cls, v: str) -> str:
        """验证 API Key"""
        # 移除首尾空白
        v = v.strip()

        # 检查危险字符（不应包含 SQL 注入字符）
        dangerous_chars = ["'", '"', ";", "--", "/*", "*/", "<", ">"]
        for char in dangerous_chars:
            if char in v:
                raise ValueError(f"API Key 包含非法字符: {char}")

        return v

    @field_validator("notes")
    @classmethod
    def sanitize_notes(cls, v: str | None) -> str | None:
        """清理备注"""
        if v is None:
            return v
        # 复用文本清理逻辑
        return CreateProviderRequest.sanitize_text(v)


class UpdateUserRequest(BaseModel):
    """更新用户请求"""

    username: str | None = Field(None, min_length=1, max_length=50)
    email: str | None = Field(None, max_length=100)
    password: str | None = Field(None, description="新密码（留空保持不变）")
    unlimited: bool | None = Field(None, description="是否无限制（true=无限制，false=有限制）")
    is_active: bool | None = None
    role: str | None = None
    allowed_providers: list[str] | None = Field(None, description="允许使用的提供商 ID 列表")
    allowed_api_formats: list[str] | None = Field(None, description="允许使用的 API 格式列表")
    allowed_models: list[str] | None = Field(None, description="允许使用的模型名称列表")
    rate_limit: int | None = Field(
        None,
        ge=0,
        description="每分钟请求限制；null 表示继承系统默认，0 表示不限制",
    )

    @field_validator("username")
    @classmethod
    def validate_username(cls, v: str | None) -> str | None:
        """验证用户名"""
        if v is None:
            return v

        if not re.match(r"^[a-zA-Z0-9_.\-]+$", v):
            raise ValueError("用户名只能包含字母、数字、下划线、连字符和点号")

        return v

    @field_validator("email")
    @classmethod
    def validate_email(cls, v: str | None) -> str | None:
        """验证邮箱"""
        if v is None:
            return v

        # 简单的邮箱格式验证
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式不正确")

        return v.lower()

    @field_validator("password")
    @classmethod
    def validate_password(cls, v: str | None) -> str | None:
        """验证密码"""
        if v is None:
            return v

        valid, error_msg = PasswordValidator.validate_basic_input(v)
        if not valid:
            raise ValueError(error_msg or "密码格式无效")

        return v

    @field_validator("role")
    @classmethod
    def validate_role(cls, v: str | None) -> str | None:
        """验证角色"""
        if v is None:
            return v

        valid_roles = ["admin", "user"]
        if v not in valid_roles:
            raise ValueError(f"无效的角色，有效值为: {', '.join(valid_roles)}")

        return v
