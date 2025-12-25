"""
ProviderEndpoint 相关的 API 模型定义
"""

import re
from datetime import datetime
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, ConfigDict, Field, field_validator

from src.models.admin_requests import ProxyConfig

# ========== ProviderEndpoint CRUD ==========


class ProviderEndpointCreate(BaseModel):
    """创建 Endpoint 请求"""

    provider_id: str = Field(..., description="Provider ID")
    api_format: str = Field(..., description="API 格式 (CLAUDE, OPENAI, CLAUDE_CLI, OPENAI_CLI)")
    base_url: str = Field(..., min_length=1, max_length=500, description="API 基础 URL")

    # 请求配置
    headers: Optional[Dict[str, Any]] = Field(default=None, description="自定义请求头或headers规则")
    timeout: int = Field(default=300, ge=10, le=600, description="超时时间（秒）")
    max_retries: int = Field(default=3, ge=0, le=10, description="最大重试次数")

    # 限制
    max_concurrent: Optional[int] = Field(default=None, ge=1, description="最大并发数")
    rate_limit: Optional[int] = Field(default=None, ge=1, description="速率限制（请求/秒）")

    # 额外配置
    config: Optional[Dict[str, Any]] = Field(default=None, description="额外配置（JSON）")

    # 代理配置
    proxy: Optional[ProxyConfig] = Field(default=None, description="代理配置")

    @field_validator("api_format")
    @classmethod
    def validate_api_format(cls, v: str) -> str:
        """验证 API 格式"""
        from src.core.enums import APIFormat

        allowed = [fmt.value for fmt in APIFormat]
        v_upper = v.upper()
        if v_upper not in allowed:
            raise ValueError(f"API 格式必须是 {allowed} 之一")
        return v_upper

    @field_validator("base_url")
    @classmethod
    def validate_base_url(cls, v: str) -> str:
        if not re.match(r"^https?://", v, re.IGNORECASE):
            raise ValueError("URL 必须以 http:// 或 https:// 开头")

        return v.rstrip("/")  # 移除末尾斜杠


class ProviderEndpointUpdate(BaseModel):
    """更新 Endpoint 请求"""

    base_url: Optional[str] = Field(
        default=None, min_length=1, max_length=500, description="API 基础 URL"
    )
    headers: Optional[Dict[str, Any]] = Field(default=None, description="自定义请求头或headers规则")
    timeout: Optional[int] = Field(default=None, ge=10, le=600, description="超时时间（秒）")
    max_retries: Optional[int] = Field(default=None, ge=0, le=10, description="最大重试次数")
    max_concurrent: Optional[int] = Field(default=None, ge=1, description="最大并发数")
    rate_limit: Optional[int] = Field(default=None, ge=1, description="速率限制")
    is_active: Optional[bool] = Field(default=None, description="是否启用")
    config: Optional[Dict[str, Any]] = Field(default=None, description="额外配置")
    proxy: Optional[ProxyConfig] = Field(default=None, description="代理配置")

    @field_validator("base_url")
    @classmethod
    def validate_base_url(cls, v: Optional[str]) -> Optional[str]:
        """验证 API URL"""
        if v is None:
            return v

        if not re.match(r"^https?://", v, re.IGNORECASE):
            raise ValueError("URL 必须以 http:// 或 https:// 开头")

        return v.rstrip("/")  # 移除末尾斜杠


class ProviderEndpointResponse(BaseModel):
    """Endpoint 响应"""

    id: str
    provider_id: str
    provider_name: str  # 冗余字段，方便前端显示

    # API 配置
    api_format: str
    base_url: str

    # 请求配置
    headers: Optional[Dict[str, Any]] = None
    timeout: int
    max_retries: int

    # 限制
    max_concurrent: Optional[int] = None
    rate_limit: Optional[int] = None

    # 状态
    is_active: bool

    # 额外配置
    config: Optional[Dict[str, Any]] = None

    # 代理配置（响应中密码已脱敏）
    proxy: Optional[Dict[str, Any]] = Field(default=None, description="代理配置（密码已脱敏）")

    # 统计（从 Keys 聚合）
    total_keys: int = Field(default=0, description="总 Key 数量")
    active_keys: int = Field(default=0, description="活跃 Key 数量")

    # 时间戳
    created_at: datetime
    updated_at: datetime

    model_config = ConfigDict(from_attributes=True)


