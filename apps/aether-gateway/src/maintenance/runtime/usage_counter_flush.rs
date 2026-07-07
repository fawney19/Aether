use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::data::GatewayDataState;
use aether_data::DataLayerError;
use aether_data_contracts::repository::usage::UsageCounterFlushSummary;
use aether_runtime::{MetricKind, MetricLabel, MetricSample};

const USAGE_COUNTER_FLUSH_INTERVAL_MS_ENV: &str = "AETHER_GATEWAY_USAGE_COUNTER_FLUSH_INTERVAL_MS";
const USAGE_COUNTER_FLUSH_BATCH_SIZE_ENV: &str = "AETHER_GATEWAY_USAGE_COUNTER_FLUSH_BATCH_SIZE";
const USAGE_COUNTER_FLUSH_CATCH_UP_BURST_LIMIT_ENV: &str =
    "AETHER_GATEWAY_USAGE_COUNTER_FLUSH_CATCH_UP_BURST_LIMIT";
const USAGE_COUNTER_DELTA_CLEANUP_INTERVAL_MS_ENV: &str =
    "AETHER_GATEWAY_USAGE_COUNTER_DELTA_CLEANUP_INTERVAL_MS";
const USAGE_COUNTER_DELTA_CLEANUP_BATCH_SIZE_ENV: &str =
    "AETHER_GATEWAY_USAGE_COUNTER_DELTA_CLEANUP_BATCH_SIZE";
const USAGE_COUNTER_DELTA_RETENTION_SECS_ENV: &str =
    "AETHER_GATEWAY_USAGE_COUNTER_DELTA_RETENTION_SECS";

const DEFAULT_USAGE_COUNTER_FLUSH_INTERVAL_MS: u64 = 1_000;
const DEFAULT_USAGE_COUNTER_FLUSH_BATCH_SIZE: usize = 1_000;
const DEFAULT_USAGE_COUNTER_FLUSH_CATCH_UP_BURST_LIMIT: usize = 20;
const DEFAULT_USAGE_COUNTER_DELTA_CLEANUP_INTERVAL_MS: u64 = 60_000;
const DEFAULT_USAGE_COUNTER_DELTA_CLEANUP_BATCH_SIZE: usize = 5_000;
const DEFAULT_USAGE_COUNTER_DELTA_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageCounterFlushWorkerConfig {
    pub(crate) flush_interval: Duration,
    pub(crate) flush_batch_size: usize,
    pub(crate) flush_catch_up_burst_limit: usize,
    pub(crate) cleanup_interval: Duration,
    pub(crate) cleanup_batch_size: usize,
    pub(crate) delta_retention_secs: u64,
}

impl Default for UsageCounterFlushWorkerConfig {
    fn default() -> Self {
        Self {
            flush_interval: Duration::from_millis(DEFAULT_USAGE_COUNTER_FLUSH_INTERVAL_MS),
            flush_batch_size: DEFAULT_USAGE_COUNTER_FLUSH_BATCH_SIZE,
            flush_catch_up_burst_limit: DEFAULT_USAGE_COUNTER_FLUSH_CATCH_UP_BURST_LIMIT,
            cleanup_interval: Duration::from_millis(
                DEFAULT_USAGE_COUNTER_DELTA_CLEANUP_INTERVAL_MS,
            ),
            cleanup_batch_size: DEFAULT_USAGE_COUNTER_DELTA_CLEANUP_BATCH_SIZE,
            delta_retention_secs: DEFAULT_USAGE_COUNTER_DELTA_RETENTION_SECS,
        }
    }
}

impl UsageCounterFlushWorkerConfig {
    pub(crate) fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            flush_interval: Duration::from_millis(env_u64(
                USAGE_COUNTER_FLUSH_INTERVAL_MS_ENV,
                duration_millis_u64(defaults.flush_interval),
            )),
            flush_batch_size: env_usize(
                USAGE_COUNTER_FLUSH_BATCH_SIZE_ENV,
                defaults.flush_batch_size,
            ),
            flush_catch_up_burst_limit: env_usize(
                USAGE_COUNTER_FLUSH_CATCH_UP_BURST_LIMIT_ENV,
                defaults.flush_catch_up_burst_limit,
            ),
            cleanup_interval: Duration::from_millis(env_u64(
                USAGE_COUNTER_DELTA_CLEANUP_INTERVAL_MS_ENV,
                duration_millis_u64(defaults.cleanup_interval),
            )),
            cleanup_batch_size: env_usize(
                USAGE_COUNTER_DELTA_CLEANUP_BATCH_SIZE_ENV,
                defaults.cleanup_batch_size,
            ),
            delta_retention_secs: env_u64(
                USAGE_COUNTER_DELTA_RETENTION_SECS_ENV,
                defaults.delta_retention_secs,
            ),
        }
        .normalized()
    }

    fn normalized(mut self) -> Self {
        self.flush_interval = self.flush_interval.max(Duration::from_millis(1));
        self.flush_batch_size = self.flush_batch_size.max(1);
        self.flush_catch_up_burst_limit = self.flush_catch_up_burst_limit.max(1);
        self.cleanup_interval = self.cleanup_interval.max(Duration::from_millis(1));
        self.cleanup_batch_size = self.cleanup_batch_size.max(1);
        self
    }
}

