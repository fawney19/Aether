"""
额度周期重置定时任务

支持按天数周期重置额度：
- quota_reset_day: 重置周期（天数），例如7=每周，30=每月
- quota_last_reset_at: 上次重置时间，用于计算下次重置

使用统一的 TaskScheduler 进行调度。
"""

from datetime import datetime, timezone

from src.core.enums import ProviderBillingType
from src.core.logger import logger
from src.database import create_session
from src.models.database import Provider
from src.services.system.scheduler import get_scheduler


class QuotaScheduler:
    """额度周期重置调度器"""

    def __init__(self):
        self.running = False

    async def start(self):
        """启动调度器"""
        if self.running:
            logger.warning("Quota scheduler already running")
            return

        self.running = True
        logger.info("Quota scheduler started")

        scheduler = get_scheduler()

        # 每小时检查一次额度重置
        scheduler.add_interval_job(
            self._scheduled_quota_check,
            hours=1,
            job_id="quota_reset_check",
            name="额度周期重置检查",
        )

        # 启动时立即执行一次检查
        await self._check_and_reset_quotas()

    async def stop(self):
        """停止调度器"""
        if not self.running:
            return

        self.running = False
        logger.info("Quota scheduler stopped")

    async def _scheduled_quota_check(self):
        """额度检查任务（定时调用）"""
        await self._check_and_reset_quotas()

    async def _check_and_reset_quotas(self):
        """检查并重置周期额度"""

        db = create_session()
        try:
            # 获取所有定额类型的提供商
            providers = (
                db.query(Provider)
                .filter(
                    Provider.billing_type == ProviderBillingType.MONTHLY_QUOTA,
                    Provider.is_active == True,
                )
                .all()
            )

            if not providers:
                logger.debug("No quota providers to check")
                return

            now = datetime.now(timezone.utc)
            reset_count = 0

            for provider in providers:
                try:
                    # 如果没有上次重置时间，初始化为当前时间
                    if provider.quota_last_reset_at is None:
                        provider.quota_last_reset_at = now
                        db.commit()
                        logger.info(f"Initialized quota_last_reset_at for provider {provider.name}")
                        continue

                    # 计算距离上次重置的天数
                    days_since_reset = (now - provider.quota_last_reset_at).days

                    # 如果达到或超过重置周期，执行重置
                    if days_since_reset >= provider.quota_reset_day:
                        logger.info(f"Resetting quota for provider {provider.name}")

                        provider.monthly_used_usd = 0.0
                        provider.quota_last_reset_at = now
                        reset_count += 1

                    # 检查是否过期
                    if provider.quota_expires_at and provider.quota_expires_at < now:
                        logger.warning(f"Provider {provider.name} quota expired")
                        # 可以选择禁用过期的提供商
                        # provider.is_active = False

                except Exception as e:
                    logger.exception(f"Error processing provider {provider.name}: {e}")

            if reset_count > 0:
                db.commit()
                logger.info(f"Reset quotas for {reset_count} providers")
        finally:
            db.close()

    async def force_reset(self, provider_id: str = None):
        """手动强制重置额度"""
        db = create_session()
        try:
            now = datetime.now(timezone.utc)
            if provider_id:
                # 重置指定提供商
                provider = db.query(Provider).filter(Provider.id == provider_id).first()
                if provider and provider.billing_type == ProviderBillingType.MONTHLY_QUOTA:
                    provider.monthly_used_usd = 0.0
                    provider.quota_last_reset_at = now
                    db.commit()
                    logger.info(f"Force reset quota for provider {provider.name}")
            else:
                # 重置所有定额提供商
                providers = (
                    db.query(Provider)
                    .filter(Provider.billing_type == ProviderBillingType.MONTHLY_QUOTA)
                    .all()
                )
                for provider in providers:
                    provider.monthly_used_usd = 0.0
                    provider.quota_last_reset_at = now
                db.commit()
                logger.info(f"Force reset quotas for {len(providers)} providers")
        finally:
            db.close()


# 全局单例
_quota_scheduler = None


def get_quota_scheduler() -> QuotaScheduler:
    """获取全局调度器实例"""
    global _quota_scheduler
    if _quota_scheduler is None:
        _quota_scheduler = QuotaScheduler()
    return _quota_scheduler
