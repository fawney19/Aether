"""管理员代理节点（ProxyNode）管理端点

用于 aether-proxy 在 VPS 上注册、心跳、注销节点，以及管理员查看/删除节点记录。
"""

from __future__ import annotations

import ipaddress
from dataclasses import dataclass
from typing import Any

from fastapi import APIRouter, Depends, Query, Request
from pydantic import BaseModel, Field, ValidationError, field_validator
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import ApiRequestPipeline
from src.core.exceptions import InvalidRequestException
from src.database import get_db
from src.services.proxy_node.service import ProxyNodeService, node_to_dict

router = APIRouter(prefix="/api/admin/proxy-nodes", tags=["Admin - Proxy Nodes"])
pipeline = ApiRequestPipeline()


# ---------------------------------------------------------------------------
# Pydantic 请求模型
# ---------------------------------------------------------------------------


class ProxyNodeRegisterRequest(BaseModel):
    name: str = Field(..., min_length=1, max_length=100, description="节点名")
    ip: str = Field(..., description="公网 IP（IPv4/IPv6）")
    port: int = Field(0, ge=0, le=65535, description="代理端口（tunnel 模式下为 0）")
    region: str | None = Field(None, max_length=100, description="区域标签")
    heartbeat_interval: int = Field(30, ge=5, le=600, description="心跳间隔（秒）")

    # 指标（可选）
    active_connections: int | None = Field(None, ge=0, description="当前活跃连接数")
    total_requests: int | None = Field(None, ge=0, description="累计请求数")
    avg_latency_ms: float | None = Field(None, ge=0, description="平均连接建立延迟(ms)")

    # 硬件信息
    hardware_info: dict | None = Field(None, description="硬件信息 JSON")
    estimated_max_concurrency: int | None = Field(None, ge=0, description="估算最大并发连接数")
    proxy_metadata: dict[str, Any] | None = Field(None, description="aether-proxy 元数据（版本等）")
    proxy_version: str | None = Field(
        None, max_length=20, description="兼容字段：aether-proxy 软件版本"
    )

    @field_validator("ip")
    @classmethod
    def validate_ip(cls, v: str) -> str:
        v = v.strip()
        try:
            ipaddress.ip_address(v)
        except ValueError as exc:
            raise ValueError("ip 必须是合法的 IPv4/IPv6 地址") from exc
        return v


class ProxyNodeHeartbeatRequest(BaseModel):
    node_id: str = Field(..., min_length=1, max_length=36, description="节点 ID")
    heartbeat_interval: int | None = Field(None, ge=5, le=600, description="心跳间隔（秒）")

    active_connections: int | None = Field(None, ge=0, description="当前活跃连接数")
    total_requests: int | None = Field(None, ge=0, description="累计请求数")
    avg_latency_ms: float | None = Field(None, ge=0, description="平均连接建立延迟(ms)")
    proxy_metadata: dict[str, Any] | None = Field(None, description="aether-proxy 元数据（版本等）")
    proxy_version: str | None = Field(
        None, max_length=20, description="兼容字段：aether-proxy 软件版本"
    )


class ProxyNodeUnregisterRequest(BaseModel):
    node_id: str = Field(..., min_length=1, max_length=36, description="节点 ID")


class ProxyNodeRemoteConfigRequest(BaseModel):
    """管理端远程配置 — 通过心跳下发给 aether-proxy"""

    node_name: str | None = Field(None, min_length=1, max_length=100, description="节点名称")
    allowed_ports: list[int] | None = Field(None, description="允许代理的目标端口")
    log_level: str | None = Field(None, description="日志级别 (trace/debug/info/warn/error)")
    heartbeat_interval: int | None = Field(None, ge=5, le=600, description="心跳间隔（秒）")
    upgrade_to: str | None = Field(None, max_length=50, description="下发升级目标版本")

    @field_validator("allowed_ports")
    @classmethod
    def validate_ports(cls, v: list[int] | None) -> list[int] | None:
        if v is not None:
            for port in v:
                if not 1 <= port <= 65535:
                    raise ValueError(f"端口 {port} 不在有效范围 (1-65535)")
        return v

    @field_validator("log_level")
    @classmethod
    def validate_log_level(cls, v: str | None) -> str | None:
        if v is not None and v not in ("trace", "debug", "info", "warn", "error"):
            raise ValueError("log_level 必须是 trace/debug/info/warn/error 之一")
        return v

    @field_validator("upgrade_to")
    @classmethod
    def validate_upgrade_to(cls, v: str | None) -> str | None:
        if v is None:
            return None
        vv = v.strip()
        if not vv:
            return None
        return vv


