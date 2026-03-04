from __future__ import annotations

from typing import Any

from sqlalchemy.orm import Session

from src.config.settings import config
from src.core.api_format.conversion.internal_video import VideoStatus
from src.core.logger import logger
from src.models.database import ApiKey, Provider, Usage, User
from src.services.usage.service import UsageService


class VideoTaskBillingService:
    """视频任务计费/结算服务。"""

    def __init__(self, db: Session) -> None:
        self.db = db

    async def _create_fallback_usage_for_video_task(self, task: Any, request_id: str) -> bool:
        """
        Fallback: create a Usage row if it's missing (should be rare).

        This keeps behavior compatible with the old Phase2 finalize logic.
        """
        user_obj = self.db.query(User).filter(User.id == task.user_id).first()
        api_key_obj = (
            self.db.query(ApiKey).filter(ApiKey.id == task.api_key_id).first()
            if getattr(task, "api_key_id", None)
            else None
        )
        provider_obj = (
            self.db.query(Provider).filter(Provider.id == task.provider_id).first()
            if getattr(task, "provider_id", None)
            else None
        )
        provider_name = provider_obj.name if provider_obj else "unknown"

        response_time_ms: int | None = None
        if getattr(task, "submitted_at", None) and getattr(task, "completed_at", None):
            delta = task.completed_at - task.submitted_at
            response_time_ms = int(delta.total_seconds() * 1000)

        request_headers: dict[str, Any] | None = None
        if isinstance(getattr(task, "request_metadata", None), dict):
            task_meta = task.request_metadata
            for header_key in ("request_headers", "headers", "original_headers"):
                raw_headers = task_meta.get(header_key)
                if isinstance(raw_headers, dict):
                    request_headers = dict(raw_headers)
                    break

        try:
            await UsageService.record_usage_with_custom_cost(
                db=self.db,
                user=user_obj,
                api_key=api_key_obj,
                provider=provider_name,
                model=task.model,
                request_type="video",
                total_cost_usd=0.0,
                request_cost_usd=0.0,
                input_tokens=0,
                output_tokens=0,
                cache_creation_input_tokens=0,
                cache_read_input_tokens=0,
                api_format=task.client_api_format,
                endpoint_api_format=task.provider_api_format,
                has_format_conversion=bool(getattr(task, "format_converted", False)),
                is_stream=False,
                response_time_ms=response_time_ms,
                first_byte_time_ms=None,
                status_code=200 if task.status == "completed" else 500,
                error_message=(
                    None
                    if task.status == "completed"
                    else (task.error_message or task.error_code or "video_task_failed")
                ),
                metadata={
                    "fallback_created": True,
                    "video_task_id": task.id,
                },
                request_headers=request_headers,
                request_body=getattr(task, "original_request_body", None),
                provider_request_headers=None,
                response_headers=None,
                client_response_headers=None,
                response_body=None,
                request_id=request_id,
                provider_id=getattr(task, "provider_id", None),
                provider_endpoint_id=getattr(task, "endpoint_id", None),
                provider_api_key_id=getattr(task, "key_id", None),
                status="completed" if task.status == "completed" else "failed",
                target_model=None,
            )
            return True
        except Exception as exc:
            logger.exception(
                "Failed to create fallback usage for video task={}: {}",
                task.id,
                str(exc),
            )
            return False

    async def finalize_video_task(self, task: Any) -> bool:
        """
        Update billing/usage for a completed/failed video task.

        Async video billing flow:
        - Submit success: Usage is already settled with cost=0
        - Poll completion: update actual cost (success -> bill, failure -> keep 0)

        Returns True when updated, False when skipped (already finalized).
        """
        from datetime import datetime, timezone

        from src.services.billing.dimension_collector_service import DimensionCollectorService
        from src.services.billing.formula_engine import BillingIncompleteError, FormulaEngine
        from src.services.billing.rule_service import BillingRuleService

        request_id = getattr(task, "request_id", None) or getattr(task, "id", None)
        if not request_id:
            return False

        existing = self.db.query(Usage).filter(Usage.request_id == request_id).first()
        if not existing:
            logger.warning(
                "Usage not found for video task, creating fallback: task_id={} request_id={}",
                getattr(task, "id", None),
                request_id,
            )
            return await self._create_fallback_usage_for_video_task(task, request_id)

        metadata = existing.request_metadata or {}
        if metadata.get("billing_updated_at"):
            logger.debug(
                "Video task billing already updated: task_id={} request_id={}",
                getattr(task, "id", None),
                request_id,
            )
            return False

        response_time_ms: int | None = None
        if getattr(task, "submitted_at", None) and getattr(task, "completed_at", None):
            delta = task.completed_at - task.submitted_at
            response_time_ms = int(delta.total_seconds() * 1000)

        base_dimensions: dict[str, Any] = {
            "duration_seconds": getattr(task, "duration_seconds", None),
            "resolution": getattr(task, "resolution", None),
            "aspect_ratio": getattr(task, "aspect_ratio", None),
            "size": getattr(task, "size", None) or "",
            "retry_count": getattr(task, "retry_count", 0),
        }

        collector_metadata: dict[str, Any] = {
            "task": {
                "id": getattr(task, "id", None),
                "external_task_id": getattr(task, "external_task_id", None),
                "model": getattr(task, "model", None),
                "duration_seconds": getattr(task, "duration_seconds", None),
                "resolution": getattr(task, "resolution", None),
                "aspect_ratio": getattr(task, "aspect_ratio", None),
                "size": getattr(task, "size", None),
                "retry_count": getattr(task, "retry_count", 0),
                "video_size_bytes": getattr(task, "video_size_bytes", None),
            },
            "result": {
                "video_url": getattr(task, "video_url", None),
                "video_urls": getattr(task, "video_urls", None) or [],
            },
        }

        poll_raw = None
        if isinstance(getattr(task, "request_metadata", None), dict):
            poll_raw = task.request_metadata.get("poll_raw_response")

        dims = DimensionCollectorService(self.db).collect_dimensions(
            api_format=getattr(task, "provider_api_format", None),
            task_type="video",
            request=getattr(task, "original_request_body", None) or {},
            response=poll_raw if isinstance(poll_raw, dict) else None,
            metadata=collector_metadata,
            base_dimensions=base_dimensions,
        )

        # Prefer frozen rule snapshot from submit stage.
        rule_snapshot = None
        if isinstance(getattr(task, "request_metadata", None), dict):
            rule_snapshot = task.request_metadata.get("billing_rule_snapshot")

        expression = None
        variables: dict[str, Any] | None = None
        dimension_mappings: dict[str, dict[str, Any]] | None = None
        rule_id = None
        rule_name = None
        rule_scope = None

        if isinstance(rule_snapshot, dict) and rule_snapshot.get("status") == "ok":
            rule_id = rule_snapshot.get("rule_id")
            rule_name = rule_snapshot.get("rule_name")
            rule_scope = rule_snapshot.get("scope")
            expression = rule_snapshot.get("expression")
            variables = rule_snapshot.get("variables") or {}
            dimension_mappings = rule_snapshot.get("dimension_mappings") or {}
        else:
            lookup = BillingRuleService.find_rule(
                self.db,
                provider_id=getattr(task, "provider_id", None),
                model_name=getattr(task, "model", None),
                task_type="video",
            )
            if lookup:
                rule = lookup.rule
                rule_id = getattr(rule, "id", None)
                rule_name = getattr(rule, "name", None)
                rule_scope = getattr(lookup, "scope", None)
                expression = getattr(rule, "expression", None)
                variables = getattr(rule, "variables", None) or {}
                dimension_mappings = getattr(rule, "dimension_mappings", None) or {}

        billing_snapshot: dict[str, Any] = {
            "schema_version": "1.0",
            "rule_id": str(rule_id) if rule_id else None,
            "rule_name": str(rule_name) if rule_name else None,
            "scope": str(rule_scope) if rule_scope else None,
            "expression": str(expression) if expression else None,
            "dimensions_used": dims,
            "missing_required": [],
            "cost": 0.0,
            "status": "no_rule",
            "calculated_at": datetime.now(timezone.utc).isoformat(),
        }

        cost = 0.0
        is_success = str(getattr(task, "status", "")) in {
            VideoStatus.COMPLETED.value,
            "completed",
        }

        if is_success and expression:
            engine = FormulaEngine()
            try:
                result = engine.evaluate(
                    expression=str(expression),
                    variables=variables,
                    dimensions=dims,
                    dimension_mappings=dimension_mappings,
                    strict_mode=config.billing_strict_mode,
                )
                billing_snapshot["status"] = result.status
                billing_snapshot["missing_required"] = result.missing_required
                if result.status == "complete":
                    cost = float(result.cost)
                billing_snapshot["cost"] = cost
            except BillingIncompleteError as exc:
                # strict_mode=true: mark task failed and hide artifacts (avoid free pass)
                task.status = VideoStatus.FAILED.value
                task.error_code = "billing_incomplete"
                task.error_message = f"Missing required dimensions: {exc.missing_required}"
                task.video_url = None
                task.video_urls = None
                billing_snapshot["status"] = "incomplete"
                billing_snapshot["missing_required"] = exc.missing_required
                billing_snapshot["cost"] = 0.0
                cost = 0.0
            except Exception as exc:
                billing_snapshot["status"] = "incomplete"
                billing_snapshot["error"] = str(exc)
                billing_snapshot["cost"] = 0.0
                cost = 0.0

        # Write back to task.request_metadata for audit/recalc.
        task_meta = dict(task.request_metadata) if getattr(task, "request_metadata", None) else {}
        task_meta["billing_snapshot"] = billing_snapshot
        task.request_metadata = task_meta

        updated = UsageService.update_settled_billing(
            self.db,
            request_id=request_id,
            total_cost_usd=cost,
            request_cost_usd=cost,
            status="completed" if str(getattr(task, "status", "")) == "completed" else "failed",
            status_code=200 if str(getattr(task, "status", "")) == "completed" else 500,
            error_message=(
                None
                if str(getattr(task, "status", "")) == "completed"
                else (
                    getattr(task, "error_message", None)
                    or getattr(task, "error_code", None)
                    or "video_task_failed"
                )
            ),
            response_time_ms=response_time_ms,
            billing_snapshot=billing_snapshot,
            extra_metadata={
                "dimensions": dims,
                "raw_response_ref": {
                    "video_task_id": getattr(task, "id", None),
                    "field": "video_tasks.request_metadata.poll_raw_response",
                },
            },
        )

        if updated:
            logger.debug(
                "Updated video task billing: task_id={} request_id={} cost={:.6f}",
                getattr(task, "id", None),
                request_id,
                cost,
            )
        else:
            logger.warning(
                "Failed to update video task billing (may already be updated): "
                "task_id={} request_id={}",
                getattr(task, "id", None),
                request_id,
            )

        return bool(updated)
