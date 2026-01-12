"""
管理接口的 Pydantic 请求模型

提供完整的输入验证和安全过滤
"""

import re
from datetime import datetime
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field, field_validator, model_validator

from src.core.enums import APIFormat, ProviderBillingType


class ProxyConfig(BaseModel):
    """代理配置"""

    url: str = Field(..., description="代理 URL (http://, https://, socks5://)")
    username: Optional[str] = Field(None, max_length=255, description="代理用户名")
    password: Optional[str] = Field(None, max_length=500, description="代理密码")
    enabled: bool = Field(True, description="是否启用代理（false 时保留配置但不使用）")

    @field_validator("url")
    @classmethod
    def validate_proxy_url(cls, v: str) -> str:
        """验证代理 URL 格式"""
        from urllib.parse import urlparse

        v = v.strip()

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


class CreateProviderRequest(BaseModel):
    """创建 Provider 请求"""

    name: str = Field(..., min_length=1, max_length=100, description="提供商名称（唯一）")
    description: Optional[str] = Field(None, max_length=1000, description="描述")
    website: Optional[str] = Field(None, max_length=500, description="官网地址")

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
            "SELECT", "INSERT", "UPDATE", "DELETE", "DROP", "CREATE",
            "ALTER", "TRUNCATE", "UNION", "EXEC", "EXECUTE", "--", "/*", "*/"
        ]
        v_upper = v.upper()
        for keyword in sql_keywords:
            if keyword in v_upper:
                raise ValueError(f"名称包含非法关键字: {keyword}")

        return v
    billing_type: Optional[str] = Field(
        ProviderBillingType.PAY_AS_YOU_GO.value, description="计费类型"
    )
    monthly_quota_usd: Optional[float] = Field(None, ge=0, description="周期配额（美元）")
    quota_reset_day: Optional[int] = Field(30, ge=1, le=365, description="配额重置周期（天数）")
    quota_last_reset_at: Optional[datetime] = Field(None, description="当前周期开始时间")
    quota_expires_at: Optional[datetime] = Field(None, description="配额过期时间")
    provider_priority: Optional[int] = Field(100, ge=0, le=1000, description="提供商优先级（数字越小越优先）")
    is_active: Optional[bool] = Field(True, description="是否启用")
    concurrent_limit: Optional[int] = Field(None, ge=0, description="并发限制")
    # 请求配置（从 Endpoint 迁移）
    timeout: Optional[int] = Field(300, ge=1, le=600, description="请求超时（秒）")
    max_retries: Optional[int] = Field(2, ge=0, le=10, description="最大重试次数")
    proxy: Optional[ProxyConfig] = Field(None, description="代理配置")
    config: Optional[Dict[str, Any]] = Field(None, description="其他配置")

    @field_validator("name", "description")
    @classmethod
    def sanitize_text(cls, v: Optional[str]) -> Optional[str]:
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
    def validate_website(cls, v: Optional[str]) -> Optional[str]:
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
    def validate_billing_type(cls, v: Optional[str]) -> Optional[str]:
        """验证计费类型"""
        if v is None:
            return ProviderBillingType.PAY_AS_YOU_GO.value

        try:
            ProviderBillingType(v)
            return v
        except ValueError:
            valid_types = [t.value for t in ProviderBillingType]
            raise ValueError(f"无效的计费类型，有效值为: {', '.join(valid_types)}")


class UpdateProviderRequest(BaseModel):
    """更新 Provider 请求"""

    name: Optional[str] = Field(None, min_length=1, max_length=100)
    description: Optional[str] = Field(None, max_length=1000)
    website: Optional[str] = Field(None, max_length=500)
    billing_type: Optional[str] = None
    monthly_quota_usd: Optional[float] = Field(None, ge=0)
    quota_reset_day: Optional[int] = Field(None, ge=1, le=365)
    quota_last_reset_at: Optional[datetime] = None
    quota_expires_at: Optional[datetime] = None
    provider_priority: Optional[int] = Field(None, ge=0, le=1000)
    is_active: Optional[bool] = None
    concurrent_limit: Optional[int] = Field(None, ge=0)
    # 请求配置（从 Endpoint 迁移）
    timeout: Optional[int] = Field(None, ge=1, le=600, description="请求超时（秒）")
    max_retries: Optional[int] = Field(None, ge=0, le=10, description="最大重试次数")
    proxy: Optional[ProxyConfig] = Field(None, description="代理配置")
    config: Optional[Dict[str, Any]] = None

    # 复用相同的验证器
    _sanitize_text = field_validator("name", "description")(
        CreateProviderRequest.sanitize_text.__func__
    )
    _validate_website = field_validator("website")(CreateProviderRequest.validate_website.__func__)
    _validate_billing_type = field_validator("billing_type")(
        CreateProviderRequest.validate_billing_type.__func__
    )


