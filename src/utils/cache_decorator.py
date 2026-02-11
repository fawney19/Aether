"""缓存装饰器工具"""

import functools
import hashlib
from collections.abc import Callable
from typing import Any

from src.clients.redis_client import get_redis_client_sync
from src.core.logger import logger
from src.utils import json_helper as json


def _is_adapter_instance(obj: Any) -> bool:
    """检查对象是否是 ApiAdapter 的实例（延迟导入避免循环依赖）"""
    try:
        from src.api.base.adapter import ApiAdapter

        return isinstance(obj, ApiAdapter)
    except ImportError:
        return False


def _is_api_context(obj: Any) -> bool:
    """检查对象是否是 ApiRequestContext（通过 duck typing）"""
    return hasattr(obj, "user") and hasattr(obj, "db")


def _hash_vary(vary: dict[str, Any]) -> str:
    """Build a short stable hash for cache key variations."""
    try:
        raw = json.dumps(vary, sort_keys=True, ensure_ascii=False, default=str)
    except Exception:
        raw = str(vary)
    return hashlib.sha1(raw.encode("utf-8")).hexdigest()[:16]


def cache_result(
    key_prefix: str,
    ttl: int = 60,
    user_specific: bool = True,
    *,
    vary_by: list[str] | None = None,
) -> Callable:
    """
    缓存函数结果的装饰器

    Args:
        key_prefix: 缓存键前缀
        ttl: 缓存过期时间（秒）
        user_specific: 是否针对用户缓存（从 context.user.id 获取）
    """

    def decorator(func: Callable) -> Callable:
        @functools.wraps(func)
        async def wrapper(*args: Any, **kwargs: Any) -> Any:
            redis_client = get_redis_client_sync()

            # 如果 Redis 不可用，直接执行原函数
            if redis_client is None:
                return await func(*args, **kwargs)

            # 构建缓存键
            try:
                # 从 args 中获取 context
                # 对于实例方法，args[0] 是 self，args[1] 才是 context
                # 对于普通函数，args[0] 是 context
                context = None
                adapter_self = None

                if len(args) >= 2 and _is_adapter_instance(args[0]) and _is_api_context(args[1]):
                    # 实例方法: handle(self, context)
                    adapter_self = args[0]
                    context = args[1]
                elif len(args) >= 1 and _is_api_context(args[0]):
                    # 普通函数或 context 在第一个位置
                    context = args[0]
                elif len(args) >= 1 and _is_adapter_instance(args[0]):
                    # 实例方法但 context 可能在 kwargs 中
                    adapter_self = args[0]
                    context = kwargs.get("context")

                if user_specific and context and hasattr(context, "user") and context.user:
                    cache_key = f"{key_prefix}:user:{context.user.id}"
                else:
                    cache_key = f"{key_prefix}:global"

                # If there are extra parameters, include them into the key.
                # - When vary_by is provided: hash the selected attributes to keep key short.
                # - Otherwise keep backward-compatible "days/limit" suffix behavior.
                if adapter_self and hasattr(adapter_self, "__dict__"):
                    if vary_by:
                        vary: dict[str, Any] = {}
                        for attr_name in vary_by:
                            if hasattr(adapter_self, attr_name):
                                vary[attr_name] = getattr(adapter_self, attr_name)
                        if vary:
                            cache_key += f":v:{_hash_vary(vary)}"
                    else:
                        for attr_name in ["days", "limit"]:
                            if hasattr(adapter_self, attr_name):
                                attr_value = getattr(adapter_self, attr_name)
                                cache_key += f":{attr_name}:{attr_value}"

                # 尝试从缓存获取
                cached = await redis_client.get(cache_key)
                if cached:
                    try:
                        result = json.loads(cached)
                        logger.trace(f"缓存命中: {cache_key}")
                        return result
                    except json.JSONDecodeError as e:
                        logger.warning(f"缓存解析失败，删除损坏缓存: {cache_key}, 错误: {e}")
                        try:
                            await redis_client.delete(cache_key)
                        except Exception:
                            pass

                # 执行原函数
                result = await func(*args, **kwargs)

                # 保存到缓存
                try:
                    await redis_client.setex(
                        cache_key, ttl, json.dumps(result, ensure_ascii=False, default=str)
                    )
                    logger.trace(f"缓存已保存: {cache_key}, TTL: {ttl}s")
                except Exception as e:
                    logger.warning(f"保存缓存失败: {e}")

                return result

            except Exception as e:
                logger.warning(f"缓存处理出错: {e}, 直接执行原函数")
                return await func(*args, **kwargs)

        return wrapper

    return decorator
