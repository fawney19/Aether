use std::time::Duration;

use crate::error::RedisResultExt;
use crate::redis::{
    cmd, run_lane_with_timeout, script, RedisCmd, RedisConnectionLane, RedisConnectionRouter,
    RedisKeyspace, RedisLaneDiagnostics,
};
use crate::{
    DataLayerError, RateLimitCheck, RateLimitInput, RateLimitScope, RuntimeSemaphoreError,
    ScoreWindowU64Stats, UsageLimitCheck, UsageLimitInput, UsageLimitReleaseInput,
    SCORE_WINDOW_AGGREGATION_MEMBER_LIMIT,
};

const SCORE_WINDOW_STATS_SCRIPT: &str = include_str!("score_window.lua");
const SCORE_WINDOW_STATS_PIPELINE_KEY_LIMIT: usize = 16;

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

pub(super) const USAGE_LIMIT_CHECK_AND_CONSUME_SCRIPT: &str = r#"
local count = #KEYS
local now = tonumber(ARGV[1])
local event_id = ARGV[2]

-- Large mixed windows are copied in bounded commands on an exclusive WATCH
-- connection. This read-only pass must finish before pruning any of the rules.
if ARGV[count * 3 + 3] ~= 'inline' and redis.acl_check_cmd then
    local deferred = {2}
    for i = 1, count do
        local key = KEYS[i]
        if redis.call('ZCARD', key) > 4096 then
            local cutoff = now - tonumber(ARGV[(i - 1) * 3 + 4]) * 1000
            local expired = redis.pcall('ZCOUNT', key, '-inf', cutoff)
            if type(expired) == 'number' and expired > 4096 then
                local live = redis.call('ZCARD', key) - expired
                local temporary = key .. ':__usage_copy:acl'
                if live > 256
                    and redis.acl_check_cmd('WATCH', key)
                    and redis.acl_check_cmd('UNWATCH')
                    and redis.acl_check_cmd('MULTI')
                    and redis.acl_check_cmd('EXEC')
                    and redis.acl_check_cmd('EVAL', 'return 1', 0)
                    and redis.acl_check_cmd('EXISTS', temporary)
                    and redis.acl_check_cmd('ZRANGE', key, 0, 511, 'WITHSCORES')
                    and redis.acl_check_cmd('ZADD', temporary, 0, 'acl')
                    and redis.acl_check_cmd('PTTL', key)
                    and redis.acl_check_cmd('PEXPIRE', temporary, 60000)
                    and redis.acl_check_cmd('PERSIST', temporary)
                    and redis.acl_check_cmd('UNLINK', key, temporary)
                    and redis.acl_check_cmd('RENAME', temporary, key) then
                    deferred[#deferred + 1] = i
                    deferred[#deferred + 1] = expired
                    deferred[#deferred + 1] = live
                end
            end
        end
    end
    if #deferred > 1 then return deferred end
end

local function replace_with_survivors(key, live)
    if not redis.acl_check_cmd then return false end
    local temporary = key .. ':__usage_trim'
    local exists = redis.pcall('EXISTS', temporary)
    local ttl = redis.pcall('PTTL', key)
    if exists ~= 0 or type(ttl) ~= 'number' or ttl == 0 or ttl < -1 then
        return false
    end
    local rows = redis.call('ZRANGE', key, -live, -1, 'WITHSCORES')
    local args = {}
    for i = 1, #rows, 2 do
        args[#args + 1] = rows[i + 1]
        args[#args + 1] = rows[i]
    end
    -- Validate every write before detaching the original. The temporary key
    -- is fully built first; restricted ACLs keep the original cleanup path.
    if not redis.acl_check_cmd('ZADD', temporary, unpack(args))
        or not redis.acl_check_cmd('UNLINK', key)
        or not redis.acl_check_cmd('UNLINK', temporary)
        or not redis.acl_check_cmd('RENAME', temporary, key)
        or (ttl > 0 and not redis.acl_check_cmd('PEXPIRE', temporary, ttl)) then
        return false
    end
    if type(redis.pcall('ZADD', temporary, unpack(args))) ~= 'number' then
        return false
    end
    if ttl > 0 and redis.pcall('PEXPIRE', temporary, ttl) ~= 1 then
        redis.call('UNLINK', temporary)
        return false
    end
    if type(redis.pcall('UNLINK', key)) ~= 'number' then
        redis.call('UNLINK', temporary)
        return false
    end
    redis.call('RENAME', temporary, key)
    return true
end

local function prune_window(key, cutoff)
    local cardinality = redis.call('ZCARD', key)
    if cardinality == 0 then return 0 end
    if cardinality > 256 then
        local earliest = redis.call('ZRANGE', key, 0, 0, 'WITHSCORES')
        if #earliest >= 2 and tonumber(earliest[2]) > cutoff then
            return cardinality
        end
        local latest = redis.call('ZRANGE', key, -1, -1, 'WITHSCORES')
        if #latest >= 2 and tonumber(latest[2]) <= cutoff then
            -- The entire window is expired. Redis can free the detached object
            -- off its command thread while this key is reused.
            local result = redis.pcall('UNLINK', key)
            if type(result) == 'number' then return 0 end
        elseif cardinality > 1024 then
            local expired = redis.pcall('ZCOUNT', key, '-inf', cutoff)
            if type(expired) == 'number' then
                local live = cardinality - expired
                if live > 0 and live <= 256 and expired > live * 4
                    and replace_with_survivors(key, live) then
                    return live
                end
            end
        end
    end
    -- Preserve the existing path for large live windows and restricted ACLs.
    -- Partial deferred deletion could resurrect entries on out-of-order calls.
    return cardinality - redis.call('ZREMRANGEBYSCORE', key, '-inf', cutoff)
end

local current_counts = {}
for i = 1, count do
    local window_ms = tonumber(ARGV[(i - 1) * 3 + 4]) * 1000
    current_counts[i] = prune_window(KEYS[i], now - window_ms)
end

for i = 1, count do
    local limit = tonumber(ARGV[(i - 1) * 3 + 3])
    local window_ms = tonumber(ARGV[(i - 1) * 3 + 4]) * 1000
    local already_consumed = redis.call('ZSCORE', KEYS[i], event_id)
    if not already_consumed then
        local current = current_counts[i]
        if current >= limit then
            local earliest = redis.call('ZRANGE', KEYS[i], 0, 0, 'WITHSCORES')
            local retry_after = 1
            if #earliest >= 2 then
                local retry_after_ms = math.max(1, tonumber(earliest[2]) + window_ms - now)
                retry_after = math.ceil(retry_after_ms / 1000)
            end
            return {0, i, limit, retry_after}
        end
    end
end

for i = 1, count do
    local retention = tonumber(ARGV[(i - 1) * 3 + 5])
    redis.call('ZADD', KEYS[i], 'NX', now, event_id)
    redis.call('EXPIRE', KEYS[i], retention + 1)
end
return {1, 0, 0, 0}
"#;

const USAGE_LIMIT_RELEASE_SCRIPT: &str = r#"
for i = 1, #KEYS do
    redis.call('ZREM', KEYS[i], ARGV[1])
end
return 1
"#;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RedisRuntimeDiagnostics {
    pub connected_clients: Option<u64>,
    pub blocked_clients: Option<u64>,
    pub total_connections_received: Option<u64>,
    pub rejected_connections: Option<u64>,
    pub total_commands_processed: Option<u64>,
    pub instantaneous_ops_per_sec: Option<u64>,
    pub total_error_replies: Option<u64>,
    pub expired_keys: Option<u64>,
    pub evicted_keys: Option<u64>,
    pub keyspace_hits: Option<u64>,
    pub keyspace_misses: Option<u64>,
    pub used_memory_bytes: Option<u64>,
    pub maxmemory_bytes: Option<u64>,
    pub memory_fragmentation_ratio_basis_points: Option<u64>,
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

    pub(crate) async fn kv_set_if_absent(
        &self,
        key: &str,
        value: String,
        ttl: Duration,
    ) -> Result<bool, DataLayerError> {
        let namespaced_key = self.keyspace.key(key);
        let ttl_ms = u64::try_from(ttl.as_millis().max(1)).unwrap_or(u64::MAX);
        let mut command = cmd("SET");
        command
            .arg(namespaced_key)
            .arg(value)
            .arg("NX")
            .arg("PX")
            .arg(ttl_ms);
        let response = self
            .query::<Option<String>>(
                RedisConnectionLane::Fast,
                "runtime kv set if absent",
                command,
            )
            .await?;
        match response.as_deref() {
            Some("OK") => Ok(true),
            None => Ok(false),
            Some(value) => Err(DataLayerError::UnexpectedValue(format!(
                "unexpected runtime kv set if absent response {value}"
            ))),
        }
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

    pub(crate) async fn check_and_consume_usage_limits(
        &self,
        input: UsageLimitInput<'_>,
    ) -> Result<UsageLimitCheck, DataLayerError> {
        let keys = input
            .rules
            .iter()
            .map(|rule| self.keyspace.key(rule.key))
            .collect::<Vec<_>>();
        let raw = run_lane_with_timeout(
            &self.connections,
            RedisConnectionLane::Fast,
            Some(self.command_timeout_ms.unwrap_or(30_000)),
            "runtime usage limit check",
            super::usage_cleanup::check_and_consume(&self.connections, &keys, &input),
        )
        .await?;
        match raw.first().copied() {
            Some(1) if raw.len() >= 4 => return Ok(UsageLimitCheck::Allowed),
            Some(0) if raw.len() >= 4 => {}
            _ => {
                return Err(DataLayerError::UnexpectedValue(
                    "runtime usage limit script returned an invalid response".to_string(),
                ));
            }
        }
        let rule_index = raw
            .get(1)
            .copied()
            .and_then(|value| usize::try_from(value.saturating_sub(1)).ok())
            .filter(|index| *index < input.rules.len())
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(
                    "runtime usage limit script returned an invalid rule index".to_string(),
                )
            })?;
        let limit = raw
            .get(2)
            .copied()
            .and_then(|value| u64::try_from(value).ok())
            .filter(|limit| *limit > 0)
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue(
                    "runtime usage limit script returned an invalid limit".to_string(),
                )
            })?;
        let retry_after = raw
            .get(3)
            .copied()
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(1)
            .max(1);
        Ok(UsageLimitCheck::Rejected {
            rule_index,
            limit,
            retry_after,
        })
    }

    pub(crate) async fn release_usage_limits(
        &self,
        input: UsageLimitReleaseInput<'_>,
    ) -> Result<(), DataLayerError> {
        let script = script(USAGE_LIMIT_RELEASE_SCRIPT);
        let mut invocation = script.prepare_invoke();
        for rule in input.rules {
            invocation.key(self.keyspace.key(rule.key));
        }
        invocation.arg(input.event_id);
        run_lane_with_timeout(
            &self.connections,
            RedisConnectionLane::Fast,
            self.command_timeout_ms,
            "runtime usage limit release",
            async {
                let mut connection = self.connections.connection(RedisConnectionLane::Fast);
                invocation
                    .invoke_async::<i64>(&mut connection)
                    .await
                    .map_redis_err()
            },
        )
        .await?;
        Ok(())
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

    pub(crate) async fn score_window_u64_stats_by_min(
        &self,
        keys: &[String],
        min_score: f64,
    ) -> Result<Vec<Option<ScoreWindowU64Stats>>, DataLayerError> {
        let script = script(SCORE_WINDOW_STATS_SCRIPT);
        let mut output = Vec::with_capacity(keys.len());
        for batch in keys.chunks(SCORE_WINDOW_STATS_PIPELINE_KEY_LIMIT) {
            let values: Vec<(u8, String, u64)> = run_lane_with_timeout(
                &self.connections,
                RedisConnectionLane::Admin,
                self.command_timeout_ms,
                "runtime score window stats",
                async {
                    let mut connection = self.connections.connection(RedisConnectionLane::Admin);
                    let mut pipeline = redis::pipe();
                    for key in batch {
                        pipeline
                            .cmd("EVALSHA")
                            .arg(script.get_hash())
                            .arg(1)
                            .arg(self.keyspace.key(key))
                            .arg(min_score)
                            .arg(SCORE_WINDOW_AGGREGATION_MEMBER_LIMIT);
                    }
                    match pipeline.query_async(&mut connection).await {
                        Err(err) if err.kind() == redis::ErrorKind::NoScriptError => {
                            script
                                .prepare_invoke()
                                .load_async(&mut connection)
                                .await
                                .map_redis_err()?;
                            pipeline.query_async(&mut connection).await.map_redis_err()
                        }
                        result => result.map_redis_err(),
                    }
                },
            )
            .await?;
            if values.len() != batch.len() {
                return Err(DataLayerError::UnexpectedValue(
                    "runtime score window stats result count mismatch".to_string(),
                ));
            }
            for (aggregated, sum, positive_count) in values {
                output.push(match aggregated {
                    0 => None,
                    1 => Some(ScoreWindowU64Stats {
                        sum: sum.parse().map_err(|_| {
                            DataLayerError::UnexpectedValue(
                                "runtime score window stats returned an invalid sum".to_string(),
                            )
                        })?,
                        positive_count,
                    }),
                    _ => {
                        return Err(DataLayerError::UnexpectedValue(
                            "runtime score window stats returned an invalid status".to_string(),
                        ))
                    }
                });
            }
        }
        Ok(output)
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
        blocked_clients: parse_info_u64(info, "blocked_clients"),
        total_connections_received: parse_info_u64(info, "total_connections_received"),
        rejected_connections: parse_info_u64(info, "rejected_connections"),
        total_commands_processed: parse_info_u64(info, "total_commands_processed"),
        instantaneous_ops_per_sec: parse_info_u64(info, "instantaneous_ops_per_sec"),
        total_error_replies: parse_info_u64(info, "total_error_replies"),
        expired_keys: parse_info_u64(info, "expired_keys"),
        evicted_keys: parse_info_u64(info, "evicted_keys"),
        keyspace_hits: parse_info_u64(info, "keyspace_hits"),
        keyspace_misses: parse_info_u64(info, "keyspace_misses"),
        used_memory_bytes: parse_info_u64(info, "used_memory"),
        maxmemory_bytes: parse_info_u64(info, "maxmemory"),
        memory_fragmentation_ratio_basis_points: parse_info_f64_basis_points(
            info,
            "mem_fragmentation_ratio",
        ),
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

fn parse_info_f64_basis_points(info: &str, key: &str) -> Option<u64> {
    info.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name != key {
            return None;
        }
        let parsed = value.trim().parse::<f64>().ok()?;
        (parsed.is_finite() && parsed >= 0.0).then(|| (parsed * 10_000.0).round() as u64)
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
            "# Clients\r\nconnected_clients:5\r\nblocked_clients:2\r\n# Memory\r\nused_memory:1048576\r\nmaxmemory:8388608\r\nmem_fragmentation_ratio:1.25\r\n# Stats\r\ntotal_connections_received:42\r\nrejected_connections:0\r\ntotal_commands_processed:99\r\ninstantaneous_ops_per_sec:7\r\ntotal_error_replies:1\r\nexpired_keys:3\r\nevicted_keys:4\r\nkeyspace_hits:10\r\nkeyspace_misses:2\r\n",
            Vec::new(),
        );

        assert_eq!(
            parsed,
            RedisRuntimeDiagnostics {
                connected_clients: Some(5),
                blocked_clients: Some(2),
                total_connections_received: Some(42),
                rejected_connections: Some(0),
                total_commands_processed: Some(99),
                instantaneous_ops_per_sec: Some(7),
                total_error_replies: Some(1),
                expired_keys: Some(3),
                evicted_keys: Some(4),
                keyspace_hits: Some(10),
                keyspace_misses: Some(2),
                used_memory_bytes: Some(1_048_576),
                maxmemory_bytes: Some(8_388_608),
                memory_fragmentation_ratio_basis_points: Some(12_500),
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
