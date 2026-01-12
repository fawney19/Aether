"""
API端点请求/响应模型定义
"""

import re
from datetime import datetime
from typing import Any, Dict, List, Literal, Optional

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from ..core.enums import UserRole


# ========== 认证相关 ==========
class LoginRequest(BaseModel):
    """登录请求"""

    email: str = Field(..., min_length=1, max_length=255, description="邮箱/用户名")
    password: str = Field(..., min_length=1, max_length=128, description="密码")
    auth_type: Literal["local", "ldap"] = Field(default="local", description="认证类型")

    @classmethod
    @field_validator("password")
    def validate_password(cls, v):
        """验证密码不为空且去除前后空格"""
        v = v.strip()
        if not v:
            raise ValueError("密码不能为空")
        return v

    @model_validator(mode="after")
    def validate_login(self):
        """根据认证类型校验并规范化登录标识"""
        identifier = self.email.strip()

        if not identifier:
            raise ValueError("用户名/邮箱不能为空")

        # 本地和 LDAP 登录都支持用户名或邮箱
        # 如果是邮箱格式，转换为小写
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if re.match(email_pattern, identifier):
            self.email = identifier.lower()
        else:
            self.email = identifier

        return self


class LoginResponse(BaseModel):
    """登录响应"""

    access_token: str
    refresh_token: str  # 刷新令牌
    token_type: str = "bearer"
    expires_in: int = 86400  # Token有效期（秒），默认24小时
    user_id: str
    email: str
    username: str
    role: str


class RefreshTokenRequest(BaseModel):
    """刷新令牌请求"""

    refresh_token: str = Field(..., description="刷新令牌")


class RefreshTokenResponse(BaseModel):
    """刷新令牌响应"""

    access_token: str
    refresh_token: str  # 返回新的刷新令牌
    token_type: str = "bearer"
    expires_in: int = 86400  # Token有效期（秒），默认24小时


class RegisterRequest(BaseModel):
    """注册请求"""

    email: str = Field(..., min_length=3, max_length=255, description="邮箱地址")
    username: str = Field(..., min_length=2, max_length=50, description="用户名")
    password: str = Field(..., min_length=6, max_length=128, description="密码")

    @classmethod
    @field_validator("email")
    def validate_email(cls, v):
        """验证邮箱格式"""
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式无效")
        return v.lower()

    @classmethod
    @field_validator("username")
    def validate_username(cls, v):
        """验证用户名格式"""
        v = v.strip()
        if not v:
            raise ValueError("用户名不能为空")
        if not re.match(r"^[a-zA-Z0-9_-]+$", v):
            raise ValueError("用户名只能包含字母、数字、下划线和短横线")
        return v

    @classmethod
    @field_validator("password")
    def validate_password(cls, v):
        """验证密码强度"""
        if len(v) < 6:
            raise ValueError("密码至少需要6个字符")
        if not re.search(r"[A-Z]", v):
            raise ValueError("密码必须包含至少一个大写字母")
        if not re.search(r"[a-z]", v):
            raise ValueError("密码必须包含至少一个小写字母")
        if not re.search(r"\d", v):
            raise ValueError("密码必须包含至少一个数字")
        return v


class RegisterResponse(BaseModel):
    """注册响应"""

    user_id: str
    email: str
    username: str
    message: str


class LogoutResponse(BaseModel):
    """登出响应"""

    message: str
    success: bool


class SendVerificationCodeRequest(BaseModel):
    """发送验证码请求"""

    email: str = Field(..., min_length=3, max_length=255, description="邮箱地址")

    @field_validator("email")
    @classmethod
    def validate_email(cls, v):
        """验证邮箱格式"""
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式无效")
        return v.lower()


class SendVerificationCodeResponse(BaseModel):
    """发送验证码响应"""

    message: str
    success: bool
    expire_minutes: Optional[int] = None


class VerifyEmailRequest(BaseModel):
    """验证邮箱请求"""

    email: str = Field(..., min_length=3, max_length=255, description="邮箱地址")
    code: str = Field(..., min_length=6, max_length=6, description="6位验证码")

    @field_validator("email")
    @classmethod
    def validate_email(cls, v):
        """验证邮箱格式"""
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式无效")
        return v.lower()

    @field_validator("code")
    @classmethod
    def validate_code(cls, v):
        """验证验证码格式"""
        v = v.strip()
        if not v.isdigit():
            raise ValueError("验证码必须是6位数字")
        if len(v) != 6:
            raise ValueError("验证码必须是6位数字")
        return v


