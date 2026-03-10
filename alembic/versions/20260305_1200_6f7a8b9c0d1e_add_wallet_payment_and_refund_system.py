"""add_wallet_payment_and_refund_system

Revision ID: 6f7a8b9c0d1e
Revises: 6a9b8c7d5e4f
Create Date: 2026-03-05 12:00:00.000000+00:00
"""

from __future__ import annotations

from collections.abc import Sequence
from datetime import datetime, timedelta, timezone
from decimal import Decimal, ROUND_HALF_UP
import json
import uuid

import sqlalchemy as sa
from sqlalchemy import inspect
from sqlalchemy.dialects import postgresql

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "6f7a8b9c0d1e"
down_revision: str | None = "6a9b8c7d5e4f"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None

_MONEY_QUANT = Decimal("0.00000001")
_JSONB_TYPE = sa.JSON().with_variant(postgresql.JSONB(astext_type=sa.Text()), "postgresql")


def _table_exists(table_name: str) -> bool:
    bind = op.get_bind()
    insp = inspect(bind)
    insp.clear_cache()
    return table_name in insp.get_table_names()


def _column_exists(table_name: str, column_name: str) -> bool:
    bind = op.get_bind()
    insp = inspect(bind)
    insp.clear_cache()
    return column_name in [c["name"] for c in insp.get_columns(table_name)]


def _index_exists(table_name: str, index_name: str) -> bool:
    bind = op.get_bind()
    insp = inspect(bind)
    insp.clear_cache()
    return any(i.get("name") == index_name for i in insp.get_indexes(table_name))


def _to_decimal(value: object | None) -> Decimal:
    if value is None:
        return Decimal("0")
    return Decimal(str(value)).quantize(_MONEY_QUANT, rounding=ROUND_HALF_UP)


# NUMERIC(20, 8) -> 整数部分最多 12 位 -> 绝对值上限
_NUMERIC_20_8_CAP = Decimal("999999999999.99999999")


def _clamp(value: Decimal) -> Decimal:
    """Clamp a Decimal to fit NUMERIC(20, 8) range."""
    if value > _NUMERIC_20_8_CAP:
        return _NUMERIC_20_8_CAP
    if value < -_NUMERIC_20_8_CAP:
        return -_NUMERIC_20_8_CAP
    return value


def _insert_wallet(
    conn: sa.Connection,
    *,
    user_id: str | None,
    api_key_id: str | None,
    balance: Decimal,
    gift_balance: Decimal,
    limit_mode: str,
    total_recharged: Decimal,
    total_consumed: Decimal,
    total_adjusted: Decimal,
    created_at: object,
    updated_at: object,
) -> str:
    wallet_id = str(uuid.uuid4())
    conn.execute(
        sa.text(
            """
            INSERT INTO wallets (
                id, user_id, api_key_id, balance, gift_balance, limit_mode, currency, status,
                total_recharged, total_consumed, total_refunded, total_adjusted,
                version, created_at, updated_at
            ) VALUES (
                :id, :user_id, :api_key_id, :balance, :gift_balance, :limit_mode, 'USD', 'active',
                :total_recharged, :total_consumed, 0, :total_adjusted,
                0, :created_at, :updated_at
            )
            """
        ),
        {
            "id": wallet_id,
            "user_id": user_id,
            "api_key_id": api_key_id,
            "balance": balance,
            "gift_balance": gift_balance,
            "limit_mode": limit_mode,
            "total_recharged": total_recharged,
            "total_consumed": total_consumed,
            "total_adjusted": total_adjusted,
            "created_at": created_at,
            "updated_at": updated_at,
        },
    )
    return wallet_id


def _insert_wallet_migration_tx(
    conn: sa.Connection,
    *,
    wallet_id: str,
    balance: Decimal,
    recharge_balance: Decimal,
    gift_balance: Decimal,
    created_at: object,
) -> None:
    conn.execute(
        sa.text(
            """
            INSERT INTO wallet_transactions (
                id, wallet_id, category, reason_code, amount,
                balance_before, balance_after,
                recharge_balance_before, recharge_balance_after,
                gift_balance_before, gift_balance_after,
                link_type, link_id, description, created_at
            ) VALUES (
                :id, :wallet_id, 'adjust', 'adjust_system', :amount,
                0, :amount,
                0, :recharge_balance_after,
                0, :gift_balance_after,
                'system_task', :link_id, :description, :created_at
            )
            """
        ),
        {
            "id": str(uuid.uuid4()),
            "wallet_id": wallet_id,
            "amount": balance,
            "recharge_balance_after": recharge_balance,
            "gift_balance_after": gift_balance,
            "link_id": wallet_id,
            "description": "历史钱包迁移初始化",
            "created_at": created_at,
        },
    )


