use std::io::Write;

use aether_data_contracts::repository::usage::{
    parse_usage_body_ref, usage_body_ref, UsageBodyField,
};
use aether_data_contracts::DataLayerError;
use chrono::{DateTime, Utc};
use flate2::{write::GzEncoder, Compression};
use futures_util::TryStreamExt;
use serde_json::{Map, Value};
use sqlx::Row;
use tracing::warn;

use crate::data::GatewayDataState;

use super::{
    system_config_bool, usage_cleanup_settings, usage_cleanup_window, ExpiredApiKeyRow,
    UsageBodyCleanupRow, UsageBodyCompressionRow, UsageCleanupSummary, CLEAR_USAGE_BODY_FIELDS_SQL,
    CLEAR_USAGE_HEADER_FIELDS_SQL, CLEAR_USAGE_HTTP_AUDIT_BODY_REFS_SQL,
    CLEAR_USAGE_HTTP_AUDIT_HEADERS_SQL, DELETE_EMPTY_USAGE_HTTP_AUDITS_SQL,
    DELETE_EXPIRED_API_KEY_SQL, DELETE_OLD_USAGE_RECORDS_SQL, DELETE_USAGE_BODY_BLOBS_SQL,
    DISABLE_EXPIRED_API_KEY_SQL, EXPIRED_API_KEY_PRE_CLEAN_BATCH_SIZE,
    NULLIFY_REQUEST_CANDIDATE_API_KEY_BATCH_SQL, NULLIFY_USAGE_API_KEY_BATCH_SQL,
    SELECT_EXPIRED_ACTIVE_API_KEYS_SQL, SELECT_USAGE_BODY_COMPRESSION_BATCH_SQL,
    SELECT_USAGE_BODY_COMPRESSION_ROW_SQL, SELECT_USAGE_HEADER_BATCH_SQL,
    SELECT_USAGE_LEGACY_BODY_REF_METADATA_BATCH_SQL,
    SELECT_USAGE_STALE_BODY_BATCH_SQL, UPDATE_USAGE_BODY_COMPRESSION_SQL,
    UPDATE_USAGE_REQUEST_METADATA_SQL, UPSERT_USAGE_BODY_BLOB_SQL,
    UPSERT_USAGE_HTTP_AUDIT_BODY_REFS_SQL,
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
    let legacy_body_refs_migrated = migrate_legacy_usage_body_ref_metadata(
        &pool,
        window.detail_cutoff,
        settings.batch_size,
        Some(window.compressed_cutoff),
    )
    .await?;
    let body_cleaned = cleanup_usage_stale_body_fields(
        &pool,
        window.compressed_cutoff,
        settings.batch_size,
        Some(window.log_cutoff),
    )
    .await?;
    let body_externalized = compress_usage_body_fields(
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
        body_externalized,
        legacy_body_refs_migrated,
        body_cleaned,
        header_cleaned,
        keys_cleaned,
        records_deleted,
    })
}

