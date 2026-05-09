"""Codex Responses WebSocket bridge.

The current Codex client can use a WebSocket transport for the Responses API:
it sends JSON text request frames and expects each Responses streaming event as
one JSON text message. Aether already speaks Responses over HTTP/SSE, so this
module keeps the business path intact and only adapts the wire protocol.
"""

from __future__ import annotations

import codecs
import json
import uuid
from collections.abc import AsyncIterator, Callable
from dataclasses import dataclass
from typing import Any

from fastapi import HTTPException, WebSocket, WebSocketDisconnect
from fastapi.responses import Response, StreamingResponse
from sqlalchemy.orm import Session
from starlette.requests import Request
from starlette.websockets import WebSocketState

from src.api.base.adapter import ApiMode
from src.api.base.pipeline import ApiRequestPipeline
from src.api.handlers.openai_cli import OpenAICliAdapter
from src.core.logger import logger
from src.database.database import create_session

CODEX_TURN_STATE_HEADER = "x-codex-turn-state"

_HOP_BY_HOP_WS_HEADERS = {
    "connection",
    "upgrade",
    "sec-websocket-accept",
    "sec-websocket-extensions",
    "sec-websocket-key",
    "sec-websocket-protocol",
    "sec-websocket-version",
}


@dataclass(frozen=True)
class CodexWsRequest:
    kind: str
    body: dict[str, Any] | None = None


class CodexWsProtocolError(ValueError):
    def __init__(
        self,
        message: str,
        *,
        status_code: int = 400,
        error_type: str = "invalid_request_error",
    ) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.error_type = error_type


def parse_codex_ws_request(text: str) -> CodexWsRequest:
    """Parse one Codex WebSocket request text frame."""
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as exc:
        raise CodexWsProtocolError("Request frame must be valid JSON") from exc

    if not isinstance(payload, dict):
        raise CodexWsProtocolError("Request frame must be a JSON object")

    kind = payload.get("type")
    if not isinstance(kind, str) or not kind:
        raise CodexWsProtocolError("Request frame must include a string type")

    if kind == "response.processed":
        response_id = payload.get("response_id")
        if response_id is not None and not isinstance(response_id, str):
            raise CodexWsProtocolError("response.processed response_id must be a string")
        return CodexWsRequest(kind=kind)

    if kind == "response.create":
        return CodexWsRequest(kind=kind, body=response_create_http_body(payload))

    raise CodexWsProtocolError(f"Unsupported Codex websocket request type: {kind}")


def response_create_http_body(payload: dict[str, Any]) -> dict[str, Any]:
    """Convert a Codex response.create frame to a Responses HTTP body."""
    if payload.get("type") != "response.create":
        raise CodexWsProtocolError("Expected response.create request frame")

    body = dict(payload)
    body.pop("type", None)
    if not body:
        raise CodexWsProtocolError("response.create request body cannot be empty")
    return body


def extract_sse_payloads(block: str) -> list[str]:
    """Extract JSON payload strings from one SSE event block."""
    data_lines: list[str] = []
    stripped_block = block.strip()
    for raw_line in block.splitlines():
        line = raw_line.strip()
        if not line or line.startswith(":"):
            continue
        if line.startswith("data:"):
            data_lines.append(line[5:].lstrip())
        elif line.startswith("event:") and " data:" in line:
            data_lines.append(line.split(" data:", 1)[1].strip())

    if data_lines:
        payload = "\n".join(data_lines).strip()
        if payload and payload != "[DONE]":
            return [payload]
        return []

    if stripped_block.startswith("{"):
        return [stripped_block]
    return []


async def iter_sse_payloads(chunks: AsyncIterator[bytes | str]) -> AsyncIterator[str]:
    """Yield JSON payload strings from an SSE byte stream."""
    decoder = codecs.getincrementaldecoder("utf-8")()
    buffer = ""

    async for chunk in chunks:
        if isinstance(chunk, bytes):
            buffer += decoder.decode(chunk)
        else:
            buffer += chunk

        buffer = buffer.replace("\r\n", "\n").replace("\r", "\n")
        while "\n\n" in buffer:
            block, buffer = buffer.split("\n\n", 1)
            for payload in extract_sse_payloads(block):
                yield payload

    tail = decoder.decode(b"", final=True)
    if tail:
        buffer += tail
    buffer = buffer.replace("\r\n", "\n").replace("\r", "\n")
    if buffer.strip():
        for payload in extract_sse_payloads(buffer):
            yield payload