def _backfill_wallets(conn: sa.Connection) -> None:
    if not _table_exists("wallets") or not _table_exists("users") or not _table_exists("api_keys"):
        return

    existing_user_wallets = {
        str(row[0])
        for row in conn.execute(sa.text("SELECT user_id FROM wallets WHERE user_id IS NOT NULL"))
        if row[0] is not None
    }
    existing_api_key_wallets = {
        str(row[0])
        for row in conn.execute(sa.text("SELECT api_key_id FROM wallets WHERE api_key_id IS NOT NULL"))
        if row[0] is not None
    }

    user_rows = conn.execute(
        sa.text(
            """
            SELECT id, quota_usd, used_usd, total_usd, created_at, updated_at
            FROM users
            """
        )
    ).mappings()

    for row in user_rows:
        user_id = str(row["id"])
        if user_id in existing_user_wallets:
            continue

        quota_raw = row["quota_usd"]
        used = _to_decimal(row["used_usd"])
        total = _to_decimal(row["total_usd"])

        is_unlimited = quota_raw is None
        quota = _to_decimal(quota_raw)
        remaining = Decimal("0") if is_unlimited else (quota - used)

        if remaining > Decimal("0"):
            recharge_balance = Decimal("0")
            gift_balance = remaining
        else:
            recharge_balance = remaining
            gift_balance = Decimal("0")

        limit_mode = "unlimited" if is_unlimited else "finite"
        total_recharged = Decimal("0")
        total_adjusted = Decimal("0") if is_unlimited else max(quota, Decimal("0"))
        total_consumed = max(total if total > 0 else used, Decimal("0"))

        recharge_balance = _clamp(recharge_balance)
        gift_balance = _clamp(gift_balance)
        total_adjusted = _clamp(total_adjusted)
        total_consumed = _clamp(total_consumed)

        total_balance = recharge_balance + gift_balance
        wallet_id = _insert_wallet(
            conn,
            user_id=user_id,
            api_key_id=None,
            balance=recharge_balance,
            gift_balance=gift_balance,
            limit_mode=limit_mode,
            total_recharged=total_recharged,
            total_consumed=total_consumed,
            total_adjusted=total_adjusted,
            created_at=row["created_at"],
            updated_at=row["updated_at"],
        )
        _insert_wallet_migration_tx(
            conn,
            wallet_id=wallet_id,
            balance=_clamp(total_balance),
            recharge_balance=recharge_balance,
            gift_balance=gift_balance,
            created_at=row["created_at"],
        )

    api_key_rows = conn.execute(
        sa.text(
            """
            SELECT
                id,
                COALESCE(current_balance_usd, 0) AS current_balance_usd,
                COALESCE(balance_used_usd, 0) AS balance_used_usd,
                created_at,
                updated_at
            FROM api_keys
            WHERE is_standalone = true
            """
        )
    ).mappings()
    for row in api_key_rows:
        api_key_id = str(row["id"])
        if api_key_id in existing_api_key_wallets:
            continue

        current_balance = _to_decimal(row["current_balance_usd"])
        used_balance = _to_decimal(row["balance_used_usd"])
        recharge_balance = _clamp(current_balance - used_balance)
        total_recharged = _clamp(max(current_balance, Decimal("0")))
        total_consumed = _clamp(max(used_balance, Decimal("0")))

        wallet_id = _insert_wallet(
            conn,
            user_id=None,
            api_key_id=api_key_id,
            balance=recharge_balance,
            gift_balance=Decimal("0"),
            limit_mode="finite",
            total_recharged=total_recharged,
            total_consumed=total_consumed,
            total_adjusted=Decimal("0"),
            created_at=row["created_at"],
            updated_at=row["updated_at"],
        )
        _insert_wallet_migration_tx(
            conn,
            wallet_id=wallet_id,
            balance=recharge_balance,
            recharge_balance=recharge_balance,
            gift_balance=Decimal("0"),
            created_at=row["created_at"],
        )


def _backfill_usage_wallet_ids(conn: sa.Connection) -> None:
    if not _table_exists("wallets") or not _table_exists("usage"):
        return

    conn.execute(
        sa.text(
            """
            UPDATE usage AS u
            SET wallet_id = w.id
            FROM wallets AS w
            JOIN api_keys AS ak ON ak.id = w.api_key_id
            WHERE u.wallet_id IS NULL
              AND u.api_key_id = ak.id
              AND ak.is_standalone = true
            """
        )
    )
    conn.execute(
        sa.text(
            """
            UPDATE usage AS u
            SET wallet_id = w.id
            FROM wallets AS w
            WHERE u.wallet_id IS NULL
              AND w.user_id IS NOT NULL
              AND u.user_id = w.user_id
            """
        )
    )


