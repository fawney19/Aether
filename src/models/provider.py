"""
提供商相关数据库模型

包含: Provider, ProviderEndpoint, ProxyNodeStatus, ProxyNode, ProviderAPIKey
"""

from __future__ import annotations

import uuid
from datetime import datetime, timezone
from enum import Enum as PyEnum

from sqlalchemy import (
    JSON,
    BigInteger,
    Boolean,
    Column,
    DateTime,
    Enum,
    Float,
    ForeignKey,
    Index,
    Integer,
    String,
    Text,
    UniqueConstraint,
)
from sqlalchemy.dialects.postgresql import JSONB
from sqlalchemy.orm import relationship

from src.core.enums import ProviderBillingType

from ._base import Base, ExportMixin


class Provider(ExportMixin, Base):
    """提供商配置表"""

    __tablename__ = "providers"

    _export_exclude = frozenset(
        {
            "id",
            "monthly_used_usd",
            "quota_last_reset_at",
            "quota_expires_at",
            "created_at",
            "updated_at",
        }
    )

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    name = Column(String(100), unique=True, nullable=False, index=True)  # 提供商名称（唯一）
    description = Column(Text, nullable=True)  # 提供商描述
    website = Column(String(500), nullable=True)  # 主站网站

    # Provider 类型（用于模板化固定 Provider / 自定义 Provider）
    # - custom: 自定义
    # - claude_code / codex / gemini_cli / antigravity: 固定类型
    provider_type = Column(String(20), default="custom", nullable=False)

    # 计费类型配置
    billing_type = Column(
        Enum(
            ProviderBillingType,
            name="providerbillingtype",
            create_type=False,
            values_callable=lambda x: [e.value for e in x],
        ),
        default=ProviderBillingType.PAY_AS_YOU_GO,
        nullable=False,
    )

    # 月卡配置
    monthly_quota_usd = Column(Float, nullable=True)  # 月卡总额度
    monthly_used_usd = Column(Float, default=0.0)  # 本月已用额度
    quota_reset_day = Column(Integer, default=30)  # 额度重置周期(天数)，例如：7=每周，30=每月
    quota_last_reset_at = Column(DateTime(timezone=True), nullable=True)  # 上次额度重置时间
    quota_expires_at = Column(DateTime(timezone=True), nullable=True)  # 月卡过期时间

    # 提供商优先级 (数字越小越优先，用于提供商优先模式下的 Provider 排序)
    # 0-10: 急需消耗(如即将过期的月卡)
    # 11-50: 优先消耗(月卡)
    # 51-100: 正常消费(按量付费)
    # 101+: 备用(高成本或限制严格的)
    provider_priority = Column(Integer, default=100)

    # 格式转换时是否保持优先级（默认 False）
    # - False: 需要格式转换时，该提供商的候选会被降级到不需要转换的候选之后
    # - True: 即使需要格式转换，也保持原优先级排名
    # 注意：如果系统配置 keep_priority_on_conversion=true，此字段被忽略（所有提供商都保持优先级）
    keep_priority_on_conversion = Column(Boolean, default=False, nullable=False)

    # 是否允许格式转换（默认 False）
    # - True: 该提供商可以作为格式转换的目标（全局开关关闭时也可跳过端点检查）
    # - False: 默认不作为格式转换目标；此时需要端点 format_acceptance_config 显式允许才可跨格式
    # 优先级逻辑：
    # - 全局开关 ON  -> 强制允许跨格式（忽略此字段与端点检查）
    # - 全局开关 OFF -> 若此字段 ON -> 允许跨格式（跳过端点检查）
    # - 否则        -> 由端点 format_acceptance_config 决定是否允许
    enable_format_conversion = Column(Boolean, default=False, nullable=False)

    # 状态
    is_active = Column(Boolean, default=True, nullable=False)

    # 限制
    concurrent_limit = Column(Integer, nullable=True)  # 并发请求限制

    # 请求配置
    max_retries = Column(Integer, default=2, nullable=True)  # 最大重试次数
    proxy = Column(JSONB, nullable=True)  # 代理配置: {url, username, password, enabled}

    # 超时配置（秒），为 None 时使用全局配置
    stream_first_byte_timeout = Column(Float, nullable=True)  # 流式请求首字节超时
    request_timeout = Column(Float, nullable=True)  # 非流式请求整体超时

    # 配置
    config = Column(JSON, nullable=True)  # 额外配置（如Azure deployment name等）

    # 时间戳
    created_at = Column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False
    )
    updated_at = Column(
        DateTime(timezone=True),
        default=lambda: datetime.now(timezone.utc),
        onupdate=lambda: datetime.now(timezone.utc),
        nullable=False,
    )

    # 关系
    models = relationship("Model", back_populates="provider", cascade="all, delete-orphan")
    endpoints = relationship(
        "ProviderEndpoint", back_populates="provider", cascade="all, delete-orphan"
    )
    api_keys = relationship(
        "ProviderAPIKey", back_populates="provider", cascade="all, delete-orphan"
    )
    api_key_mappings = relationship(
        "ApiKeyProviderMapping", back_populates="provider", cascade="all, delete-orphan"
    )
    usage_tracking = relationship(
        "ProviderUsageTracking", back_populates="provider", cascade="all, delete-orphan"
    )


