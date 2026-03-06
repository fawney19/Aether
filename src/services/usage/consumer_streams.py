"""
Usage Redis Streams consumer.

高性能消费者实现，支持批量处理和单次提交多条记录。
"""

from __future__ import annotations

import asyncio
import json
import os
import socket
import time
from datetime import datetime, timezone
from typing import Any

from redis.exceptions import ConnectionError as RedisConnectionError
from redis.exceptions import ResponseError
from redis.exceptions import TimeoutError as RedisTimeoutError
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from src.clients.redis_client import get_redis_client
from src.config.settings import config
from src.core.logger import logger
from src.database.database import create_session
from src.services.usage.events import UsageEvent, UsageEventType
from src.services.usage.service import UsageService


def _consumer_name() -> str:
    host = socket.gethostname() or "unknown"
    return f"{host}:{os.getpid()}"


def _parse_body(value: Any) -> Any:
    """将 JSON 字符串 body 反序列化为 dict，否则原样返回。

    QueueTelemetryWriter 会将 body 序列化为 JSON 字符串以便传输，
    消费者需要将其反序列化回 dict 以正确存入 JSON 列。
    """
    if value is None or isinstance(value, dict):
        return value
    if isinstance(value, str):
        try:
            return json.loads(value)
        except (json.JSONDecodeError, TypeError):
            # 解析失败，保留原字符串（可能已被截断）
            return value
    return value


def _event_to_record(event: UsageEvent) -> dict[str, Any]:
    """将 UsageEvent 转换为 record_usage_batch 所需的字典格式"""
    data = event.data
    status = "completed"
    if event.event_type == UsageEventType.FAILED:
        status = "failed"
    elif event.event_type == UsageEventType.CANCELLED:
        status = "cancelled"

    finalized_at = None
    if event.timestamp_ms > 0:
        finalized_at = datetime.fromtimestamp(event.timestamp_ms / 1000, tz=timezone.utc)

    return {
        "request_id": event.request_id,
        "user_id": data.get("user_id"),
        "api_key_id": data.get("api_key_id"),
        "provider": data.get("provider") or "unknown",
        "model": data.get("model") or "unknown",
        "input_tokens": data.get("input_tokens") or 0,
        "output_tokens": data.get("output_tokens") or 0,
        "cache_creation_input_tokens": data.get("cache_creation_input_tokens") or 0,
        "cache_read_input_tokens": data.get("cache_read_input_tokens") or 0,
        "cache_creation_input_tokens_5m": data.get("cache_creation_input_tokens_5m") or 0,
        "cache_creation_input_tokens_1h": data.get("cache_creation_input_tokens_1h") or 0,
        "request_type": data.get("request_type") or "chat",
        "api_format": data.get("api_format"),
        "api_family": data.get("api_family"),
        "endpoint_kind": data.get("endpoint_kind"),
        "endpoint_api_format": data.get("endpoint_api_format"),
        "has_format_conversion": data.get("has_format_conversion"),
        "is_stream": data.get("is_stream", True),
        "response_time_ms": data.get("response_time_ms"),
        "first_byte_time_ms": data.get("first_byte_time_ms"),
        "status_code": data.get("status_code") or 200,
        "error_message": data.get("error_message"),
        "metadata": data.get("metadata"),
        "request_headers": data.get("request_headers"),
        "request_body": _parse_body(data.get("request_body")),
        "provider_request_headers": data.get("provider_request_headers"),
        "provider_request_body": _parse_body(data.get("provider_request_body")),
        "response_headers": data.get("response_headers"),
        "client_response_headers": data.get("client_response_headers"),
        "response_body": _parse_body(data.get("response_body")),
        "client_response_body": _parse_body(data.get("client_response_body")),
        "provider_id": data.get("provider_id"),
        "provider_endpoint_id": data.get("provider_endpoint_id"),
        "provider_api_key_id": data.get("provider_api_key_id"),
        "status": status,
        "target_model": data.get("target_model"),
        "finalized_at": finalized_at,
    }


async def ensure_usage_stream_group() -> None:
    redis_client = await get_redis_client(require_redis=False)
    if not redis_client:
        return
    try:
        await redis_client.xgroup_create(
            config.usage_queue_stream_key,
            config.usage_queue_stream_group,
            id="0-0",
            mkstream=True,
        )
        logger.info(f"[usage-queue] Created consumer group {config.usage_queue_stream_group}")
    except ResponseError as exc:
        if "BUSYGROUP" in str(exc):
            return
        raise