class ProxyNodeBatchUpgradeRequest(BaseModel):
    version: str = Field(..., min_length=1, max_length=50, description="目标版本号")

    @field_validator("version")
    @classmethod
    def validate_version(cls, v: str) -> str:
        vv = v.strip()
        if not vv:
            raise ValueError("version 不能为空")
        return vv


class ManualProxyNodeCreateRequest(BaseModel):
    """手动创建代理节点"""

    name: str = Field(..., min_length=1, max_length=100, description="节点名")
    proxy_url: str = Field(
        ..., min_length=1, max_length=500, description="代理 URL (http/https/socks5)"
    )
    username: str | None = Field(None, max_length=255, description="代理用户名")
    password: str | None = Field(None, max_length=500, description="代理密码")
    region: str | None = Field(None, max_length=100, description="区域标签")

    @field_validator("proxy_url")
    @classmethod
    def validate_proxy_url(cls, v: str) -> str:
        import re
        from urllib.parse import urlparse

        v = v.strip()
        if not re.match(r"^(http|https|socks5)://", v, re.IGNORECASE):
            raise ValueError("代理 URL 必须以 http://, https:// 或 socks5:// 开头")
        parsed = urlparse(v)
        if not parsed.hostname:
            raise ValueError("代理 URL 必须包含有效的 host")
        return v


class ManualProxyNodeUpdateRequest(BaseModel):
    """更新手动代理节点"""

    name: str | None = Field(None, min_length=1, max_length=100, description="节点名")
    proxy_url: str | None = Field(None, min_length=1, max_length=500, description="代理 URL")
    username: str | None = Field(None, max_length=255, description="代理用户名")
    password: str | None = Field(None, max_length=500, description="代理密码")
    region: str | None = Field(None, max_length=100, description="区域标签")

    @field_validator("proxy_url")
    @classmethod
    def validate_proxy_url(cls, v: str | None) -> str | None:
        if v is None:
            return None
        import re
        from urllib.parse import urlparse

        v = v.strip()
        if not re.match(r"^(http|https|socks5)://", v, re.IGNORECASE):
            raise ValueError("代理 URL 必须以 http://, https:// 或 socks5:// 开头")
        parsed = urlparse(v)
        if not parsed.hostname:
            raise ValueError("代理 URL 必须包含有效的 host")
        return v


# ---------------------------------------------------------------------------
# 路由端点
# ---------------------------------------------------------------------------


