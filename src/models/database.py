"""
数据库模型定义
"""

import hashlib
import secrets
import uuid
from datetime import datetime, timezone
from enum import Enum as PyEnum
from typing import Optional

import bcrypt
from sqlalchemy import (
    JSON,
    BigInteger,
    Boolean,
    CheckConstraint,
    Column,
    DateTime,
    Enum,
    Float,
    ForeignKey,
    Index,
    Integer,
    LargeBinary,
    String,
    Text,
    UniqueConstraint,
)
from sqlalchemy.dialects.postgresql import JSONB
from sqlalchemy.orm import declarative_base, relationship

from ..config import config
from ..core.enums import AuthSource, ProviderBillingType, UserRole

Base = declarative_base()


class User(Base):
    """用户模型"""

    __tablename__ = "users"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    # OAuth 用户可能没有邮箱；Postgres unique 允许多个 NULL
    email = Column(String(255), unique=True, index=True, nullable=True)
    # 注意：所有创建用户的入口必须显式写入 true/false，禁止依赖默认值
    email_verified = Column(Boolean, nullable=False)
    username = Column(String(100), unique=True, index=True, nullable=False)
    # OAuth 用户可能没有本地密码（v1 仅做字段兼容）
    password_hash = Column(String(255), nullable=True)
    role = Column(
        Enum(
            UserRole,
            name="userrole",
            create_type=False,
            values_callable=lambda x: [e.value for e in x],
        ),
        default=UserRole.USER,
        nullable=False,
    )
    auth_source = Column(
        Enum(
            AuthSource,
            name="authsource",
            create_type=False,
            values_callable=lambda x: [e.value for e in x],
        ),
        default=AuthSource.LOCAL,
        nullable=False,
    )

    # LDAP 标识（仅 auth_source=ldap 时使用，用于在邮箱变更/用户名冲突时稳定关联本地账户）
    ldap_dn = Column(String(512), nullable=True, index=True)
    ldap_username = Column(String(255), nullable=True, index=True)

    # 访问限制（NULL 表示不限制，允许访问所有资源）
    allowed_providers = Column(JSON, nullable=True)  # 允许使用的提供商 ID 列表
    allowed_api_formats = Column(JSON, nullable=True)  # 允许使用的 API 格式列表
    allowed_models = Column(JSON, nullable=True)  # 允许使用的模型名称列表

    # Key 能力配置
    model_capability_settings = Column(JSON, nullable=True)  # 用户针对特定模型的能力配置
    # 示例: {"claude-sonnet-4-20250514": {"cache_1h": true}}

    # 配额管理
    quota_usd = Column(Float, nullable=True)  # 美元配额(NULL 表示无限制)
    used_usd = Column(Float, default=0.0)  # 当前周期已使用美元
    total_usd = Column(Float, default=0.0)  # 累积消费总额

    # 状态
    is_active = Column(Boolean, default=True, nullable=False)
    is_deleted = Column(Boolean, default=False, nullable=False)

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
    last_login_at = Column(DateTime(timezone=True), nullable=True)

    # 关系 - CASCADE delete: 让数据库处理级联删除
    api_keys = relationship("ApiKey", back_populates="user", cascade="all, delete-orphan")
    management_tokens = relationship(
        "ManagementToken", back_populates="user", cascade="all, delete-orphan"
    )
    preferences = relationship(
        "UserPreference", back_populates="user", cascade="all, delete-orphan", passive_deletes=True
    )
    quotas = relationship(
        "UserQuota", back_populates="user", cascade="all, delete-orphan", passive_deletes=True
    )
    announcement_reads = relationship(
        "AnnouncementRead",
        back_populates="user",
        cascade="all, delete-orphan",
        passive_deletes=True,
    )

    # 关系 - SET NULL: 保留历史记录，让数据库处理 SET NULL
    usage_records = relationship("Usage", back_populates="user", passive_deletes=True)
    authored_announcements = relationship(
        "Announcement",
        back_populates="author",
        foreign_keys="Announcement.author_id",
        passive_deletes=True,
    )
    audit_logs = relationship("AuditLog", back_populates="user", passive_deletes=True)

    def set_password(self, password: str):
        """设置密码"""
        self.password_hash = bcrypt.hashpw(password.encode("utf-8"), bcrypt.gensalt()).decode(
            "utf-8"
        )

    def verify_password(self, password: str) -> bool:
        """验证密码"""
        if not self.password_hash:
            return False
        return bcrypt.checkpw(password.encode("utf-8"), self.password_hash.encode("utf-8"))


