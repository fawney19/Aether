"""
代理节点 CRUD 服务

提供 ProxyNode 的注册、心跳、注销、手动节点管理、连通性测试、远程配置等业务逻辑。
路由层（routes.py）通过此 service 操作数据库，不再直接编写 DB 查询。
"""

from __future__ import annotations

import re
import uuid
from datetime import datetime, timezone
from typing import Any
from urllib.parse import urlparse

import httpx
from sqlalchemy import func, update
from sqlalchemy.orm import Session

from src.core.exceptions import InvalidRequestException, NotFoundException
from src.models.database import (
    Provider,
    ProviderEndpoint,
    ProxyNode,
    ProxyNodeEvent,
    ProxyNodeStatus,
    SystemConfig,
)

from .resolver import (
    inject_auth_into_proxy_url,
    invalidate_system_proxy_cache,
    make_proxy_param,
)

# ---------------------------------------------------------------------------
# 辅助函数
# ---------------------------------------------------------------------------


def _mask_password(password: str | None) -> str | None:
    """脱敏密码，仅显示前2位和后2位（长度不足 8 时全部遮蔽）"""
    if not password:
        return None
    if len(password) < 8:
        return "****"
    return password[:2] + "****" + password[-2:]


def node_to_dict(node: ProxyNode) -> dict[str, Any]:
    """将 ProxyNode 实例序列化为字典（供 API 响应使用）"""
    d = {
        "id": node.id,
        "name": node.name,
        "ip": node.ip,
        "port": node.port,
        "region": node.region,
        "status": node.status.value if node.status else None,
        "is_manual": bool(node.is_manual),
        "tunnel_mode": bool(node.tunnel_mode),
        "tunnel_connected": bool(node.tunnel_connected),
        "tunnel_connected_at": node.tunnel_connected_at,
        "registered_by": node.registered_by,
        "last_heartbeat_at": node.last_heartbeat_at,
        "heartbeat_interval": node.heartbeat_interval,
        "active_connections": node.active_connections,
        "total_requests": node.total_requests,
        "avg_latency_ms": node.avg_latency_ms,
        "failed_requests": node.failed_requests,
        "dns_failures": node.dns_failures,
        "stream_errors": node.stream_errors,
        "proxy_metadata": node.proxy_metadata,
        "hardware_info": node.hardware_info,
        "estimated_max_concurrency": node.estimated_max_concurrency,
        "remote_config": node.remote_config,
        "config_version": node.config_version,
        "created_at": node.created_at,
        "updated_at": node.updated_at,
    }
    # 手动节点附带代理配置（密码脱敏）
    if node.is_manual:
        d["proxy_url"] = node.proxy_url
        d["proxy_username"] = node.proxy_username
        d["proxy_password"] = _mask_password(node.proxy_password)
    return d


def _parse_host_port(proxy_url: str) -> tuple[str, int]:
    """从代理 URL 中解析 host 和 port（含协议前缀，避免唯一约束冲突）"""
    parsed = urlparse(proxy_url)
    host = parsed.hostname or "manual"
    default_ports = {"https": 443, "socks5": 1080}
    port = parsed.port or default_ports.get((parsed.scheme or "").lower(), 80)
    # 添加协议前缀区分同 host:port 不同协议的场景
    scheme = (parsed.scheme or "http").lower()
    if scheme != "http":
        host = f"{scheme}://{host}"
    return host, port


def _sanitize_proxy_error(err: Exception) -> str:
    """去除异常消息中可能包含的代理 URL 凭据（如 HMAC 签名）"""
    return re.sub(r"://[^@/]+@", "://***@", str(err))


def _normalize_proxy_metadata(
    proxy_metadata: Any | None, proxy_version: str | None = None
) -> dict[str, Any] | None:
    """规范化 proxy 元数据，兼容旧版单独上报 proxy_version。"""
    normalized: dict[str, Any] = {}
    if isinstance(proxy_metadata, dict):
        normalized = {str(k): v for k, v in proxy_metadata.items() if k is not None}

    version: str | None = None
    raw_version = normalized.pop("version", None)
    if isinstance(raw_version, str) and raw_version.strip():
        version = raw_version.strip()[:20]
    if proxy_version is not None and proxy_version.strip():
        version = proxy_version.strip()[:20]
    if version is not None:
        normalized["version"] = version

    return normalized or None


