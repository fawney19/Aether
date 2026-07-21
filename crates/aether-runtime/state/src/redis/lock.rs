use std::future::Future;

use crate::error::RedisResultExt;
use crate::redis::{
    run_lane_with_timeout, RedisClientConfig, RedisClientFactory, RedisConnectionLane,
    RedisConnectionRouter, RedisKeyspace,
};
use crate::DataLayerError;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RedisLockKey(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisLockLease {
    pub key: RedisLockKey,
    pub owner: String,
    pub token: String,
    pub fencing_token: u64,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedisLockRunnerConfig {
    pub command_timeout_ms: Option<u64>,
    pub default_ttl_ms: u64,
}

impl Default for RedisLockRunnerConfig {
    fn default() -> Self {
        Self {
            command_timeout_ms: Some(1_000),
            default_ttl_ms: 15_000,
        }
    }
}

impl RedisLockRunnerConfig {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        if matches!(self.command_timeout_ms, Some(0)) {
            return Err(DataLayerError::InvalidConfiguration(
                "redis lock command_timeout_ms must be positive".to_string(),
            ));
        }
        if self.default_ttl_ms == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "redis lock default_ttl_ms must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RedisLockRunner {
    connections: RedisConnectionRouter,
    keyspace: RedisKeyspace,
    config: RedisLockRunnerConfig,
}

impl RedisLockRunner {
    pub(crate) fn new(
        connections: RedisConnectionRouter,
        keyspace: RedisKeyspace,
        config: RedisLockRunnerConfig,
    ) -> Result<Self, DataLayerError> {
        config.validate()?;
        Ok(Self {
            connections,
            keyspace,
            config,
        })
    }

    pub async fn from_config(
        config: RedisClientConfig,
        runner_config: RedisLockRunnerConfig,
    ) -> Result<Self, DataLayerError> {
        let factory = RedisClientFactory::new(config)?;
        let keyspace = factory.config().keyspace();
        let connections = factory
            .connect_router(runner_config.command_timeout_ms)
            .await?;
        Self::new(connections, keyspace, runner_config)
    }

    pub fn keyspace(&self) -> &RedisKeyspace {
        &self.keyspace
    }

    pub fn config(&self) -> RedisLockRunnerConfig {
        self.config
    }

    pub async fn try_acquire(
        &self,
        key: &RedisLockKey,
        owner: &str,
        ttl_ms: Option<u64>,
    ) -> Result<Option<RedisLockLease>, DataLayerError> {
        validate_owner(owner)?;
        validate_key(key)?;
        let ttl_ms = self.resolve_ttl_ms(ttl_ms)?;
        let token = format!("{owner}:{}", Uuid::new_v4());
        let fencing_key = format!("{}:fencing", key.0);

        self.run_with_timeout(RedisConnectionLane::Fast, "redis lock acquire", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Fast);
            let fencing_token = redis::Script::new(
                "local acquired = redis.call('set', KEYS[1], ARGV[1], 'NX', 'PX', ARGV[2]) \n\
                 if not acquired then return 0 end \n\
                 return redis.call('incr', KEYS[2])",
            )
            .key(&key.0)
            .key(&fencing_key)
            .arg(&token)
            .arg(ttl_ms)
            .invoke_async::<i64>(&mut connection)
            .await
            .map_redis_err()?;

            Ok((fencing_token > 0).then(|| RedisLockLease {
                key: key.clone(),
                owner: owner.to_string(),
                token,
                fencing_token: u64::try_from(fencing_token).unwrap_or(u64::MAX),
                ttl_ms,
            }))
        })
        .await
    }

    pub async fn release(&self, lease: &RedisLockLease) -> Result<bool, DataLayerError> {
        validate_lease(lease)?;
        self.run_with_timeout(RedisConnectionLane::Fast, "redis lock release", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Fast);
            let deleted = redis::Script::new(
                "if redis.call('get', KEYS[1]) == ARGV[1] then \
                     return redis.call('del', KEYS[1]) \
                 else \
                     return 0 \
                 end",
            )
            .key(&lease.key.0)
            .arg(&lease.token)
            .invoke_async::<i32>(&mut connection)
            .await
            .map_redis_err()?;
            Ok(deleted > 0)
        })
        .await
    }

    pub async fn renew(
        &self,
        lease: &RedisLockLease,
        ttl_ms: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        validate_lease(lease)?;
        let ttl_ms = self.resolve_ttl_ms(ttl_ms)?;

        self.run_with_timeout(RedisConnectionLane::Fast, "redis lock renew", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Fast);
            let renewed = redis::Script::new(
                "if redis.call('get', KEYS[1]) == ARGV[1] then \
                     return redis.call('pexpire', KEYS[1], ARGV[2]) \
                 else \
                     return 0 \
                 end",
            )
            .key(&lease.key.0)
            .arg(&lease.token)
            .arg(ttl_ms)
            .invoke_async::<i32>(&mut connection)
            .await
            .map_redis_err()?;
            Ok(renewed > 0)
        })
        .await
    }

    async fn run_with_timeout<T, F>(
        &self,
        lane: RedisConnectionLane,
        operation: &'static str,
        future: F,
    ) -> Result<T, DataLayerError>
    where
        F: Future<Output = Result<T, DataLayerError>>,
    {
        run_lane_with_timeout(
            &self.connections,
            lane,
            self.config.command_timeout_ms,
            operation,
            future,
        )
        .await
    }

    fn resolve_ttl_ms(&self, ttl_ms: Option<u64>) -> Result<u64, DataLayerError> {
        let ttl_ms = ttl_ms.unwrap_or(self.config.default_ttl_ms);
        if ttl_ms == 0 {
            return Err(DataLayerError::InvalidInput(
                "redis lock ttl_ms must be positive".to_string(),
            ));
        }
        Ok(ttl_ms)
    }
}

