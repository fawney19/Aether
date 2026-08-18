use std::collections::BTreeMap;

use aether_data_contracts::repository::usage::{
    api_key_usage_contribution, model_usage_contribution, provider_api_key_usage_contribution,
    ApiKeyLastUsedDelta, ApiKeyUsageDelta, ManagementTokenCounterDelta, ModelUsageDelta,
    ProviderApiKeyUsageDelta, ProxyNodeCounterDelta, StoredRequestUsageAudit,
    UsageCounterFlushSummary, UsageCounterHealthSnapshot, UsageCounterPendingHealthSnapshot,
};
use aether_data_contracts::DataLayerError;
use sqlx::{MySql, MySqlPool, QueryBuilder, Row};

use crate::error::SqlResultExt;

const KIND_API_KEY: &str = "api_key";
const KIND_PROVIDER_API_KEY: &str = "provider_api_key";
const KIND_MODEL: &str = "model";
const KIND_PROVIDER_MONTHLY: &str = "provider_monthly";
const KIND_PROXY_NODE: &str = "proxy_node";
const KIND_MANAGEMENT_TOKEN: &str = "management_token";
const KIND_API_KEY_LAST_USED: &str = "api_key_last_used";

const CLAIM_SQL: &str = r#"
SELECT
  id,
  kind,
  target_id,
  request_count_delta,
  total_requests_delta,
  success_count_delta,
  sla_eligible_count_delta,
  error_count_delta,
  user_error_count_delta,
  dns_failures_delta,
  stream_errors_delta,
  total_tokens_delta,
  total_cost_usd_delta,
  total_response_time_ms_delta,
  last_used_at_unix_secs,
  last_used_ip,
  candidate_last_used_at_unix_secs,
  removed_last_used_at_unix_secs,
  usage_created_at_unix_secs
FROM usage_counter_deltas
WHERE processed_at IS NULL
ORDER BY created_at ASC, id ASC
LIMIT ?
FOR UPDATE SKIP LOCKED
"#;

struct DeltaRow {
    id: String,
    kind: String,
    target_id: String,
    request_count_delta: i64,
    total_requests_delta: i64,
    success_count_delta: i64,
    sla_eligible_count_delta: i64,
    error_count_delta: i64,
    user_error_count_delta: i64,
    dns_failures_delta: i64,
    stream_errors_delta: i64,
    total_tokens_delta: i64,
    total_cost_usd_delta: f64,
    total_response_time_ms_delta: i64,
    last_used_at_unix_secs: Option<u64>,
    last_used_ip: Option<String>,
    candidate_last_used_at_unix_secs: Option<u64>,
    removed_last_used_at_unix_secs: Option<u64>,
    usage_created_at_unix_secs: Option<u64>,
}

#[derive(Default)]
struct Aggregates {
    api_keys: BTreeMap<String, ApiKeyUsageDelta>,
    provider_api_keys: BTreeMap<String, ProviderApiKeyUsageDelta>,
    models: BTreeMap<String, ModelUsageDelta>,
    provider_monthly: BTreeMap<String, f64>,
    proxy_nodes: BTreeMap<String, ProxyNodeCounterDelta>,
    management_tokens: BTreeMap<String, ManagementTokenCounterDelta>,
    api_key_last_used: BTreeMap<String, ApiKeyLastUsedDelta>,
}