class ApiKey(Base):
    """API密钥模型"""

    __tablename__ = "api_keys"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    user_id = Column(String(36), ForeignKey("users.id", ondelete="CASCADE"), nullable=False)
    key_hash = Column(String(64), unique=True, index=True, nullable=False)  # API密钥的SHA256哈希
    key_encrypted = Column(Text, nullable=True)  # 加密后的完整密钥，用于查看
    name = Column(String(100), nullable=True)  # 密钥名称，便于用户管理

    # 使用统计
    total_requests = Column(Integer, default=0)
    total_cost_usd = Column(Float, default=0.0)

    # 余额管理（仅用于独立余额 Key）
    balance_used_usd = Column(Float, default=0.0)  # 已使用余额（USD），用于统计
    current_balance_usd = Column(Float, nullable=True)  # 当前余额（USD），NULL 表示无限制
    is_standalone = Column(
        Boolean, default=False, nullable=False
    )  # 是否为独立余额 Key（给非注册用户使用）

    # 访问限制（NULL 表示不限制，允许访问所有资源）
    allowed_providers = Column(JSON, nullable=True)  # 允许使用的提供商 ID 列表
    allowed_api_formats = Column(JSON, nullable=True)  # 允许使用的 API 格式列表
    allowed_models = Column(JSON, nullable=True)  # 允许使用的模型名称列表
    rate_limit = Column(Integer, default=None, nullable=True)  # 每分钟请求限制，None = 无限制
    concurrent_limit = Column(Integer, default=5, nullable=True)  # 并发请求限制

    # Key 能力配置
    force_capabilities = Column(JSON, nullable=True)  # 强制开启的能力
    # 示例: {"cache_1h": true} - 强制所有支持的模型都用 1h 缓存

    # 状态
    is_active = Column(Boolean, default=True, nullable=False)
    is_locked = Column(Boolean, default=False, nullable=False)  # 管理员锁定，用户无法使用/操作
    last_used_at = Column(DateTime(timezone=True), nullable=True)
    expires_at = Column(DateTime(timezone=True), nullable=True)  # 过期时间
    auto_delete_on_expiry = Column(Boolean, default=False, nullable=False)  # 过期后是否自动删除

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
    user = relationship("User", back_populates="api_keys")
    usage_records = relationship("Usage", back_populates="api_key")
    provider_mappings = relationship(
        "ApiKeyProviderMapping", back_populates="api_key", cascade="all, delete-orphan"
    )

    @staticmethod
    def generate_key() -> str:
        """生成API密钥（使用加密安全的随机数生成器）"""
        import string

        # 只使用字母和数字，避免特殊字符
        alphabet = string.ascii_letters + string.digits
        random_part = "".join(secrets.choice(alphabet) for _ in range(32))
        return f"{config.api_key_prefix}-{random_part}"

    @staticmethod
    def hash_key(api_key: str) -> str:
        """对API密钥进行哈希"""
        return hashlib.sha256(api_key.encode()).hexdigest()

    def set_key(self, api_key: str) -> None:
        """
        设置API密钥(用于测试和数据初始化)

        Args:
            api_key: 明文API密钥

        注意: 此方法会设置 key_hash 和 key_encrypted
        """
        from src.core.crypto import crypto_service

        # 设置哈希(用于验证)
        self.key_hash = self.hash_key(api_key)

        # 设置加密的完整密钥(用于显示和管理)
        self.key_encrypted = crypto_service.encrypt(api_key)

    def verify_key(self, api_key: str) -> bool:
        """
        验证API密钥是否匹配(用于测试)

        Args:
            api_key: 明文API密钥

        Returns:
            bool: 密钥是否匹配
        """
        return self.key_hash == self.hash_key(api_key)

    def get_display_key(self) -> str:
        """获取用于显示的脱敏密钥（前缀...后4位）"""
        from src.core.crypto import crypto_service

        if self.key_encrypted:
            try:
                # 使用静默模式，避免在显示场景打印错误日志
                full_key = crypto_service.decrypt(self.key_encrypted, silent=True)
                # 格式：sk-SpJ3y...sdf4
                prefix = full_key[:10] if len(full_key) >= 10 else full_key[: len(full_key) // 2]
                suffix = full_key[-4:] if len(full_key) >= 4 else ""
                return f"{prefix}...{suffix}"
            except Exception:
                pass
        # 降级：无法解密时返回占位符
        return "sk-****"


class Usage(Base):
    """使用记录模型"""

    __tablename__ = "usage"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    user_id = Column(String(36), ForeignKey("users.id", ondelete="SET NULL"), nullable=True)
    api_key_id = Column(String(36), ForeignKey("api_keys.id", ondelete="SET NULL"), nullable=True)

    # 请求信息
    request_id = Column(String(100), unique=True, index=True, nullable=False)
    provider_name = Column(String(100), nullable=False)  # Provider 名称（非外键）
    model = Column(String(100), nullable=False)
    target_model = Column(String(100), nullable=True, comment="映射后的目标模型名（若无映射则为空）")

    # Provider 侧追踪信息（记录最终成功的 Provider/Endpoint/Key）
    provider_id = Column(String(36), ForeignKey("providers.id", ondelete="SET NULL"), nullable=True)
    provider_endpoint_id = Column(
        String(36), ForeignKey("provider_endpoints.id", ondelete="SET NULL"), nullable=True
    )
    provider_api_key_id = Column(
        String(36), ForeignKey("provider_api_keys.id", ondelete="SET NULL"), nullable=True
    )

    # Token统计
    input_tokens = Column(Integer, default=0)
    output_tokens = Column(Integer, default=0)
    total_tokens = Column(Integer, default=0)

    # 缓存相关 tokens (for Claude models)
    cache_creation_input_tokens = Column(Integer, default=0)
    cache_read_input_tokens = Column(Integer, default=0)

    # 成本计算
    input_cost_usd = Column(Float, default=0.0)
    output_cost_usd = Column(Float, default=0.0)
    cache_cost_usd = Column(Float, default=0.0)  # 总缓存成本（兼容旧数据）
    cache_creation_cost_usd = Column(Float, default=0.0)  # 缓存创建成本
    cache_read_cost_usd = Column(Float, default=0.0)  # 缓存读取成本
    request_cost_usd = Column(Float, default=0.0)  # 按次计费成本
    total_cost_usd = Column(Float, default=0.0)

    # 真实成本计算（表面成本 × 倍率）
    actual_input_cost_usd = Column(Float, default=0.0)  # 真实输入成本
    actual_output_cost_usd = Column(Float, default=0.0)  # 真实输出成本
    actual_cache_creation_cost_usd = Column(Float, default=0.0)  # 真实缓存创建成本
    actual_cache_read_cost_usd = Column(Float, default=0.0)  # 真实缓存读取成本
    actual_request_cost_usd = Column(Float, default=0.0)  # 真实按次计费成本
    actual_total_cost_usd = Column(Float, default=0.0)  # 真实总成本
    rate_multiplier = Column(Float, default=1.0)  # 使用的倍率（来自 ProviderAPIKey）

    # 历史价格记录（每1M tokens的美元价格，记录请求时的实际价格）
    input_price_per_1m = Column(Float, nullable=True)  # 输入单价
    output_price_per_1m = Column(Float, nullable=True)  # 输出单价
    cache_creation_price_per_1m = Column(Float, nullable=True)  # 缓存创建单价
    cache_read_price_per_1m = Column(Float, nullable=True)  # 缓存读取单价
    price_per_request = Column(Float, nullable=True)  # 按次计费单价（历史记录）

    # 请求详情
    request_type = Column(String(50))  # chat, completion, embedding等
    api_format = Column(String(50), nullable=True)  # API 格式: CLAUDE, OPENAI 等
    is_stream = Column(Boolean, default=False)  # 是否为流式请求
    status_code = Column(Integer)
    error_message = Column(Text, nullable=True)
    response_time_ms = Column(Integer)  # 总响应时间（毫秒）
    first_byte_time_ms = Column(Integer, nullable=True)  # 首字时间/TTFB（毫秒）

    # 请求状态追踪
    # pending: 请求开始处理中
    # streaming: 流式响应进行中
    # completed: 请求成功完成
    # failed: 请求失败
    status = Column(String(20), default="completed", nullable=False, index=True)

    # 完整请求和响应记录
    request_headers = Column(JSON, nullable=True)  # 客户端请求头
    request_body = Column(JSON, nullable=True)  # 请求体（7天内未压缩）
    provider_request_headers = Column(JSON, nullable=True)  # 向提供商发送的请求头
    response_headers = Column(JSON, nullable=True)  # 提供商响应头
    client_response_headers = Column(JSON, nullable=True)  # 返回给客户端的响应头
    response_body = Column(JSON, nullable=True)  # 响应体（7天内未压缩）

    # 压缩存储字段（7天后自动压缩到这里）
    request_body_compressed = Column(LargeBinary, nullable=True)  # gzip压缩的请求体
    response_body_compressed = Column(LargeBinary, nullable=True)  # gzip压缩的响应体

    # 元数据
    request_metadata = Column(JSON, nullable=True)  # 存储额外信息

    # 时间戳
    created_at = Column(
        DateTime(timezone=True),
        default=lambda: datetime.now(timezone.utc),
        nullable=False,
        index=True,
    )

    # 关系
    user = relationship("User", back_populates="usage_records")
    api_key = relationship("ApiKey", back_populates="usage_records")
    provider_obj = relationship("Provider")  # 使用 provider_obj 避免与 provider 字段名冲突
    provider_endpoint = relationship("ProviderEndpoint")
    provider_api_key = relationship("ProviderAPIKey")

    def get_request_body(self):
        """获取请求体（自动解压）"""
        if self.request_body is not None:
            return self.request_body
        if self.request_body_compressed is not None:
            from src.utils.compression import decompress_json

            return decompress_json(self.request_body_compressed)
        return None

    def get_response_body(self):
        """获取响应体（自动解压）"""
        if self.response_body is not None:
            return self.response_body
        if self.response_body_compressed is not None:
            from src.utils.compression import decompress_json

            return decompress_json(self.response_body_compressed)
        return None


class UserQuota(Base):
    """用户配额历史记录"""

    __tablename__ = "user_quotas"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    user_id = Column(String(36), ForeignKey("users.id", ondelete="CASCADE"), nullable=False)

    # 配额类型
    quota_type = Column(String(50), nullable=False)  # monthly, daily, custom

    # 配额值
    quota_usd = Column(Float, nullable=False)

    # 时间范围
    period_start = Column(DateTime(timezone=True), nullable=False)
    period_end = Column(DateTime(timezone=True), nullable=False)

    # 使用情况
    used_usd = Column(Float, default=0.0)

    # 状态
    is_active = Column(Boolean, default=True)

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
    user = relationship("User", back_populates="quotas")


class SystemConfig(Base):
    """系统配置表"""

    __tablename__ = "system_configs"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    key = Column(String(100), unique=True, nullable=False)
    value = Column(JSON, nullable=False)
    description = Column(Text, nullable=True)

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


class LDAPConfig(Base):
    """LDAP认证配置表 - 单行配置"""

    __tablename__ = "ldap_configs"

    id = Column(Integer, primary_key=True, autoincrement=True)
    server_url = Column(String(255), nullable=False)  # ldap://host:389 或 ldaps://host:636
    bind_dn = Column(String(255), nullable=False)  # 绑定账号 DN
    bind_password_encrypted = Column(Text, nullable=True)  # 加密的绑定密码（允许 NULL 表示已清除）
    base_dn = Column(String(255), nullable=False)  # 用户搜索基础 DN
    user_search_filter = Column(
        String(500), default="(uid={username})", nullable=False
    )  # 用户搜索过滤器
    username_attr = Column(String(50), default="uid", nullable=False)  # 用户名属性 (uid/sAMAccountName)
    email_attr = Column(String(50), default="mail", nullable=False)  # 邮箱属性
    display_name_attr = Column(String(50), default="cn", nullable=False)  # 显示名称属性
    is_enabled = Column(Boolean, default=False, nullable=False)  # 是否启用 LDAP 认证
    is_exclusive = Column(
        Boolean, default=False, nullable=False
    )  # 是否仅允许 LDAP 登录（禁用本地认证）
    use_starttls = Column(Boolean, default=False, nullable=False)  # 是否使用 STARTTLS
    connect_timeout = Column(Integer, default=10, nullable=False)  # 连接超时时间（秒）

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

    def set_bind_password(self, password: str) -> None:
        """
        设置并加密绑定密码

        Args:
            password: 明文密码
        """
        from src.core.crypto import crypto_service

        self.bind_password_encrypted = crypto_service.encrypt(password)

    def get_bind_password(self) -> str:
        """
        获取解密后的绑定密码

        Returns:
            str: 解密后的明文密码

        Raises:
            DecryptionException: 解密失败时抛出异常
        """
        from src.core.crypto import crypto_service

        if not self.bind_password_encrypted:
            return ""
        return crypto_service.decrypt(self.bind_password_encrypted)


class OAuthProvider(Base):
    """OAuth Provider 配置表（按 provider_type 唯一）"""

    __tablename__ = "oauth_providers"

    # 使用 provider_type 作为主键，便于通过 URL 参数直接定位配置
    provider_type = Column(String(50), primary_key=True)
    display_name = Column(String(100), nullable=False)

    client_id = Column(String(255), nullable=False)
    client_secret_encrypted = Column(Text, nullable=True)  # 允许 NULL 表示尚未配置/已清除

    # 可选覆盖端点（需在业务层做白名单校验）
    authorization_url_override = Column(String(500), nullable=True)
    token_url_override = Column(String(500), nullable=True)
    userinfo_url_override = Column(String(500), nullable=True)

    # 可选覆盖 scopes（JSON 列表）
    scopes = Column(JSON, nullable=True)

    # 服务端控制 redirect_uri 与前端回调 URL
    redirect_uri = Column(String(500), nullable=False)
    frontend_callback_url = Column(String(500), nullable=False)

    # Provider 特定配置/映射
    attribute_mapping = Column(JSON, nullable=True)
    extra_config = Column(JSON, nullable=True)

    is_enabled = Column(Boolean, default=False, nullable=False)

    created_at = Column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False
    )
    updated_at = Column(
        DateTime(timezone=True),
        default=lambda: datetime.now(timezone.utc),
        onupdate=lambda: datetime.now(timezone.utc),
        nullable=False,
    )

    def set_client_secret(self, secret: str) -> None:
        """设置并加密 client_secret"""
        from src.core.crypto import crypto_service

        self.client_secret_encrypted = crypto_service.encrypt(secret)

    def get_client_secret(self) -> str:
        """获取解密后的 client_secret（未配置时返回空串）"""
        from src.core.crypto import crypto_service

        if not self.client_secret_encrypted:
            return ""
        return crypto_service.decrypt(self.client_secret_encrypted)