def build_heartbeat_ack(node: ProxyNode) -> dict[str, Any]:
    """从心跳后的节点构建 ACK 响应 payload（供 hub 控制面回调使用）。"""
    result: dict[str, Any] = {}
    if not node.remote_config:
        return result
    result["remote_config"] = node.remote_config
    result["config_version"] = node.config_version or 0
    if isinstance(node.remote_config, dict):
        raw_upgrade = node.remote_config.get("upgrade_to")
        if isinstance(raw_upgrade, str) and raw_upgrade.strip():
            result["upgrade_to"] = raw_upgrade.strip()
    return result


async def _test_proxy_connectivity(proxy_url: str) -> dict[str, Any]:
    """通过代理 URL 测试连通性，返回标准化结果 dict"""
    import time as _time

    test_url = "https://cloudflare.com/cdn-cgi/trace"
    start = _time.monotonic()
    proxy_param = make_proxy_param(proxy_url)

    try:
        async with httpx.AsyncClient(
            proxy=proxy_param,
            timeout=httpx.Timeout(15.0, connect=10.0),
        ) as client:
            response = await client.get(test_url)
            elapsed_ms = round((_time.monotonic() - start) * 1000, 1)

            exit_ip = None
            if response.status_code == 200:
                for line in response.text.splitlines():
                    if line.startswith("ip="):
                        exit_ip = line.split("=", 1)[1].strip()
                        break

            return {
                "success": True,
                "latency_ms": elapsed_ms,
                "exit_ip": exit_ip,
                "error": None,
            }
    except httpx.ProxyError as exc:
        elapsed_ms = round((_time.monotonic() - start) * 1000, 1)
        return {
            "success": False,
            "latency_ms": elapsed_ms,
            "exit_ip": None,
            "error": f"代理连接失败: {_sanitize_proxy_error(exc)}",
        }
    except httpx.ConnectError as exc:
        elapsed_ms = round((_time.monotonic() - start) * 1000, 1)
        return {
            "success": False,
            "latency_ms": elapsed_ms,
            "exit_ip": None,
            "error": f"连接失败: {_sanitize_proxy_error(exc)}",
        }
    except httpx.TimeoutException:
        elapsed_ms = round((_time.monotonic() - start) * 1000, 1)
        return {
            "success": False,
            "latency_ms": elapsed_ms,
            "exit_ip": None,
            "error": "连接超时（15秒）",
        }
    except Exception as exc:
        elapsed_ms = round((_time.monotonic() - start) * 1000, 1)
        return {
            "success": False,
            "latency_ms": elapsed_ms,
            "exit_ip": None,
            "error": _sanitize_proxy_error(exc),
        }


def _build_test_proxy_url(node: ProxyNode) -> str:
    """为测试连通性构建代理 URL（无需节点在线）"""
    if node.is_manual:
        proxy_url = node.proxy_url
        if not proxy_url:
            raise InvalidRequestException("手动节点缺少 proxy_url")
        if node.proxy_username:
            proxy_url = inject_auth_into_proxy_url(
                proxy_url, node.proxy_username, node.proxy_password
            )
        return proxy_url
    else:
        # aether-proxy 节点均为 tunnel 模式，不支持通过代理 URL 测试
        raise InvalidRequestException("aether-proxy tunnel 节点不支持代理 URL 连通性测试")


async def _test_tunnel_connectivity(node_id: str) -> dict[str, Any]:
    """通过 WebSocket tunnel 测试连通性，返回标准化结果 dict"""
    import time as _time

    from .tunnel_transport import create_tunnel_transport

    test_url = "https://cloudflare.com/cdn-cgi/trace"
    transport = create_tunnel_transport(node_id, timeout=15.0)
    start = _time.monotonic()

    try:
        async with httpx.AsyncClient(transport=transport) as client:
            response = await client.get(test_url)
            elapsed_ms = round((_time.monotonic() - start) * 1000, 1)

            exit_ip = None
            if response.status_code == 200:
                for line in response.text.splitlines():
                    if line.startswith("ip="):
                        exit_ip = line.split("=", 1)[1].strip()
                        break

            return {
                "success": True,
                "latency_ms": elapsed_ms,
                "exit_ip": exit_ip,
                "error": None,
            }
    except httpx.ConnectError as exc:
        elapsed_ms = round((_time.monotonic() - start) * 1000, 1)
        return {
            "success": False,
            "latency_ms": elapsed_ms,
            "exit_ip": None,
            "error": f"tunnel 连接失败: {_sanitize_proxy_error(exc)}",
        }
    except httpx.TimeoutException:
        elapsed_ms = round((_time.monotonic() - start) * 1000, 1)
        return {
            "success": False,
            "latency_ms": elapsed_ms,
            "exit_ip": None,
            "error": "连接超时（15秒）",
        }
    except Exception as exc:
        elapsed_ms = round((_time.monotonic() - start) * 1000, 1)
        return {
            "success": False,
            "latency_ms": elapsed_ms,
            "exit_ip": None,
            "error": _sanitize_proxy_error(exc),
        }


