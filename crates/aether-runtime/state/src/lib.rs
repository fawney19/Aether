mod error;
mod memory;
pub mod redis;
mod score_window;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use crate::redis::{
    RedisClientConfig, RedisConsumerGroup, RedisConsumerName, RedisKeyspace, RedisKvRunner,
    RedisKvRunnerConfig, RedisLaneDiagnostics, RedisLockLease, RedisLockRunner,
    RedisLockRunnerConfig, RedisRuntimeDiagnostics, RedisStreamEntry, RedisStreamName,
    RedisStreamReclaimConfig, RedisStreamRunner, RedisStreamRunnerConfig,
};
use async_trait::async_trait;
pub use error::DataLayerError;
use memory::MemoryRuntimeBackend;
pub use memory::MemoryRuntimeStateConfig;
pub use score_window::{ScoreWindowU64Stats, SCORE_WINDOW_AGGREGATION_MEMBER_LIMIT};
use tokio::task::JoinHandle;
use tracing::warn;
use uuid::Uuid;

const DEFAULT_KV_TTL_SECONDS: u64 = 300;
// Runtime coordination must tolerate brief executor scheduling pauses under large streaming
// fan-in. This remains comfortably below the gateway's default 10s lease-renew interval and 30s
// lease TTL, so fail-closed fencing still has ample time to stop a stale holder.
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 2_000;
const DEFAULT_STREAM_BLOCK_TIMEOUT_GRACE_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStateBackendMode {
    Auto,
    Memory,
    Redis,
}

impl RuntimeStateBackendMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Memory => "memory",
            Self::Redis => "redis",
        }
    }
}