class UsageQueueConsumer:
    """Usage 队列消费者

    性能优化：
    - 缓存配置值避免重复属性访问
    - STREAMING 事件使用 pipeline 批量 ACK
    - 记录事件批量写入数据库
    """

    def __init__(self) -> None:
        self._running = False
        self._task: asyncio.Task | None = None
        self._consumer = _consumer_name()
        self._last_claim = 0.0
        self._last_metrics_log = 0.0
        # 缓存配置值，避免热路径上的属性访问开销
        self._stream_key = config.usage_queue_stream_key
        self._stream_group = config.usage_queue_stream_group
        self._batch_size = config.usage_queue_consumer_batch
        self._block_ms = config.usage_queue_consumer_block_ms
        self._claim_idle_ms = config.usage_queue_claim_idle_ms
        self._claim_interval = config.usage_queue_claim_interval_seconds
        self._max_retries = config.usage_queue_max_retries
        self._dlq_key = config.usage_queue_dlq_key
        self._dlq_maxlen = config.usage_queue_dlq_maxlen
        self._metrics_interval = config.usage_queue_metrics_interval_seconds

    @staticmethod
    def _is_duplicate_key_error(exc: IntegrityError) -> bool:
        """判断是否为重复键错误（唯一约束冲突）"""
        err_str = str(exc).lower()
        return "unique" in err_str or "duplicate" in err_str

    async def start(self) -> None:
        if self._running:
            return
        self._running = True
        self._task = asyncio.create_task(self._run(), name="usage-queue-consumer")
        logger.info(f"[usage-queue] Consumer started: {self._consumer}")

    async def stop(self) -> None:
        if not self._running:
            return
        self._running = False
        if self._task:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
        logger.info(f"[usage-queue] Consumer stopped: {self._consumer}")

    async def _run(self) -> None:
        while self._running:
            try:
                redis_client = await get_redis_client(require_redis=False)
                if not redis_client:
                    await asyncio.sleep(1)
                    continue

                await self._maybe_claim_pending(redis_client)
                await self._read_new(redis_client)
                await self._log_metrics(redis_client)
            except asyncio.CancelledError:
                break
            except (RedisTimeoutError, RedisConnectionError) as exc:
                logger.warning(f"[usage-queue] Redis connection issue: {exc}")
                await asyncio.sleep(1)
            except Exception as exc:
                logger.exception(f"[usage-queue] Consumer loop error: {exc}")
                await asyncio.sleep(1)

    async def _maybe_claim_pending(self, redis_client: Any) -> None:
        now = time.time()
        if now - self._last_claim < self._claim_interval:
            return
        self._last_claim = now
        try:
            result = await redis_client.xautoclaim(
                self._stream_key,
                self._stream_group,
                self._consumer,
                min_idle_time=self._claim_idle_ms,
                start_id="0-0",
                count=self._batch_size,
            )
        except ResponseError as exc:
            logger.warning(f"[usage-queue] XAUTOCLAIM failed: {exc}")
            return
        if not result:
            return
        _, messages = result[:2]
        await self._process_messages(redis_client, messages)

    async def _read_new(self, redis_client: Any) -> None:
        try:
            result = await redis_client.xreadgroup(
                groupname=self._stream_group,
                consumername=self._consumer,
                streams={self._stream_key: ">"},
                count=self._batch_size,
                block=self._block_ms,
            )
        except ResponseError as exc:
            if "NOGROUP" in str(exc):
                await ensure_usage_stream_group()
                return
            raise
        if not result:
            return
        for _stream, messages in result:
            await self._process_messages(redis_client, messages)

    async def _process_messages(self, redis_client: Any, messages: list) -> None:
        """批量处理消息，区分 STREAMING（状态更新）和其他事件（记录写入）"""
        if not messages:
            return

        # 分类消息
        streaming_messages: list[tuple[str, UsageEvent]] = []
        record_messages: list[tuple[str, dict[str, Any], UsageEvent]] = []
        failed_messages: list[tuple[str, dict[str, Any], Exception]] = []

        for message_id, fields in messages:
            try:
                event = UsageEvent.from_stream_fields(fields)
                if event.event_type == UsageEventType.STREAMING:
                    streaming_messages.append((message_id, event))
                else:
                    record_messages.append((message_id, fields, event))
            except Exception as exc:
                failed_messages.append((message_id, fields, exc))

        # 批量处理 STREAMING 事件（状态更新）
        if streaming_messages:
            await self._process_streaming_batch(redis_client, streaming_messages)

        # 批量处理记录事件
        if record_messages:
            await self._process_record_batch(redis_client, record_messages)

        # 处理解析失败的消息
        for message_id, fields, exc in failed_messages:
            await self._handle_processing_error(redis_client, message_id, fields, exc)

    async def _process_streaming_batch(
        self,
        redis_client: Any,
        messages: list[tuple[str, UsageEvent]],
    ) -> None:
        """批量处理 STREAMING 事件（状态更新）"""
        success_ids: list[str] = []

        for message_id, event in messages:
            try:
                await self._apply_streaming_event(event)
                success_ids.append(message_id)
            except Exception as exc:
                await self._handle_processing_error(redis_client, message_id, {}, exc)

        # 使用 pipeline 批量 ACK 成功处理的消息
        if success_ids:
            pipe = redis_client.pipeline()
            for message_id in success_ids:
                pipe.xack(self._stream_key, self._stream_group, message_id)
            await pipe.execute()

    async def _process_record_batch(
        self,
        redis_client: Any,
        messages: list[tuple[str, dict[str, Any], UsageEvent]],
    ) -> None:
        """批量处理记录类型的事件"""
        db = create_session()

        try:
            # 准备批量记录数据
            records: list[dict[str, Any]] = []
            message_ids: list[str] = []

            for message_id, fields, event in messages:
                records.append(_event_to_record(event))
                message_ids.append(message_id)

            # 批量写入
            await UsageService.record_usage_batch(db, records)

            # 使用 pipeline 批量 ACK 提升性能
            pipe = redis_client.pipeline()
            for message_id in message_ids:
                pipe.xack(self._stream_key, self._stream_group, message_id)
            await pipe.execute()

            logger.debug(f"[usage-queue] Batch processed {len(records)} records")

        except Exception as exc:
            # 批量处理失败，回退到逐条处理（复用已创建的 db session）
            logger.warning(
                f"[usage-queue] Batch processing failed, falling back to individual: {exc}"
            )
            try:
                db.rollback()  # 清理批量失败的事务状态
            except Exception:
                pass
            success_ids: list[str] = []
            for message_id, fields, event in messages:
                try:
                    await self._apply_record_event(event, db=db)
                    success_ids.append(message_id)
                except IntegrityError as ie:
                    # 重复 request_id 导致的唯一约束冲突，视为成功（记录已存在）
                    try:
                        db.rollback()
                    except Exception:
                        pass
                    if self._is_duplicate_key_error(ie):
                        logger.debug(
                            f"[usage-queue] Duplicate request_id, skipping: {event.request_id}"
                        )
                        success_ids.append(message_id)
                    else:
                        await self._handle_processing_error(redis_client, message_id, fields, ie)
                except Exception as individual_exc:
                    await self._handle_processing_error(
                        redis_client, message_id, fields, individual_exc
                    )
            # 批量 ACK 成功处理的消息
            if success_ids:
                pipe = redis_client.pipeline()
                for message_id in success_ids:
                    pipe.xack(self._stream_key, self._stream_group, message_id)
                await pipe.execute()
        finally:
            db.close()

    async def _handle_processing_error(
        self,
        redis_client: Any,
        message_id: str,
        fields: dict[str, Any],
        error: Exception,
    ) -> None:
        retries = await self._get_delivery_count(redis_client, message_id)
        if retries >= self._max_retries:
            try:
                dlq_fields = dict(fields)
                dlq_fields["source_id"] = message_id
                dlq_fields["error"] = str(error)[:200]
                if self._dlq_maxlen > 0:
                    await redis_client.xadd(
                        self._dlq_key,
                        dlq_fields,
                        maxlen=self._dlq_maxlen,
                        approximate=True,
                    )
                else:
                    await redis_client.xadd(self._dlq_key, dlq_fields)
                await redis_client.xack(self._stream_key, self._stream_group, message_id)
                logger.error(
                    f"[usage-queue] Message moved to DLQ after {retries} attempts: {message_id}"
                )
            except Exception as exc:
                logger.error(f"[usage-queue] Failed to move message to DLQ: {exc}")
        else:
            logger.warning(
                f"[usage-queue] Processing failed (attempt {retries}): {message_id} error={error}"
            )

    async def _get_delivery_count(self, redis_client: Any, message_id: str) -> int:
        try:
            pending = await redis_client.xpending_range(
                self._stream_key,
                self._stream_group,
                min=message_id,
                max=message_id,
                count=1,
            )
            if not pending:
                return 0
            info = pending[0]
            if isinstance(info, dict):
                return int(info.get("times_delivered", 0))
            if isinstance(info, (list, tuple)) and len(info) >= 4:
                return int(info[3])
        except Exception:
            pass
        return 0

    async def _apply_streaming_event(self, event: UsageEvent) -> None:
        """处理 STREAMING 事件（状态更新）"""
        data = event.data
        db = create_session()
        try:
            UsageService.update_usage_status(
                db=db,
                request_id=event.request_id,
                status="streaming",
                provider=data.get("provider"),
                target_model=data.get("target_model"),
                first_byte_time_ms=data.get("first_byte_time_ms"),
                provider_id=data.get("provider_id"),
                provider_endpoint_id=data.get("provider_endpoint_id"),
                provider_api_key_id=data.get("provider_api_key_id"),
                api_format=data.get("api_format"),
                endpoint_api_format=data.get("endpoint_api_format"),
                has_format_conversion=data.get("has_format_conversion"),
                request_headers=data.get("request_headers"),
                request_body=data.get("request_body"),
                provider_request_headers=data.get("provider_request_headers"),
                provider_request_body=data.get("provider_request_body"),
            )
        finally:
            db.close()

    async def _apply_record_event(self, event: UsageEvent, db: Session | None = None) -> None:
        """处理记录类型事件（逐条写入，用于 fallback）

        Args:
            event: 使用事件
            db: 可选的数据库会话。如果提供，复用该会话；否则创建新会话
        """
        from src.models.database import ApiKey, User

        data = event.data
        own_session = db is None
        if own_session:
            db = create_session()
        try:
            status = "completed"
            if event.event_type == UsageEventType.FAILED:
                status = "failed"
            elif event.event_type == UsageEventType.CANCELLED:
                status = "cancelled"

            user = None
            api_key = None
            if data.get("user_id"):
                user = db.query(User).filter(User.id == data["user_id"]).first()
            if data.get("api_key_id"):
                api_key = db.query(ApiKey).filter(ApiKey.id == data["api_key_id"]).first()

            await UsageService.record_usage(
                db=db,
                user=user,
                api_key=api_key,
                provider=data.get("provider") or "unknown",
                model=data.get("model") or "unknown",
                input_tokens=int(data.get("input_tokens") or 0),
                output_tokens=int(data.get("output_tokens") or 0),
                cache_creation_input_tokens=int(data.get("cache_creation_input_tokens") or 0),
                cache_read_input_tokens=int(data.get("cache_read_input_tokens") or 0),
                request_type=data.get("request_type") or "chat",
                api_format=data.get("api_format"),
                endpoint_api_format=data.get("endpoint_api_format"),
                has_format_conversion=bool(data.get("has_format_conversion") or False),
                is_stream=bool(data.get("is_stream", True)),
                response_time_ms=data.get("response_time_ms"),
                first_byte_time_ms=data.get("first_byte_time_ms"),
                status_code=int(data.get("status_code") or 200),
                error_message=data.get("error_message"),
                metadata=data.get("metadata"),
                request_headers=data.get("request_headers"),
                request_body=_parse_body(data.get("request_body")),
                provider_request_headers=data.get("provider_request_headers"),
                provider_request_body=_parse_body(data.get("provider_request_body")),
                response_headers=data.get("response_headers"),
                client_response_headers=data.get("client_response_headers"),
                response_body=_parse_body(data.get("response_body")),
                client_response_body=_parse_body(data.get("client_response_body")),
                request_id=event.request_id,
                provider_id=data.get("provider_id"),
                provider_endpoint_id=data.get("provider_endpoint_id"),
                provider_api_key_id=data.get("provider_api_key_id"),
                status=status,
                target_model=data.get("target_model"),
                finalized_at=(
                    datetime.fromtimestamp(event.timestamp_ms / 1000, tz=timezone.utc)
                    if event.timestamp_ms > 0
                    else None
                ),
            )
        finally:
            if own_session:
                db.close()

    async def _apply_event(self, event: UsageEvent) -> None:
        """处理单个事件（兼容旧接口，用于测试）"""
        if event.event_type == UsageEventType.STREAMING:
            await self._apply_streaming_event(event)
        else:
            await self._apply_record_event(event)

    async def _log_metrics(self, redis_client: Any) -> None:
        now = time.time()
        if now - self._last_metrics_log < self._metrics_interval:
            return
        self._last_metrics_log = now
        try:
            # 使用 XINFO GROUPS 获取更准确的 lag（未处理消息数）
            groups_info = await redis_client.xinfo_groups(self._stream_key)
            lag = 0
            pending_count = 0
            for group in groups_info:
                if isinstance(group, dict) and group.get("name") == self._stream_group:
                    lag = group.get("lag", 0) or 0
                    pending_count = group.get("pending", 0) or 0
                    break
            # lag=未读消息数, pending=已读但未ACK的消息数
            if lag > 0 or pending_count > 0:
                logger.info(f"[usage-queue] lag={lag} pending={pending_count}")
        except Exception as exc:
            logger.debug(f"[usage-queue] metrics log failed: {exc}")


_consumer_instance: UsageQueueConsumer | None = None


async def start_usage_queue_consumer() -> UsageQueueConsumer | None:
    global _consumer_instance
    if not config.usage_queue_enabled:
        return None
    await ensure_usage_stream_group()
    if _consumer_instance is None:
        _consumer_instance = UsageQueueConsumer()
    await _consumer_instance.start()
    return _consumer_instance


async def stop_usage_queue_consumer() -> None:
    global _consumer_instance
    if _consumer_instance:
        await _consumer_instance.stop()
        _consumer_instance = None