async fn migrate_legacy_usage_body_ref_metadata(
    pool: &aether_data::postgres::PostgresPool,
    cutoff_time: DateTime<Utc>,
    batch_size: usize,
    newer_than: Option<DateTime<Utc>>,
) -> Result<usize, DataLayerError> {
    if matches!(newer_than, Some(value) if value >= cutoff_time) {
        warn!(
            cutoff_time = %cutoff_time,
            newer_than = ?newer_than,
            "gateway usage legacy body-ref migration skipped due to invalid window"
        );
        return Ok(0);
    }

    let mut total_migrated = 0usize;
    loop {
        let rows = sqlx::query(SELECT_USAGE_LEGACY_BODY_REF_METADATA_BATCH_SQL)
            .bind(cutoff_time)
            .bind(newer_than)
            .bind(i64::try_from(batch_size).unwrap_or(i64::MAX))
            .fetch_all(pool)
            .await
            .map_err(postgres_error)?
            .into_iter()
            .map(|row| {
                Ok(UsageLegacyBodyRefMetadataRow {
                    id: row.try_get::<String, _>("id").map_err(postgres_error)?,
                    request_id: row
                        .try_get::<String, _>("request_id")
                        .map_err(postgres_error)?,
                    request_metadata: row
                        .try_get::<Option<Value>, _>("request_metadata")
                        .map_err(postgres_error)?,
                })
            })
            .collect::<Result<Vec<_>, DataLayerError>>()?;
        if rows.is_empty() {
            break;
        }

        let mut batch_migrated = 0usize;
        for row in rows {
            let Some(plan) =
                migrate_legacy_body_ref_metadata_plan(&row.request_id, row.request_metadata)
            else {
                continue;
            };
            let mut tx = pool.begin().await.map_err(postgres_error)?;
            if plan.refs.any_present() {
                sqlx::query(UPSERT_USAGE_HTTP_AUDIT_BODY_REFS_SQL)
                    .bind(&row.request_id)
                    .bind(plan.refs.request_body_ref.as_deref())
                    .bind(plan.refs.provider_request_body_ref.as_deref())
                    .bind(plan.refs.response_body_ref.as_deref())
                    .bind(plan.refs.client_response_body_ref.as_deref())
                    .bind("ref_backed")
                    .execute(&mut *tx)
                    .await
                    .map_err(postgres_error)?;
            }
            let updated = sqlx::query(UPDATE_USAGE_REQUEST_METADATA_SQL)
                .bind(&row.id)
                .bind(plan.request_metadata)
                .execute(&mut *tx)
                .await
                .map_err(postgres_error)?
                .rows_affected();
            tx.commit().await.map_err(postgres_error)?;
            if updated > 0 {
                batch_migrated += 1;
            }
        }

        total_migrated += batch_migrated;
        if batch_migrated == 0 || batch_migrated < batch_size {
            break;
        }
    }

    Ok(total_migrated)
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
        let mut stream = sqlx::query(SELECT_USAGE_HEADER_BATCH_SQL)
            .bind(cutoff_time)
            .bind(newer_than)
            .bind(i64::try_from(batch_size).unwrap_or(i64::MAX))
            .fetch(pool);
        let mut rows = Vec::new();
        while let Some(row) = stream.try_next().await.map_err(postgres_error)? {
            rows.push(UsageBodyCleanupRow {
                id: row.try_get::<String, _>("id").map_err(postgres_error)?,
                request_id: row
                    .try_get::<String, _>("request_id")
                    .map_err(postgres_error)?,
            });
        }
        if rows.is_empty() {
            break;
        }
        let ids = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
        let request_ids = rows
            .iter()
            .map(|row| row.request_id.clone())
            .collect::<Vec<_>>();

        let cleaned = sqlx::query(CLEAR_USAGE_HEADER_FIELDS_SQL)
            .bind(ids)
            .execute(pool)
            .await
            .map_err(postgres_error)?
            .rows_affected();
        sqlx::query(CLEAR_USAGE_HTTP_AUDIT_HEADERS_SQL)
            .bind(&request_ids)
            .execute(pool)
            .await
            .map_err(postgres_error)?;
        sqlx::query(DELETE_EMPTY_USAGE_HTTP_AUDITS_SQL)
            .bind(request_ids)
            .execute(pool)
            .await
            .map_err(postgres_error)?;
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
        let mut stream = sqlx::query(SELECT_USAGE_STALE_BODY_BATCH_SQL)
            .bind(cutoff_time)
            .bind(newer_than)
            .bind(i64::try_from(batch_size).unwrap_or(i64::MAX))
            .fetch(pool);
        let mut rows = Vec::new();
        while let Some(row) = stream.try_next().await.map_err(postgres_error)? {
            rows.push(UsageBodyCleanupRow {
                id: row.try_get::<String, _>("id").map_err(postgres_error)?,
                request_id: row
                    .try_get::<String, _>("request_id")
                    .map_err(postgres_error)?,
            });
        }
        if rows.is_empty() {
            break;
        }
        let ids = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
        let request_ids = rows
            .iter()
            .map(|row| row.request_id.clone())
            .collect::<Vec<_>>();

        let cleaned = sqlx::query(CLEAR_USAGE_BODY_FIELDS_SQL)
            .bind(ids)
            .execute(pool)
            .await
            .map_err(postgres_error)?
            .rows_affected();
        sqlx::query(DELETE_USAGE_BODY_BLOBS_SQL)
            .bind(&request_ids)
            .execute(pool)
            .await
            .map_err(postgres_error)?;
        sqlx::query(CLEAR_USAGE_HTTP_AUDIT_BODY_REFS_SQL)
            .bind(&request_ids)
            .execute(pool)
            .await
            .map_err(postgres_error)?;
        sqlx::query(DELETE_EMPTY_USAGE_HTTP_AUDITS_SQL)
            .bind(request_ids)
            .execute(pool)
            .await
            .map_err(postgres_error)?;
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
        let mut stream = sqlx::query(SELECT_USAGE_BODY_COMPRESSION_BATCH_SQL)
            .bind(cutoff_time)
            .bind(newer_than)
            .bind(i64::try_from(batch_size).unwrap_or(i64::MAX))
            .fetch(pool);
        let mut ids = Vec::new();
        while let Some(row) = stream.try_next().await.map_err(postgres_error)? {
            ids.push(row.try_get::<String, _>("id").map_err(postgres_error)?);
        }
        if ids.is_empty() {
            break;
        }

        let mut batch_success = 0usize;
        for id in ids {
            let row = sqlx::query(SELECT_USAGE_BODY_COMPRESSION_ROW_SQL)
                .bind(&id)
                .fetch_optional(pool)
                .await
                .map_err(postgres_error)?;
            let Some(row) = row else {
                continue;
            };
            let row = UsageBodyCompressionRow {
                id: row.try_get::<String, _>("id").map_err(postgres_error)?,
                request_id: row
                    .try_get::<String, _>("request_id")
                    .map_err(postgres_error)?,
                request_body: row
                    .try_get::<Option<Value>, _>("request_body")
                    .map_err(postgres_error)?,
                request_body_compressed: row
                    .try_get::<Option<Vec<u8>>, _>("request_body_compressed")
                    .map_err(postgres_error)?,
                response_body: row
                    .try_get::<Option<Value>, _>("response_body")
                    .map_err(postgres_error)?,
                response_body_compressed: row
                    .try_get::<Option<Vec<u8>>, _>("response_body_compressed")
                    .map_err(postgres_error)?,
                provider_request_body: row
                    .try_get::<Option<Value>, _>("provider_request_body")
                    .map_err(postgres_error)?,
                provider_request_body_compressed: row
                    .try_get::<Option<Vec<u8>>, _>("provider_request_body_compressed")
                    .map_err(postgres_error)?,
                client_response_body: row
                    .try_get::<Option<Value>, _>("client_response_body")
                    .map_err(postgres_error)?,
                client_response_body_compressed: row
                    .try_get::<Option<Vec<u8>>, _>("client_response_body_compressed")
                    .map_err(postgres_error)?,
            };
            let detached = build_usage_body_externalization(&row)?;
            if detached.refs.any_present() {
                let mut tx = pool.begin().await.map_err(postgres_error)?;
                for blob in &detached.blobs {
                    sqlx::query(UPSERT_USAGE_BODY_BLOB_SQL)
                        .bind(&blob.body_ref)
                        .bind(&row.request_id)
                        .bind(blob.body_field)
                        .bind(&blob.payload_gzip)
                        .execute(&mut *tx)
                        .await
                        .map_err(postgres_error)?;
                }
                sqlx::query(UPSERT_USAGE_HTTP_AUDIT_BODY_REFS_SQL)
                    .bind(&row.request_id)
                    .bind(detached.refs.request_body_ref.as_deref())
                    .bind(detached.refs.provider_request_body_ref.as_deref())
                    .bind(detached.refs.response_body_ref.as_deref())
                    .bind(detached.refs.client_response_body_ref.as_deref())
                    .bind("ref_backed")
                    .execute(&mut *tx)
                    .await
                    .map_err(postgres_error)?;
                let updated = sqlx::query(UPDATE_USAGE_BODY_COMPRESSION_SQL)
                    .bind(&row.id)
                    .execute(&mut *tx)
                    .await
                    .map_err(postgres_error)?
                    .rows_affected();
                tx.commit().await.map_err(postgres_error)?;
                if updated > 0 {
                    batch_success += 1;
                }
                continue;
            }

            let updated = sqlx::query(UPDATE_USAGE_BODY_COMPRESSION_SQL)
                .bind(&row.id)
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

fn compress_usage_json_value(value: &Value) -> Result<Vec<u8>, DataLayerError> {
    let bytes = serde_json::to_vec(value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to serialize usage json for gzip: {err}"))
    })?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(&bytes).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to gzip usage json: {err}"))
    })?;
    encoder.finish().map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to finish gzipped usage json: {err}"))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UsageDetachedBodyBlobWrite {
    body_ref: String,
    body_field: &'static str,
    payload_gzip: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageDetachedBodyRefs {
    request_body_ref: Option<String>,
    provider_request_body_ref: Option<String>,
    response_body_ref: Option<String>,
    client_response_body_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct UsageLegacyBodyRefMetadataRow {
    id: String,
    request_id: String,
    request_metadata: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageLegacyBodyRefMigrationPlan {
    refs: UsageDetachedBodyRefs,
    request_metadata: Option<Value>,
}

impl UsageDetachedBodyRefs {
    fn any_present(&self) -> bool {
        self.request_body_ref.is_some()
            || self.provider_request_body_ref.is_some()
            || self.response_body_ref.is_some()
            || self.client_response_body_ref.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageBodyExternalizationPlan {
    blobs: Vec<UsageDetachedBodyBlobWrite>,
    refs: UsageDetachedBodyRefs,
}

fn migrate_legacy_body_ref_metadata_plan(
    request_id: &str,
    request_metadata: Option<Value>,
) -> Option<UsageLegacyBodyRefMigrationPlan> {
    let mut metadata = match request_metadata {
        Some(Value::Object(object)) => object,
        _ => return None,
    };

    let mut refs = UsageDetachedBodyRefs::default();
    let mut removed_any = false;
    for field in [
        UsageBodyField::RequestBody,
        UsageBodyField::ProviderRequestBody,
        UsageBodyField::ResponseBody,
        UsageBodyField::ClientResponseBody,
    ] {
        let key = field.as_ref_key();
        let Some(value) = metadata.remove(key) else {
            continue;
        };
        removed_any = true;
        let parsed = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(parse_usage_body_ref)
            .filter(|(parsed_request_id, parsed_field)| {
                parsed_request_id == request_id && *parsed_field == field
            })
            .map(|(parsed_request_id, parsed_field)| {
                usage_body_ref(&parsed_request_id, parsed_field)
            });
        match field {
            UsageBodyField::RequestBody => refs.request_body_ref = parsed,
            UsageBodyField::ProviderRequestBody => refs.provider_request_body_ref = parsed,
            UsageBodyField::ResponseBody => refs.response_body_ref = parsed,
            UsageBodyField::ClientResponseBody => refs.client_response_body_ref = parsed,
        }
    }

    if !removed_any {
        return None;
    }

    Some(UsageLegacyBodyRefMigrationPlan {
        refs,
        request_metadata: (!metadata.is_empty()).then_some(Value::Object(metadata)),
    })
}

fn build_usage_body_externalization(
    row: &UsageBodyCompressionRow,
) -> Result<UsageBodyExternalizationPlan, DataLayerError> {
    let mut plan = UsageBodyExternalizationPlan::default();
    maybe_externalize_usage_body_field(
        &mut plan,
        &row.request_id,
        UsageBodyField::RequestBody,
        row.request_body.as_ref(),
        row.request_body_compressed.as_deref(),
    )?;
    maybe_externalize_usage_body_field(
        &mut plan,
        &row.request_id,
        UsageBodyField::ProviderRequestBody,
        row.provider_request_body.as_ref(),
        row.provider_request_body_compressed.as_deref(),
    )?;
    maybe_externalize_usage_body_field(
        &mut plan,
        &row.request_id,
        UsageBodyField::ResponseBody,
        row.response_body.as_ref(),
        row.response_body_compressed.as_deref(),
    )?;
    maybe_externalize_usage_body_field(
        &mut plan,
        &row.request_id,
        UsageBodyField::ClientResponseBody,
        row.client_response_body.as_ref(),
        row.client_response_body_compressed.as_deref(),
    )?;
    Ok(plan)
}

fn maybe_externalize_usage_body_field(
    plan: &mut UsageBodyExternalizationPlan,
    request_id: &str,
    field: UsageBodyField,
    inline_body: Option<&Value>,
    compressed_body: Option<&[u8]>,
) -> Result<(), DataLayerError> {
    let Some(payload_gzip) = (match inline_body {
        Some(value) => Some(compress_usage_json_value(value)?),
        None => compressed_body.map(|value| value.to_vec()),
    }) else {
        return Ok(());
    };
    let body_ref = usage_body_ref(request_id, field);
    plan.blobs.push(UsageDetachedBodyBlobWrite {
        body_ref: body_ref.clone(),
        body_field: field.as_storage_field(),
        payload_gzip,
    });
    match field {
        UsageBodyField::RequestBody => plan.refs.request_body_ref = Some(body_ref),
        UsageBodyField::ProviderRequestBody => plan.refs.provider_request_body_ref = Some(body_ref),
        UsageBodyField::ResponseBody => plan.refs.response_body_ref = Some(body_ref),
        UsageBodyField::ClientResponseBody => plan.refs.client_response_body_ref = Some(body_ref),
    }
    Ok(())
}

async fn cleanup_expired_api_keys(
    pool: &aether_data::postgres::PostgresPool,
    auto_delete_expired_keys: bool,
) -> Result<usize, DataLayerError> {
    let mut expired_keys = sqlx::query(SELECT_EXPIRED_ACTIVE_API_KEYS_SQL).fetch(pool);
    let mut cleaned = 0usize;
    while let Some(row) = expired_keys.try_next().await.map_err(postgres_error)? {
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

#[cfg(test)]
mod tests {
    use std::io::Read;

    use flate2::read::GzDecoder;
    use serde_json::json;

    use super::{
        build_usage_body_externalization, compress_usage_json_value,
        migrate_legacy_body_ref_metadata_plan, UsageBodyCompressionRow,
    };

    fn inflate_json(bytes: &[u8]) -> serde_json::Value {
        let mut decoder = GzDecoder::new(bytes);
        let mut decoded = Vec::new();
        decoder
            .read_to_end(&mut decoded)
            .expect("gzip should decode");
        serde_json::from_slice(&decoded).expect("json should decode")
    }

    #[test]
    fn usage_body_externalization_moves_inline_json_into_ref_backed_blobs() {
        let row = UsageBodyCompressionRow {
            id: "usage-1".to_string(),
            request_id: "req-1".to_string(),
            request_body: Some(json!({"hello": "world"})),
            request_body_compressed: None,
            response_body: None,
            response_body_compressed: None,
            provider_request_body: Some(json!({"provider": true})),
            provider_request_body_compressed: None,
            client_response_body: None,
            client_response_body_compressed: None,
        };

        let plan = build_usage_body_externalization(&row).expect("plan should build");

        assert_eq!(plan.blobs.len(), 2);
        assert_eq!(
            plan.refs.request_body_ref.as_deref(),
            Some("usage://request/req-1/request_body")
        );
        assert_eq!(
            plan.refs.provider_request_body_ref.as_deref(),
            Some("usage://request/req-1/provider_request_body")
        );
        assert_eq!(
            inflate_json(&plan.blobs[0].payload_gzip),
            json!({"hello": "world"})
        );
        assert_eq!(
            inflate_json(&plan.blobs[1].payload_gzip),
            json!({"provider": true})
        );
    }

    #[test]
    fn usage_body_externalization_reuses_existing_compressed_payloads() {
        let compressed = compress_usage_json_value(&json!({"legacy": true}))
            .expect("compressed payload should build");
        let row = UsageBodyCompressionRow {
            id: "usage-1".to_string(),
            request_id: "req-legacy".to_string(),
            request_body: None,
            request_body_compressed: Some(compressed.clone()),
            response_body: None,
            response_body_compressed: None,
            provider_request_body: None,
            provider_request_body_compressed: None,
            client_response_body: None,
            client_response_body_compressed: None,
        };

        let plan = build_usage_body_externalization(&row).expect("plan should build");

        assert_eq!(plan.blobs.len(), 1);
        assert_eq!(plan.blobs[0].payload_gzip, compressed);
        assert_eq!(
            plan.refs.request_body_ref.as_deref(),
            Some("usage://request/req-legacy/request_body")
        );
    }

    #[test]
    fn legacy_body_ref_metadata_migration_moves_matching_refs_and_strips_keys() {
        let plan = migrate_legacy_body_ref_metadata_plan(
            "req-1",
            Some(json!({
                "trace_id": "trace-1",
                "request_body_ref": "usage://request/req-1/request_body",
                "response_body_ref": "usage://request/req-1/response_body"
            })),
        )
        .expect("migration plan should exist");

        assert_eq!(
            plan.refs.request_body_ref.as_deref(),
            Some("usage://request/req-1/request_body")
        );
        assert_eq!(
            plan.refs.response_body_ref.as_deref(),
            Some("usage://request/req-1/response_body")
        );
        assert_eq!(
            plan.request_metadata,
            Some(json!({
                "trace_id": "trace-1"
            }))
        );
    }

    #[test]
    fn legacy_body_ref_metadata_migration_strips_invalid_and_cross_request_refs() {
        let plan = migrate_legacy_body_ref_metadata_plan(
            "req-1",
            Some(json!({
                "request_body_ref": "blob://legacy-request",
                "provider_request_body_ref": "usage://request/req-other/provider_request_body",
                "candidate_index": 2
            })),
        )
        .expect("migration plan should exist");

        assert!(!plan.refs.any_present());
        assert_eq!(
            plan.request_metadata,
            Some(json!({
                "candidate_index": 2
            }))
        );
    }
}
