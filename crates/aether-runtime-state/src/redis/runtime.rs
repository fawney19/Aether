use std::time::Duration;

use crate::error::RedisResultExt;
use crate::redis::{
    cmd, run_lane_with_timeout, script, RedisCmd, RedisConnectionLane, RedisConnectionRouter,
    RedisKeyspace, RedisLaneDiagnostics,
};
use crate::{
    DataLayerError, RateLimitCheck, RateLimitInput, RateLimitScope, RuntimeSemaphoreError,
};

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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RedisRuntimeDiagnostics {
    pub connected_clients: Option<u64>,
    pub total_connections_received: Option<u64>,
    pub lanes: Vec<RedisLaneDiagnostics>,
}

#[derive(Debug, Clone)]
pub(crate) struct RedisRuntimeRunner {
    connections: RedisConnectionRouter,
    keyspace: RedisKeyspace,
    command_timeout_ms: Option<u64>,
}

impl RedisRuntimeRunner {
    pub(crate) fn new(
        connections: RedisConnectionRouter,
        keyspace: RedisKeyspace,
        command_timeout_ms: Option<u64>,
    ) -> Self {
        Self {
            connections,
            keyspace,
            command_timeout_ms,
        }
    }

    pub(crate) async fn ping(&self) -> Result<(), DataLayerError> {
        let pong = self
            .query_string(RedisConnectionLane::Fast, "runtime redis ping", cmd("PING"))
            .await?;
        if pong.eq_ignore_ascii_case("PONG") {
            Ok(())
        } else {
            Err(DataLayerError::UnexpectedValue(format!(
                "unexpected runtime redis ping response {pong}"
            )))
        }
    }

    pub(crate) async fn diagnostics(&self) -> Result<RedisRuntimeDiagnostics, DataLayerError> {
        let info = self
            .query_string(
                RedisConnectionLane::Admin,
                "runtime redis diagnostics",
                cmd("INFO"),
            )
            .await?;
        Ok(parse_diagnostics(
            &info,
            self.connections.lane_diagnostics(),
        ))
    }

    pub(crate) async fn kv_set_plain(
        &self,
        key: &str,
        value: String,
    ) -> Result<(), DataLayerError> {
        let namespaced_key = self.keyspace.key(key);
        let mut command = cmd("SET");
        command.arg(namespaced_key).arg(value);
        self.query_string(RedisConnectionLane::Fast, "runtime kv set", command)
            .await?;
        Ok(())
    }

    pub(crate) async fn kv_set_with_ttl(
        &self,
        key: &str,
        value: String,
        ttl: Duration,
    ) -> Result<(), DataLayerError> {
        let namespaced_key = self.keyspace.key(key);
        let mut command = cmd("PSETEX");
        command
            .arg(namespaced_key)
            .arg(u64::try_from(ttl.as_millis().max(1)).unwrap_or(u64::MAX))
            .arg(value);
        self.query_string(RedisConnectionLane::Fast, "runtime kv set ttl", command)
            .await?;
        Ok(())
    }

    pub(crate) async fn kv_get_many(
        &self,
        keys: &[String],
    ) -> Result<Vec<Option<String>>, DataLayerError> {
        let namespaced = keys
            .iter()
            .map(|key| self.keyspace.key(key))
            .collect::<Vec<_>>();
        let mut command = cmd("MGET");
        command.arg(&namespaced);
        self.query(RedisConnectionLane::Fast, "runtime kv mget", command)
            .await
    }

    pub(crate) async fn kv_delete_many(&self, keys: &[String]) -> Result<usize, DataLayerError> {
        let prefix = self.keyspace.key("");
        let namespaced = keys
            .iter()
            .map(|key| {
                if key_belongs_to_prefix(key, &prefix) {
                    key.clone()
                } else {
                    self.keyspace.key(key)
                }
            })
            .collect::<Vec<_>>();
        let mut command = cmd("DEL");
        command.arg(&namespaced);
        let deleted = self
            .query_i64(
                RedisConnectionLane::Admin,
                "runtime kv delete many",
                command,
            )
            .await?;
        Ok(usize::try_from(deleted).unwrap_or(0))
    }

