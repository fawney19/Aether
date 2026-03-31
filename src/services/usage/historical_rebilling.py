from __future__ import annotations

import asyncio
from datetime import date, datetime, timedelta, timezone
from decimal import Decimal
from typing import Any

from sqlalchemy import and_, or_, update
from sqlalchemy.orm import Session

from src.core.api_format.signature import normalize_signature_key
from src.core.logger import logger
from src.database import create_session
from src.models.database import ApiKey, Provider, ProviderAPIKey, Usage
from src.services.billing.precision import to_money_decimal
from src.services.billing.service import BillingService
from src.services.billing.token_normalization import normalize_input_tokens_for_billing
from src.services.system.config import SystemConfigService
from src.services.usage._recording_helpers import sanitize_request_metadata
from src.services.wallet import WalletDailyUsageLedgerService, WalletService


class HistoricalUsageRebillingService:
    """升级后一次性的历史 Usage 重算维护任务。"""

    TARGET_VERSION = "2026-03-31-usage-billing-recalc-v1"
    STATE_KEY = "historical_usage_rebilling_state"
    ENABLED_KEY = "historical_usage_rebilling_enabled"
    USAGE_BATCH_SIZE_KEY = "historical_usage_rebilling_usage_batch_size"
    LEDGER_BATCH_DAYS_KEY = "historical_usage_rebilling_ledger_batch_days"
    PAUSE_MS_KEY = "historical_usage_rebilling_pause_ms"

    DEFAULT_USAGE_BATCH_SIZE = 100
    DEFAULT_LEDGER_BATCH_DAYS = 30
    DEFAULT_PAUSE_MS = 100

    @classmethod
    async def run_once_in_background(cls) -> None:
        """后台循环执行，直到本次维护版本完成。"""
        while True:
            finished = await asyncio.to_thread(cls._run_single_step)
            if finished:
                return
            await asyncio.sleep(cls._get_pause_seconds())

    @classmethod
    def _get_pause_seconds(cls) -> float:
        db = create_session()
        try:
            pause_ms = int(
                SystemConfigService.get_config(db, cls.PAUSE_MS_KEY, cls.DEFAULT_PAUSE_MS)
                or cls.DEFAULT_PAUSE_MS
            )
        except Exception:
            pause_ms = cls.DEFAULT_PAUSE_MS
        finally:
            db.close()
        return max(pause_ms, 0) / 1000.0

    @classmethod
    def _run_single_step(cls) -> bool:
        db = create_session()
        try:
            if not bool(SystemConfigService.get_config(db, cls.ENABLED_KEY, True)):
                logger.info("历史计费重算维护已禁用，跳过")
                return True

            state = cls._load_state(db)
            if cls._is_completed_state(state):
                return True

            if not state or state.get("version") != cls.TARGET_VERSION:
                state = cls._build_initial_state()
                cls._save_state(db, state)
                logger.info(
                    "初始化历史计费重算维护任务: version={}, cutoff={}",
                    cls.TARGET_VERSION,
                    state.get("cutoff_created_at"),
                )
                return False

            phase = str(state.get("phase") or "usage_reprice")
            if phase == "usage_reprice":
                finished = cls._process_usage_batch(db, state)
            elif phase == "ledger_rebuild":
                finished = cls._process_ledger_batch(db, state)
            else:
                state["status"] = "completed"
                state["phase"] = "completed"
                state["completed_at"] = datetime.now(timezone.utc).isoformat()
                cls._save_state(db, state)
                return True

            if finished:
                state["updated_at"] = datetime.now(timezone.utc).isoformat()
                cls._save_state(db, state)
                return cls._is_completed_state(state)

            state["updated_at"] = datetime.now(timezone.utc).isoformat()
            cls._save_state(db, state)
            return False
        except Exception as exc:
            logger.exception("历史计费重算维护执行失败: {}", exc)
            try:
                db.rollback()
            except Exception:
                pass
            cls._mark_failed_state(exc)
            return True
        finally:
            db.close()

    @classmethod
    def _build_initial_state(cls) -> dict[str, Any]:
        now = datetime.now(timezone.utc)
        return {
            "version": cls.TARGET_VERSION,
            "status": "running",
            "phase": "usage_reprice",
            "cutoff_created_at": now.isoformat(),
            "usage_cursor": None,
            "ledger_current_date": None,
            "ledger_end_date": None,
            "processed_count": 0,
            "updated_count": 0,
            "skipped_count": 0,
            "usage_batch_count": 0,
            "ledger_batch_count": 0,
            "started_at": now.isoformat(),
            "updated_at": now.isoformat(),
            "completed_at": None,
            "last_error": None,
        }

    @classmethod
    def _load_state(cls, db: Session) -> dict[str, Any] | None:
        value = SystemConfigService.get_config(db, cls.STATE_KEY)
        return value if isinstance(value, dict) else None

    @classmethod
    def _save_state(cls, db: Session, state: dict[str, Any]) -> None:
        SystemConfigService.set_config(
            db,
            cls.STATE_KEY,
            state,
            description="历史计费重算后台维护状态（升级后一次性任务）",
        )

    @classmethod
    def _mark_failed_state(cls, exc: Exception) -> None:
        db = create_session()
        try:
            state = cls._load_state(db) or cls._build_initial_state()
            state["version"] = cls.TARGET_VERSION
            state["status"] = "failed"
            state["last_error"] = str(exc)
            state["updated_at"] = datetime.now(timezone.utc).isoformat()
            cls._save_state(db, state)
        except Exception:
            logger.exception("写入历史计费重算失败状态时出错")
        finally:
            db.close()

    @classmethod
    def _is_completed_state(cls, state: dict[str, Any] | None) -> bool:
        return bool(
            state
            and state.get("version") == cls.TARGET_VERSION
            and state.get("status") == "completed"
        )

    @classmethod
    def _process_usage_batch(cls, db: Session, state: dict[str, Any]) -> bool:
        cutoff_created_at = cls._parse_datetime(state.get("cutoff_created_at"))
        if cutoff_created_at is None:
            cutoff_created_at = datetime.now(timezone.utc)
            state["cutoff_created_at"] = cutoff_created_at.isoformat()

        batch_size = int(
            SystemConfigService.get_config(
                db, cls.USAGE_BATCH_SIZE_KEY, cls.DEFAULT_USAGE_BATCH_SIZE
            )
            or cls.DEFAULT_USAGE_BATCH_SIZE
        )
        batch_size = max(1, batch_size)

        cursor = state.get("usage_cursor")
        query = (
            db.query(Usage)
            .filter(
                Usage.billing_status == "settled",
                Usage.created_at < cutoff_created_at,
            )
            .order_by(Usage.created_at.asc(), Usage.id.asc())
        )

        cursor_created_at = cls._parse_datetime((cursor or {}).get("created_at"))
        cursor_id = (cursor or {}).get("id")
        if cursor_created_at is not None and isinstance(cursor_id, str) and cursor_id:
            query = query.filter(
                or_(
                    Usage.created_at > cursor_created_at,
                    and_(Usage.created_at == cursor_created_at, Usage.id > cursor_id),
                )
            )

        rows = query.limit(batch_size).all()
        if not rows:
            cls._prepare_ledger_phase(db, state, cutoff_created_at=cutoff_created_at)
            return False

        processed = 0
        updated = 0
        skipped = 0

        for usage in rows:
            processed += 1
            changed = cls._recalculate_usage(db, usage)
            if changed is True:
                updated += 1
            else:
                skipped += 1

        last_usage = rows[-1]
        state["usage_cursor"] = {
            "created_at": cls._ensure_datetime(last_usage.created_at).isoformat(),
            "id": last_usage.id,
        }
        state["processed_count"] = int(state.get("processed_count") or 0) + processed
        state["updated_count"] = int(state.get("updated_count") or 0) + updated
        state["skipped_count"] = int(state.get("skipped_count") or 0) + skipped
        state["usage_batch_count"] = int(state.get("usage_batch_count") or 0) + 1
        db.flush()
        logger.info(
            "历史计费重算批次完成: processed={}, updated={}, skipped={}, phase=usage_reprice",
            processed,
            updated,
            skipped,
        )
        return False

    @classmethod
    def _prepare_ledger_phase(
        cls,
        db: Session,
        state: dict[str, Any],
        *,
        cutoff_created_at: datetime,
    ) -> None:
        tz = WalletDailyUsageLedgerService.get_timezone()
        cutoff_local_date = cutoff_created_at.astimezone(tz).date()

        rows = (
            db.query(Usage.finalized_at)
            .filter(
                Usage.billing_status == "settled",
                Usage.wallet_id.isnot(None),
                Usage.finalized_at.isnot(None),
                Usage.created_at < cutoff_created_at,
            )
            .order_by(Usage.finalized_at.asc())
            .limit(1)
            .all()
        )
        if not rows:
            state["phase"] = "completed"
            state["status"] = "completed"
            state["completed_at"] = datetime.now(timezone.utc).isoformat()
            return

        earliest_finalized_at = cls._ensure_datetime(rows[0][0])
        start_date = earliest_finalized_at.astimezone(tz).date()
        end_date = cutoff_local_date - timedelta(days=1)

        if start_date > end_date:
            state["phase"] = "completed"
            state["status"] = "completed"
            state["completed_at"] = datetime.now(timezone.utc).isoformat()
            return

        state["phase"] = "ledger_rebuild"
        state["ledger_current_date"] = start_date.isoformat()
        state["ledger_end_date"] = end_date.isoformat()
        logger.info(
            "历史计费重算进入账本刷新阶段: start_date={}, end_date={}",
            state["ledger_current_date"],
            state["ledger_end_date"],
        )

    @classmethod
    def _process_ledger_batch(cls, db: Session, state: dict[str, Any]) -> bool:
        current_date = cls._parse_date(state.get("ledger_current_date"))
        end_date = cls._parse_date(state.get("ledger_end_date"))
        if current_date is None or end_date is None or current_date > end_date:
            state["phase"] = "completed"
            state["status"] = "completed"
            state["completed_at"] = datetime.now(timezone.utc).isoformat()
            return True

        batch_days = int(
            SystemConfigService.get_config(
                db, cls.LEDGER_BATCH_DAYS_KEY, cls.DEFAULT_LEDGER_BATCH_DAYS
            )
            or cls.DEFAULT_LEDGER_BATCH_DAYS
        )
        batch_days = max(1, batch_days)

        processed_days = 0
        day = current_date
        while day <= end_date and processed_days < batch_days:
            WalletDailyUsageLedgerService.aggregate_day(db, day, commit=False)
            processed_days += 1
            day += timedelta(days=1)

        state["ledger_batch_count"] = int(state.get("ledger_batch_count") or 0) + 1
        if day > end_date:
            state["phase"] = "completed"
            state["status"] = "completed"
            state["completed_at"] = datetime.now(timezone.utc).isoformat()
            state["ledger_current_date"] = None
            logger.info("历史计费重算维护已完成: version={}", cls.TARGET_VERSION)
            return True

        state["ledger_current_date"] = day.isoformat()
        logger.info(
            "历史计费重算账本刷新批次完成: processed_days={}, next_date={}",
            processed_days,
            state["ledger_current_date"],
        )
        return False

    @classmethod
    def _recalculate_usage(cls, db: Session, usage: Usage) -> bool | None:
        previous_total_cost = to_money_decimal(usage.total_cost_usd or 0)
        previous_actual_total_cost = to_money_decimal(usage.actual_total_cost_usd or 0)

        payload = cls._build_recalculated_payload(db, usage)
        if payload is None:
            return None

        next_total_cost = to_money_decimal(payload["total_cost_usd"] or 0)
        next_actual_total_cost = to_money_decimal(payload["actual_total_cost_usd"] or 0)
        delta = next_total_cost - previous_total_cost
        actual_delta = next_actual_total_cost - previous_actual_total_cost

        changed = False
        for key, value in payload.items():
            current = getattr(usage, key)
            if current != value:
                setattr(usage, key, value)
                changed = True

        if delta != Decimal("0"):
            WalletService.reconcile_usage_charge_delta(
                db,
                usage=usage,
                previous_amount_usd=previous_total_cost,
                next_amount_usd=next_total_cost,
            )
            cls._increment_api_key_total_cost(db, usage.api_key_id, delta)
        if actual_delta != Decimal("0"):
            cls._increment_provider_api_key_total_cost(db, usage.provider_api_key_id, actual_delta)
            cls._increment_provider_period_usage(db, usage, actual_delta)

        return changed or delta != Decimal("0") or actual_delta != Decimal("0")

    @classmethod
    def _build_recalculated_payload(cls, db: Session, usage: Usage) -> dict[str, Any] | None:
        billing_task_type = str(usage.request_type or "").lower()
        if billing_task_type not in {"chat", "cli", "video", "image", "audio"}:
            billing_task_type = "chat"

        if billing_task_type in {"video", "image", "audio"}:
            return cls._build_formula_payload(db, usage, billing_task_type)
        return cls._build_token_payload(db, usage, billing_task_type)

    @classmethod
    def _build_formula_payload(
        cls,
        db: Session,
        usage: Usage,
        billing_task_type: str,
    ) -> dict[str, Any] | None:
        if not usage.provider_id or not usage.model:
            return None

        metadata = cls._copy_metadata(usage)
        billing_snapshot = metadata.get("billing_snapshot") if isinstance(metadata, dict) else None
        dims = cls._extract_billing_dimensions(billing_snapshot)
        if not dims:
            return None

        billing = BillingService(db)
        result = billing.calculate(
            task_type=billing_task_type,
            model=usage.model,
            provider_id=usage.provider_id,
            dimensions=dims,
            strict_mode=None,
        )
        snap = result.snapshot
        if getattr(snap, "status", None) != "complete":
            return None

        total_cost = to_money_decimal(snap.total_cost or 0.0)
        request_cost = total_cost

        actual_rate_multiplier = cls._get_usage_rate_multiplier(usage)
        is_free_tier = cls._usage_is_free_tier(usage)
        actual_request_cost = Decimal("0") if is_free_tier else request_cost * actual_rate_multiplier
        actual_total_cost = Decimal("0") if is_free_tier else total_cost * actual_rate_multiplier

        metadata["billing_snapshot"] = snap.to_dict()
        metadata["billing_updated_at"] = datetime.now(timezone.utc).isoformat()
        metadata["billing_recalc_version"] = cls.TARGET_VERSION
        metadata["billing_recalc_pricing_source"] = "latest_model_pricing"
        sanitized_metadata = sanitize_request_metadata(metadata)

        return {
            "input_cost_usd": Decimal("0"),
            "output_cost_usd": Decimal("0"),
            "cache_cost_usd": Decimal("0"),
            "cache_creation_cost_usd": Decimal("0"),
            "cache_creation_cost_usd_5m": Decimal("0"),
            "cache_creation_cost_usd_1h": Decimal("0"),
            "cache_read_cost_usd": Decimal("0"),
            "request_cost_usd": request_cost,
            "total_cost_usd": total_cost,
            "actual_input_cost_usd": Decimal("0"),
            "actual_output_cost_usd": Decimal("0"),
            "actual_cache_creation_cost_usd": Decimal("0"),
            "actual_cache_creation_cost_usd_5m": Decimal("0"),
            "actual_cache_creation_cost_usd_1h": Decimal("0"),
            "actual_cache_read_cost_usd": Decimal("0"),
            "actual_cache_cost_usd": Decimal("0"),
            "actual_request_cost_usd": actual_request_cost,
            "actual_total_cost_usd": actual_total_cost,
            "input_price_per_1m": None,
            "output_price_per_1m": None,
            "cache_creation_price_per_1m": None,
            "cache_creation_price_per_1m_5m": None,
            "cache_creation_price_per_1m_1h": None,
            "cache_read_price_per_1m": None,
            "price_per_request": request_cost,
            "request_metadata": sanitized_metadata,
        }

    @classmethod
    def _build_token_payload(
        cls,
        db: Session,
        usage: Usage,
        billing_task_type: str,
    ) -> dict[str, Any] | None:
        if not usage.provider_id or not usage.model:
            return None

        billing_api_format = cls._resolve_billing_api_format(usage)
        raw_input_tokens = cls._get_usage_raw_input_tokens(usage)
        cache_read_tokens = cls._to_int(usage.cache_read_input_tokens)
        input_tokens_for_billing = normalize_input_tokens_for_billing(
            billing_api_format,
            raw_input_tokens,
            cache_read_tokens,
        )

        is_failed_request = cls._to_int(usage.status_code) >= 400 or bool(usage.error_message)
        request_count = 0 if is_failed_request else 1
        cache_creation_tokens = cls._to_int(usage.cache_creation_input_tokens)
        ttl_5m_tokens = max(
            min(cls._to_int(usage.cache_creation_input_tokens_5m), cache_creation_tokens),
            0,
        )
        ttl_1h_tokens = max(
            min(
                cls._to_int(usage.cache_creation_input_tokens_1h),
                max(cache_creation_tokens - ttl_5m_tokens, 0),
            ),
            0,
        )
        remaining_cache_creation_tokens = max(
            cache_creation_tokens - ttl_5m_tokens - ttl_1h_tokens,
            0,
        )
        has_cache_tokens = bool(cache_creation_tokens > 0 or cache_read_tokens > 0)
        effective_cache_ttl_minutes = cls._determine_effective_cache_ttl_minutes(
            db,
            usage,
            has_cache_tokens=has_cache_tokens,
            ttl_5m_tokens=ttl_5m_tokens,
            ttl_1h_tokens=ttl_1h_tokens,
        )

        billing = BillingService(db)
        total_input_context = input_tokens_for_billing + cache_creation_tokens + cache_read_tokens

        def _make_dims(
            *,
            input_tokens: int,
            output_tokens: int,
            cache_creation_input_tokens: int,
            cache_read_input_tokens: int,
            request_count_value: int,
            cache_ttl_minutes: int | None,
        ) -> dict[str, Any]:
            dims: dict[str, Any] = {
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "cache_creation_input_tokens": cache_creation_input_tokens,
                "cache_read_input_tokens": cache_read_input_tokens,
                "request_count": request_count_value,
                "total_input_context": total_input_context,
            }
            if cache_ttl_minutes is not None:
                dims["cache_ttl_minutes"] = cache_ttl_minutes
            return dims

        def _run_billing(dimensions: dict[str, Any]) -> Any:
            return billing.calculate(
                task_type=billing_task_type,
                model=usage.model,
                provider_id=usage.provider_id or "",
                dimensions=dimensions,
                strict_mode=None,
            ).snapshot

        mixed_cache_creation_ttls = ttl_5m_tokens > 0 and ttl_1h_tokens > 0

        input_cost = Decimal("0")
        output_cost = Decimal("0")
        cache_creation_cost = Decimal("0")
        cache_creation_cost_5m = Decimal("0")
        cache_creation_cost_1h = Decimal("0")
        cache_read_cost = Decimal("0")
        request_cost = Decimal("0")
        input_price = None
        output_price = None
        cache_creation_price = None
        cache_creation_price_5m = None
        cache_creation_price_1h = None
        cache_read_price = None
        request_price = None

        if mixed_cache_creation_ttls:
            common_snap = _run_billing(
                _make_dims(
                    input_tokens=input_tokens_for_billing,
                    output_tokens=cls._to_int(usage.output_tokens),
                    cache_creation_input_tokens=0,
                    cache_read_input_tokens=cache_read_tokens,
                    request_count_value=request_count,
                    cache_ttl_minutes=effective_cache_ttl_minutes,
                )
            )
            cache_5m_snap = _run_billing(
                _make_dims(
                    input_tokens=0,
                    output_tokens=0,
                    cache_creation_input_tokens=ttl_5m_tokens,
                    cache_read_input_tokens=0,
                    request_count_value=0,
                    cache_ttl_minutes=5,
                )
            )
            cache_1h_snap = _run_billing(
                _make_dims(
                    input_tokens=0,
                    output_tokens=0,
                    cache_creation_input_tokens=ttl_1h_tokens,
                    cache_read_input_tokens=0,
                    request_count_value=0,
                    cache_ttl_minutes=60,
                )
            )
            remaining_snap = None
            if remaining_cache_creation_tokens > 0:
                remaining_snap = _run_billing(
                    _make_dims(
                        input_tokens=0,
                        output_tokens=0,
                        cache_creation_input_tokens=remaining_cache_creation_tokens,
                        cache_read_input_tokens=0,
                        request_count_value=0,
                        cache_ttl_minutes=effective_cache_ttl_minutes,
                    )
                )

            if any(
                getattr(snap, "status", None) != "complete"
                for snap in [common_snap, cache_5m_snap, cache_1h_snap]
            ):
                return None
            if remaining_snap is not None and getattr(remaining_snap, "status", None) != "complete":
                return None

            common_breakdown = common_snap.cost_breakdown or {}
            common_vars = common_snap.resolved_variables or {}
            cache_5m_breakdown = cache_5m_snap.cost_breakdown or {}
            cache_5m_vars = cache_5m_snap.resolved_variables or {}
            cache_1h_breakdown = cache_1h_snap.cost_breakdown or {}
            cache_1h_vars = cache_1h_snap.resolved_variables or {}
            remaining_breakdown = remaining_snap.cost_breakdown or {} if remaining_snap else {}
            remaining_vars = remaining_snap.resolved_variables or {} if remaining_snap else {}

            input_cost = to_money_decimal(common_breakdown.get("input_cost", 0.0) or 0.0)
            output_cost = to_money_decimal(common_breakdown.get("output_cost", 0.0) or 0.0)
            cache_read_cost = to_money_decimal(common_breakdown.get("cache_read_cost", 0.0) or 0.0)
            request_cost = to_money_decimal(common_breakdown.get("request_cost", 0.0) or 0.0)
            cache_creation_cost_5m = to_money_decimal(
                cache_5m_breakdown.get("cache_creation_cost", 0.0) or 0.0
            )
            cache_creation_cost_1h = to_money_decimal(
                cache_1h_breakdown.get("cache_creation_cost", 0.0) or 0.0
            )
            remaining_cache_creation_cost = to_money_decimal(
                remaining_breakdown.get("cache_creation_cost", 0.0) or 0.0
            )
            cache_creation_cost = (
                cache_creation_cost_5m + cache_creation_cost_1h + remaining_cache_creation_cost
            )

            input_price = cls._as_money_or_none(common_vars.get("input_price_per_1m"))
            output_price = cls._as_money_or_none(common_vars.get("output_price_per_1m"))
            cache_creation_price_5m = cls._as_money_or_none(
                cache_5m_vars.get("cache_creation_price_per_1m")
            )
            cache_creation_price_1h = cls._as_money_or_none(
                cache_1h_vars.get("cache_creation_price_per_1m")
            )
            remaining_cache_creation_price = cls._as_money_or_none(
                remaining_vars.get("cache_creation_price_per_1m")
            )
            cache_read_price = cls._as_money_or_none(common_vars.get("cache_read_price_per_1m"))
            request_price = cls._as_money_or_none(common_vars.get("price_per_request"))

            cache_creation_price = remaining_cache_creation_price
            if (
                remaining_cache_creation_tokens == 0
                and cache_creation_price_5m is not None
                and cache_creation_price_1h is not None
                and cache_creation_price_5m == cache_creation_price_1h
            ):
                cache_creation_price = cache_creation_price_5m

            base_snapshot = cls._snapshot_to_dict(common_snap)
            if not base_snapshot:
                base_snapshot = cls._snapshot_to_dict(cache_5m_snap)
            if not base_snapshot:
                base_snapshot = cls._snapshot_to_dict(cache_1h_snap)

            resolved_dimensions = {
                "input_tokens": input_tokens_for_billing,
                "output_tokens": cls._to_int(usage.output_tokens),
                "cache_creation_input_tokens": cache_creation_tokens,
                "cache_read_input_tokens": cache_read_tokens,
                "request_count": request_count,
                "total_input_context": total_input_context,
            }
            if effective_cache_ttl_minutes is not None:
                resolved_dimensions["cache_ttl_minutes"] = effective_cache_ttl_minutes

            resolved_variables = dict(base_snapshot.get("resolved_variables") or {})
            resolved_variables["input_price_per_1m"] = cls._serialize_money_or_none(input_price)
            resolved_variables["output_price_per_1m"] = cls._serialize_money_or_none(output_price)
            resolved_variables["cache_read_price_per_1m"] = cls._serialize_money_or_none(cache_read_price)
            resolved_variables["price_per_request"] = cls._serialize_money_or_none(request_price)
            resolved_variables["cache_creation_price_per_1m_5m"] = cls._serialize_money_or_none(
                cache_creation_price_5m
            )
            resolved_variables["cache_creation_price_per_1m_1h"] = cls._serialize_money_or_none(
                cache_creation_price_1h
            )
            if cache_creation_price is None:
                resolved_variables.pop("cache_creation_price_per_1m", None)
            else:
                resolved_variables["cache_creation_price_per_1m"] = cls._serialize_money_or_none(
                    cache_creation_price
                )

            billing_snapshot_payload = {
                **base_snapshot,
                "resolved_dimensions": resolved_dimensions,
                "dimensions_used": resolved_dimensions,
                "resolved_variables": resolved_variables,
                "cost_breakdown": {
                    "input_cost": float(input_cost),
                    "output_cost": float(output_cost),
                    "cache_creation_cost": float(cache_creation_cost),
                    "cache_read_cost": float(cache_read_cost),
                    "request_cost": float(request_cost),
                },
                "total_cost": float(
                    input_cost + output_cost + cache_creation_cost + cache_read_cost + request_cost
                ),
                "cost": float(
                    input_cost + output_cost + cache_creation_cost + cache_read_cost + request_cost
                ),
                "cache_creation_split": {
                    "5m": {
                        "tokens": ttl_5m_tokens,
                        "price_per_1m": cls._serialize_money_or_none(cache_creation_price_5m),
                        "cost": float(cache_creation_cost_5m),
                    },
                    "1h": {
                        "tokens": ttl_1h_tokens,
                        "price_per_1m": cls._serialize_money_or_none(cache_creation_price_1h),
                        "cost": float(cache_creation_cost_1h),
                    },
                },
            }
            if remaining_cache_creation_tokens > 0:
                billing_snapshot_payload["cache_creation_split"]["other"] = {
                    "tokens": remaining_cache_creation_tokens,
                    "price_per_1m": cls._serialize_money_or_none(remaining_cache_creation_price),
                    "cost": float(remaining_cache_creation_cost),
                }
        else:
            snap = _run_billing(
                _make_dims(
                    input_tokens=input_tokens_for_billing,
                    output_tokens=cls._to_int(usage.output_tokens),
                    cache_creation_input_tokens=cache_creation_tokens,
                    cache_read_input_tokens=cache_read_tokens,
                    request_count_value=request_count,
                    cache_ttl_minutes=effective_cache_ttl_minutes,
                )
            )
            if getattr(snap, "status", None) != "complete":
                return None

            breakdown = snap.cost_breakdown or {}
            vars_map = snap.resolved_variables or {}
            input_cost = to_money_decimal(breakdown.get("input_cost", 0.0) or 0.0)
            output_cost = to_money_decimal(breakdown.get("output_cost", 0.0) or 0.0)
            cache_creation_cost = to_money_decimal(breakdown.get("cache_creation_cost", 0.0) or 0.0)
            cache_read_cost = to_money_decimal(breakdown.get("cache_read_cost", 0.0) or 0.0)
            request_cost = to_money_decimal(breakdown.get("request_cost", 0.0) or 0.0)
            input_price = cls._as_money_or_none(vars_map.get("input_price_per_1m"))
            output_price = cls._as_money_or_none(vars_map.get("output_price_per_1m"))
            cache_creation_price = cls._as_money_or_none(
                vars_map.get("cache_creation_price_per_1m")
            )
            cache_read_price = cls._as_money_or_none(vars_map.get("cache_read_price_per_1m"))
            request_price = cls._as_money_or_none(vars_map.get("price_per_request"))
            billing_snapshot_payload = cls._snapshot_to_dict(snap)

            if cache_creation_tokens > 0:
                if ttl_5m_tokens > 0 and ttl_1h_tokens == 0:
                    cache_creation_cost_5m = cache_creation_cost
                    cache_creation_price_5m = cache_creation_price
                elif ttl_1h_tokens > 0 and ttl_5m_tokens == 0:
                    cache_creation_cost_1h = cache_creation_cost
                    cache_creation_price_1h = cache_creation_price
                elif effective_cache_ttl_minutes is not None and effective_cache_ttl_minutes <= 5:
                    cache_creation_cost_5m = cache_creation_cost
                    cache_creation_price_5m = cache_creation_price
                else:
                    cache_creation_cost_1h = cache_creation_cost
                    cache_creation_price_1h = cache_creation_price

        cache_cost = cache_creation_cost + cache_read_cost
        total_cost = input_cost + output_cost + cache_creation_cost + cache_read_cost + request_cost

        actual_rate_multiplier = cls._get_usage_rate_multiplier(usage)
        is_free_tier = cls._usage_is_free_tier(usage)
        if is_free_tier:
            actual_input_cost = Decimal("0")
            actual_output_cost = Decimal("0")
            actual_cache_creation_cost = Decimal("0")
            actual_cache_creation_cost_5m = Decimal("0")
            actual_cache_creation_cost_1h = Decimal("0")
            actual_cache_read_cost = Decimal("0")
            actual_cache_cost = Decimal("0")
            actual_request_cost = Decimal("0")
            actual_total_cost = Decimal("0")
        else:
            actual_input_cost = input_cost * actual_rate_multiplier
            actual_output_cost = output_cost * actual_rate_multiplier
            actual_cache_creation_cost = cache_creation_cost * actual_rate_multiplier
            actual_cache_creation_cost_5m = cache_creation_cost_5m * actual_rate_multiplier
            actual_cache_creation_cost_1h = cache_creation_cost_1h * actual_rate_multiplier
            actual_cache_read_cost = cache_read_cost * actual_rate_multiplier
            actual_cache_cost = actual_cache_creation_cost + actual_cache_read_cost
            actual_request_cost = request_cost * actual_rate_multiplier
            actual_total_cost = total_cost * actual_rate_multiplier

        metadata = cls._copy_metadata(usage)
        metadata["billing_snapshot"] = billing_snapshot_payload
        metadata["billing_updated_at"] = datetime.now(timezone.utc).isoformat()
        metadata["billing_recalc_version"] = cls.TARGET_VERSION
        metadata["billing_recalc_pricing_source"] = "latest_model_pricing"
        sanitized_metadata = sanitize_request_metadata(metadata)

        return {
            "input_cost_usd": input_cost,
            "output_cost_usd": output_cost,
            "cache_cost_usd": cache_cost,
            "cache_creation_cost_usd": cache_creation_cost,
            "cache_creation_cost_usd_5m": cache_creation_cost_5m,
            "cache_creation_cost_usd_1h": cache_creation_cost_1h,
            "cache_read_cost_usd": cache_read_cost,
            "request_cost_usd": request_cost,
            "total_cost_usd": total_cost,
            "actual_input_cost_usd": actual_input_cost,
            "actual_output_cost_usd": actual_output_cost,
            "actual_cache_creation_cost_usd": actual_cache_creation_cost,
            "actual_cache_creation_cost_usd_5m": actual_cache_creation_cost_5m,
            "actual_cache_creation_cost_usd_1h": actual_cache_creation_cost_1h,
            "actual_cache_read_cost_usd": actual_cache_read_cost,
            "actual_cache_cost_usd": actual_cache_cost,
            "actual_request_cost_usd": actual_request_cost,
            "actual_total_cost_usd": actual_total_cost,
            "input_price_per_1m": input_price,
            "output_price_per_1m": output_price,
            "cache_creation_price_per_1m": cache_creation_price,
            "cache_creation_price_per_1m_5m": cache_creation_price_5m,
            "cache_creation_price_per_1m_1h": cache_creation_price_1h,
            "cache_read_price_per_1m": cache_read_price,
            "price_per_request": request_price,
            "request_metadata": sanitized_metadata,
        }

    @classmethod
    def _determine_effective_cache_ttl_minutes(
        cls,
        db: Session,
        usage: Usage,
        *,
        has_cache_tokens: bool,
        ttl_5m_tokens: int,
        ttl_1h_tokens: int,
    ) -> int | None:
        metadata = cls._copy_metadata(usage)
        billing_snapshot = metadata.get("billing_snapshot") if isinstance(metadata, dict) else None
        dims = cls._extract_billing_dimensions(billing_snapshot)
        ttl_from_snapshot = dims.get("cache_ttl_minutes") if isinstance(dims, dict) else None
        try:
            if ttl_from_snapshot is not None:
                return int(ttl_from_snapshot)
        except Exception:
            pass

        if has_cache_tokens:
            if ttl_1h_tokens > 0 and ttl_5m_tokens == 0:
                return 60
            if ttl_5m_tokens > 0 and ttl_1h_tokens == 0:
                return 5
            if ttl_1h_tokens > 0:
                return 60

        if has_cache_tokens and usage.provider_api_key_id:
            try:
                key_ttl = (
                    db.query(ProviderAPIKey.cache_ttl_minutes)
                    .filter(ProviderAPIKey.id == usage.provider_api_key_id)
                    .scalar()
                )
                if key_ttl is not None:
                    key_ttl_int = int(key_ttl)
                    if key_ttl_int >= 0:
                        return key_ttl_int
            except Exception:
                return None

        return None

    @classmethod
    def _resolve_billing_api_format(cls, usage: Usage) -> str | None:
        for candidate in [usage.endpoint_api_format, usage.api_format]:
            if not candidate:
                continue
            try:
                return normalize_signature_key(str(candidate))
            except Exception:
                continue
        return None

    @classmethod
    def _get_usage_raw_input_tokens(cls, usage: Usage) -> int:
        input_context_tokens = usage.input_context_tokens
        if input_context_tokens is not None:
            return cls._to_int(input_context_tokens)
        return cls._to_int(usage.input_tokens) + cls._to_int(usage.cache_read_input_tokens)

    @staticmethod
    def _copy_metadata(usage: Usage) -> dict[str, Any]:
        metadata = usage.request_metadata
        return dict(metadata) if isinstance(metadata, dict) else {}

    @staticmethod
    def _extract_billing_dimensions(billing_snapshot: Any) -> dict[str, Any]:
        if not isinstance(billing_snapshot, dict):
            return {}
        dims = billing_snapshot.get("resolved_dimensions")
        if isinstance(dims, dict):
            return dict(dims)
        dims = billing_snapshot.get("dimensions_used")
        if isinstance(dims, dict):
            return dict(dims)
        return {}

    @staticmethod
    def _snapshot_to_dict(snapshot: Any) -> dict[str, Any]:
        try:
            data = snapshot.to_dict()
        except Exception:
            return {}
        return data if isinstance(data, dict) else {}

    @staticmethod
    def _to_int(value: Any) -> int:
        try:
            return int(value or 0)
        except Exception:
            return 0

    @staticmethod
    def _parse_datetime(value: Any) -> datetime | None:
        if isinstance(value, datetime):
            return value if value.tzinfo is not None else value.replace(tzinfo=timezone.utc)
        if isinstance(value, str):
            try:
                parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
                return parsed if parsed.tzinfo is not None else parsed.replace(tzinfo=timezone.utc)
            except ValueError:
                return None
        return None

    @staticmethod
    def _ensure_datetime(value: Any) -> datetime:
        if isinstance(value, datetime):
            return value if value.tzinfo is not None else value.replace(tzinfo=timezone.utc)
        raise ValueError(f"invalid datetime value: {value!r}")

    @staticmethod
    def _parse_date(value: Any) -> date | None:
        if isinstance(value, date):
            return value
        if isinstance(value, str):
            try:
                return date.fromisoformat(value)
            except ValueError:
                return None
        return None

    @staticmethod
    def _as_money_or_none(value: Any) -> Decimal | None:
        try:
            if value is None:
                return None
            return to_money_decimal(value)
        except Exception:
            return None

    @staticmethod
    def _serialize_money_or_none(value: Decimal | None) -> float | None:
        if value is None:
            return None
        return float(value)

    @staticmethod
    def _get_usage_rate_multiplier(usage: Usage) -> Decimal:
        return to_money_decimal(getattr(usage, "rate_multiplier", None) or 1.0)

    @staticmethod
    def _usage_is_free_tier(usage: Usage) -> bool:
        total_cost = to_money_decimal(getattr(usage, "total_cost_usd", None) or 0)
        actual_total_cost = to_money_decimal(getattr(usage, "actual_total_cost_usd", None) or 0)
        return total_cost > Decimal("0") and actual_total_cost == Decimal("0")

    @classmethod
    def _increment_api_key_total_cost(
        cls,
        db: Session,
        api_key_id: str | None,
        delta: Decimal,
    ) -> None:
        if not api_key_id or delta == Decimal("0"):
            return
        db.execute(
            update(ApiKey)
            .where(ApiKey.id == api_key_id)
            .values(total_cost_usd=ApiKey.total_cost_usd + delta)
        )

    @classmethod
    def _increment_provider_api_key_total_cost(
        cls,
        db: Session,
        provider_api_key_id: str | None,
        delta: Decimal,
    ) -> None:
        if not provider_api_key_id or delta == Decimal("0"):
            return
        db.execute(
            update(ProviderAPIKey)
            .where(ProviderAPIKey.id == provider_api_key_id)
            .values(total_cost_usd=ProviderAPIKey.total_cost_usd + delta)
        )

    @classmethod
    def _increment_provider_period_usage(
        cls,
        db: Session,
        usage: Usage,
        delta: Decimal,
    ) -> None:
        if not usage.provider_id or delta == Decimal("0"):
            return

        provider = (
            db.query(Provider.id, Provider.quota_last_reset_at)
            .filter(Provider.id == usage.provider_id)
            .first()
        )
        if provider is None:
            return

        usage_created_at = cls._ensure_datetime(usage.created_at)
        quota_last_reset_at = cls._parse_datetime(getattr(provider, "quota_last_reset_at", None))
        if quota_last_reset_at is not None and usage_created_at < quota_last_reset_at:
            return

        db.execute(
            update(Provider)
            .where(Provider.id == usage.provider_id)
            .values(monthly_used_usd=Provider.monthly_used_usd + delta)
        )

