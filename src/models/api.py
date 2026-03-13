"""
API端点请求/响应模型定义
"""

from __future__ import annotations

import re
from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from ..core.enums import UserRole
from ..core.validators import PasswordValidator


# ========== 认证相关 ==========
class LoginRequest(BaseModel):
    """登录请求"""

    email: str = Field(..., min_length=1, max_length=255, description="邮箱/用户名")
    password: str = Field(..., min_length=1, max_length=128, description="密码")
    auth_type: Literal["local", "ldap"] = Field(default="local", description="认证类型")

    @classmethod
    @field_validator("password")
    def validate_password(cls, v: Any) -> Any:
        """验证密码不为空且去除前后空格"""
        v = v.strip()
        if not v:
            raise ValueError("密码不能为空")
        return v

    @model_validator(mode="after")
    def validate_login(self) -> Any:
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
    email: str | None = None
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

    email: str | None = Field(None, max_length=255, description="邮箱地址（可选）")
    username: str = Field(..., min_length=2, max_length=50, description="用户名")
    password: str = Field(..., min_length=6, max_length=128, description="密码")

    @field_validator("email")
    @classmethod
    def validate_email(cls, v: Any) -> Any:
        """验证邮箱格式（如果提供）"""
        if v is None:
            return None
        v = v.strip()
        if not v:
            return None
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式无效")
        return v.lower()

    @classmethod
    @field_validator("username")
    def validate_username(cls, v: Any) -> Any:
        """验证用户名格式"""
        v = v.strip()
        if not v:
            raise ValueError("用户名不能为空")
        if not re.match(r"^[a-zA-Z0-9_.\-]+$", v):
            raise ValueError("用户名只能包含字母、数字、下划线、连字符和点号")
        return v

    @classmethod
    @field_validator("password")
    def validate_password(cls, v: Any) -> Any:
        """基础校验（长度上下限），策略级别校验在服务层按系统配置执行。"""
        valid, error_msg = PasswordValidator.validate(v)
        if not valid:
            raise ValueError(error_msg or "密码格式无效")
        return v


class RegisterResponse(BaseModel):
    """注册响应"""

    user_id: str
    email: str | None = None
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
    def validate_email(cls, v: Any) -> Any:
        """验证邮箱格式"""
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式无效")
        return v.lower()


class SendVerificationCodeResponse(BaseModel):
    """发送验证码响应"""

    message: str
    success: bool
    expire_minutes: int | None = None


class VerifyEmailRequest(BaseModel):
    """验证邮箱请求"""

    email: str = Field(..., min_length=3, max_length=255, description="邮箱地址")
    code: str = Field(..., min_length=6, max_length=6, description="6位验证码")

    @field_validator("email")
    @classmethod
    def validate_email(cls, v: Any) -> Any:
        """验证邮箱格式"""
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式无效")
        return v.lower()

    @field_validator("code")
    @classmethod
    def validate_code(cls, v: Any) -> Any:
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
    def validate_email(cls, v: Any) -> Any:
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
    cooldown_remaining: int | None = Field(None, description="发送冷却剩余秒数")
    code_expires_in: int | None = Field(None, description="验证码剩余有效秒数")


class RegistrationSettingsResponse(BaseModel):
    """注册设置响应（公开接口返回）"""

    enable_registration: bool
    require_email_verification: bool
    email_configured: bool = Field(description="是否配置了邮箱服务")
    password_policy_level: str = Field(description="密码策略等级：weak/medium/strong")


