"""vertex_ai_provider_type

Migrate legacy Vertex auth_type/provider_type into the new model:
- provider_type=vertex_ai
- auth_type=service_account (legacy vertex_ai renamed)
- fixed Vertex endpoints: gemini:chat + claude:chat

Revision ID: 2a624af8dd3a
Revises: 48afe197cc15
Create Date: 2026-02-28 15:00:00.000000+00:00
"""

from __future__ import annotations

import uuid

import sqlalchemy as sa

from alembic import op

# revision identifiers, used by Alembic.
revision = "2a624af8dd3a"
down_revision = "48afe197cc15"
branch_labels = None
depends_on = None

_VERTEX_BASE_URL = "https://aiplatform.googleapis.com"
_VERTEX_ENDPOINTS: tuple[tuple[str, str, str], ...] = (
    ("gemini:chat", "gemini", "chat"),
    ("claude:chat", "claude", "chat"),
)
_VERTEX_KEY_FORMATS_SA = '["gemini:chat","claude:chat"]'
_VERTEX_KEY_FORMATS_API_KEY = '["gemini:chat"]'


def _select_vertex_provider_ids(conn: sa.Connection) -> list[str]:
    """Collect providers that should be treated as Vertex after migration."""
    rows = conn.execute(
        sa.text("""
            SELECT DISTINCT p.id
            FROM providers p
            LEFT JOIN provider_api_keys pak ON pak.provider_id = p.id
            WHERE lower(COALESCE(p.provider_type, '')) = 'vertex_ai'
               OR pak.auth_type = 'vertex_ai'
        """)
    )
    return [str(row[0]) for row in rows if row[0]]


def _ensure_fixed_vertex_endpoints(conn: sa.Connection, provider_ids: list[str]) -> None:
    """Ensure every Vertex provider has fixed gemini:chat + claude:chat endpoints."""
    for provider_id in provider_ids:
        provider_max_retries = (
            conn.execute(
                sa.text("""
                    SELECT COALESCE(max_retries, 2)
                    FROM providers
                    WHERE id = :provider_id
                """),
                {"provider_id": provider_id},
            ).scalar()
            or 2
        )

        for api_format, api_family, endpoint_kind in _VERTEX_ENDPOINTS:
            # Normalize existing fixed endpoint fields.
            conn.execute(
                sa.text("""
                    UPDATE provider_endpoints
                    SET
                        api_family = :api_family,
                        endpoint_kind = :endpoint_kind,
                        base_url = :base_url,
                        custom_path = NULL,
                        is_active = TRUE,
                        updated_at = CURRENT_TIMESTAMP
                    WHERE provider_id = :provider_id
                      AND api_format = :api_format
                """),
                {
                    "provider_id": provider_id,
                    "api_format": api_format,
                    "api_family": api_family,
                    "endpoint_kind": endpoint_kind,
                    "base_url": _VERTEX_BASE_URL,
                },
            )

            exists = conn.execute(
                sa.text("""
                    SELECT 1
                    FROM provider_endpoints
                    WHERE provider_id = :provider_id
                      AND api_format = :api_format
                    LIMIT 1
                """),
                {"provider_id": provider_id, "api_format": api_format},
            ).first()

            if not exists:
                conn.execute(
                    sa.text("""
                        INSERT INTO provider_endpoints (
                            id,
                            provider_id,
                            api_format,
                            api_family,
                            endpoint_kind,
                            base_url,
                            custom_path,
                            header_rules,
                            body_rules,
                            max_retries,
                            is_active,
                            config,
                            format_acceptance_config,
                            proxy,
                            created_at,
                            updated_at
                        )
                        VALUES (
                            :id,
                            :provider_id,
                            :api_format,
                            :api_family,
                            :endpoint_kind,
                            :base_url,
                            NULL,
                            NULL,
                            NULL,
                            :max_retries,
                            TRUE,
                            NULL,
                            NULL,
                            NULL,
                            CURRENT_TIMESTAMP,
                            CURRENT_TIMESTAMP
                        )
                    """),
                    {
                        "id": str(uuid.uuid4()),
                        "provider_id": provider_id,
                        "api_format": api_format,
                        "api_family": api_family,
                        "endpoint_kind": endpoint_kind,
                        "base_url": _VERTEX_BASE_URL,
                        "max_retries": int(provider_max_retries),
                    },
                )

        # Vertex fixed-provider model: disable non-fixed endpoints.
        conn.execute(
            sa.text("""
                UPDATE provider_endpoints
                SET
                    is_active = FALSE,
                    updated_at = CURRENT_TIMESTAMP
                WHERE provider_id = :provider_id
                  AND api_format NOT IN ('gemini:chat', 'claude:chat')
            """),
            {"provider_id": provider_id},
        )