impl Aggregates {
    fn from_rows(rows: &[DeltaRow]) -> Result<Self, DataLayerError> {
        let mut aggregates = Self::default();
        for row in rows {
            if !row.total_cost_usd_delta.is_finite() {
                return Err(DataLayerError::UnexpectedValue(format!(
                    "usage_counter_deltas.total_cost_usd_delta is not finite for {}",
                    row.id
                )));
            }
            match row.kind.as_str() {
                KIND_API_KEY => {
                    let entry = aggregates
                        .api_keys
                        .entry(row.target_id.clone())
                        .or_default();
                    entry.total_requests += row.total_requests_delta;
                    entry.total_tokens += row.total_tokens_delta;
                    entry.total_cost_usd += row.total_cost_usd_delta;
                    merge_optional_max(
                        &mut entry.candidate_last_used_at_unix_secs,
                        row.candidate_last_used_at_unix_secs,
                    );
                    merge_optional_max(
                        &mut entry.removed_last_used_at_unix_secs,
                        row.removed_last_used_at_unix_secs,
                    );
                }
                KIND_PROVIDER_API_KEY => {
                    let entry = aggregates
                        .provider_api_keys
                        .entry(row.target_id.clone())
                        .or_default();
                    entry.request_count += row.request_count_delta;
                    entry.success_count += row.success_count_delta;
                    entry.sla_eligible_count += row.sla_eligible_count_delta;
                    entry.error_count += row.error_count_delta;
                    entry.user_error_count += row.user_error_count_delta;
                    entry.total_tokens += row.total_tokens_delta;
                    entry.total_cost_usd += row.total_cost_usd_delta;
                    entry.total_response_time_ms += row.total_response_time_ms_delta;
                    merge_optional_max(
                        &mut entry.candidate_last_used_at_unix_secs,
                        row.candidate_last_used_at_unix_secs,
                    );
                    merge_optional_max(
                        &mut entry.removed_last_used_at_unix_secs,
                        row.removed_last_used_at_unix_secs,
                    );
                    merge_optional_max(
                        &mut entry.usage_created_at_unix_secs,
                        row.usage_created_at_unix_secs,
                    );
                }
                KIND_MODEL => {
                    aggregates
                        .models
                        .entry(row.target_id.clone())
                        .or_default()
                        .request_count += row.request_count_delta;
                }
                KIND_PROVIDER_MONTHLY => {
                    *aggregates
                        .provider_monthly
                        .entry(row.target_id.clone())
                        .or_default() += row.total_cost_usd_delta;
                }
                KIND_PROXY_NODE => {
                    let entry = aggregates
                        .proxy_nodes
                        .entry(row.target_id.clone())
                        .or_insert(ProxyNodeCounterDelta {
                            node_id: row.target_id.clone(),
                            total_requests_delta: 0,
                            failed_requests_delta: 0,
                            dns_failures_delta: 0,
                            stream_errors_delta: 0,
                        });
                    entry.total_requests_delta += row.total_requests_delta;
                    entry.failed_requests_delta += row.error_count_delta;
                    entry.dns_failures_delta += row.dns_failures_delta;
                    entry.stream_errors_delta += row.stream_errors_delta;
                }
                KIND_MANAGEMENT_TOKEN => {
                    let entry = aggregates
                        .management_tokens
                        .entry(row.target_id.clone())
                        .or_insert(ManagementTokenCounterDelta {
                            token_id: row.target_id.clone(),
                            usage_count_delta: 0,
                            last_used_at_unix_secs: None,
                            last_used_ip: None,
                        });
                    entry.usage_count_delta += row.request_count_delta;
                    merge_latest_timestamp_with_value(
                        &mut entry.last_used_at_unix_secs,
                        &mut entry.last_used_ip,
                        row.last_used_at_unix_secs,
                        row.last_used_ip.clone(),
                    );
                }
                KIND_API_KEY_LAST_USED => {
                    let Some(last_used_at_unix_secs) = row.last_used_at_unix_secs else {
                        continue;
                    };
                    let entry = aggregates
                        .api_key_last_used
                        .entry(row.target_id.clone())
                        .or_insert(ApiKeyLastUsedDelta {
                            api_key_id: row.target_id.clone(),
                            last_used_at_unix_secs,
                        });
                    if last_used_at_unix_secs > entry.last_used_at_unix_secs {
                        entry.last_used_at_unix_secs = last_used_at_unix_secs;
                    }
                }
                other => {
                    return Err(DataLayerError::UnexpectedValue(format!(
                        "unknown usage counter delta kind: {other}"
                    )));
                }
            }
        }
        Ok(aggregates)
    }
}

pub(super) async fn flush(
    pool: &MySqlPool,
    batch_size: usize,
) -> Result<UsageCounterFlushSummary, DataLayerError> {
    if batch_size == 0 {
        return Ok(UsageCounterFlushSummary::default());
    }
    let limit = i64::try_from(batch_size).map_err(|_| {
        DataLayerError::InvalidInput(format!(
            "usage counter flush batch size is out of range: {batch_size}"
        ))
    })?;

    let mut tx = pool.begin().await.map_sql_err()?;
    let rows = sqlx::query(CLAIM_SQL)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_sql_err()?
        .iter()
        .map(map_row)
        .collect::<Result<Vec<_>, _>>()?;
    if rows.is_empty() {
        tx.rollback().await.map_sql_err()?;
        return Ok(UsageCounterFlushSummary::default());
    }

    let aggregates = Aggregates::from_rows(&rows)?;
    for (target_id, delta) in &aggregates.api_keys {
        apply_api_key(&mut tx, target_id, delta).await?;
    }
    for (target_id, delta) in &aggregates.models {
        apply_model(&mut tx, target_id, delta).await?;
    }
    for (target_id, delta) in &aggregates.provider_api_keys {
        apply_provider_api_key(&mut tx, target_id, delta).await?;
    }
    for (target_id, delta) in &aggregates.provider_monthly {
        apply_provider_monthly(&mut tx, target_id, *delta).await?;
    }
    for (target_id, delta) in &aggregates.proxy_nodes {
        apply_proxy_node(&mut tx, target_id, delta).await?;
    }
    for (target_id, delta) in &aggregates.management_tokens {
        apply_management_token(&mut tx, target_id, delta).await?;
    }
    for (target_id, delta) in &aggregates.api_key_last_used {
        apply_api_key_last_used(&mut tx, target_id, delta).await?;
    }

    let now = current_unix_secs();
    let mut mark = QueryBuilder::<MySql>::new("UPDATE usage_counter_deltas SET processed_at = ");
    mark.push_bind(now).push(" WHERE id IN (");
    {
        let mut ids = mark.separated(", ");
        for row in &rows {
            ids.push_bind(&row.id);
        }
    }
    mark.push(")");
    mark.build().execute(&mut *tx).await.map_sql_err()?;
    tx.commit().await.map_sql_err()?;

    Ok(UsageCounterFlushSummary {
        rows_claimed: rows.len(),
        api_key_targets: aggregates.api_keys.len(),
        provider_api_key_targets: aggregates.provider_api_keys.len(),
        model_targets: aggregates.models.len(),
        provider_monthly_targets: aggregates.provider_monthly.len(),
        proxy_node_targets: aggregates.proxy_nodes.len(),
        management_token_targets: aggregates.management_tokens.len(),
        api_key_last_used_targets: aggregates.api_key_last_used.len(),
    })
}