class UserOAuthLink(Base):
    """用户与 OAuth Provider 的绑定关系"""

    __tablename__ = "user_oauth_links"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()))
    user_id = Column(
        String(36),
        ForeignKey("users.id", ondelete="CASCADE"),
        nullable=False,
        index=True,
    )
    provider_type = Column(
        String(50),
        ForeignKey("oauth_providers.provider_type", ondelete="CASCADE"),
        nullable=False,
        index=True,
    )
    provider_user_id = Column(String(255), nullable=False)
    provider_username = Column(String(255), nullable=True)
    provider_email = Column(String(255), nullable=True)
    extra_data = Column(JSON, nullable=True)

    linked_at = Column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False
    )
    last_login_at = Column(DateTime(timezone=True), nullable=True)

    __table_args__ = (
        UniqueConstraint("provider_type", "provider_user_id", name="uq_oauth_provider_user"),
        UniqueConstraint("user_id", "provider_type", name="uq_user_oauth_provider"),
    )


class Provider(Base):
    """提供商配置表"""

    __tablename__ = "providers"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    name = Column(String(100), unique=True, nullable=False, index=True)  # 提供商名称（唯一）
    description = Column(Text, nullable=True)  # 提供商描述
    website = Column(String(500), nullable=True)  # 主站网站

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

    # 状态
    is_active = Column(Boolean, default=True, nullable=False)

    # 限制
    concurrent_limit = Column(Integer, nullable=True)  # 并发请求限制

    # 请求配置
    max_retries = Column(Integer, default=2, nullable=True)  # 最大重试次数
    proxy = Column(JSONB, nullable=True)  # 代理配置: {url, username, password, enabled}

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


class ProviderEndpoint(Base):
    """提供商端点 - 一个提供商可以有多个 API 格式端点"""

    __tablename__ = "provider_endpoints"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    provider_id = Column(String(36), ForeignKey("providers.id", ondelete="CASCADE"), nullable=False)

    # API 格式和配置
    api_format = Column(String(50), nullable=False)  # 存储 APIFormat 枚举值的字符串
    base_url = Column(String(500), nullable=False)

    # 请求配置
    header_rules = Column(JSON, nullable=True)  # 请求头规则 [{action, key, value, from, to}]
    max_retries = Column(Integer, default=2)  # 最大重试次数

    # 状态
    is_active = Column(Boolean, default=True, nullable=False)

    # 路径配置
    custom_path = Column(
        String(200), nullable=True
    )  # 自定义请求路径，为空则使用 API 格式的默认路径

    # 额外配置
    config = Column(JSON, nullable=True)  # 端点特定配置（不推荐使用，优先使用专用字段）

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
    )


