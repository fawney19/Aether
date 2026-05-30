#!/usr/bin/env python3
"""Small HTTP helper that runs Aether Docker Compose update commands.

This helper is intended to run as an optional sidecar with access to the host
Docker socket and the deployment directory. The main app talks to it over the
private Compose network with a shared token.
"""

from __future__ import annotations

import json
import os
import re
import shlex
import subprocess
import threading
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


DEFAULT_UPDATE_COMMAND = "bash ./update.sh"

TOKEN = os.environ.get("AETHER_DOCKER_UPDATE_TOKEN", "").strip()
BASE_COMMAND = os.environ.get("AETHER_DOCKER_UPDATE_COMMAND", DEFAULT_UPDATE_COMMAND).strip() or DEFAULT_UPDATE_COMMAND
WORKDIR = os.environ.get("AETHER_DOCKER_UPDATE_WORKDIR", "/workspace").strip() or "/workspace"
LISTEN_HOST = os.environ.get("AETHER_DOCKER_UPDATE_LISTEN", "0.0.0.0").strip() or "0.0.0.0"
LISTEN_PORT = int(os.environ.get("AETHER_DOCKER_UPDATE_PORT", "18086"))
TIMEOUT_SECS = int(os.environ.get("AETHER_DOCKER_UPDATE_TIMEOUT_SECS", "900"))
OUTPUT_TAIL_CHARS = int(os.environ.get("AETHER_DOCKER_UPDATE_OUTPUT_TAIL_CHARS", "12000"))

operation_lock = threading.Lock()
status_lock = threading.Lock()
operation_status: dict[str, object] = {
    "status": "idle",
    "phase": "idle",
    "operation": None,
    "running": False,
    "output": None,
    "detail": None,
    "exit_code": None,
    "progress_label": None,
    "downloaded_bytes": None,
    "total_bytes": None,
    "progress_percent": None,
    "started_at": None,
    "updated_at": None,
}

ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
PERCENT_RE = re.compile(r"(?<!\d)(100|[0-9]{1,2})(?:\.\d+)?%")
SIZE_RE = re.compile(
    r"(?P<done>[0-9]+(?:\.[0-9]+)?)\s*(?P<done_unit>[kmgt]?i?b|b)"
    r"\s*/\s*"
    r"(?P<total>[0-9]+(?:\.[0-9]+)?)\s*(?P<total_unit>[kmgt]?i?b|b)",
    re.IGNORECASE,
)


def response_body(result: str, **fields: object) -> bytes:
    payload = {"status": result, **fields}
    return json.dumps(payload, ensure_ascii=False).encode("utf-8")


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def command_for(path: str) -> list[str] | None:
    base = shlex.split(BASE_COMMAND)
    if path == "/prepare":
        return [*base, "--prepare"]
    if path == "/apply":
        return [*base, "--apply-prepared"]
    if path == "/full":
        return base
    return None


def output_tail(value: str) -> str:
    if len(value) <= OUTPUT_TAIL_CHARS:
        return value
    return value[-OUTPUT_TAIL_CHARS:]


def clean_output_line(value: str) -> str:
    return ANSI_RE.sub("", value.replace("\r", "\n")).strip()


def parse_size(value: str, unit: str) -> int:
    multipliers = {
        "b": 1,
        "kb": 1000,
        "kib": 1024,
        "mb": 1000**2,
        "mib": 1024**2,
        "gb": 1000**3,
        "gib": 1024**3,
        "tb": 1000**4,
        "tib": 1024**4,
    }
    return int(float(value) * multipliers.get(unit.lower(), 1))


def parse_progress(line: str) -> tuple[int | None, int | None, int | None]:
    percent: int | None = None
    bytes_match = SIZE_RE.search(line)
    downloaded_bytes: int | None = None
    total_bytes: int | None = None
    if bytes_match:
        downloaded_bytes = parse_size(bytes_match.group("done"), bytes_match.group("done_unit"))
        total_bytes = parse_size(bytes_match.group("total"), bytes_match.group("total_unit"))
        if total_bytes > 0:
            percent = min(100, int(downloaded_bytes * 100 / total_bytes))

    percent_match = PERCENT_RE.search(line)
    if percent_match:
        percent = int(percent_match.group(1))

    if percent is not None:
        percent = min(percent, 95)
    return percent, downloaded_bytes, total_bytes


def status_snapshot() -> dict[str, object]:
    with status_lock:
        return dict(operation_status)


def update_status(**fields: object) -> None:
    with status_lock:
        operation_status.update(fields)
        operation_status["updated_at"] = now_iso()


def append_status_output(line: str) -> None:
    if not line:
        return
    with status_lock:
        current = operation_status.get("output")
        output = f"{current or ''}{line}\n"
        operation_status["output"] = output_tail(output)
        operation_status["updated_at"] = now_iso()


def reset_operation_status(operation: str) -> None:
    phase = "downloading" if operation == "prepare" else "restarting"
    label = "docker_image" if operation == "prepare" else "container"
    update_status(
        status="running",
        phase=phase,
        operation=operation,
        running=True,
        output="",
        detail=None,
        exit_code=None,
        progress_label=label,
        downloaded_bytes=None,
        total_bytes=None,
        progress_percent=0 if operation == "prepare" else None,
        started_at=now_iso(),
    )


