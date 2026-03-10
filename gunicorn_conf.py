# Gunicorn configuration file
from __future__ import annotations

import gc
import os
from typing import Any

# worker 心跳超时（秒）：异步 worker 在此时间内必须向 arbiter 发送心跳
# 对于 UvicornWorker，事件循环偶发阻塞（GC、同步 IO）可能延迟心跳
# 默认 300 秒，通过 GUNICORN_TIMEOUT 环境变量可调整
timeout = int(os.getenv("GUNICORN_TIMEOUT", "300"))

# max-requests 触发 worker 轮换时，旧 worker 的最大存活时间（秒）
# 超过此时间后旧 worker 会被 SIGKILL 强制回收，防止因长 streaming 连接导致僵尸进程
graceful_timeout = int(os.getenv("GUNICORN_GRACEFUL_TIMEOUT", "120"))


def _log_current_rss(log: Any, message: str) -> None:
    try:
        import resource

        rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        log.info(f"{message}: {rss} KB")
    except ImportError:
        pass


def when_ready(server: Any) -> None:
    """
    Called just after the server is started.
    Freeze GC before forking workers to optimize Copy-on-Write memory sharing.
    """
    collected = gc.collect()
    gc.freeze()
    server.log.info(f"GC collected {collected} unreachable objects before freeze")
    server.log.info("GC frozen for Copy-on-Write optimization")
    server.log.info(f"Objects in permanent generation: {gc.get_freeze_count()}")


def post_fork(server: Any, worker: Any) -> None:
    _log_current_rss(server.log, f"Worker {worker.pid} RSS after fork")


def post_worker_init(worker: Any) -> None:
    _log_current_rss(worker.log, f"Worker {worker.pid} RSS after app init")