class VerifyEmailResponse(BaseModel):
    """验证邮箱响应"""

    message: str
    success: bool


class VerificationStatusRequest(BaseModel):
    """验证状态查询请求"""

    email: str = Field(..., min_length=3, max_length=255, description="邮箱地址")

    @field_validator("email")
    @classmethod
    def validate_email(cls, v):
        """验证邮箱格式"""
        v = v.strip().lower()
        if not v:
            raise ValueError("邮箱不能为空")
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式无效")
        return v


class VerificationStatusResponse(BaseModel):
    """验证状态响应"""

    email: str
    has_pending_code: bool = Field(description="是否有待验证的验证码")
    is_verified: bool = Field(description="邮箱是否已验证")
    cooldown_remaining: Optional[int] = Field(None, description="发送冷却剩余秒数")
    code_expires_in: Optional[int] = Field(None, description="验证码剩余有效秒数")


class RegistrationSettingsResponse(BaseModel):
    """注册设置响应（公开接口返回）"""

    enable_registration: bool
    require_email_verification: bool


# ========== 用户管理 ==========
class CreateUserRequest(BaseModel):
    """创建用户请求"""

    username: str = Field(..., min_length=2, max_length=50, description="用户名")
    password: str = Field(..., min_length=6, max_length=128, description="密码")
    email: str = Field(..., min_length=3, max_length=255, description="邮箱地址")
    role: Optional[UserRole] = Field(UserRole.USER, description="用户角色")
    quota_usd: Optional[float] = Field(default=10.0, description="USD配额，null表示无限制")

    @field_validator("quota_usd", mode="before")
    @classmethod
    def validate_quota_usd(cls, v):
        """验证配额值，允许null表示无限制"""
        if v is None:
            return None
        if isinstance(v, (int, float)) and v >= 0 and v <= 10000:
            return float(v)
        if isinstance(v, (int, float)):
            raise ValueError("配额必须在 0-10000 范围内")
        return v

    @classmethod
    @field_validator("email")
    def validate_email(cls, v):
        """验证邮箱格式"""
        v = v.strip()
        if not v:
            raise ValueError("邮箱不能为空")
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式无效")
        return v.lower()

    @classmethod
    @field_validator("username")
    def validate_username(cls, v):
        """验证用户名格式"""
        v = v.strip()
        if not v:
            raise ValueError("用户名不能为空")
        if not re.match(r"^[a-zA-Z0-9_-]+$", v):
            raise ValueError("用户名只能包含字母、数字、下划线和短横线")
        return v

    @classmethod
    @field_validator("password")
    def validate_password(cls, v):
        """验证密码强度"""
        if len(v) < 6:
            raise ValueError("密码至少需要6个字符")
        if not re.search(r"[A-Z]", v):
            raise ValueError("密码必须包含至少一个大写字母")
        if not re.search(r"[a-z]", v):
            raise ValueError("密码必须包含至少一个小写字母")
        if not re.search(r"\d", v):
            raise ValueError("密码必须包含至少一个数字")
        return v


class UpdateUserRequest(BaseModel):
    """更新用户请求"""

    email: Optional[str] = None
    username: Optional[str] = None
    password: Optional[str] = None
    role: Optional[UserRole] = None
    allowed_providers: Optional[List[str]] = None  # 允许使用的提供商 ID 列表
    allowed_api_formats: Optional[List[str]] = None  # 允许使用的 API 格式列表
    allowed_models: Optional[List[str]] = None  # 允许使用的模型名称列表
    quota_usd: Optional[float] = None
    is_active: Optional[bool] = None

    @field_validator("quota_usd", mode="before")
    @classmethod
    def validate_quota_usd(cls, v):
        """验证配额值，允许null表示无限制"""
        if v is None:
            return None
        if isinstance(v, (int, float)) and v >= 0 and v <= 10000:
            return float(v)
        if isinstance(v, (int, float)):
            raise ValueError("配额必须在 0-10000 范围内")
        return v


class CreateApiKeyRequest(BaseModel):
    """创建API密钥请求"""

    name: Optional[str] = None
    allowed_providers: Optional[List[str]] = None  # 允许使用的提供商 ID 列表
    allowed_api_formats: Optional[List[str]] = None  # 允许使用的 API 格式列表
    allowed_models: Optional[List[str]] = None  # 允许使用的模型名称列表
    rate_limit: Optional[int] = None  # None = 无限制
    expire_days: Optional[int] = None  # None = 永不过期，数字 = 多少天后过期（兼容旧版）
    expires_at: Optional[str] = None  # ISO 日期字符串，如 "2025-12-31"，优先于 expire_days
    initial_balance_usd: Optional[float] = Field(
        None, description="初始余额（USD），仅用于独立Key，None = 无限制"
    )
    is_standalone: bool = Field(False, description="是否为独立余额Key（给非注册用户使用）")
    auto_delete_on_expiry: bool = Field(
        False, description="过期后是否自动删除（True=物理删除，False=仅禁用）"
    )