class ProviderEndpoint(ExportMixin, Base):
    """提供商端点 - 一个提供商可以有多个 API 格式端点"""

    __tablename__ = "provider_endpoints"

    _export_exclude = frozenset(
        {
            "id",
            "provider_id",
            "api_family",
            "endpoint_kind",
            "created_at",
            "updated_at",
        }
    )

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    provider_id = Column(String(36), ForeignKey("providers.id", ondelete="CASCADE"), nullable=False)

    # API 格式和配置
    # 新模式：存储 endpoint signature key（family:kind），如 "openai:chat"
    api_format = Column(String(50), nullable=False)
    # 新架构字段（Phase 1/3）：用于将 api_format 拆分为结构化维度
    api_family = Column(String(50), nullable=True)  # openai/claude/gemini
    endpoint_kind = Column(String(50), nullable=True)  # chat/cli/video/...
    base_url = Column(String(500), nullable=False)

    # 请求配置
    header_rules = Column(JSON, nullable=True)  # 请求头规则 [{action, key, value, from, to}]
    body_rules = Column(JSON, nullable=True)  # 请求体规则 [{action, path, value, from, to}]
    max_retries = Column(Integer, default=2)  # 最大重试次数

    # 状态
    is_active = Column(Boolean, default=True, nullable=False)

    # 路径配置
    custom_path = Column(
        String(200), nullable=True
    )  # 自定义请求路径，为空则使用 API 格式的默认路径

    # 额外配置
    config = Column(JSON, nullable=True)  # 端点特定配置（不推荐使用，优先使用专用字段）

    # 格式转换配置
    format_acceptance_config = Column(
        JSON,
        nullable=True,
        default=None,
        comment="格式接受策略配置（跨格式转换开关/白黑名单等）",
    )

    # 代理配置
    proxy = Column(JSONB, nullable=True)  # 代理配置: {url, username, password}

    # 时间戳
    created_at = Column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False
    )
    updated_at = Column(
        DateTime(timezone=True),
        default=lambda: datetime.now(timezone.utc),
        onupdate=lambda: datetime.now(timezone.utc),
        nullable=False,
    )

    # 关系
    provider = relationship("Provider", back_populates="endpoints")

    # 唯一约束和索引在表定义后
    __table_args__ = (
        UniqueConstraint("provider_id", "api_format", name="uq_provider_api_format"),
        Index("idx_endpoint_format_active", "api_format", "is_active"),
        Index("idx_provider_family_kind", "provider_id", "api_family", "endpoint_kind"),
    )


