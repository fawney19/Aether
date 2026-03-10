from __future__ import annotations

from types import SimpleNamespace
from unittest.mock import MagicMock

from src.models.database import Model, Provider, ProviderAPIKey
from src.services.model.global_model import GlobalModelService


def test_auto_disassociate_short_circuits_when_unlimited_key_exists() -> None:
    db = MagicMock()
    provider_query = MagicMock()
    provider_query.filter.return_value.first.return_value = SimpleNamespace(name="Provider A")

    unlimited_query = MagicMock()
    unlimited_query.filter.return_value.limit.return_value.first.return_value = object()

    def _query(*entities: object) -> MagicMock:
        entity = entities[0]
        if entity is Provider:
            return provider_query
        if entity is ProviderAPIKey.id:
            return unlimited_query
        if entity is Model:
            raise AssertionError("model query should not run when unlimited key exists")
        raise AssertionError(f"unexpected query: {entities}")

    db.query.side_effect = _query

    result = GlobalModelService.auto_disassociate_provider_by_key_whitelist(db, "provider-1")

    assert result == {"success": [], "errors": []}
    db.delete.assert_not_called()
    db.commit.assert_not_called()


def test_auto_disassociate_deletes_unmatched_auto_associated_models(
    monkeypatch,
) -> None:
    db = MagicMock()
    provider_query = MagicMock()
    provider_query.filter.return_value.first.return_value = SimpleNamespace(name="Provider B")

    unlimited_query = MagicMock()
    unlimited_query.filter.return_value.limit.return_value.first.return_value = None

    allowed_models_query = MagicMock()
    # db.query(ProviderAPIKey.allowed_models).all() returns list of tuples
    allowed_models_query.filter.return_value.all.return_value = [
        (["gpt-4o"],),
        ([],),
    ]

    model = SimpleNamespace(
        id="model-1",
        global_model=SimpleNamespace(
            id="gm-1",
            name="claude-sonnet",
            config={"model_mappings": ["claude-*"]},
        ),
    )
    models_query = MagicMock()
    models_query.options.return_value.filter.return_value.all.return_value = [model]

    def _query(*entities: object) -> MagicMock:
        entity = entities[0]
        if entity is Provider:
            return provider_query
        if entity is ProviderAPIKey.id:
            return unlimited_query
        if entity is ProviderAPIKey.allowed_models:
            return allowed_models_query
        if entity is Model:
            return models_query
        raise AssertionError(f"unexpected query: {entities}")

    db.query.side_effect = _query
    monkeypatch.setattr(
        "src.core.model_permissions.match_model_with_pattern",
        lambda pattern, allowed_model: pattern == allowed_model,
    )

    result = GlobalModelService.auto_disassociate_provider_by_key_whitelist(db, "provider-2")

    assert result["errors"] == []
    assert result["success"] == [
        {
            "model_id": "model-1",
            "global_model_id": "gm-1",
            "global_model_name": "claude-sonnet",
        }
    ]
    db.delete.assert_called_once_with(model)
    db.commit.assert_called_once()
