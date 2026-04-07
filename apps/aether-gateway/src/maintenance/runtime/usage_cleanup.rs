use std::io::Write;

use aether_data_contracts::DataLayerError;
use chrono::{DateTime, Utc};
use flate2::{write::GzEncoder, Compression};
use serde_json::Value;
use sqlx::Row;
use tracing::warn;

use crate::data::GatewayDataState;

use super::{
    system_config_bool, usage_cleanup_settings, usage_cleanup_window, ExpiredApiKeyRow,
    UsageBodyCompressionRow, UsageCleanupSummary, CLEAR_USAGE_BODY_FIELDS_SQL,
    CLEAR_USAGE_HEADER_FIELDS_SQL, DELETE_EXPIRED_API_KEY_SQL, DELETE_OLD_USAGE_RECORDS_SQL,
    DISABLE_EXPIRED_API_KEY_SQL, EXPIRED_API_KEY_PRE_CLEAN_BATCH_SIZE,
    NULLIFY_REQUEST_CANDIDATE_API_KEY_BATCH_SQL, NULLIFY_USAGE_API_KEY_BATCH_SQL,
    SELECT_EXPIRED_ACTIVE_API_KEYS_SQL, SELECT_USAGE_BODY_COMPRESSION_BATCH_SQL,
    SELECT_USAGE_HEADER_BATCH_SQL, SELECT_USAGE_STALE_BODY_BATCH_SQL,
    UPDATE_USAGE_BODY_COMPRESSION_SQL,
};

pub(super) async fn perform_usage_cleanup_once(
    data: &GatewayDataState,
) -> Result<UsageCleanupSummary, DataLayerError> {
    let Some(pool) = data.postgres_pool() else {
        return Ok(UsageCleanupSummary::default());
    };
    if !system_config_bool(data, "enable_auto_cleanup", true).await? {
        return Ok(UsageCleanupSummary::default());
    }

    let settings = usage_cleanup_settings(data).await?;
    let window = usage_cleanup_window(Utc::now(), settings);
    let records_deleted =
        delete_old_usage_records(&pool, window.log_cutoff, settings.batch_size).await?;
    let header_cleaned = cleanup_usage_header_fields(
        &pool,
        window.header_cutoff,
        settings.batch_size,
        Some(window.log_cutoff),
    )
    .await?;
    let body_cleaned = cleanup_usage_stale_body_fields(
        &pool,
        window.compressed_cutoff,
        settings.batch_size,
        Some(window.log_cutoff),
    )
    .await?;
    let body_compressed = compress_usage_body_fields(
        &pool,
        window.detail_cutoff,
        settings.batch_size,
        Some(window.compressed_cutoff),
    )
    .await?;
    let keys_cleaned =
        match cleanup_expired_api_keys(&pool, settings.auto_delete_expired_keys).await {
            Ok(count) => count,
            Err(err) => {
                warn!(error = %err, "gateway expired api key cleanup failed");
                0
            }
        };

    Ok(UsageCleanupSummary {
        body_compressed,
        body_cleaned,
        header_cleaned,
        keys_cleaned,
        records_deleted,
    })
}

async fn delete_old_usage_records(
    pool: &aether_data::postgres::PostgresPool,
    cutoff_time: DateTime<Utc>,
    batch_size: usize,
) -> Result<usize, DataLayerError> {
    let mut total_deleted = 0usize;
    loop {
        let deleted = sqlx::query(DELETE_OLD_USAGE_RECORDS_SQL)
            .bind(cutoff_time)
            .bind(i64::try_from(batch_size).unwrap_or(i64::MAX))
            .execute(pool)
            .await
            .map_err(postgres_error)?
            .rows_affected();
        let deleted = usize::try_from(deleted).unwrap_or(usize::MAX);
        total_deleted += deleted;
        if deleted < batch_size {
            break;
        }
    }
    Ok(total_deleted)
}