pub(super) async fn enqueue_proxy_node(
    pool: &MySqlPool,
    delta: ProxyNodeCounterDelta,
) -> Result<bool, DataLayerError> {
    if delta.is_noop() {
        return Ok(false);
    }
    let node_id = delta.node_id.trim().to_string();
    let request_id = format!("proxy_node:{node_id}:{}", uuid::Uuid::new_v4());
    let mut tx = pool.begin().await.map_sql_err()?;
    insert_delta(
        &mut tx,
        DeltaInsert {
            request_id: &request_id,
            kind: KIND_PROXY_NODE,
            target_id: &node_id,
            total_requests_delta: delta.total_requests_delta,
            error_count_delta: delta.failed_requests_delta,
            dns_failures_delta: delta.dns_failures_delta,
            stream_errors_delta: delta.stream_errors_delta,
            ..DeltaInsert::default()
        },
    )
    .await?;
    tx.commit().await.map_sql_err()?;
    Ok(true)
}

pub(super) async fn enqueue_management_token(
    pool: &MySqlPool,
    delta: ManagementTokenCounterDelta,
) -> Result<bool, DataLayerError> {
    if delta.is_noop() {
        return Ok(false);
    }
    let token_id = delta.token_id.trim().to_string();
    let last_used_ip = delta
        .last_used_ip
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let last_used_at = delta
        .last_used_at_unix_secs
        .unwrap_or_else(|| current_unix_secs().max(0) as u64);
    let request_id = format!("management_token:{token_id}:{}", uuid::Uuid::new_v4());
    let mut tx = pool.begin().await.map_sql_err()?;
    insert_delta(
        &mut tx,
        DeltaInsert {
            request_id: &request_id,
            kind: KIND_MANAGEMENT_TOKEN,
            target_id: &token_id,
            request_count_delta: delta.usage_count_delta,
            last_used_at_unix_secs: Some(last_used_at),
            last_used_ip: last_used_ip.as_deref(),
            ..DeltaInsert::default()
        },
    )
    .await?;
    tx.commit().await.map_sql_err()?;
    Ok(true)
}

pub(super) async fn enqueue_api_key_last_used(
    pool: &MySqlPool,
    delta: ApiKeyLastUsedDelta,
) -> Result<bool, DataLayerError> {
    if delta.is_noop() {
        return Ok(false);
    }
    let api_key_id = delta.api_key_id.trim().to_string();
    let request_id = format!("api_key_last_used:{api_key_id}:{}", uuid::Uuid::new_v4());
    let mut tx = pool.begin().await.map_sql_err()?;
    insert_delta(
        &mut tx,
        DeltaInsert {
            request_id: &request_id,
            kind: KIND_API_KEY_LAST_USED,
            target_id: &api_key_id,
            last_used_at_unix_secs: Some(delta.last_used_at_unix_secs),
            ..DeltaInsert::default()
        },
    )
    .await?;
    tx.commit().await.map_sql_err()?;
    Ok(true)
}

pub(super) async fn cleanup_processed(
    pool: &MySqlPool,
    cutoff_unix_secs: u64,
    batch_size: usize,
) -> Result<usize, DataLayerError> {
    if batch_size == 0 {
        return Ok(0);
    }
    let cutoff = to_i64(cutoff_unix_secs, "usage counter cleanup cutoff")?;
    let limit = i64::try_from(batch_size).map_err(|_| {
        DataLayerError::InvalidInput(format!(
            "usage counter cleanup batch size is out of range: {batch_size}"
        ))
    })?;
    let deleted = sqlx::query(
        r#"
DELETE FROM usage_counter_deltas
WHERE id IN (
  SELECT id FROM (
    SELECT id
    FROM usage_counter_deltas
    WHERE processed_at IS NOT NULL AND processed_at < ?
    ORDER BY processed_at ASC, created_at ASC, id ASC
    LIMIT ?
  ) AS doomed
)
"#,
    )
    .bind(cutoff)
    .bind(limit)
    .execute(pool)
    .await
    .map_sql_err()?
    .rows_affected();
    Ok(usize::try_from(deleted).unwrap_or(usize::MAX))
}