# ========== 用户管理 ==========
class CreateUserRequest(BaseModel):
    """创建用户请求"""

    username: str = Field(..., min_length=2, max_length=50, description="用户名")
    password: str = Field(..., min_length=6, max_length=128, description="密码")
    email: str | None = Field(None, max_length=255, description="邮箱地址（可选）")
    role: UserRole | None = Field(UserRole.USER, description="用户角色")
    initial_gift_usd: float | None = Field(
        default=None, description="初始赠款（USD），null 表示使用系统默认初始赠款"
    )
    unlimited: bool = Field(default=False, description="是否无限制")
    # 访问限制字段
    allowed_providers: list[str] | None = Field(
        default=None, description="允许使用的提供商ID列表，null表示无限制"
    )
    allowed_api_formats: list[str] | None = Field(
        default=None, description="允许使用的API格式列表，null表示无限制"
    )
    allowed_models: list[str] | None = Field(
        default=None, description="允许使用的模型名称列表，null表示无限制"
    )
    rate_limit: int | None = Field(
        default=None,
        ge=0,
        description="每分钟请求限制；null 表示继承系统默认，0 表示不限制",
    )

    @field_validator("initial_gift_usd", mode="before")
    @classmethod
    def validate_initial_gift_usd(cls, v: Any) -> Any:
        """验证初始赠款金额，null 表示使用系统默认初始赠款。"""
        if v is None:
            return None
        if isinstance(v, (int, float)) and v >= 0 and v <= 10000:
            return float(v)
        if isinstance(v, (int, float)):
            raise ValueError("初始赠款必须在 0-10000 范围内")
        return v

    @field_validator("email")
    @classmethod
    def validate_email(cls, v: str | None) -> str | None:
        """验证邮箱格式（如果提供）"""
        if v is None:
            return None
        v = v.strip()
        if not v:
            return None
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式无效")
        return v.lower()

    @field_validator("username")
    @classmethod
    def validate_username(cls, v: Any) -> Any:
        """验证用户名格式"""
        v = v.strip()
        if not v:
            raise ValueError("用户名不能为空")
        if not re.match(r"^[a-zA-Z0-9_.\-]+$", v):
            raise ValueError("用户名只能包含字母、数字、下划线、连字符和点号")
        return v

    @field_validator("allowed_api_formats")
    @classmethod
    def validate_allowed_api_formats(cls, v: list[str] | None) -> list[str] | None:
        """校验并规范化 allowed_api_formats（endpoint signature: family:kind）。"""
        if v is None:
            return None
        from src.core.api_format import list_endpoint_definitions, resolve_endpoint_definition
        from src.core.api_format.signature import normalize_signature_key

        allowed = [d.signature_key for d in list_endpoint_definitions()]
        out: list[str] = []
        seen: set[str] = set()
        for fmt in v:
            if not fmt:
                continue
            norm = normalize_signature_key(fmt)
            if resolve_endpoint_definition(norm) is None:
                raise ValueError(f"allowed_api_formats 必须是以下之一: {allowed}，当前值: {fmt}")
            if norm in seen:
                continue
            seen.add(norm)
            out.append(norm)
        return out

    @classmethod
    @field_validator("password")
    def validate_password(cls, v: Any) -> Any:
        """基础校验（长度上下限），策略级别校验在服务层按系统配置执行。"""
        valid, error_msg = PasswordValidator.validate(v)
        if not valid:
            raise ValueError(error_msg or "密码格式无效")
        return v


class UpdateUserRequest(BaseModel):
    """更新用户请求"""

    email: str | None = None
    username: str | None = None
    password: str | None = None
    role: UserRole | None = None
    unlimited: bool | None = None
    allowed_providers: list[str] | None = None  # 允许使用的提供商 ID 列表
    allowed_api_formats: list[str] | None = None  # 允许使用的 API 格式列表
    allowed_models: list[str] | None = None  # 允许使用的模型名称列表
    rate_limit: int | None = Field(
        default=None,
        ge=0,
        description="每分钟请求限制；null 表示继承系统默认，0 表示不限制",
    )
    is_active: bool | None = None

    @field_validator("allowed_api_formats")
    @classmethod
    def validate_allowed_api_formats(cls, v: list[str] | None) -> list[str] | None:
        # 与 CreateUserRequest 保持一致
        return CreateUserRequest.validate_allowed_api_formats(v)


class CreateApiKeyRequest(BaseModel):
    """创建API密钥请求"""

    name: str | None = None
    allowed_providers: list[str] | None = None  # 允许使用的提供商 ID 列表
    allowed_api_formats: list[str] | None = None  # 允许使用的 API 格式列表
    allowed_models: list[str] | None = None  # 允许使用的模型名称列表
    rate_limit: int | None = Field(
        None,
        ge=0,
        description="每分钟请求限制；独立Key: null=继承系统默认，0=不限制；普通Key: 0=不限制",
    )
    expire_days: int | None = None  # None = 永不过期，数字 = 多少天后过期
    expires_at: str | None = None  # ISO 日期字符串，如 "2025-12-31"，优先于 expire_days
    initial_balance_usd: float | None = Field(
        None, description="初始余额（USD），仅用于独立Key，None = 无限制"
    )
    unlimited_balance: bool | None = Field(
        None, description="是否无限余额（编辑独立Key时用于切换额度模式）"
    )
    is_standalone: bool = Field(False, description="是否为独立余额Key（给非注册用户使用）")
    auto_delete_on_expiry: bool = Field(
        False, description="过期后是否自动删除（True=物理删除，False=仅禁用）"
    )

    @field_validator("allowed_api_formats")
    @classmethod
    def validate_allowed_api_formats(cls, v: list[str] | None) -> list[str] | None:
        # 与 CreateUserRequest 保持一致
        return CreateUserRequest.validate_allowed_api_formats(v)