def _backfill_usage_bucket_snapshots(conn: sa.Connection) -> None:
    if not _table_exists("usage"):
        return
    if not _column_exists("usage", "wallet_recharge_balance_before") or not _column_exists(
        "usage", "wallet_recharge_balance_after"
    ):
        return
    if not _column_exists("usage", "wallet_gift_balance_before") or not _column_exists(
        "usage", "wallet_gift_balance_after"
    ):
        return

    conn.execute(
        sa.text(
            """
            UPDATE usage
            SET wallet_recharge_balance_before = wallet_balance_before
            WHERE wallet_recharge_balance_before IS NULL
              AND wallet_balance_before IS NOT NULL
            """
        )
    )
    conn.execute(
        sa.text(
            """
            UPDATE usage
            SET wallet_recharge_balance_after = wallet_balance_after
            WHERE wallet_recharge_balance_after IS NULL
              AND wallet_balance_after IS NOT NULL
            """
        )
    )
    conn.execute(
        sa.text(
            """
            UPDATE usage
            SET wallet_gift_balance_before = 0
            WHERE wallet_gift_balance_before IS NULL
              AND wallet_balance_before IS NOT NULL
            """
        )
    )
    conn.execute(
        sa.text(
            """
            UPDATE usage
            SET wallet_gift_balance_after = 0
            WHERE wallet_gift_balance_after IS NULL
              AND wallet_balance_after IS NOT NULL
            """
        )
    )


def _backfill_wallet_transaction_schema(conn: sa.Connection) -> None:
    if not _table_exists("wallet_transactions"):
        return

    if _column_exists("wallet_transactions", "recharge_balance_before"):
        conn.execute(
            sa.text(
                """
                UPDATE wallet_transactions
                SET recharge_balance_before = balance_before
                WHERE recharge_balance_before IS NULL
                """
            )
        )
    if _column_exists("wallet_transactions", "recharge_balance_after"):
        conn.execute(
            sa.text(
                """
                UPDATE wallet_transactions
                SET recharge_balance_after = balance_after
                WHERE recharge_balance_after IS NULL
                """
            )
        )
    if _column_exists("wallet_transactions", "gift_balance_before"):
        conn.execute(
            sa.text(
                """
                UPDATE wallet_transactions
                SET gift_balance_before = 0
                WHERE gift_balance_before IS NULL
                """
            )
        )
    if _column_exists("wallet_transactions", "gift_balance_after"):
        conn.execute(
            sa.text(
                """
                UPDATE wallet_transactions
                SET gift_balance_after = 0
                WHERE gift_balance_after IS NULL
                """
            )
        )

    conn.execute(
        sa.text(
            """
            UPDATE wallet_transactions
            SET category = 'adjust'
            WHERE category IS NULL
            """
        )
    )
    conn.execute(
        sa.text(
            """
            UPDATE wallet_transactions
            SET reason_code = 'adjust_admin'
            WHERE reason_code IS NULL
            """
        )
    )
    conn.execute(
        sa.text(
            """
            UPDATE wallet_transactions
            SET link_type = 'system_task'
            WHERE link_type IS NULL
            """
        )
    )
    conn.execute(
        sa.text(
            """
            UPDATE wallet_transactions
            SET link_id = wallet_id
            WHERE link_id IS NULL
            """
        )
    )

    # 对早期用户迁移流水修正分账户快照（用户剩余额度属于赠款）。
    conn.execute(
        sa.text(
            """
            UPDATE wallet_transactions AS tx
            SET
                recharge_balance_before = 0,
                recharge_balance_after = CASE WHEN w.balance < 0 THEN w.balance ELSE 0 END,
                gift_balance_before = 0,
                gift_balance_after = CASE WHEN w.gift_balance > 0 THEN w.gift_balance ELSE 0 END
            FROM wallets AS w
            WHERE tx.wallet_id = w.id
              AND w.user_id IS NOT NULL
              AND tx.reason_code = 'adjust_system'
              AND tx.link_type = 'system_task'
            """
        )
    )


def _ensure_initial_gift_config(conn: sa.Connection) -> None:
    if not _table_exists("system_configs"):
        return

    existing = conn.execute(
        sa.text(
            """
            SELECT 1
            FROM system_configs
            WHERE key = 'default_user_initial_gift_usd'
            LIMIT 1
            """
        )
    ).first()
    if existing is not None:
        return

    initial_gift_value = 10.0
    conn.execute(
        sa.text(
            """
            INSERT INTO system_configs (
                id, key, value, description, created_at, updated_at
            ) VALUES (
                :id, 'default_user_initial_gift_usd', CAST(:value AS JSONB), :description, NOW(), NOW()
            )
            """
        ),
        {
            "id": str(uuid.uuid4()),
            "value": json.dumps(initial_gift_value),
            "description": "新用户默认初始赠款（美元）",
        },
    )


def _cleanup_standalone_quota_config(conn: sa.Connection) -> None:
    if not _table_exists("system_configs"):
        return

    conn.execute(
        sa.text(
            """
            DELETE FROM system_configs
            WHERE key IN (
                'enable_standalone_key_quota_reset',
                'standalone_key_quota_reset_time',
                'standalone_key_quota_reset_interval_days',
                'standalone_key_quota_reset_mode',
                'standalone_key_quota_reset_key_ids',
                'standalone_key_quota_last_reset_at'
            )
            """
        )
    )


def _drop_obsolete_api_key_balance_columns() -> None:
    if not _table_exists("api_keys"):
        return
    if _column_exists("api_keys", "balance_used_usd"):
        op.drop_column("api_keys", "balance_used_usd")
    if _column_exists("api_keys", "current_balance_usd"):
        op.drop_column("api_keys", "current_balance_usd")