pub(super) async fn read_health(
    pool: &MySqlPool,
) -> Result<UsageCounterHealthSnapshot, DataLayerError> {
    let row = sqlx::query(
        r#"
SELECT
  (SELECT CAST(COUNT(*) AS SIGNED) FROM usage_counter_deltas WHERE processed_at IS NULL)
    AS pending_rows,
  (SELECT CAST(COUNT(*) AS SIGNED) FROM usage_counter_deltas WHERE processed_at IS NOT NULL)
    AS processed_rows,
  (SELECT MIN(created_at) FROM usage_counter_deltas WHERE processed_at IS NULL)
    AS oldest_pending_created_at_unix_secs,
  (SELECT MAX(processed_at) FROM usage_counter_deltas WHERE processed_at IS NOT NULL)
    AS latest_processed_at_unix_secs
"#,
    )
    .fetch_one(pool)
    .await
    .map_sql_err()?;
    let mut snapshot = UsageCounterHealthSnapshot {
        pending_rows: nonnegative_u64(row.try_get("pending_rows").map_sql_err()?),
        processed_rows: nonnegative_u64(row.try_get("processed_rows").map_sql_err()?),
        oldest_pending_created_at_unix_secs: optional_nonnegative_u64(
            row.try_get("oldest_pending_created_at_unix_secs")
                .map_sql_err()?,
        ),
        latest_processed_at_unix_secs: optional_nonnegative_u64(
            row.try_get("latest_processed_at_unix_secs").map_sql_err()?,
        ),
        pending_by_kind: BTreeMap::new(),
    };
    for row in pending_health_rows(pool).await? {
        snapshot.pending_by_kind.insert(row.0, row.1);
    }
    Ok(snapshot)
}

pub(super) async fn read_pending_health(
    pool: &MySqlPool,
) -> Result<UsageCounterPendingHealthSnapshot, DataLayerError> {
    let mut snapshot = UsageCounterPendingHealthSnapshot::default();
    for (kind, pending_rows, oldest) in pending_health_rows(pool).await? {
        snapshot.pending_rows = snapshot.pending_rows.saturating_add(pending_rows);
        if let Some(oldest) = oldest {
            snapshot.oldest_pending_created_at_unix_secs = Some(
                snapshot
                    .oldest_pending_created_at_unix_secs
                    .map_or(oldest, |current| current.min(oldest)),
            );
        }
        snapshot.pending_by_kind.insert(kind, pending_rows);
    }
    Ok(snapshot)
}

async fn pending_health_rows(
    pool: &MySqlPool,
) -> Result<Vec<(String, u64, Option<u64>)>, DataLayerError> {
    let rows = sqlx::query(
        r#"
SELECT
  kind,
  CAST(COUNT(*) AS SIGNED) AS pending_rows,
  MIN(created_at) AS oldest_pending_created_at_unix_secs
FROM usage_counter_deltas
WHERE processed_at IS NULL
GROUP BY kind
ORDER BY kind ASC
"#,
    )
    .fetch_all(pool)
    .await
    .map_sql_err()?;
    rows.iter()
        .map(|row| {
            Ok((
                row.try_get("kind").map_sql_err()?,
                nonnegative_u64(row.try_get("pending_rows").map_sql_err()?),
                optional_nonnegative_u64(
                    row.try_get("oldest_pending_created_at_unix_secs")
                        .map_sql_err()?,
                ),
            ))
        })
        .collect()
}

