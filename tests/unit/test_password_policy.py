import pytest
from pydantic import ValidationError

from src.core.validators import PasswordPolicyLevel, PasswordValidator
from src.models.api import LoginRequest


class TestPasswordPolicy:
    def test_default_policy_is_weak(self) -> None:
        valid, error = PasswordValidator.validate("123456")

        assert valid is True
        assert error is None

    @pytest.mark.parametrize(
        ("password", "expected_error"),
        [
            ("1234567", "密码长度至少为8个字符"),
            ("12345678", "密码必须包含至少一个字母"),
            ("abcdefgh", "密码必须包含至少一个数字"),
        ],
    )
    def test_medium_policy_rejects_weak_patterns(self, password: str, expected_error: str) -> None:
        valid, error = PasswordValidator.validate(password, policy=PasswordPolicyLevel.MEDIUM)

        assert valid is False
        assert error == expected_error

    def test_medium_policy_accepts_letters_and_digits(self) -> None:
        valid, error = PasswordValidator.validate("abc12345", policy=PasswordPolicyLevel.MEDIUM)

        assert valid is True
        assert error is None

    @pytest.mark.parametrize(
        ("password", "expected_error"),
        [
            ("abc12345", "密码必须包含至少一个大写字母"),
            ("ABC12345", "密码必须包含至少一个小写字母"),
            ("Abcdefgh", "密码必须包含至少一个数字"),
            ("Abcd1234", "密码必须包含至少一个特殊字符"),
        ],
    )
    def test_strong_policy_requires_upper_lower_and_digit(
        self, password: str, expected_error: str
    ) -> None:
        valid, error = PasswordValidator.validate(password, policy=PasswordPolicyLevel.STRONG)

        assert valid is False
        assert error == expected_error

    def test_strong_policy_accepts_mixed_password(self) -> None:
        valid, error = PasswordValidator.validate("Abcd1234!", policy="strong")

        assert valid is True
        assert error is None

    def test_rejects_password_longer_than_72_bytes(self) -> None:
        valid, error = PasswordValidator.validate("a" * 80)

        assert valid is False
        assert error == "密码长度不能超过72字节"

    def test_rejects_multibyte_password_longer_than_72_bytes(self) -> None:
        valid, error = PasswordValidator.validate("中" * 25)

        assert valid is False
        assert error == "密码长度不能超过72字节"

    def test_login_request_preserves_leading_and_trailing_spaces(self) -> None:
        request = LoginRequest.model_validate(
            {"email": "tester", "password": "  pass word  ", "auth_type": "local"}
        )

        assert request.password == "  pass word  "

    def test_login_request_rejects_password_longer_than_72_bytes(self) -> None:
        with pytest.raises(ValidationError, match="密码长度不能超过72字节"):
            LoginRequest.model_validate(
                {"email": "tester", "password": "a" * 80, "auth_type": "local"}
            )