def _drop_obsolete_user_quota_schema() -> None:
    if _table_exists("users"):
        if _column_exists("users", "total_usd"):
            op.drop_column("users", "total_usd")
        if _column_exists("users", "used_usd"):
            op.drop_column("users", "used_usd")
        if _column_exists("users", "quota_usd"):
            op.drop_column("users", "quota_usd")

    if _table_exists("user_quotas"):
        op.drop_table("user_quotas")


def _restore_user_quota_schema_for_downgrade() -> None:
    if _table_exists("users"):
        if not _column_exists("users", "quota_usd"):
            op.add_column("users", sa.Column("quota_usd", sa.Float(), nullable=True))
        if not _column_exists("users", "used_usd"):
            op.add_column(
                "users",
                sa.Column("used_usd", sa.Float(), nullable=True, server_default="0.0"),
            )
        if not _column_exists("users", "total_usd"):
            op.add_column(
                "users",
                sa.Column("total_usd", sa.Float(), nullable=True, server_default="0.0"),
            )

    if not _table_exists("user_quotas"):
        op.create_table(
            "user_quotas",
            sa.Column("id", sa.String(length=36), primary_key=True),
            sa.Column(
                "user_id",
                sa.String(length=36),
                sa.ForeignKey("users.id", ondelete="CASCADE"),
                nullable=False,
            ),
            sa.Column("quota_type", sa.String(length=50), nullable=False),
            sa.Column("quota_usd", sa.Float(), nullable=False),
            sa.Column("period_start", sa.DateTime(timezone=True), nullable=False),
            sa.Column("period_end", sa.DateTime(timezone=True), nullable=False),
            sa.Column("used_usd", sa.Float(), nullable=True, server_default="0.0"),
            sa.Column("is_active", sa.Boolean(), nullable=True, server_default=sa.text("true")),
            sa.Column(
                "created_at",
                sa.DateTime(timezone=True),
                nullable=False,
                server_default=sa.func.now(),
            ),
            sa.Column(
                "updated_at",
                sa.DateTime(timezone=True),
                nullable=False,
                server_default=sa.func.now(),
            ),
        )
    if _table_exists("user_quotas") and _index_exists("user_quotas", "ix_user_quotas_id"):
        op.drop_index("ix_user_quotas_id", table_name="user_quotas")


def _backfill_legacy_quota_data_for_downgrade(conn: sa.Connection) -> None:
    """将钱包体系数据回填到旧配额字段，避免 downgrade 后用户数据丢失。"""

    if not _table_exists("wallets"):
        return

    if _table_exists("users"):
        # 旧用户配额模型无法表达“充值/赠款双账户”，按总可用余额 + 已消费折叠回填：
        # quota_usd ~= spendable + consumed, used_usd/total_usd ~= consumed。
        conn.execute(
            sa.text(
                """
                UPDATE users AS u
                SET
                    quota_usd = CASE
                        WHEN w.limit_mode = 'unlimited' THEN NULL
                        ELSE CAST(
                            COALESCE(w.balance, 0)
                            + COALESCE(w.gift_balance, 0)
                            + COALESCE(w.total_consumed, 0)
                            AS DOUBLE PRECISION
                        )
                    END,
                    used_usd = CAST(COALESCE(w.total_consumed, 0) AS DOUBLE PRECISION),
                    total_usd = CAST(COALESCE(w.total_consumed, 0) AS DOUBLE PRECISION)
                FROM wallets AS w
                WHERE w.user_id IS NOT NULL
                  AND w.user_id = u.id
                """
            )
        )

    if _table_exists("api_keys"):
        # 逆向恢复历史独立 Key 字段：
        # current_balance_usd - balance_used_usd = 当前可用余额(balance)。
        conn.execute(
            sa.text(
                """
                UPDATE api_keys AS ak
                SET
                    current_balance_usd = CAST(
                        COALESCE(w.balance, 0) + COALESCE(w.total_consumed, 0)
                        AS DOUBLE PRECISION
                    ),
                    balance_used_usd = CAST(COALESCE(w.total_consumed, 0) AS DOUBLE PRECISION)
                FROM wallets AS w
                WHERE w.api_key_id IS NOT NULL
                  AND w.api_key_id = ak.id
                """
            )
        )

    if _table_exists("user_quotas") and _table_exists("users"):
        now = datetime.now(timezone.utc)
        period_end = now + timedelta(days=30)
        user_rows = conn.execute(
            sa.text(
                """
                SELECT id, quota_usd, used_usd
                FROM users
                WHERE quota_usd IS NOT NULL
                """
            )
        ).mappings()

        for row in user_rows:
            exists = conn.execute(
                sa.text(
                    """
                    SELECT 1
                    FROM user_quotas
                    WHERE user_id = :user_id
                      AND is_active = true
                    LIMIT 1
                    """
                ),
                {"user_id": row["id"]},
            ).first()
            if exists is not None:
                continue

            conn.execute(
                sa.text(
                    """
                    INSERT INTO user_quotas (
                        id,
                        user_id,
                        quota_type,
                        quota_usd,
                        period_start,
                        period_end,
                        used_usd,
                        is_active,
                        created_at,
                        updated_at
                    ) VALUES (
                        :id,
                        :user_id,
                        :quota_type,
                        :quota_usd,
                        :period_start,
                        :period_end,
                        :used_usd,
                        true,
                        :created_at,
                        :updated_at
                    )
                    """
                ),
                {
                    "id": str(uuid.uuid4()),
                    "user_id": row["id"],
                    "quota_type": "monthly",
                    "quota_usd": row["quota_usd"],
                    "period_start": now,
                    "period_end": period_end,
                    "used_usd": row["used_usd"] or 0.0,
                    "created_at": now,
                    "updated_at": now,
                },
            )


