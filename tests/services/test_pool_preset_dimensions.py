"""Tests for pool preset dimension registry and built-in dimensions."""

from __future__ import annotations

from types import SimpleNamespace

import src.services.provider.pool.dimensions  # noqa: F401
from src.services.provider.pool.dimensions import get_preset_dimension, get_preset_names
from src.services.provider.pool.dimensions._helpers import extract_plan_type


def _key(metadata: dict, *, plan_type: str | None = None) -> SimpleNamespace:
    return SimpleNamespace(upstream_metadata=metadata, oauth_plan_type=plan_type)


def test_registry_discovers_builtin_dimensions() -> None:
    names = get_preset_names()
    assert {
        "free_team_first",
        "recent_refresh",
        "quota_balanced",
        "single_account",
        "priority_first",
        "health_first",
        "latency_first",
        "cost_first",
    }.issubset(names)


def test_universal_dimensions_are_applicable_to_any_provider() -> None:
    for name in ("quota_balanced", "single_account"):
        dim = get_preset_dimension(name)
        assert dim is not None
        assert dim.is_applicable("openai") is True
        assert dim.is_applicable("codex") is True
        assert dim.is_applicable("unknown_provider") is True


def test_provider_specific_dimensions_are_filtered() -> None:
    for name in ("free_team_first", "recent_refresh"):
        dim = get_preset_dimension(name)
        assert dim is not None
        assert dim.is_applicable("codex") is True
        assert dim.is_applicable("kiro") is True
        assert dim.is_applicable("openai") is False


def test_builtin_dimensions_compute_metric_in_range() -> None:
    all_key_ids = ["k1", "k2", "k3"]
    lru_scores = {"k1": 100.0, "k2": 800.0, "k3": 300.0}
    keys_by_id = {
        "k1": _key(
            {
                "codex": {
                    "plan_type": "plus",
                    "primary_reset_seconds": 900,
                    "primary_used_percent": 60,
                }
            }
        ),
        "k2": _key(
            {
                "codex": {
                    "plan_type": "free",
                    "primary_reset_seconds": 120,
                    "primary_used_percent": 20,
                }
            }
        ),
        "k3": _key(
            {
                "kiro": {
                    "next_reset_at": 4102444800,
                    "usage_percentage": 45,
                    "subscription_title": "Kiro Team",
                }
            },
            plan_type="team",
        ),
    }

    for name in get_preset_names():
        dim = get_preset_dimension(name)
        assert dim is not None
        mode = dim.default_mode
        metric = dim.compute_metric(
            key_id="k1",
            all_key_ids=all_key_ids,
            keys_by_id=keys_by_id,
            lru_scores=lru_scores,
            context={},
            mode=mode,
        )
        assert 0.0 <= metric <= 1.0


def test_extract_plan_type_prefers_quota_metadata_over_stale_persisted_value() -> None:
    key = _key(
        {
            "codex": {
                "plan_type": "plus",
                "primary_used_percent": 12,
            }
        },
        plan_type="free",
    )

    assert extract_plan_type(key) == "plus"
