"""Per-key request fingerprint generation and lazy persistence helpers."""

from __future__ import annotations

import asyncio
import hashlib
import random
import secrets
import threading
import uuid
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from typing import Any

from src.core.logger import logger

# curl_cffi impersonate profile pool (Chrome family)
CHROME_IMPERSONATE_PROFILES: tuple[str, ...] = (
    "chrome110",
    "chrome116",
    "chrome119",
    "chrome120",
    "chrome123",
    "chrome124",
    "chrome131",
    "chrome133",
)

KNOWN_IMPERSONATE_PROFILES: frozenset[str] = frozenset(CHROME_IMPERSONATE_PROFILES)

_CHROME_VERSION_BY_PROFILE: dict[str, str] = {
    "chrome110": "110.0.5481.177",
    "chrome116": "116.0.5845.188",
    "chrome119": "119.0.6045.214",
    "chrome120": "120.0.6099.216",
    "chrome123": "123.0.6312.122",
    "chrome124": "124.0.6367.243",
    "chrome131": "131.0.6778.265",
    "chrome133": "133.0.6943.142",
}

_PLATFORM_VARIANTS: tuple[tuple[str, str, str, str], ...] = (
    ("Linux", "x64", "X11; Linux x86_64", "Linux x86_64"),
    ("Linux", "arm64", "X11; Linux arm64", "Linux arm64"),
    ("Windows", "x64", "Windows NT 10.0; Win64; x64", "Windows x64"),
    ("MacOS", "x64", "Macintosh; Intel Mac OS X 10_15_7", "Darwin x64"),
    ("MacOS", "arm64", "Macintosh; ARM Mac OS X 14_0_0", "Darwin arm64"),
)

_STAINLESS_PACKAGE_VERSIONS: tuple[str, ...] = ("0.68.0", "0.69.0", "0.70.0", "0.71.0")
_NODE_VERSIONS: tuple[str, ...] = ("v20.18.1", "v22.12.0", "v22.14.0", "v24.13.0")
_ELECTRON_VERSIONS: tuple[str, ...] = ("35.5.1", "36.7.1", "37.3.0", "38.7.0", "39.2.3")
_STAINLESS_TIMEOUTS: tuple[str, ...] = ("600", "900")

_PENDING_LAZY_PERSIST: set[str] = set()
_PENDING_LAZY_PERSIST_LOCK = threading.Lock()
_PENDING_LAZY_PERSIST_MAX = 2000


@dataclass(frozen=True, slots=True)
class FingerprintProfile:
    # TLS layer
    impersonate: str
    # Claude Code / Claude Chat feature dimensions
    stainless_package_version: str
    stainless_os: str
    stainless_arch: str
    stainless_runtime_version: str
    stainless_timeout: str
    # Generic dimensions
    user_agent: str
    node_version: str
    chrome_version: str
    electron_version: str
    vscode_session_id: str
    platform_info: str


def _build_rng(seed: str | None) -> random.Random:
    if seed is None:
        return random.Random(secrets.randbits(64))
    digest = hashlib.sha256(seed.encode("utf-8")).digest()
    return random.Random(int.from_bytes(digest[:8], "big", signed=False))


def _normalize_impersonate(raw: Any, fallback: str) -> str:
    value = str(raw or "").strip().lower()
    return value if value in KNOWN_IMPERSONATE_PROFILES else fallback


def _build_user_agent(
    platform_token: str,
    chrome_version: str,
    electron_version: str,
) -> str:
    return (
        f"Mozilla/5.0 ({platform_token}) AppleWebKit/537.36 (KHTML, like Gecko) "
        f"Chrome/{chrome_version} Electron/{electron_version} Safari/537.36"
    )


def resolve_platform_token(
    fp: dict[str, str] | None = None,
    *,
    platform_info: str | None = None,
    stainless_os: str | None = None,
    stainless_arch: str | None = None,
) -> str:
    """Resolve a platform token string from fingerprint dict or explicit kwargs."""
    if fp is not None:
        os_name = str(fp.get("stainless_os") or "").lower()
        arch = str(fp.get("stainless_arch") or "").lower()
        info = str(fp.get("platform_info") or "").lower()
    else:
        os_name = str(stainless_os or "").lower()
        arch = str(stainless_arch or "").lower()
        info = str(platform_info or "").lower()

    if os_name.startswith("win") or "windows" in info:
        return "Windows NT 10.0; Win64; x64"
    if os_name in {"darwin", "mac", "macos"} or "darwin" in info:
        return (
            "Macintosh; ARM Mac OS X 14_0_0"
            if arch in {"arm64", "aarch64"}
            else "Macintosh; Intel Mac OS X 10_15_7"
        )
    return "X11; Linux arm64" if arch in {"arm64", "aarch64"} else "X11; Linux x86_64"