class UserResponse(BaseModel):
    """用户响应"""

    id: str
    email: str | None = None
    username: str
    role: UserRole
    allowed_providers: list[str] | None = None  # 允许使用的提供商 ID 列表
    allowed_api_formats: list[str] | None = None  # 允许使用的 API 格式列表
    allowed_models: list[str] | None = None  # 允许使用的模型名称列表
    rate_limit: int | None = None
    unlimited: bool = False
    is_active: bool
    created_at: datetime
    updated_at: datetime
    last_login_at: datetime | None


class ApiKeyResponse(BaseModel):
    """API密钥响应"""

    id: str
    user_id: str
    key: str | None = None  # 仅在创建时返回完整密钥
    key_display: str | None = None  # 脱敏后的密钥显示
    name: str | None
    total_requests: int
    total_tokens: int
    total_cost_usd: float
    allowed_providers: list[str] | None
    allowed_models: list[str] | None
    rate_limit: int | None
    is_active: bool
    expires_at: datetime | None = None
    is_standalone: bool = False
    force_capabilities: dict[str, bool] | None = None  # 强制开启的能力
    created_at: datetime
    last_used_at: datetime | None


# ========== 提供商管理 ==========
class ProviderCreate(BaseModel):
    """创建提供商请求

    架构说明：
    - Provider 仅包含提供商的元数据和计费配置
    - API格式、URL、认证等配置应在 ProviderEndpoint 中设置
    - API密钥应在 ProviderAPIKey 中设置
    """

    name: str = Field(..., min_length=1, max_length=100, description="提供商名称（唯一）")
    description: str | None = Field(None, description="提供商描述")
    website: str | None = Field(None, max_length=500, description="主站网站")

    # Provider 级别的配置
    rate_limit: int | None = Field(None, description="每分钟请求限制")
    concurrent_limit: int | None = Field(None, description="并发请求限制")
    config: dict | None = Field(None, description="额外配置")
    is_active: bool = Field(False, description="是否启用（默认false，需要配置API密钥后才能启用）")

    # 超时配置（秒），为空时使用全局配置
    stream_first_byte_timeout: float | None = Field(
        None, ge=1, le=300, description="流式请求首字节超时（秒）"
    )
    request_timeout: float | None = Field(
        None, ge=1, le=600, description="非流式请求整体超时（秒）"
    )


class ProviderUpdate(BaseModel):
    """更新提供商请求"""

    name: str | None = Field(None, min_length=1, max_length=100)
    description: str | None = None
    website: str | None = Field(None, max_length=500)
    api_format: str | None = None
    base_url: str | None = None
    headers: dict | None = None
    max_retries: int | None = Field(None, ge=0, le=10)
    priority: int | None = None
    weight: float | None = Field(None, gt=0)
    rate_limit: int | None = None
    concurrent_limit: int | None = None
    config: dict | None = None
    is_active: bool | None = None

    # 超时配置（秒），为空时使用全局配置
    stream_first_byte_timeout: float | None = Field(
        None, ge=1, le=300, description="流式请求首字节超时（秒）"
    )
    request_timeout: float | None = Field(
        None, ge=1, le=600, description="非流式请求整体超时（秒）"
    )


class ProviderResponse(BaseModel):
    """提供商响应"""

    id: str
    name: str
    description: str | None
    website: str | None
    api_format: str
    base_url: str
    headers: dict | None
    max_retries: int
    priority: int
    weight: float
    rate_limit: int | None
    concurrent_limit: int | None
    config: dict | None
    is_active: bool
    created_at: datetime
    updated_at: datetime
    models_count: int = 0
    active_models_count: int = 0
    api_keys_count: int = 0

    # 超时配置
    stream_first_byte_timeout: float | None = None
    request_timeout: float | None = None

    model_config = ConfigDict(from_attributes=True)


