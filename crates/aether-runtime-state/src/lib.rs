mod error;
mod memory;
pub mod redis;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::redis::{cmd as redis_cmd, script as redis_script, RedisCmd};
pub use crate::redis::{
    RedisClient, RedisClientConfig, RedisClientFactory, RedisConsumerGroup, RedisConsumerName,
    RedisKeyspace, RedisKvRunner, RedisKvRunnerConfig, RedisLockLease, RedisLockRunner,
    RedisLockRunnerConfig, RedisStreamEntry, RedisStreamName, RedisStreamReclaimConfig,
    RedisStreamRunner, RedisStreamRunnerConfig,
};
use async_trait::async_trait;
pub use error::DataLayerError;
use error::RedisResultExt;
use memory::MemoryRuntimeBackend;
pub use memory::MemoryRuntimeStateConfig;
use tokio::task::JoinHandle;
use tracing::warn;
use uuid::Uuid;

const DEFAULT_KV_TTL_SECONDS: u64 = 300;
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 1_000;

const RATE_LIMIT_CHECK_AND_CONSUME_SCRIPT: &str = r#"
local user_key = KEYS[1]
local key_key = KEYS[2]
local user_limit = tonumber(ARGV[1])
local key_limit = tonumber(ARGV[2])
local ttl = tonumber(ARGV[3])

local user_count = 0
if user_limit > 0 then
    user_count = tonumber(redis.call('GET', user_key) or '0')
    if user_count >= user_limit then
        return {0, 1, user_limit, 0}
    end
end

local key_count = 0
if key_limit > 0 then
    key_count = tonumber(redis.call('GET', key_key) or '0')
    if key_count >= key_limit then
        return {0, 2, key_limit, 0}
    end
end

local remaining = -1
if user_limit > 0 then
    user_count = redis.call('INCR', user_key)
    redis.call('EXPIRE', user_key, ttl)
    remaining = user_limit - user_count
end

if key_limit > 0 then
    key_count = redis.call('INCR', key_key)
    redis.call('EXPIRE', key_key, ttl)
    local key_remaining = key_limit - key_count
    if remaining == -1 or key_remaining < remaining then
        remaining = key_remaining
    end
end

return {1, 0, 0, remaining}
"#;

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
}