async fn cleanup_usage_header_fields(
    pool: &aether_data::postgres::PostgresPool,
    cutoff_time: DateTime<Utc>,
    batch_size: usize,
    newer_than: Option<DateTime<Utc>>,
) -> Result<usize, DataLayerError> {
    if matches!(newer_than, Some(value) if value >= cutoff_time) {
        warn!(
            cutoff_time = %cutoff_time,
            newer_than = ?newer_than,
            "gateway usage header cleanup skipped due to invalid window"
        );
        return Ok(0);
    }

    let mut total_cleaned = 0usize;
    loop {
        let ids = sqlx::query(SELECT_USAGE_HEADER_BATCH_SQL)
            .bind(cutoff_time)
            .bind(newer_than)
            .bind(i64::try_from(batch_size).unwrap_or(i64::MAX))
            .fetch_all(pool)
            .await
            .map_err(postgres_error)?
            .into_iter()
            .map(|row| row.try_get::<String, _>("id").map_err(postgres_error))
            .collect::<Result<Vec<_>, DataLayerError>>()?;
        if ids.is_empty() {
            break;
        }

        let cleaned = sqlx::query(CLEAR_USAGE_HEADER_FIELDS_SQL)
            .bind(ids)
            .execute(pool)
            .await
            .map_err(postgres_error)?
            .rows_affected();
        let cleaned = usize::try_from(cleaned).unwrap_or(usize::MAX);
        total_cleaned += cleaned;
        if cleaned == 0 || cleaned < batch_size {
            break;
        }
    }
    Ok(total_cleaned)
}

async fn cleanup_usage_stale_body_fields(
    pool: &aether_data::postgres::PostgresPool,
    cutoff_time: DateTime<Utc>,
    batch_size: usize,
    newer_than: Option<DateTime<Utc>>,
) -> Result<usize, DataLayerError> {
    if matches!(newer_than, Some(value) if value >= cutoff_time) {
        warn!(
            cutoff_time = %cutoff_time,
            newer_than = ?newer_than,
            "gateway usage body cleanup skipped due to invalid window"
        );
        return Ok(0);
    }

    let mut total_cleaned = 0usize;
    loop {
        let ids = sqlx::query(SELECT_USAGE_STALE_BODY_BATCH_SQL)
            .bind(cutoff_time)
            .bind(newer_than)
            .bind(i64::try_from(batch_size).unwrap_or(i64::MAX))
            .fetch_all(pool)
            .await
            .map_err(postgres_error)?
            .into_iter()
            .map(|row| row.try_get::<String, _>("id").map_err(postgres_error))
            .collect::<Result<Vec<_>, DataLayerError>>()?;
        if ids.is_empty() {
            break;
        }

        let cleaned = sqlx::query(CLEAR_USAGE_BODY_FIELDS_SQL)
            .bind(ids)
            .execute(pool)
            .await
            .map_err(postgres_error)?
            .rows_affected();
        let cleaned = usize::try_from(cleaned).unwrap_or(usize::MAX);
        total_cleaned += cleaned;
        if cleaned == 0 || cleaned < batch_size {
            break;
        }
    }
    Ok(total_cleaned)
}

async fn compress_usage_body_fields(
    pool: &aether_data::postgres::PostgresPool,
    cutoff_time: DateTime<Utc>,
    batch_size: usize,
    newer_than: Option<DateTime<Utc>>,
) -> Result<usize, DataLayerError> {
    if matches!(newer_than, Some(value) if value >= cutoff_time) {
        warn!(
            cutoff_time = %cutoff_time,
            newer_than = ?newer_than,
            "gateway usage body compression skipped due to invalid window"
        );
        return Ok(0);
    }

    let mut total_compressed = 0usize;
    let mut no_progress_count = 0usize;
    let batch_size = batch_size.clamp(1, 25);
    loop {
        let rows = sqlx::query(SELECT_USAGE_BODY_COMPRESSION_BATCH_SQL)
            .bind(cutoff_time)
            .bind(newer_than)
            .bind(i64::try_from(batch_size).unwrap_or(i64::MAX))
            .fetch_all(pool)
            .await
            .map_err(postgres_error)?
            .into_iter()
            .map(|row| {
                Ok(UsageBodyCompressionRow {
                    id: row.try_get::<String, _>("id").map_err(postgres_error)?,
                    request_body: row
                        .try_get::<Option<Value>, _>("request_body")
                        .map_err(postgres_error)?,
                    response_body: row
                        .try_get::<Option<Value>, _>("response_body")
                        .map_err(postgres_error)?,
                    provider_request_body: row
                        .try_get::<Option<Value>, _>("provider_request_body")
                        .map_err(postgres_error)?,
                    client_response_body: row
                        .try_get::<Option<Value>, _>("client_response_body")
                        .map_err(postgres_error)?,
                })
            })
            .collect::<Result<Vec<_>, DataLayerError>>()?;
        if rows.is_empty() {
            break;
        }

        let mut batch_success = 0usize;
        for row in rows {
            let compressed = (
                compress_usage_json_value(row.request_body.as_ref()),
                compress_usage_json_value(row.response_body.as_ref()),
                compress_usage_json_value(row.provider_request_body.as_ref()),
                compress_usage_json_value(row.client_response_body.as_ref()),
            );
            let updated = sqlx::query(UPDATE_USAGE_BODY_COMPRESSION_SQL)
                .bind(row.id)
                .bind(compressed.0)
                .bind(compressed.1)
                .bind(compressed.2)
                .bind(compressed.3)
                .execute(pool)
                .await
                .map_err(postgres_error)?
                .rows_affected();
            if updated > 0 {
                batch_success += 1;
            }
        }

        if batch_success == 0 {
            no_progress_count += 1;
            if no_progress_count >= 3 {
                warn!(
                    "gateway usage body compression stopped after repeated zero-progress batches"
                );
                break;
            }
        } else {
            no_progress_count = 0;
        }
        total_compressed += batch_success;
    }
    Ok(total_compressed)
}