fn validate_owner(owner: &str) -> Result<(), DataLayerError> {
    if owner.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "redis lock owner cannot be empty".to_string(),
        ));
    }
    Ok(())
}

fn validate_key(key: &RedisLockKey) -> Result<(), DataLayerError> {
    if key.0.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "redis lock key cannot be empty".to_string(),
        ));
    }
    Ok(())
}

fn validate_lease(lease: &RedisLockLease) -> Result<(), DataLayerError> {
    validate_key(&lease.key)?;
    validate_owner(&lease.owner)?;
    if lease.token.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "redis lock token cannot be empty".to_string(),
        ));
    }
    if lease.fencing_token == 0 {
        return Err(DataLayerError::InvalidInput(
            "redis lock fencing_token must be positive".to_string(),
        ));
    }
    if lease.ttl_ms == 0 {
        return Err(DataLayerError::InvalidInput(
            "redis lock ttl_ms must be positive".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        validate_key, validate_lease, validate_owner, RedisLockKey, RedisLockLease,
        RedisLockRunnerConfig,
    };

    #[test]
    fn validates_runner_config() {
        assert!(RedisLockRunnerConfig {
            command_timeout_ms: Some(0),
            ..RedisLockRunnerConfig::default()
        }
        .validate()
        .is_err());
        assert!(RedisLockRunnerConfig {
            default_ttl_ms: 0,
            ..RedisLockRunnerConfig::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn runner_reuses_client_and_keyspace() {
        RedisLockRunnerConfig::default()
            .validate()
            .expect("default lock config should be valid");
    }

    #[test]
    fn rejects_invalid_owner_or_lease_before_network() {
        assert!(validate_owner("").is_err());
        assert!(validate_key(&RedisLockKey(String::new())).is_err());
        assert!(validate_lease(&RedisLockLease {
            key: RedisLockKey("aether:lock:poller".to_string()),
            owner: "worker-1".to_string(),
            token: String::new(),
            fencing_token: 1,
            ttl_ms: 1_000,
        })
        .is_err());
    }
}
