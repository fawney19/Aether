"""
Tunnel Hub 配置

控制 worker 是否通过 aether-hub 转发 tunnel 帧。

设计约束：
- Hub 作为 Docker 内部固定服务运行
- 不对外暴露运行时配置项（不依赖 TUNNEL_HUB_* 环境变量）
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from urllib.parse import quote

_DOCKER_HUB_URL = "http://127.0.0.1:8085"
_DOCKER_HUB_CONNECT_TIMEOUT_SECONDS = 5.0


@dataclass(frozen=True)
class HubConfig:
    enabled: bool
    url: str
    connect_timeout_seconds: float

    @property
    def local_relay_base_url(self) -> str:
        return f"{self.url.rstrip('/')}/local/relay"

    def local_relay_url(self, node_id: str) -> str:
        return f"{self.local_relay_base_url}/{quote(node_id, safe='')}"


_hub_config: HubConfig | None = None


def _is_docker_runtime() -> bool:
    if os.getenv("DOCKER_CONTAINER", "").strip().lower() == "true":
        return True
    return os.path.exists("/.dockerenv")


def get_hub_config() -> HubConfig:
    """读取 Hub 配置（进程内缓存）。"""
    global _hub_config
    if _hub_config is not None:
        return _hub_config

    docker_runtime = _is_docker_runtime()
    _hub_config = HubConfig(
        enabled=docker_runtime,
        url=_DOCKER_HUB_URL,
        connect_timeout_seconds=_DOCKER_HUB_CONNECT_TIMEOUT_SECONDS,
    )
    return _hub_config


def reset_hub_config_cache() -> None:
    """测试或热更新场景下清理配置缓存。"""
    global _hub_config
    _hub_config = None
