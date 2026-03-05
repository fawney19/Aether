"""
缓存预热服务

在应用启动时预热关键缓存，避免用户首次访问时的长时间等待。

预热的缓存包括：
- 管理员仪表盘统计数据
- 管理员热力图数据
- 每日统计数据
"""

import asyncio
import time
from dataclasses import dataclass, field
from typing import Any

from sqlalchemy.orm import Session

from src.core.enums import UserRole
from src.core.logger import logger
from src.database import create_session


@dataclass
class WarmupContext:
    """缓存预热专用的简化 Context"""

    db: Session
    user: Any  # User model
    audit_metadata: dict[str, Any] = field(default_factory=dict)

    def add_audit_metadata(self, **kwargs: Any) -> None:
        """兼容 ApiRequestContext 接口"""
        self.audit_metadata.update(kwargs)


class CacheWarmupService:
    """缓存预热服务"""

    @classmethod
    async def warmup_all(cls, delay_seconds: float = 3.0) -> None:
        """
        预热所有关键缓存

        Args:
            delay_seconds: 启动后延迟执行的秒数，确保系统完全就绪
        """
        await asyncio.sleep(delay_seconds)

        logger.info("开始预热关键缓存...")
        start_time = time.time()

        results = await asyncio.gather(
            cls._warmup_admin_dashboard_stats(),
            cls._warmup_admin_heatmap(),
            cls._warmup_daily_stats(),
            return_exceptions=True,
        )

        success_count = sum(1 for r in results if r is True)
        error_count = sum(1 for r in results if isinstance(r, Exception))
        elapsed = time.time() - start_time

        if error_count > 0:
            logger.warning(
                f"缓存预热完成: {success_count}/3 成功, {error_count} 失败, 耗时 {elapsed:.2f}s"
            )
        else:
            logger.info(f"缓存预热完成: {success_count}/3 成功, 耗时 {elapsed:.2f}s")

    @classmethod
    async def _warmup_admin_dashboard_stats(cls) -> bool:
        """预热管理员仪表盘统计缓存"""
        db = None
        try:
            from src.api.dashboard.routes import (  # TODO(arch): 提取 dashboard 统计计算到 services 层
                AdminDashboardStatsAdapter,
            )
            from src.models.database import User as DBUser

            db = create_session()

            # 获取一个管理员用户用于构造 context
            admin_user = db.query(DBUser).filter(DBUser.role == UserRole.ADMIN).first()
            if not admin_user:
                logger.info("缓存预热: 无管理员用户，跳过仪表盘统计预热")
                return True

            context = WarmupContext(db=db, user=admin_user)
            adapter = AdminDashboardStatsAdapter()
            await adapter.handle(context)

            logger.debug("缓存预热: 管理员仪表盘统计已预热")
            return True

        except Exception as e:
            logger.warning(f"缓存预热失败 (仪表盘统计): {e}")
            return False
        finally:
            if db:
                db.close()

    @classmethod
    async def _warmup_admin_heatmap(cls) -> bool:
        """预热管理员热力图缓存"""
        db = None
        try:
            from src.services.usage.service import UsageService

            db = create_session()

            # 预热全局热力图（管理员视角）
            await UsageService.get_cached_heatmap(
                db=db,
                user_id=None,
                include_actual_cost=True,
            )

            logger.debug("缓存预热: 管理员热力图已预热")
            return True

        except Exception as e:
            logger.warning(f"缓存预热失败 (热力图): {e}")
            return False
        finally:
            if db:
                db.close()

    @classmethod
    async def _warmup_daily_stats(cls) -> bool:
        """预热每日统计缓存"""
        db = None
        try:
            from src.api.dashboard.routes import (  # TODO(arch): 提取 dashboard 统计计算到 services 层
                DashboardDailyStatsAdapter,
            )
            from src.models.database import User as DBUser

            db = create_session()

            # 获取一个管理员用户
            admin_user = db.query(DBUser).filter(DBUser.role == UserRole.ADMIN).first()
            if not admin_user:
                logger.info("缓存预热: 无管理员用户，跳过每日统计预热")
                return True

            context = WarmupContext(db=db, user=admin_user)

            # 预热 7 天的每日统计
            adapter = DashboardDailyStatsAdapter(days=7)
            await adapter.handle(context)

            logger.debug("缓存预热: 每日统计已预热")
            return True

        except Exception as e:
            logger.warning(f"缓存预热失败 (每日统计): {e}")
            return False
        finally:
            if db:
                db.close()


async def start_cache_warmup() -> None:
    """启动缓存预热（作为后台任务）"""
    asyncio.create_task(CacheWarmupService.warmup_all())
