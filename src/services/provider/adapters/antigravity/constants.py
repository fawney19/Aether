"""Antigravity 全局常量定义。

注意：这里的 PROVIDER_TYPE 指的是 Provider.provider_type（用于路由与特判），
不是 endpoint signature（family:kind）。
"""

from __future__ import annotations

import platform
import re
import threading

# ============== API 端点 ==============
PROD_BASE_URL = "https://cloudcode-pa.googleapis.com"
DAILY_BASE_URL = "https://daily-cloudcode-pa.googleapis.com"
SANDBOX_BASE_URL = "https://daily-cloudcode-pa.sandbox.googleapis.com"

# ============== User-Agent ==============
VERSION_FETCH_URL = "https://antigravity-auto-updater-974169037036.us-central1.run.app"
_FALLBACK_VERSION = "1.15.8"
_VERSION_RE = re.compile(r"\d+\.\d+\.\d+")


def _detect_platform_tag() -> str:
    """检测当前运行平台，格式与 Go runtime 保持一致。"""
    os_name = platform.system().lower()  # linux, darwin, windows
    arch = platform.machine().lower()
    if arch in ("x86_64", "amd64"):
        arch = "amd64"
    elif arch in ("aarch64", "arm64"):
        arch = "arm64"
    return f"{os_name}/{arch}"


_PLATFORM_TAG = _detect_platform_tag()

# HTTP Header User-Agent（向后兼容：模块级常量保留，新代码应使用 get_http_user_agent()）
HTTP_USER_AGENT = f"antigravity/{_FALLBACK_VERSION} {_PLATFORM_TAG}"

# V1InternalRequest.userAgent 字段（固定值）
REQUEST_USER_AGENT = "antigravity"

# --- 动态 User-Agent 支持 ---
_ua_lock = threading.Lock()
_ua_version: str = _FALLBACK_VERSION


def get_http_user_agent() -> str:
    """返回当前 HTTP User-Agent 字符串（支持动态版本号更新）。"""
    with _ua_lock:
        return f"antigravity/{_ua_version} {_PLATFORM_TAG}"


def update_user_agent_version(version: str) -> None:
    """更新 User-Agent 中的版本号（由 refresh_user_agent 调用）。"""
    global HTTP_USER_AGENT, _ua_version  # noqa: PLW0603
    version = str(version or "").strip()
    if not version:
        return
    with _ua_lock:
        _ua_version = version
        # Backward compat: keep module-level constant in sync.
        HTTP_USER_AGENT = f"antigravity/{_ua_version} {_PLATFORM_TAG}"

def parse_version_string(text: str) -> str | None:
    """从任意文本中提取 X.Y.Z 格式的版本号。"""
    m = _VERSION_RE.search(text)
    return m.group(0) if m else None


# ============== URL 可用性 ==============
URL_UNAVAILABLE_TTL_SECONDS = 300  # 5 分钟

# ============== Thinking Signature ==============
DUMMY_THOUGHT_SIGNATURE = "skip_thought_signature_validator"
MIN_SIGNATURE_LENGTH = 50  # 与 Antigravity-Manager 对齐

# ============== Thinking Budget ==============
THINKING_BUDGET_AUTO_CAP = 24576
THINKING_BUDGET_DEFAULT_INJECT = 24576  # 对齐 AM wrapper.rs (was 16000)
# 给输出留的空间（对齐 Antigravity-Manager：普通模型 8192，图像模型 2048）
OUTPUT_OVERHEAD = 8192
OUTPUT_OVERHEAD_IMAGE = 2048
# 模型最大输出限制（防止超限）
MODEL_MAX_OUTPUT_LIMIT = 65536
# 包含这些关键字的模型会自动注入 thinkingConfig（如果缺失）
THINKING_MODELS_AUTO_INJECT_KEYWORDS = ("thinking", "gemini-2.0-pro", "gemini-3-pro")

# ============== Model Alias Mapping ==============
# 对齐 AM common_utils.rs: 将预览/别名映射回上游物理模型名
MODEL_ALIAS_MAP: dict[str, str] = {
    "gemini-3-pro-preview": "gemini-3-pro-high",
    "gemini-3-pro-image-preview": "gemini-3-pro-image",
    "gemini-3-flash-preview": "gemini-3-flash",
}

# ============== Google Search (Grounding) ==============
# 对齐 AM common_utils.rs: 仅 gemini-2.5-flash 支持 googleSearch tool
WEB_SEARCH_MODEL = "gemini-2.5-flash"
# 联网工具检测关键字（对齐 AM detects_networking_tool）
NETWORKING_TOOL_KEYWORDS = frozenset(
    {
        "web_search",
        "google_search",
        "web_search_20250305",
        "google_search_retrieval",
    }
)

