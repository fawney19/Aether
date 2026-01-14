"""Provider admin routes export."""

from fastapi import APIRouter

from .auto_sync import router as auto_sync_router
from .models import router as models_router
from .routes import router as routes_router
from .summary import router as summary_router

router = APIRouter(prefix="/api/admin/providers", tags=["Admin - Providers"])

# Provider CRUD
router.include_router(routes_router)

# Provider summary & health monitor
router.include_router(summary_router)

# Provider models management
router.include_router(models_router)

# Provider models auto sync
router.include_router(auto_sync_router)

__all__ = ["router"]