    pub(crate) async fn kv_ttl_seconds(&self, key: &str) -> Result<Option<i64>, DataLayerError> {
        let namespaced_key = self.keyspace.key(key);
        let mut command = cmd("TTL");
        command.arg(&namespaced_key);
        let ttl = self
            .query_i64(RedisConnectionLane::Fast, "runtime kv ttl", command)
            .await?;
        Ok((ttl >= -1).then_some(ttl))
    }

    pub(crate) async fn scan_keys(
        &self,
        pattern: &str,
        count: usize,
    ) -> Result<Vec<String>, DataLayerError> {
        let pattern = self.keyspace.key(pattern);
        run_lane_with_timeout(
            &self.connections,
            RedisConnectionLane::Admin,
            self.command_timeout_ms,
            "runtime scan keys",
            async {
                let mut connection = self.connections.connection(RedisConnectionLane::Admin);
                let mut cursor = 0u64;
                let mut keys = Vec::new();
                loop {
                    let (next_cursor, mut batch) = cmd("SCAN")
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
            },
        )
        .await
    }

    pub(crate) async fn check_and_consume_rate_limit(
        &self,
        input: RateLimitInput<'_>,
    ) -> Result<RateLimitCheck, DataLayerError> {
        let user_key = self.keyspace.key(input.user_key);
        let key_key = self.keyspace.key(input.key_key);
        let raw = run_lane_with_timeout(
            &self.connections,
            RedisConnectionLane::Fast,
            self.command_timeout_ms,
            "runtime rate limit check",
            async {
                let mut connection = self.connections.connection(RedisConnectionLane::Fast);
                script(RATE_LIMIT_CHECK_AND_CONSUME_SCRIPT)
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

    pub(crate) async fn set_add(&self, key: &str, member: &str) -> Result<bool, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("SADD");
        command.arg(&key).arg(member);
        Ok(self
            .query_i64(RedisConnectionLane::Fast, "runtime set add", command)
            .await?
            > 0)
    }

    pub(crate) async fn set_remove(&self, key: &str, member: &str) -> Result<bool, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("SREM");
        command.arg(&key).arg(member);
        Ok(self
            .query_i64(RedisConnectionLane::Fast, "runtime set remove", command)
            .await?
            > 0)
    }

    pub(crate) async fn set_members(&self, key: &str) -> Result<Vec<String>, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("SMEMBERS");
        command.arg(&key);
        let mut values = self
            .query::<Vec<String>>(RedisConnectionLane::Admin, "runtime set members", command)
            .await?;
        values.sort();
        Ok(values)
    }

    pub(crate) async fn set_len(&self, key: &str) -> Result<usize, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("SCARD");
        command.arg(&key);
        let len = self
            .query_i64(RedisConnectionLane::Fast, "runtime set len", command)
            .await?;
        Ok(usize::try_from(len).unwrap_or(0))
    }

