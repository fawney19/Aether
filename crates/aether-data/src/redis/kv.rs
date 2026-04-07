use std::future::Future;
use std::time::Duration;

use crate::error::RedisResultExt;
use crate::redis::{RedisClient, RedisKeyspace};
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
    client: RedisClient,
    keyspace: RedisKeyspace,
    config: RedisKvRunnerConfig,
}

impl RedisKvRunner {
    pub fn new(
        client: RedisClient,
        keyspace: RedisKeyspace,
        config: RedisKvRunnerConfig,
    ) -> Result<Self, DataLayerError> {
        config.validate()?;
        Ok(Self {
            client,
            keyspace,
            config,
        })
    }

    pub fn client(&self) -> &RedisClient {
        &self.client
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
        self.run_with_timeout("redis kv setex", async {
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_redis_err()?;
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

    pub async fn del(&self, key: &str) -> Result<i64, DataLayerError> {
        let namespaced_key = self.keyspace.key(key);
        self.run_with_timeout("redis kv del", async {
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_redis_err()?;
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
        operation: &'static str,
        future: F,
    ) -> Result<T, DataLayerError>
    where
        F: Future<Output = Result<T, DataLayerError>>,
    {
        if let Some(timeout_ms) = self.config.command_timeout_ms {
            tokio::time::timeout(Duration::from_millis(timeout_ms), future)
                .await
                .map_err(|_| {
                    DataLayerError::TimedOut(format!("{operation} exceeded {timeout_ms}ms timeout"))
                })?
        } else {
            future.await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RedisKvRunner, RedisKvRunnerConfig};
    use crate::redis::{RedisClientConfig, RedisClientFactory, RedisKeyspace};

    fn build_runner() -> RedisKvRunner {
        let config = RedisClientConfig {
            url: "redis://localhost/0".to_string(),
            key_prefix: Some("aether-test".to_string()),
        };
        let factory = RedisClientFactory::new(config).expect("redis factory");
        let client = factory.connect_lazy().expect("connect");
        let keyspace = factory.config().keyspace();
        RedisKvRunner::new(client, keyspace, RedisKvRunnerConfig::default()).expect("runner build")
    }

    #[test]
    fn runner_reuses_client_keyspace_and_config() {
        let runner = build_runner();
        assert_eq!(
            runner.keyspace().key("kv:setex:1"),
            "aether-test:kv:setex:1"
        );
        assert_eq!(runner.config(), RedisKvRunnerConfig::default());
        let _client = runner.client();
    }

    #[test]
    fn rejects_zero_default_ttl() {
        let config = RedisKvRunnerConfig {
            command_timeout_ms: Some(100),
            default_ttl_seconds: 0,
        };
        assert!(RedisKvRunner::new(
            RedisClientFactory::new(RedisClientConfig {
                url: "redis://localhost/0".to_string(),
                key_prefix: Some("aether-test".to_string()),
            })
            .expect("redis factory")
            .connect_lazy()
            .expect("redis client"),
            RedisKeyspace::new(Some("aether-test")),
            config,
        )
        .is_err());
    }
}