class UserResponse(BaseModel):
    """用户响应"""

    id: str
    email: str
    username: str
    role: UserRole
    allowed_providers: Optional[List[str]] = None  # 允许使用的提供商 ID 列表
    allowed_api_formats: Optional[List[str]] = None  # 允许使用的 API 格式列表
    allowed_models: Optional[List[str]] = None  # 允许使用的模型名称列表
    quota_usd: float
    used_usd: float
    is_active: bool
    created_at: datetime
    updated_at: datetime
    last_login_at: Optional[datetime]


class ApiKeyResponse(BaseModel):
    """API密钥响应"""

    id: str
    user_id: str
    key: Optional[str] = None  # 仅在创建时返回完整密钥
    key_display: Optional[str] = None  # 脱敏后的密钥显示
    name: Optional[str]
    total_requests: int
    total_tokens: int
    total_cost_usd: float
    allowed_providers: Optional[List[str]]
    allowed_models: Optional[List[str]]
    rate_limit: int
    is_active: bool
    expires_at: Optional[datetime] = None
    balance_used_usd: float = 0.0
    current_balance_usd: Optional[float] = None  # NULL = 无限制
    is_standalone: bool = False
    force_capabilities: Optional[Dict[str, bool]] = None  # 强制开启的能力
    created_at: datetime
    last_used_at: Optional[datetime]


# ========== 提供商管理 ==========
class ProviderCreate(BaseModel):
    """创建提供商请求

    架构说明：
    - Provider 仅包含提供商的元数据和计费配置
    - API格式、URL、认证等配置应在 ProviderEndpoint 中设置
    - API密钥应在 ProviderAPIKey 中设置
    """

    name: str = Field(..., min_length=1, max_length=100, description="提供商名称（唯一）")
    description: Optional[str] = Field(None, description="提供商描述")
    website: Optional[str] = Field(None, max_length=500, description="主站网站")

    # Provider 级别的配置
    rate_limit: Optional[int] = Field(None, description="每分钟请求限制")
    concurrent_limit: Optional[int] = Field(None, description="并发请求限制")
    config: Optional[dict] = Field(None, description="额外配置")
    is_active: bool = Field(False, description="是否启用（默认false，需要配置API密钥后才能启用）")


class ProviderUpdate(BaseModel):
    """更新提供商请求"""

    name: Optional[str] = Field(None, min_length=1, max_length=100)
    description: Optional[str] = None
    website: Optional[str] = Field(None, max_length=500)
    api_format: Optional[str] = None
    base_url: Optional[str] = None
    headers: Optional[dict] = None
    timeout: Optional[int] = Field(None, ge=1, le=600)
    max_retries: Optional[int] = Field(None, ge=0, le=10)
    priority: Optional[int] = None
    weight: Optional[float] = Field(None, gt=0)
    rate_limit: Optional[int] = None
    concurrent_limit: Optional[int] = None
    config: Optional[dict] = None
    is_active: Optional[bool] = None


class ProviderResponse(BaseModel):
    """提供商响应"""

    id: str
    name: str
    description: Optional[str]
    website: Optional[str]
    api_format: str
    base_url: str
    headers: Optional[dict]
    timeout: int
    max_retries: int
    priority: int
    weight: float
    rate_limit: Optional[int]
    concurrent_limit: Optional[int]
    config: Optional[dict]
    is_active: bool
    created_at: datetime
    updated_at: datetime
    models_count: int = 0
    active_models_count: int = 0
    api_keys_count: int = 0

    model_config = ConfigDict(from_attributes=True)


