use crate::error::RedisResultExt;
use crate::redis::RedisKeyspace;
use crate::DataLayerError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

pub(crate) type RedisClient = redis::Client;
pub(crate) type RedisManagedConnection = redis::aio::ConnectionManager;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RedisClientConfig {
    pub url: String,
    pub key_prefix: Option<String>,
}

impl RedisClientConfig {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        let raw = self.url.trim();
        if raw.is_empty() {
            return Err(DataLayerError::InvalidConfiguration(
                "redis url cannot be empty".to_string(),
            ));
        }
        url::Url::parse(raw).map_err(|err| {
            DataLayerError::InvalidConfiguration(format!("invalid redis url: {err}"))
        })?;
        Ok(())
    }

    pub fn keyspace(&self) -> RedisKeyspace {
        RedisKeyspace::new(self.key_prefix.as_deref())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RedisClientFactory {
    config: RedisClientConfig,
}

impl RedisClientFactory {
    pub(crate) fn new(config: RedisClientConfig) -> Result<Self, DataLayerError> {
        config.validate()?;
        Ok(Self { config })
    }

    pub(crate) fn config(&self) -> &RedisClientConfig {
        &self.config
    }

    pub(crate) fn connect_lazy(&self) -> Result<RedisClient, DataLayerError> {
        RedisClient::open(self.config.url.clone()).map_redis_err()
    }

    pub(crate) async fn connect_router(
        &self,
        command_timeout_ms: Option<u64>,
    ) -> Result<RedisConnectionRouter, DataLayerError> {
        RedisConnectionRouter::connect(self.connect_lazy()?, command_timeout_ms).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedisConnectionLane {
    Fast,
    Stream,
    BlockingStream,
    Admin,
}

impl RedisConnectionLane {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Stream => "stream",
            Self::BlockingStream => "blocking_stream",
            Self::Admin => "admin",
        }
    }
}

#[derive(Clone)]
pub(crate) struct RedisConnectionRouter {
    fast: RedisManagedConnection,
    stream: RedisManagedConnection,
    blocking_stream: RedisManagedConnection,
    admin: RedisManagedConnection,
    metrics: Arc<RedisConnectionMetrics>,
}

impl std::fmt::Debug for RedisConnectionRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisConnectionRouter")
            .field("lanes", &["fast", "stream", "blocking_stream", "admin"])
            .finish()
    }
}

impl RedisConnectionRouter {
    pub(crate) async fn connect(
        client: RedisClient,
        command_timeout_ms: Option<u64>,
    ) -> Result<Self, DataLayerError> {
        let fast = connect_lane(
            &client,
            connection_manager_config(command_timeout_ms),
            RedisConnectionLane::Fast,
        )
        .await?;
        let stream = connect_lane(
            &client,
            connection_manager_config(command_timeout_ms),
            RedisConnectionLane::Stream,
        )
        .await?;
        let blocking_stream = connect_lane(
            &client,
            connection_manager_config(command_timeout_ms),
            RedisConnectionLane::BlockingStream,
        )
        .await?;
        let admin = connect_lane(
            &client,
            connection_manager_config(command_timeout_ms),
            RedisConnectionLane::Admin,
        )
        .await?;
        info!(
            redis_lanes = "fast,stream,blocking_stream,admin",
            "runtime redis connection lanes initialized"
        );
        Ok(Self {
            fast,
            stream,
            blocking_stream,
            admin,
            metrics: Arc::new(RedisConnectionMetrics::default()),
        })
    }

    pub(crate) fn connection(&self, lane: RedisConnectionLane) -> RedisManagedConnection {
        match lane {
            RedisConnectionLane::Fast => self.fast.clone(),
            RedisConnectionLane::Stream => self.stream.clone(),
            RedisConnectionLane::BlockingStream => self.blocking_stream.clone(),
            RedisConnectionLane::Admin => self.admin.clone(),
        }
    }

    pub(crate) fn record_error(&self, lane: RedisConnectionLane) {
        self.metrics
            .for_lane(lane)
            .errors
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_timeout(&self, lane: RedisConnectionLane) {
        self.metrics
            .for_lane(lane)
            .timeouts
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn lane_diagnostics(&self) -> Vec<RedisLaneDiagnostics> {
        [
            RedisConnectionLane::Fast,
            RedisConnectionLane::Stream,
            RedisConnectionLane::BlockingStream,
            RedisConnectionLane::Admin,
        ]
        .into_iter()
        .map(|lane| {
            let metrics = self.metrics.for_lane(lane);
            RedisLaneDiagnostics {
                lane: lane.as_str(),
                command_errors: metrics.errors.load(Ordering::Relaxed),
                command_timeouts: metrics.timeouts.load(Ordering::Relaxed),
            }
        })
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RedisLaneDiagnostics {
    pub lane: &'static str,
    pub command_errors: u64,
    pub command_timeouts: u64,
}

#[derive(Default)]
struct RedisConnectionMetrics {
    fast: RedisLaneMetrics,
    stream: RedisLaneMetrics,
    blocking_stream: RedisLaneMetrics,
    admin: RedisLaneMetrics,
}

impl RedisConnectionMetrics {
    fn for_lane(&self, lane: RedisConnectionLane) -> &RedisLaneMetrics {
        match lane {
            RedisConnectionLane::Fast => &self.fast,
            RedisConnectionLane::Stream => &self.stream,
            RedisConnectionLane::BlockingStream => &self.blocking_stream,
            RedisConnectionLane::Admin => &self.admin,
        }
    }
}

#[derive(Default)]
struct RedisLaneMetrics {
    errors: AtomicU64,
    timeouts: AtomicU64,
}

fn connection_manager_config(
    command_timeout_ms: Option<u64>,
) -> redis::aio::ConnectionManagerConfig {
    let mut config = redis::aio::ConnectionManagerConfig::new();
    if let Some(timeout_ms) = command_timeout_ms {
        config = config.set_connection_timeout(Duration::from_millis(timeout_ms));
    }
    config
}

async fn connect_lane(
    client: &RedisClient,
    config: redis::aio::ConnectionManagerConfig,
    lane: RedisConnectionLane,
) -> Result<RedisManagedConnection, DataLayerError> {
    client
        .get_connection_manager_with_config(config)
        .await
        .map_err(|err| {
            DataLayerError::Redis(format!(
                "failed to initialize runtime redis {} lane: {err}",
                lane.as_str()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::{RedisClientConfig, RedisClientFactory};

    #[test]
    fn factory_builds_lazy_client_from_valid_config() {
        let config = RedisClientConfig {
            url: "redis://127.0.0.1/0".to_string(),
            key_prefix: Some("aether".to_string()),
        };
        let factory = RedisClientFactory::new(config.clone()).expect("factory should build");

        assert_eq!(factory.config(), &config);
        let _client = factory
            .connect_lazy()
            .expect("lazy redis client should build");
    }
}