def upgrade() -> None:
    if not _table_exists("wallets"):
        op.create_table(
            "wallets",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("user_id", sa.String(length=36), nullable=True),
            sa.Column("api_key_id", sa.String(length=36), nullable=True),
            sa.Column("balance", sa.Numeric(20, 8), nullable=False, server_default="0"),
            sa.Column("gift_balance", sa.Numeric(20, 8), nullable=False, server_default="0"),
            sa.Column("limit_mode", sa.String(length=20), nullable=False, server_default="finite"),
            sa.Column("currency", sa.String(length=3), nullable=False, server_default="USD"),
            sa.Column("status", sa.String(length=20), nullable=False, server_default="active"),
            sa.Column("total_recharged", sa.Numeric(20, 8), nullable=False, server_default="0"),
            sa.Column("total_consumed", sa.Numeric(20, 8), nullable=False, server_default="0"),
            sa.Column("total_refunded", sa.Numeric(20, 8), nullable=False, server_default="0"),
            sa.Column("total_adjusted", sa.Numeric(20, 8), nullable=False, server_default="0"),
            sa.Column("version", sa.Integer(), nullable=False, server_default="0"),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
            sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="SET NULL"),
            sa.ForeignKeyConstraint(["api_key_id"], ["api_keys.id"], ondelete="SET NULL"),
            sa.PrimaryKeyConstraint("id"),
            sa.CheckConstraint(
                "(user_id IS NOT NULL AND api_key_id IS NULL) "
                "OR (user_id IS NULL AND api_key_id IS NOT NULL) "
                "OR (user_id IS NULL AND api_key_id IS NULL)",
                name="ck_wallet_single_owner",
            ),
            sa.CheckConstraint("gift_balance >= 0", name="ck_wallets_gift_balance_non_negative"),
            sa.UniqueConstraint("user_id", name="uq_wallets_user_id"),
            sa.UniqueConstraint("api_key_id", name="uq_wallets_api_key_id"),
        )
        # user_id/api_key_id 的 UniqueConstraint 已隐含唯一索引，无需额外创建
        op.create_index("idx_wallets_status", "wallets", ["status"])
        op.create_index("idx_wallets_limit_mode", "wallets", ["limit_mode"])
    else:
        if not _column_exists("wallets", "gift_balance"):
            op.add_column(
                "wallets",
                sa.Column("gift_balance", sa.Numeric(20, 8), nullable=False, server_default="0"),
            )
        if not _column_exists("wallets", "limit_mode"):
            op.add_column(
                "wallets",
                sa.Column("limit_mode", sa.String(length=20), nullable=False, server_default="finite"),
            )
        if not _index_exists("wallets", "idx_wallets_limit_mode"):
            op.create_index("idx_wallets_limit_mode", "wallets", ["limit_mode"])
    if _index_exists("wallets", "ix_wallets_id"):
        op.drop_index("ix_wallets_id", table_name="wallets")

    if _table_exists("usage"):
        if not _column_exists("usage", "wallet_id"):
            op.add_column(
                "usage",
                sa.Column(
                    "wallet_id",
                    sa.String(length=36),
                    sa.ForeignKey("wallets.id", ondelete="SET NULL"),
                    nullable=True,
                ),
            )
        if not _column_exists("usage", "wallet_balance_before"):
            op.add_column("usage", sa.Column("wallet_balance_before", sa.Numeric(20, 8), nullable=True))
        if not _column_exists("usage", "wallet_balance_after"):
            op.add_column("usage", sa.Column("wallet_balance_after", sa.Numeric(20, 8), nullable=True))
        if not _column_exists("usage", "wallet_recharge_balance_before"):
            op.add_column(
                "usage",
                sa.Column("wallet_recharge_balance_before", sa.Numeric(20, 8), nullable=True),
            )
        if not _column_exists("usage", "wallet_recharge_balance_after"):
            op.add_column(
                "usage",
                sa.Column("wallet_recharge_balance_after", sa.Numeric(20, 8), nullable=True),
            )
        if not _column_exists("usage", "wallet_gift_balance_before"):
            op.add_column(
                "usage",
                sa.Column("wallet_gift_balance_before", sa.Numeric(20, 8), nullable=True),
            )
        if not _column_exists("usage", "wallet_gift_balance_after"):
            op.add_column(
                "usage",
                sa.Column("wallet_gift_balance_after", sa.Numeric(20, 8), nullable=True),
            )
        if not _index_exists("usage", "idx_usage_wallet_finalized"):
            op.create_index("idx_usage_wallet_finalized", "usage", ["wallet_id", "finalized_at"])

    if not _table_exists("wallet_transactions"):
        op.create_table(
            "wallet_transactions",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("wallet_id", sa.String(length=36), nullable=False),
            sa.Column("category", sa.String(length=20), nullable=False),
            sa.Column("reason_code", sa.String(length=40), nullable=False),
            sa.Column("amount", sa.Numeric(20, 8), nullable=False),
            sa.Column("balance_before", sa.Numeric(20, 8), nullable=False),
            sa.Column("balance_after", sa.Numeric(20, 8), nullable=False),
            sa.Column("recharge_balance_before", sa.Numeric(20, 8), nullable=True),
            sa.Column("recharge_balance_after", sa.Numeric(20, 8), nullable=True),
            sa.Column("gift_balance_before", sa.Numeric(20, 8), nullable=True),
            sa.Column("gift_balance_after", sa.Numeric(20, 8), nullable=True),
            sa.Column("link_type", sa.String(length=30), nullable=True),
            sa.Column("link_id", sa.String(length=100), nullable=True),
            sa.Column("operator_id", sa.String(length=36), nullable=True),
            sa.Column("description", sa.Text(), nullable=True),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.ForeignKeyConstraint(["wallet_id"], ["wallets.id"], ondelete="CASCADE"),
            sa.ForeignKeyConstraint(["operator_id"], ["users.id"], ondelete="SET NULL"),
            sa.PrimaryKeyConstraint("id"),
        )
        op.create_index(
            "idx_wallet_tx_wallet_created", "wallet_transactions", ["wallet_id", "created_at"]
        )
        op.create_index("idx_wallet_tx_link", "wallet_transactions", ["link_type", "link_id"])
        op.create_index(
            "idx_wallet_tx_category_created", "wallet_transactions", ["category", "created_at"]
        )
        op.create_index(
            "idx_wallet_tx_reason_created", "wallet_transactions", ["reason_code", "created_at"]
        )
    else:
        if not _column_exists("wallet_transactions", "recharge_balance_before"):
            op.add_column(
                "wallet_transactions",
                sa.Column("recharge_balance_before", sa.Numeric(20, 8), nullable=True),
            )
        if not _column_exists("wallet_transactions", "recharge_balance_after"):
            op.add_column(
                "wallet_transactions",
                sa.Column("recharge_balance_after", sa.Numeric(20, 8), nullable=True),
            )
        if not _column_exists("wallet_transactions", "gift_balance_before"):
            op.add_column(
                "wallet_transactions",
                sa.Column("gift_balance_before", sa.Numeric(20, 8), nullable=True),
            )
        if not _column_exists("wallet_transactions", "gift_balance_after"):
            op.add_column(
                "wallet_transactions",
                sa.Column("gift_balance_after", sa.Numeric(20, 8), nullable=True),
            )
        if not _column_exists("wallet_transactions", "category"):
            op.add_column(
                "wallet_transactions",
                sa.Column("category", sa.String(length=20), nullable=True),
            )
        if not _column_exists("wallet_transactions", "reason_code"):
            op.add_column(
                "wallet_transactions",
                sa.Column("reason_code", sa.String(length=40), nullable=True),
            )
        if not _column_exists("wallet_transactions", "link_type"):
            op.add_column(
                "wallet_transactions",
                sa.Column("link_type", sa.String(length=30), nullable=True),
            )
        if not _column_exists("wallet_transactions", "link_id"):
            op.add_column(
                "wallet_transactions",
                sa.Column("link_id", sa.String(length=100), nullable=True),
            )
    if _index_exists("wallet_transactions", "ix_wallet_transactions_id"):
        op.drop_index("ix_wallet_transactions_id", table_name="wallet_transactions")

    if not _table_exists("payment_orders"):
        op.create_table(
            "payment_orders",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("order_no", sa.String(length=64), nullable=False),
            sa.Column("wallet_id", sa.String(length=36), nullable=False),
            sa.Column("user_id", sa.String(length=36), nullable=True),
            sa.Column("amount_usd", sa.Numeric(20, 8), nullable=False),
            sa.Column("pay_amount", sa.Numeric(20, 2), nullable=True),
            sa.Column("pay_currency", sa.String(length=3), nullable=True),
            sa.Column("exchange_rate", sa.Numeric(18, 8), nullable=True),
            sa.Column("refunded_amount_usd", sa.Numeric(20, 8), nullable=False, server_default="0"),
            sa.Column("refundable_amount_usd", sa.Numeric(20, 8), nullable=False, server_default="0"),
            sa.Column("payment_method", sa.String(length=30), nullable=False),
            sa.Column("gateway_order_id", sa.String(length=128), nullable=True),
            sa.Column("gateway_response", _JSONB_TYPE, nullable=True),
            sa.Column("status", sa.String(length=20), nullable=False, server_default="pending"),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("paid_at", sa.DateTime(timezone=True), nullable=True),
            sa.Column("credited_at", sa.DateTime(timezone=True), nullable=True),
            sa.Column("expires_at", sa.DateTime(timezone=True), nullable=True),
            sa.ForeignKeyConstraint(["wallet_id"], ["wallets.id"], ondelete="RESTRICT"),
            sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="SET NULL"),
            sa.PrimaryKeyConstraint("id"),
            sa.UniqueConstraint("order_no", name="uq_payment_orders_order_no"),
        )
        op.create_index(
            "idx_payment_orders_wallet_created", "payment_orders", ["wallet_id", "created_at"]
        )
        op.create_index("idx_payment_orders_user_created", "payment_orders", ["user_id", "created_at"])
        op.create_index("idx_payment_orders_status", "payment_orders", ["status"])
        op.create_index(
            "idx_payment_orders_gateway_order_id", "payment_orders", ["gateway_order_id"]
        )
    if _index_exists("payment_orders", "ix_payment_orders_id"):
        op.drop_index("ix_payment_orders_id", table_name="payment_orders")

    if not _table_exists("payment_callbacks"):
        op.create_table(
            "payment_callbacks",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("payment_order_id", sa.String(length=36), nullable=True),
            sa.Column("payment_method", sa.String(length=30), nullable=False),
            sa.Column("callback_key", sa.String(length=128), nullable=False),
            sa.Column("order_no", sa.String(length=64), nullable=True),
            sa.Column("gateway_order_id", sa.String(length=128), nullable=True),
            sa.Column("payload_hash", sa.String(length=128), nullable=True),
            sa.Column("signature_valid", sa.Boolean(), nullable=False, server_default=sa.text("false")),
            sa.Column("status", sa.String(length=20), nullable=False, server_default="received"),
            sa.Column("payload", _JSONB_TYPE, nullable=True),
            sa.Column("error_message", sa.Text(), nullable=True),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("processed_at", sa.DateTime(timezone=True), nullable=True),
            sa.ForeignKeyConstraint(
                ["payment_order_id"], ["payment_orders.id"], ondelete="SET NULL"
            ),
            sa.PrimaryKeyConstraint("id"),
            sa.UniqueConstraint("callback_key", name="uq_payment_callbacks_callback_key"),
        )
        op.create_index("idx_payment_callbacks_order", "payment_callbacks", ["order_no"])
        op.create_index(
            "idx_payment_callbacks_gateway_order", "payment_callbacks", ["gateway_order_id"]
        )
        op.create_index("idx_payment_callbacks_created", "payment_callbacks", ["created_at"])
    if _index_exists("payment_callbacks", "ix_payment_callbacks_id"):
        op.drop_index("ix_payment_callbacks_id", table_name="payment_callbacks")

    if not _table_exists("refund_requests"):
        op.create_table(
            "refund_requests",
            sa.Column("id", sa.String(length=36), nullable=False),
            sa.Column("refund_no", sa.String(length=64), nullable=False),
            sa.Column("wallet_id", sa.String(length=36), nullable=False),
            sa.Column("user_id", sa.String(length=36), nullable=True),
            sa.Column("payment_order_id", sa.String(length=36), nullable=True),
            sa.Column("source_type", sa.String(length=30), nullable=False),
            sa.Column("source_id", sa.String(length=100), nullable=True),
            sa.Column("refund_mode", sa.String(length=30), nullable=False),
            sa.Column("amount_usd", sa.Numeric(20, 8), nullable=False),
            sa.Column("status", sa.String(length=30), nullable=False, server_default="pending_approval"),
            sa.Column("reason", sa.Text(), nullable=True),
            sa.Column("requested_by", sa.String(length=36), nullable=True),
            sa.Column("approved_by", sa.String(length=36), nullable=True),
            sa.Column("processed_by", sa.String(length=36), nullable=True),
            sa.Column("gateway_refund_id", sa.String(length=128), nullable=True),
            sa.Column("payout_method", sa.String(length=50), nullable=True),
            sa.Column("payout_reference", sa.String(length=255), nullable=True),
            sa.Column("payout_proof", _JSONB_TYPE, nullable=True),
            sa.Column("failure_reason", sa.Text(), nullable=True),
            sa.Column("idempotency_key", sa.String(length=128), nullable=True),
            sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
            sa.Column("processed_at", sa.DateTime(timezone=True), nullable=True),
            sa.Column("completed_at", sa.DateTime(timezone=True), nullable=True),
            sa.ForeignKeyConstraint(["wallet_id"], ["wallets.id"], ondelete="RESTRICT"),
            sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="SET NULL"),
            sa.ForeignKeyConstraint(["payment_order_id"], ["payment_orders.id"], ondelete="SET NULL"),
            sa.ForeignKeyConstraint(["requested_by"], ["users.id"], ondelete="SET NULL"),
            sa.ForeignKeyConstraint(["approved_by"], ["users.id"], ondelete="SET NULL"),
            sa.ForeignKeyConstraint(["processed_by"], ["users.id"], ondelete="SET NULL"),
            sa.PrimaryKeyConstraint("id"),
            sa.UniqueConstraint("refund_no", name="uq_refund_requests_refund_no"),
            sa.UniqueConstraint("idempotency_key", name="uq_refund_requests_idempotency_key"),
        )
        op.create_index("idx_refund_wallet_created", "refund_requests", ["wallet_id", "created_at"])
        op.create_index("idx_refund_user_created", "refund_requests", ["user_id", "created_at"])
        op.create_index("idx_refund_status", "refund_requests", ["status"])
    if _index_exists("refund_requests", "ix_refund_requests_id"):
        op.drop_index("ix_refund_requests_id", table_name="refund_requests")

    conn = op.get_bind()
    _backfill_wallets(conn)
    _backfill_usage_wallet_ids(conn)
    _backfill_usage_bucket_snapshots(conn)
    _backfill_wallet_transaction_schema(conn)

    if _table_exists("wallet_transactions"):
        op.alter_column(
            "wallet_transactions",
            "category",
            existing_type=sa.String(length=20),
            nullable=False,
        )
        op.alter_column(
            "wallet_transactions",
            "reason_code",
            existing_type=sa.String(length=40),
            nullable=False,
        )

        if _index_exists("wallet_transactions", "idx_wallet_tx_reference"):
            op.drop_index("idx_wallet_tx_reference", table_name="wallet_transactions")
        if _index_exists("wallet_transactions", "idx_wallet_tx_type_created"):
            op.drop_index("idx_wallet_tx_type_created", table_name="wallet_transactions")
        if not _index_exists("wallet_transactions", "idx_wallet_tx_link"):
            op.create_index("idx_wallet_tx_link", "wallet_transactions", ["link_type", "link_id"])
        if not _index_exists("wallet_transactions", "idx_wallet_tx_category_created"):
            op.create_index(
                "idx_wallet_tx_category_created",
                "wallet_transactions",
                ["category", "created_at"],
            )
        if not _index_exists("wallet_transactions", "idx_wallet_tx_reason_created"):
            op.create_index(
                "idx_wallet_tx_reason_created",
                "wallet_transactions",
                ["reason_code", "created_at"],
            )

        if _column_exists("wallet_transactions", "tx_type"):
            op.drop_column("wallet_transactions", "tx_type")
        if _column_exists("wallet_transactions", "reference_type"):
            op.drop_column("wallet_transactions", "reference_type")
        if _column_exists("wallet_transactions", "reference_id"):
            op.drop_column("wallet_transactions", "reference_id")

    _ensure_initial_gift_config(conn)
    _cleanup_standalone_quota_config(conn)
    _drop_obsolete_user_quota_schema()
    _drop_obsolete_api_key_balance_columns()


