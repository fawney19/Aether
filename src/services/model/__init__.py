"""
模型服务模块

包含模型管理、成本计算等功能。
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from src.services.model.availability import ModelAvailabilityQuery
    from src.services.model.cost import ModelCostService
    from src.services.model.fetch_scheduler import ModelFetchScheduler, get_model_fetch_scheduler
    from src.services.model.global_model import GlobalModelService
    from src.services.model.service import ModelService

__all__ = [
    "ModelService",
    "GlobalModelService",
    "ModelCostService",
    "ModelAvailabilityQuery",
    "ModelFetchScheduler",
    "get_model_fetch_scheduler",
]


def __getattr__(name: str) -> Any:
    """Lazy attribute access to avoid import-time side effects.

    Importing `src.services.model` should not eagerly import the whole model
    service stack (scheduler/services), which can create circular imports during
    test collection.
    """

    if name == "ModelAvailabilityQuery":
        from src.services.model.availability import ModelAvailabilityQuery as _ModelAvailabilityQuery

        return _ModelAvailabilityQuery

    if name == "ModelCostService":
        from src.services.model.cost import ModelCostService as _ModelCostService

        return _ModelCostService

    if name == "ModelFetchScheduler":
        from src.services.model.fetch_scheduler import ModelFetchScheduler as _ModelFetchScheduler

        return _ModelFetchScheduler

    if name == "get_model_fetch_scheduler":
        from src.services.model.fetch_scheduler import (
            get_model_fetch_scheduler as _get_model_fetch_scheduler,
        )

        return _get_model_fetch_scheduler

    if name == "GlobalModelService":
        from src.services.model.global_model import GlobalModelService as _GlobalModelService

        return _GlobalModelService

    if name == "ModelService":
        from src.services.model.service import ModelService as _ModelService

        return _ModelService

    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
