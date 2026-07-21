use crate::error::RedisResultExt;
use crate::redis::RedisKeyspace;
use crate::DataLayerError;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

pub(crate) type RedisClient = redis::Client;
pub(crate) type RedisManagedConnection = redis::aio::ConnectionManager;

const DEFAULT_STREAM_LANES: usize = 4;
const DEFAULT_BLOCKING_STREAM_LANES_FALLBACK: usize = 4;
const DEFAULT_BLOCKING_STREAM_LANES_CAP: usize = 16;
const MAX_BLOCKING_STREAM_LANES_CAP: usize = 64;
pub(crate) const REDIS_COMMAND_LATENCY_BUCKETS_MS: [u64; 12] =
    [1, 5, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000];
const REDIS_COMMAND_LATENCY_BUCKET_COUNT: usize = REDIS_COMMAND_LATENCY_BUCKETS_MS.len() + 1;

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
        self.connect_router_with_blocking_stream_lanes(command_timeout_ms, None)
            .await
    }

    pub(crate) async fn connect_router_with_blocking_stream_lanes(
        &self,
        command_timeout_ms: Option<u64>,
        blocking_stream_lanes: Option<usize>,
    ) -> Result<RedisConnectionRouter, DataLayerError> {
        RedisConnectionRouter::connect(
            self.connect_lazy()?,
            command_timeout_ms,
            blocking_stream_lanes,
        )
        .await
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
    stream: Arc<Vec<RedisManagedConnection>>,
    stream_next: Arc<AtomicUsize>,
    blocking_stream: Arc<Vec<RedisManagedConnection>>,
    blocking_stream_next: Arc<AtomicUsize>,
    admin: RedisManagedConnection,
    metrics: Arc<RedisConnectionMetrics>,
}

impl std::fmt::Debug for RedisConnectionRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisConnectionRouter")
            .field("lanes", &["fast", "stream", "blocking_stream", "admin"])
            .field("stream_lanes", &self.stream.len())
            .field("blocking_stream_lanes", &self.blocking_stream.len())
            .finish()
    }
}

impl RedisConnectionRouter {
    pub(crate) async fn connect(
        client: RedisClient,
        command_timeout_ms: Option<u64>,
        blocking_stream_lanes: Option<usize>,
    ) -> Result<Self, DataLayerError> {
        let fast = connect_lane(
            &client,
            connection_manager_config(command_timeout_ms),
            RedisConnectionLane::Fast,
            command_timeout_ms,
        )
        .await?;
        let stream = connect_stream_lanes(&client, command_timeout_ms).await?;
        let blocking_stream =
            connect_blocking_stream_lanes(&client, command_timeout_ms, blocking_stream_lanes)
                .await?;
        let admin = connect_lane(
            &client,
            connection_manager_config(command_timeout_ms),
            RedisConnectionLane::Admin,
            command_timeout_ms,
        )
        .await?;
        let stream_lanes = stream.len();
        let blocking_stream_lanes = blocking_stream.len();
        info!(
            redis_lanes = "fast,stream,blocking_stream,admin",
            redis_stream_lanes = stream_lanes,
            redis_blocking_stream_lanes = blocking_stream_lanes,
            "runtime redis connection lanes initialized"
        );
        Ok(Self {
            fast,
            stream: Arc::new(stream),
            stream_next: Arc::new(AtomicUsize::new(0)),
            blocking_stream: Arc::new(blocking_stream),
            blocking_stream_next: Arc::new(AtomicUsize::new(0)),
            admin,
            metrics: Arc::new(RedisConnectionMetrics::default()),
        })
    }

