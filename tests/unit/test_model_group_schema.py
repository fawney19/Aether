from __future__ import annotations

from src.models.database import ModelGroupModel


def test_model_group_model_declares_updated_at_column() -> None:
    column = ModelGroupModel.__table__.columns["updated_at"]

    assert hasattr(ModelGroupModel, "updated_at")
    assert column.nullable is False
    assert column.default is not None
    assert column.onupdate is not None
