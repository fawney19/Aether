"""
认证插件基类
定义认证插件的接口和认证上下文
"""

from __future__ import annotations

from abc import abstractmethod
from dataclasses import dataclass
from typing import Any

from fastapi import Request
from sqlalchemy.orm import Session

from ..common import BasePlugin


@dataclass
class AuthContext:
    """
    认证上下文
    包含认证后的用户信息和权限
    """

    user_id: int
    user_name: str
    api_key_id: int | None = None
    api_key_name: str | None = None
    permissions: dict[str, bool] = None
    billing_info: dict[str, Any] = None
    metadata: dict[str, Any] = None

    def __post_init__(self) -> None:
        if self.permissions is None:
            self.permissions = {}
        if self.metadata is None:
            self.metadata = {}


class AuthPlugin(BasePlugin):
    """
    认证插件基类
    所有认证插件必须继承此类并实现authenticate方法
    """

    def __init__(
        self,
        name: str,
        priority: int = 0,
        version: str = "1.0.0",
        author: str = "Unknown",
        description: str = "",
        api_version: str = "1.0",
        dependencies: list[str] | None = None,
        provides: list[str] | None = None,
        config: dict[str, Any] | None = None,
    ):
        """
        初始化认证插件

        Args:
            name: 插件名称
            priority: 优先级（数字越大优先级越高）
            version: 插件版本
            author: 插件作者
            description: 插件描述
            api_version: API版本
            dependencies: 依赖的其他插件
            provides: 提供的服务
            config: 配置字典
        """
        super().__init__(
            name=name,
            priority=priority,
            version=version,
            author=author,
            description=description,
            api_version=api_version,
            dependencies=dependencies,
            provides=provides,
            config=config,
        )

    @abstractmethod
    async def authenticate(self, request: Request, db: Session) -> AuthContext | None:
        """
        执行认证

        Args:
            request: FastAPI请求对象
            db: 数据库会话

        Returns:
            成功返回AuthContext，失败返回None
        """
        pass

    @abstractmethod
    def get_credentials(self, request: Request) -> str | None:
        """
        从请求中提取认证凭据

        Args:
            request: FastAPI请求对象

        Returns:
            认证凭据字符串，如果没有找到返回None
        """
        pass

    def is_applicable(self, request: Request) -> bool:
        """
        检查此插件是否适用于当前请求

        Args:
            request: FastAPI请求对象

        Returns:
            如果插件适用返回True
        """
        # 默认情况下，如果能提取到凭据就适用
        return self.get_credentials(request) is not None
