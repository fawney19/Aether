use std::collections::BTreeMap;
use std::future::Future;

use redis::from_redis_value;
use redis::streams::StreamReadReply;
use redis::Value as RedisValue;

use crate::error::{redis_error, RedisResultExt};
use crate::redis::{
    run_lane_with_timeout, RedisClientConfig, RedisClientFactory, RedisConnectionLane,
    RedisConnectionRouter, RedisKeyspace,
};
use crate::{DataLayerError, RuntimeQueueStats};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RedisStreamName(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RedisConsumerGroup(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RedisConsumerName(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisStreamEntry {
    pub id: String,
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisStreamReclaimResult {
    pub next_start_id: String,
    pub entries: Vec<RedisStreamEntry>,
    pub deleted_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedisStreamReclaimConfig {
    pub min_idle_ms: u64,
    pub count: usize,
}

impl Default for RedisStreamReclaimConfig {
    fn default() -> Self {
        Self {
            min_idle_ms: 60_000,
            count: 32,
        }
    }
}

impl RedisStreamReclaimConfig {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        if self.min_idle_ms == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "redis stream reclaim min_idle_ms must be positive".to_string(),
            ));
        }
        if self.count == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "redis stream reclaim count must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedisStreamRunnerConfig {
    pub command_timeout_ms: Option<u64>,
    pub read_block_ms: Option<u64>,
    pub read_count: usize,
}

impl Default for RedisStreamRunnerConfig {
    fn default() -> Self {
        Self {
            command_timeout_ms: Some(2_000),
            read_block_ms: Some(1_000),
            read_count: 32,
        }
    }
}

impl RedisStreamRunnerConfig {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        if matches!(self.command_timeout_ms, Some(0)) {
            return Err(DataLayerError::InvalidConfiguration(
                "redis stream command_timeout_ms must be positive".to_string(),
            ));
        }
        if matches!(self.read_block_ms, Some(0)) {
            return Err(DataLayerError::InvalidConfiguration(
                "redis stream read_block_ms must be positive".to_string(),
            ));
        }
        if let (Some(command_timeout_ms), Some(read_block_ms)) =
            (self.command_timeout_ms, self.read_block_ms)
        {
            if command_timeout_ms <= read_block_ms {
                return Err(DataLayerError::InvalidConfiguration(
                    "redis stream command_timeout_ms must be greater than read_block_ms"
                        .to_string(),
                ));
            }
        }
        if self.read_count == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "redis stream read_count must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RedisStreamRunner {
    connections: RedisConnectionRouter,
    keyspace: RedisKeyspace,
    config: RedisStreamRunnerConfig,
}

impl RedisStreamRunner {
    pub(crate) fn new(
        connections: RedisConnectionRouter,
        keyspace: RedisKeyspace,
        config: RedisStreamRunnerConfig,
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
        runner_config: RedisStreamRunnerConfig,
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

    pub fn config(&self) -> RedisStreamRunnerConfig {
        self.config
    }

    pub(crate) fn with_config(
        &self,
        config: RedisStreamRunnerConfig,
    ) -> Result<Self, DataLayerError> {
        Self::new(self.connections.clone(), self.keyspace.clone(), config)
    }

    pub async fn ensure_consumer_group(
        &self,
        stream: &RedisStreamName,
        group: &RedisConsumerGroup,
        start_id: &str,
    ) -> Result<(), DataLayerError> {
        validate_stream_name(stream)?;
        validate_group(group)?;
        validate_stream_position(start_id)?;

        self.run_with_timeout(
            RedisConnectionLane::Stream,
            "redis stream ensure consumer group",
            async {
                let mut connection = self.connections.connection(RedisConnectionLane::Stream);
                let result = redis::cmd("XGROUP")
                    .arg("CREATE")
                    .arg(&stream.0)
                    .arg(&group.0)
                    .arg(start_id)
                    .arg("MKSTREAM")
                    .query_async::<String>(&mut connection)
                    .await;

                match result {
                    Ok(_) => Ok(()),
                    Err(err) if err.code() == Some("BUSYGROUP") => Ok(()),
                    Err(err) => Err(redis_error(err)),
                }
            },
        )
        .await
    }

    pub async fn append_fields(
        &self,
        stream: &RedisStreamName,
        fields: &BTreeMap<String, String>,
    ) -> Result<String, DataLayerError> {
        self.append_fields_with_maxlen(stream, fields, None).await
    }

    pub async fn append_fields_with_maxlen(
        &self,
        stream: &RedisStreamName,
        fields: &BTreeMap<String, String>,
        maxlen: Option<usize>,
    ) -> Result<String, DataLayerError> {
        validate_stream_name(stream)?;
        if fields.is_empty() {
            return Err(DataLayerError::InvalidInput(
                "redis stream fields cannot be empty".to_string(),
            ));
        }

        self.run_with_timeout(RedisConnectionLane::Stream, "redis stream append", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Stream);
            let mut command = redis::cmd("XADD");
            command.arg(&stream.0);
            if let Some(maxlen) = maxlen.filter(|value| *value > 0) {
                command.arg("MAXLEN").arg("~").arg(maxlen);
            }
            command.arg("*");
            for (key, value) in fields {
                command.arg(key).arg(value);
            }
            command
                .query_async::<String>(&mut connection)
                .await
                .map_redis_err()
        })
        .await
    }

    pub async fn append_json(
        &self,
        stream: &RedisStreamName,
        field: &str,
        payload: &serde_json::Value,
    ) -> Result<String, DataLayerError> {
        if field.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "redis stream json field cannot be empty".to_string(),
            ));
        }

        let mut fields = BTreeMap::new();
        fields.insert(
            field.to_string(),
            serde_json::to_string(payload).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "failed to serialize redis stream payload: {err}"
                ))
            })?,
        );
        self.append_fields(stream, &fields).await
    }

    pub async fn read_group(
        &self,
        stream: &RedisStreamName,
        group: &RedisConsumerGroup,
        consumer: &RedisConsumerName,
    ) -> Result<Vec<RedisStreamEntry>, DataLayerError> {
        validate_stream_name(stream)?;
        validate_group(group)?;
        validate_consumer(consumer)?;

        let lane = if self.config.read_block_ms.is_some() {
            RedisConnectionLane::BlockingStream
        } else {
            RedisConnectionLane::Stream
        };
        self.run_with_timeout(lane, "redis stream read group", async {
            let mut connection = self.connections.connection(lane);
            let mut command = redis::cmd("XREADGROUP");
            command
                .arg("GROUP")
                .arg(&group.0)
                .arg(&consumer.0)
                .arg("COUNT")
                .arg(self.config.read_count);
            if let Some(block_ms) = self.config.read_block_ms {
                command.arg("BLOCK").arg(block_ms);
            }
            command.arg("STREAMS").arg(&stream.0).arg(">");

            let reply = command
                .query_async::<RedisValue>(&mut connection)
                .await
                .map_redis_err()?;

            parse_stream_read_entries(reply)
        })
        .await
    }

    pub async fn ack(
        &self,
        stream: &RedisStreamName,
        group: &RedisConsumerGroup,
        ids: &[String],
    ) -> Result<usize, DataLayerError> {
        validate_stream_name(stream)?;
        validate_group(group)?;
        if ids.is_empty() {
            return Ok(0);
        }

        self.run_with_timeout(RedisConnectionLane::Stream, "redis stream ack", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Stream);
            let mut command = redis::cmd("XACK");
            command.arg(&stream.0).arg(&group.0);
            for id in ids {
                command.arg(id);
            }
            command
                .query_async::<usize>(&mut connection)
                .await
                .map_redis_err()
        })
        .await
    }

    pub async fn delete(
        &self,
        stream: &RedisStreamName,
        ids: &[String],
    ) -> Result<usize, DataLayerError> {
        validate_stream_name(stream)?;
        if ids.is_empty() {
            return Ok(0);
        }

        self.run_with_timeout(RedisConnectionLane::Stream, "redis stream delete", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Stream);
            let mut command = redis::cmd("XDEL");
            command.arg(&stream.0);
            for id in ids {
                command.arg(id);
            }
            command
                .query_async::<usize>(&mut connection)
                .await
                .map_redis_err()
        })
        .await
    }

    pub async fn claim_stale(
        &self,
        stream: &RedisStreamName,
        group: &RedisConsumerGroup,
        consumer: &RedisConsumerName,
        start_id: &str,
        config: RedisStreamReclaimConfig,
    ) -> Result<RedisStreamReclaimResult, DataLayerError> {
        validate_stream_name(stream)?;
        validate_group(group)?;
        validate_consumer(consumer)?;
        validate_stream_position(start_id)?;
        config.validate()?;

        self.run_with_timeout(RedisConnectionLane::Stream, "redis stream reclaim", async {
            let mut connection = self.connections.connection(RedisConnectionLane::Stream);
            let reply = redis::cmd("XAUTOCLAIM")
                .arg(&stream.0)
                .arg(&group.0)
                .arg(&consumer.0)
                .arg(config.min_idle_ms)
                .arg(start_id)
                .arg("COUNT")
                .arg(config.count)
                .query_async::<RedisValue>(&mut connection)
                .await
                .map_redis_err()?;

            parse_reclaim_result(reply)
        })
        .await
    }

    pub async fn stats(
        &self,
        stream: &RedisStreamName,
        group: Option<&RedisConsumerGroup>,
    ) -> Result<RuntimeQueueStats, DataLayerError> {
        validate_stream_name(stream)?;
        if let Some(group) = group {
            validate_group(group)?;
        }

        let stream_length = self
            .run_with_timeout(RedisConnectionLane::Stream, "redis stream xlen", async {
                let mut connection = self.connections.connection(RedisConnectionLane::Stream);
                redis::cmd("XLEN")
                    .arg(&stream.0)
                    .query_async::<u64>(&mut connection)
                    .await
                    .map_redis_err()
            })
            .await?;
        let Some(group) = group else {
            return Ok(RuntimeQueueStats {
                stream_length,
                ..RuntimeQueueStats::default()
            });
        };

        let groups = self
            .run_with_timeout(
                RedisConnectionLane::Stream,
                "redis stream xinfo groups",
                async {
                    let mut connection = self.connections.connection(RedisConnectionLane::Stream);
                    let reply = match redis::cmd("XINFO")
                        .arg("GROUPS")
                        .arg(&stream.0)
                        .query_async::<RedisValue>(&mut connection)
                        .await
                    {
                        Ok(reply) => reply,
                        Err(err) if redis_stream_stats_missing_stream(&err) => {
                            return Ok(RedisXInfoGroupStats::default());
                        }
                        Err(err) => return Err(redis_error(err)),
                    };
                    parse_xinfo_group_stats(reply, &group.0)
                },
            )
            .await?;

        let oldest_pending_idle_ms = self
            .run_with_timeout(
                RedisConnectionLane::Stream,
                "redis stream xpending summary",
                async {
                    let mut connection = self.connections.connection(RedisConnectionLane::Stream);
                    let reply = match redis::cmd("XPENDING")
                        .arg(&stream.0)
                        .arg(&group.0)
                        .query_async::<RedisValue>(&mut connection)
                        .await
                    {
                        Ok(reply) => reply,
                        Err(err) if redis_stream_stats_missing_stream_or_group(&err) => {
                            return Ok(None);
                        }
                        Err(err) => return Err(redis_error(err)),
                    };
                    parse_xpending_oldest_idle_ms(reply)
                },
            )
            .await?;

        Ok(RuntimeQueueStats {
            stream_length,
            group_pending: groups.pending.unwrap_or_default(),
            group_lag: groups.lag,
            oldest_pending_idle_ms,
        })
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

fn redis_stream_stats_missing_stream(error: &redis::RedisError) -> bool {
    error.code() == Some("ERR") && error.to_string().contains("no such key")
}

fn redis_stream_stats_missing_stream_or_group(error: &redis::RedisError) -> bool {
    let message = error.to_string();
    redis_stream_stats_missing_stream(error)
        || error.code() == Some("NOGROUP")
        || (message.contains("NOGROUP") && message.contains("consumer group"))
}

fn validate_stream_name(stream: &RedisStreamName) -> Result<(), DataLayerError> {
    if stream.0.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "redis stream name cannot be empty".to_string(),
        ));
    }
    Ok(())
}