#[derive(Debug, Default)]
pub(crate) struct UsageCounterFlushRuntimeMetrics {
    flush_batches_total: AtomicU64,
    flush_empty_batches_total: AtomicU64,
    flush_rows_claimed_total: AtomicU64,
    flush_api_key_targets_total: AtomicU64,
    flush_provider_api_key_targets_total: AtomicU64,
    flush_model_targets_total: AtomicU64,
    flush_provider_monthly_targets_total: AtomicU64,
    flush_proxy_node_targets_total: AtomicU64,
    flush_management_token_targets_total: AtomicU64,
    flush_api_key_last_used_targets_total: AtomicU64,
    flush_failed_batches_total: AtomicU64,
    flush_deferred_total: AtomicU64,
    cleanup_batches_total: AtomicU64,
    cleanup_rows_total: AtomicU64,
    cleanup_failed_batches_total: AtomicU64,
    cleanup_deferred_total: AtomicU64,
}

impl UsageCounterFlushRuntimeMetrics {
    pub(crate) fn record_flush_success(&self, summary: &UsageCounterFlushSummary) {
        if summary.rows_claimed > 0 {
            self.flush_batches_total.fetch_add(1, Ordering::AcqRel);
        } else {
            self.flush_empty_batches_total
                .fetch_add(1, Ordering::AcqRel);
        }
        self.flush_rows_claimed_total
            .fetch_add(usize_to_u64(summary.rows_claimed), Ordering::AcqRel);
        self.flush_api_key_targets_total
            .fetch_add(usize_to_u64(summary.api_key_targets), Ordering::AcqRel);
        self.flush_provider_api_key_targets_total.fetch_add(
            usize_to_u64(summary.provider_api_key_targets),
            Ordering::AcqRel,
        );
        self.flush_model_targets_total
            .fetch_add(usize_to_u64(summary.model_targets), Ordering::AcqRel);
        self.flush_provider_monthly_targets_total.fetch_add(
            usize_to_u64(summary.provider_monthly_targets),
            Ordering::AcqRel,
        );
        self.flush_proxy_node_targets_total
            .fetch_add(usize_to_u64(summary.proxy_node_targets), Ordering::AcqRel);
        self.flush_management_token_targets_total.fetch_add(
            usize_to_u64(summary.management_token_targets),
            Ordering::AcqRel,
        );
        self.flush_api_key_last_used_targets_total.fetch_add(
            usize_to_u64(summary.api_key_last_used_targets),
            Ordering::AcqRel,
        );
    }