# ---------------------------------------------------------------------------
# ProxyNodeService
# ---------------------------------------------------------------------------


class ProxyNodeService:
    """代理节点 CRUD 服务"""

    @staticmethod
    def register_node(
        db: Session,
        *,
        name: str,
        ip: str,
        port: int,
        region: str | None = None,
        heartbeat_interval: int = 30,
        hardware_info: dict[str, Any] | None = None,
        estimated_max_concurrency: int | None = None,
        active_connections: int | None = None,
        total_requests: int | None = None,
        avg_latency_ms: float | None = None,
        proxy_metadata: dict[str, Any] | None = None,
        proxy_version: str | None = None,
        registered_by: str | None = None,
    ) -> ProxyNode:
        """注册或更新 aether-proxy 节点（tunnel 模式）"""

        now = datetime.now(timezone.utc)
        normalized_proxy_metadata = _normalize_proxy_metadata(proxy_metadata, proxy_version)

        node = (
            db.query(ProxyNode)
            .filter(
                ProxyNode.ip == ip,
                ProxyNode.port == port,
                ProxyNode.is_manual == False,  # noqa: E712
            )
            .first()
        )
        if node:
            node.name = name
            node.region = region
            # 状态完全由 tunnel 连接管理（_update_tunnel_status / health_scheduler），
            # 注册不干预
            node.last_heartbeat_at = now
            node.heartbeat_interval = heartbeat_interval
            node.tunnel_mode = True
            if hardware_info is not None:
                node.hardware_info = hardware_info
            if estimated_max_concurrency is not None:
                node.estimated_max_concurrency = estimated_max_concurrency
            if active_connections is not None:
                node.active_connections = active_connections
            if total_requests is not None:
                node.total_requests = total_requests
            if avg_latency_ms is not None:
                node.avg_latency_ms = avg_latency_ms
            if normalized_proxy_metadata is not None:
                node.proxy_metadata = normalized_proxy_metadata
        else:
            node = ProxyNode(
                id=str(uuid.uuid4()),
                name=name,
                ip=ip,
                port=port,
                region=region,
                # 新节点：等 tunnel 连接后才上线
                status=ProxyNodeStatus.OFFLINE,
                registered_by=registered_by,
                last_heartbeat_at=now,
                heartbeat_interval=heartbeat_interval,
                active_connections=active_connections or 0,
                total_requests=total_requests or 0,
                avg_latency_ms=avg_latency_ms,
                proxy_metadata=normalized_proxy_metadata,
                hardware_info=hardware_info,
                estimated_max_concurrency=estimated_max_concurrency,
                tunnel_mode=True,
                created_at=now,
                updated_at=now,
            )
            db.add(node)

        db.commit()
        db.refresh(node)
        return node

    @staticmethod
    def heartbeat(
        db: Session,
        *,
        node_id: str,
        heartbeat_interval: int | None = None,
        active_connections: int | None = None,
        total_requests: int | None = None,
        avg_latency_ms: float | None = None,
        failed_requests: int | None = None,
        dns_failures: int | None = None,
        stream_errors: int | None = None,
        proxy_metadata: dict[str, Any] | None = None,
        proxy_version: str | None = None,
    ) -> ProxyNode:
        """处理节点心跳（仅 tunnel 模式节点，更新指标并修正状态不一致）

        注意: total_requests / failed_requests / dns_failures / stream_errors
        来自 Rust 端的区间增量（swap(0) 后上报），需要累加到 DB 而非覆盖。
        active_connections 和 avg_latency_ms 是实时快照，直接覆盖。
        """
        node = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
        if not node:
            raise NotFoundException(f"ProxyNode {node_id} 不存在", "proxy_node")

        if not node.tunnel_mode:
            raise InvalidRequestException(
                "non-tunnel mode is no longer supported, please upgrade aether-proxy to use tunnel mode"
            )

        now = datetime.now(timezone.utc)
        values: dict[str, Any] = {"last_heartbeat_at": now}

        # 心跳通过 tunnel 连接传输，能收到心跳说明 tunnel 一定连通。
        # 若状态不一致（例如并发写入覆盖），修正为 ONLINE。
        if node.status != ProxyNodeStatus.ONLINE or not node.tunnel_connected:
            values["status"] = ProxyNodeStatus.ONLINE
            values["tunnel_connected"] = True
            values["tunnel_connected_at"] = now
            values["updated_at"] = now

        if heartbeat_interval is not None:
            values["heartbeat_interval"] = heartbeat_interval

        # 实时快照指标 -- 直接覆盖
        if active_connections is not None:
            values["active_connections"] = active_connections
        if avg_latency_ms is not None:
            values["avg_latency_ms"] = avg_latency_ms
        normalized_proxy_metadata = _normalize_proxy_metadata(proxy_metadata, proxy_version)
        if normalized_proxy_metadata is not None:
            values["proxy_metadata"] = normalized_proxy_metadata

        # 区间增量指标 -- 使用数据库原子自增，避免并发心跳读改写丢增量
        if total_requests is not None and total_requests > 0:
            values["total_requests"] = ProxyNode.total_requests + int(total_requests)
        if failed_requests is not None and failed_requests > 0:
            values["failed_requests"] = ProxyNode.failed_requests + int(failed_requests)
        if dns_failures is not None and dns_failures > 0:
            values["dns_failures"] = ProxyNode.dns_failures + int(dns_failures)
        if stream_errors is not None and stream_errors > 0:
            values["stream_errors"] = ProxyNode.stream_errors + int(stream_errors)

        db.execute(update(ProxyNode).where(ProxyNode.id == node_id).values(**values))
        db.commit()
        db.expire_all()

        refreshed = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
        if not refreshed:
            raise NotFoundException(f"ProxyNode {node_id} 不存在", "proxy_node")
        return refreshed

    @staticmethod
    def update_tunnel_status(
        db: Session,
        *,
        node_id: str,
        connected: bool,
        conn_count: int = 0,
        detail: str | None = None,
        observed_at: datetime | None = None,
    ) -> ProxyNode | None:
        """根据 Hub 连接池状态更新 tunnel 连接状态并记录事件。"""
        node = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
        if not node:
            return None

        event_time = observed_at or datetime.now(timezone.utc)
        last_transition = node.tunnel_connected_at
        if last_transition and last_transition.tzinfo is None:
            last_transition = last_transition.replace(tzinfo=timezone.utc)

        event_type = "connected" if connected else "disconnected"
        event_detail = detail or f"[hub_node_status] conn_count={max(int(conn_count), 0)}"

        if last_transition and event_time < last_transition:
            db.add(
                ProxyNodeEvent(
                    node_id=node_id,
                    event_type=event_type,
                    detail=f"[stale_ignored] {event_detail}",
                )
            )
            db.commit()
            return node

        node.tunnel_connected = connected
        node.tunnel_connected_at = event_time
        node.status = ProxyNodeStatus.ONLINE if connected else ProxyNodeStatus.OFFLINE
        node.updated_at = event_time

        db.add(
            ProxyNodeEvent(
                node_id=node_id,
                event_type=event_type,
                detail=event_detail,
            )
        )
        db.commit()
        db.refresh(node)

        from .resolver import invalidate_proxy_node_cache

        invalidate_proxy_node_cache(node_id)
        return node

    @staticmethod
    def unregister_node(db: Session, *, node_id: str) -> ProxyNode:
        """注销节点（设置为 OFFLINE）"""
        node = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
        if not node:
            raise NotFoundException(f"ProxyNode {node_id} 不存在", "proxy_node")

        node.status = ProxyNodeStatus.OFFLINE
        node.updated_at = datetime.now(timezone.utc)
        db.commit()
        return node

    @staticmethod
    def list_nodes(
        db: Session,
        *,
        status: str | None = None,
        skip: int = 0,
        limit: int = 100,
    ) -> tuple[list[ProxyNode], int]:
        """列出代理节点（支持按状态筛选和分页）"""
        query = db.query(ProxyNode)
        if status:
            normalized = status.strip().lower()
            allowed = {"online", "offline"}
            if normalized not in allowed:
                raise InvalidRequestException(f"status 必须是以下之一: {sorted(allowed)}", "status")
            query = query.filter(ProxyNode.status == ProxyNodeStatus(normalized))

        total = int(query.with_entities(func.count(ProxyNode.id)).scalar() or 0)
        nodes = query.order_by(ProxyNode.name.asc()).offset(skip).limit(limit).all()
        return nodes, total

    @staticmethod
    def create_manual_node(
        db: Session,
        *,
        name: str,
        proxy_url: str,
        username: str | None = None,
        password: str | None = None,
        region: str | None = None,
        registered_by: str | None = None,
    ) -> ProxyNode:
        """创建手动代理节点"""
        host, port = _parse_host_port(proxy_url)
        now = datetime.now(timezone.utc)

        # 检查是否已存在同地址的节点
        existing = db.query(ProxyNode).filter(ProxyNode.ip == host, ProxyNode.port == port).first()
        if existing:
            raise InvalidRequestException(
                f"已存在相同地址的代理节点: {existing.name} ({existing.ip}:{existing.port})"
            )

        node = ProxyNode(
            id=str(uuid.uuid4()),
            name=name,
            ip=host,
            port=port,
            region=region,
            is_manual=True,
            proxy_url=proxy_url,
            proxy_username=username,
            proxy_password=password,
            status=ProxyNodeStatus.ONLINE,
            registered_by=registered_by,
            last_heartbeat_at=None,
            heartbeat_interval=0,
            active_connections=0,
            total_requests=0,
            avg_latency_ms=None,
            created_at=now,
            updated_at=now,
        )

        db.add(node)
        db.commit()
        db.refresh(node)
        return node

    @staticmethod
    def update_manual_node(
        db: Session,
        *,
        node_id: str,
        name: str | None = None,
        proxy_url: str | None = None,
        username: str | None = None,
        password: str | None = None,
        region: str | None = None,
    ) -> ProxyNode:
        """更新手动代理节点"""
        node = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
        if not node:
            raise NotFoundException(f"ProxyNode {node_id} 不存在", "proxy_node")
        if not node.is_manual:
            raise InvalidRequestException("只能编辑手动添加的代理节点")

        if name is not None:
            node.name = name
        if proxy_url is not None:
            host, port = _parse_host_port(proxy_url)
            # 检查新地址是否与其他节点冲突
            existing = (
                db.query(ProxyNode)
                .filter(ProxyNode.ip == host, ProxyNode.port == port, ProxyNode.id != node.id)
                .first()
            )
            if existing:
                raise InvalidRequestException(
                    f"已存在相同地址的代理节点: {existing.name} ({existing.ip}:{existing.port})"
                )
            node.proxy_url = proxy_url
            node.ip = host
            node.port = port
        if username is not None:
            node.proxy_username = username
        # password: None=不发送(保留原值), ""=清空, 非空=更新
        if password is not None:
            node.proxy_password = password or None
        if region is not None:
            node.region = region

        node.updated_at = datetime.now(timezone.utc)
        db.commit()
        db.refresh(node)
        return node

    @staticmethod
    def delete_node(db: Session, *, node_id: str) -> dict[str, Any]:
        """
        删除代理节点

        若该节点是系统默认代理，自动清除引用。
        返回 {"node_id": ..., "cleared_system_proxy": bool}
        """
        node = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
        if not node:
            raise NotFoundException(f"ProxyNode {node_id} 不存在", "proxy_node")

        # 若该节点是系统默认代理，自动清除引用
        was_system_proxy = False
        sys_cfg = db.query(SystemConfig).filter(SystemConfig.key == "system_proxy_node_id").first()
        if sys_cfg and sys_cfg.value == node_id:
            sys_cfg.value = None
            was_system_proxy = True

        # 清理引用该节点的 Provider / ProviderEndpoint 的 proxy 字段（批量 SQL 更新）
        cleared_providers = (
            db.query(Provider)
            .filter(Provider.proxy.isnot(None), Provider.proxy["node_id"].as_string() == node_id)
            .update({"proxy": None}, synchronize_session="fetch")
        )
        cleared_endpoints = (
            db.query(ProviderEndpoint)
            .filter(
                ProviderEndpoint.proxy.isnot(None),
                ProviderEndpoint.proxy["node_id"].as_string() == node_id,
            )
            .update({"proxy": None}, synchronize_session="fetch")
        )

        node_info = {"proxy_node_ip": node.ip, "proxy_node_port": node.port}
        db.delete(node)
        db.commit()

        if was_system_proxy:
            invalidate_system_proxy_cache()

        return {
            "node_id": node_id,
            "node_info": node_info,
            "cleared_system_proxy": was_system_proxy,
            "cleared_providers": cleared_providers,
            "cleared_endpoints": cleared_endpoints,
        }

    @staticmethod
    async def test_node(db: Session, *, node_id: str) -> dict[str, Any]:
        """测试代理节点连通性和延迟"""
        node = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
        if not node:
            raise NotFoundException(f"ProxyNode {node_id} 不存在", "proxy_node")

        # tunnel 节点：通过 WebSocket tunnel 测试
        if not node.is_manual:
            connected = bool(node.tunnel_connected) and node.status == ProxyNodeStatus.ONLINE

            if not connected:
                return {
                    "success": False,
                    "latency_ms": None,
                    "exit_ip": None,
                    "error": "tunnel 未连接",
                }
            result = await _test_tunnel_connectivity(node.id)

            # 连通性测试成功但 DB 状态不一致时，修正为 ONLINE
            if result.get("success") and (
                node.status != ProxyNodeStatus.ONLINE or not node.tunnel_connected
            ):
                node.status = ProxyNodeStatus.ONLINE
                node.tunnel_connected = True
                node.tunnel_connected_at = datetime.now(timezone.utc)
                node.updated_at = node.tunnel_connected_at
                db.commit()

                from .resolver import invalidate_proxy_node_cache

                invalidate_proxy_node_cache(node.id)

            return result

        # 手动节点：通过代理 URL 测试
        try:
            proxy_url = _build_test_proxy_url(node)
        except Exception as exc:
            return {"success": False, "latency_ms": None, "exit_ip": None, "error": str(exc)}

        return await _test_proxy_connectivity(proxy_url)

    @staticmethod
    async def test_proxy_url(
        *, proxy_url: str, username: str | None = None, password: str | None = None
    ) -> dict[str, Any]:
        """直接通过 proxy_url 测试代理连通性（无需已注册节点）"""
        if username:
            proxy_url = inject_auth_into_proxy_url(proxy_url, username, password)
        return await _test_proxy_connectivity(proxy_url)

    @staticmethod
    def update_node_config(
        db: Session, *, node_id: str, config_updates: dict[str, Any]
    ) -> ProxyNode:
        """更新 aether-proxy 节点的远程配置（通过下次心跳下发）"""
        node = db.query(ProxyNode).filter(ProxyNode.id == node_id).first()
        if not node:
            raise NotFoundException(f"ProxyNode {node_id} 不存在", "proxy_node")
        if node.is_manual:
            raise InvalidRequestException("手动节点不支持远程配置下发")

        # node_name is special: it also updates the node.name column directly
        if "node_name" in config_updates:
            node.name = config_updates["node_name"]

        # Merge with existing config (so partial updates are preserved)
        # Copy to a new dict so SQLAlchemy detects the change on the JSON column
        existing = dict(node.remote_config) if node.remote_config else {}
        for key, value in config_updates.items():
            if key == "upgrade_to" and value is None:
                existing.pop("upgrade_to", None)
                continue
            existing[key] = value

        node.remote_config = existing
        node.config_version = (node.config_version or 0) + 1
        node.updated_at = datetime.now(timezone.utc)

        db.commit()
        db.refresh(node)
        return node

    @staticmethod
    def batch_upgrade_online_nodes(db: Session, *, version: str) -> dict[str, Any]:
        """批量向在线 tunnel 节点下发 upgrade_to。"""
        normalized = version.strip()
        if not normalized:
            raise InvalidRequestException("version 不能为空")

        nodes = (
            db.query(ProxyNode)
            .filter(
                ProxyNode.is_manual == False,  # noqa: E712
                ProxyNode.tunnel_mode == True,  # noqa: E712
                ProxyNode.status == ProxyNodeStatus.ONLINE,
            )
            .all()
        )

        updated_node_ids: list[str] = []
        skipped = 0
        now = datetime.now(timezone.utc)
        for node in nodes:
            existing = dict(node.remote_config) if node.remote_config else {}
            if existing.get("upgrade_to") == normalized:
                skipped += 1
                continue
            existing["upgrade_to"] = normalized
            node.remote_config = existing
            node.config_version = (node.config_version or 0) + 1
            node.updated_at = now
            updated_node_ids.append(node.id)

        if updated_node_ids:
            db.commit()

        return {
            "version": normalized,
            "updated": len(updated_node_ids),
            "skipped": skipped,
            "node_ids": updated_node_ids,
        }