fn validate_group(group: &RedisConsumerGroup) -> Result<(), DataLayerError> {
    if group.0.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "redis consumer group cannot be empty".to_string(),
        ));
    }
    Ok(())
}

fn validate_consumer(consumer: &RedisConsumerName) -> Result<(), DataLayerError> {
    if consumer.0.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "redis consumer name cannot be empty".to_string(),
        ));
    }
    Ok(())
}

fn validate_stream_position(position: &str) -> Result<(), DataLayerError> {
    if position.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "redis stream position cannot be empty".to_string(),
        ));
    }
    Ok(())
}

fn parse_stream_read_entries(value: RedisValue) -> Result<Vec<RedisStreamEntry>, DataLayerError> {
    if matches!(value, RedisValue::Nil) {
        return Ok(Vec::new());
    }

    let reply = from_redis_value::<StreamReadReply>(&value).map_err(redis_error)?;
    Ok(reply
        .keys
        .into_iter()
        .flat_map(|key| key.ids.into_iter())
        .map(|id| RedisStreamEntry {
            id: id.id,
            fields: id
                .map
                .into_iter()
                .filter_map(|(field, value)| {
                    redis::from_redis_value::<String>(&value)
                        .ok()
                        .map(|text| (field, text))
                })
                .collect(),
        })
        .collect())
}