    pub(crate) fn record_flush_failed(&self) {
        self.flush_failed_batches_total
            .fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn record_flush_deferred(&self) {
        self.flush_deferred_total.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn record_cleanup_success(&self, rows_deleted: usize) {
        self.cleanup_batches_total.fetch_add(1, Ordering::AcqRel);
        self.cleanup_rows_total
            .fetch_add(usize_to_u64(rows_deleted), Ordering::AcqRel);
    }

    pub(crate) fn record_cleanup_failed(&self) {
        self.cleanup_failed_batches_total
            .fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn record_cleanup_deferred(&self) {
        self.cleanup_deferred_total.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn metric_samples(&self) -> Vec<MetricSample> {
        let mut samples = vec![
            MetricSample::new(
                "usage_counter_outbox_flush_batches_total",
                "Total non-empty usage counter outbox flush batches completed by this gateway process.",
                MetricKind::Counter,
                self.flush_batches_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "usage_counter_outbox_flush_empty_batches_total",
                "Total successful usage counter outbox flush checks that found no rows.",
                MetricKind::Counter,
                self.flush_empty_batches_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "usage_counter_outbox_flush_rows_claimed_total",
                "Total usage counter outbox rows claimed and processed by this gateway process.",
                MetricKind::Counter,
                self.flush_rows_claimed_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "usage_counter_outbox_flush_failed_batches_total",
                "Total usage counter outbox flush batches that failed in this gateway process.",
                MetricKind::Counter,
                self.flush_failed_batches_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "usage_counter_outbox_flush_deferred_total",
                "Total usage counter outbox flush ticks deferred because of database pool pressure.",
                MetricKind::Counter,
                self.flush_deferred_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "usage_counter_outbox_cleanup_batches_total",
                "Total usage counter outbox cleanup batches completed by this gateway process.",
                MetricKind::Counter,
                self.cleanup_batches_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "usage_counter_outbox_cleanup_rows_total",
                "Total processed usage counter outbox rows deleted by cleanup in this gateway process.",
                MetricKind::Counter,
                self.cleanup_rows_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "usage_counter_outbox_cleanup_failed_batches_total",
                "Total usage counter outbox cleanup batches that failed in this gateway process.",
                MetricKind::Counter,
                self.cleanup_failed_batches_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "usage_counter_outbox_cleanup_deferred_total",
                "Total usage counter outbox cleanup ticks deferred because of database pool pressure.",
                MetricKind::Counter,
                self.cleanup_deferred_total.load(Ordering::Acquire),
            ),
        ];
        for (kind, value) in [
            (
                "api_key",
                self.flush_api_key_targets_total.load(Ordering::Acquire),
            ),
            (
                "provider_api_key",
                self.flush_provider_api_key_targets_total
                    .load(Ordering::Acquire),
            ),
            (
                "model",
                self.flush_model_targets_total.load(Ordering::Acquire),
            ),
            (
                "provider_monthly",
                self.flush_provider_monthly_targets_total
                    .load(Ordering::Acquire),
            ),
            (
                "proxy_node",
                self.flush_proxy_node_targets_total.load(Ordering::Acquire),
            ),
            (
                "management_token",
                self.flush_management_token_targets_total
                    .load(Ordering::Acquire),
            ),
            (
                "api_key_last_used",
                self.flush_api_key_last_used_targets_total
                    .load(Ordering::Acquire),
            ),
        ] {
            samples.push(
                MetricSample::new(
                    "usage_counter_outbox_flush_targets_total",
                    "Total usage counter outbox aggregate targets updated by target kind.",
                    MetricKind::Counter,
                    value,
                )
                .with_labels(vec![MetricLabel::new("kind", kind)]),
            );
        }
        samples
    }
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn env_u64(name: &str, default_value: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default_value)
}

fn env_usize(name: &str, default_value: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default_value)
}

pub(crate) async fn run_usage_counter_flush_once(
    data: &GatewayDataState,
    batch_size: usize,
) -> Result<UsageCounterFlushSummary, DataLayerError> {
    data.flush_usage_counter_deltas(batch_size).await
}

pub(crate) async fn cleanup_processed_usage_counter_deltas_once(
    data: &GatewayDataState,
    retention_secs: u64,
    batch_size: usize,
) -> Result<usize, DataLayerError> {
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    let cutoff = now.saturating_sub(retention_secs);
    data.cleanup_processed_usage_counter_deltas(cutoff, batch_size)
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use super::{
        UsageCounterFlushRuntimeMetrics, UsageCounterFlushSummary, UsageCounterFlushWorkerConfig,
        USAGE_COUNTER_DELTA_CLEANUP_BATCH_SIZE_ENV, USAGE_COUNTER_DELTA_CLEANUP_INTERVAL_MS_ENV,
        USAGE_COUNTER_DELTA_RETENTION_SECS_ENV, USAGE_COUNTER_FLUSH_BATCH_SIZE_ENV,
        USAGE_COUNTER_FLUSH_CATCH_UP_BURST_LIMIT_ENV, USAGE_COUNTER_FLUSH_INTERVAL_MS_ENV,
    };

    const CONFIG_ENV_KEYS: &[&str] = &[
        USAGE_COUNTER_FLUSH_INTERVAL_MS_ENV,
        USAGE_COUNTER_FLUSH_BATCH_SIZE_ENV,
        USAGE_COUNTER_FLUSH_CATCH_UP_BURST_LIMIT_ENV,
        USAGE_COUNTER_DELTA_CLEANUP_INTERVAL_MS_ENV,
        USAGE_COUNTER_DELTA_CLEANUP_BATCH_SIZE_ENV,
        USAGE_COUNTER_DELTA_RETENTION_SECS_ENV,
    ];

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvVarGuard {
        values: Vec<(&'static str, Option<String>)>,
    }

    impl EnvVarGuard {
        fn new(keys: &[&'static str]) -> Self {
            let values = keys
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect();
            Self { values }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            for (key, value) in &self.values {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn sample_value(samples: &[aether_runtime::MetricSample], name: &str) -> u64 {
        samples
            .iter()
            .find(|sample| sample.name == name)
            .map(|sample| sample.value)
            .expect("sample should exist")
    }

    fn target_value(samples: &[aether_runtime::MetricSample], kind: &str) -> u64 {
        samples
            .iter()
            .find(|sample| {
                sample.name == "usage_counter_outbox_flush_targets_total"
                    && sample
                        .labels
                        .iter()
                        .any(|label| label.key == "kind" && label.value == kind)
            })
            .map(|sample| sample.value)
            .expect("target sample should exist")
    }

    #[test]
    fn usage_counter_flush_worker_config_reads_env() {
        let _lock = env_lock().lock().expect("env lock should not be poisoned");
        let _guard = EnvVarGuard::new(CONFIG_ENV_KEYS);
        std::env::set_var(USAGE_COUNTER_FLUSH_INTERVAL_MS_ENV, "100");
        std::env::set_var(USAGE_COUNTER_FLUSH_BATCH_SIZE_ENV, "2000");
        std::env::set_var(USAGE_COUNTER_FLUSH_CATCH_UP_BURST_LIMIT_ENV, "50");
        std::env::set_var(USAGE_COUNTER_DELTA_CLEANUP_INTERVAL_MS_ENV, "250");
        std::env::set_var(USAGE_COUNTER_DELTA_CLEANUP_BATCH_SIZE_ENV, "7000");
        std::env::set_var(USAGE_COUNTER_DELTA_RETENTION_SECS_ENV, "0");

        let config = UsageCounterFlushWorkerConfig::from_env();

        assert_eq!(config.flush_interval, Duration::from_millis(100));
        assert_eq!(config.flush_batch_size, 2000);
        assert_eq!(config.flush_catch_up_burst_limit, 50);
        assert_eq!(config.cleanup_interval, Duration::from_millis(250));
        assert_eq!(config.cleanup_batch_size, 7000);
        assert_eq!(config.delta_retention_secs, 0);
    }

    #[test]
    fn usage_counter_flush_worker_config_normalizes_zero_values() {
        let _lock = env_lock().lock().expect("env lock should not be poisoned");
        let _guard = EnvVarGuard::new(CONFIG_ENV_KEYS);
        for key in CONFIG_ENV_KEYS {
            std::env::set_var(key, "0");
        }

        let config = UsageCounterFlushWorkerConfig::from_env();

        assert_eq!(config.flush_interval, Duration::from_millis(1));
        assert_eq!(config.flush_batch_size, 1);
        assert_eq!(config.flush_catch_up_burst_limit, 1);
        assert_eq!(config.cleanup_interval, Duration::from_millis(1));
        assert_eq!(config.cleanup_batch_size, 1);
        assert_eq!(config.delta_retention_secs, 0);
    }

    #[test]
    fn usage_counter_flush_runtime_metrics_record_success_and_failures() {
        let metrics = UsageCounterFlushRuntimeMetrics::default();

        metrics.record_flush_success(&UsageCounterFlushSummary {
            rows_claimed: 3,
            api_key_targets: 1,
            provider_api_key_targets: 2,
            model_targets: 3,
            provider_monthly_targets: 4,
            proxy_node_targets: 5,
            management_token_targets: 6,
            api_key_last_used_targets: 7,
        });
        metrics.record_flush_success(&UsageCounterFlushSummary::default());
        metrics.record_flush_failed();
        metrics.record_flush_deferred();
        metrics.record_cleanup_success(11);
        metrics.record_cleanup_failed();
        metrics.record_cleanup_deferred();

        let samples = metrics.metric_samples();
        assert_eq!(
            sample_value(&samples, "usage_counter_outbox_flush_batches_total"),
            1
        );
        assert_eq!(
            sample_value(&samples, "usage_counter_outbox_flush_empty_batches_total"),
            1
        );
        assert_eq!(
            sample_value(&samples, "usage_counter_outbox_flush_rows_claimed_total"),
            3
        );
        assert_eq!(
            sample_value(&samples, "usage_counter_outbox_flush_failed_batches_total"),
            1
        );
        assert_eq!(
            sample_value(&samples, "usage_counter_outbox_flush_deferred_total"),
            1
        );
        assert_eq!(
            sample_value(&samples, "usage_counter_outbox_cleanup_batches_total"),
            1
        );
        assert_eq!(
            sample_value(&samples, "usage_counter_outbox_cleanup_rows_total"),
            11
        );
        assert_eq!(
            sample_value(
                &samples,
                "usage_counter_outbox_cleanup_failed_batches_total"
            ),
            1
        );
        assert_eq!(
            sample_value(&samples, "usage_counter_outbox_cleanup_deferred_total"),
            1
        );
        assert_eq!(target_value(&samples, "api_key"), 1);
        assert_eq!(target_value(&samples, "provider_api_key"), 2);
        assert_eq!(target_value(&samples, "model"), 3);
        assert_eq!(target_value(&samples, "provider_monthly"), 4);
        assert_eq!(target_value(&samples, "proxy_node"), 5);
        assert_eq!(target_value(&samples, "management_token"), 6);
        assert_eq!(target_value(&samples, "api_key_last_used"), 7);
    }
}