pub(super) async fn enqueue_usage_transition(
    tx: &mut sqlx::Transaction<'_, MySql>,
    request_id: &str,
    before: Option<&StoredRequestUsageAudit>,
    after: &StoredRequestUsageAudit,
) -> Result<(), DataLayerError> {
    let before_api_key = before.and_then(api_key_usage_contribution);
    let after_api_key = api_key_usage_contribution(after);
    match (before_api_key.as_ref(), after_api_key.as_ref()) {
        (Some(before), Some(after)) if before.api_key_id == after.api_key_id => {
            enqueue_api_key_delta(
                tx,
                request_id,
                &before.api_key_id,
                &ApiKeyUsageDelta::between(before, after),
            )
            .await?;
        }
        _ => {
            if let Some(before) = before_api_key.as_ref() {
                enqueue_api_key_delta(
                    tx,
                    request_id,
                    &before.api_key_id,
                    &ApiKeyUsageDelta::removal(before),
                )
                .await?;
            }
            if let Some(after) = after_api_key.as_ref() {
                enqueue_api_key_delta(
                    tx,
                    request_id,
                    &after.api_key_id,
                    &ApiKeyUsageDelta::addition(after),
                )
                .await?;
            }
        }
    }

    let before_model = before.and_then(model_usage_contribution);
    let after_model = model_usage_contribution(after);
    match (before_model.as_ref(), after_model.as_ref()) {
        (Some(before), Some(after)) if before.model == after.model => {
            enqueue_model_delta(
                tx,
                request_id,
                &before.model,
                &ModelUsageDelta::between(before, after),
            )
            .await?;
        }
        _ => {
            if let Some(before) = before_model.as_ref() {
                enqueue_model_delta(
                    tx,
                    request_id,
                    &before.model,
                    &ModelUsageDelta::removal(before),
                )
                .await?;
            }
            if let Some(after) = after_model.as_ref() {
                enqueue_model_delta(
                    tx,
                    request_id,
                    &after.model,
                    &ModelUsageDelta::addition(after),
                )
                .await?;
            }
        }
    }

    let before_provider = before.and_then(provider_api_key_usage_contribution);
    let after_provider = provider_api_key_usage_contribution(after);
    match (before_provider.as_ref(), after_provider.as_ref()) {
        (Some(before), Some(after)) if before.key_id == after.key_id => {
            enqueue_provider_api_key_delta(
                tx,
                request_id,
                &before.key_id,
                &ProviderApiKeyUsageDelta::between(before, after),
            )
            .await?;
        }
        _ => {
            if let Some(before) = before_provider.as_ref() {
                enqueue_provider_api_key_delta(
                    tx,
                    request_id,
                    &before.key_id,
                    &ProviderApiKeyUsageDelta::removal(before),
                )
                .await?;
            }
            if let Some(after) = after_provider.as_ref() {
                enqueue_provider_api_key_delta(
                    tx,
                    request_id,
                    &after.key_id,
                    &ProviderApiKeyUsageDelta::addition(after),
                )
                .await?;
            }
        }
    }
    Ok(())
}

pub(super) async fn enqueue_usage_transition_for_request(
    tx: &mut sqlx::Transaction<'_, MySql>,
    request_id: &str,
    before: Option<&StoredRequestUsageAudit>,
) -> Result<(), DataLayerError> {
    let row = sqlx::query(&format!(
        "{} WHERE `usage`.request_id = ? LIMIT 1",
        super::USAGE_COLUMNS
    ))
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_sql_err()?
    .ok_or_else(|| {
        DataLayerError::UnexpectedValue(format!(
            "usage row missing while preparing counter delta: {request_id}"
        ))
    })?;
    let after = super::map_usage_row(&row, false)?;
    enqueue_usage_transition(tx, request_id, before, &after).await
}

pub(super) async fn lock_and_load_usage(
    tx: &mut sqlx::Transaction<'_, MySql>,
    request_id: &str,
) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
    let exists = sqlx::query_scalar::<_, String>(
        "SELECT request_id FROM `usage` WHERE request_id = ? FOR UPDATE",
    )
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_sql_err()?;
    if exists.is_none() {
        return Ok(None);
    }
    let row = sqlx::query(&format!(
        "{} WHERE `usage`.request_id = ? LIMIT 1",
        super::USAGE_COLUMNS
    ))
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_sql_err()?;
    row.as_ref()
        .map(|row| super::map_usage_row(row, false))
        .transpose()
}

async fn enqueue_api_key_delta(
    tx: &mut sqlx::Transaction<'_, MySql>,
    request_id: &str,
    target_id: &str,
    delta: &ApiKeyUsageDelta,
) -> Result<(), DataLayerError> {
    if target_id.trim().is_empty() || delta.is_noop() {
        return Ok(());
    }
    insert_delta(
        tx,
        DeltaInsert {
            request_id,
            kind: KIND_API_KEY,
            target_id,
            total_requests_delta: delta.total_requests,
            total_tokens_delta: delta.total_tokens,
            total_cost_usd_delta: finite_or_zero(delta.total_cost_usd),
            candidate_last_used_at_unix_secs: delta.candidate_last_used_at_unix_secs,
            removed_last_used_at_unix_secs: delta.removed_last_used_at_unix_secs,
            ..DeltaInsert::default()
        },
    )
    .await
}

async fn enqueue_model_delta(
    tx: &mut sqlx::Transaction<'_, MySql>,
    request_id: &str,
    target_id: &str,
    delta: &ModelUsageDelta,
) -> Result<(), DataLayerError> {
    if target_id.trim().is_empty() || delta.is_noop() {
        return Ok(());
    }
    insert_delta(
        tx,
        DeltaInsert {
            request_id,
            kind: KIND_MODEL,
            target_id,
            request_count_delta: delta.request_count,
            ..DeltaInsert::default()
        },
    )
    .await
}

