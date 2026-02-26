"""Claude Code adapter constants."""

from __future__ import annotations

CLAUDE_MESSAGES_PATH = "/v1/messages"
DEFAULT_ANTHROPIC_VERSION = "2023-06-01"
DEFAULT_ACCEPT = "application/json"
STREAM_HELPER_METHOD = "stream"
SESSION_ID_MASKING_TTL_SECONDS = 15 * 60
# 仅代表“启用 Claude Code TLS 配置”的客户端 profile 标识（best-effort）。
TLS_PROFILE_CLAUDE_CODE = "claude_code_nodejs"

# Claude Code OAuth required betas.
BETA_CLAUDE_CODE = "claude-code-20250219"
BETA_OAUTH = "oauth-2025-04-20"
BETA_INTERLEAVED_THINKING = "interleaved-thinking-2025-05-14"
BETA_CONTEXT_1M = "context-1m-2025-08-07"

CLAUDE_CODE_REQUIRED_BETA_TOKENS: tuple[str, ...] = (
    BETA_CLAUDE_CODE,
    BETA_OAUTH,
    BETA_INTERLEAVED_THINKING,
)

# Mimic headers observed from Claude Code traffic.
CLAUDE_CODE_DEFAULT_HEADERS: dict[str, str] = {
    "X-Stainless-Lang": "js",
    "X-Stainless-Package-Version": "0.70.0",
    "X-Stainless-OS": "Linux",
    "X-Stainless-Arch": "arm64",
    "X-Stainless-Runtime": "node",
    "X-Stainless-Runtime-Version": "v24.13.0",
    "X-Stainless-Retry-Count": "0",
    "X-Stainless-Timeout": "600",
    "X-App": "cli",
    "Anthropic-Dangerous-Direct-Browser-Access": "true",
}

__all__ = [
    "BETA_CLAUDE_CODE",
    "BETA_CONTEXT_1M",
    "BETA_INTERLEAVED_THINKING",
    "BETA_OAUTH",
    "CLAUDE_CODE_DEFAULT_HEADERS",
    "CLAUDE_CODE_REQUIRED_BETA_TOKENS",
    "CLAUDE_MESSAGES_PATH",
    "DEFAULT_ACCEPT",
    "DEFAULT_ANTHROPIC_VERSION",
    "SESSION_ID_MASKING_TTL_SECONDS",
    "STREAM_HELPER_METHOD",
    "TLS_PROFILE_CLAUDE_CODE",
]