impl std::str::FromStr for RuntimeStateBackendMode {
    type Err = DataLayerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "memory" => Ok(Self::Memory),
            "redis" => Ok(Self::Redis),
            other => Err(DataLayerError::InvalidConfiguration(format!(
                "unsupported runtime backend {other}; expected auto, memory, or redis"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStateConfig {
    pub backend: RuntimeStateBackendMode,
    pub redis: Option<RedisClientConfig>,
    pub memory: MemoryRuntimeStateConfig,
    pub command_timeout_ms: Option<u64>,
    pub blocking_stream_lanes: Option<usize>,
}

impl Default for RuntimeStateConfig {
    fn default() -> Self {
        Self {
            backend: RuntimeStateBackendMode::Auto,
            redis: None,
            memory: MemoryRuntimeStateConfig::default(),
            command_timeout_ms: Some(DEFAULT_COMMAND_TIMEOUT_MS),
            blocking_stream_lanes: None,
        }
    }
}

impl RuntimeStateConfig {
    pub fn memory() -> Self {
        Self {
            backend: RuntimeStateBackendMode::Memory,
            redis: None,
            ..Self::default()
        }
    }

    pub fn redis(redis: RedisClientConfig) -> Self {
        Self {
            backend: RuntimeStateBackendMode::Redis,
            redis: Some(redis),
            ..Self::default()
        }
    }

    pub fn redis_url_from_env() -> Option<String> {
        env_value("AETHER_RUNTIME_REDIS_URL")
            .or_else(|| env_value("AETHER_GATEWAY_DATA_REDIS_URL"))
            .or_else(|| env_value("REDIS_URL"))
    }

    pub fn redis_key_prefix_from_env() -> Option<String> {
        env_value("AETHER_RUNTIME_REDIS_KEY_PREFIX")
            .or_else(|| env_value("AETHER_GATEWAY_DATA_REDIS_KEY_PREFIX"))
    }

    pub fn from_env_with_backend(backend: RuntimeStateBackendMode) -> Self {
        let redis = if matches!(backend, RuntimeStateBackendMode::Redis) {
            Self::redis_url_from_env().map(|url| RedisClientConfig {
                url,
                key_prefix: Self::redis_key_prefix_from_env(),
            })
        } else {
            None
        };
        Self {
            backend,
            redis,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), DataLayerError> {
        if matches!(self.backend, RuntimeStateBackendMode::Redis) && self.redis.is_none() {
            return Err(DataLayerError::InvalidConfiguration(
                "AETHER_RUNTIME_BACKEND=redis requires AETHER_RUNTIME_REDIS_URL, AETHER_GATEWAY_DATA_REDIS_URL, or REDIS_URL".to_string(),
            ));
        }
        if let Some(redis) = &self.redis {
            redis.validate()?;
        }
        if self.memory.max_kv_entries == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "runtime memory max_kv_entries must be positive".to_string(),
            ));
        }
        if self.memory.max_usage_limit_windows == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "runtime memory max_usage_limit_windows must be positive".to_string(),
            ));
        }
        if self.memory.max_usage_limit_events == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "runtime memory max_usage_limit_events must be positive".to_string(),
            ));
        }
        if matches!(self.command_timeout_ms, Some(0)) {
            return Err(DataLayerError::InvalidConfiguration(
                "runtime state command_timeout_ms must be positive".to_string(),
            ));
        }
        if matches!(self.blocking_stream_lanes, Some(0)) {
            return Err(DataLayerError::InvalidConfiguration(
                "runtime state blocking_stream_lanes must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStateBackendKind {
    Memory,
    Redis,
}

impl RuntimeStateBackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Redis => "redis",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeState {
    backend: Arc<RuntimeStateBackend>,
}

#[derive(Debug)]
enum RuntimeStateBackend {
    Memory(Box<MemoryRuntimeBackend>),
    Redis(Box<RedisRuntimeBackend>),
}

#[derive(Debug, Clone)]
struct RedisRuntimeBackend {
    keyspace: RedisKeyspace,
    kv: RedisKvRunner,
    lock: RedisLockRunner,
    stream: RedisStreamRunner,
    runtime: redis::RedisRuntimeRunner,
    command_timeout_ms: Option<u64>,
}

impl RuntimeState {
    pub async fn from_config(mut config: RuntimeStateConfig) -> Result<Self, DataLayerError> {
        if matches!(config.backend, RuntimeStateBackendMode::Auto) {
            config.backend = if config.redis.is_some() {
                RuntimeStateBackendMode::Redis
            } else {
                RuntimeStateBackendMode::Memory
            };
        }
        config.validate()?;
        match config.backend {
            RuntimeStateBackendMode::Memory => Ok(Self::memory(config.memory)),
            RuntimeStateBackendMode::Redis => {
                let redis = config.redis.clone().ok_or_else(|| {
                    DataLayerError::InvalidConfiguration("runtime redis config missing".to_string())
                })?;
                Self::redis_with_blocking_stream_lanes(
                    redis,
                    config.command_timeout_ms,
                    config.blocking_stream_lanes,
                )
                .await
            }
            RuntimeStateBackendMode::Auto => unreachable!("auto resolved above"),
        }
    }

    pub fn memory(config: MemoryRuntimeStateConfig) -> Self {
        Self {
            backend: Arc::new(RuntimeStateBackend::Memory(Box::new(
                MemoryRuntimeBackend::new(config),
            ))),
        }
    }

    pub async fn redis(
        config: RedisClientConfig,
        command_timeout_ms: Option<u64>,
    ) -> Result<Self, DataLayerError> {
        Self::redis_with_blocking_stream_lanes(config, command_timeout_ms, None).await
    }

    pub async fn redis_with_blocking_stream_lanes(
        config: RedisClientConfig,
        command_timeout_ms: Option<u64>,
        blocking_stream_lanes: Option<usize>,
    ) -> Result<Self, DataLayerError> {
        let factory = redis::RedisClientFactory::new(config)?;
        let keyspace = factory.config().keyspace();
        let connections = factory
            .connect_router_with_blocking_stream_lanes(command_timeout_ms, blocking_stream_lanes)
            .await?;
        let runtime = redis::RedisRuntimeRunner::new(
            connections.clone(),
            keyspace.clone(),
            command_timeout_ms,
        );
        runtime.ping().await?;
        let kv = RedisKvRunner::new(
            connections.clone(),
            keyspace.clone(),
            RedisKvRunnerConfig {
                command_timeout_ms,
                default_ttl_seconds: DEFAULT_KV_TTL_SECONDS,
            },
        )?;
        let lock = RedisLockRunner::new(
            connections.clone(),
            keyspace.clone(),
            RedisLockRunnerConfig {
                command_timeout_ms,
                ..RedisLockRunnerConfig::default()
            },
        )?;
        let stream = RedisStreamRunner::new(
            connections,
            keyspace.clone(),
            RedisStreamRunnerConfig {
                command_timeout_ms,
                read_block_ms: None,
                ..RedisStreamRunnerConfig::default()
            },
        )?;
        Ok(Self {
            backend: Arc::new(RuntimeStateBackend::Redis(Box::new(RedisRuntimeBackend {
                keyspace,
                kv,
                lock,
                stream,
                runtime,
                command_timeout_ms,
            }))),
        })
    }

    pub fn backend_kind(&self) -> RuntimeStateBackendKind {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(_) => RuntimeStateBackendKind::Memory,
            RuntimeStateBackend::Redis(_) => RuntimeStateBackendKind::Redis,
        }
    }

    pub fn is_memory(&self) -> bool {
        matches!(self.backend_kind(), RuntimeStateBackendKind::Memory)
    }

    pub fn is_redis(&self) -> bool {
        matches!(self.backend_kind(), RuntimeStateBackendKind::Redis)
    }

    pub async fn redis_diagnostics(
        &self,
    ) -> Result<Option<RedisRuntimeDiagnostics>, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(_) => Ok(None),
            RuntimeStateBackend::Redis(redis) => Ok(Some(redis.runtime.diagnostics().await?)),
        }
    }

    pub fn kv_set_local_nowait(&self, key: &str, value: String, ttl: Option<Duration>) -> bool {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => memory.kv_set_nowait(key, value, ttl),
            RuntimeStateBackend::Redis(_) => false,
        }
    }

    pub fn set_add_local_nowait(&self, key: &str, member: &str) -> bool {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => memory.set_add_nowait(key, member),
            RuntimeStateBackend::Redis(_) => false,
        }
    }

    pub fn namespace_key(&self, raw_key: &str) -> String {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(_) => raw_key.to_string(),
            RuntimeStateBackend::Redis(redis) => redis.keyspace.key(raw_key),
        }
    }

    pub fn strip_namespace<'a>(&self, namespaced_key: &'a str) -> &'a str {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(_) => namespaced_key,
            RuntimeStateBackend::Redis(redis) => {
                let probe = redis.keyspace.key("");
                let prefix = probe.trim_end_matches(':');
                namespaced_key
                    .strip_prefix(prefix)
                    .and_then(|value| value.strip_prefix(':'))
                    .unwrap_or(namespaced_key)
            }
        }
    }

    pub async fn kv_set(
        &self,
        key: &str,
        value: impl Into<String> + Send,
        ttl: Option<Duration>,
    ) -> Result<(), DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory.kv_set(key, value.into(), ttl).await;
                Ok(())
            }
            RuntimeStateBackend::Redis(redis) => {
                let value = value.into();
                if let Some(ttl) = ttl {
                    redis.runtime.kv_set_with_ttl(key, value, ttl).await?;
                } else {
                    redis.runtime.kv_set_plain(key, value).await?;
                }
                Ok(())
            }
        }
    }

    /// Atomically creates an expiring key without replacing an existing value.
    pub async fn kv_set_if_absent(
        &self,
        key: &str,
        value: impl Into<String> + Send,
        ttl: Duration,
    ) -> Result<bool, DataLayerError> {
        if ttl.is_zero() {
            return Err(DataLayerError::InvalidInput(
                "runtime kv set-if-absent ttl must be positive".to_string(),
            ));
        }
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                Ok(memory.kv_set_if_absent(key, value.into(), ttl).await)
            }
            RuntimeStateBackend::Redis(redis) => {
                redis.runtime.kv_set_if_absent(key, value.into(), ttl).await
            }
        }
    }

    pub async fn kv_get(&self, key: &str) -> Result<Option<String>, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.kv_get(key).await),
            RuntimeStateBackend::Redis(redis) => redis.kv.get(key).await,
        }
    }

    pub async fn kv_get_many(
        &self,
        keys: &[String],
    ) -> Result<Vec<Option<String>>, DataLayerError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                let mut values = Vec::with_capacity(keys.len());
                for key in keys {
                    values.push(memory.kv_get(key).await);
                }
                Ok(values)
            }
            RuntimeStateBackend::Redis(redis) => redis.runtime.kv_get_many(keys).await,
        }
    }

    pub async fn kv_take(&self, key: &str) -> Result<Option<String>, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.kv_take(key).await),
            RuntimeStateBackend::Redis(redis) => redis.kv.getdel(key).await,
        }
    }

    pub async fn kv_delete(&self, key: &str) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.kv_delete(key).await),
            RuntimeStateBackend::Redis(redis) => Ok(redis.kv.del(key).await? > 0),
        }
    }

    pub async fn kv_delete_many(&self, keys: &[String]) -> Result<usize, DataLayerError> {
        if keys.is_empty() {
            return Ok(0);
        }
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.kv_delete_many(keys).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.kv_delete_many(keys).await,
        }
    }

    pub async fn kv_exists(&self, key: &str) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.kv_exists(key).await),
            RuntimeStateBackend::Redis(redis) => redis.kv.exists(key).await,
        }
    }

    pub async fn kv_ttl_seconds(&self, key: &str) -> Result<Option<i64>, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.kv_ttl_seconds(key).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.kv_ttl_seconds(key).await,
        }
    }

    pub async fn scan_keys(
        &self,
        pattern: &str,
        count: usize,
    ) -> Result<Vec<String>, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.kv_scan(pattern).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.scan_keys(pattern, count).await,
        }
    }

    pub async fn check_and_consume_rate_limit(
        &self,
        input: RateLimitInput<'_>,
    ) -> Result<RateLimitCheck, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory
                    .check_and_consume_rate_limit(
                        input.user_key,
                        input.key_key,
                        input.bucket,
                        input.user_limit,
                        input.key_limit,
                        Duration::from_secs(input.ttl_seconds.max(1)),
                    )
                    .await
            }
            RuntimeStateBackend::Redis(redis) => {
                redis.runtime.check_and_consume_rate_limit(input).await
            }
        }
    }

    pub async fn check_and_consume_usage_limits(
        &self,
        input: UsageLimitInput<'_>,
    ) -> Result<UsageLimitCheck, DataLayerError> {
        if input.rules.is_empty() {
            return Ok(UsageLimitCheck::Allowed);
        }
        validate_usage_limit_input(input)?;
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory.check_and_consume_usage_limits(input).await
            }
            RuntimeStateBackend::Redis(redis) => {
                redis.runtime.check_and_consume_usage_limits(input).await
            }
        }
    }

    /// Removes an idempotency event from every supplied usage-limit window.
    ///
    /// This is a compensation primitive for callers that compose the short-lived runtime
    /// counters with a second durable admission store. It is intentionally idempotent.
    pub async fn release_usage_limits(
        &self,
        input: UsageLimitReleaseInput<'_>,
    ) -> Result<(), DataLayerError> {
        if input.rules.is_empty() {
            return Ok(());
        }
        validate_usage_limit_release_input(input)?;
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => memory.release_usage_limits(input).await,
            RuntimeStateBackend::Redis(redis) => redis.runtime.release_usage_limits(input).await,
        }
    }

    pub async fn rate_limit_count(&self, key: &str, bucket: u64) -> Result<u32, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => memory.rate_limit_count(key, bucket),
            RuntimeStateBackend::Redis(redis) => Ok(redis
                .kv
                .get(key)
                .await?
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or_default()),
        }
    }

    pub async fn set_add(&self, key: &str, member: &str) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.set_add(key, member).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.set_add(key, member).await,
        }
    }

    pub async fn set_remove(&self, key: &str, member: &str) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.set_remove(key, member).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.set_remove(key, member).await,
        }
    }

    pub async fn set_members(&self, key: &str) -> Result<Vec<String>, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.set_members(key).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.set_members(key).await,
        }
    }

    pub async fn set_len(&self, key: &str) -> Result<usize, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.set_len(key).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.set_len(key).await,
        }
    }

    pub async fn score_set(
        &self,
        key: &str,
        member: &str,
        score: f64,
    ) -> Result<(), DataLayerError> {
        if !score.is_finite() {
            return Err(DataLayerError::InvalidInput(
                "runtime score must be finite".to_string(),
            ));
        }
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory.score_set(key, member, score).await;
                Ok(())
            }
            RuntimeStateBackend::Redis(redis) => redis.runtime.score_set(key, member, score).await,
        }
    }

    pub async fn score_many(
        &self,
        key: &str,
        members: &[String],
    ) -> Result<Vec<Option<f64>>, DataLayerError> {
        if members.is_empty() {
            return Ok(Vec::new());
        }
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.score_many(key, members).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.score_many(key, members).await,
        }
    }

    pub async fn score_range_by_min(
        &self,
        key: &str,
        min_score: f64,
    ) -> Result<Vec<String>, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                Ok(memory.score_range_by_min(key, min_score).await)
            }
            RuntimeStateBackend::Redis(redis) => {
                redis.runtime.score_range_by_min(key, min_score).await
            }
        }
    }

    /// Aggregate at most 512 timestamped `prefix:u64` members per key without
    /// transferring their history. `None` requires an exact full-range fallback;
    /// it never represents an empty or cached window.
    pub async fn score_window_u64_stats_by_min(
        &self,
        keys: &[String],
        min_score: f64,
    ) -> Result<Vec<Option<ScoreWindowU64Stats>>, DataLayerError> {
        if !min_score.is_finite() {
            return Err(DataLayerError::InvalidInput(
                "runtime window minimum score must be finite".to_string(),
            ));
        }
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                Ok(memory.score_window_u64_stats_by_min(keys, min_score).await)
            }
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .runtime
                    .score_window_u64_stats_by_min(keys, min_score)
                    .await
            }
        }
    }

    pub async fn score_remove_by_score(
        &self,
        key: &str,
        max_score: f64,
    ) -> Result<usize, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                Ok(memory.score_remove_by_score(key, max_score).await)
            }
            RuntimeStateBackend::Redis(redis) => {
                redis.runtime.score_remove_by_score(key, max_score).await
            }
        }
    }

    pub async fn score_remove(&self, key: &str, member: &str) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.score_remove(key, member).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.score_remove(key, member).await,
        }
    }

    pub async fn score_remove_by_rank(
        &self,
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<usize, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                Ok(memory.score_remove_by_rank(key, start, stop).await)
            }
            RuntimeStateBackend::Redis(redis) => {
                redis.runtime.score_remove_by_rank(key, start, stop).await
            }
        }
    }

    pub async fn score_len(&self, key: &str) -> Result<usize, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.score_len(key).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.score_len(key).await,
        }
    }

    pub async fn key_expire(&self, key: &str, ttl: Duration) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.key_expire(key, ttl).await),
            RuntimeStateBackend::Redis(redis) => redis.runtime.key_expire(key, ttl).await,
        }
    }

    pub async fn lock_try_acquire(
        &self,
        key: &str,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<RuntimeLockLease>, DataLayerError> {
        if owner.trim().is_empty() || key.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "runtime lock key and owner cannot be empty".to_string(),
            ));
        }
        if ttl.is_zero() {
            return Err(DataLayerError::InvalidInput(
                "runtime lock ttl must be positive".to_string(),
            ));
        }
        let token = format!("{owner}:{}", Uuid::new_v4());
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                if let Some(fencing_token) = memory
                    .lock_try_acquire(key, owner, token.clone(), ttl)
                    .await
                {
                    Ok(Some(RuntimeLockLease {
                        key: key.to_string(),
                        owner: owner.to_string(),
                        token,
                        fencing_token,
                        ttl_ms: ttl.as_millis().try_into().unwrap_or(u64::MAX),
                    }))
                } else {
                    Ok(None)
                }
            }
            RuntimeStateBackend::Redis(redis) => {
                let lease = redis
                    .lock
                    .try_acquire(
                        &redis.keyspace.lock_key(key),
                        owner,
                        Some(ttl.as_millis().try_into().unwrap_or(u64::MAX)),
                    )
                    .await?;
                Ok(lease.map(|lease| RuntimeLockLease {
                    key: key.to_string(),
                    owner: lease.owner,
                    token: lease.token,
                    fencing_token: lease.fencing_token,
                    ttl_ms: lease.ttl_ms,
                }))
            }
        }
    }

    pub async fn lock_release(&self, lease: &RuntimeLockLease) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                Ok(memory.lock_release(&lease.key, &lease.token).await)
            }
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .lock
                    .release(&RedisLockLease {
                        key: redis.keyspace.lock_key(&lease.key),
                        owner: lease.owner.clone(),
                        token: lease.token.clone(),
                        fencing_token: lease.fencing_token,
                        ttl_ms: lease.ttl_ms,
                    })
                    .await
            }
        }
    }

    pub async fn lock_renew(
        &self,
        lease: &RuntimeLockLease,
        ttl: Duration,
    ) -> Result<bool, DataLayerError> {
        if ttl.is_zero() {
            return Err(DataLayerError::InvalidInput(
                "runtime lock ttl must be positive".to_string(),
            ));
        }
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                Ok(memory.lock_renew(&lease.key, &lease.token, ttl).await)
            }
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .lock
                    .renew(
                        &RedisLockLease {
                            key: redis.keyspace.lock_key(&lease.key),
                            owner: lease.owner.clone(),
                            token: lease.token.clone(),
                            fencing_token: lease.fencing_token,
                            ttl_ms: lease.ttl_ms,
                        },
                        Some(ttl.as_millis().try_into().unwrap_or(u64::MAX)),
                    )
                    .await
            }
        }
    }

    pub fn semaphore(
        &self,
        gate: &'static str,
        limit: usize,
        config: RuntimeSemaphoreConfig,
    ) -> Result<RuntimeSemaphore, RuntimeSemaphoreError> {
        RuntimeSemaphore::new(self.clone(), gate, None, limit, config)
    }

    pub fn keyed_semaphore<K: Into<String>>(
        &self,
        gate: &'static str,
        resource_key: K,
        limit: usize,
        config: RuntimeSemaphoreConfig,
    ) -> Result<RuntimeSemaphore, RuntimeSemaphoreError> {
        let resource_key = resource_key.into();
        let prefix = format!("admission:{gate}:");
        let resource_key = resource_key
            .strip_prefix(&prefix)
            .unwrap_or(resource_key.as_str());
        if resource_key.is_empty() {
            return Err(RuntimeSemaphoreError::InvalidConfiguration(
                "runtime semaphore resource key cannot be empty".to_string(),
            ));
        }
        RuntimeSemaphore::new(self.clone(), gate, Some(resource_key), limit, config)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLockLease {
    pub key: String,
    pub owner: String,
    pub token: String,
    pub fencing_token: u64,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitScope {
    User,
    Key,
}

impl RateLimitScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Key => "key",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitCheck {
    Allowed { remaining: u32 },
    Rejected { scope: RateLimitScope, limit: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitInput<'a> {
    pub user_key: &'a str,
    pub key_key: &'a str,
    pub bucket: u64,
    pub user_limit: u32,
    pub key_limit: u32,
    pub ttl_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageLimitRule<'a> {
    /// A caller-defined key containing the same non-empty Redis hash tag as every sibling rule.
    /// The key must change when the rule's window definition changes.
    pub key: &'a str,
    pub limit: u64,
    /// Duration used to decide which events still count toward the limit.
    pub window_seconds: u64,
    /// Duration for retaining this rule's backing state after the current check.
    pub retention_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageLimitInput<'a> {
    pub rules: &'a [UsageLimitRule<'a>],
    pub event_id: &'a str,
    pub now_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageLimitReleaseInput<'a> {
    pub rules: &'a [UsageLimitRule<'a>],
    pub event_id: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageLimitCheck {
    Allowed,
    Rejected {
        rule_index: usize,
        limit: u64,
        retry_after: u64,
    },
}

const MAX_REDIS_LUA_EXACT_INTEGER: u64 = (1_u64 << 53) - 1;

fn validate_usage_limit_input(input: UsageLimitInput<'_>) -> Result<(), DataLayerError> {
    if input.event_id.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "usage limit event_id must not be empty".to_string(),
        ));
    }
    if input.now_unix_ms > MAX_REDIS_LUA_EXACT_INTEGER {
        return Err(DataLayerError::InvalidInput(
            "usage limit now_unix_ms exceeds the exact Redis Lua integer range".to_string(),
        ));
    }

    let mut expected_hash_tag = None;
    for (index, rule) in input.rules.iter().enumerate() {
        if rule.key.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} key must not be empty"
            )));
        }
        if rule.limit == 0 {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} must have a positive limit"
            )));
        }
        if rule.limit > MAX_REDIS_LUA_EXACT_INTEGER {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} limit exceeds the exact Redis Lua integer range"
            )));
        }
        if rule.window_seconds == 0 {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} must have a positive window_seconds"
            )));
        }
        let window_ms = rule.window_seconds.checked_mul(1_000).ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "usage limit rule {index} window_seconds is too large"
            ))
        })?;
        if window_ms > MAX_REDIS_LUA_EXACT_INTEGER {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} window_seconds exceeds the exact Redis Lua integer range"
            )));
        }
        if rule.retention_seconds == 0 {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} must have a positive retention_seconds"
            )));
        }
        let retention_ms = rule.retention_seconds.checked_mul(1_000).ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "usage limit rule {index} retention_seconds is too large"
            ))
        })?;
        if retention_ms > MAX_REDIS_LUA_EXACT_INTEGER {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} retention_seconds exceeds the exact Redis Lua integer range"
            )));
        }
        if input
            .now_unix_ms
            .checked_add(window_ms)
            .is_none_or(|expires_at| expires_at > MAX_REDIS_LUA_EXACT_INTEGER)
        {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} window extends beyond the exact Redis Lua integer range"
            )));
        }
        if input
            .now_unix_ms
            .checked_add(retention_ms)
            .is_none_or(|expires_at| expires_at > MAX_REDIS_LUA_EXACT_INTEGER)
        {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} retention extends beyond the exact Redis Lua integer range"
            )));
        }
        if input.rules[..index]
            .iter()
            .any(|previous| previous.key == rule.key)
        {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} duplicates key {}",
                rule.key
            )));
        }

        let hash_tag = redis_hash_tag(rule.key).ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "usage limit rule {index} key must contain a non-empty Redis hash tag"
            ))
        })?;
        if let Some(expected) = expected_hash_tag {
            if hash_tag != expected {
                return Err(DataLayerError::InvalidInput(
                    "usage limit rule keys must use the same Redis hash tag".to_string(),
                ));
            }
        } else {
            expected_hash_tag = Some(hash_tag);
        }
    }
    Ok(())
}