class ProxyNodeStatus(PyEnum):
    """代理节点状态"""

    ONLINE = "online"
    UNHEALTHY = "unhealthy"
    OFFLINE = "offline"


class ProxyNode(Base):
    """代理节点表（aether-proxy 自动注册 + 手动添加）"""

    __tablename__ = "proxy_nodes"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    name = Column(String(100), nullable=False)  # 节点名
    ip = Column(String(512), nullable=False)  # 公网 IP 或手动节点的主机名（含协议前缀）
    port = Column(Integer, nullable=False)  # 代理端口
    region = Column(String(100), nullable=True)  # 区域标签

    # 手动节点专用字段
    is_manual = Column(Boolean, default=False, nullable=False, comment="是否为手动添加的代理节点")
    proxy_url = Column(String(500), nullable=True, comment="手动节点的完整代理 URL")
    proxy_username = Column(String(255), nullable=True, comment="手动节点的代理用户名")
    proxy_password = Column(String(500), nullable=True, comment="手动节点的代理密码")

    status = Column(
        Enum(
            ProxyNodeStatus,
            name="proxynodestatus",
            create_type=False,
            values_callable=lambda x: [e.value for e in x],
        ),
        default=ProxyNodeStatus.ONLINE,
        nullable=False,
    )

    registered_by = Column(
        String(36),
        ForeignKey("users.id", ondelete="SET NULL"),
        nullable=True,
        comment="注册该节点的管理员用户 ID（可空）",
    )
    last_heartbeat_at = Column(DateTime(timezone=True), nullable=True)
    heartbeat_interval = Column(Integer, default=30, nullable=False)

    # 性能指标（心跳上报）
    active_connections = Column(Integer, default=0, nullable=False)
    total_requests = Column(BigInteger, default=0, nullable=False)
    avg_latency_ms = Column(Float, nullable=True)

    # 硬件信息（注册时上报，JSON 可扩展）
    hardware_info = Column(
        JSON,
        nullable=True,
        comment="硬件信息 (cpu_cores, total_memory_mb, os_info, fd_limit, ...)",
    )
    estimated_max_concurrency = Column(
        Integer, nullable=True, comment="基于硬件估算的最大并发连接数"
    )

    # 隧道模式（proxy 主动连接 Aether 的 WebSocket 隧道）
    tunnel_mode = Column(
        Boolean, default=False, nullable=False, comment="是否使用 WebSocket 隧道模式"
    )
    tunnel_connected = Column(Boolean, default=False, nullable=False, comment="隧道是否已连接")
    tunnel_connected_at = Column(
        DateTime(timezone=True), nullable=True, comment="隧道最近一次建立时间"
    )

    # 管理端远程配置（通过心跳下发给 aether-proxy）
    remote_config = Column(
        JSON,
        nullable=True,
        comment="管理端下发的远程配置 (allowed_ports, log_level, heartbeat_interval)",
    )
    config_version = Column(
        Integer, default=0, nullable=False, comment="远程配置版本号，每次更新 +1"
    )

    created_at = Column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False
    )
    updated_at = Column(
        DateTime(timezone=True),
        default=lambda: datetime.now(timezone.utc),
        onupdate=lambda: datetime.now(timezone.utc),
        nullable=False,
    )

    __table_args__ = (UniqueConstraint("ip", "port", name="uq_proxy_node_ip_port"),)