class GlobalModel(Base):
    """全局统一模型定义 - 包含价格和能力配置

    设计原则:
    - 定义模型的基本信息和价格配置（价格为必填项）
    - Provider 级别的 Model 可以覆盖这些默认值
    - 如果 Model 的价格/能力字段为空，则使用 GlobalModel 的值
    """

    __tablename__ = "global_models"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    name = Column(String(100), unique=True, nullable=False, index=True)  # 统一模型名（唯一）
    display_name = Column(String(100), nullable=False)

    # 按次计费配置（每次请求的固定费用，美元）- 可选，与按 token 计费叠加
    default_price_per_request = Column(Float, nullable=True, default=None)  # 每次请求固定费用

    # 统一阶梯计费配置（JSON格式）- 必填
    # 固定价格也用单阶梯表示: {"tiers": [{"up_to": null, "input_price_per_1m": X, ...}]}
    # 结构示例:
    # {
    #     "tiers": [
    #         {
    #             "up_to": 128000,  # 阶梯上限（tokens），null 表示无上限
    #             "input_price_per_1m": 2.50,
    #             "output_price_per_1m": 10.00,
    #             "cache_creation_price_per_1m": 3.75,  # 可选
    #             "cache_read_price_per_1m": 0.30,      # 可选
    #             "cache_ttl_pricing": [                 # 可选：按缓存时长分价格
    #                 {"ttl_minutes": 5, "cache_read_price_per_1m": 0.30},
    #                 {"ttl_minutes": 60, "cache_read_price_per_1m": 0.50}
    #             ]
    #         },
    #         {"up_to": null, "input_price_per_1m": 1.25, ...}
    #     ]
    # }
    default_tiered_pricing = Column(JSON, nullable=False)

    # Key 能力配置 - 模型支持的能力列表（如 ["cache_1h", "context_1m"]）
    # Key 只能启用模型支持的能力
    supported_capabilities = Column(JSON, nullable=True, default=list)

    # 模型配置（JSON格式）- 包含能力、规格、元信息等
    # 结构示例:
    # {
    #     # 能力配置
    #     "streaming": true,
    #     "vision": true,
    #     "function_calling": true,
    #     "extended_thinking": false,
    #     "image_generation": false,
    #     # 规格参数
    #     "context_limit": 200000,
    #     "output_limit": 8192,
    #     # 元信息
    #     "description": "...",
    #     "icon_url": "...",
    #     "official_url": "...",
    #     "knowledge_cutoff": "2024-04",
    #     "family": "claude-3.5",
    #     "release_date": "2024-10-22",
    #     "input_modalities": ["text", "image"],
    #     "output_modalities": ["text"],
    # }
    config = Column(JSONB, nullable=True, default=dict)

    # 状态
    is_active = Column(Boolean, default=True, nullable=False)

    # 统计计数器（优化性能，避免实时查询）
    usage_count = Column(Integer, default=0, nullable=False, index=True)

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
    models = relationship("Model", back_populates="global_model")


