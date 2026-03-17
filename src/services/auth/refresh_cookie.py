from __future__ import annotations

from starlette.responses import Response

from src.config import config
from src.core.exceptions import ErrorResponse
from src.services.auth.service import REFRESH_TOKEN_EXPIRATION_DAYS


def set_refresh_token_cookie(response: Response, refresh_token: str) -> None:
    max_age_seconds = REFRESH_TOKEN_EXPIRATION_DAYS * 24 * 60 * 60
    response.set_cookie(
        key=config.auth_refresh_cookie_name,
        value=refresh_token,
        httponly=True,
        secure=config.auth_refresh_cookie_secure,
        samesite=config.auth_refresh_cookie_samesite,
        path="/api/auth",
        max_age=max_age_seconds,
    )


def clear_refresh_token_cookie(response: Response) -> None:
    response.delete_cookie(
        key=config.auth_refresh_cookie_name,
        path="/api/auth",
        secure=config.auth_refresh_cookie_secure,
        samesite=config.auth_refresh_cookie_samesite,
    )


def error_response_with_cleared_cookie(exc: Exception) -> Response:
    response = ErrorResponse.from_exception(exc)
    clear_refresh_token_cookie(response)
    return response