    pub(crate) fn connection(&self, lane: RedisConnectionLane) -> RedisManagedConnection {
        match lane {
            RedisConnectionLane::Fast => self.fast.clone(),
            RedisConnectionLane::Stream => {
                let index = next_lane_index(&self.stream_next, self.stream.len());
                self.stream[index].clone()
            }
            RedisConnectionLane::BlockingStream => {
                let index = next_lane_index(&self.blocking_stream_next, self.blocking_stream.len());
                self.blocking_stream[index].clone()
            }
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

    pub(crate) fn record_latency(&self, lane: RedisConnectionLane, elapsed: Duration) {
        self.metrics.for_lane(lane).record_latency(elapsed);
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
                command_count: metrics.command_count.load(Ordering::Relaxed),
                command_latency_total_ms: metrics.latency_total_ms.load(Ordering::Relaxed),
                command_latency_max_ms: metrics.latency_max_ms.load(Ordering::Relaxed),
                command_latency_buckets: metrics.latency_buckets(),
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
    pub command_count: u64,
    pub command_latency_total_ms: u64,
    pub command_latency_max_ms: u64,
    pub command_latency_buckets: Vec<RedisCommandLatencyBucket>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RedisCommandLatencyBucket {
    pub le_ms: Option<u64>,
    pub count: u64,
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

struct RedisLaneMetrics {
    errors: AtomicU64,
    timeouts: AtomicU64,
    command_count: AtomicU64,
    latency_total_ms: AtomicU64,
    latency_max_ms: AtomicU64,
    latency_bucket_counts: [AtomicU64; REDIS_COMMAND_LATENCY_BUCKET_COUNT],
}

impl Default for RedisLaneMetrics {
    fn default() -> Self {
        Self {
            errors: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
            command_count: AtomicU64::new(0),
            latency_total_ms: AtomicU64::new(0),
            latency_max_ms: AtomicU64::new(0),
            latency_bucket_counts: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl RedisLaneMetrics {
    fn record_latency(&self, elapsed: Duration) {
        let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        self.command_count.fetch_add(1, Ordering::Relaxed);
        self.latency_total_ms
            .fetch_add(elapsed_ms, Ordering::Relaxed);
        update_atomic_max(&self.latency_max_ms, elapsed_ms);

        let bucket_index = REDIS_COMMAND_LATENCY_BUCKETS_MS
            .iter()
            .position(|upper_bound_ms| elapsed_ms <= *upper_bound_ms)
            .unwrap_or(REDIS_COMMAND_LATENCY_BUCKETS_MS.len());
        self.latency_bucket_counts[bucket_index].fetch_add(1, Ordering::Relaxed);
    }

    fn latency_buckets(&self) -> Vec<RedisCommandLatencyBucket> {
        let mut cumulative = 0u64;
        let mut buckets = Vec::with_capacity(REDIS_COMMAND_LATENCY_BUCKET_COUNT);
        for (index, upper_bound_ms) in REDIS_COMMAND_LATENCY_BUCKETS_MS.iter().enumerate() {
            cumulative = cumulative
                .saturating_add(self.latency_bucket_counts[index].load(Ordering::Relaxed));
            buckets.push(RedisCommandLatencyBucket {
                le_ms: Some(*upper_bound_ms),
                count: cumulative,
            });
        }
        cumulative = cumulative.saturating_add(
            self.latency_bucket_counts[REDIS_COMMAND_LATENCY_BUCKETS_MS.len()]
                .load(Ordering::Relaxed),
        );
        buckets.push(RedisCommandLatencyBucket {
            le_ms: None,
            count: cumulative,
        });
        buckets
    }
}

fn update_atomic_max(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(next) => current = next,
        }
    }
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

async fn connect_stream_lanes(
    client: &RedisClient,
    command_timeout_ms: Option<u64>,
) -> Result<Vec<RedisManagedConnection>, DataLayerError> {
    let lane_count = stream_lane_count();
    let mut lanes = Vec::with_capacity(lane_count);
    for _ in 0..lane_count {
        lanes.push(
            connect_lane(
                client,
                connection_manager_config(command_timeout_ms),
                RedisConnectionLane::Stream,
                command_timeout_ms,
            )
            .await?,
        );
    }
    Ok(lanes)
}

const fn stream_lane_count() -> usize {
    DEFAULT_STREAM_LANES
}

fn next_lane_index(next: &AtomicUsize, lane_count: usize) -> usize {
    debug_assert!(lane_count > 0, "redis connection lane must not be empty");
    next.fetch_add(1, Ordering::Relaxed) % lane_count
}

async fn connect_blocking_stream_lanes(
    client: &RedisClient,
    command_timeout_ms: Option<u64>,
    requested_lanes: Option<usize>,
) -> Result<Vec<RedisManagedConnection>, DataLayerError> {
    let lane_count = blocking_stream_lane_count(requested_lanes)?;
    let mut lanes = Vec::with_capacity(lane_count);
    for _ in 0..lane_count {
        lanes.push(
            connect_lane(
                client,
                connection_manager_config(command_timeout_ms),
                RedisConnectionLane::BlockingStream,
                command_timeout_ms,
            )
            .await?,
        );
    }
    Ok(lanes)
}

fn blocking_stream_lane_count(requested_lanes: Option<usize>) -> Result<usize, DataLayerError> {
    if matches!(requested_lanes, Some(0)) {
        return Err(DataLayerError::InvalidConfiguration(
            "runtime redis blocking_stream_lanes must be positive".to_string(),
        ));
    }

    let default_lanes = default_blocking_stream_lane_count();
    Ok(requested_lanes
        .map(|lanes| lanes.max(default_lanes))
        .unwrap_or(default_lanes)
        .clamp(1, MAX_BLOCKING_STREAM_LANES_CAP))
}

fn default_blocking_stream_lane_count() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(DEFAULT_BLOCKING_STREAM_LANES_FALLBACK)
        .clamp(
            DEFAULT_BLOCKING_STREAM_LANES_FALLBACK,
            DEFAULT_BLOCKING_STREAM_LANES_CAP,
        )
}

async fn connect_lane(
    client: &RedisClient,
    config: redis::aio::ConnectionManagerConfig,
    lane: RedisConnectionLane,
    command_timeout_ms: Option<u64>,
) -> Result<RedisManagedConnection, DataLayerError> {
    let connect = client.get_connection_manager_with_config(config);
    let result = if let Some(timeout_ms) = command_timeout_ms {
        match tokio::time::timeout(Duration::from_millis(timeout_ms), connect).await {
            Ok(result) => result,
            Err(_) => {
                return Err(DataLayerError::TimedOut(format!(
                    "runtime redis {} lane connection exceeded {}ms timeout",
                    lane.as_str(),
                    timeout_ms
                )));
            }
        }
    } else {
        connect.await
    };
    result.map_err(|err| {
        DataLayerError::Redis(format!(
            "failed to initialize runtime redis {} lane: {err}",
            lane.as_str()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        blocking_stream_lane_count, default_blocking_stream_lane_count, next_lane_index,
        stream_lane_count, RedisClientConfig, RedisClientFactory, RedisLaneMetrics,
        DEFAULT_STREAM_LANES, MAX_BLOCKING_STREAM_LANES_CAP, REDIS_COMMAND_LATENCY_BUCKETS_MS,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

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

    #[test]
    fn blocking_stream_lane_count_uses_requested_as_floor() {
        let default_lanes = default_blocking_stream_lane_count();

        assert_eq!(
            blocking_stream_lane_count(None).expect("default lanes"),
            default_lanes
        );
        assert_eq!(
            blocking_stream_lane_count(Some(1)).expect("requested below default"),
            default_lanes
        );
        assert_eq!(
            blocking_stream_lane_count(Some(default_lanes + 1)).expect("requested above default"),
            default_lanes + 1
        );
        assert_eq!(
            blocking_stream_lane_count(Some(MAX_BLOCKING_STREAM_LANES_CAP + 1))
                .expect("requested above cap"),
            MAX_BLOCKING_STREAM_LANES_CAP
        );
        assert!(blocking_stream_lane_count(Some(0)).is_err());
    }

    #[test]
    fn stream_lane_count_uses_fixed_default() {
        assert_eq!(stream_lane_count(), DEFAULT_STREAM_LANES);
        assert_eq!(stream_lane_count(), 4);
    }

    #[test]
    fn lane_index_round_robins_across_all_connections() {
        let next = AtomicUsize::new(0);

        let indexes = (0..10)
            .map(|_| next_lane_index(&next, stream_lane_count()))
            .collect::<Vec<_>>();

        assert_eq!(indexes, vec![0, 1, 2, 3, 0, 1, 2, 3, 0, 1]);
    }

    #[test]
    fn lane_index_round_robin_survives_counter_wraparound() {
        let next = AtomicUsize::new(usize::MAX - 1);

        let indexes = (0..3)
            .map(|_| next_lane_index(&next, stream_lane_count()))
            .collect::<Vec<_>>();

        assert_eq!(indexes, vec![2, 3, 0]);
    }

    #[test]
    fn lane_metrics_record_cumulative_latency_buckets() {
        let metrics = RedisLaneMetrics::default();

        metrics.record_latency(Duration::from_millis(0));
        metrics.record_latency(Duration::from_millis(12));
        metrics.record_latency(Duration::from_millis(12_345));

        assert_eq!(metrics.command_count.load(Ordering::Relaxed), 3);
        assert_eq!(metrics.latency_total_ms.load(Ordering::Relaxed), 12_357);
        assert_eq!(metrics.latency_max_ms.load(Ordering::Relaxed), 12_345);

        let buckets = metrics.latency_buckets();
        let le_1 = buckets
            .iter()
            .find(|bucket| bucket.le_ms == Some(1))
            .expect("1ms bucket");
        let le_25 = buckets
            .iter()
            .find(|bucket| bucket.le_ms == Some(25))
            .expect("25ms bucket");
        let plus_inf = buckets.last().expect("+Inf bucket");

        assert_eq!(buckets.len(), REDIS_COMMAND_LATENCY_BUCKETS_MS.len() + 1);
        assert_eq!(le_1.count, 1);
        assert_eq!(le_25.count, 2);
        assert_eq!(plus_inf.le_ms, None);
        assert_eq!(plus_inf.count, 3);
    }
}