class Model(Base):
    """Provider 模型配置表 - Provider 如何使用某个 GlobalModel

    设计原则:
    - Model 表示 Provider 对某个模型的具体实现
    - global_model_id 可为空：
      - 为空时：模型尚未关联到 GlobalModel，不参与路由
      - 不为空时：模型已关联 GlobalModel，参与路由
    - provider_model_name 是 Provider 侧的实际模型名称 (可能与 GlobalModel.name 不同)
    - 价格和能力配置可为空，为空时使用 GlobalModel 的默认值
    """

    __tablename__ = "models"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    provider_id = Column(String(36), ForeignKey("providers.id"), nullable=False)
    # 可为空：NULL 表示未关联，不参与路由；非 NULL 表示已关联，参与路由
    global_model_id = Column(String(36), ForeignKey("global_models.id"), nullable=True, index=True)

    # Provider 映射配置
    provider_model_name = Column(String(200), nullable=False)  # Provider 侧的主模型名称
    # 模型名称映射列表（带优先级），用于同一模型在 Provider 侧有多个名称变体的场景
    # 格式: [{"name": "Claude-Sonnet-4.5", "priority": 1}, {"name": "Claude-Sonnet-4-5", "priority": 2}]
    # 为空时只使用 provider_model_name
    provider_model_mappings = Column(JSON, nullable=True, default=None)

    # 按次计费配置（每次请求的固定费用，美元）- 可为空，为空时使用 GlobalModel 的默认值
    price_per_request = Column(Float, nullable=True)  # 每次请求固定费用

    # 阶梯计费配置（JSON格式）- 可为空，为空时使用 GlobalModel 的默认值
    tiered_pricing = Column(JSON, nullable=True, default=None)

    # Provider 能力配置 - 可为空，为空时使用 GlobalModel 的默认值
    supports_vision = Column(Boolean, nullable=True)
    supports_function_calling = Column(Boolean, nullable=True)
    supports_streaming = Column(Boolean, nullable=True)
    supports_extended_thinking = Column(Boolean, nullable=True)
    supports_image_generation = Column(Boolean, nullable=True)

    # 状态
    is_active = Column(Boolean, default=True, nullable=False)
    is_available = Column(Boolean, default=True)  # 是否当前可用

    # 扩展配置
    config = Column(JSON, nullable=True)

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
    provider = relationship("Provider", back_populates="models")
    global_model = relationship("GlobalModel", back_populates="models")

    # 唯一约束：同一个提供商下的 provider_model_name 不能重复
    __table_args__ = (
        UniqueConstraint("provider_id", "provider_model_name", name="uq_provider_model"),
    )

    # 辅助方法：获取有效的阶梯计费配置
    def get_effective_tiered_pricing(self) -> dict | None:
        """获取有效的阶梯计费配置"""
        if self.tiered_pricing is not None:
            return self.tiered_pricing
        if self.global_model:
            return self.global_model.default_tiered_pricing
        return None

    def _get_first_tier(self) -> dict | None:
        """获取第一个阶梯（用于获取默认价格）"""
        tiered = self.get_effective_tiered_pricing()
        if tiered and tiered.get("tiers"):
            return tiered["tiers"][0]
        return None

    def get_effective_input_price(self) -> float:
        """获取有效的输入价格（从第一个阶梯）"""
        tier = self._get_first_tier()
        if tier:
            return tier.get("input_price_per_1m", 0.0)
        return 0.0

    def get_effective_output_price(self) -> float:
        """获取有效的输出价格（从第一个阶梯）"""
        tier = self._get_first_tier()
        if tier:
            return tier.get("output_price_per_1m", 0.0)
        return 0.0

    def get_effective_cache_creation_price(self) -> float | None:
        """获取有效的缓存创建价格（从第一个阶梯）"""
        tier = self._get_first_tier()
        if tier:
            return tier.get("cache_creation_price_per_1m")
        return None

    def get_effective_cache_read_price(self) -> float | None:
        """获取有效的缓存读取价格（从第一个阶梯）"""
        tier = self._get_first_tier()
        if tier:
            return tier.get("cache_read_price_per_1m")
        return None

    def get_effective_1h_cache_creation_price(self) -> float | None:
        """获取有效的 1h 缓存创建价格（从第一个阶梯）"""
        tier = self._get_first_tier()
        if tier:
            cache_ttl_pricing = tier.get("cache_ttl_pricing") or []
            for ttl_entry in cache_ttl_pricing:
                if ttl_entry.get("ttl_minutes") == 60:
                    return ttl_entry.get("cache_creation_price_per_1m")
        return None

    def get_effective_price_per_request(self) -> float | None:
        """获取有效的按次计费价格"""
        if self.price_per_request is not None:
            return self.price_per_request
        if self.global_model:
            return self.global_model.default_price_per_request
        return None

    def _get_effective_capability(self, attr_name: str, default: bool = False) -> bool:
        """获取有效的能力配置（通用辅助方法）"""
        local_value = getattr(self, attr_name, None)
        if local_value is not None:
            return bool(local_value)
        if self.global_model:
            config_key_map = {
                "supports_vision": "vision",
                "supports_function_calling": "function_calling",
                "supports_streaming": "streaming",
                "supports_extended_thinking": "extended_thinking",
                "supports_image_generation": "image_generation",
            }
            config_key = config_key_map.get(attr_name)
            if config_key:
                global_config = getattr(self.global_model, "config", None)
                if isinstance(global_config, dict):
                    global_value = global_config.get(config_key)
                    if global_value is not None:
                        return bool(global_value)
        return default

    def get_effective_supports_vision(self) -> bool:
        return self._get_effective_capability("supports_vision", False)

    def get_effective_supports_function_calling(self) -> bool:
        return self._get_effective_capability("supports_function_calling", False)

    def get_effective_supports_streaming(self) -> bool:
        return self._get_effective_capability("supports_streaming", True)

    def get_effective_supports_extended_thinking(self) -> bool:
        return self._get_effective_capability("supports_extended_thinking", False)

    def get_effective_supports_image_generation(self) -> bool:
        return self._get_effective_capability("supports_image_generation", False)

    def select_provider_model_name(
        self, affinity_key: Optional[str] = None, api_format: Optional[str] = None
    ) -> str:
        """按优先级选择要使用的 Provider 模型名称

        如果配置了 provider_model_mappings，按优先级选择（数字越小越优先）；
        相同优先级的映射通过哈希分散实现负载均衡（与 Key 调度策略一致）；
        否则返回 provider_model_name。

        Args:
            affinity_key: 用于哈希分散的亲和键（如用户 API Key 哈希），确保同一用户稳定选择同一映射
            api_format: 当前请求的 API 格式（如 CLAUDE、OPENAI 等），用于过滤适用的映射
        """
        import hashlib

        if not self.provider_model_mappings:
            return self.provider_model_name

        raw_mappings = self.provider_model_mappings
        if not isinstance(raw_mappings, list) or len(raw_mappings) == 0:
            return self.provider_model_name

        mappings: list[dict] = []
        for raw in raw_mappings:
            if not isinstance(raw, dict):
                continue
            name = raw.get("name")
            if not isinstance(name, str) or not name.strip():
                continue

            # 检查 api_formats 作用域（如果配置了且当前有 api_format）
            mapping_api_formats = raw.get("api_formats")
            if api_format and mapping_api_formats:
                # 如果配置了作用域，只有匹配时才生效
                if isinstance(mapping_api_formats, list) and api_format not in mapping_api_formats:
                    continue

            raw_priority = raw.get("priority", 1)
            try:
                priority = int(raw_priority)
            except Exception:
                priority = 1
            if priority < 1:
                priority = 1

            mappings.append({"name": name.strip(), "priority": priority})

        if not mappings:
            return self.provider_model_name

        # 按优先级排序（数字越小越优先）
        sorted_mappings = sorted(mappings, key=lambda x: x["priority"])

        # 获取最高优先级（最小数字）
        highest_priority = sorted_mappings[0]["priority"]

        # 获取所有最高优先级的映射
        top_priority_mappings = [
            mapping for mapping in sorted_mappings
            if mapping["priority"] == highest_priority
        ]

        # 如果有多个相同优先级的映射，通过哈希分散选择
        if len(top_priority_mappings) > 1 and affinity_key:
            # 为每个映射计算哈希得分，选择得分最小的
            def hash_score(mapping: dict) -> int:
                combined = f"{affinity_key}:{mapping['name']}"
                return int(hashlib.md5(combined.encode()).hexdigest(), 16)

            selected = min(top_priority_mappings, key=hash_score)
        elif len(top_priority_mappings) > 1:
            # 没有 affinity_key 时，使用确定性选择（按名称排序后取第一个）
            # 避免随机选择导致同一请求重试时选择不同的模型名称
            selected = min(top_priority_mappings, key=lambda x: x["name"])
        else:
            selected = top_priority_mappings[0]

        return selected["name"]

    def get_all_provider_model_names(self) -> list[str]:
        """获取所有可用的 Provider 模型名称（主名称 + 映射名称）"""
        names = [self.provider_model_name]
        if self.provider_model_mappings:
            for mapping in self.provider_model_mappings:
                if isinstance(mapping, dict) and mapping.get("name"):
                    names.append(mapping["name"])
        return names


class ProviderAPIKey(Base):
    """Provider API密钥表 - 直接归属于 Provider，支持多种 API 格式"""

    __tablename__ = "provider_api_keys"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)

    # 外键关系 - 直接关联 Provider
    provider_id = Column(
        String(36), ForeignKey("providers.id", ondelete="CASCADE"), nullable=False, index=True
    )

    # API 格式支持列表（核心字段）
    api_formats = Column(JSON, nullable=False, default=list)  # ["CLAUDE", "CLAUDE_CLI"]

    # API密钥信息
    api_key = Column(String(500), nullable=False)  # API密钥（加密存储）
    name = Column(String(100), nullable=False)  # 密钥名称（必填，用于识别）
    note = Column(String(500), nullable=True)  # 备注说明（可选）

    # 成本计算
    rate_multipliers = Column(
        JSON, nullable=True
    )  # 按 API 格式的成本倍率 {"CLAUDE_CLI": 1.0, "OPENAI_CLI": 0.8}

    # 优先级配置 (数字越小越优先)
    internal_priority = Column(
        Integer, default=50
    )  # Endpoint 内部优先级（用于提供商优先模式，同 Endpoint 内 Keys 的排序，同优先级参与负载均衡）
    global_priority_by_format = Column(
        JSON, nullable=True
    )  # 按 API 格式的全局优先级 {"CLAUDE": 1, "CLAUDE_CLI": 2}

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
    learned_rpm_limit = Column(
        Integer, nullable=True
    )  # 学习到的 RPM 限制（自适应模式下的有效值）
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
    last_probe_increase_at = Column(
        DateTime(timezone=True), nullable=True
    )  # 上次探测性扩容时间

    # 健康度追踪（按 API 格式存储）
    # 结构: {"CLAUDE": {"health_score": 1.0, "consecutive_failures": 0, "last_failure_at": null, "request_results_window": []}, ...}
    health_by_format = Column(JSON, nullable=True, default=dict)

    # 缓存与熔断配置
    cache_ttl_minutes = Column(
        Integer, default=5, nullable=False
    )  # 缓存TTL(分钟)，0表示不支持缓存，默认5分钟
    max_probe_interval_minutes = Column(
        Integer, default=32, nullable=False
    )  # 最大探测间隔(分钟)，默认32分钟（硬上限）

    # 熔断器状态（按 API 格式存储）
    # 结构: {"CLAUDE": {"open": false, "open_at": null, "next_probe_at": null, "half_open_until": null, "half_open_successes": 0, "half_open_failures": 0}, ...}
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