def _normalize_vertex_key_formats(conn: sa.Connection, provider_ids: list[str]) -> None:
    """Normalize key.api_formats for Vertex keys by auth type."""
    for provider_id in provider_ids:
        # Service Account (and legacy vertex_ai) keys: allow Gemini + Claude models.
        conn.execute(
            sa.text("""
                UPDATE provider_api_keys
                SET
                    api_formats = CAST(:api_formats AS json),
                    updated_at = CURRENT_TIMESTAMP
                WHERE provider_id = :provider_id
                  AND auth_type IN ('service_account', 'vertex_ai')
            """),
            {
                "provider_id": provider_id,
                "api_formats": _VERTEX_KEY_FORMATS_SA,
            },
        )

        # API Key mode on Vertex 仅支持 Gemini（Google publisher）。
        conn.execute(
            sa.text("""
                UPDATE provider_api_keys
                SET
                    api_formats = CAST(:api_formats AS json),
                    updated_at = CURRENT_TIMESTAMP
                WHERE provider_id = :provider_id
                  AND auth_type = 'api_key'
            """),
            {
                "provider_id": provider_id,
                "api_formats": _VERTEX_KEY_FORMATS_API_KEY,
            },
        )


def upgrade() -> None:
    conn = op.get_bind()

    # 1) 收集目标 Provider（兼容重复执行，先识别 legacy/new 两种来源）。
    provider_ids = _select_vertex_provider_ids(conn)

    # 2) 先重命名 auth_type（legacy vertex_ai -> service_account）。
    conn.execute(
        sa.text("""
            UPDATE provider_api_keys
            SET auth_type = 'service_account'
            WHERE auth_type = 'vertex_ai'
        """)
    )

    if not provider_ids:
        return

    # 3) 归一 provider_type，并启用格式转换（Vertex 同时承载 Gemini/Claude）。
    for provider_id in provider_ids:
        conn.execute(
            sa.text("""
                UPDATE providers
                SET
                    provider_type = 'vertex_ai',
                    enable_format_conversion = TRUE
                WHERE id = :provider_id
            """),
            {"provider_id": provider_id},
        )

    # 4) 固定端点落地：gemini:chat + claude:chat。
    _ensure_fixed_vertex_endpoints(conn, provider_ids)

    # 5) 归一 key 的 api_formats，避免调度命中旧格式。
    _normalize_vertex_key_formats(conn, provider_ids)


def downgrade() -> None:
    conn = op.get_bind()

    provider_rows = conn.execute(
        sa.text("""
            SELECT id
            FROM providers
            WHERE lower(COALESCE(provider_type, '')) = 'vertex_ai'
        """)
    )
    provider_ids = [str(row[0]) for row in provider_rows if row[0]]

    if provider_ids:
        for provider_id in provider_ids:
            conn.execute(
                sa.text("""
                    UPDATE provider_api_keys
                    SET auth_type = 'vertex_ai'
                    WHERE provider_id = :provider_id
                      AND auth_type = 'service_account'
                """),
                {"provider_id": provider_id},
            )
            conn.execute(
                sa.text("""
                    UPDATE providers
                    SET provider_type = 'custom'
                    WHERE id = :provider_id
                """),
                {"provider_id": provider_id},
            )