async fn enqueue_provider_api_key_delta(
    tx: &mut sqlx::Transaction<'_, MySql>,
    request_id: &str,
    target_id: &str,
    delta: &ProviderApiKeyUsageDelta,
) -> Result<(), DataLayerError> {
    if target_id.trim().is_empty() || delta.is_noop() {
        return Ok(());
    }
    insert_delta(
        tx,
        DeltaInsert {
            request_id,
            kind: KIND_PROVIDER_API_KEY,
            target_id,
            request_count_delta: delta.request_count,
            success_count_delta: delta.success_count,
            sla_eligible_count_delta: delta.sla_eligible_count,
            error_count_delta: delta.error_count,
            user_error_count_delta: delta.user_error_count,
            total_tokens_delta: delta.total_tokens,
            total_cost_usd_delta: finite_or_zero(delta.total_cost_usd),
            total_response_time_ms_delta: delta.total_response_time_ms,
            candidate_last_used_at_unix_secs: delta.candidate_last_used_at_unix_secs,
            removed_last_used_at_unix_secs: delta.removed_last_used_at_unix_secs,
            usage_created_at_unix_secs: delta.usage_created_at_unix_secs,
            ..DeltaInsert::default()
        },
    )
    .await
}

#[derive(Default)]
struct DeltaInsert<'a> {
    request_id: &'a str,
    kind: &'a str,
    target_id: &'a str,
    request_count_delta: i64,
    total_requests_delta: i64,
    success_count_delta: i64,
    sla_eligible_count_delta: i64,
    error_count_delta: i64,
    user_error_count_delta: i64,
    dns_failures_delta: i64,
    stream_errors_delta: i64,
    total_tokens_delta: i64,
    total_cost_usd_delta: f64,
    total_response_time_ms_delta: i64,
    last_used_at_unix_secs: Option<u64>,
    last_used_ip: Option<&'a str>,
    candidate_last_used_at_unix_secs: Option<u64>,
    removed_last_used_at_unix_secs: Option<u64>,
    usage_created_at_unix_secs: Option<u64>,
}