class UserPreference(Base):
    """用户偏好设置表"""

    __tablename__ = "user_preferences"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    user_id = Column(
        String(36), ForeignKey("users.id", ondelete="CASCADE"), unique=True, nullable=False
    )

    # 个人信息
    avatar_url = Column(String(500), nullable=True)  # 头像URL
    bio = Column(Text, nullable=True)  # 个人简介

    # 偏好设置
    default_provider_id = Column(String(36), ForeignKey("providers.id"), nullable=True)
    theme = Column(String(20), default="light")  # light/dark/auto
    language = Column(String(10), default="zh-CN")
    timezone = Column(String(50), default="Asia/Shanghai")

    # 通知设置
    email_notifications = Column(Boolean, default=True)
    usage_alerts = Column(Boolean, default=True)
    announcement_notifications = Column(Boolean, default=True)

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
    user = relationship("User", back_populates="preferences")
    default_provider = relationship("Provider")


class Announcement(Base):
    """公告表"""

    __tablename__ = "announcements"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    title = Column(String(200), nullable=False)
    content = Column(Text, nullable=False)  # 支持 Markdown
    type = Column(String(20), default="info")  # info, warning, maintenance, important
    priority = Column(Integer, default=0)  # 优先级,数字越大越重要

    # 发布信息
    author_id = Column(String(36), ForeignKey("users.id", ondelete="SET NULL"), nullable=True)
    is_active = Column(Boolean, default=True, index=True)
    is_pinned = Column(Boolean, default=False)  # 置顶

    # 时间范围
    start_time = Column(DateTime(timezone=True), nullable=True)
    end_time = Column(DateTime(timezone=True), nullable=True)

    # 时间戳
    created_at = Column(
        DateTime(timezone=True),
        default=lambda: datetime.now(timezone.utc),
        nullable=False,
        index=True,
    )
    updated_at = Column(
        DateTime(timezone=True),
        default=lambda: datetime.now(timezone.utc),
        onupdate=lambda: datetime.now(timezone.utc),
        nullable=False,
    )

    # 关系
    author = relationship("User", back_populates="authored_announcements")
    reads = relationship(
        "AnnouncementRead", back_populates="announcement", cascade="all, delete-orphan"
    )


class AnnouncementRead(Base):
    """公告已读记录表"""

    __tablename__ = "announcement_reads"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    user_id = Column(String(36), ForeignKey("users.id", ondelete="CASCADE"), nullable=False)
    announcement_id = Column(String(36), ForeignKey("announcements.id"), nullable=False)
    read_at = Column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False
    )

    # 唯一约束
    __table_args__ = (UniqueConstraint("user_id", "announcement_id", name="uq_user_announcement"),)

    # 关系
    user = relationship("User", back_populates="announcement_reads")
    announcement = relationship("Announcement", back_populates="reads")


class AuditEventType(PyEnum):
    """审计事件类型"""

    # 认证相关
    LOGIN_SUCCESS = "login_success"
    LOGIN_FAILED = "login_failed"
    LOGOUT = "logout"
    API_KEY_CREATED = "api_key_created"
    API_KEY_DELETED = "api_key_deleted"
    API_KEY_USED = "api_key_used"

    # 请求相关
    REQUEST_SUCCESS = "request_success"
    REQUEST_FAILED = "request_failed"
    REQUEST_RATE_LIMITED = "request_rate_limited"
    REQUEST_QUOTA_EXCEEDED = "request_quota_exceeded"

    # 管理操作
    USER_CREATED = "user_created"
    USER_UPDATED = "user_updated"
    USER_DELETED = "user_deleted"
    PROVIDER_ADDED = "provider_added"
    PROVIDER_UPDATED = "provider_updated"
    PROVIDER_REMOVED = "provider_removed"

    # 安全事件
    SUSPICIOUS_ACTIVITY = "suspicious_activity"
    UNAUTHORIZED_ACCESS = "unauthorized_access"
    DATA_EXPORT = "data_export"
    CONFIG_CHANGED = "config_changed"

    # Management Token 相关
    MANAGEMENT_TOKEN_CREATED = "management_token_created"
    MANAGEMENT_TOKEN_UPDATED = "management_token_updated"
    MANAGEMENT_TOKEN_DELETED = "management_token_deleted"
    MANAGEMENT_TOKEN_USED = "management_token_used"
    MANAGEMENT_TOKEN_EXPIRED = "management_token_expired"
    MANAGEMENT_TOKEN_IP_BLOCKED = "management_token_ip_blocked"