# ========== 模型管理 ==========
class ModelCreate(BaseModel):
    """创建模型请求 - 价格和能力字段可选，为空时使用 GlobalModel 默认值"""

    provider_model_name: str = Field(
        ..., min_length=1, max_length=200, description="Provider 侧的主模型名称"
    )
    provider_model_mappings: list[dict] | None = Field(
        None,
        description="模型名称映射列表，格式: [{'name': 'alias1', 'priority': 1}, ...]",
    )
    global_model_id: str = Field(..., description="关联的 GlobalModel ID（必填）")
    # 按次计费配置 - 可选，为空时使用 GlobalModel 默认值
    price_per_request: float | None = Field(
        None, ge=0, description="每次请求固定费用，为空使用默认值"
    )
    # 阶梯计费配置 - 可选，为空时使用 GlobalModel 默认值
    tiered_pricing: dict | None = Field(
        None, description="阶梯计费配置，为空使用 GlobalModel 默认值"
    )
    # 能力配置 - 可选，为空时使用 GlobalModel 默认值
    supports_vision: bool | None = Field(None, description="是否支持图像输入，为空使用默认值")
    supports_function_calling: bool | None = Field(
        None, description="是否支持函数调用，为空使用默认值"
    )
    supports_streaming: bool | None = Field(None, description="是否支持流式输出，为空使用默认值")
    supports_extended_thinking: bool | None = Field(
        None, description="是否支持扩展思考，为空使用默认值"
    )
    is_active: bool = Field(True, description="是否启用")
    config: dict | None = Field(None, description="额外配置")


class ModelUpdate(BaseModel):
    """更新模型请求"""

    provider_model_name: str | None = Field(None, min_length=1, max_length=200)
    provider_model_mappings: list[dict] | None = Field(
        None,
        description="模型名称映射列表，格式: [{'name': 'alias1', 'priority': 1}, ...]",
    )
    global_model_id: str | None = None
    # 按次计费配置
    price_per_request: float | None = Field(None, ge=0, description="每次请求固定费用")
    # 阶梯计费配置
    tiered_pricing: dict | None = Field(None, description="阶梯计费配置")
    supports_vision: bool | None = None
    supports_function_calling: bool | None = None
    supports_streaming: bool | None = None
    supports_extended_thinking: bool | None = None
    is_active: bool | None = None
    is_available: bool | None = None
    config: dict | None = None


class ModelResponse(BaseModel):
    """模型响应 - 包含 Model 配置和关联的 GlobalModel 信息

    注意：价格和能力字段返回的是有效值（优先使用 Model 配置，否则使用 GlobalModel 默认值）
    """

    id: str
    provider_id: str
    global_model_id: str
    provider_model_name: str
    provider_model_mappings: list[dict] | None = None

    # 按次计费配置
    price_per_request: float | None = None
    # 阶梯计费配置
    tiered_pricing: dict | None = None

    # Provider 能力配置 - 可选，为空表示使用 GlobalModel 默认值
    supports_vision: bool | None
    supports_function_calling: bool | None
    supports_streaming: bool | None
    supports_extended_thinking: bool | None
    supports_image_generation: bool | None

    # 有效值（合并 Model 配置和 GlobalModel 默认值后的结果）
    effective_tiered_pricing: dict | None = None
    effective_input_price: float | None = None
    effective_output_price: float | None = None
    effective_price_per_request: float | None = None
    effective_supports_vision: bool | None = None
    effective_supports_function_calling: bool | None = None
    effective_supports_streaming: bool | None = None
    effective_supports_extended_thinking: bool | None = None
    effective_supports_image_generation: bool | None = None

    # 状态
    is_active: bool
    is_available: bool

    # 时间戳
    created_at: datetime
    updated_at: datetime

    # 关联的 GlobalModel 信息
    global_model_name: str | None = None
    global_model_display_name: str | None = None

    # 有效配置（合并 Model 和 GlobalModel 的 config）
    effective_config: dict | None = None

    model_config = ConfigDict(from_attributes=True)


class ModelDetailResponse(BaseModel):
    """模型详细响应 - 包含所有字段（用于需要完整信息的场景）"""

    id: str
    provider_id: str
    name: str
    display_name: str
    description: str | None
    icon_url: str | None
    tags: list[str] | None
    input_price_per_1m: float
    output_price_per_1m: float
    cache_creation_price_per_1m: float | None
    cache_read_price_per_1m: float | None
    supports_vision: bool
    supports_function_calling: bool
    supports_streaming: bool
    is_active: bool
    is_available: bool
    config: dict | None
    created_at: datetime
    updated_at: datetime

    model_config = ConfigDict(from_attributes=True)


# ========== 系统设置 ==========
class SystemSettingsRequest(BaseModel):
    """系统设置请求"""

    default_provider: str | None = None
    default_model: str | None = None
    enable_usage_tracking: bool | None = None
    password_policy_level: Literal["weak", "medium", "strong"] | None = None