def update_progress_from_line(line: str, operation: str) -> None:
    if not line:
        return
    lower = line.lower()
    fields: dict[str, object] = {}
    if operation == "prepare":
        fields["phase"] = "downloading"
        fields["progress_label"] = "docker_image"
        current_percent = int(status_snapshot().get("progress_percent") or 0)
        if "extracting" in lower:
            fields["progress_percent"] = max(current_percent, 80)
        elif "verifying" in lower:
            fields["progress_percent"] = max(current_percent, 90)
        elif "pulled" in lower or "download complete" in lower:
            fields["progress_percent"] = max(current_percent, 95)
        elif "downloading" in lower:
            fields["progress_percent"] = max(current_percent, 5)
        percent, downloaded_bytes, total_bytes = parse_progress(line)
        if percent is not None:
            fields["progress_percent"] = max(current_percent, percent)
        if downloaded_bytes is not None:
            fields["downloaded_bytes"] = downloaded_bytes
        if total_bytes is not None:
            fields["total_bytes"] = total_bytes
    elif "backing up" in lower or "pg_dump" in lower:
        fields["phase"] = "backing_up"
        fields["progress_label"] = "database"
    else:
        fields["phase"] = "restarting"
        fields["progress_label"] = "container"

    if fields:
        update_status(**fields)


def run_command_streaming(command: list[str], operation: str) -> tuple[int, str]:
    env = os.environ.copy()
    env.setdefault("COMPOSE_PROGRESS", "plain")
    env.setdefault("BUILDKIT_PROGRESS", "plain")
    env.setdefault("DOCKER_CLI_HINTS", "false")

    reset_operation_status(operation)
    process = subprocess.Popen(
        command,
        cwd=WORKDIR,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        bufsize=1,
        env=env,
    )

    def reader() -> None:
        assert process.stdout is not None
        for raw_line in process.stdout:
            for line in clean_output_line(raw_line).splitlines():
                append_status_output(line)
                update_progress_from_line(line, operation)

    reader_thread = threading.Thread(target=reader, daemon=True)
    reader_thread.start()

    try:
        exit_code = process.wait(timeout=TIMEOUT_SECS)
    except subprocess.TimeoutExpired:
        process.kill()
        exit_code = process.wait()
        reader_thread.join(timeout=2)
        detail = f"command timed out after {TIMEOUT_SECS}s"
        update_status(
            status="timeout",
            phase="failed",
            running=False,
            detail=detail,
            exit_code=exit_code,
        )
        return exit_code, detail

    reader_thread.join(timeout=2)
    snapshot = status_snapshot()
    output = str(snapshot.get("output") or "")
    if exit_code == 0:
        terminal_phase = "prepared" if operation == "prepare" else "restarting"
        update_status(
            status="ok",
            phase=terminal_phase,
            running=False,
            exit_code=exit_code,
            progress_percent=100,
        )
    else:
        update_status(
            status="failed",
            phase="failed",
            running=False,
            detail=f"command exited with {exit_code}",
            exit_code=exit_code,
        )
    return exit_code, output_tail(output)


class Handler(BaseHTTPRequestHandler):
    server_version = "AetherDockerUpdateHelper/1.0"

    def log_message(self, fmt: str, *args: object) -> None:
        print("%s - %s" % (self.address_string(), fmt % args), flush=True)

    def send_json(self, status: HTTPStatus, body: bytes) -> None:
        self.send_response(status)
        self.send_header("content-type", "application/json; charset=utf-8")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def authorized(self) -> bool:
        if not TOKEN:
            return False
        return self.headers.get("x-aether-update-token", "") == TOKEN

    def do_GET(self) -> None:
        if self.path not in {"/health", "/status"}:
            self.send_json(HTTPStatus.NOT_FOUND, response_body("not_found"))
            return
        if not self.authorized():
            self.send_json(HTTPStatus.UNAUTHORIZED, response_body("unauthorized"))
            return
        if self.path == "/status":
            self.send_json(HTTPStatus.OK, response_body("ok", **status_snapshot()))
            return
        self.send_json(
            HTTPStatus.OK,
            response_body(
                "ok",
                workdir=WORKDIR,
                command=BASE_COMMAND,
                running=operation_lock.locked(),
            ),
        )

    def do_POST(self) -> None:
        command = command_for(self.path)
        if command is None:
            self.send_json(HTTPStatus.NOT_FOUND, response_body("not_found"))
            return
        if not self.authorized():
            self.send_json(HTTPStatus.UNAUTHORIZED, response_body("unauthorized"))
            return
        if not operation_lock.acquire(blocking=False):
            self.send_json(HTTPStatus.CONFLICT, response_body("busy", detail="update is already running"))
            return

        try:
            operation = self.path.strip("/") or "full"
            exit_code, output = run_command_streaming(command, operation)
            if exit_code == 0:
                self.send_json(
                    HTTPStatus.OK,
                    response_body("ok", exit_code=exit_code, output=output),
                )
            else:
                snapshot = status_snapshot()
                self.send_json(
                    HTTPStatus.INTERNAL_SERVER_ERROR,
                    response_body(
                        "failed",
                        exit_code=exit_code,
                        output=output,
                        detail=str(snapshot.get("detail") or f"command exited with {exit_code}"),
                    ),
                )
        except Exception as exc:  # noqa: BLE001 - report sidecar failures as JSON.
            update_status(status="failed", phase="failed", running=False, detail=str(exc))
            self.send_json(HTTPStatus.INTERNAL_SERVER_ERROR, response_body("failed", detail=str(exc)))
        finally:
            operation_lock.release()


def main() -> None:
    if not TOKEN:
        raise SystemExit("AETHER_DOCKER_UPDATE_TOKEN is required")
    if not os.path.isdir(WORKDIR):
        raise SystemExit(f"AETHER_DOCKER_UPDATE_WORKDIR is not a directory: {WORKDIR}")
    server = ThreadingHTTPServer((LISTEN_HOST, LISTEN_PORT), Handler)
    print(f"listening on {LISTEN_HOST}:{LISTEN_PORT}, workdir={WORKDIR}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