class ProviderAPIKey(ExportMixin, Base):
    """Provider API密钥表 - 直接归属于 Provider，支持多种 API 格式"""

    __tablename__ = "provider_api_keys"

    _export_exclude = frozenset(
        {
            "id",
            "provider_id",
            # 加密存储，导出时需手动解密
            "api_key",
            "auth_config",
            # 运行时状态 / 自适应学习
            "learned_rpm_limit",
            "concurrent_429_count",
            "rpm_429_count",
            "last_429_at",
            "last_429_type",
            "last_rpm_peak",
            "adjustment_history",
            "utilization_samples",
            "last_probe_increase_at",
            "health_by_format",
            "circuit_breaker_by_format",
            # 使用统计
            "request_count",
            "success_count",
            "error_count",
            "total_response_time_ms",
            "last_used_at",
            "last_error_at",
            "last_error_msg",
            # 运行时状态
            "expires_at",
            "last_models_fetch_at",
            "last_models_fetch_error",
            "upstream_metadata",
            "oauth_invalid_at",
            "oauth_invalid_reason",
            "created_at",
            "updated_at",
        }
    )

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)

    # 外键关系 - 直接关联 Provider
    provider_id = Column(
        String(36), ForeignKey("providers.id", ondelete="CASCADE"), nullable=False, index=True
    )

    # API 格式支持列表（核心字段）
    # None 表示支持所有格式（兼容历史数据），空列表 [] 表示不支持任何格式
    api_formats = Column(JSON, nullable=True, default=list)  # ["claude:chat", "claude:cli"]

    # 认证类型
    # - "api_key": 标准 API Key 认证（默认）
    # - "service_account": GCP Service Account JSON 认证
    # - "oauth": OAuth access_token / refresh_token 认证
    auth_type = Column(String(20), default="api_key", nullable=False)

    # API密钥（加密存储）
    # - auth_type="api_key" 时：存储 API Key 字符串
    # - auth_type="service_account"/"oauth" 时：可为占位符，敏感凭证存在 auth_config 中
    api_key = Column(Text, nullable=False)  # 使用 Text 支持加密后的 OAuth token

    # 认证配置（加密存储）
    # - auth_type="api_key" 时：可为空
    # - auth_type="service_account" 时：存储加密后的 Service Account JSON
    # - auth_type="oauth" 时：存储加密后的 {refresh_token, expires_at, ...}
    auth_config = Column(Text, nullable=True)
    name = Column(String(100), nullable=False)  # 密钥名称（必填，用于识别）
    note = Column(String(500), nullable=True)  # 备注说明（可选）

    # 成本计算
    rate_multipliers = Column(
        JSON, nullable=True
    )  # 按 endpoint signature 的成本倍率 {"claude:cli": 1.0, "openai:cli": 0.8}

    # 优先级配置 (数字越小越优先)
    internal_priority = Column(
        Integer, default=50
    )  # Endpoint 内部优先级（用于提供商优先模式，同 Endpoint 内 Keys 的排序，同优先级参与负载均衡）
    global_priority_by_format = Column(
        JSON, nullable=True
    )  # 按 endpoint signature 的全局优先级 {"claude:chat": 1, "claude:cli": 2}

    # RPM 限制配置（自适应学习）
    # rpm_limit 决定 RPM 控制模式：
    #   - NULL: 自适应模式，系统自动学习并调整（使用 learned_rpm_limit）
    #   - 数字: 固定限制模式，使用用户指定的值
    rpm_limit = Column(Integer, nullable=True, default=None)

    # 模型权限控制
    allowed_models = Column(JSON, nullable=True)  # 允许使用的模型列表（null = 支持所有模型）

    # Key 能力标签
    capabilities = Column(JSON, nullable=True)  # Key 拥有的能力
    # 示例: {"cache_1h": true, "context_1m": true}

    # 自适应 RPM 调整（仅当 rpm_limit = NULL 时生效）
    learned_rpm_limit = Column(Integer, nullable=True)  # 学习到的 RPM 限制（自适应模式下的有效值）
    concurrent_429_count = Column(Integer, default=0, nullable=False)  # 因并发导致的429次数
    rpm_429_count = Column(Integer, default=0, nullable=False)  # 因RPM导致的429次数
    last_429_at = Column(DateTime(timezone=True), nullable=True)  # 最后429时间
    last_429_type = Column(String(50), nullable=True)  # 最后429类型: concurrent/rpm/unknown
    last_rpm_peak = Column(Integer, nullable=True)  # 触发429时的RPM峰值
    adjustment_history = Column(JSON, nullable=True)  # RPM调整历史
    # 基于滑动窗口的利用率追踪
    utilization_samples = Column(
        JSON, nullable=True
    )  # 利用率采样窗口 [{"ts": timestamp, "util": 0.8}, ...]
    last_probe_increase_at = Column(DateTime(timezone=True), nullable=True)  # 上次探测性扩容时间

    # 健康度追踪（按 endpoint signature 存储）
    # 结构: {"claude:chat": {"health_score": 1.0, "consecutive_failures": 0, ...}, ...}
    health_by_format = Column(JSON, nullable=True, default=dict)

    # 缓存与熔断配置
    cache_ttl_minutes = Column(
        Integer, default=5, nullable=False
    )  # 缓存TTL(分钟)，0表示不支持缓存，默认5分钟
    max_probe_interval_minutes = Column(
        Integer, default=32, nullable=False
    )  # 最大探测间隔(分钟)，默认32分钟（硬上限）

    # 熔断器状态（按 endpoint signature 存储）
    # 结构: {"claude:chat": {"open": false, "open_at": null, ...}, ...}
    circuit_breaker_by_format = Column(JSON, nullable=True, default=dict)

    # 使用统计
    request_count = Column(Integer, default=0)  # 请求次数
    success_count = Column(Integer, default=0)  # 成功次数
    error_count = Column(Integer, default=0)  # 错误次数
    total_response_time_ms = Column(Integer, default=0)  # 总响应时间（用于计算平均值）
    last_used_at = Column(DateTime(timezone=True), nullable=True)  # 最后使用时间
    last_error_at = Column(DateTime(timezone=True), nullable=True)  # 最后错误时间
    last_error_msg = Column(Text, nullable=True)  # 最后错误信息

    # 状态
    is_active = Column(Boolean, default=True, nullable=False)
    expires_at = Column(DateTime(timezone=True), nullable=True)  # 过期时间

    # 自动获取模型配置
    auto_fetch_models = Column(Boolean, default=False, nullable=False)  # 是否启用自动获取模型
    last_models_fetch_at = Column(DateTime(timezone=True), nullable=True)  # 最后获取时间
    last_models_fetch_error = Column(Text, nullable=True)  # 最后获取错误信息
    locked_models = Column(JSON, nullable=True)  # 被锁定的模型列表（刷新时不会被删除）
    # 模型过滤规则（支持 * 和 ? 通配符，如 "gpt-*", "claude-?-sonnet"）
    model_include_patterns = Column(JSON, nullable=True)  # 包含规则列表，空表示不过滤（包含所有）
    model_exclude_patterns = Column(JSON, nullable=True)  # 排除规则列表，空表示不排除

    # 上游元数据（由响应头解析器采集，如 Codex 额度信息）
    upstream_metadata = Column(JSON, nullable=True, default=dict)

    # OAuth 失效状态（账号被封、授权撤销、刷新失败等）
    oauth_invalid_at = Column(DateTime(timezone=True), nullable=True)  # 失效时间
    oauth_invalid_reason = Column(String(255), nullable=True)  # 失效原因

    # Key 级别的代理配置（覆盖 Provider 级别的代理设置）
    # 结构: {"node_id": "xxx", "enabled": true} 或 {"url": "socks5://...", "enabled": true}
    # null 表示使用 Provider 级别代理（默认行为）
    proxy = Column(JSON, nullable=True, default=None)

    # 时间戳
    created_at = Column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False
    )
    updated_at = Column(
        DateTime(timezone=True),
        default=lambda: datetime.now(timezone.utc),
        onupdate=lambda: datetime.now(timezone.utc),
        nullable=False,
    )

    # 关系
    provider = relationship("Provider", back_populates="api_keys")
