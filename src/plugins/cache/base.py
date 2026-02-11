"""
缓存插件基类
定义缓存插件的接口
"""

from __future__ import annotations

import hashlib
from abc import abstractmethod
from typing import Any

from src.utils import json_helper as json

from ..common import BasePlugin


class CachePlugin(BasePlugin):
    """
    缓存插件基类
    所有缓存插件必须继承此类并实现相关方法
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
        初始化缓存插件

        Args:
            name: 插件名称
            priority: 优先级
            version: 插件版本
            author: 插件作者
            description: 插件描述
            api_version: API版本
            dependencies: 依赖列表
            provides: 提供服务列表
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
        self.default_ttl = self.config.get("default_ttl", 3600)  # 默认1小时
        self.max_size = self.config.get("max_size", 1000)  # 最大缓存项数

    @abstractmethod
    async def get(self, key: str) -> Any | None:
        """
        获取缓存值

        Args:
            key: 缓存键

        Returns:
            缓存值，如果不存在返回None
        """
        pass

    @abstractmethod
    async def set(self, key: str, value: Any, ttl: int | None = None) -> bool:
        """
        设置缓存值

        Args:
            key: 缓存键
            value: 缓存值
            ttl: 过期时间（秒），None使用默认值

        Returns:
            是否成功设置
        """
        pass

    @abstractmethod
    async def delete(self, key: str) -> bool:
        """
        删除缓存项

        Args:
            key: 缓存键

        Returns:
            是否成功删除
        """
        pass

    @abstractmethod
    async def exists(self, key: str) -> bool:
        """
        检查缓存项是否存在

        Args:
            key: 缓存键

        Returns:
            是否存在
        """
        pass

    @abstractmethod
    async def clear(self) -> bool:
        """
        清空所有缓存

        Returns:
            是否成功清空
        """
        pass

    @abstractmethod
    async def get_many(self, keys: list[str]) -> dict[str, Any]:
        """
        批量获取缓存值

        Args:
            keys: 缓存键列表

        Returns:
            键值对字典
        """
        pass

    @abstractmethod
    async def set_many(self, items: dict[str, Any], ttl: int | None = None) -> bool:
        """
        批量设置缓存值

        Args:
            items: 键值对字典
            ttl: 过期时间（秒）

        Returns:
            是否成功设置
        """
        pass

    @abstractmethod
    async def get_stats(self) -> dict[str, Any]:
        """
        获取缓存统计信息

        Returns:
            统计信息字典
        """
        pass

    def generate_key(self, *args: Any, **kwargs: Any) -> str:
        """
        生成缓存键

        Args:
            *args: 位置参数
            **kwargs: 关键字参数

        Returns:
            缓存键字符串
        """
        # 创建一个稳定的键
        key_parts = [str(arg) for arg in args]
        key_parts.extend([f"{k}:{v}" for k, v in sorted(kwargs.items())])
        key_string = "|".join(key_parts)

        # 如果键太长，使用哈希
        if len(key_string) > 250:
            hash_obj = hashlib.md5(key_string.encode())
            return f"{self.name}:{hash_obj.hexdigest()}"

        return f"{self.name}:{key_string}"

    def serialize(self, value: Any) -> str:
        """
        序列化值

        Args:
            value: 要序列化的值

        Returns:
            序列化后的字符串
        """
        return json.dumps(value, default=str)

    def deserialize(self, value: str) -> Any:
        """
        反序列化值

        Args:
            value: 序列化的字符串

        Returns:
            反序列化后的值
        """
        return json.loads(value)

    def configure(self, config: dict[str, Any]) -> Any:
        """
        配置插件

        Args:
            config: 配置字典
        """
        super().configure(config)
        self.default_ttl = config.get("default_ttl", self.default_ttl)
        self.max_size = config.get("max_size", self.max_size)
