mod client;
mod kv;
mod lock;
mod namespace;
mod runtime;
mod stream;

pub use client::{RedisClientConfig, RedisLaneDiagnostics};
pub use kv::{RedisKvRunner, RedisKvRunnerConfig};
pub use lock::{RedisLockKey, RedisLockLease, RedisLockRunner, RedisLockRunnerConfig};
pub use namespace::RedisKeyspace;
pub use runtime::RedisRuntimeDiagnostics;
pub use stream::{
    RedisConsumerGroup, RedisConsumerName, RedisStreamEntry, RedisStreamName,
    RedisStreamReclaimConfig, RedisStreamReclaimResult, RedisStreamRunner, RedisStreamRunnerConfig,
};

pub(crate) type RedisCmd = redis::Cmd;
pub(crate) type RedisScript = redis::Script;

pub(crate) use client::{RedisClientFactory, RedisConnectionLane, RedisConnectionRouter};
pub(crate) use runtime::RedisRuntimeRunner;

pub(crate) fn cmd(name: &str) -> RedisCmd {
    redis::cmd(name)
}

pub(crate) fn script(source: &str) -> RedisScript {
    redis::Script::new(source)
}

pub(crate) async fn run_lane_with_timeout<T, F>(
    connections: &RedisConnectionRouter,
    lane: RedisConnectionLane,
    timeout_ms: Option<u64>,
    operation: &'static str,
    future: F,
) -> Result<T, crate::DataLayerError>
where
    F: std::future::Future<Output = Result<T, crate::DataLayerError>>,
{
    let result = if let Some(timeout_ms) = timeout_ms {
        match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), future).await {
            Ok(result) => result,
            Err(_) => {
                connections.record_timeout(lane);
                return Err(crate::DataLayerError::TimedOut(format!(
                    "{operation} exceeded {timeout_ms}ms timeout"
                )));
            }
        }
    } else {
        future.await
    };
    if result.is_err() {
        connections.record_error(lane);
    }
    result
}