class SystemSettingsResponse(BaseModel):
    """系统设置响应"""

    default_provider: str | None
    default_model: str | None
    enable_usage_tracking: bool
    password_policy_level: str


# ========== 使用统计 ==========
class UsageStatsResponse(BaseModel):
    """使用统计响应"""

    total_requests: int
    total_tokens: int
    total_cost_usd: float
    daily_requests: int
    daily_tokens: int
    daily_cost_usd: float
    model_usage: dict[str, dict[str, Any]]
    provider_usage: dict[str, dict[str, Any]]


# ========== 公开API响应模型 ==========
class PublicProviderResponse(BaseModel):
    """公开的提供商信息响应"""

    id: str
    name: str
    description: str | None
    website: str | None
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
    description: str | None = None
    tags: list[str] | None = None
    icon_url: str | None = None
    # 价格信息
    input_price_per_1m: float | None = None
    output_price_per_1m: float | None = None
    cache_creation_price_per_1m: float | None = None
    cache_read_price_per_1m: float | None = None
    # 功能支持
    supports_vision: bool | None = None
    supports_function_calling: bool | None = None
    supports_streaming: bool | None = None
    is_active: bool = True


class ProviderStatsResponse(BaseModel):
    """提供商统计信息响应"""

    total_providers: int
    active_providers: int
    total_models: int
    active_models: int
    supported_formats: list[str]


class PublicGlobalModelResponse(BaseModel):
    """公开的 GlobalModel 信息响应（用户可见）"""

    id: str
    name: str
    display_name: str | None = None
    is_active: bool = True
    # 按次计费配置
    default_price_per_request: float | None = None
    # 阶梯计费配置
    default_tiered_pricing: dict | None = None
    # Key 能力配置
    supported_capabilities: list[str] | None = None
    # 模型配置（JSON）
    config: dict | None = None
    # 调用次数
    usage_count: int = 0


class PublicGlobalModelListResponse(BaseModel):
    """公开的 GlobalModel 列表响应"""

    models: list[PublicGlobalModelResponse]
    total: int


# ========== 个人中心相关模型 ==========
class UpdateProfileRequest(BaseModel):
    """更新个人信息请求"""

    email: str | None = None
    username: str | None = None


class UpdatePreferencesRequest(BaseModel):
    """更新偏好设置请求"""

    avatar_url: str | None = None
    bio: str | None = None
    default_provider_id: int | None = None
    theme: str | None = None
    language: str | None = None
    timezone: str | None = None
    email_notifications: bool | None = None
    usage_alerts: bool | None = None
    announcement_notifications: bool | None = None


class ChangePasswordRequest(BaseModel):
    """修改密码请求"""

    old_password: str | None = None  # 可选：首次设置密码时不需要
    new_password: str


class CreateMyApiKeyRequest(BaseModel):
    """创建我的API密钥请求"""

    name: str
    rate_limit: int = Field(0, ge=0, description="该 Key 的每分钟请求限制，0 表示不限制")


class UpdateMyApiKeyRequest(BaseModel):
    """更新我的 API 密钥请求"""

    name: str | None = None
    rate_limit: int | None = Field(
        None,
        ge=0,
        description="该 Key 的每分钟请求限制；0 表示不限制，null 表示不修改",
    )


class ProviderConfig(BaseModel):
    """提供商配置"""

    provider_id: str = Field(..., description="提供商ID")
    priority: int = Field(100, description="优先级（越高越优先）")
    weight: float = Field(1.0, description="负载均衡权重")
    enabled: bool = Field(True, description="是否启用")


class UpdateApiKeyProvidersRequest(BaseModel):
    """更新API密钥可用提供商请求"""

    allowed_providers: list[ProviderConfig] | None = None  # 提供商配置列表


# ========== 公告相关模型 ==========
class CreateAnnouncementRequest(BaseModel):
    """创建公告请求"""

    title: str
    content: str  # 支持Markdown
    type: str = "info"  # info, warning, maintenance, important
    priority: int = 0
    is_pinned: bool = False
    start_time: datetime | None = None
    end_time: datetime | None = None


class UpdateAnnouncementRequest(BaseModel):
    """更新公告请求"""

    title: str | None = None
    content: str | None = None
    type: str | None = None
    priority: int | None = None
    is_active: bool | None = None
    is_pinned: bool | None = None
    start_time: datetime | None = None
    end_time: datetime | None = None
