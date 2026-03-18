from __future__ import annotations

import asyncio
import ipaddress
from typing import Any

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from src.services.proxy_node.service import ProxyNodeService, build_heartbeat_ack

router = APIRouter(prefix="/api/internal/hub", tags=["Internal - Hub"], include_in_schema=False)


class HubHeartbeatRequest(BaseModel):
    node_id: str = Field(..., min_length=1, max_length=36)
    heartbeat_interval: int | None = Field(None, ge=5, le=600)
    active_connections: int | None = Field(None, ge=0)
    total_requests: int | None = Field(None, ge=0)
    avg_latency_ms: float | None = Field(None, ge=0)
    failed_requests: int | None = Field(None, ge=0)
    dns_failures: int | None = Field(None, ge=0)
    stream_errors: int | None = Field(None, ge=0)
    proxy_metadata: dict[str, Any] | None = None
    proxy_version: str | None = Field(None, max_length=20)


class HubNodeStatusRequest(BaseModel):
    node_id: str = Field(..., min_length=1, max_length=36)
    connected: bool
    conn_count: int = Field(0, ge=0)


def _ensure_loopback(request: Request) -> None:
    host = request.client.host if request.client else ""
    try:
        if not ipaddress.ip_address(host).is_loopback:
            raise ValueError(host)
    except ValueError as exc:
        raise HTTPException(status_code=403, detail="loopback access only") from exc


@router.post("/heartbeat")
async def hub_heartbeat(request: Request, payload: HubHeartbeatRequest) -> dict[str, Any]:
    _ensure_loopback(request)

    def _sync_apply() -> dict[str, Any]:
        from src.database import create_session

        db = create_session()
        try:
            node = ProxyNodeService.heartbeat(
                db,
                node_id=payload.node_id,
                heartbeat_interval=payload.heartbeat_interval,
                active_connections=payload.active_connections,
                total_requests=payload.total_requests,
                avg_latency_ms=payload.avg_latency_ms,
                failed_requests=payload.failed_requests,
                dns_failures=payload.dns_failures,
                stream_errors=payload.stream_errors,
                proxy_metadata=payload.proxy_metadata,
                proxy_version=payload.proxy_version,
            )
            return build_heartbeat_ack(node)
        finally:
            db.close()

    try:
        return await asyncio.to_thread(_sync_apply)
    except Exception as exc:
        raise HTTPException(status_code=500, detail=f"heartbeat sync failed: {exc}") from exc


@router.post("/node-status")
async def hub_node_status(request: Request, payload: HubNodeStatusRequest) -> dict[str, Any]:
    _ensure_loopback(request)

    def _sync_apply() -> dict[str, Any]:
        from src.database import create_session

        db = create_session()
        try:
            node = ProxyNodeService.update_tunnel_status(
                db,
                node_id=payload.node_id,
                connected=payload.connected,
                conn_count=payload.conn_count,
            )
            return {"updated": node is not None}
        finally:
            db.close()

    try:
        return await asyncio.to_thread(_sync_apply)
    except Exception as exc:
        raise HTTPException(status_code=500, detail=f"node status sync failed: {exc}") from exc