fn validate_usage_limit_release_input(
    input: UsageLimitReleaseInput<'_>,
) -> Result<(), DataLayerError> {
    if input.event_id.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "usage limit event_id must not be empty".to_string(),
        ));
    }
    let mut expected_hash_tag = None;
    for (index, rule) in input.rules.iter().enumerate() {
        if rule.key.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} key must not be empty"
            )));
        }
        if input.rules[..index]
            .iter()
            .any(|previous| previous.key == rule.key)
        {
            return Err(DataLayerError::InvalidInput(format!(
                "usage limit rule {index} duplicates key {}",
                rule.key
            )));
        }
        let hash_tag = redis_hash_tag(rule.key).ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "usage limit rule {index} key must contain a non-empty Redis hash tag"
            ))
        })?;
        if let Some(expected) = expected_hash_tag {
            if hash_tag != expected {
                return Err(DataLayerError::InvalidInput(
                    "usage limit rule keys must use the same Redis hash tag".to_string(),
                ));
            }
        } else {
            expected_hash_tag = Some(hash_tag);
        }
    }
    Ok(())
}

fn redis_hash_tag(key: &str) -> Option<&str> {
    let tag_start = key.find('{')?.saturating_add(1);
    let remainder = key.get(tag_start..)?;
    let tag_end = remainder.find('}')?;
    (tag_end > 0).then_some(&remainder[..tag_end])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeQueueEntry {
    pub id: String,
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeQueueReclaimPage {
    /// Resume the next reclaim scan here; `0-0` marks the end of the current scan.
    pub next_start_id: String,
    pub entries: Vec<RuntimeQueueEntry>,
    pub deleted_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeQueueTransferOutcome {
    Transferred {
        destination_id: String,
        acked: usize,
        deleted: usize,
    },
    /// No pending entry was present. This does not assert that it was archived:
    /// another consumer, deletion, or retention policy may have removed it.
    NotPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeQueueStats {
    pub stream_length: u64,
    pub group_pending: u64,
    pub group_lag: Option<u64>,
    pub oldest_pending_idle_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeQueueReclaimConfig {
    pub min_idle_ms: u64,
    pub count: usize,
}

fn validate_runtime_queue_name(value: &str, field: &str) -> Result<(), DataLayerError> {
    if value.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(format!(
            "{field} cannot be empty"
        )));
    }
    Ok(())
}

fn validate_runtime_queue_reclaim_config(
    config: RuntimeQueueReclaimConfig,
) -> Result<(), DataLayerError> {
    if config.min_idle_ms == 0 {
        return Err(DataLayerError::InvalidInput(
            "runtime queue reclaim min_idle_ms must be positive".to_string(),
        ));
    }
    if config.count == 0 {
        return Err(DataLayerError::InvalidInput(
            "runtime queue reclaim count must be positive".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_runtime_queue_transfer(
    source: &str,
    group: &str,
    entry_id: &str,
    destination: &str,
    destination_fields: &BTreeMap<String, String>,
) -> Result<(), DataLayerError> {
    validate_runtime_queue_name(source, "runtime queue source stream")?;
    validate_runtime_queue_name(group, "runtime queue group")?;
    validate_runtime_queue_name(destination, "runtime queue destination stream")?;
    if source == destination {
        return Err(DataLayerError::InvalidInput(
            "runtime queue transfer source and destination must differ".to_string(),
        ));
    }
    if destination_fields.is_empty() {
        return Err(DataLayerError::InvalidInput(
            "runtime queue transfer destination fields cannot be empty".to_string(),
        ));
    }
    let canonical_u64 = |value: &str| {
        !value.is_empty()
            && value.bytes().all(|byte| byte.is_ascii_digit())
            && (value.len() == 1 || !value.starts_with('0'))
            && value.parse::<u64>().is_ok()
    };
    if !entry_id
        .split_once('-')
        .is_some_and(|(milliseconds, sequence)| {
            canonical_u64(milliseconds) && canonical_u64(sequence)
        })
    {
        return Err(DataLayerError::InvalidInput(
            "runtime queue transfer entry id must be a canonical u64-u64 stream id".to_string(),
        ));
    }
    Ok(())
}

#[async_trait]
pub trait RuntimeQueueStore: Send + Sync {
    async fn ensure_consumer_group(
        &self,
        stream: &str,
        group: &str,
        start_id: &str,
    ) -> Result<(), DataLayerError>;

    async fn append_fields_with_maxlen(
        &self,
        stream: &str,
        fields: &BTreeMap<String, String>,
        maxlen: Option<usize>,
    ) -> Result<String, DataLayerError>;

    async fn read_group(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        count: usize,
        block_ms: Option<u64>,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError>;

    async fn claim_stale(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        start_id: &str,
        config: RuntimeQueueReclaimConfig,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError>;

    /// Existing queue backends can retain their complete-scan behavior without implementing paging.
    async fn claim_stale_page(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        start_id: &str,
        config: RuntimeQueueReclaimConfig,
    ) -> Result<RuntimeQueueReclaimPage, DataLayerError> {
        Ok(RuntimeQueueReclaimPage {
            next_start_id: "0-0".to_string(),
            entries: self
                .claim_stale(stream, group, consumer, start_id, config)
                .await?,
            deleted_ids: Vec::new(),
        })
    }

    /// Atomically append caller-supplied fields, acknowledge the pending source entry, and
    /// delete that source ID. Repeated calls must not append when the entry is no longer pending.
    /// `None` means unsupported and has no side effects; callers may explicitly retain their
    /// existing non-atomic fallback for third-party queue implementations.
    async fn try_transfer_pending_to_stream(
        &self,
        _source: &str,
        _group: &str,
        _entry_id: &str,
        _destination: &str,
        _destination_fields: &BTreeMap<String, String>,
    ) -> Result<Option<RuntimeQueueTransferOutcome>, DataLayerError> {
        Ok(None)
    }

    async fn ack(&self, stream: &str, group: &str, ids: &[String])
        -> Result<usize, DataLayerError>;

    async fn delete(&self, stream: &str, ids: &[String]) -> Result<usize, DataLayerError>;

    async fn stats(
        &self,
        stream: &str,
        group: Option<&str>,
    ) -> Result<RuntimeQueueStats, DataLayerError>;
}

#[async_trait]
impl RuntimeQueueStore for RuntimeState {
    async fn ensure_consumer_group(
        &self,
        stream: &str,
        group: &str,
        start_id: &str,
    ) -> Result<(), DataLayerError> {
        validate_runtime_queue_name(stream, "runtime queue stream")?;
        validate_runtime_queue_name(group, "runtime queue group")?;
        validate_runtime_queue_name(start_id, "runtime queue start id")?;
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory
                    .queue_ensure_consumer_group(stream, group, start_id)
                    .await
            }
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .stream
                    .ensure_consumer_group(
                        &RedisStreamName(stream.to_string()),
                        &RedisConsumerGroup(group.to_string()),
                        start_id,
                    )
                    .await
            }
        }
    }

    async fn append_fields_with_maxlen(
        &self,
        stream: &str,
        fields: &BTreeMap<String, String>,
        maxlen: Option<usize>,
    ) -> Result<String, DataLayerError> {
        validate_runtime_queue_name(stream, "runtime queue stream")?;
        if fields.is_empty() {
            return Err(DataLayerError::InvalidInput(
                "runtime queue fields cannot be empty".to_string(),
            ));
        }
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                Ok(memory.queue_append(stream, fields.clone(), maxlen).await)
            }
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .stream
                    .append_fields_with_maxlen(&RedisStreamName(stream.to_string()), fields, maxlen)
                    .await
            }
        }
    }

    async fn read_group(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        count: usize,
        block_ms: Option<u64>,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
        validate_runtime_queue_name(stream, "runtime queue stream")?;
        validate_runtime_queue_name(group, "runtime queue group")?;
        validate_runtime_queue_name(consumer, "runtime queue consumer")?;
        if matches!(block_ms, Some(0)) {
            return Err(DataLayerError::InvalidInput(
                "runtime queue block_ms must be positive".to_string(),
            ));
        }
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory
                    .queue_read(stream, group, consumer, count, block_ms)
                    .await
            }
            RuntimeStateBackend::Redis(redis) => {
                let runner = redis.stream.with_config(RedisStreamRunnerConfig {
                    command_timeout_ms: redis_stream_command_timeout_for_block(
                        redis.command_timeout_ms,
                        block_ms,
                    ),
                    read_block_ms: block_ms,
                    read_count: count.max(1),
                })?;
                Ok(runner
                    .read_group(
                        &RedisStreamName(stream.to_string()),
                        &RedisConsumerGroup(group.to_string()),
                        &RedisConsumerName(consumer.to_string()),
                    )
                    .await?
                    .into_iter()
                    .map(|entry| RuntimeQueueEntry {
                        id: entry.id,
                        fields: entry.fields,
                    })
                    .collect())
            }
        }
    }

    async fn claim_stale(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        start_id: &str,
        config: RuntimeQueueReclaimConfig,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
        Ok(self
            .claim_stale_page(stream, group, consumer, start_id, config)
            .await?
            .entries)
    }

    async fn claim_stale_page(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        start_id: &str,
        config: RuntimeQueueReclaimConfig,
    ) -> Result<RuntimeQueueReclaimPage, DataLayerError> {
        validate_runtime_queue_name(stream, "runtime queue stream")?;
        validate_runtime_queue_name(group, "runtime queue group")?;
        validate_runtime_queue_name(consumer, "runtime queue consumer")?;
        validate_runtime_queue_name(start_id, "runtime queue start id")?;
        validate_runtime_queue_reclaim_config(config)?;
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory
                    .queue_claim_stale_page(stream, group, consumer, start_id, config)
                    .await
            }
            RuntimeStateBackend::Redis(redis) => {
                let page = redis
                    .stream
                    .claim_stale(
                        &RedisStreamName(stream.to_string()),
                        &RedisConsumerGroup(group.to_string()),
                        &RedisConsumerName(consumer.to_string()),
                        start_id,
                        RedisStreamReclaimConfig {
                            min_idle_ms: config.min_idle_ms,
                            count: config.count,
                        },
                    )
                    .await?;
                Ok(RuntimeQueueReclaimPage {
                    next_start_id: page.next_start_id,
                    entries: page
                        .entries
                        .into_iter()
                        .map(|entry| RuntimeQueueEntry {
                            id: entry.id,
                            fields: entry.fields,
                        })
                        .collect(),
                    deleted_ids: page.deleted_ids,
                })
            }
        }
    }

    async fn try_transfer_pending_to_stream(
        &self,
        source: &str,
        group: &str,
        entry_id: &str,
        destination: &str,
        destination_fields: &BTreeMap<String, String>,
    ) -> Result<Option<RuntimeQueueTransferOutcome>, DataLayerError> {
        validate_runtime_queue_transfer(source, group, entry_id, destination, destination_fields)?;
        let outcome = match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory
                    .queue_transfer_pending_to_stream(
                        source,
                        group,
                        entry_id,
                        destination,
                        destination_fields,
                    )
                    .await?
            }
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .stream
                    .try_transfer_pending_to_stream(
                        source,
                        group,
                        entry_id,
                        destination,
                        destination_fields,
                    )
                    .await?
            }
        };
        Ok(Some(outcome))
    }

    async fn ack(
        &self,
        stream: &str,
        group: &str,
        ids: &[String],
    ) -> Result<usize, DataLayerError> {
        validate_runtime_queue_name(stream, "runtime queue stream")?;
        validate_runtime_queue_name(group, "runtime queue group")?;
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => memory.queue_ack(stream, group, ids).await,
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .stream
                    .ack(
                        &RedisStreamName(stream.to_string()),
                        &RedisConsumerGroup(group.to_string()),
                        ids,
                    )
                    .await
            }
        }
    }

    async fn delete(&self, stream: &str, ids: &[String]) -> Result<usize, DataLayerError> {
        validate_runtime_queue_name(stream, "runtime queue stream")?;
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.queue_delete(stream, ids).await),
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .stream
                    .delete(&RedisStreamName(stream.to_string()), ids)
                    .await
            }
        }
    }

    async fn stats(
        &self,
        stream: &str,
        group: Option<&str>,
    ) -> Result<RuntimeQueueStats, DataLayerError> {
        validate_runtime_queue_name(stream, "runtime queue stream")?;
        if let Some(group) = group {
            validate_runtime_queue_name(group, "runtime queue group")?;
        }
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.queue_stats(stream, group).await),
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .stream
                    .stats(
                        &RedisStreamName(stream.to_string()),
                        group
                            .map(|value| RedisConsumerGroup(value.to_string()))
                            .as_ref(),
                    )
                    .await
            }
        }
    }
}

