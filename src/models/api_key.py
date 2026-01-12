"""
Provider API Key相关的API模型
"""

from datetime import datetime
from typing import Optional

from pydantic import BaseModel, ConfigDict, Field


class ProviderAPIKeyBase(BaseModel):
    """Provider API Key基础模型"""

    name: Optional[str] = Field(None, description="密钥名称/备注")
    api_key: str = Field(..., description="API密钥")
    rpm_limit: Optional[int] = Field(None, description="RPM限制（每分钟请求数），NULL=自适应模式")
    priority: int = Field(0, description="优先级（越高越优先使用）")
    is_active: bool = Field(True, description="是否启用")
    expires_at: Optional[datetime] = Field(None, description="过期时间")


class ProviderAPIKeyCreate(ProviderAPIKeyBase):
    """创建Provider API Key请求"""

    pass


class ProviderAPIKeyUpdate(BaseModel):
    """更新Provider API Key请求"""

    name: Optional[str] = None
    api_key: Optional[str] = None
    rpm_limit: Optional[int] = None
    priority: Optional[int] = None
    is_active: Optional[bool] = None
    expires_at: Optional[datetime] = None


class ProviderAPIKeyResponse(ProviderAPIKeyBase):
    """Provider API Key响应"""

    id: str
    provider_id: str
    request_count: Optional[int] = Field(0, description="请求次数")
    error_count: Optional[int] = Field(0, description="错误次数")
    last_used_at: Optional[datetime] = Field(None, description="最后使用时间")
    last_error_at: Optional[datetime] = Field(None, description="最后错误时间")
    last_error_msg: Optional[str] = Field(None, description="最后错误信息")
    created_at: datetime
    updated_at: datetime

    model_config = ConfigDict(from_attributes=True)


class ProviderAPIKeyStats(BaseModel):
    """Provider API Key统计信息"""

    id: str
    name: Optional[str]
    request_count: int
    error_count: int
    success_rate: float
    last_used_at: Optional[datetime]
    is_active: bool
    is_expired: bool