async fn insert_delta(
    tx: &mut sqlx::Transaction<'_, MySql>,
    input: DeltaInsert<'_>,
) -> Result<(), DataLayerError> {
    let request_id = input.request_id.trim();
    let target_id = input.target_id.trim();
    if request_id.is_empty() || target_id.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
INSERT INTO usage_counter_deltas (
  id, request_id, kind, target_id, request_count_delta, total_requests_delta,
  success_count_delta, sla_eligible_count_delta, error_count_delta,
  user_error_count_delta, dns_failures_delta, stream_errors_delta,
  total_tokens_delta, total_cost_usd_delta, total_response_time_ms_delta,
  last_used_at_unix_secs, last_used_ip, candidate_last_used_at_unix_secs,
  removed_last_used_at_unix_secs, usage_created_at_unix_secs, created_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(request_id)
    .bind(input.kind)
    .bind(target_id)
    .bind(input.request_count_delta)
    .bind(input.total_requests_delta)
    .bind(input.success_count_delta)
    .bind(input.sla_eligible_count_delta)
    .bind(input.error_count_delta)
    .bind(input.user_error_count_delta)
    .bind(input.dns_failures_delta)
    .bind(input.stream_errors_delta)
    .bind(input.total_tokens_delta)
    .bind(finite_or_zero(input.total_cost_usd_delta))
    .bind(input.total_response_time_ms_delta)
    .bind(optional_to_i64(
        input.last_used_at_unix_secs,
        "usage counter last_used_at_unix_secs",
    )?)
    .bind(
        input
            .last_used_ip
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    )
    .bind(optional_to_i64(
        input.candidate_last_used_at_unix_secs,
        "usage counter candidate_last_used_at_unix_secs",
    )?)
    .bind(optional_to_i64(
        input.removed_last_used_at_unix_secs,
        "usage counter removed_last_used_at_unix_secs",
    )?)
    .bind(optional_to_i64(
        input.usage_created_at_unix_secs,
        "usage counter usage_created_at_unix_secs",
    )?)
    .bind(current_unix_secs())
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

fn map_row(row: &sqlx::mysql::MySqlRow) -> Result<DeltaRow, DataLayerError> {
    Ok(DeltaRow {
        id: row.try_get("id").map_sql_err()?,
        kind: row.try_get("kind").map_sql_err()?,
        target_id: row.try_get("target_id").map_sql_err()?,
        request_count_delta: row.try_get("request_count_delta").map_sql_err()?,
        total_requests_delta: row.try_get("total_requests_delta").map_sql_err()?,
        success_count_delta: row.try_get("success_count_delta").map_sql_err()?,
        sla_eligible_count_delta: row.try_get("sla_eligible_count_delta").map_sql_err()?,
        error_count_delta: row.try_get("error_count_delta").map_sql_err()?,
        user_error_count_delta: row.try_get("user_error_count_delta").map_sql_err()?,
        dns_failures_delta: row.try_get("dns_failures_delta").map_sql_err()?,
        stream_errors_delta: row.try_get("stream_errors_delta").map_sql_err()?,
        total_tokens_delta: row.try_get("total_tokens_delta").map_sql_err()?,
        total_cost_usd_delta: row.try_get("total_cost_usd_delta").map_sql_err()?,
        total_response_time_ms_delta: row.try_get("total_response_time_ms_delta").map_sql_err()?,
        last_used_at_unix_secs: optional_u64(
            "usage_counter_deltas.last_used_at_unix_secs",
            row.try_get("last_used_at_unix_secs").map_sql_err()?,
        )?,
        last_used_ip: row.try_get("last_used_ip").map_sql_err()?,
        candidate_last_used_at_unix_secs: optional_u64(
            "usage_counter_deltas.candidate_last_used_at_unix_secs",
            row.try_get("candidate_last_used_at_unix_secs")
                .map_sql_err()?,
        )?,
        removed_last_used_at_unix_secs: optional_u64(
            "usage_counter_deltas.removed_last_used_at_unix_secs",
            row.try_get("removed_last_used_at_unix_secs")
                .map_sql_err()?,
        )?,
        usage_created_at_unix_secs: optional_u64(
            "usage_counter_deltas.usage_created_at_unix_secs",
            row.try_get("usage_created_at_unix_secs").map_sql_err()?,
        )?,
    })
}

async fn apply_api_key(
    tx: &mut sqlx::Transaction<'_, MySql>,
    target_id: &str,
    delta: &ApiKeyUsageDelta,
) -> Result<(), DataLayerError> {
    if target_id.trim().is_empty() || delta.is_noop() {
        return Ok(());
    }
    let candidate = optional_to_i64(
        delta.candidate_last_used_at_unix_secs,
        "api key candidate last used at",
    )?;
    let removed = optional_to_i64(
        delta.removed_last_used_at_unix_secs,
        "api key removed last used at",
    )?;
    sqlx::query(
        r#"
UPDATE api_keys
SET total_requests = GREATEST(COALESCE(total_requests, 0) + ?, 0),
    total_tokens = GREATEST(COALESCE(total_tokens, 0) + ?, 0),
    total_cost_usd = GREATEST(COALESCE(total_cost_usd, 0) + ?, 0),
    last_used_at = CASE
      WHEN ? IS NOT NULL THEN GREATEST(COALESCE(last_used_at, 0), ?)
      WHEN ? IS NOT NULL AND last_used_at = ? THEN (
        SELECT MAX(created_at_unix_ms)
        FROM `usage`
        WHERE api_key_id = ? AND status NOT IN ('pending', 'streaming')
      )
      ELSE last_used_at
    END
WHERE id = ?
"#,
    )
    .bind(delta.total_requests)
    .bind(delta.total_tokens)
    .bind(finite_or_zero(delta.total_cost_usd))
    .bind(candidate)
    .bind(candidate)
    .bind(removed)
    .bind(removed)
    .bind(target_id)
    .bind(target_id)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

async fn apply_model(
    tx: &mut sqlx::Transaction<'_, MySql>,
    target_id: &str,
    delta: &ModelUsageDelta,
) -> Result<(), DataLayerError> {
    if target_id.trim().is_empty() || delta.is_noop() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE global_models SET usage_count = GREATEST(COALESCE(usage_count, 0) + ?, 0), updated_at = ? WHERE name = ?",
    )
    .bind(delta.request_count)
    .bind(current_unix_secs())
    .bind(target_id)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

async fn apply_provider_api_key(
    tx: &mut sqlx::Transaction<'_, MySql>,
    target_id: &str,
    delta: &ProviderApiKeyUsageDelta,
) -> Result<(), DataLayerError> {
    if target_id.trim().is_empty() || delta.is_noop() {
        return Ok(());
    }
    let candidate = optional_to_i64(
        delta.candidate_last_used_at_unix_secs,
        "provider api key candidate last used at",
    )?;
    let removed = optional_to_i64(
        delta.removed_last_used_at_unix_secs,
        "provider api key removed last used at",
    )?;
    sqlx::query(
        r#"
UPDATE provider_api_keys
SET request_count = GREATEST(COALESCE(request_count, 0) + ?, 0),
    success_count = GREATEST(COALESCE(success_count, 0) + ?, 0),
    sla_eligible_count = GREATEST(COALESCE(sla_eligible_count, 0) + ?, 0),
    error_count = GREATEST(COALESCE(error_count, 0) + ?, 0),
    user_error_count = GREATEST(COALESCE(user_error_count, 0) + ?, 0),
    total_tokens = GREATEST(COALESCE(total_tokens, 0) + ?, 0),
    total_cost_usd = GREATEST(COALESCE(total_cost_usd, 0) + ?, 0),
    total_response_time_ms = GREATEST(COALESCE(total_response_time_ms, 0) + ?, 0),
    last_used_at = CASE
      WHEN ? IS NOT NULL THEN GREATEST(COALESCE(last_used_at, 0), ?)
      WHEN ? IS NOT NULL AND last_used_at = ? THEN (
        SELECT MAX(created_at_unix_ms)
        FROM `usage`
        WHERE provider_api_key_id = ? AND status NOT IN ('pending', 'streaming')
      )
      ELSE last_used_at
    END
WHERE id = ?
"#,
    )
    .bind(delta.request_count)
    .bind(delta.success_count)
    .bind(delta.sla_eligible_count)
    .bind(delta.error_count)
    .bind(delta.user_error_count)
    .bind(delta.total_tokens)
    .bind(finite_or_zero(delta.total_cost_usd))
    .bind(delta.total_response_time_ms)
    .bind(candidate)
    .bind(candidate)
    .bind(removed)
    .bind(removed)
    .bind(target_id)
    .bind(target_id)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

async fn apply_provider_monthly(
    tx: &mut sqlx::Transaction<'_, MySql>,
    target_id: &str,
    delta: f64,
) -> Result<(), DataLayerError> {
    if target_id.trim().is_empty() || delta == 0.0 {
        return Ok(());
    }
    if !delta.is_finite() {
        return Err(DataLayerError::UnexpectedValue(format!(
            "providers.monthly_used_usd delta is not finite for {target_id}"
        )));
    }
    sqlx::query(
        "UPDATE providers SET monthly_used_usd = COALESCE(monthly_used_usd, 0) + ?, updated_at = ? WHERE id = ?",
    )
    .bind(delta)
    .bind(current_unix_secs())
    .bind(target_id)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

async fn apply_proxy_node(
    tx: &mut sqlx::Transaction<'_, MySql>,
    target_id: &str,
    delta: &ProxyNodeCounterDelta,
) -> Result<(), DataLayerError> {
    if target_id.trim().is_empty() || delta.is_noop() {
        return Ok(());
    }
    sqlx::query(
        r#"
UPDATE proxy_nodes
SET total_requests = total_requests + GREATEST(?, 0),
    failed_requests = failed_requests + GREATEST(?, 0),
    dns_failures = dns_failures + GREATEST(?, 0),
    stream_errors = stream_errors + GREATEST(?, 0),
    updated_at = ?
WHERE id = ?
"#,
    )
    .bind(delta.total_requests_delta)
    .bind(delta.failed_requests_delta)
    .bind(delta.dns_failures_delta)
    .bind(delta.stream_errors_delta)
    .bind(current_unix_secs())
    .bind(target_id)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

async fn apply_management_token(
    tx: &mut sqlx::Transaction<'_, MySql>,
    target_id: &str,
    delta: &ManagementTokenCounterDelta,
) -> Result<(), DataLayerError> {
    if target_id.trim().is_empty() || delta.is_noop() {
        return Ok(());
    }
    let last_used_at = optional_to_i64(
        delta.last_used_at_unix_secs,
        "management token last used at",
    )?;
    sqlx::query(
        r#"
UPDATE management_tokens
SET usage_count = COALESCE(usage_count, 0) + GREATEST(?, 0),
    last_used_at = CASE
      WHEN ? IS NULL THEN last_used_at
      ELSE GREATEST(COALESCE(last_used_at, 0), ?)
    END,
    last_used_ip = COALESCE(?, last_used_ip),
    updated_at = ?
WHERE id = ?
"#,
    )
    .bind(delta.usage_count_delta)
    .bind(last_used_at)
    .bind(last_used_at)
    .bind(delta.last_used_ip.as_deref())
    .bind(current_unix_secs())
    .bind(target_id)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

async fn apply_api_key_last_used(
    tx: &mut sqlx::Transaction<'_, MySql>,
    target_id: &str,
    delta: &ApiKeyLastUsedDelta,
) -> Result<(), DataLayerError> {
    if target_id.trim().is_empty() || delta.is_noop() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE api_keys SET last_used_at = GREATEST(COALESCE(last_used_at, 0), ?) WHERE id = ?",
    )
    .bind(to_i64(
        delta.last_used_at_unix_secs,
        "api key last used at",
    )?)
    .bind(target_id)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

fn merge_optional_max(target: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        if target.is_none_or(|current| value > current) {
            *target = Some(value);
        }
    }
}

fn merge_latest_timestamp_with_value(
    target_timestamp: &mut Option<u64>,
    target_value: &mut Option<String>,
    timestamp: Option<u64>,
    value: Option<String>,
) {
    let Some(timestamp) = timestamp else {
        return;
    };
    if target_timestamp.is_none_or(|current| timestamp >= current) {
        *target_timestamp = Some(timestamp);
        if value
            .as_deref()
            .map(str::trim)
            .is_some_and(|v| !v.is_empty())
        {
            *target_value = value;
        }
    }
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn current_unix_secs() -> i64 {
    chrono::Utc::now().timestamp().max(0)
}

fn to_i64(value: u64, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field} exceeds i64: {value}")))
}

fn optional_to_i64(value: Option<u64>, field: &str) -> Result<Option<i64>, DataLayerError> {
    value.map(|value| to_i64(value, field)).transpose()
}

fn optional_u64(field: &str, value: Option<i64>) -> Result<Option<u64>, DataLayerError> {
    value
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!("{field} is negative: {value}"))
            })
        })
        .transpose()
}

fn nonnegative_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn optional_nonnegative_u64(value: Option<i64>) -> Option<u64> {
    value.map(nonnegative_u64)
}