# ========== 模型管理 ==========
class ModelCreate(BaseModel):
    """创建模型请求 - 价格和能力字段可选，为空时使用 GlobalModel 默认值"""

    provider_model_name: str = Field(
        ..., min_length=1, max_length=200, description="Provider 侧的主模型名称"
    )
    provider_model_mappings: Optional[List[dict]] = Field(
        None,
        description="模型名称映射列表，格式: [{'name': 'alias1', 'priority': 1}, ...]",
    )
    global_model_id: str = Field(..., description="关联的 GlobalModel ID（必填）")
    # 按次计费配置 - 可选，为空时使用 GlobalModel 默认值
    price_per_request: Optional[float] = Field(
        None, ge=0, description="每次请求固定费用，为空使用默认值"
    )
    # 阶梯计费配置 - 可选，为空时使用 GlobalModel 默认值
    tiered_pricing: Optional[dict] = Field(
        None, description="阶梯计费配置，为空使用 GlobalModel 默认值"
    )
    # 能力配置 - 可选，为空时使用 GlobalModel 默认值
    supports_vision: Optional[bool] = Field(None, description="是否支持图像输入，为空使用默认值")
    supports_function_calling: Optional[bool] = Field(
        None, description="是否支持函数调用，为空使用默认值"
    )
    supports_streaming: Optional[bool] = Field(None, description="是否支持流式输出，为空使用默认值")
    supports_extended_thinking: Optional[bool] = Field(
        None, description="是否支持扩展思考，为空使用默认值"
    )
    is_active: bool = Field(True, description="是否启用")
    config: Optional[dict] = Field(None, description="额外配置")


class ModelUpdate(BaseModel):
    """更新模型请求"""

    provider_model_name: Optional[str] = Field(None, min_length=1, max_length=200)
    provider_model_mappings: Optional[List[dict]] = Field(
        None,
        description="模型名称映射列表，格式: [{'name': 'alias1', 'priority': 1}, ...]",
    )
    global_model_id: Optional[str] = None
    # 按次计费配置
    price_per_request: Optional[float] = Field(None, ge=0, description="每次请求固定费用")
    # 阶梯计费配置
    tiered_pricing: Optional[dict] = Field(None, description="阶梯计费配置")
    supports_vision: Optional[bool] = None
    supports_function_calling: Optional[bool] = None
    supports_streaming: Optional[bool] = None
    supports_extended_thinking: Optional[bool] = None
    is_active: Optional[bool] = None
    is_available: Optional[bool] = None
    config: Optional[dict] = None


class ModelResponse(BaseModel):
    """模型响应 - 包含 Model 配置和关联的 GlobalModel 信息

    注意：价格和能力字段返回的是有效值（优先使用 Model 配置，否则使用 GlobalModel 默认值）
    """

    id: str
    provider_id: str
    global_model_id: Optional[str]
    provider_model_name: str
    provider_model_mappings: Optional[List[dict]] = None

    # 按次计费配置
    price_per_request: Optional[float] = None
    # 阶梯计费配置
    tiered_pricing: Optional[dict] = None

    # Provider 能力配置 - 可选，为空表示使用 GlobalModel 默认值
    supports_vision: Optional[bool]
    supports_function_calling: Optional[bool]
    supports_streaming: Optional[bool]
    supports_extended_thinking: Optional[bool]
    supports_image_generation: Optional[bool]

    # 有效值（合并 Model 配置和 GlobalModel 默认值后的结果）
    effective_tiered_pricing: Optional[dict] = None
    effective_input_price: Optional[float] = None
    effective_output_price: Optional[float] = None
    effective_price_per_request: Optional[float] = None
    effective_supports_vision: Optional[bool] = None
    effective_supports_function_calling: Optional[bool] = None
    effective_supports_streaming: Optional[bool] = None
    effective_supports_extended_thinking: Optional[bool] = None
    effective_supports_image_generation: Optional[bool] = None

    # 状态
    is_active: bool
    is_available: bool

    # 时间戳
    created_at: datetime
    updated_at: datetime

    # 关联的 GlobalModel 信息（如果有）
    global_model_name: Optional[str] = None
    global_model_display_name: Optional[str] = None

    model_config = ConfigDict(from_attributes=True)


class ModelDetailResponse(BaseModel):
    """模型详细响应 - 包含所有字段（用于需要完整信息的场景）"""

    id: str
    provider_id: str
    name: str
    display_name: str
    description: Optional[str]
    icon_url: Optional[str]
    tags: Optional[List[str]]
    input_price_per_1m: float
    output_price_per_1m: float
    cache_creation_price_per_1m: Optional[float]
    cache_read_price_per_1m: Optional[float]
    supports_vision: bool
    supports_function_calling: bool
    supports_streaming: bool
    is_active: bool
    is_available: bool
    config: Optional[dict]
    created_at: datetime
    updated_at: datetime

    model_config = ConfigDict(from_attributes=True)


# ========== 系统设置 ==========
class SystemSettingsRequest(BaseModel):
    """系统设置请求"""

    default_provider: Optional[str] = None
    default_model: Optional[str] = None
    enable_usage_tracking: Optional[bool] = None


