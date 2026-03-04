from __future__ import annotations

from enum import Enum
from typing import Any


class CandidateErrorAction(str, Enum):
    """同步执行中，候选错误处理后的标准动作。"""

    RETRY_CURRENT = "retry_current"
    NEXT_CANDIDATE = "next_candidate"
    RAISE_ERROR = "raise_error"


def classify_candidate_error_action(action: Any) -> CandidateErrorAction:
    """将 error_handler 的字符串动作分类为可控枚举。"""
    action_norm = str(action or "").strip().lower()
    if action_norm == "continue":
        return CandidateErrorAction.RETRY_CURRENT
    if action_norm == "raise":
        return CandidateErrorAction.RAISE_ERROR
    if action_norm == "break":
        return CandidateErrorAction.NEXT_CANDIDATE
    # Fail-safe：未知动作默认切到下一个候选，避免卡死在当前候选。
    return CandidateErrorAction.NEXT_CANDIDATE
