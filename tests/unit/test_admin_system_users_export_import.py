from src.api.admin.system import AdminExportUsersAdapter, AdminImportUsersAdapter
from src.core.crypto import crypto_service
from src.models.database import ApiKey


def test_export_user_api_key_prefers_plaintext_key() -> None:
    plaintext_key = "ak-user-plain-1"
    key = ApiKey(
        id="key-1",
        user_id="user-1",
        key_hash=ApiKey.hash_key(plaintext_key),
        key_encrypted=crypto_service.encrypt(plaintext_key),
        name="Demo Key",
        is_standalone=False,
        is_active=True,
    )

    data = AdminExportUsersAdapter._serialize_api_key(key, include_is_standalone=True)

    assert data["key"] == plaintext_key
    assert "key_encrypted" not in data
    assert data["key_hash"] == ApiKey.hash_key(plaintext_key)
    assert data["is_standalone"] is False


def test_import_user_api_key_material_reencrypts_plaintext_key() -> None:
    plaintext_key = "ak-user-plain-2"

    key_hash, key_encrypted = AdminImportUsersAdapter._resolve_api_key_material(
        {
            "key": plaintext_key,
            "key_hash": "stale-hash",
            "key_encrypted": "stale-ciphertext",
        }
    )

    assert key_hash == ApiKey.hash_key(plaintext_key)
    assert key_encrypted is not None
    assert crypto_service.decrypt(key_encrypted) == plaintext_key


def test_import_user_api_key_material_keeps_legacy_encrypted_payload() -> None:
    legacy_plaintext = "ak-user-legacy-1"
    legacy_encrypted = crypto_service.encrypt(legacy_plaintext)
    legacy_hash = ApiKey.hash_key(legacy_plaintext)

    key_hash, key_encrypted = AdminImportUsersAdapter._resolve_api_key_material(
        {
            "key_hash": legacy_hash,
            "key_encrypted": legacy_encrypted,
        }
    )

    assert key_hash == legacy_hash
    assert key_encrypted == legacy_encrypted
