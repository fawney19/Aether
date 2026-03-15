"""
输入验证器
包含密码复杂度验证和其他输入验证
"""

from __future__ import annotations

import re
from enum import Enum


class PasswordPolicyLevel(str, Enum):
    """密码策略级别。"""

    WEAK = "weak"
    MEDIUM = "medium"
    STRONG = "strong"


class PasswordValidator:
    """密码复杂度验证器"""

    MIN_LENGTH = 6
    MEDIUM_MIN_LENGTH = 8
    STRONG_MIN_LENGTH = 8
    MAX_BYTES = 72

    @classmethod
    def get_byte_length(cls, password: str) -> int:
        """返回密码的 UTF-8 字节长度。"""
        return len(password.encode("utf-8"))

    @classmethod
    def normalize_policy(cls, policy: PasswordPolicyLevel | str | None) -> PasswordPolicyLevel:
        """规范化密码策略级别，异常值回退为弱策略。"""
        if isinstance(policy, PasswordPolicyLevel):
            return policy
        if isinstance(policy, str):
            normalized = policy.strip().lower()
            try:
                return PasswordPolicyLevel(normalized)
            except ValueError:
                pass
        return PasswordPolicyLevel.WEAK

    @classmethod
    def validate_basic_input(cls, password: str) -> tuple[bool, str | None]:
        """验证密码输入的基础约束，不修改原始内容。"""
        if password == "":
            return False, "密码不能为空"

        if cls.get_byte_length(password) > cls.MAX_BYTES:
            return False, f"密码长度不能超过{cls.MAX_BYTES}字节"

        return True, None

    @classmethod
    def validate_login_input(cls, password: str) -> tuple[bool, str | None]:
        """验证登录时的密码输入，不修改原始内容。"""
        return cls.validate_basic_input(password)

    @classmethod
    def validate(
        cls,
        password: str,
        policy: PasswordPolicyLevel | str | None = None,
    ) -> tuple[bool, str | None]:
        """验证密码复杂度。"""
        if not password:
            return False, "密码不能为空"

        if cls.get_byte_length(password) > cls.MAX_BYTES:
            return False, f"密码长度不能超过{cls.MAX_BYTES}字节"

        policy_level = cls.normalize_policy(policy)

        # 根据策略级别确定最小长度，避免分两次报长度错误
        if policy_level in (PasswordPolicyLevel.MEDIUM, PasswordPolicyLevel.STRONG):
            min_len = cls.MEDIUM_MIN_LENGTH
        else:
            min_len = cls.MIN_LENGTH

        if len(password) < min_len:
            return False, f"密码长度至少为{min_len}个字符"

        if policy_level == PasswordPolicyLevel.MEDIUM:
            if not re.search(r"[A-Za-z]", password):
                return False, "密码必须包含至少一个字母"
            if not re.search(r"\d", password):
                return False, "密码必须包含至少一个数字"
        elif policy_level == PasswordPolicyLevel.STRONG:
            if not re.search(r"[A-Z]", password):
                return False, "密码必须包含至少一个大写字母"
            if not re.search(r"[a-z]", password):
                return False, "密码必须包含至少一个小写字母"
            if not re.search(r"\d", password):
                return False, "密码必须包含至少一个数字"
            if not re.search(r"[!@#$%^&*()_+\-=\[\]{};:'\",.<>?/\\|`~]", password):
                return False, "密码必须包含至少一个特殊字符"

        return True, None


class EmailValidator:
    """邮箱验证器"""

    EMAIL_REGEX = re.compile(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$")

    @classmethod
    def validate(cls, email: str) -> tuple[bool, str | None]:
        """
        验证邮箱格式

        Args:
            email: 待验证的邮箱

        Returns:
            (是否通过, 错误消息)
        """
        if not email:
            return False, "邮箱不能为空"

        if len(email) > 255:
            return False, "邮箱长度不能超过255个字符"

        if not cls.EMAIL_REGEX.match(email):
            return False, "邮箱格式不正确"

        return True, None


class UsernameValidator:
    """用户名验证器"""

    MIN_LENGTH = 3
    MAX_LENGTH = 30
    USERNAME_REGEX = re.compile(r"^[a-zA-Z0-9_.\-]+$")

    @classmethod
    def validate(cls, username: str) -> tuple[bool, str | None]:
        """
        验证用户名

        Args:
            username: 待验证的用户名

        Returns:
            (是否通过, 错误消息)
        """
        if not username:
            return False, "用户名不能为空"

        if len(username) < cls.MIN_LENGTH:
            return False, f"用户名长度至少为{cls.MIN_LENGTH}个字符"

        if len(username) > cls.MAX_LENGTH:
            return False, f"用户名长度不能超过{cls.MAX_LENGTH}个字符"

        if not cls.USERNAME_REGEX.match(username):
            return False, "用户名只能包含字母、数字、下划线、连字符和点号"

        # 检查保留用户名
        reserved_names = [
            "admin",
            "root",
            "system",
            "api",
            "test",
            "demo",
            "user",
            "guest",
            "bot",
            "webhook",
            "support",
        ]
        if username.lower() in reserved_names:
            return False, "该用户名为系统保留用户名"

        return True, None
