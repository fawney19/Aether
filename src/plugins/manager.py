"""
插件管理器
统一管理和协调所有插件系统
"""

from __future__ import annotations

import asyncio
import importlib
import inspect
import threading
from pathlib import Path
from typing import Any

from src.core.logger import logger
from src.plugins.auth.base import AuthPlugin
from src.plugins.cache.base import CachePlugin

# 移除审计插件 - 审计功能现在是核心服务，不再作为插件
from src.plugins.common import BasePlugin, HealthStatus
from src.plugins.load_balancer.base import LoadBalancerStrategy
from src.plugins.monitor.base import MonitorPlugin
from src.plugins.notification.base import NotificationPlugin
from src.plugins.rate_limit.base import RateLimitStrategy
from src.plugins.token.base import TokenCounterPlugin


class PluginManager:
    """
    统一的插件管理器
    负责加载、配置和管理所有类型的插件
    """

    # 当前支持的 API 版本
    SUPPORTED_API_VERSION = "1.0"

    # 插件类型映射
    PLUGIN_TYPES = {
        "auth": AuthPlugin,
        "rate_limit": RateLimitStrategy,
        "cache": CachePlugin,
        "monitor": MonitorPlugin,
        "token": TokenCounterPlugin,
        "notification": NotificationPlugin,
        "load_balancer": LoadBalancerStrategy,
        # 移除 "audit" - 审计功能现在是核心服务
    }
    # 默认按需加载集合（未显式配置时使用）
    # notification 默认不加载，避免未配置插件（如 email）在启动时初始化失败并占用内存。
    DEFAULT_ENABLED_PLUGIN_MODULES: dict[str, tuple[str, ...]] = {
        "auth": ("api_key",),
        # 默认切到 token_bucket；全局 Redis 就绪时会自动切到分布式后端。
        "rate_limit": ("token_bucket",),
        "cache": ("memory",),
        "monitor": ("prometheus",),
        "token": ("claude",),
        "notification": (),
        "load_balancer": ("sticky_priority",),
    }
    # 部分插件“实例名”与“模块名”不同，按需加载时需要映射。
    MODULE_ALIASES: dict[str, dict[str, str]] = {
        "token": {
            "claude": "claude_counter",
            "tiktoken": "tiktoken_counter",
        }
    }

    def __init__(self, config: dict[str, Any] | None = None):
        """
        初始化插件管理器

        Args:
            config: 配置字典
        """
        self.config = config or {}
        self.plugins: dict[str, dict[str, Any]] = {
            "auth": {},
            "rate_limit": {},
            "cache": {},
            "monitor": {},
            "token": {},
            "notification": {},
            "load_balancer": {},
            # 移除 "audit" - 审计功能现在是核心服务
        }
        self.default_plugins: dict[str, str | None] = {
            "auth": None,
            "rate_limit": None,
            "cache": None,
            "monitor": None,
            "token": None,
            "notification": None,
            "load_balancer": "sticky_priority",  # 默认使用粘性优先级策略
            # 移除 "audit" - 审计功能现在是核心服务
        }
        # 跟踪因版本不兼容而跳过的插件
        self._incompatible_plugins: list[str] = []

        # 自动发现和加载插件
        self._auto_discover_plugins()

        # 应用配置
        self._apply_config()

    def _auto_discover_plugins(self) -> None:
        """自动发现和加载插件"""
        plugins_dir = Path(__file__).parent

        for plugin_type in self.PLUGIN_TYPES:
            type_dir = plugins_dir / plugin_type
            if not type_dir.exists():
                continue

            enabled_modules = self._resolve_enabled_plugin_modules(plugin_type, type_dir)
            for module_stem in enabled_modules:
                module_name = f"src.plugins.{plugin_type}.{module_stem}"
                try:
                    module = importlib.import_module(module_name)
                    self._load_plugin_from_module(module, plugin_type)
                except Exception as e:
                    logger.error(f"Failed to load plugin module {module_name}: {e}")

    def _resolve_enabled_plugin_modules(self, plugin_type: str, type_dir: Path) -> list[str]:
        """解析某类型当前应加载的插件模块名列表。"""
        available_modules = {
            file_path.stem
            for file_path in type_dir.glob("*.py")
            if not file_path.name.startswith("_") and file_path.name != "base.py"
        }
        if not available_modules:
            return []

        type_config_raw = self.config.get(plugin_type, {})
        type_config = type_config_raw if isinstance(type_config_raw, dict) else {}
        enabled_modules: set[str] = set()

        # 显式配置优先：仅加载配置中启用的插件 + default 指向插件
        if type_config:
            default_name = type_config.get("default")
            if isinstance(default_name, str) and default_name:
                enabled_modules.add(default_name)

            for plugin_name, plugin_cfg in type_config.items():
                if plugin_name == "default":
                    continue
                if isinstance(plugin_cfg, dict):
                    if plugin_cfg.get("enabled", True):
                        enabled_modules.add(plugin_name)
                elif plugin_cfg is not False:
                    enabled_modules.add(plugin_name)
        else:
            # 无显式配置时使用默认按需集合
            enabled_modules.update(self.DEFAULT_ENABLED_PLUGIN_MODULES.get(plugin_type, ()))

        aliases = self.MODULE_ALIASES.get(plugin_type, {})
        resolved_modules = set()
        for module_name in enabled_modules:
            resolved_modules.add(aliases.get(module_name, module_name))

        missing_modules = resolved_modules - available_modules
        if missing_modules:
            logger.warning(
                "Plugin modules configured but not found for type {}: {}",
                plugin_type,
                sorted(missing_modules),
            )

        return sorted(resolved_modules & available_modules)

    def _is_api_version_compatible(self, plugin_api_version: str) -> bool:
        """
        检查插件 API 版本是否兼容

        采用语义化版本的主版本号兼容策略：
        - 主版本号相同则兼容
        - 例如: 支持版本 "1.0"，插件版本 "1.0", "1.1", "1.2" 都兼容

        Args:
            plugin_api_version: 插件声明的 API 版本

        Returns:
            是否兼容
        """
        try:
            supported_major = self.SUPPORTED_API_VERSION.split(".")[0]
            plugin_major = plugin_api_version.split(".")[0]
            return supported_major == plugin_major
        except (ValueError, IndexError):
            # 解析失败，假设兼容
            return True

    def _load_plugin_from_module(self, module: Any, plugin_type: str) -> None:
        """从模块加载插件类"""
        base_class = self.PLUGIN_TYPES[plugin_type]

        for name, obj in inspect.getmembers(module):
            if inspect.isclass(obj) and issubclass(obj, base_class) and obj != base_class:
                # 实例化插件
                try:
                    plugin_instance = obj()

                    # 检查 API 版本兼容性
                    plugin_api_version = getattr(plugin_instance.metadata, "api_version", "1.0")
                    if not self._is_api_version_compatible(plugin_api_version):
                        logger.warning(
                            f"Plugin {plugin_instance.name} has incompatible API version "
                            f"{plugin_api_version} (supported: {self.SUPPORTED_API_VERSION}), "
                            f"plugin will be disabled"
                        )
                        plugin_instance.enabled = False
                        self._incompatible_plugins.append(plugin_instance.name)

                    self.register_plugin(plugin_type, plugin_instance)
                    logger.info(f"Loaded {plugin_type} plugin: {plugin_instance.name}")
                except Exception as e:
                    logger.error(f"Failed to instantiate plugin {name}: {e}")

    def _apply_config(self) -> None:
        """应用配置到插件"""
        for plugin_type, plugins in self.plugins.items():
            type_config = self.config.get(plugin_type, {})

            # 设置默认插件
            if "default" in type_config:
                self.default_plugins[plugin_type] = type_config["default"]

            # 配置各个插件
            for plugin_name, plugin in plugins.items():
                plugin_config = type_config.get(plugin_name, {})
                if plugin_config:
                    plugin.configure(plugin_config)

    def register_plugin(self, plugin_type: str, plugin: Any, set_as_default: bool = False) -> None:
        """
        注册插件

        Args:
            plugin_type: 插件类型
            plugin: 插件实例
            set_as_default: 是否设为默认
        """
        if plugin_type not in self.plugins:
            raise ValueError(f"Unknown plugin type: {plugin_type}")

        # 验证插件类型
        base_class = self.PLUGIN_TYPES[plugin_type]
        if not isinstance(plugin, base_class):
            raise TypeError(
                f"Plugin must be instance of {base_class.__name__}, " f"got {type(plugin).__name__}"
            )

        # 注册插件
        self.plugins[plugin_type][plugin.name] = plugin

        # 设为默认
        if set_as_default or not self.default_plugins[plugin_type]:
            self.default_plugins[plugin_type] = plugin.name

        logger.debug(f"Registered {plugin_type} plugin: {plugin.name}")

    def unregister_plugin(self, plugin_type: str, plugin_name: str) -> Any:
        """
        注销插件

        Args:
            plugin_type: 插件类型
            plugin_name: 插件名称
        """
        if plugin_type in self.plugins:
            if plugin_name in self.plugins[plugin_type]:
                del self.plugins[plugin_type][plugin_name]

                # 如果是默认插件，清除默认设置
                if self.default_plugins[plugin_type] == plugin_name:
                    self.default_plugins[plugin_type] = None

                logger.debug(f"Unregistered {plugin_type} plugin: {plugin_name}")

    def get_plugin(self, plugin_type: str, plugin_name: str | None = None) -> Any | None:
        """
        获取插件实例

        Args:
            plugin_type: 插件类型
            plugin_name: 插件名称，不指定则返回默认插件

        Returns:
            插件实例，如果不存在返回None
        """
        if plugin_type not in self.plugins:
            return None

        if plugin_name:
            return self.plugins[plugin_type].get(plugin_name)

        # 返回默认插件
        default_name = self.default_plugins[plugin_type]
        if default_name:
            return self.plugins[plugin_type].get(default_name)

        # 如果没有默认插件，返回第一个可用的
        if self.plugins[plugin_type]:
            return next(iter(self.plugins[plugin_type].values()))

        return None

    def get_plugins_by_type(self, plugin_type: str) -> list[Any]:
        """
        获取某个类型的所有插件

        Args:
            plugin_type: 插件类型

        Returns:
            插件列表
        """
        if plugin_type not in self.plugins:
            return []

        return list(self.plugins[plugin_type].values())

    def get_enabled_plugins(self, plugin_type: str) -> list[Any]:
        """
        获取某个类型的所有启用的插件

        Args:
            plugin_type: 插件类型

        Returns:
            启用的插件列表
        """
        plugins = self.get_plugins_by_type(plugin_type)
        return [p for p in plugins if getattr(p, "enabled", True)]

    async def execute_plugin_chain(
        self, plugin_type: str, method_name: str, *args: Any, **kwargs: Any
    ) -> Any:
        """
        执行插件链（按优先级）

        Args:
            plugin_type: 插件类型
            method_name: 要调用的方法名
            *args: 位置参数
            **kwargs: 关键字参数

        Returns:
            第一个成功的结果
        """
        plugins = self.get_enabled_plugins(plugin_type)

        # 按优先级排序（如果有priority属性）
        plugins.sort(key=lambda p: getattr(p, "priority", 0), reverse=True)

        for plugin in plugins:
            if hasattr(plugin, method_name):
                method = getattr(plugin, method_name)
                try:
                    if asyncio.iscoroutinefunction(method):
                        result = await method(*args, **kwargs)
                    else:
                        result = method(*args, **kwargs)

                    if result is not None:
                        return result
                except Exception as e:
                    logger.error(f"Plugin {plugin.name} failed in {method_name}: {e}")

        return None

    def get_stats(self) -> dict[str, Any]:
        """
        获取插件管理器统计信息

        Returns:
            统计信息字典
        """
        stats = {
            "supported_api_version": self.SUPPORTED_API_VERSION,
            "plugin_counts": {},
            "enabled_counts": {},
            "default_plugins": self.default_plugins,
            "plugin_details": {},
            "incompatible_plugins": self._incompatible_plugins,
        }

        for plugin_type in self.PLUGIN_TYPES:
            all_plugins = self.get_plugins_by_type(plugin_type)
            enabled_plugins = self.get_enabled_plugins(plugin_type)

            stats["plugin_counts"][plugin_type] = len(all_plugins)
            stats["enabled_counts"][plugin_type] = len(enabled_plugins)

            # 详细信息
            stats["plugin_details"][plugin_type] = [
                {
                    "name": p.name,
                    "enabled": getattr(p, "enabled", True),
                    "priority": getattr(p, "priority", 0),
                    "class": type(p).__name__,
                    "api_version": getattr(p.metadata, "api_version", "unknown"),
                    "version": getattr(p.metadata, "version", "unknown"),
                }
                for p in all_plugins
            ]

        return stats

    async def initialize_all(self) -> dict[str, bool]:
        """
        初始化所有插件

        初始化失败的插件会被自动禁用，防止后续使用未正确初始化的插件。

        Returns:
            初始化结果字典 {plugin_name: success}
        """
        results = {}

        # 获取所有插件并按依赖顺序排序
        all_plugins = []
        for plugin_type in self.PLUGIN_TYPES:
            all_plugins.extend(self.get_plugins_by_type(plugin_type))

        # 拓扑排序处理依赖
        sorted_plugins = self._sort_plugins_by_dependencies(all_plugins)

        # 按顺序初始化插件
        for plugin in sorted_plugins:
            try:
                # 检查插件是否有 initialize 方法
                if not hasattr(plugin, "initialize"):
                    # 如果没有 initialize 方法，假设插件已经初始化完成
                    logger.debug(f"Plugin {plugin.name} has no initialize() method, skipping")
                    results[f"{plugin.name}"] = True
                    continue

                success = await plugin.initialize()
                results[f"{plugin.name}"] = success
                if success:
                    logger.info(f"Successfully initialized plugin: {plugin.name}")
                else:
                    # 初始化失败，禁用插件
                    plugin.enabled = False
                    logger.error(
                        f"Failed to initialize plugin: {plugin.name}, plugin has been disabled"
                    )
            except Exception as e:
                results[f"{plugin.name}"] = False
                # 初始化异常，禁用插件
                plugin.enabled = False
                logger.error(
                    f"Error initializing plugin {plugin.name}: {e}, plugin has been disabled"
                )

        return results

    async def shutdown_all(self) -> None:
        """
        关闭所有插件
        """
        # 获取所有插件并按依赖顺序反向排序（先关闭依赖者）
        all_plugins = []
        for plugin_type in self.PLUGIN_TYPES:
            all_plugins.extend(self.get_plugins_by_type(plugin_type))

        sorted_plugins = self._sort_plugins_by_dependencies(all_plugins)
        sorted_plugins.reverse()  # 反向关闭

        # 并发关闭插件
        shutdown_tasks = []
        for plugin in sorted_plugins:
            # 只关闭有 shutdown 方法的插件
            if hasattr(plugin, "shutdown"):
                shutdown_tasks.append(plugin.shutdown())

        if shutdown_tasks:
            try:
                await asyncio.gather(*shutdown_tasks, return_exceptions=True)
                logger.info("All plugins shut down")
            except Exception as e:
                logger.error(f"Error during plugin shutdown: {e}")

    def _sort_plugins_by_dependencies(self, plugins: list[BasePlugin]) -> list[BasePlugin]:
        """
        按依赖关系对插件进行拓扑排序

        Args:
            plugins: 插件列表

        Returns:
            排序后的插件列表

        Note:
            存在循环依赖的插件会被自动禁用
        """
        # 创建插件名称到插件对象的映射
        plugin_map = {plugin.name: plugin for plugin in plugins}

        # 计算每个插件的入度（未满足的依赖数量）
        in_degree = {plugin.name: 0 for plugin in plugins}

        # 构建依赖图
        for plugin in plugins:
            for dep in plugin.metadata.dependencies:
                if dep in in_degree:
                    in_degree[plugin.name] += 1

        # 拓扑排序
        queue = [name for name, degree in in_degree.items() if degree == 0]
        result = []

        while queue:
            current = queue.pop(0)
            result.append(plugin_map[current])

            # 减少依赖当前插件的其他插件的入度
            current_plugin = plugin_map[current]
            for plugin in plugins:
                if current in plugin.metadata.dependencies:
                    in_degree[plugin.name] -= 1
                    if in_degree[plugin.name] == 0:
                        queue.append(plugin.name)

        # 检查是否存在循环依赖
        if len(result) != len(plugins):
            remaining = [p for p in plugins if p not in result]
            circular_names = [p.name for p in remaining]
            logger.error(
                f"Circular dependency detected among plugins: {circular_names}. "
                f"These plugins will be disabled."
            )
            # 禁用存在循环依赖的插件，而不是继续加载
            for plugin in remaining:
                plugin.enabled = False
                logger.warning(f"Plugin {plugin.name} has been disabled due to circular dependency")
            # 不再将循环依赖的插件添加到结果中

        return result

    async def health_check_all(self) -> dict[str, HealthStatus]:
        """
        检查所有插件的健康状态

        Returns:
            健康状态字典 {plugin_name: status}
        """
        results = {}

        # 获取所有插件
        all_plugins = []
        for plugin_type in self.PLUGIN_TYPES:
            all_plugins.extend(self.get_plugins_by_type(plugin_type))

        # 并发检查健康状态
        health_tasks = []
        for plugin in all_plugins:
            health_tasks.append(plugin.health_check())

        if health_tasks:
            health_results = await asyncio.gather(*health_tasks, return_exceptions=True)

            for plugin, result in zip(all_plugins, health_results):
                if isinstance(result, Exception):
                    results[plugin.name] = HealthStatus.UNHEALTHY
                else:
                    results[plugin.name] = result

        return results

    def validate_plugin_dependencies(self) -> dict[str, list[str]]:
        """
        验证所有插件的依赖关系

        Returns:
            验证结果字典 {plugin_name: [missing_dependencies]}
        """
        results = {}

        # 获取所有可用插件名称
        available_plugins = {}
        for plugin_type in self.PLUGIN_TYPES:
            available_plugins[plugin_type] = [p.name for p in self.get_plugins_by_type(plugin_type)]

        # 检查每个插件的依赖
        for plugin_type in self.PLUGIN_TYPES:
            for plugin in self.get_plugins_by_type(plugin_type):
                missing_deps = plugin.validate_dependencies(available_plugins)
                if missing_deps:
                    results[plugin.name] = missing_deps

        return results

    def reload_plugin_config(
        self, plugin_type: str, plugin_name: str, new_config: dict[str, Any]
    ) -> bool:
        """
        重新加载插件配置

        Args:
            plugin_type: 插件类型
            plugin_name: 插件名称
            new_config: 新配置

        Returns:
            是否成功重新加载
        """
        plugin = self.get_plugin(plugin_type, plugin_name)
        if not plugin:
            return False

        try:
            plugin.configure(new_config)
            logger.info(f"Reloaded config for plugin {plugin_name}: {new_config}")
            return True
        except Exception as e:
            logger.error(f"Failed to reload config for plugin {plugin_name}: {e}")
            return False


# 全局插件管理器实例
_plugin_manager: PluginManager | None = None
_plugin_manager_lock = threading.Lock()


def get_plugin_manager(config: dict[str, Any] | None = None) -> PluginManager:
    """
    获取全局插件管理器实例（线程安全）

    Args:
        config: 配置字典

    Returns:
        插件管理器实例
    """
    global _plugin_manager

    if _plugin_manager is None:
        with _plugin_manager_lock:
            # 双重检查锁定模式
            if _plugin_manager is None:
                _plugin_manager = PluginManager(config)

    return _plugin_manager


def reset_plugin_manager() -> None:
    """重置插件管理器（用于测试）"""
    global _plugin_manager
    with _plugin_manager_lock:
        _plugin_manager = None
