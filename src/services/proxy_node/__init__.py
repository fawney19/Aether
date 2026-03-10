"""代理节点服务

注意: health_scheduler 不在此处导入，避免触发 system.scheduler -> system.__init__
-> maintenance_scheduler -> ... -> cache_service 的循环依赖。
使用方应直接 from src.services.proxy_node.health_scheduler import ... 导入。
"""

from .resolver import (
    build_post_kwargs,
    build_proxy_url,
    build_stream_kwargs,
    compute_proxy_cache_key,
    get_proxy_label,
    get_system_proxy_config,
    inject_auth_into_proxy_url,
    invalidate_proxy_node_cache,
    invalidate_system_proxy_cache,
    make_proxy_param,
    resolve_delegate_config,
    resolve_ops_proxy,
    resolve_ops_proxy_config,
    resolve_ops_tunnel_node_id,
    resolve_proxy_info,
)
from .service import ProxyNodeService, node_to_dict

__all__ = [
    "ProxyNodeService",
    "node_to_dict",
    "build_post_kwargs",
    "build_proxy_url",
    "build_stream_kwargs",
    "compute_proxy_cache_key",
    "inject_auth_into_proxy_url",
    "make_proxy_param",
    "get_proxy_label",
    "get_system_proxy_config",
    "invalidate_proxy_node_cache",
    "invalidate_system_proxy_cache",
    "resolve_delegate_config",
    "resolve_ops_proxy",
    "resolve_ops_proxy_config",
    "resolve_ops_tunnel_node_id",
    "resolve_proxy_info",
]
