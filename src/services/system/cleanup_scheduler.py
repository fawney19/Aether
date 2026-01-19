"""
使用记录清理定时任务

分级清理策略：
- detail_log_retention_days: 压缩 request_body 和 response_body 到压缩字段
- header_retention_days: 清空 request_headers 和 response_headers
- log_retention_days: 删除整条记录

统计聚合任务：
- 每天凌晨聚合前一天的统计数据
- 更新全局统计汇总

使用 APScheduler 进行任务调度，支持时区配置。
"""

import asyncio
from datetime import datetime, timedelta, timezone

from sqlalchemy import delete
from sqlalchemy.orm import Session

from src.core.logger import logger
from src.database import create_session
from src.models.database import AuditLog, Usage
from src.services.system.config import SystemConfigService
from src.services.system.scheduler import get_scheduler
from src.services.system.stats_aggregator import StatsAggregatorService
from src.services.user.apikey import ApiKeyService
from src.utils.compression import compress_json


class CleanupScheduler:
    """使用记录清理调度器"""

    def __init__(self):
        self.running = False
        self._interval_tasks = []
        self._stats_aggregation_lock = asyncio.Lock()

    async def start(self):
        """启动调度器"""
        if self.running:
            logger.warning("Cleanup scheduler already running")
            return

        self.running = True
        logger.info("使用记录清理调度器已启动")

        scheduler = get_scheduler()

        # 注册定时任务（使用业务时区）
        # 统计聚合任务 - 凌晨 1 点执行
        scheduler.add_cron_job(
            self._scheduled_stats_aggregation,
            hour=1,
            minute=0,
            job_id="stats_aggregation",
            name="统计数据聚合",
        )
        # 统计聚合补偿任务 - 每 30 分钟检查缺失并回填
        scheduler.add_interval_job(
            self._scheduled_stats_aggregation,
            minutes=30,
            job_id="stats_aggregation_backfill",
            name="统计数据聚合补偿",
            backfill=True,
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

        # 启动时执行一次初始化任务
        asyncio.create_task(self._run_startup_tasks())

    async def _run_startup_tasks(self):
        """启动时执行的初始化任务"""
        # 延迟一点执行，确保系统完全启动
        await asyncio.sleep(2)

        try:
            logger.info("启动时执行首次清理任务...")
            await self._perform_cleanup()
        except Exception as e:
            logger.exception(f"启动时清理任务执行出错: {e}")

        try:
            logger.info("启动时检查统计数据...")
            await self._perform_stats_aggregation(backfill=True)
        except Exception as e:
            logger.exception(f"启动时统计聚合任务出错: {e}")

    async def stop(self):
        """停止调度器"""
        if not self.running:
            return

        self.running = False
        scheduler = get_scheduler()
        scheduler.stop()

        logger.info("使用记录清理调度器已停止")

    # ========== 任务函数（APScheduler 直接调用异步函数） ==========

    async def _scheduled_stats_aggregation(self, backfill: bool = False):
        """统计聚合任务（定时调用）"""
        await self._perform_stats_aggregation(backfill=backfill)

    async def _scheduled_cleanup(self):
        """清理任务（定时调用）"""
        await self._perform_cleanup()

    async def _scheduled_monitor(self):
        """监控任务（定时调用）"""
        try:
            from src.database import log_pool_status

            log_pool_status()
        except Exception as e:
            logger.exception(f"连接池监控任务出错: {e}")

    async def _scheduled_pending_cleanup(self):
        """Pending 清理任务（定时调用）"""
        await self._perform_pending_cleanup()

    async def _scheduled_audit_cleanup(self):
        """审计日志清理任务（定时调用）"""
        await self._perform_audit_cleanup()

    # ========== 实际任务实现 ==========

    async def _perform_stats_aggregation(self, backfill: bool = False):
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

                from src.models.database import StatsDaily, User as DBUser
                from src.services.system.scheduler import APP_TIMEZONE
                from zoneinfo import ZoneInfo

                # 使用业务时区计算日期，确保与定时任务触发时间一致
                # 定时任务在 Asia/Shanghai 凌晨 1 点触发，此时应聚合 Asia/Shanghai 的"昨天"
                app_tz = ZoneInfo(APP_TIMEZONE)
                now_local = datetime.now(app_tz)
                today_local = now_local.replace(hour=0, minute=0, second=0, microsecond=0)

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

                    yesterday_business_date = today_local.date() - timedelta(days=1)
                    max_backfill_days: int = SystemConfigService.get_config(
                        db, "max_stats_backfill_days", 30
                    ) or 30

                    # 计算回填检查的起始日期
                    check_start_date = yesterday_business_date - timedelta(
                        days=max_backfill_days - 1
                    )

                    # 获取 StatsDaily 和 StatsDailyModel 中已有数据的日期集合
                    existing_daily_dates = set()
                    existing_model_dates = set()
                    existing_provider_dates = set()

                    daily_stats = (
                        db.query(StatsDaily.date)
                        .filter(StatsDaily.date >= check_start_date.isoformat())
                        .all()
                    )
                    for (stat_date,) in daily_stats:
                        if stat_date.tzinfo is None:
                            stat_date = stat_date.replace(tzinfo=timezone.utc)
                        existing_daily_dates.add(stat_date.astimezone(app_tz).date())

                    model_stats = (
                        db.query(StatsDailyModel.date)
                        .filter(StatsDailyModel.date >= check_start_date.isoformat())
                        .distinct()
                        .all()
                    )
                    for (stat_date,) in model_stats:
                        if stat_date.tzinfo is None:
                            stat_date = stat_date.replace(tzinfo=timezone.utc)
                        existing_model_dates.add(stat_date.astimezone(app_tz).date())

                    provider_stats = (
                        db.query(StatsDailyProvider.date)
                        .filter(StatsDailyProvider.date >= check_start_date.isoformat())
                        .distinct()
                        .all()
                    )
                    for (stat_date,) in provider_stats:
                        if stat_date.tzinfo is None:
                            stat_date = stat_date.replace(tzinfo=timezone.utc)
                        existing_provider_dates.add(stat_date.astimezone(app_tz).date())

                    # 找出需要回填的日期
                    all_dates = set()
                    current = check_start_date
                    while current <= yesterday_business_date:
                        all_dates.add(current)
                        current += timedelta(days=1)

                    # 需要回填 StatsDaily 的日期
                    missing_daily_dates = all_dates - existing_daily_dates
                    # 需要回填 StatsDailyModel 的日期
                    missing_model_dates = all_dates - existing_model_dates
                    # 需要回填 StatsDailyProvider 的日期
                    missing_provider_dates = all_dates - existing_provider_dates
                    # 合并所有需要处理的日期
                    dates_to_process = missing_daily_dates | missing_model_dates | missing_provider_dates

                    if dates_to_process:
                        sorted_dates = sorted(dates_to_process)
                        logger.info(
                            f"检测到 {len(dates_to_process)} 天的统计数据需要回填 "
                            f"(StatsDaily 缺失 {len(missing_daily_dates)} 天, "
                            f"StatsDailyModel 缺失 {len(missing_model_dates)} 天, "
                            f"StatsDailyProvider 缺失 {len(missing_provider_dates)} 天)"
                        )

                        users = (
                            db.query(DBUser.id).filter(DBUser.is_active.is_(True)).all()
                        )

                        failed_dates = 0
                        failed_users = 0

                        for current_date in sorted_dates:
                            try:
                                current_date_local = datetime.combine(
                                    current_date, datetime.min.time(), tzinfo=app_tz
                                )
                                # 只在缺失时才聚合对应的表
                                if current_date in missing_daily_dates:
                                    StatsAggregatorService.aggregate_daily_stats(
                                        db, current_date_local
                                    )
                                if current_date in missing_model_dates:
                                    StatsAggregatorService.aggregate_daily_model_stats(
                                        db, current_date_local
                                    )
                                if current_date in missing_provider_dates:
                                    StatsAggregatorService.aggregate_daily_provider_stats(
                                        db, current_date_local
                                    )
                                # 用户统计在任一缺失时都回填
                                for (user_id,) in users:
                                    try:
                                        StatsAggregatorService.aggregate_user_daily_stats(
                                            db, user_id, current_date_local
                                        )
                                    except Exception as e:
                                        failed_users += 1
                                        logger.warning(
                                            f"回填用户 {user_id} 日期 {current_date} 失败: {e}"
                                        )
                                        try:
                                            db.rollback()
                                        except Exception as rollback_err:
                                            logger.error(f"回滚失败: {rollback_err}")
                            except Exception as e:
                                failed_dates += 1
                                logger.warning(f"回填日期 {current_date} 失败: {e}")
                                try:
                                    db.rollback()
                                except Exception as rollback_err:
                                    logger.error(f"回滚失败: {rollback_err}")

                        StatsAggregatorService.update_summary(db)

                        if failed_dates > 0 or failed_users > 0:
                            logger.warning(
                                f"回填完成，共处理 {len(dates_to_process)} 天，"
                                f"失败: {failed_dates} 天, {failed_users} 个用户记录"
                            )
                        else:
                            logger.info(f"缺失数据回填完成，共处理 {len(dates_to_process)} 天")
                    else:
                        logger.info("统计数据已是最新，无需回填")
                    return

                # 定时任务：聚合昨天的数据
                yesterday_local = today_local - timedelta(days=1)

                StatsAggregatorService.aggregate_daily_stats(db, yesterday_local)
                StatsAggregatorService.aggregate_daily_model_stats(db, yesterday_local)
                StatsAggregatorService.aggregate_daily_provider_stats(db, yesterday_local)

                users = db.query(DBUser.id).filter(DBUser.is_active.is_(True)).all()
                for (user_id,) in users:
                    try:
                        StatsAggregatorService.aggregate_user_daily_stats(
                            db, user_id, yesterday_local
                        )
                    except Exception as e:
                        logger.warning(f"聚合用户 {user_id} 统计数据失败: {e}")
                        try:
                            db.rollback()
                        except Exception:
                            pass

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

    async def _perform_pending_cleanup(self):
        """执行 pending 状态清理"""
        db = create_session()
        try:
            from src.services.usage.service import UsageService

            # 获取配置的超时时间（默认 10 分钟）
            timeout_minutes = SystemConfigService.get_config(
                db, "pending_request_timeout_minutes", 10
            )

            # 执行清理
            cleaned_count = UsageService.cleanup_stale_pending_requests(
                db, timeout_minutes=timeout_minutes
            )

            if cleaned_count > 0:
                logger.info(f"清理了 {cleaned_count} 条超时的 pending/streaming 请求")

        except Exception as e:
            logger.exception(f"清理 pending 请求失败: {e}")
            db.rollback()
        finally:
            db.close()

    async def _perform_audit_cleanup(self):
        """执行审计日志清理任务"""
        db = create_session()
        try:
            # 检查是否启用自动清理
            if not SystemConfigService.get_config(db, "enable_auto_cleanup", True):
                logger.info("自动清理已禁用，跳过审计日志清理")
                return

            # 获取审计日志保留天数（默认 30 天，最少 7 天）
            audit_retention_days = max(
                SystemConfigService.get_config(db, "audit_log_retention_days", 30),
                7,  # 最少保留 7 天，防止误配置删除所有审计日志
            )
            batch_size = SystemConfigService.get_config(db, "cleanup_batch_size", 1000)

            cutoff_time = datetime.now(timezone.utc) - timedelta(days=audit_retention_days)

            logger.info(f"开始清理 {audit_retention_days} 天前的审计日志...")

            total_deleted = 0
            while True:
                # 先查询要删除的记录 ID（分批）
                records_to_delete = (
                    db.query(AuditLog.id)
                    .filter(AuditLog.created_at < cutoff_time)
                    .limit(batch_size)
                    .all()
                )

                if not records_to_delete:
                    break

                record_ids = [r.id for r in records_to_delete]

                # 执行删除
                result = db.execute(
                    delete(AuditLog)
                    .where(AuditLog.id.in_(record_ids))
                    .execution_options(synchronize_session=False)
                )

                rows_deleted = result.rowcount
                db.commit()

                total_deleted += rows_deleted
                logger.debug(f"已删除 {rows_deleted} 条审计日志，累计 {total_deleted} 条")

                await asyncio.sleep(0.1)

            if total_deleted > 0:
                logger.info(f"审计日志清理完成，共删除 {total_deleted} 条记录")
            else:
                logger.info("无需清理的审计日志")

        except Exception as e:
            logger.exception(f"审计日志清理失败: {e}")
            try:
                db.rollback()
            except Exception:
                pass
        finally:
            db.close()

    async def _perform_cleanup(self):
        """执行清理任务"""
        db = create_session()
        try:
            # 检查是否启用自动清理
            if not SystemConfigService.get_config(db, "enable_auto_cleanup", True):
                logger.info("自动清理已禁用，跳过清理任务")
                return

            logger.info("开始执行使用记录分级清理...")

            # 获取配置参数
            detail_retention = SystemConfigService.get_config(db, "detail_log_retention_days", 7)
            compressed_retention = SystemConfigService.get_config(
                db, "compressed_log_retention_days", 90
            )
            header_retention = SystemConfigService.get_config(db, "header_retention_days", 90)
            log_retention = SystemConfigService.get_config(db, "log_retention_days", 365)
            batch_size = SystemConfigService.get_config(db, "cleanup_batch_size", 1000)

            now = datetime.now(timezone.utc)

            # 1. 压缩详细日志 (body 字段 -> 压缩字段)
            detail_cutoff = now - timedelta(days=detail_retention)
            body_compressed = await self._cleanup_body_fields(db, detail_cutoff, batch_size)

            # 2. 清理压缩字段（90天后）
            compressed_cutoff = now - timedelta(days=compressed_retention)
            compressed_cleaned = await self._cleanup_compressed_fields(
                db, compressed_cutoff, batch_size
            )

            # 3. 清理请求头
            header_cutoff = now - timedelta(days=header_retention)
            header_cleaned = await self._cleanup_header_fields(db, header_cutoff, batch_size)

            # 4. 删除过期记录
            log_cutoff = now - timedelta(days=log_retention)
            records_deleted = await self._delete_old_records(db, log_cutoff, batch_size)

            # 5. 清理过期的API Keys
            auto_delete = SystemConfigService.get_config(db, "auto_delete_expired_keys", False)
            keys_cleaned = ApiKeyService.cleanup_expired_keys(db, auto_delete=auto_delete)

            logger.info(
                f"清理完成: 压缩 {body_compressed} 条, "
                f"清理压缩字段 {compressed_cleaned} 条, "
                f"清理header {header_cleaned} 条, "
                f"删除记录 {records_deleted} 条, "
                f"清理过期Keys {keys_cleaned} 条"
            )

        except Exception as e:
            logger.exception(f"清理任务执行失败: {e}")
            db.rollback()
        finally:
            db.close()

    async def _cleanup_body_fields(
        self, db: Session, cutoff_time: datetime, batch_size: int
    ) -> int:
        """压缩 request_body 和 response_body 字段到压缩字段

        逐条处理，确保每条记录都正确更新
        """
        from sqlalchemy import null, update

        total_compressed = 0
        no_progress_count = 0  # 连续无进展计数
        processed_ids: set = set()  # 记录已处理的 ID，防止重复处理

        while True:
            batch_db = create_session()
            try:
                # 1. 查询需要压缩的记录
                # 注意：排除已经是 NULL 或 JSON null 的记录
                records = (
                    batch_db.query(Usage.id, Usage.request_body, Usage.response_body)
                    .filter(Usage.created_at < cutoff_time)
                    .filter((Usage.request_body.isnot(None)) | (Usage.response_body.isnot(None)))
                    .limit(batch_size)
                    .all()
                )

                if not records:
                    break

                # 过滤掉实际值为 None 的记录（JSON null 被解析为 Python None）
                valid_records = [
                    (rid, req, resp)
                    for rid, req, resp in records
                    if req is not None or resp is not None
                ]

                if not valid_records:
                    # 所有记录都是 JSON null，需要清理它们
                    logger.warning(
                        f"检测到 {len(records)} 条记录的 body 字段为 JSON null，进行清理"
                    )
                    for record_id, _, _ in records:
                        batch_db.execute(
                            update(Usage)
                            .where(Usage.id == record_id)
                            .values(request_body=null(), response_body=null())
                        )
                    batch_db.commit()
                    continue

                # 检测是否有重复的 ID（说明更新未生效）
                current_ids = {r[0] for r in valid_records}
                repeated_ids = current_ids & processed_ids
                if repeated_ids:
                    logger.error(
                        f"检测到重复处理的记录 ID: {list(repeated_ids)[:5]}...，"
                        "说明数据库更新未生效，终止循环"
                    )
                    break

                batch_success = 0

                # 2. 逐条更新（确保每条都正确处理）
                for record_id, req_body, resp_body in valid_records:
                    try:
                        # 使用 null() 确保设置的是 SQL NULL 而不是 JSON null
                        result = batch_db.execute(
                            update(Usage)
                            .where(Usage.id == record_id)
                            .values(
                                request_body=null(),
                                response_body=null(),
                                request_body_compressed=compress_json(req_body)
                                if req_body
                                else None,
                                response_body_compressed=compress_json(resp_body)
                                if resp_body
                                else None,
                            )
                        )
                        if result.rowcount > 0:
                            batch_success += 1
                            processed_ids.add(record_id)
                    except Exception as e:
                        logger.warning(f"压缩记录 {record_id} 失败: {e}")
                        continue

                batch_db.commit()

                # 3. 检查是否有实际进展
                if batch_success == 0:
                    no_progress_count += 1
                    if no_progress_count >= 3:
                        logger.error(
                            f"压缩 body 字段连续 {no_progress_count} 批无进展，"
                            "终止循环以避免死循环"
                        )
                        break
                else:
                    no_progress_count = 0  # 重置计数

                total_compressed += batch_success
                logger.debug(
                    f"已压缩 {batch_success} 条记录的 body 字段，累计 {total_compressed} 条"
                )

                await asyncio.sleep(0.1)

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

    async def _cleanup_compressed_fields(
        self, db: Session, cutoff_time: datetime, batch_size: int
    ) -> int:
        """清理压缩字段（90天后删除压缩的body）

        每批使用短生命周期 session，避免 ORM 缓存问题
        """
        from sqlalchemy import null, update

        total_cleaned = 0

        while True:
            batch_db = create_session()
            try:
                # 查询需要清理压缩字段的记录
                records_to_clean = (
                    batch_db.query(Usage.id)
                    .filter(Usage.created_at < cutoff_time)
                    .filter(
                        (Usage.request_body_compressed.isnot(None))
                        | (Usage.response_body_compressed.isnot(None))
                    )
                    .limit(batch_size)
                    .all()
                )

                if not records_to_clean:
                    break

                record_ids = [r.id for r in records_to_clean]

                # 批量更新，使用 null() 确保设置 SQL NULL
                result = batch_db.execute(
                    update(Usage)
                    .where(Usage.id.in_(record_ids))
                    .values(
                        request_body_compressed=null(),
                        response_body_compressed=null(),
                    )
                )

                rows_updated = result.rowcount
                batch_db.commit()

                if rows_updated == 0:
                    logger.warning("清理压缩字段: rowcount=0，可能存在问题")
                    break

                total_cleaned += rows_updated
                logger.debug(f"已清理 {rows_updated} 条记录的压缩字段，累计 {total_cleaned} 条")

                await asyncio.sleep(0.1)

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

    async def _cleanup_header_fields(
        self, db: Session, cutoff_time: datetime, batch_size: int
    ) -> int:
        """清理 request_headers, response_headers 和 provider_request_headers 字段

        每批使用短生命周期 session，避免 ORM 缓存问题
        """
        from sqlalchemy import null, update

        total_cleaned = 0

        while True:
            batch_db = create_session()
            try:
                # 先查询需要清理的记录ID（分批）
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

                # 批量更新，使用 null() 确保设置 SQL NULL
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
                logger.debug(
                    f"已清理 {rows_updated} 条记录的 header 字段，累计 {total_cleaned} 条"
                )

                await asyncio.sleep(0.1)

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

    async def _delete_old_records(self, db: Session, cutoff_time: datetime, batch_size: int) -> int:
        """删除过期的完整记录"""
        total_deleted = 0

        while True:
            try:
                # 查询要删除的记录ID（分批）
                records_to_delete = (
                    db.query(Usage.id)
                    .filter(Usage.created_at < cutoff_time)
                    .limit(batch_size)
                    .all()
                )

                if not records_to_delete:
                    break

                record_ids = [r.id for r in records_to_delete]

                # 执行删除
                result = db.execute(
                    delete(Usage)
                    .where(Usage.id.in_(record_ids))
                    .execution_options(synchronize_session=False)
                )

                rows_deleted = result.rowcount
                db.commit()

                total_deleted += rows_deleted
                logger.debug(f"已删除 {rows_deleted} 条过期记录，累计 {total_deleted} 条")

                await asyncio.sleep(0.1)

            except Exception as e:
                logger.exception(f"删除过期记录失败: {e}")
                try:
                    db.rollback()
                except Exception:
                    pass
                break

        return total_deleted


# 全局单例
_cleanup_scheduler = None


def get_cleanup_scheduler() -> CleanupScheduler:
    """获取清理调度器单例"""
    global _cleanup_scheduler
    if _cleanup_scheduler is None:
        _cleanup_scheduler = CleanupScheduler()
    return _cleanup_scheduler
