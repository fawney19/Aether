"""
系统维护定时任务调度器

包含以下任务：
- 统计聚合：每天凌晨聚合前一天的统计数据
- Provider 签到：每天凌晨执行所有已配置 Provider 的签到
- 使用记录清理：分级清理策略（压缩、清空、删除）
- 审计日志清理：定期清理过期的审计日志
- 连接池监控：定期检查数据库连接池状态
- Pending 状态清理：清理异常的 Pending 状态记录
- Gemini 文件映射清理：清理过期的 Gemini 文件→Key 映射
- 请求候选记录清理：定期清理过期的 request_candidates 记录
- 数据库表维护：定期 VACUUM ANALYZE 防止表和索引膨胀

使用 APScheduler 进行任务调度，支持时区配置。
"""

from __future__ import annotations

import asyncio
from datetime import date, datetime, timedelta, timezone
from typing import Any

from sqlalchemy import delete, literal_column, text

from src.config.settings import config
from src.core.logger import logger
from src.database import create_session
from src.models.database import AuditLog, Provider, RequestCandidate, Usage
from src.services.provider_ops.service import ProviderOpsService
from src.services.system.config import SystemConfigService
from src.services.system.scheduler import get_scheduler
from src.services.system.stats_aggregator import StatsAggregatorService
from src.services.user.apikey import ApiKeyService
from src.utils.compression import compress_json