fn parse_reclaim_result(value: RedisValue) -> Result<RedisStreamReclaimResult, DataLayerError> {
    let RedisValue::Array(parts) = value else {
        return Err(DataLayerError::UnexpectedValue(
            "redis xautoclaim returned non-array payload".to_string(),
        ));
    };

    if parts.len() < 2 || parts.len() > 3 {
        return Err(DataLayerError::UnexpectedValue(format!(
            "redis xautoclaim returned {} top-level fields, expected 2 or 3",
            parts.len()
        )));
    }

    let next_start_id = parse_string_value(&parts[0], "redis xautoclaim next_start_id")?;
    let entries = parse_reclaim_entries(&parts[1])?;
    let deleted_ids = match parts.get(2) {
        Some(value) => parse_string_array(value, "redis xautoclaim deleted_ids")?,
        None => Vec::new(),
    };

    Ok(RedisStreamReclaimResult {
        next_start_id,
        entries,
        deleted_ids,
    })
}

fn parse_reclaim_entries(value: &RedisValue) -> Result<Vec<RedisStreamEntry>, DataLayerError> {
    match value {
        RedisValue::Array(entries) => entries.iter().map(parse_reclaim_entry).collect(),
        RedisValue::Nil => Ok(Vec::new()),
        _ => Err(DataLayerError::UnexpectedValue(
            "redis xautoclaim entries payload was not an array".to_string(),
        )),
    }
}

