"""
异步工具函数

提供在异步上下文中安全执行同步函数的工具，避免阻塞事件循环。
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable, Coroutine
from functools import partial, wraps
from typing import Any, TypeVar

T = TypeVar("T")

# 全局 task 引用集合，防止 fire-and-forget task 被 GC 回收
_background_tasks: set[asyncio.Task[Any]] = set()


def safe_create_task(coro: Coroutine[Any, Any, Any]) -> asyncio.Task[Any] | None:
    """创建后台 task 并持有引用，防止被 GC 回收。

    用于 fire-and-forget 场景（如缓存失效、异步指标上报等），
    替代裸 ``asyncio.create_task()`` 调用。

    Returns:
        创建的 Task 对象；若没有运行中的事件循环则返回 None。
    """
    try:
        loop = asyncio.get_running_loop()
    except RuntimeError:
        return None
    task = loop.create_task(coro)
    _background_tasks.add(task)
    task.add_done_callback(_background_tasks.discard)
    return task


async def run_in_executor(func: Callable[..., T], *args: Any, **kwargs: Any) -> T:
    """
    在线程池中运行同步函数，避免阻塞事件循环。

    用法:
        result = await run_in_executor(some_sync_function, arg1, arg2)
    """
    loop = asyncio.get_running_loop()
    bound = partial(func, *args, **kwargs)
    return await loop.run_in_executor(None, bound)


def async_wrap_sync(func: Callable[..., T]) -> Callable[..., Coroutine[Any, Any, T]]:
    """
    装饰器：将同步函数包装成异步函数（在线程池中执行）。

    用法:
        @async_wrap_sync
        def do_sync(...): ...

        result = await do_sync(...)
    """

    @wraps(func)
    async def wrapper(*args: Any, **kwargs: Any) -> T:
        return await run_in_executor(func, *args, **kwargs)

    return wrapper
