from fastapi import APIRouter

from .routes import router as analytics_router

router = APIRouter()
router.include_router(analytics_router)

__all__ = ["router"]
