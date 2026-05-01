"""
数据库事务管理工具
提供事务装饰器和事务上下文管理器
支持同步和异步函数
"""

from __future__ import annotations

import functools
import inspect
from collections.abc import Callable, Generator
from contextlib import contextmanager
from typing import Any

from sqlalchemy.exc import DatabaseError, IntegrityError, OperationalError
from sqlalchemy.orm import Session

from src.core.logger import logger


class TransactionError(Exception):
    """事务处理异常"""

    pass


def _find_db_session(args: Any, kwargs: Any) -> Session | None:
    """从参数中查找数据库会话"""
    # 从位置参数中查找Session
    for arg in args:
        if isinstance(arg, Session):
            return arg

    # 从关键字参数中查找Session
    for value in kwargs.values():
        if isinstance(value, Session):
            return value

    return None


def transactional(commit: bool = True, rollback_on_error: bool = True) -> Any:
    """
    事务装饰器，支持同步和异步函数

    Args:
        commit: 是否在成功时自动提交，默认True
        rollback_on_error: 是否在错误时自动回滚，默认True

    Usage:
        @transactional()
        def create_user_with_api_key(db: Session, ...):
            # 同步方法会在事务中执行
            pass

        @transactional()
        async def create_user_async(db: Session, ...):
            # 异步方法也会在事务中执行
            pass
    """

    def decorator(func: Callable) -> Callable:
        # 检查是否是异步函数
        if inspect.iscoroutinefunction(func):

            @functools.wraps(func)
            async def async_wrapper(*args: Any, **kwargs: Any) -> Any:
                db_session = _find_db_session(args, kwargs)

                if not db_session:
                    raise TransactionError(
                        f"No SQLAlchemy Session found in arguments for {func.__name__}"
                    )

                # 检查是否已经在事务中
                if db_session.in_transaction():
                    return await func(*args, **kwargs)

                transaction_id = f"{func.__module__}.{func.__name__}"
                logger.debug(f"开始异步事务: {transaction_id}")

                try:
                    result = await func(*args, **kwargs)

                    if commit:
                        db_session.commit()
                        logger.debug(f"异步事务提交成功: {transaction_id}")

                    return result

                except Exception as e:
                    if rollback_on_error:
                        try:
                            db_session.rollback()
                        except Exception:
                            pass
                        logger.error(
                            f"异步事务回滚: {transaction_id} - {type(e).__name__}: {str(e)}"
                        )
                    else:
                        logger.error(
                            f"异步事务异常（未回滚）: {transaction_id} - {type(e).__name__}: {str(e)}"
                        )
                    raise

            return async_wrapper
        else:

            @functools.wraps(func)
            def sync_wrapper(*args: Any, **kwargs: Any) -> Any:
                db_session = _find_db_session(args, kwargs)

                if not db_session:
                    raise TransactionError(
                        f"No SQLAlchemy Session found in arguments for {func.__name__}"
                    )

                # 检查是否已经在事务中
                if db_session.in_transaction():
                    return func(*args, **kwargs)

                transaction_id = f"{func.__module__}.{func.__name__}"
                logger.debug(f"开始事务: {transaction_id}")

                try:
                    result = func(*args, **kwargs)

                    if commit:
                        db_session.commit()
                        logger.debug(f"事务提交成功: {transaction_id}")

                    return result

                except Exception as e:
                    if rollback_on_error:
                        try:
                            db_session.rollback()
                        except Exception:
                            pass
                        logger.error(f"事务回滚: {transaction_id} - {type(e).__name__}: {str(e)}")
                    else:
                        logger.error(
                            f"事务异常（未回滚）: {transaction_id} - {type(e).__name__}: {str(e)}"
                        )
                    raise

            return sync_wrapper

    return decorator


@contextmanager
def transaction_scope(
    db: Session,
    commit_on_success: bool = True,
    rollback_on_error: bool = True,
    operation_name: str | None = None,
) -> Generator[Session]:
    """
    事务上下文管理器

    Args:
        db: 数据库会话
        commit_on_success: 成功时是否自动提交
        rollback_on_error: 失败时是否自动回滚
        operation_name: 操作名称，用于日志

    Usage:
        with transaction_scope(db, operation_name="create_user") as tx:
            user = User(...)
            tx.add(user)
            # 自动提交或回滚
    """
    operation_name = operation_name or "database_operation"

    # 检查是否已经在事务中
    if db.in_transaction():
        # 已经在事务中，直接返回session
        logger.debug(f"使用现有事务: {operation_name}")
        yield db
        return

    logger.debug(f"开始事务范围: {operation_name}")

    try:
        yield db

        if commit_on_success:
            db.commit()
            logger.debug(f"事务范围提交成功: {operation_name}")

    except Exception as e:
        if rollback_on_error:
            db.rollback()
            logger.error(f"事务范围回滚: {operation_name} - {type(e).__name__}: {str(e)}")
        raise


def retry_on_database_error(max_retries: int = 3, delay: float = 0.1) -> Any:
    """
    数据库错误重试装饰器

    只对瞬时错误（OperationalError：死锁、连接抖动、序列化失败等）进行重试。
    对 IntegrityError（外键/唯一/非空约束违反）以及其他 DatabaseError 子类
    （DataError/ProgrammingError 等）直接抛出，不重试——这些都是确定性失败，
    重试只会浪费时间和数据库连接。

    Args:
        max_retries: 最大重试次数
        delay: 重试延迟（秒）
    """

    def decorator(func: Callable) -> Callable:
        @functools.wraps(func)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            import random
            import time

            db_session = _find_db_session(args, kwargs)

            for attempt in range(max_retries):
                try:
                    return func(*args, **kwargs)

                except IntegrityError:
                    # 确定性错误：约束违反，重试无意义，直接回滚并抛出
                    if db_session is not None:
                        try:
                            db_session.rollback()
                        except Exception as rollback_error:
                            logger.warning(
                                "数据库约束违反后回滚 Session 失败: {} - {}",
                                type(rollback_error).__name__,
                                str(rollback_error),
                            )
                    raise

                except OperationalError as e:
                    # 瞬时错误：允许重试
                    if db_session is not None:
                        try:
                            db_session.rollback()
                        except Exception as rollback_error:
                            logger.warning(
                                "数据库操作失败后回滚 Session 失败: {} - {}",
                                type(rollback_error).__name__,
                                str(rollback_error),
                            )

                    if attempt < max_retries - 1:
                        # 随机化延迟，避免多个请求同时重试
                        actual_delay = delay * (2**attempt) + random.uniform(0, 0.1)
                        logger.warning(
                            f"数据库操作失败，{actual_delay:.2f}秒后重试 (尝试 {attempt + 1}/{max_retries}): {str(e)}"
                        )
                        time.sleep(actual_delay)
                        continue
                    else:
                        logger.error(
                            f"数据库操作最终失败，已达最大重试次数({max_retries}): {func.__name__} - {str(e)}"
                        )
                        raise

                except DatabaseError:
                    # 其他 DatabaseError 子类（DataError / ProgrammingError 等）
                    # 均为确定性错误，不重试
                    if db_session is not None:
                        try:
                            db_session.rollback()
                        except Exception as rollback_error:
                            logger.warning(
                                "数据库错误后回滚 Session 失败: {} - {}",
                                type(rollback_error).__name__,
                                str(rollback_error),
                            )
                    raise

        return wrapper

    return decorator
