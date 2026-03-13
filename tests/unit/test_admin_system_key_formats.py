import pytest

from src.api.admin.system import AdminExportConfigAdapter, AdminImportConfigAdapter
from src.core.exceptions import InvalidRequestException
from src.services.provider_ops.types import SENSITIVE_CREDENTIAL_FIELDS


def test_export_key_api_formats_falls_back_to_provider_endpoints_when_none() -> None:
    adapter = AdminExportConfigAdapter()

    result = adapter._resolve_export_key_api_formats(
        None,
        ["claude:chat", "openai:cli"],
    )

    assert result == ["claude:chat", "openai:cli"]


def test_export_key_api_formats_keeps_explicit_empty_list() -> None:
    adapter = AdminExportConfigAdapter()

    result = adapter._resolve_export_key_api_formats(
        [],
        ["openai:chat"],
    )

    assert result == []


def test_export_key_api_formats_normalizes_and_deduplicates() -> None:
    adapter = AdminExportConfigAdapter()

    result = adapter._resolve_export_key_api_formats(
        [" OPENAI:CHAT ", "openai:chat", "openai:cli", "bad-format"],
        ["claude:chat"],
    )

    assert result == ["openai:chat", "openai:cli"]


def test_import_key_api_formats_uses_supported_endpoints_alias() -> None:
    result = AdminImportConfigAdapter._extract_import_key_api_formats(
        {"supported_endpoints": ["openai:chat"]},
        {"openai:chat", "openai:cli"},
    )

    assert result == ["openai:chat"]


def test_import_key_api_formats_falls_back_to_provider_endpoints_when_none() -> None:
    result = AdminImportConfigAdapter._extract_import_key_api_formats(
        {"api_formats": None},
        {"openai:chat", "claude:cli"},
    )

    assert result == ["claude:cli", "openai:chat"]


def test_import_key_api_formats_keeps_explicit_empty_list() -> None:
    result = AdminImportConfigAdapter._extract_import_key_api_formats(
        {"api_formats": []},
        {"openai:chat"},
    )

    assert result == []


class _FakeCrypto:
    def encrypt(self, value: str) -> str:
        return f"enc:{value}"

    def decrypt(self, value: str) -> str:
        return value.removeprefix("enc:")


def test_provider_ops_sensitive_fields_include_refresh_token() -> None:
    assert "refresh_token" in SENSITIVE_CREDENTIAL_FIELDS


def test_export_provider_config_decrypts_refresh_token() -> None:
    adapter = AdminExportConfigAdapter()

    config = {
        "provider_ops": {
            "connector": {
                "credentials": {
                    "refresh_token": "enc:rt-1",
                    "api_key": "enc:key-1",
                }
            }
        }
    }

    result = adapter._decrypt_provider_config(config, _FakeCrypto())

    assert result["provider_ops"]["connector"]["credentials"]["refresh_token"] == "rt-1"
    assert result["provider_ops"]["connector"]["credentials"]["api_key"] == "key-1"
    assert config["provider_ops"]["connector"]["credentials"]["refresh_token"] == "enc:rt-1"


def test_import_provider_config_encrypts_refresh_token() -> None:
    adapter = AdminImportConfigAdapter()

    config = {
        "provider_ops": {
            "connector": {
                "credentials": {
                    "refresh_token": "rt-1",
                    "api_key": "key-1",
                }
            }
        }
    }

    result = adapter._encrypt_provider_config(config, _FakeCrypto())

    assert result["provider_ops"]["connector"]["credentials"]["refresh_token"] == "enc:rt-1"
    assert result["provider_ops"]["connector"]["credentials"]["api_key"] == "enc:key-1"
    assert config["provider_ops"]["connector"]["credentials"]["refresh_token"] == "rt-1"


def test_import_endpoint_payload_rejects_dict_base_url() -> None:
    with pytest.raises(InvalidRequestException, match="导入 Endpoint 失败"):
        AdminImportConfigAdapter._normalize_import_endpoint_payload(
            "provider-1",
            {
                "api_format": "claude:chat",
                "base_url": {"url": "https://api.anthropic.com"},
            },
        )


def test_import_endpoint_payload_normalizes_base_url() -> None:
    result = AdminImportConfigAdapter._normalize_import_endpoint_payload(
        "provider-1",
        {
            "api_format": "claude:chat",
            "base_url": "https://api.anthropic.com/",
        },
    )

    assert result["base_url"] == "https://api.anthropic.com"
    assert result["api_format"] == "claude:chat"