fn parse_reclaim_entry(value: &RedisValue) -> Result<RedisStreamEntry, DataLayerError> {
    let RedisValue::Array(parts) = value else {
        return Err(DataLayerError::UnexpectedValue(
            "redis xautoclaim entry was not an array".to_string(),
        ));
    };
    if parts.len() != 2 {
        return Err(DataLayerError::UnexpectedValue(format!(
            "redis xautoclaim entry had {} fields, expected 2",
            parts.len()
        )));
    }

    let id = parse_string_value(&parts[0], "redis xautoclaim entry id")?;
    let fields = parse_string_map(&parts[1], "redis xautoclaim entry fields")?;
    Ok(RedisStreamEntry { id, fields })
}

fn parse_string_map(
    value: &RedisValue,
    context: &str,
) -> Result<BTreeMap<String, String>, DataLayerError> {
    match value {
        RedisValue::Array(values) => {
            if values.len() % 2 != 0 {
                return Err(DataLayerError::UnexpectedValue(format!(
                    "{context} expected an even number of field elements, got {}",
                    values.len()
                )));
            }
            let mut fields = BTreeMap::new();
            for pair in values.chunks(2) {
                let key = parse_string_value(&pair[0], context)?;
                let value = parse_string_value(&pair[1], context)?;
                fields.insert(key, value);
            }
            Ok(fields)
        }
        RedisValue::Map(entries) => entries
            .iter()
            .map(|(key, value)| {
                Ok((
                    parse_string_value(key, context)?,
                    parse_string_value(value, context)?,
                ))
            })
            .collect(),
        RedisValue::Nil => Ok(BTreeMap::new()),
        _ => Err(DataLayerError::UnexpectedValue(format!(
            "{context} expected a redis array/map payload"
        ))),
    }
}

