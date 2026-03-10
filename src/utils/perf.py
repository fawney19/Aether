from __future__ import annotations

import random
import time
from typing import Any

from src.config.settings import config
from src.core.logger import logger


class PerfRecorder:
    """轻量性能记录器（可选启用）"""

    @staticmethod
    def enabled() -> bool:
        return bool(config.perf_metrics_enabled or config.perf_log_slow_ms > 0)

    @staticmethod
    def start(force: bool = False) -> float | None:
        if not force and not PerfRecorder.enabled():
            return None
        return time.perf_counter()

    @staticmethod
    def stop(
        start: float | None,
        name: str,
        labels: dict[str, str] | None = None,
        *,
        sample_rate: float | None = None,
        log_hint: str | None = None,
    ) -> float | None:
        if start is None:
            return None
        duration = time.perf_counter() - start
        PerfRecorder.record_timing(
            name,
            duration,
            labels=labels,
            sample_rate=sample_rate,
            log_hint=log_hint,
        )
        return duration

    @staticmethod
    def record_timing(
        name: str,
        duration: float,
        labels: dict[str, str] | None = None,
        *,
        sample_rate: float | None = None,
        log_hint: str | None = None,
    ) -> None:
        if not PerfRecorder.enabled():
            return
        if not PerfRecorder._should_sample(sample_rate):
            return

        duration_ms = duration * 1000.0
        if config.perf_log_slow_ms > 0 and duration_ms >= float(config.perf_log_slow_ms):
            hint = f" | {log_hint}" if log_hint else ""
            logger.info("[PERF] {} took {:.2f}ms{}", name, duration_ms, hint)

        if not config.perf_metrics_enabled:
            return

        plugin = PerfRecorder._get_monitor_plugin()
        if not plugin:
            return
        PerfRecorder._create_task(plugin.timing(PerfRecorder._metric_name(name), duration, labels))

    @staticmethod
    def record_counter(
        name: str,
        value: float = 1,
        labels: dict[str, str] | None = None,
        *,
        sample_rate: float | None = None,
    ) -> None:
        if not config.perf_metrics_enabled:
            return
        if not PerfRecorder._should_sample(sample_rate):
            return

        plugin = PerfRecorder._get_monitor_plugin()
        if not plugin:
            return
        PerfRecorder._create_task(
            plugin.increment(PerfRecorder._metric_name(name), value=value, labels=labels)
        )

    @staticmethod
    def should_store() -> bool:
        return bool(getattr(config, "perf_store_enabled", False))

    @staticmethod
    def should_store_sample() -> bool:
        if not PerfRecorder.should_store():
            return False
        rate = float(getattr(config, "perf_store_sample_rate", 1.0))
        return PerfRecorder._should_sample(rate)

    @staticmethod
    def _should_sample(sample_rate: float | None) -> bool:
        rate = float(sample_rate if sample_rate is not None else config.perf_sample_rate)
        if rate >= 1:
            return True
        if rate <= 0:
            return False
        return random.random() < rate

    @staticmethod
    def _get_monitor_plugin() -> Any | None:
        # Lazy import to avoid circular deps during app startup.
        try:
            from src.plugins.manager import get_plugin_manager
        except Exception:
            return None
        try:
            plugin = get_plugin_manager().get_plugin("monitor")
        except Exception:
            return None
        if plugin and getattr(plugin, "enabled", True):
            return plugin
        return None

    @staticmethod
    def _metric_name(name: str) -> str:
        return name if name.startswith("perf_") else f"perf_{name}"

    @staticmethod
    def _create_task(coro: Any) -> None:
        from src.utils.async_utils import safe_create_task

        safe_create_task(coro)
