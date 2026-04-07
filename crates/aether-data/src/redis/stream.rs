use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use redis::from_redis_value;
use redis::streams::StreamReadReply;
use redis::Value as RedisValue;

use crate::error::{redis_error, RedisResultExt};
use crate::redis::{RedisClient, RedisKeyspace};
use crate::DataLayerError;

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
    client: RedisClient,
    keyspace: RedisKeyspace,
    config: RedisStreamRunnerConfig,
}

impl RedisStreamRunner {
    pub fn new(
        client: RedisClient,
        keyspace: RedisKeyspace,
        config: RedisStreamRunnerConfig,
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

    pub fn config(&self) -> RedisStreamRunnerConfig {
        self.config
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

        self.run_with_timeout("redis stream ensure consumer group", async {
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_redis_err()?;
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
        })
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

        self.run_with_timeout("redis stream append", async {
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_redis_err()?;
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

        self.run_with_timeout("redis stream read group", async {
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_redis_err()?;
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
                .query_async::<StreamReadReply>(&mut connection)
                .await
                .map_redis_err()?;

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

        self.run_with_timeout("redis stream ack", async {
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_redis_err()?;
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

        self.run_with_timeout("redis stream delete", async {
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_redis_err()?;
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

        self.run_with_timeout("redis stream reclaim", async {
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_redis_err()?;
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        parse_reclaim_result, RedisConsumerGroup, RedisConsumerName, RedisStreamName,
        RedisStreamReclaimConfig, RedisStreamReclaimResult, RedisStreamRunner,
        RedisStreamRunnerConfig,
    };
    use crate::redis::{RedisClientConfig, RedisClientFactory};
    use redis::Value as RedisValue;

    fn sample_runner() -> RedisStreamRunner {
        let config = RedisClientConfig {
            url: "redis://127.0.0.1/0".to_string(),
            key_prefix: Some("aether".to_string()),
        };
        let client = RedisClientFactory::new(config.clone())
            .expect("factory should build")
            .connect_lazy()
            .expect("client should build");

        RedisStreamRunner::new(
            client,
            config.keyspace(),
            RedisStreamRunnerConfig::default(),
        )
        .expect("runner should build")
    }

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
        let runner = sample_runner();

        assert_eq!(runner.config(), RedisStreamRunnerConfig::default());
        assert_eq!(
            runner.keyspace().stream_name("audit").0,
            "aether:stream:audit"
        );
        let _client_ref = runner.client();
    }

    #[tokio::test]
    async fn rejects_invalid_inputs_before_network() {
        let runner = sample_runner();
        let stream = RedisStreamName("aether:stream:audit".to_string());
        let group = RedisConsumerGroup("audit-workers".to_string());
        let consumer = RedisConsumerName("worker-1".to_string());

        assert!(runner
            .ensure_consumer_group(&stream, &group, "")
            .await
            .is_err());
        assert!(runner
            .append_fields(&stream, &BTreeMap::new())
            .await
            .is_err());
        assert!(runner
            .append_json(&stream, "", &serde_json::json!({"ok": true}))
            .await
            .is_err());
        assert!(runner
            .read_group(&stream, &group, &RedisConsumerName(String::new()))
            .await
            .is_err());
        assert_eq!(
            runner.ack(&stream, &group, &[]).await.expect("empty ack"),
            0
        );
        assert_eq!(runner.delete(&stream, &[]).await.expect("empty delete"), 0);
        assert!(runner
            .claim_stale(
                &stream,
                &group,
                &consumer,
                "",
                RedisStreamReclaimConfig::default()
            )
            .await
            .is_err());
        let _ = consumer;
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
}