# ========== ProviderAPIKey 相关（新架构） ==========


class EndpointAPIKeyCreate(BaseModel):
    """为 Endpoint 添加 API Key"""

    endpoint_id: str = Field(..., description="Endpoint ID")
    api_key: str = Field(..., min_length=10, max_length=500, description="API Key（将自动加密）")
    name: str = Field(..., min_length=1, max_length=100, description="密钥名称（必填，用于识别）")

    # 成本计算
    rate_multiplier: float = Field(
        default=1.0, ge=0.01, description="成本倍率（真实成本 = 表面成本 × 倍率）"
    )

    # 优先级和限制（数字越小越优先）
    internal_priority: int = Field(default=50, description="Endpoint 内部优先级（提供商优先模式）")
    # max_concurrent: NULL=自适应模式（系统自动学习），数字=固定限制模式
    max_concurrent: Optional[int] = Field(
        default=None, ge=1, description="最大并发数（NULL=自适应模式）"
    )
    rate_limit: Optional[int] = Field(default=None, ge=1, description="速率限制")
    daily_limit: Optional[int] = Field(default=None, ge=1, description="每日限制")
    monthly_limit: Optional[int] = Field(default=None, ge=1, description="每月限制")
    allowed_models: Optional[List[str]] = Field(
        default=None, description="允许使用的模型列表（null = 支持所有模型）"
    )

    # 能力标签
    capabilities: Optional[Dict[str, bool]] = Field(
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
    note: Optional[str] = Field(default=None, max_length=500, description="备注说明（可选）")

    @field_validator("api_key")
    @classmethod
    def validate_api_key(cls, v: str) -> str:
        """验证 API Key 安全性"""
        # 移除首尾空白
        v = v.strip()

        # 检查最小长度
        if len(v) < 10:
            raise ValueError("API Key 长度不能少于 10 个字符")

        # 检查危险字符（SQL 注入防护）
        dangerous_chars = ["'", '"', ";", "--", "/*", "*/", "<", ">"]
        for char in dangerous_chars:
            if char in v:
                raise ValueError(f"API Key 包含非法字符: {char}")

        return v

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
    def validate_note(cls, v: Optional[str]) -> Optional[str]:
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

    api_key: Optional[str] = Field(
        default=None, min_length=10, max_length=500, description="API Key（将自动加密）"
    )
    name: Optional[str] = Field(default=None, min_length=1, max_length=100, description="密钥名称")
    rate_multiplier: Optional[float] = Field(default=None, ge=0.01, description="成本倍率")
    internal_priority: Optional[int] = Field(
        default=None, description="Endpoint 内部优先级（提供商优先模式，数字越小越优先）"
    )
    global_priority: Optional[int] = Field(
        default=None, description="全局 Key 优先级（全局 Key 优先模式，数字越小越优先）"
    )
    # max_concurrent: 使用特殊标记区分"未提供"和"设置为 null（自适应模式）"
    # - 不提供字段：不更新
    # - 提供 null：切换为自适应模式
    # - 提供数字：设置固定并发限制
    max_concurrent: Optional[int] = Field(default=None, ge=1, description="最大并发数（null=自适应模式）")
    rate_limit: Optional[int] = Field(default=None, ge=1, description="速率限制")
    daily_limit: Optional[int] = Field(default=None, ge=1, description="每日限制")
    monthly_limit: Optional[int] = Field(default=None, ge=1, description="每月限制")
    allowed_models: Optional[List[str]] = Field(default=None, description="允许使用的模型列表")
    capabilities: Optional[Dict[str, bool]] = Field(
        default=None, description="Key 能力标签，如 {'cache_1h': true, 'context_1m': true}"
    )
    cache_ttl_minutes: Optional[int] = Field(
        default=None, ge=0, le=60, description="缓存 TTL（分钟），0=禁用"
    )
    max_probe_interval_minutes: Optional[int] = Field(
        default=None, ge=2, le=32, description="熔断探测间隔（分钟），范围 2-32"
    )
    is_active: Optional[bool] = Field(default=None, description="是否启用")
    note: Optional[str] = Field(default=None, max_length=500, description="备注说明")

    @field_validator("api_key")
    @classmethod
    def validate_api_key(cls, v: Optional[str]) -> Optional[str]:
        """验证 API Key 安全性"""
        if v is None:
            return v

        v = v.strip()
        if len(v) < 10:
            raise ValueError("API Key 长度不能少于 10 个字符")

        dangerous_chars = ["'", '"', ";", "--", "/*", "*/", "<", ">"]
        for char in dangerous_chars:
            if char in v:
                raise ValueError(f"API Key 包含非法字符: {char}")

        return v

    @field_validator("name")
    @classmethod
    def validate_name(cls, v: Optional[str]) -> Optional[str]:
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
    def validate_note(cls, v: Optional[str]) -> Optional[str]:
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
    endpoint_id: str

    # Key 信息（脱敏）
    api_key_masked: str = Field(..., description="脱敏后的 Key")
    api_key_plain: Optional[str] = Field(default=None, description="完整的 Key")
    name: str = Field(..., description="密钥名称")

    # 成本计算
    rate_multiplier: float = Field(default=1.0, description="成本倍率")

    # 优先级和限制
    internal_priority: int = Field(default=50, description="Endpoint 内部优先级")
    global_priority: Optional[int] = Field(default=None, description="全局 Key 优先级")
    max_concurrent: Optional[int] = None
    rate_limit: Optional[int] = None
    daily_limit: Optional[int] = None
    monthly_limit: Optional[int] = None
    allowed_models: Optional[List[str]] = None
    capabilities: Optional[Dict[str, bool]] = Field(
        default=None, description="Key 能力标签"
    )

    # 缓存与熔断配置
    cache_ttl_minutes: int = Field(default=5, description="缓存 TTL（分钟），0=禁用")
    max_probe_interval_minutes: int = Field(default=32, description="熔断探测间隔（分钟）")

    # 健康度
    health_score: float
    consecutive_failures: int
    last_failure_at: Optional[datetime] = None

    # 熔断器状态（滑动窗口 + 半开模式）
    circuit_breaker_open: bool = Field(default=False, description="熔断器是否打开")
    circuit_breaker_open_at: Optional[datetime] = Field(default=None, description="熔断器打开时间")
    next_probe_at: Optional[datetime] = Field(default=None, description="下次进入半开状态时间")
    half_open_until: Optional[datetime] = Field(default=None, description="半开状态结束时间")
    half_open_successes: Optional[int] = Field(default=0, description="半开状态成功次数")
    half_open_failures: Optional[int] = Field(default=0, description="半开状态失败次数")
    request_results_window: Optional[List[dict]] = Field(None, description="请求结果滑动窗口")

    # 使用统计
    request_count: int
    success_count: int
    error_count: int
    success_rate: float = Field(default=0.0, description="成功率")
    avg_response_time_ms: float = Field(default=0.0, description="平均响应时间（毫秒）")

    # 状态
    is_active: bool

    # 自适应并发信息
    is_adaptive: bool = Field(default=False, description="是否为自适应模式（max_concurrent=NULL）")
    learned_max_concurrent: Optional[int] = Field(None, description="学习到的并发限制")
    effective_limit: Optional[int] = Field(None, description="当前有效限制")
    # 滑动窗口利用率采样
    utilization_samples: Optional[List[dict]] = Field(None, description="利用率采样窗口")
    last_probe_increase_at: Optional[datetime] = Field(None, description="上次探测性扩容时间")
    concurrent_429_count: Optional[int] = None
    rpm_429_count: Optional[int] = None
    last_429_at: Optional[datetime] = None
    last_429_type: Optional[str] = None

    # 备注
    note: Optional[str] = None

    # 时间戳
    last_used_at: Optional[datetime] = None
    created_at: datetime
    updated_at: datetime

    model_config = ConfigDict(from_attributes=True)


# ========== 健康监控相关 ==========


class HealthStatusResponse(BaseModel):
    """健康状态响应（仅 Key 级别）"""

    # Key 健康状态
    key_id: str
    key_health_score: float
    key_consecutive_failures: int
    key_last_failure_at: Optional[datetime] = None
    key_is_active: bool
    key_statistics: Optional[Dict[str, Any]] = None

    # 熔断器状态（滑动窗口 + 半开模式）
    circuit_breaker_open: bool = False
    circuit_breaker_open_at: Optional[datetime] = None
    next_probe_at: Optional[datetime] = None
    half_open_until: Optional[datetime] = None
    half_open_successes: int = 0
    half_open_failures: int = 0


class HealthSummaryResponse(BaseModel):
    """健康状态摘要"""

    endpoints: Dict[str, int] = Field(..., description="Endpoint 统计 (total, active, unhealthy)")
    keys: Dict[str, int] = Field(..., description="Key 统计 (total, active, unhealthy)")


# ========== 并发控制相关 ==========


class ConcurrencyStatusResponse(BaseModel):
    """并发状态响应"""

    endpoint_id: Optional[str] = None
    endpoint_current_concurrency: int = Field(default=0, description="Endpoint 当前并发数")
    endpoint_max_concurrent: Optional[int] = Field(default=None, description="Endpoint 最大并发数")

    key_id: Optional[str] = None
    key_current_concurrency: int = Field(default=0, description="Key 当前并发数")
    key_max_concurrent: Optional[int] = Field(default=None, description="Key 最大并发数")


class ResetConcurrencyRequest(BaseModel):
    """重置并发计数请求"""

    endpoint_id: Optional[str] = Field(default=None, description="Endpoint ID（可选）")
    key_id: Optional[str] = Field(default=None, description="Key ID（可选）")


class KeyPriorityItem(BaseModel):
    """单个 Key 优先级项"""

    key_id: str = Field(..., description="Key ID")
    internal_priority: int = Field(..., ge=0, description="Endpoint 内部优先级（数字越小越优先）")


class BatchUpdateKeyPriorityRequest(BaseModel):
    """批量更新 Key 优先级请求"""

    priorities: List[KeyPriorityItem] = Field(..., min_length=1, description="Key 优先级列表")


# ========== 提供商摘要（增强版） ==========


class ProviderUpdateRequest(BaseModel):
    """Provider 基础配置更新请求"""

    display_name: Optional[str] = Field(None, min_length=1, max_length=100)
    description: Optional[str] = None
    website: Optional[str] = Field(None, max_length=500, description="主站网站")
    priority: Optional[int] = None
    weight: Optional[float] = Field(None, gt=0)
    provider_priority: Optional[int] = Field(None, description="提供商优先级(数字越小越优先)")
    is_active: Optional[bool] = None
    billing_type: Optional[str] = Field(
        None, description="计费类型：monthly_quota/pay_as_you_go/free_tier"
    )
    monthly_quota_usd: Optional[float] = Field(None, ge=0, description="订阅配额（美元）")
    quota_reset_day: Optional[int] = Field(None, ge=1, le=31, description="配额重置日（1-31）")
    quota_expires_at: Optional[datetime] = Field(None, description="配额过期时间")
    rpm_limit: Optional[int] = Field(
        None, ge=0, description="每分钟请求数限制（NULL=无限制，0=禁止请求）"
    )


class ProviderWithEndpointsSummary(BaseModel):
    """Provider 和 Endpoints 摘要"""

    # Provider 基本信息
    id: str
    name: str
    display_name: str
    description: Optional[str] = None
    website: Optional[str] = None
    provider_priority: int = Field(default=100, description="提供商优先级(数字越小越优先)")
    is_active: bool

    # 计费相关字段
    billing_type: Optional[str] = None
    monthly_quota_usd: Optional[float] = None
    monthly_used_usd: Optional[float] = None
    quota_reset_day: Optional[int] = Field(default=None, description="配额重置周期（天数）")
    quota_last_reset_at: Optional[datetime] = Field(default=None, description="当前周期开始时间")
    quota_expires_at: Optional[datetime] = Field(default=None, description="配额过期时间")

    # RPM 限制
    rpm_limit: Optional[int] = Field(
        default=None, description="每分钟请求数限制（NULL=无限制，0=禁止请求）"
    )
    rpm_used: Optional[int] = Field(default=None, description="当前分钟已用请求数")
    rpm_reset_at: Optional[datetime] = Field(default=None, description="RPM 重置时间")

    # Endpoint 统计
    total_endpoints: int = Field(default=0, description="总 Endpoint 数量")
    active_endpoints: int = Field(default=0, description="活跃 Endpoint 数量")

    # Key 统计（所有 Endpoints 的 Keys）
    total_keys: int = Field(default=0, description="总 Key 数量")
    active_keys: int = Field(default=0, description="活跃 Key 数量")

    # Model 统计
    total_models: int = Field(default=0, description="总模型数量")
    active_models: int = Field(default=0, description="活跃模型数量")

    # API 格式列表
    api_formats: List[str] = Field(default=[], description="支持的 API 格式列表")

    # Endpoint 健康度详情
    endpoint_health_details: List[Dict[str, Any]] = Field(
        default=[],
        description="每个 Endpoint 的健康度详情 [{api_format: str, health_score: float, is_active: bool}]",
    )

    # 健康度统计
    avg_health_score: float = Field(default=1.0, description="平均健康度")
    unhealthy_endpoints: int = Field(
        default=0, description="不健康的端点数量（health_score < 0.5）"
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
    status_code: Optional[int] = None
    latency_ms: Optional[int] = None
    error_type: Optional[str] = None
    error_message: Optional[str] = None


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
    last_event_at: Optional[datetime] = None
    events: List[EndpointHealthEvent] = Field(default_factory=list)


class ProviderEndpointHealthMonitorResponse(BaseModel):
    """Provider 下所有端点的健康监控"""

    provider_id: str
    provider_name: str
    generated_at: datetime
    endpoints: List[EndpointHealthMonitor] = Field(default_factory=list)


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
    last_event_at: Optional[datetime] = None
    events: List[EndpointHealthEvent] = Field(default_factory=list)
    timeline: List[str] = Field(
        default_factory=list,
        description="Usage 表生成的健康时间线（healthy/warning/unhealthy/unknown）",
    )
    time_range_start: Optional[datetime] = Field(
        default=None, description="时间线所覆盖区间的开始时间"
    )
    time_range_end: Optional[datetime] = Field(
        default=None, description="时间线所覆盖区间的结束时间"
    )


class ApiFormatHealthMonitorResponse(BaseModel):
    """所有 API 格式的健康监控汇总"""

    generated_at: datetime
    formats: List[ApiFormatHealthMonitor] = Field(default_factory=list)


# ========== 公开健康监控模型（不含敏感信息） ==========


class PublicHealthEvent(BaseModel):
    """公开版单个请求事件（不含敏感信息如 provider_id、key_id）"""

    timestamp: datetime
    status: str
    status_code: Optional[int] = None
    latency_ms: Optional[int] = None
    error_type: Optional[str] = None


class PublicApiFormatHealthMonitor(BaseModel):
    """公开版 API 格式健康监控信息（不含敏感信息）"""

    api_format: str
    api_path: str = Field(default="/", description="该 API 格式的本站请求路径")
    total_attempts: int = Field(default=0, description="总请求次数")
    success_count: int = Field(default=0, description="成功次数")
    failed_count: int = Field(default=0, description="失败次数")
    skipped_count: int = Field(default=0, description="跳过次数")
    success_rate: float = Field(default=1.0, description="成功率")
    last_event_at: Optional[datetime] = None
    events: List[PublicHealthEvent] = Field(default_factory=list, description="事件列表")
    timeline: List[str] = Field(
        default_factory=list,
        description="Usage 表生成的健康时间线（healthy/warning/unhealthy/unknown）",
    )
    time_range_start: Optional[datetime] = Field(
        default=None, description="时间线覆盖区间开始时间"
    )
    time_range_end: Optional[datetime] = Field(
        default=None, description="时间线覆盖区间结束时间"
    )


class PublicApiFormatHealthMonitorResponse(BaseModel):
    """公开版健康监控汇总（不含敏感信息）"""

    generated_at: datetime
    formats: List[PublicApiFormatHealthMonitor] = Field(default_factory=list)