def _normalize_text(value: Any, fallback: str) -> str:
    text = str(value or "").strip()
    return text or fallback


def _sanitize_fingerprint_dict(raw: dict[str, Any], key_id: str) -> dict[str, str]:
    generated = generate_fingerprint(seed=key_id or None)
    fp: dict[str, str] = {k: str(v) for k, v in generated.items()}
    for key, value in raw.items():
        if isinstance(value, str) and value.strip():
            fp[key] = value.strip()

    fp["impersonate"] = _normalize_impersonate(fp.get("impersonate"), generated["impersonate"])
    fp["chrome_version"] = _normalize_text(
        fp.get("chrome_version"),
        _CHROME_VERSION_BY_PROFILE.get(fp["impersonate"], generated["chrome_version"]),
    )
    fp["node_version"] = _normalize_text(fp.get("node_version"), generated["node_version"])
    fp["electron_version"] = _normalize_text(
        fp.get("electron_version"),
        generated["electron_version"],
    )
    fp["stainless_package_version"] = _normalize_text(
        fp.get("stainless_package_version"),
        generated["stainless_package_version"],
    )
    fp["stainless_os"] = _normalize_text(fp.get("stainless_os"), generated["stainless_os"])
    fp["stainless_arch"] = _normalize_text(fp.get("stainless_arch"), generated["stainless_arch"])
    fp["stainless_runtime_version"] = _normalize_text(
        fp.get("stainless_runtime_version"),
        generated["stainless_runtime_version"],
    )
    fp["stainless_timeout"] = _normalize_text(
        fp.get("stainless_timeout"),
        generated["stainless_timeout"],
    )
    fp["platform_info"] = _normalize_text(fp.get("platform_info"), generated["platform_info"])
    fp["vscode_session_id"] = _normalize_text(
        fp.get("vscode_session_id"),
        generated["vscode_session_id"],
    )

    if not str(fp.get("user_agent") or "").strip():
        fp["user_agent"] = _build_user_agent(
            resolve_platform_token(fp),
            fp["chrome_version"],
            fp["electron_version"],
        )

    return fp


def generate_fingerprint(seed: str | None = None) -> dict[str, str]:
    """Generate a serializable fingerprint profile dict."""
    rng = _build_rng(seed)

    impersonate = rng.choice(CHROME_IMPERSONATE_PROFILES)
    chrome_version = _CHROME_VERSION_BY_PROFILE.get(impersonate, "120.0.6099.216")
    node_version = rng.choice(_NODE_VERSIONS)
    electron_version = rng.choice(_ELECTRON_VERSIONS)
    stainless_os, stainless_arch, platform_token, platform_info = rng.choice(_PLATFORM_VARIANTS)

    if seed is None:
        vscode_session_id = uuid.uuid4().hex
    else:
        vscode_session_id = uuid.uuid5(uuid.NAMESPACE_URL, f"aether:fingerprint:{seed}").hex

    return {
        "impersonate": impersonate,
        "stainless_package_version": rng.choice(_STAINLESS_PACKAGE_VERSIONS),
        "stainless_os": stainless_os,
        "stainless_arch": stainless_arch,
        "stainless_runtime_version": node_version,
        "stainless_timeout": rng.choice(_STAINLESS_TIMEOUTS),
        "node_version": node_version,
        "chrome_version": chrome_version,
        "electron_version": electron_version,
        "vscode_session_id": vscode_session_id,
        "platform_info": platform_info,
        "user_agent": _build_user_agent(platform_token, chrome_version, electron_version),
    }


def _dict_to_profile(d: dict[str, str]) -> FingerprintProfile:
    return FingerprintProfile(
        impersonate=d["impersonate"],
        stainless_package_version=d["stainless_package_version"],
        stainless_os=d["stainless_os"],
        stainless_arch=d["stainless_arch"],
        stainless_runtime_version=d["stainless_runtime_version"],
        stainless_timeout=d["stainless_timeout"],
        user_agent=d["user_agent"],
        node_version=d["node_version"],
        chrome_version=d["chrome_version"],
        electron_version=d["electron_version"],
        vscode_session_id=d["vscode_session_id"],
        platform_info=d["platform_info"],
    )