fn parse_string_array(value: &RedisValue, context: &str) -> Result<Vec<String>, DataLayerError> {
    match value {
        RedisValue::Array(values) => values
            .iter()
            .map(|value| parse_string_value(value, context))
            .collect(),
        RedisValue::Nil => Ok(Vec::new()),
        _ => Err(DataLayerError::UnexpectedValue(format!(
            "{context} expected a redis array payload"
        ))),
    }
}

fn parse_string_value(value: &RedisValue, context: &str) -> Result<String, DataLayerError> {
    from_redis_value::<String>(value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!(
            "{context} was not a string-compatible redis value: {err}"
        ))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct RedisXInfoGroupStats {
    pending: Option<u64>,
    lag: Option<u64>,
}

fn parse_xinfo_group_stats(
    value: RedisValue,
    group_name: &str,
) -> Result<RedisXInfoGroupStats, DataLayerError> {
    let RedisValue::Array(groups) = value else {
        return Err(DataLayerError::UnexpectedValue(
            "redis xinfo groups returned non-array payload".to_string(),
        ));
    };
    for group in groups {
        let fields = parse_info_fields(&group, "redis xinfo group")?;
        if fields
            .get("name")
            .is_some_and(|value| value.as_str() == group_name)
        {
            return Ok(RedisXInfoGroupStats {
                pending: fields
                    .get("pending")
                    .and_then(|value| value.parse::<u64>().ok()),
                lag: fields
                    .get("lag")
                    .and_then(|value| value.parse::<u64>().ok()),
            });
        }
    }
    Ok(RedisXInfoGroupStats::default())
}

fn parse_xpending_oldest_idle_ms(value: RedisValue) -> Result<Option<u64>, DataLayerError> {
    let RedisValue::Array(parts) = value else {
        return Err(DataLayerError::UnexpectedValue(
            "redis xpending summary returned non-array payload".to_string(),
        ));
    };
    let Some(total) = parts
        .first()
        .and_then(|value| parse_u64_value(value, "redis xpending pending count").ok())
    else {
        return Ok(None);
    };
    if total == 0 {
        return Ok(None);
    }
    let Some(consumers) = parts.get(3) else {
        return Ok(None);
    };
    let RedisValue::Array(consumers) = consumers else {
        return Ok(None);
    };
    let mut oldest: Option<u64> = None;
    for consumer in consumers {
        let RedisValue::Array(fields) = consumer else {
            continue;
        };
        let Some(idle_ms) = fields
            .get(2)
            .and_then(|value| parse_u64_value(value, "redis xpending consumer idle").ok())
        else {
            continue;
        };
        oldest = Some(oldest.map_or(idle_ms, |current| current.max(idle_ms)));
    }
    Ok(oldest)
}

fn parse_info_fields(
    value: &RedisValue,
    context: &str,
) -> Result<BTreeMap<String, String>, DataLayerError> {
    match value {
        RedisValue::Array(values) => {
            if values.len() % 2 != 0 {
                return Err(DataLayerError::UnexpectedValue(format!(
                    "{context} expected an even number of field elements, got {}",
                    values.len()
                )));
            }
            let mut fields = BTreeMap::new();
            for pair in values.chunks(2) {
                if matches!(pair[1], RedisValue::Nil) {
                    continue;
                }
                fields.insert(
                    parse_string_value(&pair[0], context)?,
                    parse_string_value(&pair[1], context)?,
                );
            }
            Ok(fields)
        }
        RedisValue::Map(entries) => entries
            .iter()
            .map(|(key, value)| {
                Ok((
                    parse_string_value(key, context)?,
                    parse_string_value(value, context)?,
                ))
            })
            .collect(),
        RedisValue::Nil => Ok(BTreeMap::new()),
        _ => Err(DataLayerError::UnexpectedValue(format!(
            "{context} expected a redis array/map payload"
        ))),
    }
}

fn parse_u64_value(value: &RedisValue, context: &str) -> Result<u64, DataLayerError> {
    from_redis_value::<u64>(value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!(
            "{context} was not an unsigned integer-compatible redis value: {err}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        parse_reclaim_result, parse_stream_read_entries, parse_xinfo_group_stats,
        parse_xpending_oldest_idle_ms, redis_stream_stats_missing_stream,
        redis_stream_stats_missing_stream_or_group, validate_consumer, validate_group,
        validate_stream_name, validate_stream_position, RedisConsumerName, RedisStreamName,
        RedisStreamReclaimConfig, RedisStreamReclaimResult, RedisStreamRunnerConfig,
    };
    use redis::Value as RedisValue;

    #[test]
    fn validates_stream_runner_config() {
        assert!(RedisStreamRunnerConfig {
            command_timeout_ms: Some(0),
            ..RedisStreamRunnerConfig::default()
        }
        .validate()
        .is_err());
        assert!(RedisStreamRunnerConfig {
            read_block_ms: Some(0),
            ..RedisStreamRunnerConfig::default()
        }
        .validate()
        .is_err());
        assert!(RedisStreamRunnerConfig {
            command_timeout_ms: Some(1_000),
            read_block_ms: Some(1_000),
            ..RedisStreamRunnerConfig::default()
        }
        .validate()
        .is_err());
        assert!(RedisStreamRunnerConfig {
            read_count: 0,
            ..RedisStreamRunnerConfig::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn validates_reclaim_config() {
        assert!(RedisStreamReclaimConfig {
            min_idle_ms: 0,
            ..RedisStreamReclaimConfig::default()
        }
        .validate()
        .is_err());
        assert!(RedisStreamReclaimConfig {
            count: 0,
            ..RedisStreamReclaimConfig::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn runner_reuses_client_and_keyspace() {
        RedisStreamRunnerConfig::default()
            .validate()
            .expect("default stream config should be valid");
    }

    #[test]
    fn rejects_invalid_inputs_before_network() {
        assert!(validate_stream_name(&RedisStreamName(String::new())).is_err());
        assert!(validate_group(&super::RedisConsumerGroup(String::new())).is_err());
        assert!(validate_consumer(&RedisConsumerName(String::new())).is_err());
        assert!(validate_stream_position("").is_err());
    }

    #[test]
    fn parses_empty_blocking_read_as_no_entries() {
        let parsed = parse_stream_read_entries(RedisValue::Nil)
            .expect("nil stream read reply should be empty");

        assert!(parsed.is_empty());
    }

    #[test]
    fn parses_reclaim_result_with_deleted_ids() {
        let parsed = parse_reclaim_result(RedisValue::Array(vec![
            RedisValue::BulkString(b"0-0".to_vec()),
            RedisValue::Array(vec![RedisValue::Array(vec![
                RedisValue::BulkString(b"1710000000000-0".to_vec()),
                RedisValue::Array(vec![
                    RedisValue::BulkString(b"payload".to_vec()),
                    RedisValue::BulkString(br#"{"ok":true}"#.to_vec()),
                    RedisValue::BulkString(b"kind".to_vec()),
                    RedisValue::BulkString(b"shadow".to_vec()),
                ]),
            ])]),
            RedisValue::Array(vec![RedisValue::BulkString(b"1709999999999-0".to_vec())]),
        ]))
        .expect("reclaim result should parse");

        assert_eq!(
            parsed,
            RedisStreamReclaimResult {
                next_start_id: "0-0".to_string(),
                entries: vec![super::RedisStreamEntry {
                    id: "1710000000000-0".to_string(),
                    fields: BTreeMap::from([
                        ("kind".to_string(), "shadow".to_string()),
                        ("payload".to_string(), r#"{"ok":true}"#.to_string()),
                    ]),
                }],
                deleted_ids: vec!["1709999999999-0".to_string()],
            }
        );
    }

    #[test]
    fn parses_reclaim_result_without_deleted_ids() {
        let parsed = parse_reclaim_result(RedisValue::Array(vec![
            RedisValue::BulkString(b"0-0".to_vec()),
            RedisValue::Array(vec![]),
        ]))
        .expect("reclaim result should parse");

        assert_eq!(
            parsed,
            RedisStreamReclaimResult {
                next_start_id: "0-0".to_string(),
                entries: Vec::new(),
                deleted_ids: Vec::new(),
            }
        );
    }

    #[test]
    fn parses_xinfo_group_stats() {
        let parsed = parse_xinfo_group_stats(
            RedisValue::Array(vec![RedisValue::Array(vec![
                RedisValue::BulkString(b"name".to_vec()),
                RedisValue::BulkString(b"usage_consumers".to_vec()),
                RedisValue::BulkString(b"consumers".to_vec()),
                RedisValue::Int(4),
                RedisValue::BulkString(b"pending".to_vec()),
                RedisValue::Int(7),
                RedisValue::BulkString(b"last-delivered-id".to_vec()),
                RedisValue::BulkString(b"1710000000000-0".to_vec()),
                RedisValue::BulkString(b"entries-read".to_vec()),
                RedisValue::Int(12),
                RedisValue::BulkString(b"lag".to_vec()),
                RedisValue::Int(3),
            ])]),
            "usage_consumers",
        )
        .expect("xinfo groups should parse");

        assert_eq!(parsed.pending, Some(7));
        assert_eq!(parsed.lag, Some(3));
    }

    #[test]
    fn parses_xinfo_group_stats_with_nil_lag() {
        let parsed = parse_xinfo_group_stats(
            RedisValue::Array(vec![RedisValue::Array(vec![
                RedisValue::BulkString(b"name".to_vec()),
                RedisValue::BulkString(b"usage_consumers".to_vec()),
                RedisValue::BulkString(b"consumers".to_vec()),
                RedisValue::Int(4),
                RedisValue::BulkString(b"pending".to_vec()),
                RedisValue::Int(7),
                RedisValue::BulkString(b"last-delivered-id".to_vec()),
                RedisValue::BulkString(b"1710000000000-0".to_vec()),
                RedisValue::BulkString(b"entries-read".to_vec()),
                RedisValue::Int(12),
                RedisValue::BulkString(b"lag".to_vec()),
                RedisValue::Nil,
            ])]),
            "usage_consumers",
        )
        .expect("xinfo groups should parse nil lag");

        assert_eq!(parsed.pending, Some(7));
        assert_eq!(parsed.lag, None);
    }

    #[test]
    fn parses_xpending_oldest_idle_ms_from_summary() {
        let parsed = parse_xpending_oldest_idle_ms(RedisValue::Array(vec![
            RedisValue::Int(7),
            RedisValue::BulkString(b"1710000000000-0".to_vec()),
            RedisValue::BulkString(b"1710000000006-0".to_vec()),
            RedisValue::Array(vec![
                RedisValue::Array(vec![
                    RedisValue::BulkString(b"consumer-a".to_vec()),
                    RedisValue::Int(4),
                    RedisValue::Int(1200),
                ]),
                RedisValue::Array(vec![
                    RedisValue::BulkString(b"consumer-b".to_vec()),
                    RedisValue::Int(3),
                    RedisValue::Int(3400),
                ]),
            ]),
        ]))
        .expect("xpending should parse");

        assert_eq!(parsed, Some(3400));
    }

    #[test]
    fn parses_empty_xpending_as_none() {
        let parsed = parse_xpending_oldest_idle_ms(RedisValue::Array(vec![
            RedisValue::Int(0),
            RedisValue::Nil,
            RedisValue::Nil,
            RedisValue::Array(vec![]),
        ]))
        .expect("empty xpending should parse");

        assert_eq!(parsed, None);
    }

    #[test]
    fn classifies_missing_stream_stats_errors() {
        let missing_stream = redis::RedisError::from((
            redis::ErrorKind::ResponseError,
            "ERR",
            "no such key".to_string(),
        ));
        let missing_group = redis::RedisError::from((
            redis::ErrorKind::ResponseError,
            "ERR",
            "NOGROUP No such key 'usage:events' or consumer group 'usage_consumers'".to_string(),
        ));
        let other_error = redis::RedisError::from((
            redis::ErrorKind::ResponseError,
            "ERR",
            "wrong type".to_string(),
        ));

        assert!(redis_stream_stats_missing_stream(&missing_stream));
        assert!(redis_stream_stats_missing_stream_or_group(&missing_stream));
        assert!(!redis_stream_stats_missing_stream(&missing_group));
        assert!(redis_stream_stats_missing_stream_or_group(&missing_group));
        assert!(!redis_stream_stats_missing_stream(&other_error));
        assert!(!redis_stream_stats_missing_stream_or_group(&other_error));
    }
}