class MaintenanceScheduler:
    """系统维护任务调度器"""

    # 签到任务的 job_id
    CHECKIN_JOB_ID = "provider_checkin"

    def __init__(self) -> None:
        self.running = False
        self._interval_tasks = []
        self._stats_aggregation_lock = asyncio.Lock()

    def _get_checkin_time(self) -> tuple[int, int]:
        """获取签到任务的执行时间

        Returns:
            (hour, minute) 元组
        """
        db = create_session()
        try:
            time_str = SystemConfigService.get_config(db, "provider_checkin_time", "01:05")
            return self._parse_time_string(time_str)
        finally:
            db.close()

    @staticmethod
    def _parse_time_string(time_str: str) -> tuple[int, int]:
        """解析时间字符串为 (hour, minute) 元组

        Args:
            time_str: HH:MM 格式的时间字符串

        Returns:
            (hour, minute) 元组，解析失败返回默认值 (1, 5)
        """
        try:
            if not time_str or ":" not in time_str:
                return (1, 5)
            parts = time_str.split(":")
            hour = int(parts[0])
            minute = int(parts[1])
            # 验证范围
            if 0 <= hour <= 23 and 0 <= minute <= 59:
                return (hour, minute)
            return (1, 5)
        except (ValueError, IndexError):
            return (1, 5)

    def update_checkin_time(self, time_str: str) -> bool:
        """更新签到任务的执行时间

        Args:
            time_str: HH:MM 格式的时间字符串

        Returns:
            是否成功更新
        """
        hour, minute = self._parse_time_string(time_str)

        scheduler = get_scheduler()
        success = scheduler.reschedule_cron_job(
            self.CHECKIN_JOB_ID,
            hour=hour,
            minute=minute,
        )

        if success:
            logger.info(f"Provider 签到任务时间已更新为: {hour:02d}:{minute:02d}")

        return success

    def get_checkin_job_info(self) -> dict | None:
        """获取签到任务的信息

        Returns:
            任务信息字典
        """
        scheduler = get_scheduler()
        return scheduler.get_job_info(self.CHECKIN_JOB_ID)

    async def start(self) -> Any:
        """启动调度器"""
        if self.running:
            logger.warning("Maintenance scheduler already running")
            return

        self.running = True
        logger.info("系统维护调度器已启动")

        scheduler = get_scheduler()

        # 注册定时任务
        # 统计聚合任务 - UTC 00:05 执行
        scheduler.add_cron_job(
            self._scheduled_stats_aggregation,
            hour=0,
            minute=5,
            job_id="stats_aggregation",
            name="统计数据聚合",
            timezone="UTC",
        )
        # 小时统计聚合任务 - 每小时 05 分执行（UTC）
        scheduler.add_cron_job(
            self._scheduled_hourly_stats_aggregation,
            hour="*",
            minute=5,
            job_id="stats_hourly_aggregation",
            name="统计小时数据聚合",
            timezone="UTC",
        )
        # 清理任务 - 凌晨 3 点执行
        scheduler.add_cron_job(
            self._scheduled_cleanup,
            hour=3,
            minute=0,
            job_id="usage_cleanup",
            name="使用记录清理",
        )

        # 连接池监控 - 每 5 分钟
        scheduler.add_interval_job(
            self._scheduled_monitor,
            minutes=5,
            job_id="pool_monitor",
            name="连接池监控",
        )

        # Pending 状态清理 - 每 5 分钟
        scheduler.add_interval_job(
            self._scheduled_pending_cleanup,
            minutes=5,
            job_id="pending_cleanup",
            name="Pending状态清理",
        )

        # 审计日志清理 - 凌晨 4 点执行
        scheduler.add_cron_job(
            self._scheduled_audit_cleanup,
            hour=4,
            minute=0,
            job_id="audit_cleanup",
            name="审计日志清理",
        )

        # Gemini 文件映射清理 - 每小时执行
        scheduler.add_interval_job(
            self._scheduled_gemini_file_mapping_cleanup,
            hours=1,
            job_id="gemini_file_mapping_cleanup",
            name="Gemini文件映射清理",
        )

        # 请求候选记录清理 - 凌晨 3:30 执行
        scheduler.add_cron_job(
            self._scheduled_candidate_cleanup,
            hour=3,
            minute=30,
            job_id="candidate_cleanup",
            name="请求候选记录清理",
        )

        # 数据库表维护 - 每周日凌晨 5 点执行 VACUUM ANALYZE
        scheduler.add_cron_job(
            self._scheduled_db_maintenance,
            day_of_week="sun",
            hour=5,
            minute=0,
            job_id="db_maintenance",
            name="数据库表维护",
        )

        # Antigravity User-Agent 版本刷新 - 每 6 小时
        scheduler.add_interval_job(
            self._scheduled_antigravity_ua_refresh,
            hours=6,
            job_id="antigravity_ua_refresh",
            name="Antigravity UA版本刷新",
        )

        # Provider 签到任务 - 根据配置时间执行
        checkin_hour, checkin_minute = self._get_checkin_time()
        scheduler.add_cron_job(
            self._scheduled_provider_checkin,
            hour=checkin_hour,
            minute=checkin_minute,
            job_id=self.CHECKIN_JOB_ID,
            name="Provider签到",
        )

        # 启动时执行一次初始化任务
        if config.maintenance_startup_tasks_enabled:
            from src.utils.async_utils import safe_create_task

            safe_create_task(self._run_startup_tasks())
        else:
            logger.info("维护调度器启动任务已禁用（MAINTENANCE_STARTUP_TASKS_ENABLED=false）")

    async def _run_startup_tasks(self) -> None:
        """启动时执行的初始化任务"""
        # 延迟执行，等待系统完全启动（Redis 连接、其他后台任务稳定）
        # 增加延迟时间避免与 UsageQueueConsumer 等后台任务竞争数据库连接
        await asyncio.sleep(10)

        # 刷新 Antigravity User-Agent 版本号（不阻塞其他启动任务）
        try:
            from src.services.provider.adapters.antigravity.client import refresh_user_agent

            await refresh_user_agent()
        except Exception as e:
            logger.debug("启动时刷新 Antigravity UA 版本失败（不影响运行）: {}", e)

        try:
            logger.info("启动时清理残留的 pending/streaming 请求...")
            await self._perform_pending_cleanup()
        except Exception as e:
            logger.exception(f"启动时 pending 清理执行出错: {e}")

    async def stop(self) -> Any:
        """停止调度器"""
        if not self.running:
            return

        self.running = False
        scheduler = get_scheduler()
        scheduler.stop()

        logger.info("系统维护调度器已停止")

    # ========== 任务函数（APScheduler 直接调用异步函数） ==========

    async def _scheduled_stats_aggregation(self, backfill: bool = False) -> None:
        """统计聚合任务（定时调用）"""
        await self._perform_stats_aggregation(backfill=backfill)

    async def _scheduled_hourly_stats_aggregation(self) -> None:
        """小时统计聚合任务（定时调用）"""
        await self._perform_hourly_stats_aggregation()

    async def _scheduled_cleanup(self) -> None:
        """清理任务（定时调用）"""
        await self._perform_cleanup()

    async def _scheduled_monitor(self) -> None:
        """监控任务（定时调用）"""
        try:
            from src.database import log_pool_status

            log_pool_status()
        except Exception as e:
            logger.exception(f"连接池监控任务出错: {e}")

    async def _scheduled_pending_cleanup(self) -> None:
        """Pending 清理任务（定时调用）"""
        await self._perform_pending_cleanup()

    async def _scheduled_audit_cleanup(self) -> None:
        """审计日志清理任务（定时调用）"""
        await self._perform_audit_cleanup()

    async def _scheduled_candidate_cleanup(self) -> None:
        """请求候选记录清理任务（定时调用）"""
        await self._perform_candidate_cleanup()

    async def _scheduled_db_maintenance(self) -> None:
        """数据库表维护任务（定时调用）"""
        await self._perform_db_maintenance()

    async def _scheduled_gemini_file_mapping_cleanup(self) -> None:
        """Gemini 文件映射清理任务（定时调用）"""
        await self._perform_gemini_file_mapping_cleanup()

    async def _scheduled_antigravity_ua_refresh(self) -> None:
        """Antigravity User-Agent 版本刷新（定时调用）"""
        try:
            from src.services.provider.adapters.antigravity.client import refresh_user_agent

            await refresh_user_agent()
        except Exception as e:
            logger.debug("定时刷新 Antigravity UA 版本失败: {}", e)

    async def _scheduled_provider_checkin(self) -> None:
        """Provider 签到任务（定时调用）"""
        await self._perform_provider_checkin()

    # ========== 实际任务实现 ==========

    async def _perform_stats_aggregation(self, backfill: bool = False) -> None:
        """执行统计聚合任务

        Args:
            backfill: 是否回填历史数据（启动时检查缺失的日期）
        """
        if self._stats_aggregation_lock.locked():
            logger.info("统计聚合任务正在运行，跳过本次触发")
            return

        async with self._stats_aggregation_lock:
            db = create_session()
            try:
                # 检查是否启用统计聚合
                if not SystemConfigService.get_config(db, "enable_stats_aggregation", True):
                    logger.info("统计聚合已禁用，跳过聚合任务")
                    return

                logger.info("开始执行统计数据聚合...")

                from src.models.database import StatsDaily
                from src.models.database import User as DBUser

                # 使用 UTC 日期，定时任务在 UTC 00:05 触发，聚合 UTC 昨天
                now_utc = datetime.now(timezone.utc)
                today_utc = now_utc.replace(hour=0, minute=0, second=0, microsecond=0)

                if backfill:
                    # 启动时检查并回填缺失的日期
                    from src.models.database import StatsSummary

                    summary = db.query(StatsSummary).first()
                    if not summary:
                        # 首次运行，回填所有历史数据
                        logger.info("检测到首次运行，开始回填历史统计数据...")
                        days_to_backfill = SystemConfigService.get_config(
                            db, "stats_backfill_days", 365
                        )
                        count = StatsAggregatorService.backfill_historical_data(
                            db, days=days_to_backfill
                        )
                        logger.info(f"历史数据回填完成，共 {count} 天")
                        return

                    # 非首次运行，检查最近是否有缺失的日期需要回填
                    from src.models.database import StatsDailyModel, StatsDailyProvider

                    yesterday_utc_date = today_utc.date() - timedelta(days=1)
                    max_backfill_days: int = (
                        SystemConfigService.get_config(db, "max_stats_backfill_days", 30) or 30
                    )

                    # 计算回填检查的起始日期
                    check_start_date = yesterday_utc_date - timedelta(days=max_backfill_days - 1)
                    check_start_dt = datetime.combine(
                        check_start_date, datetime.min.time(), tzinfo=timezone.utc
                    )

                    # 单次查询获取三张统计表中已有数据的日期（UNION ALL 合并）
                    existing_daily_dates: set[date] = set()
                    existing_model_dates: set[date] = set()
                    existing_provider_dates: set[date] = set()

                    q_daily = db.query(
                        StatsDaily.date.label("dt"),
                        literal_column("'daily'").label("src"),
                    ).filter(StatsDaily.date >= check_start_dt)
                    q_model = (
                        db.query(
                            StatsDailyModel.date.label("dt"),
                            literal_column("'model'").label("src"),
                        )
                        .filter(StatsDailyModel.date >= check_start_dt)
                        .distinct()
                    )
                    q_provider = (
                        db.query(
                            StatsDailyProvider.date.label("dt"),
                            literal_column("'provider'").label("src"),
                        )
                        .filter(StatsDailyProvider.date >= check_start_dt)
                        .distinct()
                    )
                    combined = q_daily.union_all(q_model).union_all(q_provider).all()

                    for stat_date, src in combined:
                        if stat_date.tzinfo is None:
                            stat_date = stat_date.replace(tzinfo=timezone.utc)
                        d = stat_date.date()
                        if src == "daily":
                            existing_daily_dates.add(d)
                        elif src == "model":
                            existing_model_dates.add(d)
                        else:
                            existing_provider_dates.add(d)

                    # 找出需要回填的日期
                    all_dates = set()
                    current = check_start_date
                    while current <= yesterday_utc_date:
                        all_dates.add(current)
                        current += timedelta(days=1)

                    # 需要回填 StatsDaily 的日期
                    missing_daily_dates = all_dates - existing_daily_dates
                    # 需要回填 StatsDailyModel 的日期
                    missing_model_dates = all_dates - existing_model_dates
                    # 需要回填 StatsDailyProvider 的日期
                    missing_provider_dates = all_dates - existing_provider_dates
                    # 合并所有需要处理的日期
                    dates_to_process = (
                        missing_daily_dates | missing_model_dates | missing_provider_dates
                    )

                    if dates_to_process:
                        sorted_dates = sorted(dates_to_process)
                        logger.info(
                            f"检测到 {len(dates_to_process)} 天的统计数据需要回填 "
                            f"(StatsDaily 缺失 {len(missing_daily_dates)} 天, "
                            f"StatsDailyModel 缺失 {len(missing_model_dates)} 天, "
                            f"StatsDailyProvider 缺失 {len(missing_provider_dates)} 天)"
                        )

                        users = db.query(DBUser.id).filter(DBUser.is_active.is_(True)).all()
                        user_ids = [user_id for (user_id,) in users]

                        failed_dates = 0
                        for current_date in sorted_dates:
                            try:
                                current_date_utc = datetime.combine(
                                    current_date, datetime.min.time(), tzinfo=timezone.utc
                                )
                                StatsAggregatorService.aggregate_daily_stats_bundle(
                                    db, current_date_utc, user_ids=user_ids
                                )
                                db.expunge_all()
                            except Exception as e:
                                failed_dates += 1
                                logger.warning(f"回填日期 {current_date} 失败: {e}")
                                try:
                                    db.rollback()
                                except Exception as rollback_err:
                                    logger.error(f"回滚失败: {rollback_err}")

                        StatsAggregatorService.update_summary(db)

                        if failed_dates > 0:
                            logger.warning(
                                f"回填完成，共处理 {len(dates_to_process)} 天，"
                                f"失败: {failed_dates} 天"
                            )
                        else:
                            logger.info(f"缺失数据回填完成，共处理 {len(dates_to_process)} 天")
                    else:
                        logger.info("统计数据已是最新，无需回填")
                    return

                # 定时任务：聚合昨天 (UTC) 的数据
                yesterday_utc = today_utc - timedelta(days=1)
                users = db.query(DBUser.id).filter(DBUser.is_active.is_(True)).all()
                user_ids = [user_id for (user_id,) in users]

                StatsAggregatorService.aggregate_daily_stats_bundle(
                    db, yesterday_utc, user_ids=user_ids
                )

                StatsAggregatorService.update_summary(db)

                logger.info("统计数据聚合完成")

            except Exception as e:
                logger.exception(f"统计聚合任务执行失败: {e}")
                try:
                    db.rollback()
                except Exception:
                    pass
            finally:
                db.close()

    async def _perform_hourly_stats_aggregation(self) -> None:
        """执行小时统计聚合任务"""
        db = create_session()
        try:
            if not SystemConfigService.get_config(db, "enable_stats_aggregation", True):
                logger.info("统计聚合已禁用，跳过小时聚合任务")
                return

            now_utc = datetime.now(timezone.utc)
            last_hour = now_utc.replace(minute=0, second=0, microsecond=0) - timedelta(hours=1)
            StatsAggregatorService.aggregate_hourly_stats_bundle(db, last_hour)
            logger.info(f"小时统计聚合完成: {last_hour.isoformat()}")
        except Exception as e:
            logger.exception(f"小时统计聚合任务执行失败: {e}")
            try:
                db.rollback()
            except Exception:
                pass
        finally:
            db.close()

    async def _perform_pending_cleanup(self) -> None:
        """执行 pending 状态清理"""

        def _do_pending_cleanup() -> int:
            db = create_session()
            try:
                from src.services.usage.service import UsageService

                timeout_minutes = SystemConfigService.get_config(
                    db, "pending_request_timeout_minutes", 10
                )
                # pending 清理涉及 candidate 表关联查询，限制批次大小以控制内存
                batch_size = min(
                    max(SystemConfigService.get_config(db, "cleanup_batch_size", 1000), 1),
                    200,
                )
                return UsageService.cleanup_stale_pending_requests(
                    db,
                    timeout_minutes=timeout_minutes,
                    batch_size=batch_size,
                )
            except Exception as e:
                logger.exception(f"清理 pending 请求失败: {e}")
                db.rollback()
                return 0
            finally:
                db.close()

        loop = asyncio.get_running_loop()
        cleaned_count = await loop.run_in_executor(None, _do_pending_cleanup)
        if cleaned_count > 0:
            logger.info(f"清理了 {cleaned_count} 条超时的 pending/streaming 请求")

    async def _perform_audit_cleanup(self) -> None:
        """执行审计日志清理任务"""

        def _do_audit_cleanup() -> int:
            db = create_session()
            try:
                if not SystemConfigService.get_config(db, "enable_auto_cleanup", True):
                    logger.info("自动清理已禁用，跳过审计日志清理")
                    return 0

                audit_retention_days = max(
                    SystemConfigService.get_config(db, "audit_log_retention_days", 30),
                    7,
                )
                batch_size = SystemConfigService.get_config(db, "cleanup_batch_size", 1000)
                cutoff_time = datetime.now(timezone.utc) - timedelta(days=audit_retention_days)

                logger.info(f"开始清理 {audit_retention_days} 天前的审计日志...")

                total_deleted = 0
                while True:
                    batch_db = create_session()
                    try:
                        records_to_delete = (
                            batch_db.query(AuditLog.id)
                            .filter(AuditLog.created_at < cutoff_time)
                            .limit(batch_size)
                            .all()
                        )

                        if not records_to_delete:
                            break

                        record_ids = [r.id for r in records_to_delete]

                        result = batch_db.execute(
                            delete(AuditLog)
                            .where(AuditLog.id.in_(record_ids))
                            .execution_options(synchronize_session=False)
                        )

                        rows_deleted = result.rowcount
                        batch_db.commit()

                        total_deleted += rows_deleted
                        logger.debug(f"已删除 {rows_deleted} 条审计日志，累计 {total_deleted} 条")
                    except Exception as e:
                        logger.exception(f"删除审计日志批次失败: {e}")
                        try:
                            batch_db.rollback()
                        except Exception:
                            pass
                        break
                    finally:
                        batch_db.close()

                return total_deleted
            except Exception as e:
                logger.exception(f"审计日志清理失败: {e}")
                return 0
            finally:
                db.close()

        loop = asyncio.get_running_loop()
        total_deleted = await loop.run_in_executor(None, _do_audit_cleanup)
        if total_deleted > 0:
            logger.info(f"审计日志清理完成，共删除 {total_deleted} 条记录")
        else:
            logger.info("无需清理的审计日志")

    async def _perform_gemini_file_mapping_cleanup(self) -> None:
        """清理过期的 Gemini 文件映射记录"""
        db = create_session()
        try:
            from src.services.gemini_files_mapping import cleanup_expired_mappings

            deleted_count = cleanup_expired_mappings(db)

            if deleted_count > 0:
                logger.info(f"清理了 {deleted_count} 条过期的 Gemini 文件映射")

        except Exception as e:
            logger.exception(f"Gemini 文件映射清理失败: {e}")
            try:
                db.rollback()
            except Exception:
                pass
        finally:
            db.close()

    async def _perform_provider_checkin(self) -> None:
        """执行 Provider 签到任务

        遍历所有已配置 provider_ops 的 Provider，触发签到。
        签到会在余额查询时一起执行（先签到再查询余额）。
        """
        db = create_session()
        try:
            # 检查是否启用签到任务
            if not SystemConfigService.get_config(db, "enable_provider_checkin", True):
                logger.info("Provider 签到已禁用，跳过签到任务")
                return

            # 获取所有已配置 provider_ops 的活跃 Provider（只查询需要的字段）
            providers = (
                db.query(Provider.id, Provider.config).filter(Provider.is_active.is_(True)).all()
            )
            provider_ids = [p.id for p in providers if p.config and p.config.get("provider_ops")]

            if not provider_ids:
                logger.info("无已配置的 Provider，跳过签到任务")
                return

            logger.info(f"开始执行 Provider 签到，共 {len(provider_ids)} 个...")

            # 释放主 session 的连接，避免在整个签到期间占用连接池
            # （后续每个 provider 将使用独立短生命周期 session）
            try:
                if db.in_transaction():
                    db.commit()
            except Exception:
                try:
                    db.rollback()
                except Exception:
                    pass
            try:
                db.close()
            except Exception:
                pass
            db = None

            # 使用信号量限制并发，避免同时发起过多请求
            concurrency = 3  # 签到任务并发数
            semaphore = asyncio.Semaphore(concurrency)

            async def _checkin_provider(provider_id: str) -> tuple[str, bool, str]:
                """执行单个 Provider 的签到"""
                async with semaphore:
                    task_db = create_session()
                    try:
                        service = ProviderOpsService(task_db)
                        # 触发余额查询（会先执行签到）
                        result = await service.query_balance(provider_id)
                        # 检查签到结果
                        checkin_success = None
                        checkin_message = ""
                        if result.data and hasattr(result.data, "extra") and result.data.extra:
                            checkin_success = result.data.extra.get("checkin_success")
                            checkin_message = result.data.extra.get("checkin_message", "")
                        if checkin_success is True:
                            return provider_id, True, checkin_message
                        elif checkin_success is False:
                            return provider_id, False, checkin_message
                        else:
                            # None 表示未执行签到（可能没配置 Cookie）
                            return provider_id, False, "未执行签到"
                    except Exception as e:
                        logger.warning(f"Provider {provider_id} 签到失败: {e}")
                        return provider_id, False, str(e)
                    finally:
                        try:
                            task_db.close()
                        except Exception:
                            pass

            # 并行执行签到
            tasks = [_checkin_provider(pid) for pid in provider_ids]
            results = await asyncio.gather(*tasks)

            # 统计结果
            success_count = sum(1 for _, success, _ in results if success)
            logger.info(f"Provider 签到完成: {success_count}/{len(provider_ids)} 成功")

            # 记录详细结果
            for provider_id, success, message in results:
                if success:
                    logger.debug(f"  - {provider_id}: 签到成功 - {message}")
                elif message != "未执行签到":
                    logger.debug(f"  - {provider_id}: 签到失败 - {message}")

        except Exception as e:
            logger.exception(f"Provider 签到任务执行失败: {e}")
        finally:
            if db is not None:
                db.close()

    async def _perform_candidate_cleanup(self) -> None:
        """清理过期的 request_candidates 记录"""

        def _do_candidate_cleanup() -> int:
            db = create_session()
            try:
                if not SystemConfigService.get_config(db, "enable_auto_cleanup", True):
                    logger.info("自动清理已禁用，跳过候选记录清理")
                    return 0

                retention_days = max(
                    SystemConfigService.get_config(db, "detail_log_retention_days", 7),
                    3,
                )
                batch_size = SystemConfigService.get_config(db, "cleanup_batch_size", 1000)
                cutoff_time = datetime.now(timezone.utc) - timedelta(days=retention_days)

                logger.info(f"开始清理 {retention_days} 天前的请求候选记录...")
            except Exception as e:
                logger.exception(f"候选记录清理配置读取失败: {e}")
                return 0
            finally:
                db.close()

            total_deleted = 0
            while True:
                batch_db = create_session()
                try:
                    records_to_delete = (
                        batch_db.query(RequestCandidate.id)
                        .filter(RequestCandidate.created_at < cutoff_time)
                        .limit(batch_size)
                        .all()
                    )

                    if not records_to_delete:
                        break

                    record_ids = [r.id for r in records_to_delete]

                    result = batch_db.execute(
                        delete(RequestCandidate)
                        .where(RequestCandidate.id.in_(record_ids))
                        .execution_options(synchronize_session=False)
                    )

                    rows_deleted = result.rowcount
                    batch_db.commit()

                    total_deleted += rows_deleted
                    logger.debug(f"已删除 {rows_deleted} 条候选记录，累计 {total_deleted} 条")
                except Exception as e:
                    logger.exception(f"删除候选记录批次失败: {e}")
                    try:
                        batch_db.rollback()
                    except Exception:
                        pass
                    break
                finally:
                    batch_db.close()

            return total_deleted

        loop = asyncio.get_running_loop()
        total_deleted = await loop.run_in_executor(None, _do_candidate_cleanup)
        if total_deleted > 0:
            logger.info(f"请求候选记录清理完成，共删除 {total_deleted} 条记录")
        else:
            logger.info("无需清理的候选记录")

    async def _perform_db_maintenance(self) -> None:
        """执行数据库表维护（VACUUM ANALYZE）

        对大表执行 VACUUM ANALYZE，防止表和索引膨胀。
        VACUUM 不能在事务内执行，需要使用 autocommit 连接。
        使用线程池执行，避免阻塞事件循环。
        """
        db = create_session()
        try:
            if not SystemConfigService.get_config(db, "enable_db_maintenance", True):
                logger.info("数据库维护已禁用，跳过")
                return
        except Exception as e:
            logger.exception(f"读取数据库维护配置失败: {e}")
            return
        finally:
            db.close()

        tables = ["usage", "request_candidates", "audit_logs"]

        logger.info(f"开始数据库表维护（VACUUM ANALYZE），目标表: {', '.join(tables)}")

        from src.database.database import _ensure_engine

        engine = _ensure_engine()

        def _vacuum_table(table_name: str) -> tuple[str, bool, str]:
            """在线程池中执行 VACUUM ANALYZE（同步阻塞操作）"""
            try:
                with engine.connect() as raw_conn:
                    conn = raw_conn.execution_options(isolation_level="AUTOCOMMIT")
                    conn.execute(text(f"VACUUM ANALYZE {table_name}"))
                return table_name, True, ""
            except Exception as e:
                return table_name, False, str(e)

        loop = asyncio.get_running_loop()
        for table in tables:
            table_name, success, error = await loop.run_in_executor(None, _vacuum_table, table)
            if success:
                logger.info(f"VACUUM ANALYZE {table_name} 完成")
            else:
                logger.warning(f"VACUUM ANALYZE {table_name} 失败: {error}")

        logger.info("数据库表维护完成")

    async def _perform_cleanup(self) -> None:
        """执行清理任务（在线程池中运行，避免阻塞事件循环）"""

        def _do_cleanup() -> None:
            db = create_session()
            try:
                if not SystemConfigService.get_config(db, "enable_auto_cleanup", True):
                    logger.info("自动清理已禁用，跳过清理任务")
                    return

                logger.info("开始执行使用记录分级清理...")

                detail_retention = SystemConfigService.get_config(
                    db, "detail_log_retention_days", 7
                )
                compressed_retention = SystemConfigService.get_config(
                    db, "compressed_log_retention_days", 30
                )
                header_retention = SystemConfigService.get_config(db, "header_retention_days", 90)
                log_retention = SystemConfigService.get_config(db, "log_retention_days", 365)
                batch_size = SystemConfigService.get_config(db, "cleanup_batch_size", 1000)
                auto_delete = SystemConfigService.get_config(db, "auto_delete_expired_keys", False)
            except Exception as e:
                logger.exception(f"清理任务配置读取失败: {e}")
                return
            finally:
                db.close()

            try:
                now = datetime.now(timezone.utc)

                # 1. 压缩详细日志 (body 字段 -> 压缩字段)
                detail_cutoff = now - timedelta(days=detail_retention)
                body_compressed = self._cleanup_body_fields(detail_cutoff, batch_size)

                # 2. 清理压缩字段
                compressed_cutoff = now - timedelta(days=compressed_retention)
                compressed_cleaned = self._cleanup_compressed_fields(compressed_cutoff, batch_size)

                # 3. 清理请求头
                header_cutoff = now - timedelta(days=header_retention)
                header_cleaned = self._cleanup_header_fields(header_cutoff, batch_size)

                # 4. 删除过期记录
                log_cutoff = now - timedelta(days=log_retention)
                records_deleted = self._delete_old_records(log_cutoff, batch_size)

                # 5. 清理过期的API Keys
                keys_db = create_session()
                try:
                    keys_cleaned = ApiKeyService.cleanup_expired_keys(
                        keys_db, auto_delete=auto_delete
                    )
                except Exception as e:
                    logger.exception(f"清理过期 Keys 失败: {e}")
                    keys_cleaned = 0
                finally:
                    keys_db.close()

                logger.info(
                    f"清理完成: 压缩 {body_compressed} 条, "
                    f"清理压缩字段 {compressed_cleaned} 条, "
                    f"清理header {header_cleaned} 条, "
                    f"删除记录 {records_deleted} 条, "
                    f"清理过期Keys {keys_cleaned} 条"
                )
            except Exception as e:
                logger.exception(f"清理任务执行失败: {e}")

        loop = asyncio.get_running_loop()
        await loop.run_in_executor(None, _do_cleanup)

    def _cleanup_body_fields(self, cutoff_time: datetime, batch_size: int) -> int:
        """压缩 request_body 和 response_body 字段到压缩字段

        逐条处理，确保每条记录都正确更新（同步方法，在线程池中调用）
        """
        from sqlalchemy import null, update

        total_compressed = 0
        no_progress_count = 0
        memory_safe_batch_size = max(1, min(batch_size, 100))

        while True:
            batch_db = create_session()
            try:
                records = (
                    batch_db.query(
                        Usage.id,
                        Usage.request_body,
                        Usage.response_body,
                        Usage.provider_request_body,
                        Usage.client_response_body,
                    )
                    .filter(Usage.created_at < cutoff_time)
                    .filter(
                        (Usage.request_body.isnot(None))
                        | (Usage.response_body.isnot(None))
                        | (Usage.provider_request_body.isnot(None))
                        | (Usage.client_response_body.isnot(None))
                    )
                    .limit(memory_safe_batch_size)
                    .all()
                )

                if not records:
                    break

                valid_records = [
                    r
                    for r in records
                    if r.request_body is not None
                    or r.response_body is not None
                    or r.provider_request_body is not None
                    or r.client_response_body is not None
                ]

                if not valid_records:
                    logger.warning(
                        f"检测到 {len(records)} 条记录的 body 字段为 JSON null，进行清理"
                    )
                    for r in records:
                        batch_db.execute(
                            update(Usage)
                            .where(Usage.id == r.id)
                            .values(
                                request_body=null(),
                                response_body=null(),
                                provider_request_body=null(),
                                client_response_body=null(),
                            )
                        )
                    batch_db.commit()
                    continue

                batch_success = 0

                for r in valid_records:
                    try:
                        result = batch_db.execute(
                            update(Usage)
                            .where(Usage.id == r.id)
                            .values(
                                request_body=null(),
                                response_body=null(),
                                provider_request_body=null(),
                                client_response_body=null(),
                                request_body_compressed=(
                                    compress_json(r.request_body) if r.request_body else None
                                ),
                                response_body_compressed=(
                                    compress_json(r.response_body) if r.response_body else None
                                ),
                                provider_request_body_compressed=(
                                    compress_json(r.provider_request_body)
                                    if r.provider_request_body
                                    else None
                                ),
                                client_response_body_compressed=(
                                    compress_json(r.client_response_body)
                                    if r.client_response_body
                                    else None
                                ),
                            )
                            .execution_options(synchronize_session=False)
                        )
                        if result.rowcount > 0:
                            batch_success += 1
                    except Exception as e:
                        logger.warning(f"压缩记录 {r.id} 失败: {e}")
                        continue

                batch_db.commit()

                if batch_success == 0:
                    no_progress_count += 1
                    if no_progress_count >= 3:
                        logger.error(
                            f"压缩 body 字段连续 {no_progress_count} 批无进展，"
                            "终止循环以避免死循环"
                        )
                        break
                else:
                    no_progress_count = 0

                total_compressed += batch_success
                logger.debug(
                    f"已压缩 {batch_success} 条记录的 body 字段，累计 {total_compressed} 条"
                )

            except Exception as e:
                logger.exception(f"压缩 body 字段失败: {e}")
                try:
                    batch_db.rollback()
                except Exception:
                    pass
                break
            finally:
                batch_db.close()

        return total_compressed

    def _cleanup_compressed_fields(self, cutoff_time: datetime, batch_size: int) -> int:
        """清理压缩字段（删除压缩的body）

        每批使用短生命周期 session（同步方法，在线程池中调用）
        """
        from sqlalchemy import null, update

        total_cleaned = 0

        while True:
            batch_db = create_session()
            try:
                records_to_clean = (
                    batch_db.query(Usage.id)
                    .filter(Usage.created_at < cutoff_time)
                    .filter(
                        (Usage.request_body_compressed.isnot(None))
                        | (Usage.response_body_compressed.isnot(None))
                        | (Usage.provider_request_body_compressed.isnot(None))
                        | (Usage.client_response_body_compressed.isnot(None))
                    )
                    .limit(batch_size)
                    .all()
                )

                if not records_to_clean:
                    break

                record_ids = [r.id for r in records_to_clean]

                result = batch_db.execute(
                    update(Usage)
                    .where(Usage.id.in_(record_ids))
                    .values(
                        request_body_compressed=null(),
                        response_body_compressed=null(),
                        provider_request_body_compressed=null(),
                        client_response_body_compressed=null(),
                    )
                )

                rows_updated = result.rowcount
                batch_db.commit()

                if rows_updated == 0:
                    logger.warning("清理压缩字段: rowcount=0，可能存在问题")
                    break

                total_cleaned += rows_updated
                logger.debug(f"已清理 {rows_updated} 条记录的压缩字段，累计 {total_cleaned} 条")

            except Exception as e:
                logger.exception(f"清理压缩字段失败: {e}")
                try:
                    batch_db.rollback()
                except Exception:
                    pass
                break
            finally:
                batch_db.close()

        return total_cleaned

    def _cleanup_header_fields(self, cutoff_time: datetime, batch_size: int) -> int:
        """清理 request_headers, response_headers 和 provider_request_headers 字段

        每批使用短生命周期 session（同步方法，在线程池中调用）
        """
        from sqlalchemy import null, update

        total_cleaned = 0

        while True:
            batch_db = create_session()
            try:
                records_to_clean = (
                    batch_db.query(Usage.id)
                    .filter(Usage.created_at < cutoff_time)
                    .filter(
                        (Usage.request_headers.isnot(None))
                        | (Usage.response_headers.isnot(None))
                        | (Usage.provider_request_headers.isnot(None))
                    )
                    .limit(batch_size)
                    .all()
                )

                if not records_to_clean:
                    break

                record_ids = [r.id for r in records_to_clean]

                result = batch_db.execute(
                    update(Usage)
                    .where(Usage.id.in_(record_ids))
                    .values(
                        request_headers=null(),
                        response_headers=null(),
                        provider_request_headers=null(),
                    )
                )

                rows_updated = result.rowcount
                batch_db.commit()

                if rows_updated == 0:
                    logger.warning("清理 header 字段: rowcount=0，可能存在问题")
                    break

                total_cleaned += rows_updated
                logger.debug(f"已清理 {rows_updated} 条记录的 header 字段，累计 {total_cleaned} 条")

            except Exception as e:
                logger.exception(f"清理 header 字段失败: {e}")
                try:
                    batch_db.rollback()
                except Exception:
                    pass
                break
            finally:
                batch_db.close()

        return total_cleaned

    def _delete_old_records(self, cutoff_time: datetime, batch_size: int) -> int:
        """删除过期的完整记录（同步方法，在线程池中调用）"""
        total_deleted = 0

        while True:
            batch_db = create_session()
            try:
                records_to_delete = (
                    batch_db.query(Usage.id)
                    .filter(Usage.created_at < cutoff_time)
                    .limit(batch_size)
                    .all()
                )

                if not records_to_delete:
                    break

                record_ids = [r.id for r in records_to_delete]

                result = batch_db.execute(
                    delete(Usage)
                    .where(Usage.id.in_(record_ids))
                    .execution_options(synchronize_session=False)
                )

                rows_deleted = result.rowcount
                batch_db.commit()

                total_deleted += rows_deleted
                logger.debug(f"已删除 {rows_deleted} 条过期记录，累计 {total_deleted} 条")

            except Exception as e:
                logger.exception(f"删除过期记录失败: {e}")
                try:
                    batch_db.rollback()
                except Exception:
                    pass
                break
            finally:
                batch_db.close()

        return total_deleted


# 全局单例
_maintenance_scheduler = None


def get_maintenance_scheduler() -> MaintenanceScheduler:
    """获取维护调度器单例"""
    global _maintenance_scheduler
    if _maintenance_scheduler is None:
        _maintenance_scheduler = MaintenanceScheduler()
    return _maintenance_scheduler