#[async_trait]
pub trait ExpiringKvStore: Send + Sync {
    async fn set(
        &self,
        key: &str,
        value: String,
        ttl: Option<Duration>,
    ) -> Result<(), DataLayerError>;
    async fn set_if_absent(
        &self,
        key: &str,
        value: String,
        ttl: Duration,
    ) -> Result<bool, DataLayerError>;
    async fn get(&self, key: &str) -> Result<Option<String>, DataLayerError>;
    async fn get_many(&self, keys: &[String]) -> Result<Vec<Option<String>>, DataLayerError>;
    async fn take(&self, key: &str) -> Result<Option<String>, DataLayerError>;
    async fn delete(&self, key: &str) -> Result<bool, DataLayerError>;
    async fn exists(&self, key: &str) -> Result<bool, DataLayerError>;
}

#[async_trait]
impl ExpiringKvStore for RuntimeState {
    async fn set(
        &self,
        key: &str,
        value: String,
        ttl: Option<Duration>,
    ) -> Result<(), DataLayerError> {
        self.kv_set(key, value, ttl).await
    }

    async fn set_if_absent(
        &self,
        key: &str,
        value: String,
        ttl: Duration,
    ) -> Result<bool, DataLayerError> {
        self.kv_set_if_absent(key, value, ttl).await
    }

    async fn get(&self, key: &str) -> Result<Option<String>, DataLayerError> {
        self.kv_get(key).await
    }

    async fn get_many(&self, keys: &[String]) -> Result<Vec<Option<String>>, DataLayerError> {
        self.kv_get_many(keys).await
    }

    async fn take(&self, key: &str) -> Result<Option<String>, DataLayerError> {
        self.kv_take(key).await
    }

    async fn delete(&self, key: &str) -> Result<bool, DataLayerError> {
        self.kv_delete(key).await
    }

    async fn exists(&self, key: &str) -> Result<bool, DataLayerError> {
        self.kv_exists(key).await
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RuntimeSemaphoreError {
    #[error("runtime semaphore {gate} is saturated at {limit}")]
    Saturated { gate: &'static str, limit: usize },
    #[error("runtime semaphore {gate} is unavailable: {message}")]
    Unavailable {
        gate: &'static str,
        limit: usize,
        message: String,
    },
    #[error("{0}")]
    InvalidConfiguration(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSemaphoreSnapshot {
    pub limit: usize,
    pub in_flight: usize,
    pub available_permits: usize,
    pub high_watermark: usize,
    pub rejected: u64,
}

impl RuntimeSemaphoreSnapshot {
    pub fn to_metric_samples(&self, gate: &'static str) -> Vec<aether_runtime::MetricSample> {
        let labels = vec![aether_runtime::MetricLabel::new("gate", gate)];
        vec![
            aether_runtime::MetricSample::new(
                "concurrency_in_flight",
                "Current number of in-flight operations guarded by the concurrency gate.",
                aether_runtime::MetricKind::Gauge,
                self.in_flight as u64,
            )
            .with_labels(labels.clone()),
            aether_runtime::MetricSample::new(
                "concurrency_available_permits",
                "Currently available permits for the concurrency gate.",
                aether_runtime::MetricKind::Gauge,
                self.available_permits as u64,
            )
            .with_labels(labels.clone()),
            aether_runtime::MetricSample::new(
                "concurrency_high_watermark",
                "Highest observed in-flight count for the concurrency gate.",
                aether_runtime::MetricKind::Gauge,
                self.high_watermark as u64,
            )
            .with_labels(labels.clone()),
            aether_runtime::MetricSample::new(
                "concurrency_rejected_total",
                "Number of operations rejected by the concurrency gate.",
                aether_runtime::MetricKind::Counter,
                self.rejected,
            )
            .with_labels(labels),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSemaphoreConfig {
    pub lease_ttl_ms: u64,
    pub renew_interval_ms: u64,
    pub command_timeout_ms: Option<u64>,
}

impl Default for RuntimeSemaphoreConfig {
    fn default() -> Self {
        Self {
            lease_ttl_ms: 30_000,
            renew_interval_ms: 10_000,
            command_timeout_ms: Some(DEFAULT_COMMAND_TIMEOUT_MS),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeSemaphore {
    state: Arc<RuntimeSemaphoreState>,
}

#[derive(Debug)]
struct RuntimeSemaphoreState {
    runtime: RuntimeState,
    gate: &'static str,
    limit: usize,
    key: String,
    config: RuntimeSemaphoreConfig,
    high_watermark: AtomicUsize,
    rejected: AtomicU64,
}

impl RuntimeSemaphore {
    fn new(
        runtime: RuntimeState,
        gate: &'static str,
        resource_key: Option<&str>,
        limit: usize,
        config: RuntimeSemaphoreConfig,
    ) -> Result<Self, RuntimeSemaphoreError> {
        if limit == 0 {
            return Err(RuntimeSemaphoreError::InvalidConfiguration(
                "runtime semaphore limit must be positive".to_string(),
            ));
        }
        if config.lease_ttl_ms == 0 || config.renew_interval_ms == 0 {
            return Err(RuntimeSemaphoreError::InvalidConfiguration(
                "runtime semaphore lease and renew intervals must be positive".to_string(),
            ));
        }
        if config.renew_interval_ms >= config.lease_ttl_ms {
            return Err(RuntimeSemaphoreError::InvalidConfiguration(
                "runtime semaphore renew_interval_ms must be smaller than lease_ttl_ms".to_string(),
            ));
        }
        Ok(Self {
            state: Arc::new(RuntimeSemaphoreState {
                key: resource_key
                    .map(|resource_key| format!("admission:{gate}:{resource_key}"))
                    .unwrap_or_else(|| format!("admission:{gate}")),
                runtime,
                gate,
                limit,
                config,
                high_watermark: AtomicUsize::new(0),
                rejected: AtomicU64::new(0),
            }),
        })
    }

    pub fn gate(&self) -> &'static str {
        self.state.gate
    }

    pub fn limit(&self) -> usize {
        self.state.limit
    }

    pub async fn try_acquire(&self) -> Result<RuntimeSemaphorePermit, RuntimeSemaphoreError> {
        self.state.try_acquire().await
    }

    pub async fn snapshot(&self) -> Result<RuntimeSemaphoreSnapshot, RuntimeSemaphoreError> {
        self.state.snapshot().await
    }
}

#[derive(Debug)]
pub struct RuntimeSemaphorePermit {
    state: Arc<RuntimeSemaphoreState>,
    token: String,
    renew_task: JoinHandle<()>,
    healthy: Arc<std::sync::atomic::AtomicBool>,
    released: bool,
}

impl aether_runtime::AdmissionPermitHealth for RuntimeSemaphorePermit {
    fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }
}

impl RuntimeSemaphorePermit {
    pub async fn release(mut self) -> Result<(), RuntimeSemaphoreError> {
        self.renew_task.abort();
        let result = self.state.release(&self.token).await;
        if result.is_ok() {
            self.released = true;
        }
        result
    }
}

impl Drop for RuntimeSemaphorePermit {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.renew_task.abort();
        let state = Arc::clone(&self.state);
        let token = self.token.clone();
        tokio::spawn(async move {
            if let Err(err) = state.release(&token).await {
                warn!(
                    gate = state.gate,
                    error = %err,
                    "failed to release runtime semaphore permit"
                );
            }
        });
    }
}

impl RuntimeSemaphoreState {
    async fn try_acquire(
        self: &Arc<Self>,
    ) -> Result<RuntimeSemaphorePermit, RuntimeSemaphoreError> {
        let token = format!("{}:{}", self.gate, Uuid::new_v4());
        let in_flight = match self.runtime.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => memory
                .semaphore_try_acquire(
                    &self.key,
                    token.clone(),
                    self.limit,
                    self.config.lease_ttl_ms,
                )
                .await
                .map_err(|count| {
                    self.rejected.fetch_add(1, Ordering::Relaxed);
                    self.observe_in_flight(count);
                    RuntimeSemaphoreError::Saturated {
                        gate: self.gate,
                        limit: self.limit,
                    }
                })?,
            RuntimeStateBackend::Redis(redis) => self.redis_try_acquire(redis, &token).await?,
        };
        self.observe_in_flight(in_flight);

        let renew_state = Arc::clone(self);
        let renew_token = token.clone();
        let healthy = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let renew_health = Arc::clone(&healthy);
        let renew_task = tokio::spawn(async move {
            let interval = Duration::from_millis(renew_state.config.renew_interval_ms);
            loop {
                tokio::time::sleep(interval).await;
                if let Err(err) = renew_state.renew(&renew_token).await {
                    renew_health.store(false, Ordering::Release);
                    warn!(
                        gate = renew_state.gate,
                        error = %err,
                        "failed to renew runtime semaphore permit"
                    );
                    break;
                }
            }
        });
        Ok(RuntimeSemaphorePermit {
            state: Arc::clone(self),
            token,
            renew_task,
            healthy,
            released: false,
        })
    }

    async fn snapshot(&self) -> Result<RuntimeSemaphoreSnapshot, RuntimeSemaphoreError> {
        let in_flight = self.live_count().await?;
        Ok(RuntimeSemaphoreSnapshot {
            limit: self.limit,
            in_flight,
            available_permits: self.limit.saturating_sub(in_flight),
            high_watermark: self.high_watermark.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
        })
    }

    async fn redis_try_acquire(
        &self,
        redis: &RedisRuntimeBackend,
        token: &str,
    ) -> Result<usize, RuntimeSemaphoreError> {
        let result = redis
            .runtime
            .semaphore_try_acquire(
                self.gate,
                self.limit,
                &self.key,
                token,
                self.config.lease_ttl_ms,
                self.config.command_timeout_ms,
            )
            .await?;
        let acquired = result.0 > 0;
        let in_flight = result.1.max(0) as usize;
        if !acquired {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            self.observe_in_flight(in_flight);
            return Err(RuntimeSemaphoreError::Saturated {
                gate: self.gate,
                limit: self.limit,
            });
        }
        Ok(in_flight)
    }

    async fn renew(&self, token: &str) -> Result<(), RuntimeSemaphoreError> {
        match self.runtime.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                if memory
                    .semaphore_renew(&self.key, token, self.config.lease_ttl_ms)
                    .await
                {
                    Ok(())
                } else {
                    Err(self.unavailable("lease token expired".to_string()))
                }
            }
            RuntimeStateBackend::Redis(redis) => {
                let renewed = redis
                    .runtime
                    .semaphore_renew(
                        self.gate,
                        self.limit,
                        &self.key,
                        token,
                        self.config.lease_ttl_ms,
                        self.config.command_timeout_ms,
                    )
                    .await?;
                if renewed == 0 {
                    return Err(self.unavailable("lease token expired".to_string()));
                }
                Ok(())
            }
        }
    }

    async fn release(&self, token: &str) -> Result<(), RuntimeSemaphoreError> {
        match self.runtime.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory.semaphore_release(&self.key, token).await;
                Ok(())
            }
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .runtime
                    .semaphore_release(
                        self.gate,
                        self.limit,
                        &self.key,
                        token,
                        self.config.command_timeout_ms,
                    )
                    .await
            }
        }
    }

    async fn live_count(&self) -> Result<usize, RuntimeSemaphoreError> {
        let count = match self.runtime.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => memory.semaphore_live_count(&self.key).await,
            RuntimeStateBackend::Redis(redis) => {
                redis
                    .runtime
                    .semaphore_live_count(
                        self.gate,
                        self.limit,
                        &self.key,
                        self.config.command_timeout_ms,
                    )
                    .await?
            }
        };
        self.observe_in_flight(count);
        Ok(count)
    }

    fn unavailable(&self, message: String) -> RuntimeSemaphoreError {
        RuntimeSemaphoreError::Unavailable {
            gate: self.gate,
            limit: self.limit,
            message,
        }
    }

    fn observe_in_flight(&self, in_flight: usize) {
        let mut observed = self.high_watermark.load(Ordering::Acquire);
        while in_flight > observed {
            match self.high_watermark.compare_exchange_weak(
                observed,
                in_flight,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(next) => observed = next,
            }
        }
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn redis_stream_command_timeout_for_block(
    command_timeout_ms: Option<u64>,
    read_block_ms: Option<u64>,
) -> Option<u64> {
    match (command_timeout_ms, read_block_ms) {
        (Some(timeout_ms), Some(block_ms)) => {
            Some(timeout_ms.max(block_ms.saturating_add(DEFAULT_STREAM_BLOCK_TIMEOUT_GRACE_MS)))
        }
        (Some(timeout_ms), None) => Some(timeout_ms),
        (None, _) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn redis_test_connection(url: &str) -> ::redis::aio::MultiplexedConnection {
        ::redis::Client::open(url)
            .expect("test Redis client")
            .get_multiplexed_async_connection()
            .await
            .expect("test Redis connection")
    }

    mod stream_receive {
        include!("redis/stream_receive_tests.rs");
    }

    mod dead_letter_transfer {
        include!("redis/dead_letter_transfer_tests.rs");
    }

    mod usage_limit_cleanup {
        include!("redis/usage_limit_cleanup_tests.rs");
    }

    #[tokio::test]
    async fn memory_kv_expires_entries() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        runtime
            .kv_set("hello", "world", Some(Duration::from_millis(5)))
            .await
            .expect("set should succeed");
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(runtime.kv_get("hello").await.expect("get"), None);
        assert!(!runtime.kv_exists("hello").await.expect("exists"));
    }

    #[tokio::test]
    async fn memory_kv_take_consumes_entry_once() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        runtime
            .kv_set("nonce", "payload", Some(Duration::from_secs(60)))
            .await
            .expect("set should succeed");
        assert_eq!(
            runtime.kv_take("nonce").await.expect("take").as_deref(),
            Some("payload")
        );
        assert_eq!(runtime.kv_take("nonce").await.expect("take"), None);
    }

    #[tokio::test]
    async fn runtime_backends_share_atomic_kv_set_if_absent_contract() {
        let memory = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        assert_kv_set_if_absent_contract(&memory).await;

        let Some((_redis, redis_runtime)) = redis_runtime_for_test("kv-set-if-absent").await else {
            return;
        };
        assert_kv_set_if_absent_contract(&redis_runtime).await;
    }

    #[tokio::test]
    async fn memory_rate_limit_rejects_after_limit() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let input = RateLimitInput {
            user_key: "rpm:user:1:1",
            key_key: "rpm:key:1:1",
            bucket: 1,
            user_limit: 1,
            key_limit: 0,
            ttl_seconds: 60,
        };
        assert!(matches!(
            runtime
                .check_and_consume_rate_limit(input)
                .await
                .expect("first"),
            RateLimitCheck::Allowed { .. }
        ));
        assert_eq!(
            runtime
                .rate_limit_count(input.user_key, input.bucket)
                .await
                .expect("count after first"),
            1
        );
        assert_eq!(
            runtime
                .check_and_consume_rate_limit(input)
                .await
                .expect("second"),
            RateLimitCheck::Rejected {
                scope: RateLimitScope::User,
                limit: 1
            }
        );
        assert_eq!(
            runtime
                .rate_limit_count(input.user_key, input.bucket)
                .await
                .expect("count after reject"),
            1
        );
    }

    #[tokio::test]
    async fn memory_rate_limit_keeps_user_limit_atomic_across_api_keys() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let first_key = RateLimitInput {
            user_key: "rpm:user:shared:1",
            key_key: "rpm:key:first:1",
            bucket: 1,
            user_limit: 2,
            key_limit: 10,
            ttl_seconds: 60,
        };
        let second_key = RateLimitInput {
            key_key: "rpm:key:second:1",
            ..first_key
        };

        assert!(matches!(
            runtime
                .check_and_consume_rate_limit(first_key)
                .await
                .expect("first key"),
            RateLimitCheck::Allowed { .. }
        ));
        assert!(matches!(
            runtime
                .check_and_consume_rate_limit(second_key)
                .await
                .expect("second key"),
            RateLimitCheck::Allowed { .. }
        ));
        assert_eq!(
            runtime
                .check_and_consume_rate_limit(first_key)
                .await
                .expect("user limit"),
            RateLimitCheck::Rejected {
                scope: RateLimitScope::User,
                limit: 2,
            }
        );
        assert_eq!(
            runtime
                .rate_limit_count(first_key.user_key, first_key.bucket)
                .await
                .expect("user count"),
            2
        );
        assert_eq!(
            runtime
                .rate_limit_count(first_key.key_key, first_key.bucket)
                .await
                .expect("first key count"),
            1
        );
        assert_eq!(
            runtime
                .rate_limit_count(second_key.key_key, second_key.bucket)
                .await
                .expect("second key count"),
            1
        );
    }