@router.post("/register")
async def register_proxy_node(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminRegisterProxyNodeAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/heartbeat")
async def heartbeat_proxy_node(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminHeartbeatProxyNodeAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/unregister")
async def unregister_proxy_node(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminUnregisterProxyNodeAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("")
async def list_proxy_nodes(
    request: Request,
    status: str | None = Query(None, description="按状态筛选：online/offline"),
    skip: int = Query(0, ge=0),
    limit: int = Query(100, ge=1, le=1000),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminListProxyNodesAdapter(status=status, skip=skip, limit=limit)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/manual")
async def create_manual_proxy_node(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminCreateManualProxyNodeAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/upgrade")
async def batch_upgrade_proxy_nodes(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminBatchUpgradeProxyNodesAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/{node_id}")
async def update_manual_proxy_node(
    node_id: str, request: Request, db: Session = Depends(get_db)
) -> Any:
    adapter = AdminUpdateManualProxyNodeAdapter(node_id=node_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{node_id}")
async def delete_proxy_node(node_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminDeleteProxyNodeAdapter(node_id=node_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/{node_id}/test")
async def test_proxy_node(node_id: str, request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminTestProxyNodeAdapter(node_id=node_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("/test-url")
async def test_proxy_url(request: Request, db: Session = Depends(get_db)) -> Any:
    adapter = AdminTestProxyUrlAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/{node_id}/config")
async def update_proxy_node_config(
    node_id: str, request: Request, db: Session = Depends(get_db)
) -> Any:
    adapter = AdminUpdateProxyNodeConfigAdapter(node_id=node_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{node_id}/events")
async def list_proxy_node_events(
    node_id: str,
    request: Request,
    limit: int = Query(50, ge=1, le=200),
    db: Session = Depends(get_db),
) -> Any:
    adapter = AdminListProxyNodeEventsAdapter(node_id=node_id, limit=limit)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


# ---------------------------------------------------------------------------
# 辅助函数
# ---------------------------------------------------------------------------


def _format_validation_error(exc: ValidationError) -> str:
    parts: list[str] = []
    for err in exc.errors():
        field = " -> ".join(str(x) for x in err.get("loc", []))
        msg = str(err.get("msg", "invalid"))
        parts.append(f"{field}: {msg}")
    return "; ".join(parts) or "输入验证失败"


# ---------------------------------------------------------------------------
# Adapter 实现
# ---------------------------------------------------------------------------


@dataclass
class AdminRegisterProxyNodeAdapter(AdminApiAdapter):
    name: str = "admin_register_proxy_node"

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = context.ensure_json_body()
        try:
            req = ProxyNodeRegisterRequest.model_validate(payload)
        except ValidationError as exc:
            raise InvalidRequestException("输入验证失败: " + _format_validation_error(exc))

        node = ProxyNodeService.register_node(
            context.db,
            name=req.name,
            ip=req.ip,
            port=req.port,
            region=req.region,
            heartbeat_interval=req.heartbeat_interval,
            hardware_info=req.hardware_info,
            estimated_max_concurrency=req.estimated_max_concurrency,
            active_connections=req.active_connections,
            total_requests=req.total_requests,
            avg_latency_ms=req.avg_latency_ms,
            proxy_metadata=req.proxy_metadata,
            proxy_version=req.proxy_version,
            registered_by=context.user.id if context.user else None,
        )

        context.add_audit_metadata(
            action="proxy_node_register",
            proxy_node_id=node.id,
            proxy_node_ip=node.ip,
            proxy_node_port=node.port,
        )

        return {"node_id": node.id, "node": node_to_dict(node)}


@dataclass
class AdminHeartbeatProxyNodeAdapter(AdminApiAdapter):
    name: str = "admin_heartbeat_proxy_node"

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = context.ensure_json_body()
        try:
            req = ProxyNodeHeartbeatRequest.model_validate(payload)
        except ValidationError as exc:
            raise InvalidRequestException("输入验证失败: " + _format_validation_error(exc))

        node = ProxyNodeService.heartbeat(
            context.db,
            node_id=req.node_id,
            heartbeat_interval=req.heartbeat_interval,
            active_connections=req.active_connections,
            total_requests=req.total_requests,
            avg_latency_ms=req.avg_latency_ms,
            proxy_metadata=req.proxy_metadata,
            proxy_version=req.proxy_version,
        )

        context.add_audit_metadata(
            action="proxy_node_heartbeat",
            proxy_node_id=node.id,
        )

        return {"message": "heartbeat ok", "node": node_to_dict(node)}


@dataclass
class AdminUnregisterProxyNodeAdapter(AdminApiAdapter):
    name: str = "admin_unregister_proxy_node"

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = context.ensure_json_body()
        try:
            req = ProxyNodeUnregisterRequest.model_validate(payload)
        except ValidationError as exc:
            raise InvalidRequestException("输入验证失败: " + _format_validation_error(exc))

        node = ProxyNodeService.unregister_node(context.db, node_id=req.node_id)

        context.add_audit_metadata(
            action="proxy_node_unregister",
            proxy_node_id=node.id,
        )

        return {"message": "unregistered", "node_id": node.id}


@dataclass
class AdminListProxyNodesAdapter(AdminApiAdapter):
    name: str = "admin_list_proxy_nodes"
    status: str | None = None
    skip: int = 0
    limit: int = 100

    async def handle(self, context: ApiRequestContext) -> Any:
        nodes, total = ProxyNodeService.list_nodes(
            context.db, status=self.status, skip=self.skip, limit=self.limit
        )
        return {
            "items": [node_to_dict(n) for n in nodes],
            "total": total,
            "skip": self.skip,
            "limit": self.limit,
        }


@dataclass
class AdminDeleteProxyNodeAdapter(AdminApiAdapter):
    name: str = "admin_delete_proxy_node"
    node_id: str = ""

    async def handle(self, context: ApiRequestContext) -> Any:
        result = ProxyNodeService.delete_node(context.db, node_id=self.node_id)

        context.add_audit_metadata(
            action="proxy_node_delete",
            proxy_node_id=self.node_id,
            **result.get("node_info", {}),
        )

        was_system_proxy = result["cleared_system_proxy"]
        cleared_providers = result.get("cleared_providers", 0)
        cleared_endpoints = result.get("cleared_endpoints", 0)

        parts = ["deleted"]
        if was_system_proxy:
            parts.append("system default proxy cleared")
        if cleared_providers or cleared_endpoints:
            parts.append(
                f"cleared proxy from {cleared_providers} provider(s) "
                f"and {cleared_endpoints} endpoint(s)"
            )

        return {
            "message": ", ".join(parts),
            "node_id": self.node_id,
            "cleared_system_proxy": was_system_proxy,
            "cleared_providers": cleared_providers,
            "cleared_endpoints": cleared_endpoints,
        }


@dataclass
class AdminCreateManualProxyNodeAdapter(AdminApiAdapter):
    name: str = "admin_create_manual_proxy_node"

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = context.ensure_json_body()
        try:
            req = ManualProxyNodeCreateRequest.model_validate(payload)
        except ValidationError as exc:
            raise InvalidRequestException("输入验证失败: " + _format_validation_error(exc))

        node = ProxyNodeService.create_manual_node(
            context.db,
            name=req.name,
            proxy_url=req.proxy_url,
            username=req.username,
            password=req.password,
            region=req.region,
            registered_by=context.user.id if context.user else None,
        )

        context.add_audit_metadata(
            action="proxy_node_manual_create",
            proxy_node_id=node.id,
        )

        return {"node_id": node.id, "node": node_to_dict(node)}


@dataclass
class AdminUpdateManualProxyNodeAdapter(AdminApiAdapter):
    name: str = "admin_update_manual_proxy_node"
    node_id: str = ""

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = context.ensure_json_body()
        try:
            req = ManualProxyNodeUpdateRequest.model_validate(payload)
        except ValidationError as exc:
            raise InvalidRequestException("输入验证失败: " + _format_validation_error(exc))

        node = ProxyNodeService.update_manual_node(
            context.db,
            node_id=self.node_id,
            name=req.name,
            proxy_url=req.proxy_url,
            username=req.username,
            password=req.password,
            region=req.region,
        )

        context.add_audit_metadata(
            action="proxy_node_manual_update",
            proxy_node_id=node.id,
        )

        return {"node_id": node.id, "node": node_to_dict(node)}


@dataclass
class AdminTestProxyNodeAdapter(AdminApiAdapter):
    """测试代理节点连通性和延迟"""

    name: str = "admin_test_proxy_node"
    node_id: str = ""

    async def handle(self, context: ApiRequestContext) -> Any:
        return await ProxyNodeService.test_node(context.db, node_id=self.node_id)


@dataclass
class AdminUpdateProxyNodeConfigAdapter(AdminApiAdapter):
    """更新 aether-proxy 节点的远程配置（通过下次心跳下发）"""

    name: str = "admin_update_proxy_node_config"
    node_id: str = ""

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = context.ensure_json_body()
        try:
            req = ProxyNodeRemoteConfigRequest.model_validate(payload)
        except ValidationError as exc:
            raise InvalidRequestException("输入验证失败: " + _format_validation_error(exc))

        # Build config dict with only the supplied fields
        config_updates: dict[str, Any] = {}
        fields_set = req.model_fields_set
        if req.node_name is not None:
            config_updates["node_name"] = req.node_name
        if req.allowed_ports is not None:
            config_updates["allowed_ports"] = req.allowed_ports
        if req.log_level is not None:
            config_updates["log_level"] = req.log_level
        if req.heartbeat_interval is not None:
            config_updates["heartbeat_interval"] = req.heartbeat_interval
        if "upgrade_to" in fields_set:
            config_updates["upgrade_to"] = req.upgrade_to

        node = ProxyNodeService.update_node_config(
            context.db, node_id=self.node_id, config_updates=config_updates
        )

        context.add_audit_metadata(
            action="proxy_node_config_update",
            proxy_node_id=node.id,
            config_version=node.config_version,
        )

        return {
            "node_id": node.id,
            "config_version": node.config_version,
            "remote_config": node.remote_config,
            "node": node_to_dict(node),
        }


@dataclass
class AdminBatchUpgradeProxyNodesAdapter(AdminApiAdapter):
    """批量向在线 tunnel 节点下发升级指令。"""

    name: str = "admin_batch_upgrade_proxy_nodes"

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = context.ensure_json_body()
        try:
            req = ProxyNodeBatchUpgradeRequest.model_validate(payload)
        except ValidationError as exc:
            raise InvalidRequestException("输入验证失败: " + _format_validation_error(exc))

        result = ProxyNodeService.batch_upgrade_online_nodes(context.db, version=req.version)
        context.add_audit_metadata(
            action="proxy_node_batch_upgrade",
            version=result["version"],
            updated=result["updated"],
            skipped=result["skipped"],
        )
        return result


class TestProxyUrlRequest(BaseModel):
    proxy_url: str = Field(..., min_length=1, max_length=500)
    username: str | None = Field(None, max_length=255)
    password: str | None = Field(None, max_length=500)


@dataclass
class AdminTestProxyUrlAdapter(AdminApiAdapter):
    """通过 proxy_url 直接测试代理连通性（无需已注册节点）"""

    name: str = "admin_test_proxy_url"

    async def handle(self, context: ApiRequestContext) -> Any:
        payload = context.ensure_json_body()
        try:
            req = TestProxyUrlRequest.model_validate(payload)
        except ValidationError as exc:
            raise InvalidRequestException("输入验证失败: " + _format_validation_error(exc))

        return await ProxyNodeService.test_proxy_url(
            proxy_url=req.proxy_url,
            username=req.username,
            password=req.password,
        )


@dataclass
class AdminListProxyNodeEventsAdapter(AdminApiAdapter):
    """查询代理节点连接事件（连接/断开/错误历史）"""

    name: str = "admin_list_proxy_node_events"
    node_id: str = ""
    limit: int = 50

    async def handle(self, context: ApiRequestContext) -> Any:
        from src.models.database import ProxyNodeEvent

        events = (
            context.db.query(ProxyNodeEvent)
            .filter(ProxyNodeEvent.node_id == self.node_id)
            .order_by(ProxyNodeEvent.created_at.desc())
            .limit(self.limit)
            .all()
        )
        return {
            "items": [
                {
                    "id": e.id,
                    "event_type": e.event_type,
                    "detail": e.detail,
                    "created_at": e.created_at,
                }
                for e in events
            ],
        }
