from __future__ import annotations

import gzip
import json
import time
import uuid
from dataclasses import dataclass, field
from typing import Any

from fastapi import HTTPException, Request
from sqlalchemy.orm import Session

from src.core.api_format.headers import get_header_value
from src.core.http_compression import is_gzip_content_encoding, normalize_content_encoding
from src.core.logger import logger
from src.models.database import ApiKey, ManagementToken, User
from src.utils.perf import PerfRecorder
from src.utils.request_utils import get_client_ip


@dataclass
class ApiRequestContext:
    """统一的API请求上下文，贯穿Pipeline与格式适配器。"""

    request: Request
    db: Session
    user: User | None
    api_key: ApiKey | None
    request_id: str
    start_time: float
    client_ip: str
    user_agent: str
    original_headers: dict[str, str]
    query_params: dict[str, str]
    raw_body: bytes | None = None
    json_body: dict[str, Any] | None = None
    balance_remaining: float | None = None
    mode: str = "standard"  # standard / proxy
    api_format_hint: str | None = None

    # URL 路径参数（如 Gemini API 的 /v1beta/models/{model}:generateContent）
    path_params: dict[str, Any] = field(default_factory=dict)

    # Management Token（用于管理 API 认证）
    management_token: ManagementToken | None = None

    # 供适配器扩展的状态存储
    extra: dict[str, Any] = field(default_factory=dict)
    audit_metadata: dict[str, Any] = field(default_factory=dict)

    # 高频轮询端点日志抑制标志
    quiet_logging: bool = False
    client_content_encoding: str | None = None
    client_accept_encoding: str | None = None

    def ensure_json_body(self) -> dict[str, Any]:
        """确保请求体已解析为JSON并返回。"""
        if self.json_body is not None:
            return self.json_body

        if not self.raw_body:
            raise HTTPException(status_code=400, detail="请求体不能为空")

        perf_metrics = getattr(self.request.state, "perf_metrics", None)
        perf_sampled = isinstance(perf_metrics, dict) and bool(perf_metrics)
        parse_start = PerfRecorder.start(force=perf_sampled)

        def _record_parse_duration(duration: float | None) -> None:
            if duration is None:
                return
            if not isinstance(perf_metrics, dict):
                return
            perf_metrics.setdefault("pipeline", {})["json_parse_ms"] = int(duration * 1000)

        body_to_parse = self.raw_body
        content_encoding = self.client_content_encoding or normalize_content_encoding(
            get_header_value(self.original_headers, "content-encoding")
        )
        if is_gzip_content_encoding(content_encoding):
            try:
                body_to_parse = gzip.decompress(body_to_parse)
            except OSError as exc:
                parse_duration = PerfRecorder.stop(
                    parse_start,
                    "pipeline_json_parse",
                    labels={"mode": self.mode},
                )
                _record_parse_duration(parse_duration)
                logger.warning("gzip 请求体解压失败: {}", exc)
                raise HTTPException(status_code=400, detail="gzip 请求体解压失败") from exc

        try:
            self.json_body = json.loads(body_to_parse.decode("utf-8"))
            parse_duration = PerfRecorder.stop(
                parse_start,
                "pipeline_json_parse",
                labels={"mode": self.mode},
            )
            _record_parse_duration(parse_duration)
        except json.JSONDecodeError as exc:
            parse_duration = PerfRecorder.stop(
                parse_start,
                "pipeline_json_parse",
                labels={"mode": self.mode},
            )
            _record_parse_duration(parse_duration)
            logger.warning(f"解析JSON失败: {exc}")
            raise HTTPException(status_code=400, detail="请求体必须是合法的JSON") from exc

        return self.json_body

    def add_audit_metadata(self, **values: Any) -> None:
        """向审计日志附加字段（会自动过滤 None）。"""
        for key, value in values.items():
            if value is not None:
                self.audit_metadata[key] = value

    def extend_audit_metadata(self, data: dict[str, Any]) -> None:
        """批量附加审计字段。"""
        for key, value in data.items():
            if value is not None:
                self.audit_metadata[key] = value

    @classmethod
    def build(
        cls,
        request: Request,
        db: Session,
        user: User | None,
        api_key: ApiKey | None,
        raw_body: bytes | None = None,
        mode: str = "standard",
        api_format_hint: str | None = None,
        path_params: dict[str, Any] | None = None,
    ) -> ApiRequestContext:
        """创建上下文实例并提前读取必要的元数据。"""
        request_id = getattr(request.state, "request_id", None) or str(uuid.uuid4())[:8]
        setattr(request.state, "request_id", request_id)

        start_time = time.time()
        client_ip = get_client_ip(request)
        user_agent = request.headers.get("user-agent", "unknown")
        client_content_encoding = normalize_content_encoding(
            request.headers.get("content-encoding")
        )
        client_accept_encoding = request.headers.get("accept-encoding")
        if isinstance(client_accept_encoding, str):
            client_accept_encoding = client_accept_encoding.strip() or None

        context = cls(
            request=request,
            db=db,
            user=user,
            api_key=api_key,
            request_id=request_id,
            start_time=start_time,
            client_ip=client_ip,
            user_agent=user_agent,
            original_headers=dict(request.headers),
            query_params=dict(request.query_params),
            raw_body=raw_body,
            mode=mode,
            api_format_hint=api_format_hint,
            path_params=path_params or {},
            client_content_encoding=client_content_encoding,
            client_accept_encoding=client_accept_encoding,
        )

        perf_metrics = getattr(request.state, "perf_metrics", None)
        if isinstance(perf_metrics, dict) and perf_metrics:
            context.extra["perf"] = perf_metrics

        # 便于插件/日志引用
        request.state.request_id = request_id
        if user:
            request.state.user_id = user.id
        if api_key:
            request.state.api_key_id = api_key.id

        return context