    #[tokio::test]
    async fn memory_usage_limits_check_all_windows_before_consuming() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let rules = [
            UsageLimitRule {
                key: "usage:{user-1}:qps",
                limit: 2,
                window_seconds: 10,
                retention_seconds: 10,
            },
            UsageLimitRule {
                key: "usage:{user-1}:weekly",
                limit: 1,
                window_seconds: 60,
                retention_seconds: 60,
            },
        ];

        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &rules,
                    event_id: "request-1",
                    now_unix_ms: 100_000,
                })
                .await
                .expect("first request"),
            UsageLimitCheck::Allowed
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &rules,
                    event_id: "request-2",
                    now_unix_ms: 105_000,
                })
                .await
                .expect("second request"),
            UsageLimitCheck::Rejected {
                rule_index: 1,
                limit: 1,
                retry_after: 55,
            }
        );

        let qps_only = [rules[0]];
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &qps_only,
                    event_id: "request-3",
                    now_unix_ms: 105_000,
                })
                .await
                .expect("weekly rejection must not consume qps"),
            UsageLimitCheck::Allowed
        );
    }

    #[tokio::test]
    async fn memory_usage_limits_are_true_sliding_windows_and_idempotent() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let rules = [UsageLimitRule {
            key: "usage:{user-1}:rolling-10",
            limit: 2,
            window_seconds: 10,
            retention_seconds: 10,
        }];
        let consume = |event_id, now_unix_ms| UsageLimitInput {
            rules: &rules,
            event_id,
            now_unix_ms,
        };

        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("request-1", 100_000))
                .await
                .unwrap(),
            UsageLimitCheck::Allowed
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("request-2", 105_000))
                .await
                .unwrap(),
            UsageLimitCheck::Allowed
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("request-2", 106_000))
                .await
                .unwrap(),
            UsageLimitCheck::Allowed,
            "replaying the same event must not consume twice"
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("request-3", 109_000))
                .await
                .unwrap(),
            UsageLimitCheck::Rejected {
                rule_index: 0,
                limit: 2,
                retry_after: 1,
            }
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("request-3", 110_000))
                .await
                .unwrap(),
            UsageLimitCheck::Allowed,
            "the event at the exact rolling cutoff must expire"
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("request-4", 111_000))
                .await
                .unwrap(),
            UsageLimitCheck::Rejected {
                rule_index: 0,
                limit: 2,
                retry_after: 4,
            },
            "the first event's expiry must not reset when later events arrive"
        );
    }

    #[tokio::test]
    async fn usage_limit_input_requires_unique_co_located_rule_keys() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let different_tags = [
            UsageLimitRule {
                key: "usage:{user-1}:one",
                limit: 1,
                window_seconds: 1,
                retention_seconds: 1,
            },
            UsageLimitRule {
                key: "usage:{user-2}:two",
                limit: 1,
                window_seconds: 1,
                retention_seconds: 1,
            },
        ];
        assert!(matches!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &different_tags,
                    event_id: "request-1",
                    now_unix_ms: 100_000,
                })
                .await,
            Err(DataLayerError::InvalidInput(_))
        ));

        let zero_retention = [UsageLimitRule {
            retention_seconds: 0,
            ..different_tags[0]
        }];
        assert!(matches!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &zero_retention,
                    event_id: "request-1",
                    now_unix_ms: 100_000,
                })
                .await,
            Err(DataLayerError::InvalidInput(message))
                if message.contains("retention_seconds")
        ));

        let duplicate_keys = [different_tags[0], different_tags[0]];
        assert!(matches!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &duplicate_keys,
                    event_id: "request-1",
                    now_unix_ms: 100_000,
                })
                .await,
            Err(DataLayerError::InvalidInput(_))
        ));
    }

    #[tokio::test]
    async fn memory_usage_limits_apply_the_current_window_when_configuration_changes() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let long_window = [UsageLimitRule {
            key: "usage:{user-1}:changing-window",
            limit: 1,
            window_seconds: 60,
            retention_seconds: 60,
        }];
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &long_window,
                    event_id: "request-1",
                    now_unix_ms: 100_000,
                })
                .await
                .unwrap(),
            UsageLimitCheck::Allowed
        );

        let short_window = [UsageLimitRule {
            window_seconds: 10,
            retention_seconds: 10,
            ..long_window[0]
        }];
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &short_window,
                    event_id: "request-2",
                    now_unix_ms: 111_000,
                })
                .await
                .unwrap(),
            UsageLimitCheck::Allowed,
            "the old event is outside the newly configured shorter window"
        );
    }

    #[tokio::test]
    async fn memory_usage_limits_keep_subsecond_events_in_a_one_second_window() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let rules = [UsageLimitRule {
            key: "usage:{user-1}:qps",
            limit: 1,
            window_seconds: 1,
            retention_seconds: 1,
        }];
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &rules,
                    event_id: "request-1",
                    now_unix_ms: 1_900,
                })
                .await
                .unwrap(),
            UsageLimitCheck::Allowed
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &rules,
                    event_id: "request-2",
                    now_unix_ms: 2_100,
                })
                .await
                .unwrap(),
            UsageLimitCheck::Rejected {
                rule_index: 0,
                limit: 1,
                retry_after: 1,
            }
        );
    }

    #[tokio::test]
    async fn memory_usage_limits_do_not_expire_epoch_events_before_the_window_elapses() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let rules = [UsageLimitRule {
            key: "usage:{user-1}:epoch",
            limit: 1,
            window_seconds: 1,
            retention_seconds: 1,
        }];
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &rules,
                    event_id: "request-1",
                    now_unix_ms: 0,
                })
                .await
                .unwrap(),
            UsageLimitCheck::Allowed
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &rules,
                    event_id: "request-2",
                    now_unix_ms: 500,
                })
                .await
                .unwrap(),
            UsageLimitCheck::Rejected {
                rule_index: 0,
                limit: 1,
                retry_after: 1,
            }
        );
    }

    #[test]
    fn runtime_memory_usage_limit_capacity_must_be_positive() {
        let mut config = RuntimeStateConfig::memory();
        config.memory.max_usage_limit_windows = 0;
        assert!(matches!(
            config.validate(),
            Err(DataLayerError::InvalidConfiguration(message))
                if message.contains("max_usage_limit_windows")
        ));

        let mut config = RuntimeStateConfig::memory();
        config.memory.max_usage_limit_events = 0;
        assert!(matches!(
            config.validate(),
            Err(DataLayerError::InvalidConfiguration(message))
                if message.contains("max_usage_limit_events")
        ));
    }

    #[tokio::test]
    async fn memory_rate_limit_concurrent_checks_do_not_exceed_limit() {
        let runtime =
            std::sync::Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let input = RateLimitInput {
            user_key: "rpm:user:concurrent:1",
            key_key: "rpm:key:concurrent:1",
            bucket: 1,
            user_limit: 32,
            key_limit: 64,
            ttl_seconds: 60,
        };
        let mut tasks = Vec::new();
        for _ in 0..128 {
            let runtime = std::sync::Arc::clone(&runtime);
            tasks.push(tokio::spawn(async move {
                runtime
                    .check_and_consume_rate_limit(input)
                    .await
                    .expect("concurrent rate-limit check")
            }));
        }

        let mut allowed = 0;
        let mut rejected = 0;
        for task in tasks {
            match task.await.expect("rate-limit task") {
                RateLimitCheck::Allowed { .. } => allowed += 1,
                RateLimitCheck::Rejected {
                    scope: RateLimitScope::User,
                    limit: 32,
                } => rejected += 1,
                other => panic!("unexpected rate-limit result: {other:?}"),
            }
        }
        assert_eq!(allowed, 32);
        assert_eq!(rejected, 96);
    }

    #[tokio::test]
    async fn memory_lock_fencing_tokens_increase_after_release() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let first = runtime
            .lock_try_acquire("fencing", "node-a", Duration::from_secs(1))
            .await
            .expect("first acquire")
            .expect("first lease");
        assert!(first.fencing_token > 0);
        assert!(runtime.lock_release(&first).await.expect("first release"));

        let second = runtime
            .lock_try_acquire("fencing", "node-b", Duration::from_secs(1))
            .await
            .expect("second acquire")
            .expect("second lease");
        assert!(second.fencing_token > first.fencing_token);
    }

    #[tokio::test]
    async fn memory_expired_lock_cannot_be_renewed() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let expired = runtime
            .lock_try_acquire("expired-fencing", "node-a", Duration::from_millis(10))
            .await
            .expect("acquire")
            .expect("lease");
        tokio::time::sleep(Duration::from_millis(30)).await;

        assert!(!runtime
            .lock_renew(&expired, Duration::from_secs(1))
            .await
            .expect("expired renew should be rejected"));
        let replacement = runtime
            .lock_try_acquire("expired-fencing", "node-b", Duration::from_secs(1))
            .await
            .expect("replacement acquire")
            .expect("replacement lease");
        assert!(replacement.fencing_token > expired.fencing_token);
    }

    #[tokio::test]
    async fn memory_semaphore_holds_until_permit_drop() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let gate = runtime
            .semaphore("test", 1, RuntimeSemaphoreConfig::default())
            .expect("gate should build");
        let permit = gate.try_acquire().await.expect("first permit");
        assert!(matches!(
            gate.try_acquire().await.expect_err("second rejected"),
            RuntimeSemaphoreError::Saturated { .. }
        ));
        drop(permit);
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(gate.snapshot().await.expect("snapshot").in_flight, 0);
    }

    #[tokio::test]
    async fn keyed_memory_semaphores_share_only_the_same_subject_key() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let first = runtime
            .keyed_semaphore(
                "plan_usage_concurrency",
                "admission:plan_usage_concurrency:user-1",
                1,
                RuntimeSemaphoreConfig::default(),
            )
            .expect("first gate");
        let same_subject = runtime
            .keyed_semaphore(
                "plan_usage_concurrency",
                "admission:plan_usage_concurrency:user-1",
                1,
                RuntimeSemaphoreConfig::default(),
            )
            .expect("same subject gate");
        let other_subject = runtime
            .keyed_semaphore(
                "plan_usage_concurrency",
                "admission:plan_usage_concurrency:user-2",
                1,
                RuntimeSemaphoreConfig::default(),
            )
            .expect("other subject gate");

        let permit = first.try_acquire().await.expect("first permit");
        assert!(matches!(
            same_subject.try_acquire().await,
            Err(RuntimeSemaphoreError::Saturated { limit: 1, .. })
        ));
        assert!(other_subject.try_acquire().await.is_ok());
        drop(permit);
        for _ in 0..20 {
            if same_subject.snapshot().await.expect("snapshot").in_flight == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(same_subject.try_acquire().await.is_ok());
    }

    #[tokio::test]
    async fn memory_keyed_semaphores_isolate_resource_capacity() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let first = runtime
            .keyed_semaphore(
                "provider_key",
                "key-a",
                1,
                RuntimeSemaphoreConfig::default(),
            )
            .expect("first gate should build");
        let second = runtime
            .keyed_semaphore(
                "provider_key",
                "key-b",
                1,
                RuntimeSemaphoreConfig::default(),
            )
            .expect("second gate should build");
        let permit = first.try_acquire().await.expect("first permit");

        assert!(matches!(
            first
                .try_acquire()
                .await
                .expect_err("same key should saturate"),
            RuntimeSemaphoreError::Saturated { .. }
        ));
        let second_permit = second
            .try_acquire()
            .await
            .expect("different key should retain independent capacity");

        drop(second_permit);
        drop(permit);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn memory_semaphore_marks_permit_unhealthy_after_lease_loss() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let gate = runtime
            .semaphore(
                "lease-health",
                1,
                RuntimeSemaphoreConfig {
                    lease_ttl_ms: 20,
                    renew_interval_ms: 5,
                    command_timeout_ms: Some(50),
                },
            )
            .expect("gate should build");
        let permit = gate.try_acquire().await.expect("permit should acquire");
        assert!(aether_runtime::AdmissionPermitHealth::is_healthy(&permit));

        std::thread::sleep(Duration::from_millis(30));
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(!aether_runtime::AdmissionPermitHealth::is_healthy(&permit));
    }

    #[tokio::test]
    async fn redis_runtime_reuses_fixed_connections_for_repeated_operations() {
        let Some(redis) = TestRedisServer::start().await else {
            return;
        };
        let runtime = RuntimeState::redis(
            RedisClientConfig {
                url: redis.redis_url.clone(),
                key_prefix: Some(format!("aether-runtime-test-{}", std::process::id())),
            },
            Some(1_000),
        )
        .await
        .expect("runtime should connect");
        let before = runtime
            .redis_diagnostics()
            .await
            .expect("diagnostics")
            .expect("redis diagnostics")
            .total_connections_received
            .expect("total connections");

        for index in 0..200 {
            let key = format!("kv:{index}");
            runtime
                .kv_set(
                    &key,
                    format!("value-{index}"),
                    Some(Duration::from_secs(30)),
                )
                .await
                .expect("set");
            assert_eq!(
                runtime.kv_get(&key).await.expect("get").as_deref(),
                Some(format!("value-{index}").as_str())
            );
        }

        let after = runtime
            .redis_diagnostics()
            .await
            .expect("diagnostics")
            .expect("redis diagnostics")
            .total_connections_received
            .expect("total connections");
        assert_eq!(
            after, before,
            "runtime Redis operations should reuse initialized lanes"
        );
    }

    #[tokio::test]
    async fn redis_runtime_instances_share_ttl_kv_across_reinitialization() {
        let external_redis_url = std::env::var("AETHER_TEST_REDIS_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let redis = if external_redis_url.is_none() {
            TestRedisServer::start().await
        } else {
            None
        };
        let Some(redis_url) =
            external_redis_url.or_else(|| redis.as_ref().map(|redis| redis.redis_url.clone()))
        else {
            return;
        };
        let key_prefix = format!("aether-history-test-{}", std::process::id());
        let runtime_config = || RedisClientConfig {
            url: redis_url.clone(),
            key_prefix: Some(key_prefix.clone()),
        };
        let writer = RuntimeState::redis(runtime_config(), Some(1_000))
            .await
            .expect("writer runtime should connect");
        let reader = RuntimeState::redis(runtime_config(), Some(1_000))
            .await
            .expect("reader runtime should connect");
        let history_key = "ai:responses:history:v1:shared-record";

        writer
            .kv_set(
                history_key,
                "persisted-history",
                Some(Duration::from_secs(30)),
            )
            .await
            .expect("writer should persist history");
        assert_eq!(
            reader
                .kv_get(history_key)
                .await
                .expect("reader get")
                .as_deref(),
            Some("persisted-history")
        );

        drop(writer);
        drop(reader);
        let restarted = RuntimeState::redis(runtime_config(), Some(1_000))
            .await
            .expect("restarted runtime should connect");
        assert_eq!(
            restarted
                .kv_get(history_key)
                .await
                .expect("restarted get")
                .as_deref(),
            Some("persisted-history")
        );
        assert!(matches!(
            restarted
                .kv_ttl_seconds(history_key)
                .await
                .expect("history ttl"),
            Some(1..=30)
        ));
    }

    #[tokio::test]
    async fn redis_lock_fencing_tokens_increase_and_expired_lease_cannot_renew() {
        let Some((_redis, runtime)) = redis_runtime_for_test("lock-fencing").await else {
            return;
        };
        let first = runtime
            .lock_try_acquire("fencing", "node-a", Duration::from_millis(20))
            .await
            .expect("first acquire")
            .expect("first lease");
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!runtime
            .lock_renew(&first, Duration::from_secs(1))
            .await
            .expect("expired renew should be rejected"));

        let second = runtime
            .lock_try_acquire("fencing", "node-b", Duration::from_secs(1))
            .await
            .expect("second acquire")
            .expect("second lease");
        assert!(second.fencing_token > first.fencing_token);
        assert!(runtime.lock_release(&second).await.expect("second release"));

        let third = runtime
            .lock_try_acquire("fencing", "node-c", Duration::from_secs(1))
            .await
            .expect("third acquire")
            .expect("third lease");
        assert!(third.fencing_token > second.fencing_token);
    }

    #[tokio::test]
    async fn redis_blocking_stream_read_does_not_block_fast_lane() {
        let Some(redis) = TestRedisServer::start().await else {
            return;
        };
        let runtime = RuntimeState::redis(
            RedisClientConfig {
                url: redis.redis_url.clone(),
                key_prefix: Some(format!("aether-block-test-{}", std::process::id())),
            },
            Some(1_000),
        )
        .await
        .expect("runtime should connect");
        RuntimeQueueStore::ensure_consumer_group(&runtime, "blocking-stream", "workers", "0-0")
            .await
            .expect("consumer group");

        let blocking_runtime = runtime.clone();
        let blocking = tokio::spawn(async move {
            RuntimeQueueStore::read_group(
                &blocking_runtime,
                "blocking-stream",
                "workers",
                "consumer-a",
                1,
                Some(500),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        runtime
            .kv_set("fast-lane", "ok", Some(Duration::from_secs(30)))
            .await
            .expect("fast lane set should complete while stream read blocks");
        assert_eq!(
            runtime
                .kv_get("fast-lane")
                .await
                .expect("fast lane get")
                .as_deref(),
            Some("ok")
        );
        let _ = blocking.await.expect("blocking task join");
    }

    #[tokio::test]
    async fn redis_concurrent_blocking_stream_reads_do_not_share_single_connection() {
        let Some(redis) = TestRedisServer::start().await else {
            return;
        };
        let runtime = RuntimeState::redis(
            RedisClientConfig {
                url: redis.redis_url.clone(),
                key_prefix: Some(format!("aether-block-pool-test-{}", std::process::id())),
            },
            Some(1_000),
        )
        .await
        .expect("runtime should connect");
        RuntimeQueueStore::ensure_consumer_group(&runtime, "blocking-stream", "workers", "0-0")
            .await
            .expect("consumer group");

        let mut handles = Vec::new();
        for index in 0..4 {
            let blocking_runtime = runtime.clone();
            handles.push(tokio::spawn(async move {
                let consumer = format!("consumer-{index}");
                RuntimeQueueStore::read_group(
                    &blocking_runtime,
                    "blocking-stream",
                    "workers",
                    &consumer,
                    1,
                    Some(600),
                )
                .await
            }));
        }

        for handle in handles {
            let result = handle.await.expect("blocking task join");
            assert!(
                !matches!(result, Err(DataLayerError::TimedOut(_))),
                "concurrent blocking stream reads should not queue behind one connection"
            );
            assert!(result.expect("blocking read should succeed").is_empty());
        }
    }

    #[tokio::test]
    async fn redis_large_stream_batches_preserve_fields_across_read_reclaim_and_ack() {
        let Some(redis) = TestRedisServer::start().await else {
            return;
        };
        for protocol in ["resp2", "resp3"] {
            let runtime = RuntimeState::redis(
                RedisClientConfig {
                    url: format!("{}?protocol={protocol}", redis.redis_url),
                    key_prefix: Some(format!("large-batch-{protocol}")),
                },
                Some(5_000),
            )
            .await
            .expect("large batch runtime should connect");
            let stream = "usage:large-batch";
            let group = "workers";
            RuntimeQueueStore::ensure_consumer_group(&runtime, stream, group, "0-0")
                .await
                .unwrap();
            let payload = format!(
                "{}\r\n\"escaped\"\\\u{4e2d}\u{6587}",
                "x".repeat(512 * 1024)
            );
            let mut expected = BTreeMap::new();
            for sequence in 0..24 {
                let fields = BTreeMap::from([
                    ("payload".to_string(), payload.clone()),
                    ("sequence".to_string(), sequence.to_string()),
                    ("legacy_marker".to_string(), "preserve exactly".to_string()),
                ]);
                let id =
                    RuntimeQueueStore::append_fields_with_maxlen(&runtime, stream, &fields, None)
                        .await
                        .unwrap();
                expected.insert(id, sequence.to_string());
            }
            let mut readers = tokio::task::JoinSet::new();
            for index in 0..3 {
                let runtime = runtime.clone();
                readers.spawn(async move {
                    RuntimeQueueStore::read_group(
                        &runtime,
                        stream,
                        group,
                        &format!("reader-{index}"),
                        8,
                        Some(1),
                    )
                    .await
                    .unwrap()
                });
            }
            let mut delivered = std::collections::BTreeSet::new();
            while let Some(entries) = readers.join_next().await {
                let entries = entries.unwrap();
                assert_eq!(entries.len(), 8);
                for entry in entries {
                    assert_eq!(entry.fields.len(), 3);
                    assert_eq!(entry.fields["payload"].as_bytes(), payload.as_bytes());
                    assert_eq!(entry.fields["sequence"], expected[&entry.id]);
                    assert_eq!(entry.fields["legacy_marker"], "preserve exactly");
                    assert!(delivered.insert(entry.id));
                }
            }
            assert_eq!(delivered.len(), 24);
            let stats = RuntimeQueueStore::stats(&runtime, stream, Some(group))
                .await
                .unwrap();
            assert_eq!(stats.group_pending, 24);
            assert_eq!(stats.group_lag, Some(0));

            tokio::time::sleep(Duration::from_millis(20)).await;
            let mut reclaimed = std::collections::BTreeSet::new();
            while reclaimed.len() < 24 {
                let entries = RuntimeQueueStore::claim_stale(
                    &runtime,
                    stream,
                    group,
                    "retry-consumer",
                    "0-0",
                    RuntimeQueueReclaimConfig {
                        min_idle_ms: 1,
                        count: 5,
                    },
                )
                .await
                .unwrap();
                assert!(!entries.is_empty());
                assert!(entries.len() <= 5);
                let mut ids = Vec::new();
                for entry in entries {
                    assert_eq!(entry.fields.len(), 3);
                    assert_eq!(entry.fields["payload"].as_bytes(), payload.as_bytes());
                    assert_eq!(entry.fields["sequence"], expected[&entry.id]);
                    assert_eq!(entry.fields["legacy_marker"], "preserve exactly");
                    assert!(reclaimed.insert(entry.id.clone()));
                    ids.push(entry.id);
                }
                assert_eq!(
                    RuntimeQueueStore::ack(&runtime, stream, group, &ids)
                        .await
                        .unwrap(),
                    ids.len()
                );
                assert_eq!(
                    RuntimeQueueStore::delete(&runtime, stream, &ids)
                        .await
                        .unwrap(),
                    ids.len()
                );
            }
            assert_eq!(reclaimed, delivered);
            let stats = RuntimeQueueStore::stats(&runtime, stream, Some(group))
                .await
                .unwrap();
            assert_eq!(stats.stream_length, 0);
            assert_eq!(stats.group_pending, 0);
            assert_eq!(stats.group_lag, Some(0));
            eprintln!(
                "verified {protocol}: 24 large records, 3 readers, read/reclaim/ack complete"
            );
        }
    }

    #[tokio::test]
    async fn redis_connection_manager_recovers_after_restart() {
        let Some(mut redis) = TestRedisServer::start().await else {
            return;
        };
        let runtime = RuntimeState::redis(
            RedisClientConfig {
                url: redis.redis_url.clone(),
                key_prefix: Some(format!("aether-restart-test-{}", std::process::id())),
            },
            Some(500),
        )
        .await
        .expect("runtime should connect");
        runtime
            .kv_set("before-restart", "ok", Some(Duration::from_secs(30)))
            .await
            .expect("initial set");

        redis.stop();
        let _ = runtime
            .kv_set("during-restart", "may-fail", Some(Duration::from_secs(30)))
            .await;
        redis.restart().await.expect("redis restart");

        let mut recovered = false;
        for _ in 0..20 {
            if runtime
                .kv_set("after-restart", "ok", Some(Duration::from_secs(30)))
                .await
                .is_ok()
            {
                recovered = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            recovered,
            "connection manager should reconnect after restart"
        );
    }

    #[tokio::test]
    async fn runtime_backends_share_kv_score_and_queue_contracts() {
        let memory = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        assert_kv_score_and_queue_contract(&memory).await;

        let Some((_redis, redis_runtime)) = redis_runtime_for_test("shared-contract").await else {
            return;
        };
        assert_kv_score_and_queue_contract(&redis_runtime).await;
    }

    #[tokio::test]
    async fn runtime_backends_share_bounded_score_window_aggregation() {
        let memory = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        assert_bounded_score_window_aggregation(&memory).await;

        let Some((_server, runtime)) = redis_runtime_for_test("score-window").await else {
            return;
        };
        assert_bounded_score_window_aggregation(&runtime).await;
    }

    async fn assert_bounded_score_window_aggregation(runtime: &RuntimeState) {
        let keys = (0..35)
            .map(|index| format!("window:{index}"))
            .collect::<Vec<_>>();
        for (member, score) in [
            ("expired:999", 99.999),
            ("boundary:7", 100.0),
            ("recent:9007199254740993", 101.0),
            ("nested:prefix:+00012", 102.0),
            ("zero:0", 103.0),
            ("invalid:1.5", 104.0),
            ("invalid:-1", 105.0),
            ("invalid:18446744073709551616", 106.0),
            ("invalid: 12", 107.0),
            ("missing-separator", 108.0),
        ] {
            runtime
                .score_set(&keys[0], member, score)
                .await
                .expect("seed values");
        }
        runtime
            .score_set(&keys[1], "max:18446744073709551615", 100.0)
            .await
            .expect("seed max");
        runtime
            .score_set(&keys[2], "max:18446744073709551615", 100.0)
            .await
            .expect("seed overflow");
        runtime
            .score_set(&keys[2], "additional:2", 100.0)
            .await
            .expect("seed overflow addition");
        for index in 0..SCORE_WINDOW_AGGREGATION_MEMBER_LIMIT {
            runtime
                .score_set(&keys[3], &format!("{index}:3"), 100.0)
                .await
                .expect("seed bounded window");
        }
        runtime
            .score_set(&keys[3], "expired:9999", 0.0)
            .await
            .expect("seed expired sample");
        let stats = runtime
            .score_window_u64_stats_by_min(&keys, 100.0)
            .await
            .expect("aggregate");
        assert_eq!(
            stats.len(),
            keys.len(),
            "pipeline batches preserve key order"
        );
        assert_eq!(
            stats[0],
            Some(ScoreWindowU64Stats {
                sum: 9_007_199_254_741_012,
                positive_count: 3
            })
        );
        assert_eq!(
            stats[1],
            Some(ScoreWindowU64Stats {
                sum: u64::MAX,
                positive_count: 1
            })
        );
        assert_eq!(
            stats[2],
            Some(ScoreWindowU64Stats {
                sum: u64::MAX,
                positive_count: 2
            })
        );
        assert_eq!(
            stats[3],
            Some(ScoreWindowU64Stats {
                sum: 1536,
                positive_count: 512
            })
        );
        assert!(stats[4..]
            .iter()
            .all(|stats| *stats == Some(ScoreWindowU64Stats::default())));

        runtime
            .score_set(&keys[3], "overflowing-window:11", 101.0)
            .await
            .expect("exceed server limit");
        let stats = runtime
            .score_window_u64_stats_by_min(&keys[3..4], 100.0)
            .await
            .expect("bounded fallback");
        assert_eq!(
            stats,
            vec![None],
            "oversized windows require the full exact read"
        );
        let members = runtime
            .score_range_by_min(&keys[3], 100.0)
            .await
            .expect("full window");
        assert_eq!(
            ScoreWindowU64Stats::from_members(members.iter().map(String::as_str)).sum,
            1547
        );
        runtime
            .score_remove(&keys[3], "overflowing-window:11")
            .await
            .expect("remove newest");
        assert_eq!(
            runtime
                .score_window_u64_stats_by_min(&keys[3..4], 100.0)
                .await
                .expect("read after remove")[0]
                .unwrap()
                .sum,
            1536
        );
        runtime
            .score_set(&keys[3], "0:3", 99.0)
            .await
            .expect("move sample outside window");
        assert_eq!(
            runtime
                .score_window_u64_stats_by_min(&keys[3..4], 100.0)
                .await
                .expect("read changed score")[0]
                .unwrap()
                .sum,
            1533
        );
        assert!(runtime
            .score_window_u64_stats_by_min(&[], 100.0)
            .await
            .expect("empty query")
            .is_empty());
        assert!(runtime
            .score_window_u64_stats_by_min(&keys, f64::NAN)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn redis_score_window_aggregation_reloads_scripts_without_caching_old_cost() {
        let Some((server, runtime)) = redis_runtime_for_test("score-window-reload").await else {
            return;
        };
        let keys = vec!["reload:cost".to_string()];
        runtime
            .score_set(&keys[0], "first:7", 100.0)
            .await
            .expect("first cost");
        assert_eq!(
            runtime
                .score_window_u64_stats_by_min(&keys, 100.0)
                .await
                .expect("first aggregate")[0]
                .unwrap()
                .sum,
            7
        );
        let client = ::redis::Client::open(server.redis_url.as_str()).expect("test Redis client");
        let mut connection = client
            .get_multiplexed_async_connection()
            .await
            .expect("test connection");
        ::redis::cmd("SCRIPT")
            .arg("FLUSH")
            .query_async::<()>(&mut connection)
            .await
            .expect("flush scripts");
        runtime
            .score_set(&keys[0], "second:11", 101.0)
            .await
            .expect("new cost");
        assert_eq!(
            runtime
                .score_window_u64_stats_by_min(&keys, 100.0)
                .await
                .expect("reload aggregate")[0]
                .unwrap()
                .sum,
            18
        );
        assert_eq!(
            runtime
                .score_window_u64_stats_by_min(&keys, 101.0)
                .await
                .expect("changed window")[0]
                .unwrap()
                .sum,
            11
        );
        runtime
            .key_expire(&keys[0], Duration::ZERO)
            .await
            .expect("expire window");
        assert_eq!(
            runtime
                .score_window_u64_stats_by_min(&keys, 100.0)
                .await
                .expect("expired aggregate"),
            vec![Some(ScoreWindowU64Stats::default())]
        );
    }

    #[tokio::test]
    async fn redis_score_window_aggregation_observes_completed_concurrent_writes() {
        let Some((_server, runtime)) = redis_runtime_for_test("score-window-concurrent").await
        else {
            return;
        };
        let writer_runtime = runtime.clone();
        let (written_tx, mut written_rx) = tokio::sync::mpsc::channel(8);
        let writer = tokio::spawn(async move {
            for index in 1..=128_u64 {
                writer_runtime
                    .score_set("concurrent:cost", &format!("{index}:2"), 100.0)
                    .await
                    .expect("concurrent write");
                written_tx.send(index).await.expect("notify reader");
            }
        });
        let keys = vec!["concurrent:cost".to_string()];
        while let Some(written) = written_rx.recv().await {
            let stats = runtime
                .score_window_u64_stats_by_min(&keys, 100.0)
                .await
                .expect("concurrent aggregate")[0]
                .unwrap();
            assert!(
                stats.positive_count >= written,
                "completed writes must not be hidden by a stale aggregate"
            );
            assert_eq!(
                stats.sum,
                stats.positive_count * 2,
                "one script observes one consistent window"
            );
        }
        writer.await.expect("writer task");
        assert_eq!(
            runtime
                .score_window_u64_stats_by_min(&keys, 100.0)
                .await
                .expect("final aggregate")[0]
                .unwrap()
                .sum,
            256
        );
    }

    #[tokio::test]
    async fn runtime_backends_reject_invalid_shared_inputs() {
        let memory = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        assert_invalid_shared_inputs(&memory).await;

        let Some((_redis, redis_runtime)) = redis_runtime_for_test("invalid-contract").await else {
            return;
        };
        assert_invalid_shared_inputs(&redis_runtime).await;
    }

    #[tokio::test]
    async fn runtime_backends_share_atomic_sliding_usage_limit_contract() {
        let memory = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        assert_sliding_usage_limit_contract(&memory).await;
        assert_concurrent_usage_limit_cap(&memory).await;

        let Some((_redis, redis_runtime)) = redis_runtime_for_test("usage-limits").await else {
            return;
        };
        assert_sliding_usage_limit_contract(&redis_runtime).await;
        assert_concurrent_usage_limit_cap(&redis_runtime).await;
    }

    #[tokio::test]
    async fn runtime_backends_share_short_remaining_usage_limit_retention_contract() {
        let memory = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let redis = redis_runtime_for_test("usage-limit-retention").await;
        let rules = [UsageLimitRule {
            key: "usage:{retention-user}:period-bucket",
            limit: 1,
            window_seconds: 60,
            retention_seconds: 1,
        }];

        assert_usage_limit_retention_seed(&memory, &rules).await;
        if let Some((_, redis_runtime)) = &redis {
            assert_usage_limit_retention_seed(redis_runtime, &rules).await;
        }

        // Redis deliberately adds one second of expiry grace to the requested retention.
        tokio::time::sleep(Duration::from_millis(2_200)).await;

        assert_usage_limit_retention_expired(&memory, &rules).await;
        if let Some((_, redis_runtime)) = &redis {
            assert_usage_limit_retention_expired(redis_runtime, &rules).await;
        }
    }

    #[tokio::test]
    async fn memory_blocking_queue_read_does_not_block_kv_operations() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        RuntimeQueueStore::ensure_consumer_group(&runtime, "memory-blocking", "workers", "0-0")
            .await
            .expect("consumer group");

        let blocking_runtime = runtime.clone();
        let blocking = tokio::spawn(async move {
            RuntimeQueueStore::read_group(
                &blocking_runtime,
                "memory-blocking",
                "workers",
                "consumer-a",
                1,
                Some(100),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        runtime
            .kv_set("memory-fast-lane", "ok", Some(Duration::from_millis(100)))
            .await
            .expect("set should complete while memory stream read blocks");
        assert_eq!(
            runtime
                .kv_get("memory-fast-lane")
                .await
                .expect("get")
                .as_deref(),
            Some("ok")
        );
        assert!(blocking
            .await
            .expect("blocking task join")
            .expect("read should complete")
            .is_empty());
    }

    #[test]
    fn redis_stream_timeout_expands_past_blocking_read() {
        assert_eq!(
            redis_stream_command_timeout_for_block(Some(1_000), Some(1_000)),
            Some(2_000)
        );
        assert_eq!(
            redis_stream_command_timeout_for_block(Some(5_000), Some(500)),
            Some(5_000)
        );
        assert_eq!(
            redis_stream_command_timeout_for_block(None, Some(500)),
            None
        );
    }

    async fn assert_kv_score_and_queue_contract(runtime: &RuntimeState) {
        runtime
            .kv_set("contract:ttl:set", "value", Some(Duration::from_millis(30)))
            .await
            .expect("set with ttl");
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            runtime.kv_get("contract:ttl:set").await.expect("ttl get"),
            None
        );

        runtime
            .kv_set("contract:ttl:expire", "value", None)
            .await
            .expect("set without ttl");
        assert!(runtime
            .key_expire("contract:ttl:expire", Duration::from_millis(30))
            .await
            .expect("expire existing key"));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            runtime
                .kv_get("contract:ttl:expire")
                .await
                .expect("expired get"),
            None
        );

        runtime
            .kv_set("contract:ttl:zero", "value", None)
            .await
            .expect("set zero ttl key");
        assert!(runtime
            .key_expire("contract:ttl:zero", Duration::ZERO)
            .await
            .expect("zero expire existing key"));
        assert_eq!(
            runtime
                .kv_get("contract:ttl:zero")
                .await
                .expect("zero expired get"),
            None
        );

        runtime
            .set_add("contract:set", "member")
            .await
            .expect("set add");
        assert!(runtime
            .key_expire("contract:set", Duration::from_millis(30))
            .await
            .expect("expire set"));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(runtime.set_len("contract:set").await.expect("set len"), 0);

        for (member, score) in [("a", 1.0), ("b", 2.0), ("c", 3.0), ("d", 4.0)] {
            runtime
                .score_set("contract:zset", member, score)
                .await
                .expect("score set");
        }
        assert_eq!(
            runtime
                .score_range_by_min("contract:zset", 0.0)
                .await
                .expect("score range"),
            vec!["a", "b", "c", "d"]
        );
        assert_eq!(
            runtime
                .score_remove_by_rank("contract:zset", 0, -3)
                .await
                .expect("rank trim"),
            2
        );
        assert_eq!(
            runtime
                .score_many(
                    "contract:zset",
                    &["a".to_string(), "b".to_string(), "c".to_string()]
                )
                .await
                .expect("score many"),
            vec![None, None, Some(3.0)]
        );
        assert!(runtime
            .key_expire("contract:zset", Duration::from_millis(30))
            .await
            .expect("expire zset"));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            runtime
                .score_len("contract:zset")
                .await
                .expect("expired zset len"),
            0
        );

        RuntimeQueueStore::ensure_consumer_group(runtime, "contract:stream", "workers", "0-0")
            .await
            .expect("consumer group");
        let fields = BTreeMap::from([("payload".to_string(), "one".to_string())]);
        let first = RuntimeQueueStore::append_fields_with_maxlen(
            runtime,
            "contract:stream",
            &fields,
            Some(100),
        )
        .await
        .expect("append first");
        let fields = BTreeMap::from([("payload".to_string(), "two".to_string())]);
        let second = RuntimeQueueStore::append_fields_with_maxlen(
            runtime,
            "contract:stream",
            &fields,
            Some(100),
        )
        .await
        .expect("append second");
        let delivered = RuntimeQueueStore::read_group(
            runtime,
            "contract:stream",
            "workers",
            "consumer-a",
            10,
            None,
        )
        .await
        .expect("read group");
        assert_eq!(
            delivered
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            vec![first.clone(), second.clone()]
        );
        let stats = RuntimeQueueStore::stats(runtime, "contract:stream", Some("workers"))
            .await
            .expect("queue stats");
        assert_eq!(stats.stream_length, 2);
        assert_eq!(stats.group_pending, 2);
        assert_eq!(stats.group_lag, Some(0));
        assert!(RuntimeQueueStore::read_group(
            runtime,
            "contract:stream",
            "workers",
            "consumer-a",
            10,
            None
        )
        .await
        .expect("second read")
        .is_empty());
        tokio::time::sleep(Duration::from_millis(20)).await;
        let claimed = RuntimeQueueStore::claim_stale(
            runtime,
            "contract:stream",
            "workers",
            "consumer-b",
            "0-0",
            RuntimeQueueReclaimConfig {
                min_idle_ms: 1,
                count: 10,
            },
        )
        .await
        .expect("claim stale");
        let ids = claimed
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![first.clone(), second.clone()]);
        let stats = RuntimeQueueStore::stats(runtime, "contract:stream", Some("workers"))
            .await
            .expect("queue stats after claim");
        assert_eq!(stats.group_pending, 2);
        assert!(stats.oldest_pending_idle_ms.unwrap_or_default() <= 5000);
        assert_eq!(
            RuntimeQueueStore::ack(runtime, "contract:stream", "workers", &ids)
                .await
                .expect("ack"),
            2
        );
        assert_eq!(
            RuntimeQueueStore::delete(runtime, "contract:stream", &ids)
                .await
                .expect("delete"),
            2
        );
        let stats = RuntimeQueueStore::stats(runtime, "contract:stream", Some("workers"))
            .await
            .expect("queue stats after delete");
        assert_eq!(stats.stream_length, 0);
        assert_eq!(stats.group_pending, 0);
        assert_eq!(stats.group_lag, Some(0));
        assert!(RuntimeQueueStore::read_group(
            runtime,
            "contract:stream",
            "workers",
            "consumer-b",
            10,
            None
        )
        .await
        .expect("read after delete")
        .is_empty());
    }

    async fn assert_invalid_shared_inputs(runtime: &RuntimeState) {
        assert!(matches!(
            runtime
                .kv_set_if_absent("contract:invalid-ttl", "value", Duration::ZERO)
                .await,
            Err(DataLayerError::InvalidInput(_))
        ));
        assert!(matches!(
            runtime
                .score_set("contract:invalid-score", "nan", f64::NAN)
                .await,
            Err(DataLayerError::InvalidInput(_))
        ));
        assert!(matches!(
            RuntimeQueueStore::read_group(runtime, "", "workers", "consumer-a", 1, None).await,
            Err(DataLayerError::InvalidInput(_))
        ));
        assert!(matches!(
            RuntimeQueueStore::claim_stale(
                runtime,
                "contract:stream",
                "workers",
                "consumer-a",
                "0-0",
                RuntimeQueueReclaimConfig {
                    min_idle_ms: 0,
                    count: 1,
                },
            )
            .await,
            Err(DataLayerError::InvalidInput(_))
        ));
    }

    async fn assert_kv_set_if_absent_contract(runtime: &RuntimeState) {
        let key = "contract:kv:set-if-absent";
        assert!(runtime
            .kv_set_if_absent(key, "first", Duration::from_millis(30))
            .await
            .expect("first set-if-absent should succeed"));
        assert!(!runtime
            .kv_set_if_absent(key, "second", Duration::from_secs(30))
            .await
            .expect("duplicate set-if-absent should be rejected"));
        assert_eq!(
            runtime
                .kv_get(key)
                .await
                .expect("existing value should be readable")
                .as_deref(),
            Some("first"),
            "a rejected set-if-absent must not replace the existing value"
        );

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(runtime
            .kv_set_if_absent(key, "after-expiry", Duration::from_secs(30))
            .await
            .expect("expired key should be reusable"));
        assert_eq!(
            runtime
                .kv_get(key)
                .await
                .expect("replacement value should be readable")
                .as_deref(),
            Some("after-expiry")
        );

        let concurrent_key = "contract:kv:set-if-absent:concurrent";
        let mut tasks = Vec::new();
        for index in 0..64 {
            let runtime = runtime.clone();
            tasks.push(tokio::spawn(async move {
                runtime
                    .kv_set_if_absent(
                        concurrent_key,
                        format!("candidate-{index}"),
                        Duration::from_secs(30),
                    )
                    .await
                    .expect("concurrent set-if-absent should complete")
            }));
        }
        let mut created = 0;
        for task in tasks {
            if task.await.expect("set-if-absent task should join") {
                created += 1;
            }
        }
        assert_eq!(
            created, 1,
            "exactly one concurrent caller may create the key"
        );
        assert!(runtime
            .kv_get(concurrent_key)
            .await
            .expect("winning value should be readable")
            .is_some());
    }

    async fn assert_sliding_usage_limit_contract(runtime: &RuntimeState) {
        let rules = [
            UsageLimitRule {
                key: "usage:{shared-user}:short",
                limit: 2,
                window_seconds: 10,
                retention_seconds: 10,
            },
            UsageLimitRule {
                key: "usage:{shared-user}:long",
                limit: 1,
                window_seconds: 60,
                retention_seconds: 60,
            },
        ];
        let consume = |event_id, now_unix_ms, rules| UsageLimitInput {
            rules,
            event_id,
            now_unix_ms,
        };

        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("event-1", 100_000, &rules))
                .await
                .expect("first event"),
            UsageLimitCheck::Allowed
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("event-1", 101_000, &rules))
                .await
                .expect("idempotent replay"),
            UsageLimitCheck::Allowed
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("event-2", 105_000, &rules))
                .await
                .expect("long window rejection"),
            UsageLimitCheck::Rejected {
                rule_index: 1,
                limit: 1,
                retry_after: 55,
            }
        );

        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("event-3", 105_000, &rules[..1]))
                .await
                .expect("rejection must leave short rule untouched"),
            UsageLimitCheck::Allowed
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("event-4", 109_000, &rules[..1]))
                .await
                .expect("short rule rejection"),
            UsageLimitCheck::Rejected {
                rule_index: 0,
                limit: 2,
                retry_after: 1,
            }
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("event-4", 110_000, &rules[..1]))
                .await
                .expect("event at cutoff expires"),
            UsageLimitCheck::Allowed
        );

        let qps = [UsageLimitRule {
            key: "usage:{shared-user}:qps",
            limit: 1,
            window_seconds: 1,
            retention_seconds: 1,
        }];
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("qps-1", 200_900, &qps))
                .await
                .expect("first qps event"),
            UsageLimitCheck::Allowed
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("qps-2", 201_100, &qps))
                .await
                .expect("subsecond qps rejection"),
            UsageLimitCheck::Rejected {
                rule_index: 0,
                limit: 1,
                retry_after: 1,
            }
        );
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("qps-2", 201_900, &qps))
                .await
                .expect("qps event at cutoff"),
            UsageLimitCheck::Allowed
        );

        runtime
            .release_usage_limits(UsageLimitReleaseInput {
                rules: &qps,
                event_id: "qps-2",
            })
            .await
            .expect("release consumed qps event");
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(consume("qps-3", 201_900, &qps))
                .await
                .expect("released qps capacity should be reusable"),
            UsageLimitCheck::Allowed
        );
        runtime
            .release_usage_limits(UsageLimitReleaseInput {
                rules: &qps,
                event_id: "qps-2",
            })
            .await
            .expect("release should be idempotent");
    }

    async fn assert_usage_limit_retention_seed(
        runtime: &RuntimeState,
        rules: &[UsageLimitRule<'_>],
    ) {
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules,
                    event_id: "period-event-1",
                    now_unix_ms: 100_000,
                })
                .await
                .expect("seed short-retention period bucket"),
            UsageLimitCheck::Allowed
        );
    }

    async fn assert_usage_limit_retention_expired(
        runtime: &RuntimeState,
        rules: &[UsageLimitRule<'_>],
    ) {
        assert_eq!(
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules,
                    event_id: "period-event-2",
                    now_unix_ms: 102_200,
                })
                .await
                .expect("expired period bucket must be reusable"),
            UsageLimitCheck::Allowed,
            "retention must expire the bucket before its full counting window"
        );
    }

    async fn assert_concurrent_usage_limit_cap(runtime: &RuntimeState) {
        let mut tasks = Vec::new();
        for index in 0..64 {
            let runtime = runtime.clone();
            tasks.push(tokio::spawn(async move {
                let event_id = format!("parallel-event-{index}");
                let rules = [UsageLimitRule {
                    key: "usage:{shared-user}:parallel",
                    limit: 8,
                    window_seconds: 60,
                    retention_seconds: 60,
                }];
                runtime
                    .check_and_consume_usage_limits(UsageLimitInput {
                        rules: &rules,
                        event_id: &event_id,
                        now_unix_ms: 300_000,
                    })
                    .await
                    .expect("concurrent usage limit check")
            }));
        }

        let mut allowed = 0;
        let mut rejected = 0;
        for task in tasks {
            match task.await.expect("concurrent usage limit task") {
                UsageLimitCheck::Allowed => allowed += 1,
                UsageLimitCheck::Rejected {
                    rule_index: 0,
                    limit: 8,
                    retry_after: 60,
                } => rejected += 1,
                unexpected => panic!("unexpected usage limit result: {unexpected:?}"),
            }
        }
        assert_eq!(allowed, 8);
        assert_eq!(rejected, 56);
    }

    async fn redis_runtime_for_test(prefix: &str) -> Option<(TestRedisServer, RuntimeState)> {
        let redis = TestRedisServer::start().await?;
        let runtime = RuntimeState::redis(
            RedisClientConfig {
                url: redis.redis_url.clone(),
                key_prefix: Some(format!(
                    "aether-runtime-test-{prefix}-{}",
                    std::process::id()
                )),
            },
            Some(1_000),
        )
        .await
        .ok()?;
        Some((redis, runtime))
    }

    struct TestRedisServer {
        child: Option<Child>,
        binary: String,
        port: u16,
        workdir: PathBuf,
        redis_url: String,
    }

    impl TestRedisServer {
        async fn start() -> Option<Self> {
            let port = reserve_local_port().ok()?;
            let workdir = std::env::temp_dir().join(format!(
                "aether-runtime-state-redis-{}-{port}",
                std::process::id()
            ));
            std::fs::create_dir_all(&workdir).ok()?;
            let binary = std::env::var("AETHER_REDIS_SERVER_BIN")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "redis-server".to_string());
            let mut server = Self {
                child: None,
                binary,
                port,
                workdir,
                redis_url: format!("redis://127.0.0.1:{port}/0"),
            };
            server.restart().await.ok()?;
            Some(server)
        }

        fn stop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }

        async fn restart(&mut self) -> Result<(), Box<dyn std::error::Error>> {
            self.stop();
            let child = Command::new(&self.binary)
                .arg("--save")
                .arg("")
                .arg("--appendonly")
                .arg("no")
                .arg("--port")
                .arg(self.port.to_string())
                .arg("--dir")
                .arg(&self.workdir)
                .arg("--bind")
                .arg("127.0.0.1")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            self.child = Some(child);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while tokio::time::Instant::now() < deadline {
                if redis_ping(self.port).await.unwrap_or(false) {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            self.stop();
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out waiting for test redis-server",
            )
            .into())
        }
    }

    impl Drop for TestRedisServer {
        fn drop(&mut self) {
            self.stop();
            let _ = std::fs::remove_dir_all(&self.workdir);
        }
    }

    async fn redis_ping(port: u16) -> Result<bool, std::io::Error> {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        stream.write_all(b"*1\r\n$4\r\nPING\r\n").await?;
        let mut buffer = [0_u8; 16];
        let len = stream.read(&mut buffer).await?;
        Ok(buffer[..len].starts_with(b"+PONG"))
    }

    fn reserve_local_port() -> Result<u16, std::io::Error> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        Ok(port)
    }
}