class ManagementToken(Base):
    """Management Token 模型 - 用于程序化管理 API 调用"""

    __tablename__ = "management_tokens"

    # Token 格式常量
    TOKEN_PREFIX = "ae_"
    TOKEN_RANDOM_LENGTH = 40

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    user_id = Column(String(36), ForeignKey("users.id", ondelete="CASCADE"), nullable=False)

    # Token 信息
    token_hash = Column(String(64), unique=True, index=True, nullable=False)  # SHA256 哈希
    token_prefix = Column(String(12), nullable=True)  # Token 前缀用于显示（如 ae_xxxxxxxx）
    name = Column(String(100), nullable=False)  # Token 名称
    description = Column(Text, nullable=True)  # 描述

    # IP 白名单（可选）
    allowed_ips = Column(JSON, nullable=True)  # 允许的 IP 列表，NULL = 不限制
    # 格式: ["192.168.1.1", "10.0.0.0/24"]

    # 有效期
    expires_at = Column(DateTime(timezone=True), nullable=True)  # NULL = 永不过期

    # 使用统计
    last_used_at = Column(DateTime(timezone=True), nullable=True)
    last_used_ip = Column(String(45), nullable=True)
    usage_count = Column(Integer, default=0)  # 使用次数

    # 状态
    is_active = Column(Boolean, default=True, nullable=False)

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
    user = relationship("User", back_populates="management_tokens")

    # 索引和约束
    __table_args__ = (
        Index("idx_management_tokens_user_id", "user_id"),
        Index("idx_management_tokens_is_active", "is_active"),
        UniqueConstraint("user_id", "name", name="uq_management_tokens_user_name"),
        # IP 白名单必须为 NULL（不限制）或非空数组，禁止空数组
        # 注意：JSON 类型的 NULL 可能被序列化为 JSON 'null'，需要同时处理
        CheckConstraint(
            "allowed_ips IS NULL OR allowed_ips::text = 'null' OR json_array_length(allowed_ips) > 0",
            name="check_allowed_ips_not_empty",
        ),
    )

    @staticmethod
    def generate_token() -> str:
        """生成 Management Token（使用加密安全的随机数）"""
        import string

        alphabet = string.ascii_letters + string.digits
        random_part = "".join(
            secrets.choice(alphabet) for _ in range(ManagementToken.TOKEN_RANDOM_LENGTH)
        )
        return f"{ManagementToken.TOKEN_PREFIX}{random_part}"

    @staticmethod
    def hash_token(token: str) -> str:
        """对 Token 进行 SHA256 哈希

        安全性说明（当前方案是安全的）：
        - Token 熵为 62^40（约 2^238），暴力破解在计算上不可行
        - 结合速率限制（默认 30 次/分钟/IP），在线攻击不可行
        - 不需要盐值：盐值用于防止彩虹表攻击，但 Token 是高熵随机值，
          不存在可预计算的"常见值"，因此彩虹表攻击不适用
        """
        return hashlib.sha256(token.encode()).hexdigest()

    def set_token(self, token: str) -> None:
        """设置 Token（只存储哈希和前缀用于显示）"""
        self.token_hash = self.hash_token(token)
        # 存储前缀用于显示（ae_ + 4 个字符，共 7 个字符）
        self.token_prefix = token[:7] if len(token) > 7 else token

    def get_display_token(self) -> str:
        """获取用于显示的脱敏 Token（显示前缀 + 掩码）"""
        if self.token_prefix:
            return f"{self.token_prefix}...****"
        return "ae_****"

    def is_ip_allowed(self, client_ip: str) -> bool:
        """检查 IP 是否在白名单中

        安全策略：
        - None 或不设置表示不限制（允许所有 IP）
        - 非空列表表示只允许列表中的 IP
        - 无效的白名单条目会被记录并跳过
        - 无效的客户端 IP 直接拒绝
        - 支持 IPv4 映射的 IPv6 地址规范化
        """
        if self.allowed_ips is None:
            return True  # 未设置白名单，不限制

        import ipaddress

        from src.core.logger import logger

        # 防御性检查：空列表应该在数据库层被拒绝，但这里再检查一次
        if not self.allowed_ips:
            logger.critical(f"Management Token {self.id} - allowed_ips 为空列表（违反数据库约束）")
            return False  # fail-safe

        def normalize_ip(ip_str: str) -> ipaddress.IPv4Address | ipaddress.IPv6Address | None:
            """规范化 IP 地址，将 IPv4 映射的 IPv6 转换为 IPv4"""
            try:
                ip = ipaddress.ip_address(ip_str)
                if isinstance(ip, ipaddress.IPv6Address) and ip.ipv4_mapped:
                    return ip.ipv4_mapped
                return ip
            except ValueError:
                return None

        # 规范化客户端 IP
        client = normalize_ip(client_ip)
        if client is None:
            logger.error(f"Management Token {self.id} - 拒绝无效的客户端 IP: {client_ip}")
            return False

        valid_entries = 0
        for allowed in self.allowed_ips:
            try:
                if "/" in allowed:
                    # CIDR 格式
                    network = ipaddress.ip_network(allowed, strict=False)
                    valid_entries += 1
                    if client in network:
                        return True
                else:
                    # 精确 IP
                    allowed_ip = normalize_ip(allowed)
                    if allowed_ip is None:
                        logger.error(f"Management Token {self.id} - 白名单包含无效条目: {allowed}")
                        continue
                    valid_entries += 1
                    if client == allowed_ip:
                        return True
            except ValueError:
                logger.error(f"Management Token {self.id} - 白名单包含无效条目: {allowed}")
                continue

        # 如果白名单全部无效，记录严重错误并拒绝
        if valid_entries == 0:
            logger.critical(f"Management Token {self.id} - 白名单全部无效，拒绝所有访问")

        return False

    @property
    def is_expired(self) -> bool:
        """检查 Token 是否已过期（时区安全）"""
        if not self.expires_at:
            return False

        expires = self.expires_at
        if expires.tzinfo is None:
            # 数据库中的时间应该有时区信息，如果没有则表示数据完整性问题
            from src.core.logger import logger

            logger.error(f"Management Token {self.id} expires_at 缺少时区信息（数据完整性问题）")
            expires = expires.replace(tzinfo=timezone.utc)

        return expires < datetime.now(timezone.utc)


class AuditLog(Base):
    """审计日志模型"""

    __tablename__ = "audit_logs"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()), index=True)
    event_type = Column(String(50), nullable=False, index=True)
    user_id = Column(
        String(36), ForeignKey("users.id", ondelete="SET NULL"), nullable=True, index=True
    )
    api_key_id = Column(String(36), nullable=True)

    # 事件详情
    description = Column(Text, nullable=False)
    ip_address = Column(String(45), nullable=True)
    user_agent = Column(String(500), nullable=True)
    request_id = Column(String(100), nullable=True, index=True)

    # 相关数据
    event_metadata = Column(JSON, nullable=True)

    # 响应信息
    status_code = Column(Integer, nullable=True)
    error_message = Column(Text, nullable=True)

    # 时间戳
    created_at = Column(
        DateTime(timezone=True),
        default=lambda: datetime.now(timezone.utc),
        nullable=False,
        index=True,
    )

    # 关系
    user = relationship("User", back_populates="audit_logs")


class RequestCandidate(Base):
    """请求候选记录 - 追踪所有候选（包括未使用的）"""

    __tablename__ = "request_candidates"

    # 主键
    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()))

    # 关联字段
    request_id = Column(String(100), nullable=False, index=True)
    user_id = Column(String(36), ForeignKey("users.id", ondelete="CASCADE"), nullable=True)
    api_key_id = Column(String(36), ForeignKey("api_keys.id", ondelete="CASCADE"), nullable=True)

    # 候选信息
    candidate_index = Column(Integer, nullable=False)  # 候选序号（从0开始）
    retry_index = Column(Integer, nullable=False, default=0)  # 重试序号（从0开始）
    provider_id = Column(String(36), ForeignKey("providers.id", ondelete="CASCADE"), nullable=True)
    endpoint_id = Column(
        String(36), ForeignKey("provider_endpoints.id", ondelete="CASCADE"), nullable=True
    )
    key_id = Column(
        String(36), ForeignKey("provider_api_keys.id", ondelete="CASCADE"), nullable=True
    )

    # 状态信息
    status = Column(String(20), nullable=False)  # 'pending', 'success', 'failed', 'skipped'
    skip_reason = Column(Text, nullable=True)  # 跳过/失败原因
    is_cached = Column(Boolean, default=False)  # 是否为缓存亲和性候选

    # 执行结果信息（当 status = success/failed 时）
    status_code = Column(Integer, nullable=True)  # HTTP 状态码
    error_type = Column(String(50), nullable=True)  # 错误类型
    error_message = Column(Text, nullable=True)  # 错误消息
    latency_ms = Column(Integer, nullable=True)  # 延迟（毫秒）
    concurrent_requests = Column(Integer, nullable=True)  # 并发请求数

    # 元数据
    extra_data = Column(JSON, nullable=True)
    required_capabilities = Column(JSON, nullable=True)  # 请求实际需要的能力标签

    # 时间戳
    created_at = Column(
        DateTime(timezone=True), default=lambda: datetime.now(timezone.utc), nullable=False
    )
    started_at = Column(DateTime(timezone=True), nullable=True)  # 开始执行时间
    finished_at = Column(DateTime(timezone=True), nullable=True)  # 完成时间

    # 唯一约束和索引
    __table_args__ = (
        UniqueConstraint(
            "request_id", "candidate_index", "retry_index", name="uq_request_candidate_with_retry"
        ),
        Index("idx_request_candidates_request_id", "request_id"),
        Index("idx_request_candidates_status", "status"),
        Index("idx_request_candidates_provider_id", "provider_id"),
    )

    # 关系
    user = relationship("User")
    api_key = relationship("ApiKey")
    provider = relationship("Provider")
    endpoint = relationship("ProviderEndpoint")
    key = relationship("ProviderAPIKey")


