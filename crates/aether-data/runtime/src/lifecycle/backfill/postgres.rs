use std::collections::{HashMap, HashSet};

use sqlx::{
    migrate::{Migrate, MigrateError, Migrator},
    query, query_scalar, Connection, PgConnection, PgPool, Row,
};
use tracing::{error, info, warn};

use super::types::PendingBackfillInfo;

static BACKFILL_MIGRATOR: Migrator = sqlx::migrate!("./backfills/postgres");

const SCHEMA_BACKFILLS_TABLE_EXISTS_SQL: &str =
    "SELECT to_regclass('public.schema_backfills') IS NOT NULL";
const ENSURE_SCHEMA_BACKFILLS_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS public.schema_backfills (
    version bigint NOT NULL,
    description text NOT NULL,
    success boolean NOT NULL DEFAULT TRUE,
    checksum bytea NOT NULL,
    execution_time bigint NOT NULL DEFAULT 0,
    applied_at timestamp with time zone NOT NULL DEFAULT now(),
    CONSTRAINT schema_backfills_pkey PRIMARY KEY (version)
)
"#;
const LIST_APPLIED_BACKFILLS_SQL: &str = r#"
SELECT version, checksum
FROM schema_backfills
WHERE success IS TRUE
ORDER BY version ASC
"#;
const INSERT_APPLIED_BACKFILL_SQL: &str = r#"
INSERT INTO schema_backfills (
    version,
    description,
    success,
    checksum,
    execution_time,
    applied_at
) VALUES (
    $1,
    $2,
    TRUE,
    $3,
    $4,
    NOW()
)
ON CONFLICT (version) DO NOTHING
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AppliedBackfill {
    pub(super) version: i64,
    pub(super) checksum: Vec<u8>,
}

pub async fn run_backfills(pool: &PgPool) -> Result<(), MigrateError> {
    let mut conn = pool.acquire().await?;

    if BACKFILL_MIGRATOR.locking {
        conn.lock().await?;
    }

    let result = run_backfills_locked(&mut conn).await;

    if BACKFILL_MIGRATOR.locking {
        match conn.unlock().await {
            Ok(()) => {}
            Err(unlock_error) if result.is_ok() => return Err(unlock_error),
            Err(unlock_error) => {
                warn!(
                    error = %unlock_error,
                    "database backfill lock release failed after backfill error"
                );
            }
        }
    }

    result
}

pub async fn pending_backfills(pool: &PgPool) -> Result<Vec<PendingBackfillInfo>, MigrateError> {
    let mut conn = pool.acquire().await?;
    pending_backfills_locked(&mut conn).await
}

async fn run_backfills_locked(conn: &mut PgConnection) -> Result<(), MigrateError> {
    ensure_schema_backfills_table(conn).await?;

    let applied_backfills = list_applied_backfills(conn).await?;
    validate_applied_backfills(&applied_backfills)?;

    let applied_by_version: HashMap<_, _> = applied_backfills
        .iter()
        .map(|backfill| (backfill.version, backfill))
        .collect();

    let pending_backfills: Vec<_> = BACKFILL_MIGRATOR
        .iter()
        .filter(|backfill| backfill.migration_type.is_up_migration())
        .filter(|backfill| !applied_by_version.contains_key(&backfill.version))
        .collect();

    if pending_backfills.is_empty() {
        info!(
            pending_backfills = 0,
            "database backfills already up to date"
        );
        return Ok(());
    }

    info!(
        pending_backfills = pending_backfills.len(),
        "database backfills pending"
    );

    for (index, backfill) in pending_backfills.iter().enumerate() {
        let current = index + 1;
        let total = pending_backfills.len();
        info!(
            current,
            total,
            version = backfill.version,
            description = %backfill.description,
            "applying database backfill"
        );

        let mut tx = conn.begin().await?;
        let started_at = std::time::Instant::now();
        sqlx::raw_sql(&backfill.sql).execute(&mut *tx).await?;
        let elapsed_ms = i64::try_from(started_at.elapsed().as_millis()).unwrap_or(i64::MAX);
        query(INSERT_APPLIED_BACKFILL_SQL)
            .bind(backfill.version)
            .bind(backfill.description.as_ref())
            .bind(backfill.checksum.as_ref())
            .bind(elapsed_ms)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        info!(
            current,
            total,
            version = backfill.version,
            description = %backfill.description,
            elapsed_ms,
            "applied database backfill"
        );
    }

    info!(pending_backfills = 0, "database backfills complete");
    Ok(())
}

async fn pending_backfills_locked(
    conn: &mut PgConnection,
) -> Result<Vec<PendingBackfillInfo>, MigrateError> {
    ensure_schema_backfills_table(conn).await?;
    let applied_backfills = list_applied_backfills(conn).await?;
    validate_applied_backfills(&applied_backfills)?;
    Ok(pending_backfills_from_applied(&applied_backfills))
}

async fn ensure_schema_backfills_table(conn: &mut PgConnection) -> Result<(), MigrateError> {
    let exists: bool = query_scalar(SCHEMA_BACKFILLS_TABLE_EXISTS_SQL)
        .fetch_one(&mut *conn)
        .await?;
    if exists {
        return Ok(());
    }
    query(ENSURE_SCHEMA_BACKFILLS_TABLE_SQL)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn list_applied_backfills(
    conn: &mut PgConnection,
) -> Result<Vec<AppliedBackfill>, MigrateError> {
    let rows = query(LIST_APPLIED_BACKFILLS_SQL)
        .fetch_all(&mut *conn)
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok(AppliedBackfill {
                version: row.try_get("version")?,
                checksum: row.try_get("checksum")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(MigrateError::from)
}

fn validate_applied_backfills(applied_backfills: &[AppliedBackfill]) -> Result<(), MigrateError> {
    if BACKFILL_MIGRATOR.ignore_missing {
        return Ok(());
    }

    let known_versions: HashSet<_> = BACKFILL_MIGRATOR
        .iter()
        .map(|backfill| backfill.version)
        .collect();

    for applied_backfill in applied_backfills {
        if !known_versions.contains(&applied_backfill.version) {
            error!(
                version = applied_backfill.version,
                "applied database backfill is missing from embedded backfills"
            );
            return Err(MigrateError::VersionMissing(applied_backfill.version));
        }
    }

    for backfill in BACKFILL_MIGRATOR
        .iter()
        .filter(|backfill| backfill.migration_type.is_up_migration())
    {
        let Some(applied) = applied_backfills
            .iter()
            .find(|applied| applied.version == backfill.version)
        else {
            continue;
        };

        if backfill.checksum != applied.checksum {
            warn!(
                version = backfill.version,
                description = %backfill.description,
                "applied database backfill checksum differs from embedded backfill; skipping strict enforcement"
            );
        }
    }

    Ok(())
}

pub(super) fn pending_backfills_from_applied(
    applied_backfills: &[AppliedBackfill],
) -> Vec<PendingBackfillInfo> {
    let applied_versions: HashSet<_> = applied_backfills
        .iter()
        .map(|backfill| backfill.version)
        .collect();
    BACKFILL_MIGRATOR
        .iter()
        .filter(|backfill| backfill.migration_type.is_up_migration())
        .filter(|backfill| !applied_versions.contains(&backfill.version))
        .map(|backfill| PendingBackfillInfo {
            version: backfill.version,
            description: backfill.description.to_string(),
        })
        .collect()
}
