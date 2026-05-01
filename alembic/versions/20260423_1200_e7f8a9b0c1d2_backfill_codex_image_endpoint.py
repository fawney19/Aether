"""backfill_codex_image_endpoint

Backfill openai:image endpoint for existing Codex providers, and extend
Codex provider API keys' api_formats to include openai:image.

固定类型 Provider 的 endpoint 列表只在创建时写入模板；给 Codex 模板
新增 openai:image 后，存量 Codex Provider 不会自动补，前端又禁用手动
添加 endpoint，所以需要这条迁移把存量补齐。

Revision ID: e7f8a9b0c1d2
Revises: c7d8e9f0a1b2
Create Date: 2026-04-23 12:00:00.000000+00:00
"""

from __future__ import annotations

import json
import uuid

import sqlalchemy as sa

from alembic import op

revision = "e7f8a9b0c1d2"
down_revision = "c7d8e9f0a1b2"
branch_labels = None
depends_on = None


_PROVIDER_TYPE = "codex"
_TARGET_SIG = "openai:image"
_TARGET_FAMILY = "openai"
_TARGET_KIND = "image"
_TARGET_BASE_URL = "https://chatgpt.com/backend-api/codex"


def upgrade() -> None:
    conn = op.get_bind()

    # 1) 为每个 Codex provider 补一条 openai:image endpoint（若不存在）
    providers = conn.execute(
        sa.text(
            "SELECT id, max_retries FROM providers WHERE provider_type = :ptype"
        ),
        {"ptype": _PROVIDER_TYPE},
    ).fetchall()

    created_endpoints = 0
    for row in providers:
        provider_id = row[0]
        max_retries = row[1] or 2

        existing = conn.execute(
            sa.text(
                """
                SELECT 1 FROM provider_endpoints
                WHERE provider_id = :pid AND api_format = :fmt
                LIMIT 1
                """
            ),
            {"pid": provider_id, "fmt": _TARGET_SIG},
        ).fetchone()
        if existing:
            continue

        conn.execute(
            sa.text(
                """
                INSERT INTO provider_endpoints
                    (id, provider_id, api_format, api_family, endpoint_kind,
                     base_url, custom_path, header_rules, body_rules,
                     max_retries, is_active, config, proxy,
                     format_acceptance_config, created_at, updated_at)
                VALUES
                    (:id, :pid, :fmt, :family, :kind,
                     :base_url, NULL, NULL, NULL,
                     :max_retries, TRUE, NULL, NULL,
                     NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                """
            ),
            {
                "id": str(uuid.uuid4()),
                "pid": provider_id,
                "fmt": _TARGET_SIG,
                "family": _TARGET_FAMILY,
                "kind": _TARGET_KIND,
                "base_url": _TARGET_BASE_URL,
                "max_retries": max_retries,
            },
        )
        created_endpoints += 1

    if created_endpoints:
        print(f"  created {created_endpoints} codex openai:image endpoint(s)")

    # 2) 扩充 Codex provider API keys 的 api_formats
    #    仅对已经有 openai:cli 或 openai:compact 的 key 扩充（代表该 key 被配置
    #    为服务 Codex 反代），避免污染早期的"仅供其他用途"的残留 key。
    #
    # 列类型是 JSON（不是 JSONB），没有 @> / 串联 || 操作符；直接 Python 侧
    # 判断 + 逐行 UPDATE 最稳妥。
    codex_keys = conn.execute(
        sa.text(
            """
            SELECT k.id, k.api_formats
            FROM provider_api_keys k
            JOIN providers p ON p.id = k.provider_id
            WHERE p.provider_type = :ptype
            """
        ),
        {"ptype": _PROVIDER_TYPE},
    ).fetchall()

    updated_keys = 0
    for row in codex_keys:
        key_id = row[0]
        raw_formats = row[1]

        if isinstance(raw_formats, str):
            try:
                formats = json.loads(raw_formats)
            except Exception:
                formats = None
        else:
            formats = raw_formats

        if not isinstance(formats, list):
            continue
        if _TARGET_SIG in formats:
            continue
        if not ({"openai:cli", "openai:compact"} & set(formats)):
            continue

        new_formats = list(formats) + [_TARGET_SIG]
        conn.execute(
            sa.text(
                """
                UPDATE provider_api_keys
                SET api_formats = CAST(:formats AS json),
                    updated_at = CURRENT_TIMESTAMP
                WHERE id = :kid
                """
            ),
            {
                "kid": key_id,
                "formats": json.dumps(new_formats, ensure_ascii=False),
            },
        )
        updated_keys += 1

    if updated_keys:
        print(f"  extended api_formats on {updated_keys} codex key(s)")


def downgrade() -> None:
    # 回滚不自动删除 endpoint/key 数据以防误伤。若确有需要，请手动处理。
    return