fn compress_usage_json_value(value: Option<&Value>) -> Option<Vec<u8>> {
    let value = value?;
    let bytes = serde_json::to_vec(value).ok()?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(&bytes).ok()?;
    encoder.finish().ok()
}

async fn cleanup_expired_api_keys(
    pool: &aether_data::postgres::PostgresPool,
    auto_delete_expired_keys: bool,
) -> Result<usize, DataLayerError> {
    let expired_keys = sqlx::query(SELECT_EXPIRED_ACTIVE_API_KEYS_SQL)
        .fetch_all(pool)
        .await
        .map_err(postgres_error)?;
    let mut cleaned = 0usize;
    for row in &expired_keys {
        let api_key_id = row.try_get::<String, _>("id").map_err(postgres_error)?;
        let key = ExpiredApiKeyRow {
            id: api_key_id.as_str(),
            auto_delete_on_expiry: row
                .try_get::<Option<bool>, _>("auto_delete_on_expiry")
                .map_err(postgres_error)?,
        };
        let should_delete = key
            .auto_delete_on_expiry
            .unwrap_or(auto_delete_expired_keys);
        if should_delete {
            nullify_expired_api_key_usage_refs(pool, key.id).await?;
            nullify_expired_api_key_candidate_refs(pool, key.id).await?;
            let deleted = sqlx::query(DELETE_EXPIRED_API_KEY_SQL)
                .bind(key.id)
                .execute(pool)
                .await
                .map_err(postgres_error)?
                .rows_affected();
            if deleted > 0 {
                cleaned += 1;
            }
        } else {
            let updated = sqlx::query(DISABLE_EXPIRED_API_KEY_SQL)
                .bind(key.id)
                .bind(Utc::now())
                .execute(pool)
                .await
                .map_err(postgres_error)?
                .rows_affected();
            if updated > 0 {
                cleaned += 1;
            }
        }
    }
    Ok(cleaned)
}

async fn nullify_expired_api_key_usage_refs(
    pool: &aether_data::postgres::PostgresPool,
    api_key_id: &str,
) -> Result<(), DataLayerError> {
    loop {
        let updated = sqlx::query(NULLIFY_USAGE_API_KEY_BATCH_SQL)
            .bind(api_key_id)
            .bind(i64::try_from(EXPIRED_API_KEY_PRE_CLEAN_BATCH_SIZE).unwrap_or(i64::MAX))
            .execute(pool)
            .await
            .map_err(postgres_error)?
            .rows_affected();
        let updated = usize::try_from(updated).unwrap_or(usize::MAX);
        if updated < EXPIRED_API_KEY_PRE_CLEAN_BATCH_SIZE {
            break;
        }
    }
    Ok(())
}

async fn nullify_expired_api_key_candidate_refs(
    pool: &aether_data::postgres::PostgresPool,
    api_key_id: &str,
) -> Result<(), DataLayerError> {
    loop {
        let updated = sqlx::query(NULLIFY_REQUEST_CANDIDATE_API_KEY_BATCH_SQL)
            .bind(api_key_id)
            .bind(i64::try_from(EXPIRED_API_KEY_PRE_CLEAN_BATCH_SIZE).unwrap_or(i64::MAX))
            .execute(pool)
            .await
            .map_err(postgres_error)?
            .rows_affected();
        let updated = usize::try_from(updated).unwrap_or(usize::MAX);
        if updated < EXPIRED_API_KEY_PRE_CLEAN_BATCH_SIZE {
            break;
        }
    }
    Ok(())
}

fn postgres_error(error: sqlx::Error) -> DataLayerError {
    DataLayerError::postgres(error)
}
