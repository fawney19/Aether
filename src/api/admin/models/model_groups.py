"""
ModelGroup Admin API

提供模型分组的 CRUD、模型成员管理和路由策略管理接口。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from fastapi import APIRouter, Depends, Request, Response
from pydantic import BaseModel, Field
from sqlalchemy.orm import Session

from src.api.base.admin_adapter import AdminApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.database import get_db
from src.models.pydantic_models import (
    ModelGroupCreate,
    ModelGroupDetailResponse,
    ModelGroupListResponse,
    ModelGroupModelRef,
    ModelGroupResponse,
    ModelGroupRouteRequest,
    ModelGroupRouteResponse,
    ModelGroupUpdate,
    ModelGroupUserGroupRef,
)
from src.services.model.group_service import ModelGroupRoutePayload, ModelGroupService

router = APIRouter(prefix="/groups", tags=["Admin - Model Groups"])
pipeline = get_pipeline()


class ReplaceModelGroupModelsRequest(BaseModel):
    """完整替换模型分组中的模型。"""

    model_ids: list[str] = Field(default_factory=list, description="GlobalModel ID 列表")


class ReplaceModelGroupRoutesRequest(BaseModel):
    """完整替换模型分组路由规则。"""

    routes: list[ModelGroupRouteRequest] = Field(default_factory=list)


def _serialize_model_group(group: Any, *, model_count: int = 0, user_group_count: int = 0) -> ModelGroupResponse:
    return ModelGroupResponse(
        id=str(group.id),
        name=str(group.name),
        display_name=str(group.display_name),
        description=group.description,
        default_user_billing_multiplier=float(group.default_user_billing_multiplier or 1.0),
        routing_mode=str(group.routing_mode),
        is_default=bool(group.is_default),
        is_active=bool(group.is_active),
        sort_order=int(group.sort_order or 0),
        model_count=model_count,
        user_group_count=user_group_count,
        created_at=group.created_at,
        updated_at=group.updated_at,
    )


def _serialize_model_group_detail(group: Any) -> ModelGroupDetailResponse:
    models = [
        ModelGroupModelRef(
            id=str(link.id),
            global_model_id=str(link.global_model_id),
            model_name=str(link.global_model.name),
            model_display_name=str(link.global_model.display_name),
            is_active=bool(link.global_model.is_active),
        )
        for link in list(getattr(group, "model_links", None) or [])
        if getattr(link, "global_model", None) is not None
    ]
    models.sort(key=lambda item: (item.model_display_name.lower(), item.model_name.lower()))

    routes = [
        ModelGroupRouteResponse(
            id=str(link.id),
            provider_id=str(link.provider_id),
            provider_name=getattr(link.provider, "name", None),
            provider_api_key_id=(
                str(link.provider_api_key_id) if getattr(link, "provider_api_key_id", None) else None
            ),
            provider_api_key_name=getattr(link.provider_api_key, "name", None),
            priority=int(link.priority or 0),
            user_billing_multiplier_override=(
                float(link.user_billing_multiplier_override)
                if link.user_billing_multiplier_override is not None
                else None
            ),
            is_active=bool(link.is_active),
            notes=link.notes,
        )
        for link in list(getattr(group, "route_links", None) or [])
    ]
    routes.sort(
        key=lambda item: (
            item.priority,
            item.provider_name or "",
            item.provider_api_key_name or "",
            item.provider_api_key_id or "",
        )
    )

    user_groups = [
        ModelGroupUserGroupRef(
            user_group_id=str(link.user_group_id),
            user_group_name=str(link.user_group.name) if getattr(link, "user_group", None) else "",
            priority=int(link.priority or 0),
            is_active=bool(link.is_active),
        )
        for link in list(getattr(group, "user_group_links", None) or [])
        if getattr(link, "user_group", None) is not None
    ]
    user_groups.sort(key=lambda item: (item.priority, item.user_group_name))

    return ModelGroupDetailResponse(
        **_serialize_model_group(
            group,
            model_count=len(models),
            user_group_count=len(user_groups),
        ).model_dump(),
        models=models,
        routes=routes,
        user_groups=user_groups,
    )


def _to_route_payloads(routes: list[ModelGroupRouteRequest]) -> list[ModelGroupRoutePayload]:
    return [
        ModelGroupRoutePayload(
            provider_id=item.provider_id,
            provider_api_key_id=item.provider_api_key_id,
            priority=item.priority,
            user_billing_multiplier_override=item.user_billing_multiplier_override,
            is_active=item.is_active,
            notes=item.notes,
        )
        for item in routes
    ]


@router.get("", response_model=ModelGroupListResponse)
async def list_model_groups(request: Request, db: Session = Depends(get_db)) -> ModelGroupListResponse:
    adapter = AdminListModelGroupsAdapter()
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.post("", response_model=ModelGroupDetailResponse, status_code=201)
async def create_model_group(
    request: Request,
    payload: ModelGroupCreate,
    db: Session = Depends(get_db),
) -> ModelGroupDetailResponse:
    adapter = AdminCreateModelGroupAdapter(payload=payload)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.get("/{group_id}", response_model=ModelGroupDetailResponse)
async def get_model_group(
    request: Request,
    group_id: str,
    db: Session = Depends(get_db),
) -> ModelGroupDetailResponse:
    adapter = AdminGetModelGroupAdapter(group_id=group_id)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.patch("/{group_id}", response_model=ModelGroupDetailResponse)
async def update_model_group(
    request: Request,
    group_id: str,
    payload: ModelGroupUpdate,
    db: Session = Depends(get_db),
) -> ModelGroupDetailResponse:
    adapter = AdminUpdateModelGroupAdapter(group_id=group_id, payload=payload)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.delete("/{group_id}", status_code=204, response_class=Response)
async def delete_model_group(
    request: Request,
    group_id: str,
    db: Session = Depends(get_db),
) -> Response:
    adapter = AdminDeleteModelGroupAdapter(group_id=group_id)
    await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    return Response(status_code=204)


@router.put("/{group_id}/models", response_model=ModelGroupDetailResponse)
async def replace_model_group_models(
    request: Request,
    group_id: str,
    payload: ReplaceModelGroupModelsRequest,
    db: Session = Depends(get_db),
) -> ModelGroupDetailResponse:
    adapter = AdminReplaceModelGroupModelsAdapter(group_id=group_id, payload=payload)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@router.put("/{group_id}/routes", response_model=ModelGroupDetailResponse)
async def replace_model_group_routes(
    request: Request,
    group_id: str,
    payload: ReplaceModelGroupRoutesRequest,
    db: Session = Depends(get_db),
) -> ModelGroupDetailResponse:
    adapter = AdminReplaceModelGroupRoutesAdapter(group_id=group_id, payload=payload)
    return await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)


@dataclass
class AdminListModelGroupsAdapter(AdminApiAdapter):
    async def handle(self, context: ApiRequestContext) -> ModelGroupListResponse:  # type: ignore[override]
        rows = ModelGroupService.list_groups(context.db)
        groups = [
            _serialize_model_group(group, model_count=int(model_count), user_group_count=int(user_group_count))
            for group, model_count, user_group_count in rows
        ]
        return ModelGroupListResponse(model_groups=groups, total=len(groups))


@dataclass
class AdminCreateModelGroupAdapter(AdminApiAdapter):
    payload: ModelGroupCreate

    async def handle(self, context: ApiRequestContext) -> ModelGroupDetailResponse:  # type: ignore[override]
        group = ModelGroupService.create_group(
            context.db,
            name=self.payload.name,
            display_name=self.payload.display_name,
            description=self.payload.description,
            default_user_billing_multiplier=self.payload.default_user_billing_multiplier,
            routing_mode=self.payload.routing_mode,
            is_active=self.payload.is_active,
            sort_order=self.payload.sort_order,
            commit=False,
        )
        ModelGroupService.replace_group_models(
            context.db,
            group.id,
            self.payload.model_ids,
            commit=False,
        )
        ModelGroupService.replace_group_routes(
            context.db,
            group.id,
            _to_route_payloads(self.payload.routes),
            commit=False,
        )
        context.db.commit()
        detail = ModelGroupService.get_group_detail(context.db, group.id)
        if detail is None:
            raise ValueError("模型分组创建后读取失败")
        return _serialize_model_group_detail(detail)


@dataclass
class AdminGetModelGroupAdapter(AdminApiAdapter):
    group_id: str

    async def handle(self, context: ApiRequestContext) -> ModelGroupDetailResponse:  # type: ignore[override]
        group = ModelGroupService.get_group_detail(context.db, self.group_id)
        if group is None:
            from fastapi import HTTPException

            raise HTTPException(status_code=404, detail="模型分组不存在")
        return _serialize_model_group_detail(group)


@dataclass
class AdminUpdateModelGroupAdapter(AdminApiAdapter):
    group_id: str
    payload: ModelGroupUpdate

    async def handle(self, context: ApiRequestContext) -> ModelGroupDetailResponse:  # type: ignore[override]
        data = self.payload.model_dump(exclude_unset=True)
        model_ids = data.pop("model_ids", None)
        routes = data.pop("routes", None)

        group = ModelGroupService.update_group(context.db, self.group_id, commit=False, **data)
        if group is None:
            from fastapi import HTTPException

            raise HTTPException(status_code=404, detail="模型分组不存在")

        if model_ids is not None:
            ModelGroupService.replace_group_models(
                context.db,
                self.group_id,
                model_ids,
                commit=False,
            )
        if routes is not None:
            ModelGroupService.replace_group_routes(
                context.db,
                self.group_id,
                _to_route_payloads([ModelGroupRouteRequest.model_validate(route) for route in routes]),
                commit=False,
            )
        context.db.commit()
        detail = ModelGroupService.get_group_detail(context.db, self.group_id)
        if detail is None:
            raise ValueError("模型分组更新后读取失败")
        return _serialize_model_group_detail(detail)


@dataclass
class AdminDeleteModelGroupAdapter(AdminApiAdapter):
    group_id: str

    async def handle(self, context: ApiRequestContext) -> Any:  # type: ignore[override]
        deleted = ModelGroupService.delete_group(context.db, self.group_id)
        if not deleted:
            from fastapi import HTTPException

            raise HTTPException(status_code=404, detail="模型分组不存在")
        return None


@dataclass
class AdminReplaceModelGroupModelsAdapter(AdminApiAdapter):
    group_id: str
    payload: ReplaceModelGroupModelsRequest

    async def handle(self, context: ApiRequestContext) -> ModelGroupDetailResponse:  # type: ignore[override]
        ModelGroupService.replace_group_models(
            context.db,
            self.group_id,
            self.payload.model_ids,
        )
        detail = ModelGroupService.get_group_detail(context.db, self.group_id)
        if detail is None:
            from fastapi import HTTPException

            raise HTTPException(status_code=404, detail="模型分组不存在")
        return _serialize_model_group_detail(detail)


@dataclass
class AdminReplaceModelGroupRoutesAdapter(AdminApiAdapter):
    group_id: str
    payload: ReplaceModelGroupRoutesRequest

    async def handle(self, context: ApiRequestContext) -> ModelGroupDetailResponse:  # type: ignore[override]
        ModelGroupService.replace_group_routes(
            context.db,
            self.group_id,
            _to_route_payloads(self.payload.routes),
        )
        detail = ModelGroupService.get_group_detail(context.db, self.group_id)
        if detail is None:
            from fastapi import HTTPException

            raise HTTPException(status_code=404, detail="模型分组不存在")
        return _serialize_model_group_detail(detail)