# ==================== 统计数据模型 ====================


class StatsDaily(Base):
    """每日统计快照 - 用于快速查询历史数据"""

    __tablename__ = "stats_daily"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()))

    # 统计日期 (UTC)
    date = Column(DateTime(timezone=True), nullable=False, unique=True, index=True)

    # 请求统计
    total_requests = Column(Integer, default=0, nullable=False)
    success_requests = Column(Integer, default=0, nullable=False)
    error_requests = Column(Integer, default=0, nullable=False)

    # Token 统计
    input_tokens = Column(BigInteger, default=0, nullable=False)
    output_tokens = Column(BigInteger, default=0, nullable=False)
    cache_creation_tokens = Column(BigInteger, default=0, nullable=False)
    cache_read_tokens = Column(BigInteger, default=0, nullable=False)

    # 成本统计 (USD)
    total_cost = Column(Float, default=0.0, nullable=False)
    actual_total_cost = Column(Float, default=0.0, nullable=False)  # 倍率后成本
    input_cost = Column(Float, default=0.0, nullable=False)
    output_cost = Column(Float, default=0.0, nullable=False)
    cache_creation_cost = Column(Float, default=0.0, nullable=False)
    cache_read_cost = Column(Float, default=0.0, nullable=False)

    # 性能统计
    avg_response_time_ms = Column(Float, default=0.0, nullable=False)
    fallback_count = Column(Integer, default=0, nullable=False)  # Provider 切换次数

    # 使用维度统计
    unique_models = Column(Integer, default=0, server_default="0", nullable=False)
    unique_providers = Column(Integer, default=0, server_default="0", nullable=False)

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


class StatsDailyModel(Base):
    """每日模型统计快照 - 用于快速查询每日模型维度数据"""

    __tablename__ = "stats_daily_model"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()))

    # 统计日期 (UTC)
    date = Column(DateTime(timezone=True), nullable=False, index=True)

    # 模型名称
    model = Column(String(100), nullable=False)

    # 请求统计
    total_requests = Column(Integer, default=0, nullable=False)

    # Token 统计
    input_tokens = Column(BigInteger, default=0, nullable=False)
    output_tokens = Column(BigInteger, default=0, nullable=False)
    cache_creation_tokens = Column(BigInteger, default=0, nullable=False)
    cache_read_tokens = Column(BigInteger, default=0, nullable=False)

    # 成本统计 (USD)
    total_cost = Column(Float, default=0.0, nullable=False)

    # 性能统计
    avg_response_time_ms = Column(Float, default=0.0, nullable=False)

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

    # 唯一约束：每个模型每天只有一条记录
    __table_args__ = (
        UniqueConstraint("date", "model", name="uq_stats_daily_model"),
        Index("idx_stats_daily_model_date", "date"),
        Index("idx_stats_daily_model_date_model", "date", "model"),
    )


class StatsDailyProvider(Base):
    """每日供应商统计快照 - 用于快速查询每日供应商维度数据"""

    __tablename__ = "stats_daily_provider"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()))

    # 统计日期 (UTC)
    date = Column(DateTime(timezone=True), nullable=False, index=True)

    # 供应商名称
    provider_name = Column(String(100), nullable=False)

    # 请求统计
    total_requests = Column(Integer, default=0, nullable=False)

    # Token 统计
    input_tokens = Column(BigInteger, default=0, nullable=False)
    output_tokens = Column(BigInteger, default=0, nullable=False)
    cache_creation_tokens = Column(BigInteger, default=0, nullable=False)
    cache_read_tokens = Column(BigInteger, default=0, nullable=False)

    # 成本统计 (USD)
    total_cost = Column(Float, default=0.0, nullable=False)

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

    # 唯一约束：每个供应商每天只有一条记录
    __table_args__ = (
        UniqueConstraint("date", "provider_name", name="uq_stats_daily_provider"),
        Index("idx_stats_daily_provider_date", "date"),
        Index("idx_stats_daily_provider_date_provider", "date", "provider_name"),
    )


class StatsSummary(Base):
    """全局统计汇总 - 单行记录，存储截止到昨天的累计数据"""

    __tablename__ = "stats_summary"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()))

    # 统计截止日期 (不含当天)
    cutoff_date = Column(DateTime(timezone=True), nullable=False)

    # 累计请求统计
    all_time_requests = Column(Integer, default=0, nullable=False)
    all_time_success_requests = Column(Integer, default=0, nullable=False)
    all_time_error_requests = Column(Integer, default=0, nullable=False)

    # 累计 Token 统计
    all_time_input_tokens = Column(BigInteger, default=0, nullable=False)
    all_time_output_tokens = Column(BigInteger, default=0, nullable=False)
    all_time_cache_creation_tokens = Column(BigInteger, default=0, nullable=False)
    all_time_cache_read_tokens = Column(BigInteger, default=0, nullable=False)

    # 累计成本统计 (USD)
    all_time_cost = Column(Float, default=0.0, nullable=False)
    all_time_actual_cost = Column(Float, default=0.0, nullable=False)

    # 累计用户/API Key 统计 (快照)
    total_users = Column(Integer, default=0, nullable=False)
    active_users = Column(Integer, default=0, nullable=False)
    total_api_keys = Column(Integer, default=0, nullable=False)
    active_api_keys = Column(Integer, default=0, nullable=False)

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


class StatsUserDaily(Base):
    """用户每日统计快照 - 用于用户仪表盘"""

    __tablename__ = "stats_user_daily"

    id = Column(String(36), primary_key=True, default=lambda: str(uuid.uuid4()))

    # 用户关联
    user_id = Column(String(36), ForeignKey("users.id", ondelete="CASCADE"), nullable=False)

    # 统计日期 (UTC)
    date = Column(DateTime(timezone=True), nullable=False, index=True)

    # 请求统计
    total_requests = Column(Integer, default=0, nullable=False)
    success_requests = Column(Integer, default=0, nullable=False)
    error_requests = Column(Integer, default=0, nullable=False)

    # Token 统计
    input_tokens = Column(BigInteger, default=0, nullable=False)
    output_tokens = Column(BigInteger, default=0, nullable=False)
    cache_creation_tokens = Column(BigInteger, default=0, nullable=False)
    cache_read_tokens = Column(BigInteger, default=0, nullable=False)

    # 成本统计 (USD)
    total_cost = Column(Float, default=0.0, nullable=False)

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

    # 唯一约束：每个用户每天只有一条记录
    __table_args__ = (
        UniqueConstraint("user_id", "date", name="uq_stats_user_daily"),
        Index("idx_stats_user_daily_user_date", "user_id", "date"),
    )

    # 关系
    user = relationship("User")


# 导入扩展的数据库模型
from .database_extensions import ApiKeyProviderMapping, ProviderUsageTracking