    pub(crate) async fn score_set(
        &self,
        key: &str,
        member: &str,
        score: f64,
    ) -> Result<(), DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("ZADD");
        command.arg(&key).arg(score).arg(member);
        self.query_i64(RedisConnectionLane::Fast, "runtime score set", command)
            .await?;
        Ok(())
    }

    pub(crate) async fn score_many(
        &self,
        key: &str,
        members: &[String],
    ) -> Result<Vec<Option<f64>>, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("ZMSCORE");
        command.arg(&key);
        for member in members {
            command.arg(member);
        }
        self.query(RedisConnectionLane::Fast, "runtime score many", command)
            .await
    }

    pub(crate) async fn score_range_by_min(
        &self,
        key: &str,
        min_score: f64,
    ) -> Result<Vec<String>, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("ZRANGEBYSCORE");
        command.arg(&key).arg(min_score).arg("+inf");
        self.query(RedisConnectionLane::Admin, "runtime score range", command)
            .await
    }

    pub(crate) async fn score_remove_by_score(
        &self,
        key: &str,
        max_score: f64,
    ) -> Result<usize, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("ZREMRANGEBYSCORE");
        command.arg(&key).arg("-inf").arg(max_score);
        let removed = self
            .query_i64(RedisConnectionLane::Admin, "runtime score trim", command)
            .await?;
        Ok(usize::try_from(removed).unwrap_or(0))
    }

    pub(crate) async fn score_remove(
        &self,
        key: &str,
        member: &str,
    ) -> Result<bool, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("ZREM");
        command.arg(&key).arg(member);
        Ok(self
            .query_i64(RedisConnectionLane::Fast, "runtime score remove", command)
            .await?
            > 0)
    }

    pub(crate) async fn score_remove_by_rank(
        &self,
        key: &str,
        start: i64,
        stop: i64,
    ) -> Result<usize, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("ZREMRANGEBYRANK");
        command.arg(&key).arg(start).arg(stop);
        let removed = self
            .query_i64(
                RedisConnectionLane::Admin,
                "runtime score rank trim",
                command,
            )
            .await?;
        Ok(usize::try_from(removed).unwrap_or(0))
    }

    pub(crate) async fn score_len(&self, key: &str) -> Result<usize, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("ZCARD");
        command.arg(&key);
        let len = self
            .query_i64(RedisConnectionLane::Fast, "runtime score len", command)
            .await?;
        Ok(usize::try_from(len).unwrap_or(0))
    }

    pub(crate) async fn key_expire(
        &self,
        key: &str,
        ttl: Duration,
    ) -> Result<bool, DataLayerError> {
        let key = self.keyspace.key(key);
        let mut command = cmd("PEXPIRE");
        command
            .arg(&key)
            .arg(u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX));
        Ok(self
            .query_i64(RedisConnectionLane::Fast, "runtime key expire", command)
            .await?
            > 0)
    }

    pub(crate) async fn semaphore_try_acquire(
        &self,
        gate: &'static str,
        limit: usize,
        key: &str,
        token: &str,
        lease_ttl_ms: u64,
        timeout_ms: Option<u64>,
    ) -> Result<(i64, i64), RuntimeSemaphoreError> {
        let now_ms = crate::unix_time_ms();
        let expires_at_ms = now_ms.saturating_add(lease_ttl_ms);
        let key = self.keyspace.key(key);
        let timeout_ms = timeout_ms.or(self.command_timeout_ms);
        run_lane_with_timeout(
            &self.connections,
            RedisConnectionLane::Fast,
            timeout_ms,
            "runtime semaphore acquire",
            async {
                let mut connection = self.connections.connection(RedisConnectionLane::Fast);
                script(
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
                .arg(limit as i64)
                .arg(token)
                .arg(lease_ttl_ms as i64)
                .invoke_async::<(i64, i64)>(&mut connection)
                .await
                .map_redis_err()
            },
        )
        .await
        .map_err(|err| RuntimeSemaphoreError::Unavailable {
            gate,
            limit,
            message: format!("acquire failed: {err}"),
        })
    }

    pub(crate) async fn semaphore_renew(
        &self,
        gate: &'static str,
        limit: usize,
        key: &str,
        token: &str,
        lease_ttl_ms: u64,
        timeout_ms: Option<u64>,
    ) -> Result<i64, RuntimeSemaphoreError> {
        let now_ms = crate::unix_time_ms();
        let expires_at_ms = now_ms.saturating_add(lease_ttl_ms);
        let key = self.keyspace.key(key);
        let timeout_ms = timeout_ms.or(self.command_timeout_ms);
        run_lane_with_timeout(
            &self.connections,
            RedisConnectionLane::Fast,
            timeout_ms,
            "runtime semaphore renew",
            async {
                let mut connection = self.connections.connection(RedisConnectionLane::Fast);
                script(
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
                .arg(lease_ttl_ms as i64)
                .invoke_async::<i64>(&mut connection)
                .await
                .map_redis_err()
            },
        )
        .await
        .map_err(|err| RuntimeSemaphoreError::Unavailable {
            gate,
            limit,
            message: format!("renew failed: {err}"),
        })
    }

    pub(crate) async fn semaphore_release(
        &self,
        gate: &'static str,
        limit: usize,
        key: &str,
        token: &str,
        timeout_ms: Option<u64>,
    ) -> Result<(), RuntimeSemaphoreError> {
        let key = self.keyspace.key(key);
        let timeout_ms = timeout_ms.or(self.command_timeout_ms);
        run_lane_with_timeout(
            &self.connections,
            RedisConnectionLane::Fast,
            timeout_ms,
            "runtime semaphore release",
            async {
                let mut connection = self.connections.connection(RedisConnectionLane::Fast);
                script(
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
                .map(|_| ())
                .map_redis_err()
            },
        )
        .await
        .map_err(|err| RuntimeSemaphoreError::Unavailable {
            gate,
            limit,
            message: format!("release failed: {err}"),
        })
    }

    pub(crate) async fn semaphore_live_count(
        &self,
        gate: &'static str,
        limit: usize,
        key: &str,
        timeout_ms: Option<u64>,
    ) -> Result<usize, RuntimeSemaphoreError> {
        let now_ms = crate::unix_time_ms();
        let key = self.keyspace.key(key);
        let timeout_ms = timeout_ms.or(self.command_timeout_ms);
        run_lane_with_timeout(
            &self.connections,
            RedisConnectionLane::Fast,
            timeout_ms,
            "runtime semaphore snapshot",
            async {
                let mut connection = self.connections.connection(RedisConnectionLane::Fast);
                script(
                    "redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', ARGV[1]); \
                 return redis.call('ZCARD', KEYS[1]);",
                )
                .key(&key)
                .arg(now_ms as i64)
                .invoke_async::<i64>(&mut connection)
                .await
                .map(|value| value.max(0) as usize)
                .map_redis_err()
            },
        )
        .await
        .map_err(|err| RuntimeSemaphoreError::Unavailable {
            gate,
            limit,
            message: format!("snapshot failed: {err}"),
        })
    }

    async fn query<T>(
        &self,
        lane: RedisConnectionLane,
        operation: &'static str,
        command: RedisCmd,
    ) -> Result<T, DataLayerError>
    where
        T: redis::FromRedisValue,
    {
        run_lane_with_timeout(
            &self.connections,
            lane,
            self.command_timeout_ms,
            operation,
            async {
                let mut connection = self.connections.connection(lane);
                command
                    .query_async::<T>(&mut connection)
                    .await
                    .map_redis_err()
            },
        )
        .await
    }

    async fn query_i64(
        &self,
        lane: RedisConnectionLane,
        operation: &'static str,
        command: RedisCmd,
    ) -> Result<i64, DataLayerError> {
        self.query(lane, operation, command).await
    }

    async fn query_string(
        &self,
        lane: RedisConnectionLane,
        operation: &'static str,
        command: RedisCmd,
    ) -> Result<String, DataLayerError> {
        self.query(lane, operation, command).await
    }
}

fn parse_diagnostics(info: &str, lanes: Vec<RedisLaneDiagnostics>) -> RedisRuntimeDiagnostics {
    RedisRuntimeDiagnostics {
        connected_clients: parse_info_u64(info, "connected_clients"),
        total_connections_received: parse_info_u64(info, "total_connections_received"),
        lanes,
    }
}

fn parse_info_u64(info: &str, key: &str) -> Option<u64> {
    info.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name == key)
            .then(|| value.trim().parse::<u64>().ok())
            .flatten()
    })
}

fn key_belongs_to_prefix(key: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || key == prefix
        || key
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(':'))
}

#[cfg(test)]
mod tests {
    use super::{key_belongs_to_prefix, parse_diagnostics, RedisRuntimeDiagnostics};

    #[test]
    fn parses_runtime_diagnostics_from_info() {
        let parsed = parse_diagnostics(
            "# Clients\r\nconnected_clients:5\r\n# Stats\r\ntotal_connections_received:42\r\n",
            Vec::new(),
        );

        assert_eq!(
            parsed,
            RedisRuntimeDiagnostics {
                connected_clients: Some(5),
                total_connections_received: Some(42),
                lanes: Vec::new(),
            }
        );
    }

    #[test]
    fn detects_namespaced_key_prefix_on_boundary() {
        assert!(key_belongs_to_prefix("aether:cache:item", "aether"));
        assert!(key_belongs_to_prefix("aether", "aether"));
        assert!(key_belongs_to_prefix("raw:key", ""));
        assert!(!key_belongs_to_prefix("aetherish:cache:item", "aether"));
    }
}
