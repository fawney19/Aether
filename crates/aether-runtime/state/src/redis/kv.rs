use std::future::Future;

use crate::error::RedisResultExt;
use crate::redis::{
    run_lane_with_timeout, RedisClientConfig, RedisClientFactory, RedisConnectionLane,
    RedisConnectionRouter, RedisKeyspace,
};
use crate::DataLayerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedisKvRunnerConfig {
    pub command_timeout_ms: Option<u64>,
    pub default_ttl_seconds: u64,
}

impl Default for RedisKvRunnerConfig {
    fn default() -> Self {
        Self {
            command_timeout_ms: Some(1_000),
            default_ttl_seconds: 300,
        }
    }
}

impl RedisKvRunnerConfig {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        if let Some(timeout) = self.command_timeout_ms {
            if timeout == 0 {
                return Err(DataLayerError::InvalidConfiguration(
                    "redis kv command_timeout_ms must be positive".to_string(),
                ));
            }
        }
        if self.default_ttl_seconds == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "redis kv default_ttl_seconds must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RedisKvRunner {
    connections: RedisConnectionRouter,
    keyspace: RedisKeyspace,
    config: RedisKvRunnerConfig,
}

impl RedisKvRunner {
    pub(crate) fn new(
        connections: RedisConnectionRouter,
        keyspace: RedisKeyspace,
        config: RedisKvRunnerConfig,
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
        runner_config: RedisKvRunnerConfig,
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

    pub fn config(&self) -> RedisKvRunnerConfig {
        self.config
    }

    pub async fn setex(
        &self,
        key: &str,
        value: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<String, DataLayerError> {
        let resolved_ttl = ttl_seconds.unwrap_or(self.config.default_ttl_seconds);
        let namespaced_key = self.keyspace.key(key);
        self.run_with_timeout(RedisConnectionLane::Fast, "redis kv setex", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Fast);
            redis::cmd("SETEX")
                .arg(&namespaced_key)
                .arg(resolved_ttl)
                .arg(value)
                .query_async(&mut connection)
                .await
                .map_redis_err()
        })
        .await
    }

    pub async fn get(&self, key: &str) -> Result<Option<String>, DataLayerError> {
        let namespaced_key = self.keyspace.key(key);
        self.run_with_timeout(RedisConnectionLane::Fast, "redis kv get", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Fast);
            redis::cmd("GET")
                .arg(&namespaced_key)
                .query_async(&mut connection)
                .await
                .map_redis_err()
        })
        .await
    }

    pub async fn getdel(&self, key: &str) -> Result<Option<String>, DataLayerError> {
        let namespaced_key = self.keyspace.key(key);
        self.run_with_timeout(RedisConnectionLane::Fast, "redis kv getdel", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Fast);
            redis::cmd("GETDEL")
                .arg(&namespaced_key)
                .query_async(&mut connection)
                .await
                .map_redis_err()
        })
        .await
    }

    pub async fn exists(&self, key: &str) -> Result<bool, DataLayerError> {
        let namespaced_key = self.keyspace.key(key);
        let exists = self
            .run_with_timeout(RedisConnectionLane::Fast, "redis kv exists", async {
                let mut connection = self.connections.connection(RedisConnectionLane::Fast);
                redis::cmd("EXISTS")
                    .arg(&namespaced_key)
                    .query_async::<i64>(&mut connection)
                    .await
                    .map_redis_err()
            })
            .await?;
        Ok(exists > 0)
    }

    pub async fn del(&self, key: &str) -> Result<i64, DataLayerError> {
        let namespaced_key = self.keyspace.key(key);
        self.run_with_timeout(RedisConnectionLane::Fast, "redis kv del", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Fast);
            redis::cmd("DEL")
                .arg(&namespaced_key)
                .query_async(&mut connection)
                .await
                .map_redis_err()
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
}

#[cfg(test)]
mod tests {
    use super::RedisKvRunnerConfig;

    #[test]
    fn validates_default_config() {
        RedisKvRunnerConfig::default()
            .validate()
            .expect("default kv config should be valid");
    }

    #[test]
    fn rejects_zero_default_ttl() {
        let config = RedisKvRunnerConfig {
            command_timeout_ms: Some(100),
            default_ttl_seconds: 0,
        };
        assert!(config.validate().is_err());
    }
}