def load_fingerprint(raw: dict[str, Any] | None, key_id: str) -> FingerprintProfile:
    """Load fingerprint from DB JSON with deterministic fallback by key_id."""
    if raw and isinstance(raw, dict):
        normalized = _sanitize_fingerprint_dict(raw, key_id)
    else:
        normalized = generate_fingerprint(seed=key_id or None)
    return _dict_to_profile(normalized)


def serialize_fingerprint(fp: FingerprintProfile) -> dict[str, str]:
    return {k: str(v) for k, v in asdict(fp).items()}


def normalize_fingerprint(raw: dict[str, Any], key_id: str) -> dict[str, str]:
    return serialize_fingerprint(load_fingerprint(raw, key_id))


def _persist_fingerprint_if_missing_sync(key_id: str, fp: dict[str, str]) -> None:
    from src.database import create_session
    from src.models.database import ProviderAPIKey

    db = create_session()
    try:
        updated = (
            db.query(ProviderAPIKey)
            .filter(
                ProviderAPIKey.id == key_id,
                ProviderAPIKey.fingerprint.is_(None),
            )
            .update(
                {
                    ProviderAPIKey.fingerprint: fp,
                    ProviderAPIKey.updated_at: datetime.now(timezone.utc),
                },
                synchronize_session=False,
            )
        )
        if updated > 0:
            db.commit()
    except Exception as exc:
        db.rollback()
        logger.debug("lazy fingerprint persist failed for key {}: {}", key_id[:8], str(exc))
    finally:
        db.close()


def _mark_pending_persist(key_id: str) -> bool:
    with _PENDING_LAZY_PERSIST_LOCK:
        if key_id in _PENDING_LAZY_PERSIST:
            return False
        if len(_PENDING_LAZY_PERSIST) >= _PENDING_LAZY_PERSIST_MAX:
            return False
        _PENDING_LAZY_PERSIST.add(key_id)
        return True


def _clear_pending_persist(key_id: str) -> None:
    with _PENDING_LAZY_PERSIST_LOCK:
        _PENDING_LAZY_PERSIST.discard(key_id)


def schedule_lazy_fingerprint_persist(key_id: str, fp: dict[str, str]) -> None:
    """Persist generated fingerprint in background when key.fingerprint is missing."""
    key_id = str(key_id or "").strip()
    if not key_id:
        return
    if not _mark_pending_persist(key_id):
        return

    payload = dict(fp)

    async def _persist_async() -> None:
        try:
            await asyncio.to_thread(_persist_fingerprint_if_missing_sync, key_id, payload)
        finally:
            _clear_pending_persist(key_id)

    try:
        loop = asyncio.get_running_loop()
    except RuntimeError:
        try:
            _persist_fingerprint_if_missing_sync(key_id, payload)
        finally:
            _clear_pending_persist(key_id)
        return

    task = loop.create_task(_persist_async())

    def _on_done(done_task: asyncio.Task[None]) -> None:
        try:
            done_task.result()
        except Exception as exc:
            logger.debug("lazy fingerprint background task failed: {}", str(exc))

    task.add_done_callback(_on_done)


def ensure_key_fingerprint(
    key: Any,
    *,
    persist_if_missing: bool = False,
) -> FingerprintProfile:
    """Get a key fingerprint, generating deterministic fallback if missing."""
    key_id = str(getattr(key, "id", "") or "").strip()
    raw = getattr(key, "fingerprint", None)

    if isinstance(raw, dict) and raw:
        return load_fingerprint(raw, key_id)

    generated = generate_fingerprint(seed=key_id or None)

    if persist_if_missing and key_id:
        schedule_lazy_fingerprint_persist(key_id, generated)

    return _dict_to_profile(generated)


__all__ = [
    "CHROME_IMPERSONATE_PROFILES",
    "FingerprintProfile",
    "KNOWN_IMPERSONATE_PROFILES",
    "ensure_key_fingerprint",
    "generate_fingerprint",
    "load_fingerprint",
    "normalize_fingerprint",
    "resolve_platform_token",
    "schedule_lazy_fingerprint_persist",
    "serialize_fingerprint",
]
