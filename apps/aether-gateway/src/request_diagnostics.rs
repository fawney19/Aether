use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aether_data::DatabasePoolSummary;
use serde_json::{Map, Value};

tokio::task_local! {
    static REQUEST_DIAGNOSTICS: Arc<RequestDiagnostics>;
}

#[derive(Debug, Default)]
pub(crate) struct RequestDiagnostics {
    inner: Mutex<RequestDiagnosticsInner>,
}

#[derive(Debug, Default)]
struct RequestDiagnosticsInner {
    request_accepted_at: Option<Instant>,
    db_operations: BTreeMap<&'static str, DbOperationTiming>,
    db_pool: Option<DbPoolObservation>,
}

#[derive(Debug, Clone, Copy, Default)]
struct DbOperationTiming {
    count: u64,
    sum_ms: u64,
    max_ms: u64,
}

#[derive(Debug, Clone, Copy)]
struct DbPoolObservation {
    max_checked_out: u64,
    max_pool_size: u64,
    min_idle: u64,
    max_connections: u64,
    max_usage_rate_x100: u64,
}

impl RequestDiagnostics {
    fn record_request_accepted_at(&self, accepted_at: Instant) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.request_accepted_at = Some(accepted_at);
    }

    pub(crate) fn request_accepted_at(&self) -> Option<Instant> {
        let Ok(inner) = self.inner.lock() else {
            return None;
        };
        inner.request_accepted_at
    }

    fn record_db_timing_ms(&self, operation: &'static str, elapsed_ms: u64) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let timing = inner.db_operations.entry(operation).or_default();
        timing.count = timing.count.saturating_add(1);
        timing.sum_ms = timing.sum_ms.saturating_add(elapsed_ms);
        timing.max_ms = timing.max_ms.max(elapsed_ms);
    }

    fn record_db_pool_summary(&self, summary: DatabasePoolSummary) {
        let observation = DbPoolObservation {
            max_checked_out: summary.checked_out as u64,
            max_pool_size: summary.pool_size as u64,
            min_idle: summary.idle as u64,
            max_connections: u64::from(summary.max_connections),
            max_usage_rate_x100: (summary.usage_rate * 100.0).max(0.0).round() as u64,
        };
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.db_pool = Some(match inner.db_pool {
            Some(existing) => DbPoolObservation {
                max_checked_out: existing.max_checked_out.max(observation.max_checked_out),
                max_pool_size: existing.max_pool_size.max(observation.max_pool_size),
                min_idle: existing.min_idle.min(observation.min_idle),
                max_connections: existing.max_connections.max(observation.max_connections),
                max_usage_rate_x100: existing
                    .max_usage_rate_x100
                    .max(observation.max_usage_rate_x100),
            },
            None => observation,
        });
    }

    pub(crate) fn db_timings_metadata(&self) -> Option<Value> {
        let Ok(inner) = self.inner.lock() else {
            return None;
        };
        if inner.db_operations.is_empty() && inner.db_pool.is_none() {
            return None;
        }

        let mut total_count = 0_u64;
        let mut query_total_ms = 0_u64;
        let mut query_max_ms = 0_u64;
        let mut operations = Map::new();
        for (operation, timing) in &inner.db_operations {
            total_count = total_count.saturating_add(timing.count);
            query_total_ms = query_total_ms.saturating_add(timing.sum_ms);
            query_max_ms = query_max_ms.max(timing.max_ms);
            operations.insert(
                (*operation).to_string(),
                Value::Object(Map::from_iter([
                    ("count".to_string(), Value::from(timing.count)),
                    ("sum".to_string(), Value::from(timing.sum_ms)),
                    ("max".to_string(), Value::from(timing.max_ms)),
                ])),
            );
        }

        let mut metadata = Map::new();
        if !operations.is_empty() {
            metadata.insert("query_count".to_string(), Value::from(total_count));
            metadata.insert("query_total".to_string(), Value::from(query_total_ms));
            metadata.insert("query_max".to_string(), Value::from(query_max_ms));
            metadata.insert("operations".to_string(), Value::Object(operations));
        }
        if let Some(pool) = inner.db_pool {
            metadata.insert(
                "pool".to_string(),
                Value::Object(Map::from_iter([
                    (
                        "max_checked_out".to_string(),
                        Value::from(pool.max_checked_out),
                    ),
                    ("max_pool_size".to_string(), Value::from(pool.max_pool_size)),
                    ("min_idle".to_string(), Value::from(pool.min_idle)),
                    (
                        "max_connections".to_string(),
                        Value::from(pool.max_connections),
                    ),
                    (
                        "max_usage_rate".to_string(),
                        Value::from(pool.max_usage_rate_x100 as f64 / 100.0),
                    ),
                ])),
            );
        }

        Some(Value::Object(metadata))
    }
}

pub(crate) async fn scope_request_diagnostics<F>(future: F) -> F::Output
where
    F: Future,
{
    REQUEST_DIAGNOSTICS
        .scope(Arc::new(RequestDiagnostics::default()), future)
        .await
}

pub(crate) fn current_request_diagnostics() -> Option<Arc<RequestDiagnostics>> {
    REQUEST_DIAGNOSTICS.try_with(Arc::clone).ok()
}

pub(crate) fn record_request_accepted_at(accepted_at: Instant) {
    if let Some(diagnostics) = current_request_diagnostics() {
        diagnostics.record_request_accepted_at(accepted_at);
    }
}

pub(crate) async fn observe_db_operation<F>(
    operation: &'static str,
    pool_summary: Option<DatabasePoolSummary>,
    future: F,
) -> F::Output
where
    F: Future,
{
    if let Some(summary) = pool_summary {
        record_db_pool_summary(summary);
    }
    let started_at = Instant::now();
    let output = future.await;
    record_db_timing_ms(operation, started_at.elapsed().as_millis() as u64);
    output
}

pub(crate) fn record_db_timing_ms(operation: &'static str, elapsed_ms: u64) {
    if let Some(diagnostics) = current_request_diagnostics() {
        diagnostics.record_db_timing_ms(operation, elapsed_ms);
    }
}

pub(crate) fn record_db_pool_summary(summary: DatabasePoolSummary) {
    if let Some(diagnostics) = current_request_diagnostics() {
        diagnostics.record_db_pool_summary(summary);
    }
}

pub(crate) fn attach_request_diagnostics_to_report_context(
    report_context: Option<Value>,
    diagnostics: Option<&Arc<RequestDiagnostics>>,
) -> Option<Value> {
    let Some(db_timings_ms) = diagnostics.and_then(|diagnostics| diagnostics.db_timings_metadata())
    else {
        return report_context;
    };

    let mut object = match report_context {
        Some(Value::Object(object)) => object,
        Some(other) => Map::from_iter([("seed".to_string(), other)]),
        None => Map::new(),
    };
    object.insert("db_timings_ms".to_string(), db_timings_ms);
    Some(Value::Object(object))
}

pub(crate) fn attach_current_request_diagnostics_to_report_context(
    report_context: Option<&Value>,
) -> Option<Value> {
    let diagnostics = current_request_diagnostics()?;
    attach_request_diagnostics_to_report_context(report_context.cloned(), Some(&diagnostics))
}
