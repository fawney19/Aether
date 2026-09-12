use std::sync::OnceLock;

use super::client::RedisBlockingStreamLease;
use super::runtime::USAGE_LIMIT_CHECK_AND_CONSUME_SCRIPT;
use super::{cmd, RedisConnectionLane, RedisConnectionRouter};
use crate::error::RedisResultExt;
use crate::{DataLayerError, UsageLimitInput};

const COPY_CHUNK_SIZE: i64 = 512;
const MAX_COPY_ATTEMPTS: usize = 8;
const COPY_SCRIPT: &str = include_str!("usage_copy.lua");
const COMMIT_PREFIX: &str = include_str!("usage_copy_commit.lua");

fn usage_args(command: &mut redis::Cmd, input: &UsageLimitInput<'_>) {
    command.arg(input.now_unix_ms).arg(input.event_id);
    for rule in input.rules {
        command
            .arg(rule.limit)
            .arg(rule.window_seconds)
            .arg(rule.retention_seconds);
    }
}

pub(super) async fn check_and_consume(
    connections: &RedisConnectionRouter,
    keys: &[String],
    input: &UsageLimitInput<'_>,
) -> Result<Vec<i64>, DataLayerError> {
    static SCRIPT: OnceLock<redis::Script> = OnceLock::new();
    let script = SCRIPT.get_or_init(|| redis::Script::new(USAGE_LIMIT_CHECK_AND_CONSUME_SCRIPT));
    let mut invocation = script.prepare_invoke();
    for key in keys {
        invocation.key(key);
    }
    invocation.arg(input.now_unix_ms).arg(input.event_id);
    for rule in input.rules {
        invocation
            .arg(rule.limit)
            .arg(rule.window_seconds)
            .arg(rule.retention_seconds);
    }
    let result: Vec<i64> = invocation
        .invoke_async(&mut connections.connection(RedisConnectionLane::Fast))
        .await
        .map_redis_err()?;
    if result.first() != Some(&2) {
        return Ok(result);
    }

    let mut lease = connections.usage_cleanup_connection().await?;
    for _ in 0..MAX_COPY_ATTEMPTS {
        let mut temporary_keys = Vec::new();
        let result = copy_and_commit(&mut lease, keys, input, &mut temporary_keys).await;
        // EXEC clears WATCH even on a conflict. Errors discard the lease,
        // including any pending WATCH/MULTI state.
        if let Ok(Some((result, committed))) = result.as_ref() {
            // Never add another fallible round trip after admission. EXEC already
            // cleared WATCH; the recheck fast path instead discards its watched lease.
            if *committed {
                lease.recycle();
            }
            return Ok(result.clone());
        } else if result.is_ok() {
            lease.query(&cmd("UNWATCH")).await?;
            if !temporary_keys.is_empty() {
                lease.query(cmd("UNLINK").arg(&temporary_keys)).await?;
            }
        } else if !temporary_keys.is_empty() {
            // A fresh connection cannot accidentally queue cleanup inside a failed MULTI.
            let mut connection = connections.connection(RedisConnectionLane::Admin);
            let _ = cmd("UNLINK")
                .arg(&temporary_keys)
                .query_async::<usize>(&mut connection)
                .await;
        }
        result?;
        tokio::task::yield_now().await;
    }
    lease.recycle();
    Err(DataLayerError::Redis(
        "usage window cleanup conflicted repeatedly; retry the request".to_string(),
    ))
}

async fn copy_and_commit(
    lease: &mut RedisBlockingStreamLease,
    keys: &[String],
    input: &UsageLimitInput<'_>,
    temporary_keys: &mut Vec<String>,
) -> Result<Option<(Vec<i64>, bool)>, DataLayerError> {
    lease.query(cmd("WATCH").arg(keys)).await?;
    let mut check = cmd("EVAL");
    check
        .arg(USAGE_LIMIT_CHECK_AND_CONSUME_SCRIPT)
        .arg(keys.len())
        .arg(keys);
    usage_args(&mut check, input);
    let plan: Vec<i64> =
        redis::from_owned_redis_value(lease.query(&check).await?).map_redis_err()?;
    if plan.first() != Some(&2) {
        return Ok(Some((plan, false)));
    }
    if plan.len() < 4 || !(plan.len() - 1).is_multiple_of(3) {
        return Err(DataLayerError::UnexpectedValue(
            "invalid usage window copy plan".to_string(),
        ));
    }

    let nonce = uuid::Uuid::new_v4();
    for window in plan[1..].chunks_exact(3) {
        let [index, expired, live] = [window[0], window[1], window[2]];
        let key = usize::try_from(index - 1)
            .ok()
            .and_then(|index| keys.get(index))
            .filter(|_| expired > 0 && live > 0)
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue("invalid usage window copy range".to_string())
            })?;
        let temporary = format!("{key}:__usage_copy:{nonce}");
        let mut offset = 0;
        while offset < live {
            let take = COPY_CHUNK_SIZE.min(live - offset);
            let value = lease
                .query(
                    cmd("EVAL")
                        .arg(COPY_SCRIPT)
                        .arg(2)
                        .arg(key)
                        .arg(&temporary)
                        .arg(expired + offset)
                        .arg(expired + offset + take - 1)
                        .arg(offset),
                )
                .await?;
            let copied: i64 = redis::from_owned_redis_value(value).map_redis_err()?;
            if offset == 0 && copied > 0 {
                temporary_keys.push(temporary.clone());
            }
            if copied != take {
                return Ok(None);
            }
            offset += take;
        }
    }

    static COMMIT: OnceLock<String> = OnceLock::new();
    let source =
        COMMIT.get_or_init(|| format!("{COMMIT_PREFIX}\n{USAGE_LIMIT_CHECK_AND_CONSUME_SCRIPT}"));
    let mut commit = cmd("EVAL");
    commit
        .arg(source)
        .arg(keys.len() + temporary_keys.len())
        .arg(keys)
        .arg(&*temporary_keys)
        .arg(keys.len());
    usage_args(&mut commit, input);
    for window in plan[1..].chunks_exact(3) {
        commit.arg(window[0]).arg(window[2]);
    }
    lease.query(&cmd("MULTI")).await?;
    lease.query(&commit).await?;
    let replies: Option<Vec<redis::Value>> =
        redis::from_owned_redis_value(lease.query(&cmd("EXEC")).await?).map_redis_err()?;
    let Some(mut replies) = replies else {
        return Ok(None);
    };
    let reply = replies.pop().ok_or_else(|| {
        DataLayerError::UnexpectedValue("empty usage window commit response".to_string())
    })?;
    let result: Vec<i64> = redis::from_owned_redis_value(reply).map_redis_err()?;
    if result.first() == Some(&2) {
        return Ok(None);
    }
    Ok(Some((result, true)))
}