impl Default for RuntimeStateConfig {
    fn default() -> Self {
        Self {
            backend: RuntimeStateBackendMode::Auto,
            redis: None,
            memory: MemoryRuntimeStateConfig::default(),
            command_timeout_ms: Some(DEFAULT_COMMAND_TIMEOUT_MS),
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
    Memory(MemoryRuntimeBackend),
    Redis(RedisRuntimeBackend),
}

#[derive(Debug, Clone)]
struct RedisRuntimeBackend {
    client: RedisClient,
    keyspace: RedisKeyspace,
    kv: RedisKvRunner,
    lock: RedisLockRunner,
    stream: RedisStreamRunner,
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
                Self::redis(redis, config.command_timeout_ms).await
            }
            RuntimeStateBackendMode::Auto => unreachable!("auto resolved above"),
        }
    }

    pub fn memory(config: MemoryRuntimeStateConfig) -> Self {
        Self {
            backend: Arc::new(RuntimeStateBackend::Memory(MemoryRuntimeBackend::new(
                config,
            ))),
        }
    }

    pub async fn redis(
        config: RedisClientConfig,
        command_timeout_ms: Option<u64>,
    ) -> Result<Self, DataLayerError> {
        let factory = RedisClientFactory::new(config)?;
        let client = factory.connect_lazy()?;
        let keyspace = factory.config().keyspace();
        ping_redis(&client, command_timeout_ms).await?;
        let kv = RedisKvRunner::new(
            client.clone(),
            keyspace.clone(),
            RedisKvRunnerConfig {
                command_timeout_ms,
                default_ttl_seconds: DEFAULT_KV_TTL_SECONDS,
            },
        )?;
        let lock = RedisLockRunner::new(
            client.clone(),
            keyspace.clone(),
            RedisLockRunnerConfig {
                command_timeout_ms,
                ..RedisLockRunnerConfig::default()
            },
        )?;
        let stream = RedisStreamRunner::new(
            client.clone(),
            keyspace.clone(),
            RedisStreamRunnerConfig {
                command_timeout_ms,
                read_block_ms: None,
                ..RedisStreamRunnerConfig::default()
            },
        )?;
        Ok(Self {
            backend: Arc::new(RuntimeStateBackend::Redis(RedisRuntimeBackend {
                client,
                keyspace,
                kv,
                lock,
                stream,
                command_timeout_ms,
            })),
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
                    redis
                        .kv
                        .setex(key, &value, Some(ttl.as_secs().max(1)))
                        .await?;
                } else {
                    let namespaced_key = redis.keyspace.key(key);
                    run_redis_with_timeout(redis.command_timeout_ms, "runtime kv set", async {
                        let mut connection = redis
                            .client
                            .get_multiplexed_async_connection()
                            .await
                            .map_redis_err()?;
                        redis_cmd("SET")
                            .arg(namespaced_key)
                            .arg(value)
                            .query_async::<String>(&mut connection)
                            .await
                            .map_redis_err()
                    })
                    .await?;
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
            RuntimeStateBackend::Redis(redis) => {
                let namespaced = keys
                    .iter()
                    .map(|key| redis.keyspace.key(key))
                    .collect::<Vec<_>>();
                run_redis_with_timeout(redis.command_timeout_ms, "runtime kv mget", async {
                    let mut connection = redis
                        .client
                        .get_multiplexed_async_connection()
                        .await
                        .map_redis_err()?;
                    redis_cmd("MGET")
                        .arg(&namespaced)
                        .query_async::<Vec<Option<String>>>(&mut connection)
                        .await
                        .map_redis_err()
                })
                .await
            }
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
            RuntimeStateBackend::Redis(redis) => {
                let namespaced = keys
                    .iter()
                    .map(|key| {
                        if key.starts_with(redis.keyspace.key("").trim_end_matches(':')) {
                            key.clone()
                        } else {
                            redis.keyspace.key(key)
                        }
                    })
                    .collect::<Vec<_>>();
                let deleted = run_redis_with_timeout(
                    redis.command_timeout_ms,
                    "runtime kv delete many",
                    async {
                        let mut connection = redis
                            .client
                            .get_multiplexed_async_connection()
                            .await
                            .map_redis_err()?;
                        redis_cmd("DEL")
                            .arg(&namespaced)
                            .query_async::<i64>(&mut connection)
                            .await
                            .map_redis_err()
                    },
                )
                .await?;
                Ok(usize::try_from(deleted).unwrap_or(0))
            }
        }
    }

    pub async fn kv_exists(&self, key: &str) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.kv_exists(key).await),
            RuntimeStateBackend::Redis(redis) => redis.kv.exists(key).await,
        }
    }

    pub async fn counter_increment(&self, key: &str) -> Result<u64, DataLayerError> {
        validate_counter_key(key)?;
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.counter_increment(key).await),
            RuntimeStateBackend::Redis(redis) => {
                let key = redis.keyspace.key(key);
                let next = redis_query_i64(redis, "runtime counter increment", {
                    let mut command = redis_cmd("INCR");
                    command.arg(&key);
                    command
                })
                .await?;
                u64::try_from(next).map_err(|_| {
                    DataLayerError::UnexpectedValue(format!(
                        "runtime counter increment returned negative value {next}"
                    ))
                })
            }
        }
    }

    pub async fn counter_get(&self, key: &str) -> Result<u64, DataLayerError> {
        validate_counter_key(key)?;
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.counter_get(key).await),
            RuntimeStateBackend::Redis(redis) => {
                let namespaced_key = redis.keyspace.key(key);
                let raw = run_redis_with_timeout(
                    redis.command_timeout_ms,
                    "runtime counter get",
                    async {
                        let mut connection = redis
                            .client
                            .get_multiplexed_async_connection()
                            .await
                            .map_redis_err()?;
                        redis_cmd("GET")
                            .arg(&namespaced_key)
                            .query_async::<Option<String>>(&mut connection)
                            .await
                            .map_redis_err()
                    },
                )
                .await?;
                raw.as_deref().unwrap_or("0").parse::<u64>().map_err(|err| {
                    DataLayerError::UnexpectedValue(format!(
                        "runtime counter {key} contains non-u64 value: {err}"
                    ))
                })
            }
        }
    }

    pub async fn kv_ttl_seconds(&self, key: &str) -> Result<Option<i64>, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.kv_ttl_seconds(key).await),
            RuntimeStateBackend::Redis(redis) => {
                let namespaced_key = redis.keyspace.key(key);
                let ttl =
                    run_redis_with_timeout(redis.command_timeout_ms, "runtime kv ttl", async {
                        let mut connection = redis
                            .client
                            .get_multiplexed_async_connection()
                            .await
                            .map_redis_err()?;
                        redis_cmd("TTL")
                            .arg(&namespaced_key)
                            .query_async::<i64>(&mut connection)
                            .await
                            .map_redis_err()
                    })
                    .await?;
                Ok((ttl >= -1).then_some(ttl))
            }
        }
    }

    pub async fn scan_keys(
        &self,
        pattern: &str,
        count: usize,
    ) -> Result<Vec<String>, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.kv_scan(pattern).await),
            RuntimeStateBackend::Redis(redis) => {
                let pattern = redis.keyspace.key(pattern);
                run_redis_with_timeout(redis.command_timeout_ms, "runtime scan keys", async {
                    let mut connection = redis
                        .client
                        .get_multiplexed_async_connection()
                        .await
                        .map_redis_err()?;
                    let mut cursor = 0u64;
                    let mut keys = Vec::new();
                    loop {
                        let (next_cursor, mut batch) = redis_cmd("SCAN")
                            .arg(cursor)
                            .arg("MATCH")
                            .arg(&pattern)
                            .arg("COUNT")
                            .arg(count.max(1))
                            .query_async::<(u64, Vec<String>)>(&mut connection)
                            .await
                            .map_redis_err()?;
                        keys.append(&mut batch);
                        if next_cursor == 0 {
                            break;
                        }
                        cursor = next_cursor;
                    }
                    keys.sort();
                    Ok(keys)
                })
                .await
            }
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
                let user_key = redis.keyspace.key(input.user_key);
                let key_key = redis.keyspace.key(input.key_key);
                let raw = run_redis_with_timeout(
                    redis.command_timeout_ms,
                    "runtime rate limit check",
                    async {
                        let mut connection = redis
                            .client
                            .get_multiplexed_async_connection()
                            .await
                            .map_redis_err()?;
                        redis_script(RATE_LIMIT_CHECK_AND_CONSUME_SCRIPT)
                            .key(user_key)
                            .key(key_key)
                            .arg(i64::from(input.user_limit))
                            .arg(i64::from(input.key_limit))
                            .arg(i64::try_from(input.ttl_seconds.max(1)).unwrap_or(i64::MAX))
                            .invoke_async::<Vec<i64>>(&mut connection)
                            .await
                            .map_redis_err()
                    },
                )
                .await?;
                if raw.first().copied().unwrap_or_default() == 1 {
                    return Ok(RateLimitCheck::Allowed {
                        remaining: raw
                            .get(3)
                            .copied()
                            .and_then(|value| u32::try_from(value).ok())
                            .unwrap_or_default(),
                    });
                }
                let scope = match raw.get(1).copied().unwrap_or_default() {
                    2 => RateLimitScope::Key,
                    _ => RateLimitScope::User,
                };
                let limit = raw
                    .get(2)
                    .copied()
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(match scope {
                        RateLimitScope::User => input.user_limit,
                        RateLimitScope::Key => input.key_limit,
                    });
                Ok(RateLimitCheck::Rejected { scope, limit })
            }
        }
    }

    pub async fn set_add(&self, key: &str, member: &str) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.set_add(key, member).await),
            RuntimeStateBackend::Redis(redis) => {
                let key = redis.keyspace.key(key);
                let mut command = redis_cmd("SADD");
                command.arg(&key).arg(member);
                let added = redis_query_i64(redis, "runtime set add", command).await?;
                Ok(added > 0)
            }
        }
    }

    pub async fn set_remove(&self, key: &str, member: &str) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.set_remove(key, member).await),
            RuntimeStateBackend::Redis(redis) => {
                let key = redis.keyspace.key(key);
                let mut command = redis_cmd("SREM");
                command.arg(&key).arg(member);
                let removed = redis_query_i64(redis, "runtime set remove", command).await?;
                Ok(removed > 0)
            }
        }
    }

    pub async fn set_members(&self, key: &str) -> Result<Vec<String>, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.set_members(key).await),
            RuntimeStateBackend::Redis(redis) => {
                let key = redis.keyspace.key(key);
                let mut values = run_redis_with_timeout(
                    redis.command_timeout_ms,
                    "runtime set members",
                    async {
                        let mut connection = redis
                            .client
                            .get_multiplexed_async_connection()
                            .await
                            .map_redis_err()?;
                        redis_cmd("SMEMBERS")
                            .arg(&key)
                            .query_async::<Vec<String>>(&mut connection)
                            .await
                            .map_redis_err()
                    },
                )
                .await?;
                values.sort();
                Ok(values)
            }
        }
    }

    pub async fn set_len(&self, key: &str) -> Result<usize, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.set_len(key).await),
            RuntimeStateBackend::Redis(redis) => {
                let key = redis.keyspace.key(key);
                let mut command = redis_cmd("SCARD");
                command.arg(&key);
                let len = redis_query_i64(redis, "runtime set len", command).await?;
                Ok(usize::try_from(len).unwrap_or(0))
            }
        }
    }

    pub async fn score_set(
        &self,
        key: &str,
        member: &str,
        score: f64,
    ) -> Result<(), DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                memory.score_set(key, member, score).await;
                Ok(())
            }
            RuntimeStateBackend::Redis(redis) => {
                let key = redis.keyspace.key(key);
                let mut command = redis_cmd("ZADD");
                command.arg(&key).arg(score).arg(member);
                redis_query_i64(redis, "runtime score set", command).await?;
                Ok(())
            }
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
            RuntimeStateBackend::Redis(redis) => {
                let key = redis.keyspace.key(key);
                run_redis_with_timeout(redis.command_timeout_ms, "runtime score many", async {
                    let mut connection = redis
                        .client
                        .get_multiplexed_async_connection()
                        .await
                        .map_redis_err()?;
                    let mut command = redis_cmd("ZMSCORE");
                    command.arg(&key);
                    for member in members {
                        command.arg(member);
                    }
                    command
                        .query_async::<Vec<Option<f64>>>(&mut connection)
                        .await
                        .map_redis_err()
                })
                .await
            }
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
                let key = redis.keyspace.key(key);
                run_redis_with_timeout(redis.command_timeout_ms, "runtime score range", async {
                    let mut connection = redis
                        .client
                        .get_multiplexed_async_connection()
                        .await
                        .map_redis_err()?;
                    redis_cmd("ZRANGEBYSCORE")
                        .arg(&key)
                        .arg(min_score)
                        .arg("+inf")
                        .query_async::<Vec<String>>(&mut connection)
                        .await
                        .map_redis_err()
                })
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
                let key = redis.keyspace.key(key);
                let removed =
                    run_redis_with_timeout(redis.command_timeout_ms, "runtime score trim", async {
                        let mut connection = redis
                            .client
                            .get_multiplexed_async_connection()
                            .await
                            .map_redis_err()?;
                        redis_cmd("ZREMRANGEBYSCORE")
                            .arg(&key)
                            .arg("-inf")
                            .arg(max_score)
                            .query_async::<i64>(&mut connection)
                            .await
                            .map_redis_err()
                    })
                    .await?;
                Ok(usize::try_from(removed).unwrap_or(0))
            }
        }
    }

    pub async fn score_remove_by_rank(
        &self,
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<usize, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(_) => Ok(0),
            RuntimeStateBackend::Redis(redis) => {
                let key = redis.keyspace.key(key);
                let removed = run_redis_with_timeout(
                    redis.command_timeout_ms,
                    "runtime score rank trim",
                    async {
                        let mut connection = redis
                            .client
                            .get_multiplexed_async_connection()
                            .await
                            .map_redis_err()?;
                        redis_cmd("ZREMRANGEBYRANK")
                            .arg(&key)
                            .arg(start)
                            .arg(stop)
                            .query_async::<i64>(&mut connection)
                            .await
                            .map_redis_err()
                    },
                )
                .await?;
                Ok(usize::try_from(removed).unwrap_or(0))
            }
        }
    }

    pub async fn score_len(&self, key: &str) -> Result<usize, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.score_len(key).await),
            RuntimeStateBackend::Redis(redis) => {
                let key = redis.keyspace.key(key);
                let len = redis_query_i64(redis, "runtime score len", {
                    let mut command = redis_cmd("ZCARD");
                    command.arg(&key);
                    command
                })
                .await?;
                Ok(usize::try_from(len).unwrap_or(0))
            }
        }
    }

    pub async fn key_expire(&self, key: &str, ttl: Duration) -> Result<bool, DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(_) => Ok(true),
            RuntimeStateBackend::Redis(redis) => {
                let key = redis.keyspace.key(key);
                let updated = redis_query_i64(redis, "runtime key expire", {
                    let mut command = redis_cmd("EXPIRE");
                    command.arg(&key).arg(ttl.as_secs().max(1));
                    command
                })
                .await?;
                Ok(updated > 0)
            }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeQueueReclaimConfig {
    pub min_idle_ms: u64,
    pub count: usize,
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
}

