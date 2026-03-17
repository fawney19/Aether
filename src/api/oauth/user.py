"""OAuth 用户端点（需登录）。"""

from typing import Any, cast

from fastapi import APIRouter, Depends, HTTPException, Request, status
from sqlalchemy.orm import Session
from starlette.responses import RedirectResponse

from src.api.base.adapter import ApiMode
from src.api.base.authenticated_adapter import AuthenticatedApiAdapter
from src.api.base.context import ApiRequestContext
from src.api.base.pipeline import get_pipeline
from src.clients.redis_client import get_redis_client
from src.database import get_db
from src.models.database import User
from src.services.auth.oauth.service import OAuthService
from src.services.auth.oauth.state import consume_oauth_bind_token, create_oauth_bind_token
from src.services.auth.session_service import CLIENT_DEVICE_ID_HEADER, SessionService

router = APIRouter(prefix="/api/user/oauth", tags=["User - OAuth"])
pipeline = get_pipeline()


@router.get("/bindable-providers")
async def list_bindable_providers(
    request: Request, db: Session = Depends(get_db)
) -> dict[str, Any]:
    adapter = ListBindableProvidersAdapter()
    result = await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    return cast(dict[str, Any], result)


@router.get("/links")
async def list_my_oauth_links(request: Request, db: Session = Depends(get_db)) -> dict[str, Any]:
    adapter = ListMyOAuthLinksAdapter()
    result = await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    return cast(dict[str, Any], result)


@router.post("/{provider_type}/bind-token")
async def create_bind_token(
    provider_type: str, request: Request, db: Session = Depends(get_db)
) -> dict[str, Any]:
    """创建一次性 OAuth 绑定令牌，用于浏览器跳转场景的安全认证"""
    adapter = CreateBindTokenAdapter(provider_type=provider_type)
    result = await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    return cast(dict[str, Any], result)


@router.get("/{provider_type}/bind")
async def bind_oauth_provider(
    provider_type: str,
    request: Request,
    db: Session = Depends(get_db),
    bind_token: str | None = None,
) -> RedirectResponse:
    """发起 OAuth 绑定流程，支持通过 bind_token 参数进行安全认证"""
    adapter = BindOAuthProviderAdapter(provider_type=provider_type, bind_token=bind_token)
    result = await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    return cast(RedirectResponse, result)


@router.delete("/{provider_type}")
async def unbind_oauth_provider(
    provider_type: str, request: Request, db: Session = Depends(get_db)
) -> dict[str, Any]:
    adapter = UnbindOAuthProviderAdapter(provider_type=provider_type)
    result = await pipeline.run(adapter=adapter, http_request=request, db=db, mode=adapter.mode)
    return cast(dict[str, Any], result)


class ListBindableProvidersAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:  # type: ignore[override]
        assert context.user is not None
        providers = await OAuthService.list_bindable_providers(context.db, context.user)
        return {"providers": providers}


class ListMyOAuthLinksAdapter(AuthenticatedApiAdapter):
    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:  # type: ignore[override]
        assert context.user is not None
        links = await OAuthService.list_user_links(context.db, context.user)
        return {"links": links}


class CreateBindTokenAdapter(AuthenticatedApiAdapter):
    """创建一次性 OAuth 绑定令牌，用于浏览器跳转场景"""

    def __init__(self, provider_type: str):
        self.provider_type = provider_type

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:  # type: ignore[override]
        assert context.user is not None
        assert context.user.id is not None

        # 验证 provider 是否存在且可绑定
        bindable = await OAuthService.list_bindable_providers(context.db, context.user)
        if not any(p["provider_type"] == self.provider_type for p in bindable):
            raise HTTPException(status_code=400, detail="无法绑定该 Provider")

        redis = await get_redis_client(require_redis=True)
        if redis is None:
            raise HTTPException(status_code=503, detail="Redis 不可用")

        token = await create_oauth_bind_token(
            redis, user_id=context.user.id, provider_type=self.provider_type
        )
        return {"bind_token": token}


class BindOAuthProviderAdapter(AuthenticatedApiAdapter):
    """发起 OAuth 绑定流程，支持两种认证方式：
    1. Authorization header (标准方式)
    2. bind_token 参数 (浏览器跳转场景)
    """

    def __init__(self, provider_type: str, bind_token: str | None = None):
        self.provider_type = provider_type
        self.bind_token = bind_token
        self._user_from_bind_token: User | None = None

    @property
    def mode(self) -> ApiMode:  # type: ignore[override]
        # 如果有 bind_token，使用 PUBLIC mode 跳过 header 认证
        if self.bind_token:
            return ApiMode.PUBLIC
        return ApiMode.USER

    def authorize(self, context: ApiRequestContext) -> None:
        # 如果是 bind_token 模式，不在这里检查（会在 handle 中验证）
        if self.bind_token:
            return
        # 标准模式，检查用户
        if not context.user:
            raise HTTPException(status_code=401, detail="未登录")

    async def handle(self, context: ApiRequestContext) -> RedirectResponse:  # type: ignore[override]
        user: User | None = context.user

        # 如果使用 bind_token，验证并获取用户
        if self.bind_token:
            redis = await get_redis_client(require_redis=True)
            if redis is None:
                raise HTTPException(status_code=503, detail="Redis 不可用")

            token_data = await consume_oauth_bind_token(redis, self.bind_token)
            if not token_data:
                raise HTTPException(status_code=401, detail="无效或过期的绑定令牌")

            # 验证 provider_type 匹配
            if token_data.provider_type != self.provider_type:
                raise HTTPException(status_code=400, detail="绑定令牌与 Provider 不匹配")

            # 从数据库获取用户
            user = context.db.query(User).filter(User.id == token_data.user_id).first()
            if not user or not user.is_active or user.is_deleted:
                raise HTTPException(status_code=403, detail="用户不存在或已禁用")

        assert user is not None
        client_device_id: str | None = None
        if context.request.headers.get(CLIENT_DEVICE_ID_HEADER) or context.request.query_params.get(
            "client_device_id"
        ):
            client_device_id = SessionService.extract_client_device_id(context.request)
        url = await OAuthService.build_bind_authorize_url(
            context.db,
            user,
            self.provider_type,
            client_device_id=client_device_id,
        )
        return RedirectResponse(url=url, status_code=status.HTTP_302_FOUND)


class UnbindOAuthProviderAdapter(AuthenticatedApiAdapter):
    def __init__(self, provider_type: str):
        self.provider_type = provider_type

    async def handle(self, context: ApiRequestContext) -> dict[str, Any]:  # type: ignore[override]
        assert context.user is not None
        await OAuthService.unbind_provider(context.db, context.user, self.provider_type)
        return {"message": "解绑成功"}
