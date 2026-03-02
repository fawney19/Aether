"""
输入验证器
包含密码复杂度验证和其他输入验证
"""

from __future__ import annotations

import re
from pathlib import Path

from src.core.logger import logger


class PasswordValidator:
    """密码复杂度验证器"""

    MIN_LENGTH = 8
    MAX_LENGTH = 128
    _UPPERCASE_REGEX = re.compile(r"[A-Z]")
    _LOWERCASE_REGEX = re.compile(r"[a-z]")
    _DIGIT_REGEX = re.compile(r"\d")
    _COMMON_PASSWORDS_PATH = (
        Path(__file__).resolve().parent.parent / "data" / "common_passwords.txt"
    )
    _COMMON_PASSWORDS: set[str] | None = None

    @classmethod
    def _load_common_passwords(cls) -> set[str]:
        """惰性加载常见弱密码黑名单。"""
        if cls._COMMON_PASSWORDS is not None:
            return cls._COMMON_PASSWORDS

        common_passwords: set[str] = set()
        try:
            with cls._COMMON_PASSWORDS_PATH.open("r", encoding="utf-8") as file:
                common_passwords = {line.strip().lower() for line in file if line.strip()}
            logger.info(f"已加载常见密码黑名单: {len(common_passwords)} 条")
        except FileNotFoundError:
            logger.warning(
                f"常见密码黑名单文件不存在: {cls._COMMON_PASSWORDS_PATH}，将仅执行长度和复杂度校验"
            )
        except Exception as exc:
            logger.warning(f"加载常见密码黑名单失败: {exc}，将仅执行长度和复杂度校验")

        cls._COMMON_PASSWORDS = common_passwords
        return common_passwords

    @classmethod
    def validate(cls, password: str) -> tuple[bool, str | None]:
        """
        验证密码复杂度

        要求：
        - 长度至少8个字符
        - 必须包含大写字母、小写字母、数字
        - 不允许使用常见弱密码黑名单中的密码（1K）

        Args:
            password: 待验证的密码

        Returns:
            (是否通过, 错误消息)
        """
        if not password:
            return False, "密码不能为空"

        if len(password) < cls.MIN_LENGTH:
            return False, f"密码长度至少为{cls.MIN_LENGTH}个字符"

        if len(password) > cls.MAX_LENGTH:
            return False, f"密码长度不能超过{cls.MAX_LENGTH}个字符"

        if not cls._UPPERCASE_REGEX.search(password):
            return False, "密码必须包含至少一个大写字母"

        if not cls._LOWERCASE_REGEX.search(password):
            return False, "密码必须包含至少一个小写字母"

        if not cls._DIGIT_REGEX.search(password):
            return False, "密码必须包含至少一个数字"

        common_passwords = cls._load_common_passwords()
        if password.lower() in common_passwords:
            return False, "密码过于简单，请使用更复杂的密码"

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