# ============== Image Generation ==============
# 上游图像生成模型的固定名称
IMAGE_GEN_UPSTREAM_MODEL = "gemini-3-pro-image"
# 模型后缀 → 宽高比映射
IMAGE_ASPECT_RATIO_SUFFIXES: dict[str, str] = {
    "-21x9": "21:9",
    "-21-9": "21:9",
    "-16x9": "16:9",
    "-16-9": "16:9",
    "-9x16": "9:16",
    "-9-16": "9:16",
    "-4x3": "4:3",
    "-4-3": "4:3",
    "-3x4": "3:4",
    "-3-4": "3:4",
    "-3x2": "3:2",
    "-3-2": "3:2",
    "-2x3": "2:3",
    "-2-3": "2:3",
    "-5x4": "5:4",
    "-5-4": "5:4",
    "-4x5": "4:5",
    "-4-5": "4:5",
    "-1x1": "1:1",
    "-1-1": "1:1",
}
# 标准宽高比字符串（用于直接匹配 size 参数）
STANDARD_ASPECT_RATIOS = frozenset(
    {
        "21:9",
        "16:9",
        "9:16",
        "4:3",
        "3:4",
        "3:2",
        "2:3",
        "5:4",
        "4:5",
        "1:1",
    }
)
# 宽高比容差匹配表：(ratio, label)
ASPECT_RATIO_TABLE: tuple[tuple[float, str], ...] = (
    (21.0 / 9.0, "21:9"),
    (16.0 / 9.0, "16:9"),
    (4.0 / 3.0, "4:3"),
    (3.0 / 4.0, "3:4"),
    (9.0 / 16.0, "9:16"),
    (3.0 / 2.0, "3:2"),
    (2.0 / 3.0, "2:3"),
    (5.0 / 4.0, "5:4"),
    (4.0 / 5.0, "4:5"),
    (1.0, "1:1"),
)

# ============== Retry ==============
RETRY_429_BASE_SECONDS = 5.0
RETRY_503_BASE_SECONDS = 10.0
RETRY_503_MAX_SECONDS = 60.0
RETRY_500_BASE_SECONDS = 3.0

# ============== v1internal 路径 ==============
V1INTERNAL_PATH_TEMPLATE = "/v1internal:{action}"

# ============== Signature 错误关键字（用于 400 错误检测） ==============
SIGNATURE_ERROR_KEYWORDS = (
    "Invalid `signature`",
    "thinking.signature",
    "thinking.thinking",
    "Corrupted thought signature",
)

# ============== Antigravity System Instruction ==============
ANTIGRAVITY_SYSTEM_INSTRUCTION = (
    "You are Antigravity, a powerful agentic AI coding assistant designed by the "
    "Google Deepmind team working on Advanced Agentic Coding.\n"
    "You are pair programming with a USER to solve their coding task. The task may "
    "require creating a new codebase, modifying or debugging an existing codebase, "
    "or simply answering a question.\n"
    "**Absolute paths only**\n"
    "**Proactiveness**"
)

__all__ = [
    "ANTIGRAVITY_SYSTEM_INSTRUCTION",
    "ASPECT_RATIO_TABLE",
    "DAILY_BASE_URL",
    "DUMMY_THOUGHT_SIGNATURE",
    "HTTP_USER_AGENT",
    "IMAGE_ASPECT_RATIO_SUFFIXES",
    "IMAGE_GEN_UPSTREAM_MODEL",
    "MIN_SIGNATURE_LENGTH",
    "MODEL_ALIAS_MAP",
    "MODEL_MAX_OUTPUT_LIMIT",
    "NETWORKING_TOOL_KEYWORDS",
    "OUTPUT_OVERHEAD",
    "OUTPUT_OVERHEAD_IMAGE",
    "PROD_BASE_URL",
    "REQUEST_USER_AGENT",
    "RETRY_429_BASE_SECONDS",
    "RETRY_500_BASE_SECONDS",
    "RETRY_503_BASE_SECONDS",
    "RETRY_503_MAX_SECONDS",
    "SANDBOX_BASE_URL",
    "SIGNATURE_ERROR_KEYWORDS",
    "STANDARD_ASPECT_RATIOS",
    "THINKING_BUDGET_AUTO_CAP",
    "THINKING_BUDGET_DEFAULT_INJECT",
    "THINKING_MODELS_AUTO_INJECT_KEYWORDS",
    "URL_UNAVAILABLE_TTL_SECONDS",
    "V1INTERNAL_PATH_TEMPLATE",
    "VERSION_FETCH_URL",
    "WEB_SEARCH_MODEL",
    "get_http_user_agent",
    "parse_version_string",
    "update_user_agent_version",
]
