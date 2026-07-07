mod error;
mod memory;
pub mod redis;

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
use tokio::task::JoinHandle;
use tracing::warn;
use uuid::Uuid;

const DEFAULT_KV_TTL_SECONDS: u64 = 300;
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 1_000;

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
        let token = format!("{owner}:{}", Uuid::new_v4());
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                if memory
                    .lock_try_acquire(key, owner, token.clone(), ttl)
                    .await
                {
                    Ok(Some(RuntimeLockLease {
                        key: key.to_string(),
                        owner: owner.to_string(),
                        token,
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
        RuntimeSemaphore::new(self.clone(), gate, limit, config)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLockLease {
    pub key: String,
    pub owner: String,
    pub token: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeQueueEntry {
    pub id: String,
    pub fields: BTreeMap<String, String>,
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
        validate_runtime_queue_name(stream, "runtime queue stream")?;
        validate_runtime_queue_name(group, "runtime queue group")?;
        validate_runtime_queue_name(consumer, "runtime queue consumer")?;
        validate_runtime_queue_name(start_id, "runtime queue start id")?;
        validate_runtime_queue_reclaim_config(config)?;
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory
                    .queue_claim_stale(stream, group, consumer, start_id, config)
                    .await
            }
            RuntimeStateBackend::Redis(redis) => Ok(redis
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
                .await?
                .entries
                .into_iter()
                .map(|entry| RuntimeQueueEntry {
                    id: entry.id,
                    fields: entry.fields,
                })
                .collect()),
        }
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
                key: format!("admission:{gate}"),
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
}

impl Drop for RuntimeSemaphorePermit {
    fn drop(&mut self) {
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
        let renew_task = tokio::spawn(async move {
            let interval = Duration::from_millis(renew_state.config.renew_interval_ms);
            loop {
                tokio::time::sleep(interval).await;
                if let Err(err) = renew_state.renew(&renew_token).await {
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
            Some(timeout_ms.max(block_ms.saturating_add(DEFAULT_COMMAND_TIMEOUT_MS)))
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
    async fn runtime_backends_reject_invalid_shared_inputs() {
        let memory = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        assert_invalid_shared_inputs(&memory).await;

        let Some((_redis, redis_runtime)) = redis_runtime_for_test("invalid-contract").await else {
            return;
        };
        assert_invalid_shared_inputs(&redis_runtime).await;
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