#[async_trait]
impl RuntimeQueueStore for RuntimeState {
    async fn ensure_consumer_group(
        &self,
        stream: &str,
        group: &str,
        start_id: &str,
    ) -> Result<(), DataLayerError> {
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(_) => Ok(()),
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
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => Ok(memory.queue_read(stream, count).await),
            RuntimeStateBackend::Redis(redis) => {
                let runner = RedisStreamRunner::new(
                    redis.client.clone(),
                    redis.keyspace.clone(),
                    RedisStreamRunnerConfig {
                        command_timeout_ms: redis_stream_command_timeout_for_block(
                            redis.command_timeout_ms,
                            block_ms,
                        ),
                        read_block_ms: block_ms,
                        read_count: count.max(1),
                    },
                )?;
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
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => {
                Ok(memory.queue_claim_stale(stream, config).await)
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
        match self.backend.as_ref() {
            RuntimeStateBackend::Memory(_) => Ok(ids.len()),
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
        let now_ms = unix_time_ms();
        let expires_at_ms = now_ms.saturating_add(self.config.lease_ttl_ms);
        let key = redis.keyspace.key(&self.key);
        let result = self
            .run_redis("acquire", redis, async {
                let mut connection = redis
                    .client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(|err| self.unavailable(format!("connect failed: {err}")))?;
                redis_script(
                    "redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', ARGV[1]); \
                     local count = redis.call('ZCARD', KEYS[1]); \
                     if count >= tonumber(ARGV[3]) then \
                        redis.call('PEXPIRE', KEYS[1], ARGV[5]); \
                        return {0, count}; \
                     end; \
                     redis.call('ZADD', KEYS[1], ARGV[2], ARGV[4]); \
                     count = redis.call('ZCARD', KEYS[1]); \
                     redis.call('PEXPIRE', KEYS[1], ARGV[5]); \
                     return {1, count};",
                )
                .key(&key)
                .arg(now_ms as i64)
                .arg(expires_at_ms as i64)
                .arg(self.limit as i64)
                .arg(token)
                .arg(self.config.lease_ttl_ms as i64)
                .invoke_async::<(i64, i64)>(&mut connection)
                .await
                .map_err(|err| self.unavailable(format!("acquire failed: {err}")))
            })
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
                let now_ms = unix_time_ms();
                let expires_at_ms = now_ms.saturating_add(self.config.lease_ttl_ms);
                let key = redis.keyspace.key(&self.key);
                let renewed = self
                    .run_redis("renew", redis, async {
                        let mut connection = redis
                            .client
                            .get_multiplexed_async_connection()
                            .await
                            .map_err(|err| self.unavailable(format!("connect failed: {err}")))?;
                        redis_script(
                            "redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', ARGV[1]); \
                             local score = redis.call('ZSCORE', KEYS[1], ARGV[2]); \
                             if not score then return 0; end; \
                             redis.call('ZADD', KEYS[1], 'XX', ARGV[3], ARGV[2]); \
                             redis.call('PEXPIRE', KEYS[1], ARGV[4]); \
                             return 1;",
                        )
                        .key(&key)
                        .arg(now_ms as i64)
                        .arg(token)
                        .arg(expires_at_ms as i64)
                        .arg(self.config.lease_ttl_ms as i64)
                        .invoke_async::<i64>(&mut connection)
                        .await
                        .map_err(|err| self.unavailable(format!("renew failed: {err}")))
                    })
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
                let key = redis.keyspace.key(&self.key);
                self.run_redis("release", redis, async {
                    let mut connection = redis
                        .client
                        .get_multiplexed_async_connection()
                        .await
                        .map_err(|err| self.unavailable(format!("connect failed: {err}")))?;
                    redis_script(
                        "local removed = redis.call('ZREM', KEYS[1], ARGV[1]); \
                         if removed > 0 and redis.call('ZCARD', KEYS[1]) == 0 then \
                            redis.call('DEL', KEYS[1]); \
                         end; \
                         return removed;",
                    )
                    .key(&key)
                    .arg(token)
                    .invoke_async::<i64>(&mut connection)
                    .await
                    .map_err(|err| self.unavailable(format!("release failed: {err}")))?;
                    Ok(())
                })
                .await
            }
        }
    }

    async fn live_count(&self) -> Result<usize, RuntimeSemaphoreError> {
        let count = match self.runtime.backend.as_ref() {
            RuntimeStateBackend::Memory(memory) => memory.semaphore_live_count(&self.key).await,
            RuntimeStateBackend::Redis(redis) => {
                let now_ms = unix_time_ms();
                let key = redis.keyspace.key(&self.key);
                self.run_redis("snapshot", redis, async {
                    let mut connection = redis
                        .client
                        .get_multiplexed_async_connection()
                        .await
                        .map_err(|err| self.unavailable(format!("connect failed: {err}")))?;
                    redis_script(
                        "redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', ARGV[1]); \
                         return redis.call('ZCARD', KEYS[1]);",
                    )
                    .key(&key)
                    .arg(now_ms as i64)
                    .invoke_async::<i64>(&mut connection)
                    .await
                    .map(|value| value.max(0) as usize)
                    .map_err(|err| self.unavailable(format!("snapshot failed: {err}")))
                })
                .await?
            }
        };
        self.observe_in_flight(count);
        Ok(count)
    }

    async fn run_redis<T, F>(
        &self,
        operation: &'static str,
        redis: &RedisRuntimeBackend,
        future: F,
    ) -> Result<T, RuntimeSemaphoreError>
    where
        F: Future<Output = Result<T, RuntimeSemaphoreError>>,
    {
        if let Some(timeout_ms) = self.config.command_timeout_ms.or(redis.command_timeout_ms) {
            tokio::time::timeout(Duration::from_millis(timeout_ms), future)
                .await
                .map_err(|_| {
                    self.unavailable(format!("{operation} exceeded {timeout_ms}ms timeout"))
                })?
        } else {
            future.await
        }
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

async fn ping_redis(client: &RedisClient, timeout_ms: Option<u64>) -> Result<(), DataLayerError> {
    run_redis_with_timeout(timeout_ms, "runtime redis ping", async {
        let mut connection = client
            .get_multiplexed_async_connection()
            .await
            .map_redis_err()?;
        let pong = redis_cmd("PING")
            .query_async::<String>(&mut connection)
            .await
            .map_redis_err()?;
        if pong.eq_ignore_ascii_case("PONG") {
            Ok(())
        } else {
            Err(DataLayerError::UnexpectedValue(format!(
                "unexpected runtime redis ping response {pong}"
            )))
        }
    })
    .await
}

async fn run_redis_with_timeout<T, F>(
    timeout_ms: Option<u64>,
    operation: &'static str,
    future: F,
) -> Result<T, DataLayerError>
where
    F: Future<Output = Result<T, DataLayerError>>,
{
    if let Some(timeout_ms) = timeout_ms {
        tokio::time::timeout(Duration::from_millis(timeout_ms), future)
            .await
            .map_err(|_| {
                DataLayerError::TimedOut(format!("{operation} exceeded {timeout_ms}ms timeout"))
            })?
    } else {
        future.await
    }
}

async fn redis_query_i64(
    redis: &RedisRuntimeBackend,
    operation: &'static str,
    command: RedisCmd,
) -> Result<i64, DataLayerError> {
    run_redis_with_timeout(redis.command_timeout_ms, operation, async {
        let mut connection = redis
            .client
            .get_multiplexed_async_connection()
            .await
            .map_redis_err()?;
        command
            .query_async::<i64>(&mut connection)
            .await
            .map_redis_err()
    })
    .await
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_counter_key(key: &str) -> Result<(), DataLayerError> {
    if key.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "runtime counter key cannot be empty".to_string(),
        ));
    }
    Ok(())
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
    async fn memory_counter_increment_is_monotonic_and_readable() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());

        assert_eq!(runtime.counter_get("counter:test").await.expect("get"), 0);
        assert_eq!(
            runtime
                .counter_increment("counter:test")
                .await
                .expect("first increment"),
            1
        );
        assert_eq!(
            runtime
                .counter_increment("counter:test")
                .await
                .expect("second increment"),
            2
        );
        assert_eq!(runtime.counter_get("counter:test").await.expect("get"), 2);
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
                .check_and_consume_rate_limit(input)
                .await
                .expect("second"),
            RateLimitCheck::Rejected {
                scope: RateLimitScope::User,
                limit: 1
            }
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
}
