from __future__ import annotations

from types import SimpleNamespace

from src.services.provider.fingerprint import (
    CHROME_IMPERSONATE_PROFILES,
    ensure_key_fingerprint,
    generate_fingerprint,
    load_fingerprint,
)


def test_generate_fingerprint_is_deterministic_with_seed() -> None:
    first = generate_fingerprint(seed="key-123")
    second = generate_fingerprint(seed="key-123")
    assert first == second


def test_load_fingerprint_falls_back_for_invalid_impersonate() -> None:
    profile = load_fingerprint({"impersonate": "invalid-profile"}, "key-abc")
    assert profile.impersonate in CHROME_IMPERSONATE_PROFILES


def test_ensure_key_fingerprint_returns_profile_when_missing() -> None:
    key = SimpleNamespace(id="key-xyz", fingerprint=None)
    profile = ensure_key_fingerprint(key, persist_if_missing=False)

    assert profile.impersonate in CHROME_IMPERSONATE_PROFILES
    # Deterministic: same key_id produces same profile
    profile2 = ensure_key_fingerprint(key, persist_if_missing=False)
    assert profile.impersonate == profile2.impersonate


def test_ensure_key_fingerprint_uses_existing_fingerprint() -> None:
    existing = generate_fingerprint(seed="key-existing")
    key = SimpleNamespace(id="key-existing", fingerprint=existing)
    profile = ensure_key_fingerprint(key, persist_if_missing=False)

    assert profile.impersonate == existing["impersonate"]