def build_codex_ws_error_event(
    *,
    status_code: int,
    message: str,
    error_type: str | None = None,
    code: str | None = None,
    headers: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Build the wrapped error shape parsed by Codex's websocket client."""
    status_code = int(status_code or 500)
    if error_type is None:
        error_type = "server_error" if status_code >= 500 else "invalid_request_error"

    error: dict[str, Any] = {
        "type": error_type,
        "message": message,
    }
    if code:
        error["code"] = code

    event: dict[str, Any] = {
        "type": "error",
        "status": status_code,
        "error": error,
    }
    if headers:
        event["headers"] = dict(headers)
    return event


async def handle_codex_responses_websocket(
    websocket: WebSocket,
    *,
    pipeline: ApiRequestPipeline,
    db_factory: Callable[[], Session] | None = None,
) -> None:
    """Serve Codex Responses WebSocket traffic on top of the HTTP pipeline."""
    session_factory = db_factory or create_session
    turn_state = websocket.headers.get(CODEX_TURN_STATE_HEADER) or str(uuid.uuid4())
    await websocket.accept(headers=[(CODEX_TURN_STATE_HEADER.encode(), turn_state.encode())])

    logger.debug("[CodexWS] accepted websocket responses connection")

    while True:
        try:
            message = await websocket.receive()
        except WebSocketDisconnect:
            logger.debug("[CodexWS] websocket disconnected")
            return

        message_type = message.get("type")
        if message_type == "websocket.disconnect":
            logger.debug("[CodexWS] websocket disconnected")
            return

        if "bytes" in message and message.get("bytes") is not None:
            await _send_ws_error(
                websocket,
                status_code=400,
                message="Binary websocket frames are not supported",
                error_type="invalid_request_error",
            )
            await _close_websocket(websocket, code=1003)
            return

        text = message.get("text")
        if text is None:
            continue

        try:
            request = parse_codex_ws_request(text)
        except CodexWsProtocolError as exc:
            await _send_ws_error(
                websocket,
                status_code=exc.status_code,
                message=str(exc),
                error_type=exc.error_type,
            )
            continue

        if request.kind == "response.processed":
            logger.debug("[CodexWS] response.processed acknowledged")
            continue

        if request.body is None:
            await _send_ws_error(
                websocket,
                status_code=400,
                message="response.create frame is missing a request body",
            )
            continue

        await _run_response_create(
            websocket,
            pipeline=pipeline,
            db_factory=session_factory,
            body=request.body,
        )


async def _run_response_create(
    websocket: WebSocket,
    *,
    pipeline: ApiRequestPipeline,
    db_factory: Callable[[], Session],
    body: dict[str, Any],
) -> None:
    db = db_factory()
    response: Any = None
    try:
        http_request = _build_synthetic_request(websocket, body)
        adapter = OpenAICliAdapter()
        response = await pipeline.run(
            adapter=adapter,
            http_request=http_request,
            db=db,
            mode=ApiMode.PROXY,
            api_format_hint=adapter.allowed_api_formats[0],
        )
        db.commit()
    except HTTPException as exc:
        db.rollback()
        await _send_http_exception(websocket, exc)
        return
    except Exception as exc:
        db.rollback()
        logger.exception("[CodexWS] response.create failed")
        await _send_ws_error(
            websocket,
            status_code=500,
            message=str(exc) or "Internal server error",
            error_type="server_error",
        )
        return
    finally:
        db.close()

    await _send_pipeline_response(websocket, response)


def _build_synthetic_request(websocket: WebSocket, body: dict[str, Any]) -> Request:
    body_bytes = json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    headers = _build_http_headers_from_websocket(websocket)
    path = websocket.scope.get("path") or "/v1/responses"
    scheme = "https" if websocket.url.scheme == "wss" else "http"
    sent_body = False

    async def receive() -> dict[str, Any]:
        nonlocal sent_body
        if not sent_body:
            sent_body = True
            return {
                "type": "http.request",
                "body": body_bytes,
                "more_body": False,
            }
        return {
            "type": "http.request",
            "body": b"",
            "more_body": False,
        }

    scope: dict[str, Any] = {
        "type": "http",
        "asgi": websocket.scope.get("asgi", {"version": "3.0"}),
        "http_version": websocket.scope.get("http_version", "1.1"),
        "method": "POST",
        "scheme": scheme,
        "path": path,
        "raw_path": str(path).encode("ascii", errors="ignore"),
        "query_string": websocket.scope.get("query_string", b""),
        "root_path": websocket.scope.get("root_path", ""),
        "headers": headers,
        "client": websocket.scope.get("client"),
        "server": websocket.scope.get("server"),
        "state": {},
    }
    return Request(scope, receive)


def _build_http_headers_from_websocket(websocket: WebSocket) -> list[tuple[bytes, bytes]]:
    headers: list[tuple[bytes, bytes]] = []
    seen: set[str] = set()
    for key, value in websocket.headers.items():
        lower_key = key.lower()
        if lower_key in _HOP_BY_HOP_WS_HEADERS:
            continue
        seen.add(lower_key)
        headers.append((lower_key.encode("latin-1"), value.encode("latin-1")))

    if "content-type" not in seen:
        headers.append((b"content-type", b"application/json"))
    if "accept" not in seen:
        headers.append((b"accept", b"text/event-stream"))
    return headers


async def _send_pipeline_response(websocket: WebSocket, response: Any) -> None:
    if isinstance(response, StreamingResponse):
        await _send_streaming_response(websocket, response)
        return

    status_code = int(getattr(response, "status_code", 200) or 200)
    headers = _string_headers(getattr(response, "headers", None))
    content = _response_content(response)

    if status_code >= 400:
        await _send_error_from_content(
            websocket,
            status_code=status_code,
            content=content,
            headers=headers,
        )
        return

    if isinstance(content, dict) and content.get("type") == "response.completed":
        await websocket.send_text(json.dumps(content, ensure_ascii=False, separators=(",", ":")))
        return

    if isinstance(content, dict):
        await websocket.send_text(
            json.dumps(
                {
                    "type": "response.completed",
                    "response": content,
                },
                ensure_ascii=False,
                separators=(",", ":"),
            )
        )
        return

    await websocket.send_text(json.dumps(content, ensure_ascii=False, separators=(",", ":")))


async def _send_streaming_response(websocket: WebSocket, response: StreamingResponse) -> None:
    terminal_seen = False
    try:
        async for payload in iter_sse_payloads(response.body_iterator):
            if _is_terminal_payload(payload):
                terminal_seen = True
            await websocket.send_text(payload)
    except WebSocketDisconnect:
        logger.debug("[CodexWS] client disconnected while streaming response")
        return
    except Exception as exc:
        logger.exception("[CodexWS] failed while streaming response")
        await _send_ws_error(
            websocket,
            status_code=500,
            message=str(exc) or "Streaming response failed",
            error_type="server_error",
        )
        return
    finally:
        background = getattr(response, "background", None)
        if background is not None:
            await background()

    if not terminal_seen:
        await _send_ws_error(
            websocket,
            status_code=502,
            message="Stream closed before response.completed",
            error_type="server_error",
        )


def _is_terminal_payload(payload: str) -> bool:
    try:
        event = json.loads(payload)
    except json.JSONDecodeError:
        return False
    if not isinstance(event, dict):
        return False
    return event.get("type") in {"response.completed", "error"}


def _response_content(response: Any) -> Any:
    if isinstance(response, Response):
        body = getattr(response, "body", b"")
        if not body:
            return None
        try:
            return json.loads(body.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            return body.decode("utf-8", errors="replace")
    return response


async def _send_http_exception(websocket: WebSocket, exc: HTTPException) -> None:
    detail = exc.detail
    headers = {str(k): str(v) for k, v in (exc.headers or {}).items()}
    if isinstance(detail, dict):
        await _send_error_from_content(
            websocket,
            status_code=exc.status_code,
            content=detail,
            headers=headers,
        )
        return

    await _send_ws_error(
        websocket,
        status_code=exc.status_code,
        message=str(detail),
        headers=headers,
    )


async def _send_error_from_content(
    websocket: WebSocket,
    *,
    status_code: int,
    content: Any,
    headers: dict[str, str] | None = None,
) -> None:
    if isinstance(content, dict):
        error_obj = content.get("error")
        if isinstance(error_obj, dict):
            message = str(error_obj.get("message") or content)
            error_type = str(error_obj.get("type") or "invalid_request_error")
            code = error_obj.get("code")
            await _send_ws_error(
                websocket,
                status_code=status_code,
                message=message,
                error_type=error_type,
                code=str(code) if code is not None else None,
                headers=headers,
            )
            return
        if "detail" in content:
            await _send_ws_error(
                websocket,
                status_code=status_code,
                message=str(content["detail"]),
                headers=headers,
            )
            return

    await _send_ws_error(
        websocket,
        status_code=status_code,
        message=str(content),
        headers=headers,
    )


async def _send_ws_error(
    websocket: WebSocket,
    *,
    status_code: int,
    message: str,
    error_type: str | None = None,
    code: str | None = None,
    headers: dict[str, str] | None = None,
) -> None:
    if websocket.application_state != WebSocketState.CONNECTED:
        return
    await websocket.send_json(
        build_codex_ws_error_event(
            status_code=status_code,
            message=message,
            error_type=error_type,
            code=code,
            headers=headers,
        )
    )


async def _close_websocket(websocket: WebSocket, *, code: int) -> None:
    if websocket.application_state == WebSocketState.CONNECTED:
        await websocket.close(code=code)


def _string_headers(headers: Any) -> dict[str, str]:
    if not headers:
        return {}
    return {str(key): str(value) for key, value in headers.items()}