def downgrade() -> None:
    # 先恢复旧 schema，并在新表仍存在时回填历史数据，避免数据不可逆丢失。
    _restore_user_quota_schema_for_downgrade()

    if _table_exists("api_keys"):
        if not _column_exists("api_keys", "current_balance_usd"):
            op.add_column(
                "api_keys",
                sa.Column("current_balance_usd", sa.Float(), nullable=True),
            )
        if not _column_exists("api_keys", "balance_used_usd"):
            op.add_column(
                "api_keys",
                sa.Column("balance_used_usd", sa.Float(), nullable=True, server_default="0.0"),
            )

    conn = op.get_bind()
    _backfill_legacy_quota_data_for_downgrade(conn)

    if _table_exists("refund_requests"):
        op.drop_table("refund_requests")

    if _table_exists("payment_callbacks"):
        op.drop_table("payment_callbacks")

    if _table_exists("payment_orders"):
        op.drop_table("payment_orders")

    if _table_exists("wallet_transactions"):
        op.drop_table("wallet_transactions")

    if _table_exists("usage"):
        if _index_exists("usage", "idx_usage_wallet_finalized"):
            op.drop_index("idx_usage_wallet_finalized", table_name="usage")
        if _column_exists("usage", "wallet_gift_balance_after"):
            op.drop_column("usage", "wallet_gift_balance_after")
        if _column_exists("usage", "wallet_gift_balance_before"):
            op.drop_column("usage", "wallet_gift_balance_before")
        if _column_exists("usage", "wallet_recharge_balance_after"):
            op.drop_column("usage", "wallet_recharge_balance_after")
        if _column_exists("usage", "wallet_recharge_balance_before"):
            op.drop_column("usage", "wallet_recharge_balance_before")
        if _column_exists("usage", "wallet_balance_after"):
            op.drop_column("usage", "wallet_balance_after")
        if _column_exists("usage", "wallet_balance_before"):
            op.drop_column("usage", "wallet_balance_before")
        if _column_exists("usage", "wallet_id"):
            op.drop_column("usage", "wallet_id")

    if _table_exists("wallets"):
        op.drop_table("wallets")