class CreateEndpointRequest(BaseModel):
    """创建 Endpoint 请求"""

    provider_id: str = Field(..., description="Provider ID")
    name: str = Field(..., min_length=1, max_length=100, description="Endpoint 名称")
    base_url: str = Field(..., min_length=1, max_length=500, description="API 基础 URL")
    api_format: str = Field(..., description="API 格式（CLAUDE 或 OPENAI）")
    custom_path: Optional[str] = Field(None, max_length=200, description="自定义路径")
    priority: Optional[int] = Field(100, ge=0, le=1000, description="优先级")
    is_active: Optional[bool] = Field(True, description="是否启用")
    concurrent_limit: Optional[int] = Field(None, ge=0, description="并发限制")
    config: Optional[Dict[str, Any]] = Field(None, description="其他配置")
    proxy: Optional[ProxyConfig] = Field(None, description="代理配置")

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
        try:
            APIFormat(v)
            return v
        except ValueError:
            valid_formats = [f.value for f in APIFormat]
            raise ValueError(f"无效的 API 格式，有效值为: {', '.join(valid_formats)}")

    @field_validator("custom_path")
    @classmethod
    def validate_custom_path(cls, v: Optional[str]) -> Optional[str]:
        """验证自定义路径"""
        if v is None:
            return v

        # 确保路径不包含危险字符
        if not re.match(r"^[/a-zA-Z0-9_-]+$", v):
            raise ValueError("路径只能包含字母、数字、斜杠、下划线和连字符")

        return v


class UpdateEndpointRequest(BaseModel):
    """更新 Endpoint 请求"""

    name: Optional[str] = Field(None, min_length=1, max_length=100)
    base_url: Optional[str] = Field(None, min_length=1, max_length=500)
    api_format: Optional[str] = None
    custom_path: Optional[str] = Field(None, max_length=200)
    priority: Optional[int] = Field(None, ge=0, le=1000)
    is_active: Optional[bool] = None
    concurrent_limit: Optional[int] = Field(None, ge=0)
    config: Optional[Dict[str, Any]] = None
    proxy: Optional[ProxyConfig] = Field(None, description="代理配置")

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
    api_key: str = Field(..., min_length=1, max_length=500, description="API Key")
    priority: Optional[int] = Field(100, ge=0, le=1000, description="优先级")
    is_active: Optional[bool] = Field(True, description="是否启用")
    rpm_limit: Optional[int] = Field(None, ge=0, description="RPM 限制（NULL=自适应）")
    notes: Optional[str] = Field(None, max_length=500, description="备注")

    @field_validator("api_key")
    @classmethod
    def validate_api_key(cls, v: str) -> str:
        """验证 API Key"""
        # 移除首尾空白
        v = v.strip()

        # 检查最小长度
        if len(v) < 10:
            raise ValueError("API Key 长度不能少于 10 个字符")

        # 检查危险字符（不应包含 SQL 注入字符）
        dangerous_chars = ["'", '"', ";", "--", "/*", "*/", "<", ">"]
        for char in dangerous_chars:
            if char in v:
                raise ValueError(f"API Key 包含非法字符: {char}")

        return v

    @field_validator("notes")
    @classmethod
    def sanitize_notes(cls, v: Optional[str]) -> Optional[str]:
        """清理备注"""
        if v is None:
            return v
        # 复用文本清理逻辑
        return CreateProviderRequest.sanitize_text(v)


class UpdateUserRequest(BaseModel):
    """更新用户请求"""

    username: Optional[str] = Field(None, min_length=1, max_length=50)
    email: Optional[str] = Field(None, max_length=100)
    password: Optional[str] = Field(None, min_length=6, max_length=128, description="新密码（留空保持不变）")
    quota_usd: Optional[float] = Field(None, ge=0)
    is_active: Optional[bool] = None
    role: Optional[str] = None
    allowed_providers: Optional[List[str]] = Field(None, description="允许使用的提供商 ID 列表")
    allowed_api_formats: Optional[List[str]] = Field(None, description="允许使用的 API 格式列表")
    allowed_models: Optional[List[str]] = Field(None, description="允许使用的模型名称列表")

    @field_validator("username")
    @classmethod
    def validate_username(cls, v: Optional[str]) -> Optional[str]:
        """验证用户名"""
        if v is None:
            return v

        if not re.match(r"^[a-zA-Z0-9_-]+$", v):
            raise ValueError("用户名只能包含字母、数字、下划线和连字符")

        return v

    @field_validator("email")
    @classmethod
    def validate_email(cls, v: Optional[str]) -> Optional[str]:
        """验证邮箱"""
        if v is None:
            return v

        # 简单的邮箱格式验证
        email_pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"
        if not re.match(email_pattern, v):
            raise ValueError("邮箱格式不正确")

        return v.lower()

    @field_validator("role")
    @classmethod
    def validate_role(cls, v: Optional[str]) -> Optional[str]:
        """验证角色"""
        if v is None:
            return v

        valid_roles = ["admin", "user"]
        if v not in valid_roles:
            raise ValueError(f"无效的角色，有效值为: {', '.join(valid_roles)}")

        return v