class SystemSettingsResponse(BaseModel):
    """系统设置响应"""

    default_provider: Optional[str]
    default_model: Optional[str]
    enable_usage_tracking: bool


# ========== 使用统计 ==========
class UsageStatsResponse(BaseModel):
    """使用统计响应"""

    total_requests: int
    total_tokens: int
    total_cost_usd: float
    daily_requests: int
    daily_tokens: int
    daily_cost_usd: float
    model_usage: Dict[str, Dict[str, Any]]
    provider_usage: Dict[str, Dict[str, Any]]


# ========== 公开API响应模型 ==========
class PublicProviderResponse(BaseModel):
    """公开的提供商信息响应"""

    id: str
    name: str
    description: Optional[str]
    website: Optional[str]
    is_active: bool
    provider_priority: int  # 提供商优先级（数字越小越优先）
    # 统计信息
    models_count: int
    active_models_count: int
    endpoints_count: int  # 端点总数
    active_endpoints_count: int  # 活跃端点数


class PublicModelResponse(BaseModel):
    """公开的模型信息响应"""

    id: str
    provider_id: str
    provider_name: str
    name: str
    display_name: str
    description: Optional[str] = None
    tags: Optional[List[str]] = None
    icon_url: Optional[str] = None
    # 价格信息
    input_price_per_1m: Optional[float] = None
    output_price_per_1m: Optional[float] = None
    cache_creation_price_per_1m: Optional[float] = None
    cache_read_price_per_1m: Optional[float] = None
    # 功能支持
    supports_vision: Optional[bool] = None
    supports_function_calling: Optional[bool] = None
    supports_streaming: Optional[bool] = None
    is_active: bool = True


class ProviderStatsResponse(BaseModel):
    """提供商统计信息响应"""

    total_providers: int
    active_providers: int
    total_models: int
    active_models: int
    supported_formats: List[str]


class PublicGlobalModelResponse(BaseModel):
    """公开的 GlobalModel 信息响应（用户可见）"""

    id: str
    name: str
    display_name: Optional[str] = None
    is_active: bool = True
    # 按次计费配置
    default_price_per_request: Optional[float] = None
    # 阶梯计费配置
    default_tiered_pricing: Optional[dict] = None
    # Key 能力配置
    supported_capabilities: Optional[List[str]] = None
    # 模型配置（JSON）
    config: Optional[dict] = None


class PublicGlobalModelListResponse(BaseModel):
    """公开的 GlobalModel 列表响应"""

    models: List[PublicGlobalModelResponse]
    total: int


# ========== 个人中心相关模型 ==========
class UpdateProfileRequest(BaseModel):
    """更新个人信息请求"""

    email: Optional[str] = None
    username: Optional[str] = None


class UpdatePreferencesRequest(BaseModel):
    """更新偏好设置请求"""

    avatar_url: Optional[str] = None
    bio: Optional[str] = None
    default_provider_id: Optional[int] = None
    theme: Optional[str] = None
    language: Optional[str] = None
    timezone: Optional[str] = None
    email_notifications: Optional[bool] = None
    usage_alerts: Optional[bool] = None
    announcement_notifications: Optional[bool] = None


class ChangePasswordRequest(BaseModel):
    """修改密码请求"""

    old_password: str
    new_password: str


class CreateMyApiKeyRequest(BaseModel):
    """创建我的API密钥请求"""

    name: str


class ProviderConfig(BaseModel):
    """提供商配置"""

    provider_id: str = Field(..., description="提供商ID")
    priority: int = Field(100, description="优先级（越高越优先）")
    weight: float = Field(1.0, description="负载均衡权重")
    enabled: bool = Field(True, description="是否启用")


class UpdateApiKeyProvidersRequest(BaseModel):
    """更新API密钥可用提供商请求"""

    allowed_providers: Optional[List[ProviderConfig]] = None  # 提供商配置列表


# ========== 公告相关模型 ==========
class CreateAnnouncementRequest(BaseModel):
    """创建公告请求"""

    title: str
    content: str  # 支持Markdown
    type: str = "info"  # info, warning, maintenance, important
    priority: int = 0
    is_pinned: bool = False
    start_time: Optional[datetime] = None
    end_time: Optional[datetime] = None


class UpdateAnnouncementRequest(BaseModel):
    """更新公告请求"""

    title: Optional[str] = None
    content: Optional[str] = None
    type: Optional[str] = None
    priority: Optional[int] = None
    is_active: Optional[bool] = None
    is_pinned: Optional[bool] = None
    start_time: Optional[datetime] = None
    end_time: Optional[datetime] = None
