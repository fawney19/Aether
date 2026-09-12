use super::*;

type TestConnection = ::redis::aio::MultiplexedConnection;

// Frozen pre-optimization script, used as an oracle for out-of-order histories.
const LEGACY_USAGE_LIMIT_SCRIPT: &str = r#"
local count = #KEYS
local now = tonumber(ARGV[1])
local event_id = ARGV[2]
for i = 1, count do
    local window_ms = tonumber(ARGV[(i - 1) * 3 + 4]) * 1000
    redis.call('ZREMRANGEBYSCORE', KEYS[i], '-inf', now - window_ms)
end
for i = 1, count do
    local limit = tonumber(ARGV[(i - 1) * 3 + 3])
    local window_ms = tonumber(ARGV[(i - 1) * 3 + 4]) * 1000
    if not redis.call('ZSCORE', KEYS[i], event_id) then
        local current = redis.call('ZCARD', KEYS[i])
        if current >= limit then
            local earliest = redis.call('ZRANGE', KEYS[i], 0, 0, 'WITHSCORES')
            local retry_after = 1
            if #earliest >= 2 then
                retry_after = math.ceil(math.max(1, tonumber(earliest[2]) + window_ms - now) / 1000)
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

struct Fixture {
    _server: TestRedisServer,
    runtime: RuntimeState,
    admin: TestConnection,
}

impl Fixture {
    async fn start(protocol: &str) -> Option<Self> {
        let Some(server) = TestRedisServer::start().await else {
            eprintln!("usage cleanup {protocol} skipped: isolated Redis unavailable");
            return None;
        };
        let mut admin = redis_test_connection(&server.redis_url).await;
        ::redis::cmd("ACL")
            .arg("SETUSER")
            .arg("usage-cleanup")
            .arg("on")
            .arg(">usage-cleanup-test-password")
            .arg("~*")
            .arg("+@all")
            .query_async::<()>(&mut admin)
            .await
            .unwrap();
        let runtime = RuntimeState::redis(
            RedisClientConfig {
                url: format!(
                    "redis://usage-cleanup:usage-cleanup-test-password@127.0.0.1:{}/6?protocol={protocol}",
                    server.port
                ),
                key_prefix: Some("cleanup".to_string()),
            },
            Some(5_000),
        )
        .await
        .unwrap();
        ::redis::cmd("SELECT")
            .arg(6)
            .query_async::<()>(&mut admin)
            .await
            .unwrap();
        eprintln!("usage cleanup fixture ready: protocol={protocol} db=6 authenticated=true");
        Some(Self {
            _server: server,
            runtime,
            admin,
        })
    }

    async fn consume(
        &self,
        rules: &[UsageLimitRule<'_>],
        event_id: &str,
        now_unix_ms: u64,
    ) -> UsageLimitCheck {
        self.runtime
            .check_and_consume_usage_limits(UsageLimitInput {
                rules,
                event_id,
                now_unix_ms,
            })
            .await
            .unwrap()
    }

    async fn seed(&mut self, prefix: &str, key: &str, count: usize, timestamp: u64) {
        for start in (0..count).step_by(1_000) {
            let mut command = ::redis::cmd("ZADD");
            command.arg(format!("{prefix}:{key}"));
            for index in start..(start + 1_000).min(count) {
                command.arg(timestamp).arg(format!("old-{index:08}"));
            }
            command.query_async::<usize>(&mut self.admin).await.unwrap();
        }
        ::redis::cmd("PEXPIRE")
            .arg(format!("{prefix}:{key}"))
            .arg(600_000)
            .query_async::<usize>(&mut self.admin)
            .await
            .unwrap();
    }

    async fn rows(&mut self, prefix: &str, key: &str) -> Vec<(String, f64)> {
        ::redis::cmd("ZRANGE")
            .arg(format!("{prefix}:{key}"))
            .arg(0)
            .arg(-1)
            .arg("WITHSCORES")
            .query_async(&mut self.admin)
            .await
            .unwrap()
    }

    async fn seed_live(&mut self, prefix: &str, key: &str, count: usize) {
        for start in (0..count).step_by(512) {
            let mut command = ::redis::cmd("ZADD");
            command.arg(format!("{prefix}:{key}"));
            for index in start..(start + 512).min(count) {
                command.arg(100_001).arg(format!("live-{index:08}"));
            }
            command.query_async::<usize>(&mut self.admin).await.unwrap();
        }
    }

    async fn wait_for_copy(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let (_, keys): (u64, Vec<String>) = ::redis::cmd("SCAN")
                    .arg(0)
                    .arg("MATCH")
                    .arg("cleanup:*:__usage_copy:*")
                    .arg("COUNT")
                    .arg(1000)
                    .query_async(&mut self.admin)
                    .await
                    .unwrap();
                if let Some(key) = keys.into_iter().next() {
                    return key;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("copy must become observable between its bounded chunks")
    }

    async fn calls(&mut self, command: &str) -> u64 {
        let info: ::redis::InfoDict = ::redis::cmd("INFO")
            .arg("commandstats")
            .query_async(&mut self.admin)
            .await
            .unwrap();
        info.get::<String>(&format!("cmdstat_{command}"))
            .and_then(|value| {
                value.split(',').find_map(|field| {
                    field
                        .strip_prefix("calls=")
                        .and_then(|value| value.parse().ok())
                })
            })
            .unwrap_or(0)
    }

    async fn reset_slowlog(&mut self) {
        ::redis::cmd("CONFIG")
            .arg("SET")
            .arg("slowlog-log-slower-than")
            .arg(0)
            .arg("slowlog-max-len")
            .arg(4096)
            .query_async::<()>(&mut self.admin)
            .await
            .unwrap();
        ::redis::cmd("SLOWLOG")
            .arg("RESET")
            .query_async::<()>(&mut self.admin)
            .await
            .unwrap();
    }

    async fn max_command_us(&mut self) -> i64 {
        let entries: Vec<(i64, i64, i64, Vec<String>, String, String)> = ::redis::cmd("SLOWLOG")
            .arg("GET")
            .arg(4096)
            .query_async(&mut self.admin)
            .await
            .unwrap();
        entries.into_iter().map(|entry| entry.2).max().unwrap_or(0)
    }

    async fn legacy(
        &mut self,
        rules: &[UsageLimitRule<'_>],
        event_id: &str,
        now: u64,
    ) -> UsageLimitCheck {
        let script = ::redis::Script::new(LEGACY_USAGE_LIMIT_SCRIPT);
        let mut invocation = script.prepare_invoke();
        for rule in rules {
            invocation.key(format!("legacy:{}", rule.key));
        }
        invocation.arg(now).arg(event_id);
        for rule in rules {
            invocation
                .arg(rule.limit)
                .arg(rule.window_seconds)
                .arg(rule.retention_seconds);
        }
        let result: Vec<i64> = invocation.invoke_async(&mut self.admin).await.unwrap();
        match result.as_slice() {
            [1, 0, 0, 0] => UsageLimitCheck::Allowed,
            [0, index, limit, retry_after] => UsageLimitCheck::Rejected {
                rule_index: *index as usize - 1,
                limit: *limit as u64,
                retry_after: *retry_after as u64,
            },
            _ => panic!("unexpected legacy result: {result:?}"),
        }
    }
}

fn rule(key: &str, limit: u64, window_seconds: u64) -> UsageLimitRule<'_> {
    UsageLimitRule {
        key,
        limit,
        window_seconds,
        retention_seconds: 600,
    }
}

#[tokio::test]
async fn redis_usage_cleanup_whole_window_detaches_and_reuses_key_without_bulk_delete() {
    for protocol in ["resp2", "resp3"] {
        let Some(mut fixture) = Fixture::start(protocol).await else {
            return;
        };
        let rules = [UsageLimitRule {
            retention_seconds: 10,
            ..rule("usage:{user}:whole", 1, 10)
        }];
        fixture
            .seed("cleanup", rules[0].key, 100_000, 100_000)
            .await;
        let unlinks = fixture.calls("unlink").await;
        let trims = fixture.calls("zremrangebyscore").await;
        assert_eq!(
            fixture.consume(&rules, "old-00000000", 110_000).await,
            UsageLimitCheck::Allowed
        );
        assert_eq!(fixture.calls("unlink").await, unlinks + 1);
        assert_eq!(fixture.calls("zremrangebyscore").await, trims);
        assert_eq!(
            fixture.rows("cleanup", rules[0].key).await,
            vec![("old-00000000".to_string(), 110_000.0)]
        );
        assert_eq!(
            fixture.consume(&rules, "old-00000000", 110_500).await,
            UsageLimitCheck::Allowed
        );
        assert_eq!(fixture.rows("cleanup", rules[0].key).await[0].1, 110_000.0);
        let ttl: i64 = ::redis::cmd("PTTL")
            .arg(format!("cleanup:{}", rules[0].key))
            .query_async(&mut fixture.admin)
            .await
            .unwrap();
        assert!(
            (8_000..=11_000).contains(&ttl),
            "new key must receive its current retention: {ttl}"
        );
        fixture
            .runtime
            .release_usage_limits(UsageLimitReleaseInput {
                rules: &rules,
                event_id: "old-00000000",
            })
            .await
            .unwrap();
        assert!(fixture.rows("cleanup", rules[0].key).await.is_empty());
        assert_eq!(
            fixture.consume(&rules, "replacement", 110_500).await,
            UsageLimitCheck::Allowed
        );
    }
}

#[tokio::test]
async fn redis_usage_cleanup_preserves_live_boundary_and_out_of_order_rejection() {
    let Some(mut fixture) = Fixture::start("resp3").await else {
        return;
    };
    let rules = [rule("usage:{user}:mixed", 1, 10)];
    fixture.seed("cleanup", rules[0].key, 4_096, 100_000).await;
    ::redis::cmd("ZADD")
        .arg(format!("cleanup:{}", rules[0].key))
        .arg(100_001)
        .arg("live")
        .query_async::<usize>(&mut fixture.admin)
        .await
        .unwrap();
    let unlinks = fixture.calls("unlink").await;
    assert_eq!(
        fixture.consume(&rules, "new", 110_000).await,
        UsageLimitCheck::Rejected {
            rule_index: 0,
            limit: 1,
            retry_after: 1
        }
    );
    assert_eq!(fixture.calls("unlink").await, unlinks + 1);
    assert_eq!(
        fixture.rows("cleanup", rules[0].key).await,
        vec![("live".to_string(), 100_001.0)]
    );
    assert_eq!(
        fixture.consume(&rules, "new", 109_000).await,
        UsageLimitCheck::Rejected {
            rule_index: 0,
            limit: 1,
            retry_after: 2
        }
    );
    assert_eq!(
        fixture.consume(&rules, "new", 110_001).await,
        UsageLimitCheck::Allowed
    );
}

#[tokio::test]
async fn redis_usage_cleanup_keeps_multi_window_denial_atomic_and_cleans_later_windows() {
    let Some(mut fixture) = Fixture::start("resp2").await else {
        return;
    };
    let rules = [
        rule("usage:{user}:full", 1, 60),
        rule("usage:{user}:expired", 1, 10),
    ];
    for rule in &rules {
        fixture
            .seed(
                "cleanup",
                rule.key,
                if rule.window_seconds == 10 { 1_024 } else { 1 },
                100_000,
            )
            .await;
    }
    assert_eq!(
        fixture.consume(&rules, "denied", 110_000).await,
        UsageLimitCheck::Rejected {
            rule_index: 0,
            limit: 1,
            retry_after: 50
        }
    );
    assert!(fixture.rows("cleanup", rules[1].key).await.is_empty());
    assert_eq!(
        fixture.rows("cleanup", rules[0].key).await,
        vec![("old-00000000".to_string(), 100_000.0)]
    );
    assert_eq!(
        fixture.consume(&rules[1..], "denied", 101_000).await,
        UsageLimitCheck::Allowed
    );
    assert_eq!(
        fixture.rows("cleanup", rules[1].key).await,
        vec![("denied".to_string(), 101_000.0)]
    );
}

#[tokio::test]
async fn redis_usage_cleanup_mixed_rebuild_preserves_rows_ttl_and_replay_boundaries() {
    for protocol in ["resp2", "resp3"] {
        let Some(mut fixture) = Fixture::start(protocol).await else {
            return;
        };
        for live in [1_usize, 16, 256, 257, 4_096, 16_384] {
            let key = format!("usage:{{user}}:mixed-{live}");
            let rules = [rule(&key, live as u64, 10)];
            fixture.seed("cleanup", &key, 10_000, 100_000).await;
            let mut command = ::redis::cmd("ZADD");
            command.arg(format!("cleanup:{key}"));
            let rows = (0..live)
                .map(|i| (format!("live-{i:03}"), 100_001.0 + i as f64))
                .collect::<Vec<_>>();
            for (member, score) in &rows {
                command.arg(score).arg(member);
            }
            command
                .query_async::<usize>(&mut fixture.admin)
                .await
                .unwrap();
            let ttl_before: i64 = ::redis::cmd("PTTL")
                .arg(format!("cleanup:{key}"))
                .query_async(&mut fixture.admin)
                .await
                .unwrap();
            let unlinks = fixture.calls("unlink").await;
            assert!(matches!(
                fixture.consume(&rules, "new", 110_000).await,
                UsageLimitCheck::Rejected { .. }
            ));
            assert_eq!(fixture.rows("cleanup", &key).await, rows);
            assert_eq!(fixture.calls("unlink").await - unlinks, 1);
            let ttl_after: i64 = ::redis::cmd("PTTL")
                .arg(format!("cleanup:{key}"))
                .query_async(&mut fixture.admin)
                .await
                .unwrap();
            assert!(ttl_after <= ttl_before && ttl_after >= ttl_before - 2_000);
            assert_eq!(
                fixture.consume(&rules, "live-000", 109_000).await,
                UsageLimitCheck::Allowed
            );
            assert_eq!(fixture.rows("cleanup", &key).await, rows);
            assert!(matches!(
                fixture.consume(&rules, "old-00000000", 109_000).await,
                UsageLimitCheck::Rejected { .. }
            ));
            let temporary_exists: bool = ::redis::cmd("EXISTS")
                .arg(format!("cleanup:{key}:__usage_trim"))
                .query_async(&mut fixture.admin)
                .await
                .unwrap();
            assert!(!temporary_exists);
        }
    }
}

#[tokio::test]
async fn redis_usage_cleanup_mixed_optional_acl_denials_keep_exact_state() {
    let Some(mut fixture) = Fixture::start("resp3").await else {
        return;
    };
    for denied in ["zcount", "pttl", "exists", "unlink", "rename", "pexpire"] {
        let key = format!("usage:{{user}}:acl-{denied}");
        fixture.seed("cleanup", &key, 2_048, 100_000).await;
        ::redis::cmd("ZADD")
            .arg(format!("cleanup:{key}"))
            .arg(100_001)
            .arg("live")
            .query_async::<usize>(&mut fixture.admin)
            .await
            .unwrap();
        ::redis::cmd("ACL")
            .arg("SETUSER")
            .arg("usage-cleanup")
            .arg("+@all")
            .arg(format!("-{denied}"))
            .query_async::<()>(&mut fixture.admin)
            .await
            .unwrap();
        assert!(matches!(
            fixture.consume(&[rule(&key, 1, 10)], "new", 110_000).await,
            UsageLimitCheck::Rejected { .. }
        ));
        assert_eq!(
            fixture.rows("cleanup", &key).await,
            vec![("live".into(), 100_001.0)]
        );
    }
}

#[tokio::test]
async fn redis_usage_cleanup_mixed_preserves_nonexpiring_keys_and_existing_temporary_keys() {
    let Some(mut fixture) = Fixture::start("resp3").await else {
        return;
    };
    for collision in [false, true] {
        let key = format!("usage:{{user}}:persistent-{collision}");
        fixture.seed("cleanup", &key, 2_048, 100_000).await;
        ::redis::cmd("PERSIST")
            .arg(format!("cleanup:{key}"))
            .query_async::<bool>(&mut fixture.admin)
            .await
            .unwrap();
        ::redis::cmd("ZADD")
            .arg(format!("cleanup:{key}"))
            .arg(100_001)
            .arg("live")
            .query_async::<usize>(&mut fixture.admin)
            .await
            .unwrap();
        if collision {
            ::redis::cmd("SET")
                .arg(format!("cleanup:{key}:__usage_trim"))
                .arg("unrelated")
                .query_async::<()>(&mut fixture.admin)
                .await
                .unwrap();
        }
        assert!(matches!(
            fixture.consume(&[rule(&key, 1, 10)], "new", 110_000).await,
            UsageLimitCheck::Rejected { .. }
        ));
        let ttl: i64 = ::redis::cmd("PTTL")
            .arg(format!("cleanup:{key}"))
            .query_async(&mut fixture.admin)
            .await
            .unwrap();
        assert_eq!(ttl, -1);
        assert_eq!(
            fixture.rows("cleanup", &key).await,
            vec![("live".into(), 100_001.0)]
        );
        if collision {
            let value: String = ::redis::cmd("GET")
                .arg(format!("cleanup:{key}:__usage_trim"))
                .query_async(&mut fixture.admin)
                .await
                .unwrap();
            assert_eq!(value, "unrelated");
        }
    }
}

#[tokio::test]
async fn redis_usage_cleanup_large_and_small_windows_keep_exact_cutoffs() {
    let Some(mut fixture) = Fixture::start("resp3").await else {
        return;
    };
    for (index, count) in [0, 1, 255, 256, 257, 1_024].into_iter().enumerate() {
        let key = format!("usage:{{user}}:boundary-{index}");
        let rules = [rule(&key, 1, 1)];
        let timestamp = MAX_REDIS_LUA_EXACT_INTEGER - 2_000_000;
        fixture.seed("cleanup", &key, count, timestamp).await;
        let unlinks = fixture.calls("unlink").await;
        let result = fixture.consume(&rules, "new", timestamp + 999).await;
        assert_eq!(
            result,
            if count == 0 {
                UsageLimitCheck::Allowed
            } else {
                UsageLimitCheck::Rejected {
                    rule_index: 0,
                    limit: 1,
                    retry_after: 1,
                }
            }
        );
        assert_eq!(fixture.calls("unlink").await, unlinks);
        assert_eq!(
            fixture.consume(&rules, "new", timestamp + 1_000).await,
            UsageLimitCheck::Allowed
        );
        assert_eq!(
            fixture.calls("unlink").await,
            unlinks + u64::from(count > 256)
        );
        assert_eq!(
            fixture.rows("cleanup", &key).await,
            vec![(
                "new".to_string(),
                (timestamp + if count == 0 { 999 } else { 1_000 }) as f64
            )]
        );
    }
}

#[tokio::test]
async fn redis_usage_cleanup_unlink_acl_denial_uses_existing_atomic_trim() {
    for protocol in ["resp2", "resp3"] {
        let Some(mut fixture) = Fixture::start(protocol).await else {
            return;
        };
        let rules = [rule("usage:{user}:acl", 1, 10)];
        fixture.seed("cleanup", rules[0].key, 4_096, 100_000).await;
        ::redis::cmd("ACL")
            .arg("SETUSER")
            .arg("usage-cleanup")
            .arg("-unlink")
            .query_async::<()>(&mut fixture.admin)
            .await
            .unwrap();
        let trims = fixture.calls("zremrangebyscore").await;
        assert_eq!(
            fixture.consume(&rules, "new", 110_000).await,
            UsageLimitCheck::Allowed
        );
        assert_eq!(fixture.calls("zremrangebyscore").await, trims + 1);
        assert_eq!(
            fixture.rows("cleanup", rules[0].key).await,
            vec![("new".to_string(), 110_000.0)]
        );
    }
}

#[tokio::test]
async fn redis_usage_cleanup_concurrent_expiration_preserves_the_admission_cap() {
    let Some(mut fixture) = Fixture::start("resp3").await else {
        return;
    };
    let rules = [rule("usage:{user}:parallel-expired", 8, 10)];
    fixture.seed("cleanup", rules[0].key, 32_768, 100_000).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(64));
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..64 {
        let runtime = fixture.runtime.clone();
        let barrier = Arc::clone(&barrier);
        tasks.spawn(async move {
            let event_id = format!("new-{index}");
            barrier.wait().await;
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &rules,
                    event_id: &event_id,
                    now_unix_ms: 110_000,
                })
                .await
                .unwrap()
        });
    }
    let mut allowed = 0;
    while let Some(result) = tasks.join_next().await {
        match result.unwrap() {
            UsageLimitCheck::Allowed => allowed += 1,
            UsageLimitCheck::Rejected {
                rule_index: 0,
                limit: 8,
                retry_after: 10,
            } => {}
            other => panic!("unexpected admission: {other:?}"),
        }
    }
    assert_eq!(allowed, 8);
    assert_eq!(fixture.calls("unlink").await, 1);
    assert_eq!(fixture.rows("cleanup", rules[0].key).await.len(), 8);
}

#[tokio::test]
async fn redis_usage_cleanup_cached_script_recovers_after_flush_without_reconsuming() {
    let Some(mut fixture) = Fixture::start("resp3").await else {
        return;
    };
    let rules = [rule("usage:{user}:script-reload", 1, 10)];
    assert_eq!(
        fixture.consume(&rules, "original", 100_000).await,
        UsageLimitCheck::Allowed
    );
    ::redis::cmd("SCRIPT")
        .arg("FLUSH")
        .query_async::<()>(&mut fixture.admin)
        .await
        .unwrap();
    assert_eq!(
        fixture.consume(&rules, "original", 101_000).await,
        UsageLimitCheck::Allowed
    );
    assert_eq!(
        fixture.consume(&rules, "extra", 101_000).await,
        UsageLimitCheck::Rejected {
            rule_index: 0,
            limit: 1,
            retry_after: 9,
        }
    );
    assert_eq!(
        fixture.rows("cleanup", rules[0].key).await,
        vec![("original".to_string(), 100_000.0)]
    );
}

#[tokio::test]
async fn redis_usage_cleanup_matches_legacy_for_replays_releases_and_regressing_time() {
    for protocol in ["resp2", "resp3"] {
        let Some(mut fixture) = Fixture::start(protocol).await else {
            return;
        };
        let mut rules = [
            rule("usage:{user}:one", 5, 1),
            rule("usage:{user}:ten", 8, 10),
            rule("usage:{user}:minute", 13, 60),
        ];
        for rule in rules {
            for prefix in ["cleanup", "legacy"] {
                fixture.seed(prefix, rule.key, 512, 50_000).await;
            }
        }
        let mut random = 0x1f28_e05b_a39d_c642_u64;
        for index in 0..512 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let now = if index % 9 == 0 {
                110_000
            } else {
                random % 130_001
            };
            let event = format!("event-{}", random % 19);
            rules[0].limit = 1 + random % 7;
            let actual = fixture.consume(&rules, &event, now).await;
            let expected = fixture.legacy(&rules, &event, now).await;
            assert_eq!(
                actual, expected,
                "protocol={protocol} step={index} now={now} event={event}"
            );
            if index % 5 == 0 {
                fixture
                    .runtime
                    .release_usage_limits(UsageLimitReleaseInput {
                        rules: &rules,
                        event_id: &event,
                    })
                    .await
                    .unwrap();
                for rule in rules {
                    ::redis::cmd("ZREM")
                        .arg(format!("legacy:{}", rule.key))
                        .arg(&event)
                        .query_async::<usize>(&mut fixture.admin)
                        .await
                        .unwrap();
                }
            }
            for rule in rules {
                assert_eq!(
                    fixture.rows("cleanup", rule.key).await,
                    fixture.rows("legacy", rule.key).await,
                    "protocol={protocol} step={index} key={}",
                    rule.key
                );
            }
        }
    }
}

#[tokio::test]
#[ignore = "isolated large-window timing baseline"]
async fn redis_usage_cleanup_large_window_timing() {
    let mut fixture = Fixture::start("resp3")
        .await
        .expect("timing baseline requires Redis");
    const MEMBERS: usize = 300_000;
    let mut measurements = Vec::new();
    for round in 0..3 {
        let key = format!("usage:{{user}}:timing-{round}");
        let rules = [rule(&key, 1, 10)];
        for prefix in ["legacy", "cleanup"] {
            fixture.seed(prefix, &key, MEMBERS, 100_000).await;
        }
        let started = std::time::Instant::now();
        assert_eq!(
            fixture.legacy(&rules, "new", 110_000).await,
            UsageLimitCheck::Allowed
        );
        let legacy_us = started.elapsed().as_micros();
        let started = std::time::Instant::now();
        assert_eq!(
            fixture.consume(&rules, "new", 110_000).await,
            UsageLimitCheck::Allowed
        );
        let optimized_us = started.elapsed().as_micros();
        assert_eq!(
            fixture.rows("cleanup", &key).await,
            fixture.rows("legacy", &key).await
        );
        measurements.push(serde_json::json!({"members":MEMBERS,"legacy_us":legacy_us,"optimized_us":optimized_us}));
    }
    eprintln!(
        "usage cleanup timing: {}",
        serde_json::to_string(&measurements).unwrap()
    );

    let mut mixed = Vec::new();
    for live in [1, 64, 256, 257, 4_096, 32_768, 150_000] {
        let key = format!("usage:{{user}}:mixed-timing-{live}");
        let rules = [rule(&key, live, 10)];
        for prefix in ["legacy", "cleanup"] {
            fixture.seed(prefix, &key, MEMBERS, 100_000).await;
            fixture.seed_live(prefix, &key, live as usize).await;
        }
        fixture.reset_slowlog().await;
        let started = std::time::Instant::now();
        let legacy = fixture.legacy(&rules, "new", 110_000).await;
        let legacy_us = started.elapsed().as_micros();
        let legacy_max_command_us = fixture.max_command_us().await;
        fixture.reset_slowlog().await;
        let started = std::time::Instant::now();
        assert_eq!(fixture.consume(&rules, "new", 110_000).await, legacy);
        let optimized_us = started.elapsed().as_micros();
        let optimized_max_command_us = fixture.max_command_us().await;
        assert_eq!(
            fixture.rows("cleanup", &key).await,
            fixture.rows("legacy", &key).await
        );
        mixed.push(serde_json::json!({"expired": MEMBERS, "live": live, "legacy_us": legacy_us, "optimized_us": optimized_us,
            "legacy_max_command_us": legacy_max_command_us, "optimized_max_command_us": optimized_max_command_us}));
    }
    eprintln!(
        "usage cleanup mixed timing: {}",
        serde_json::to_string(&mixed).unwrap()
    );

    let rules = [rule("usage:{user}:steady", 10_000, 60)];
    for prefix in ["legacy", "cleanup"] {
        fixture.seed(prefix, rules[0].key, 4_096, 100_000).await;
    }
    let mut legacy_us = Vec::new();
    let mut optimized_us = Vec::new();
    for index in 0..1_000 {
        let event_id = format!("steady-{index}");
        let now = 101_000 + index;
        let started = std::time::Instant::now();
        assert_eq!(
            fixture.legacy(&rules, &event_id, now).await,
            UsageLimitCheck::Allowed
        );
        legacy_us.push(started.elapsed().as_micros());
        let started = std::time::Instant::now();
        assert_eq!(
            fixture.consume(&rules, &event_id, now).await,
            UsageLimitCheck::Allowed
        );
        optimized_us.push(started.elapsed().as_micros());
    }
    assert_eq!(
        fixture.rows("cleanup", rules[0].key).await,
        fixture.rows("legacy", rules[0].key).await
    );
    legacy_us.sort_unstable();
    optimized_us.sort_unstable();
    eprintln!(
        "usage cleanup steady timing: {}",
        serde_json::json!({
            "requests_per_script": 1_000,
            "seeded_live_members": 4_096,
            "legacy_p50_us": legacy_us[499],
            "legacy_p95_us": legacy_us[949],
            "optimized_p50_us": optimized_us[499],
            "optimized_p95_us": optimized_us[949],
        })
    );
}

#[tokio::test]
async fn redis_usage_cleanup_chunked_copy_retries_when_another_rule_changes() {
    let Some(mut fixture) = Fixture::start("resp3").await else {
        return;
    };
    let rules = [
        rule("usage:{copy}:short", 1, 10),
        rule("usage:{copy}:large", 100_001, 10),
    ];
    fixture.seed("cleanup", rules[0].key, 1, 100_000).await;
    fixture.seed("cleanup", rules[1].key, 20_000, 100_000).await;
    fixture.seed_live("cleanup", rules[1].key, 100_000).await;
    let runtime = fixture.runtime.clone();
    let task = tokio::spawn(async move {
        runtime
            .check_and_consume_usage_limits(UsageLimitInput {
                rules: &rules,
                event_id: "copier",
                now_unix_ms: 110_000,
            })
            .await
            .unwrap()
    });
    fixture.wait_for_copy().await;
    // No earlier rule may have been pruned before the atomic commit.
    assert_eq!(fixture.rows("cleanup", rules[0].key).await.len(), 1);
    ::redis::cmd("ZADD")
        .arg(format!("cleanup:{}", rules[0].key))
        .arg(109_000)
        .arg("concurrent-consumer")
        .query_async::<usize>(&mut fixture.admin)
        .await
        .unwrap();
    ::redis::cmd("ZREM")
        .arg(format!("cleanup:{}", rules[1].key))
        .arg("live-00000000")
        .query_async::<usize>(&mut fixture.admin)
        .await
        .unwrap();
    assert_eq!(
        task.await.unwrap(),
        UsageLimitCheck::Rejected {
            rule_index: 0,
            limit: 1,
            retry_after: 9,
        }
    );
    let rows = fixture.rows("cleanup", rules[1].key).await;
    assert_eq!(rows.len(), 99_999);
    assert!(!rows
        .iter()
        .any(|(id, _)| id == "copier" || id == "live-00000000"));
    assert!(fixture.calls("watch").await >= 2);
    assert_eq!(fixture.calls("zremrangebyscore").await, 1);
}

#[tokio::test]
async fn redis_usage_cleanup_cancelled_copy_keeps_source_and_expires_scratch() {
    let Some(mut fixture) = Fixture::start("resp2").await else {
        return;
    };
    let rules = [rule("usage:{copy}:cancelled", 100_001, 10)];
    fixture.seed("cleanup", rules[0].key, 20_000, 100_000).await;
    fixture.seed_live("cleanup", rules[0].key, 100_000).await;
    let runtime = fixture.runtime.clone();
    let task = tokio::spawn(async move {
        runtime
            .check_and_consume_usage_limits(UsageLimitInput {
                rules: &rules,
                event_id: "cancelled",
                now_unix_ms: 110_000,
            })
            .await
    });
    let temporary = fixture.wait_for_copy().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(fixture.rows("cleanup", rules[0].key).await.len(), 120_000);
    let ttl: i64 = ::redis::cmd("PTTL")
        .arg(&temporary)
        .query_async(&mut fixture.admin)
        .await
        .unwrap();
    assert!(ttl > 0 && ttl <= 60_000);
    assert_eq!(
        fixture.consume(&rules, "replacement", 110_000).await,
        UsageLimitCheck::Allowed
    );
    let rows = fixture.rows("cleanup", rules[0].key).await;
    assert_eq!(rows.len(), 100_001);
    assert!(!rows.iter().any(|(id, _)| id == "cancelled"));
}

#[tokio::test]
async fn redis_usage_cleanup_chunked_copy_concurrency_keeps_both_caps() {
    let Some(mut fixture) = Fixture::start("resp3").await else {
        return;
    };
    let rules = [
        rule("usage:{copy}:parallel-one", 4104, 10),
        rule("usage:{copy}:parallel-two", 4104, 10),
    ];
    for rule in &rules {
        fixture.seed("cleanup", rule.key, 20_000, 100_000).await;
        fixture.seed_live("cleanup", rule.key, 4096).await;
    }
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..64 {
        let runtime = fixture.runtime.clone();
        tasks.spawn(async move {
            runtime
                .check_and_consume_usage_limits(UsageLimitInput {
                    rules: &rules,
                    event_id: &format!("parallel-{index}"),
                    now_unix_ms: 110_000,
                })
                .await
                .unwrap()
        });
    }
    let mut allowed = 0;
    while let Some(result) = tasks.join_next().await {
        allowed += usize::from(result.unwrap() == UsageLimitCheck::Allowed);
    }
    assert_eq!(allowed, 8);
    let first = fixture.rows("cleanup", rules[0].key).await;
    assert_eq!(first.len(), 4104);
    assert_eq!(first, fixture.rows("cleanup", rules[1].key).await);
    assert_eq!(fixture.calls("zremrangebyscore").await, 0);
}

#[tokio::test]
async fn redis_usage_cleanup_chunked_copy_optional_acl_denials_preserve_legacy_behavior() {
    let Some(mut fixture) = Fixture::start("resp3").await else {
        return;
    };
    for denied in ["watch", "unwatch", "multi", "exec", "eval", "persist"] {
        let key = format!("usage:{{copy}}:acl-{denied}");
        fixture.seed("cleanup", &key, 8192, 100_000).await;
        fixture.seed_live("cleanup", &key, 512).await;
        ::redis::cmd("ACL")
            .arg("SETUSER")
            .arg("usage-cleanup")
            .arg("+@all")
            .arg(format!("-{denied}"))
            .query_async::<()>(&mut fixture.admin)
            .await
            .unwrap();
        let trims = fixture.calls("zremrangebyscore").await;
        assert_eq!(
            fixture
                .consume(&[rule(&key, 512, 10)], "new", 110_000)
                .await,
            UsageLimitCheck::Rejected {
                rule_index: 0,
                limit: 512,
                retry_after: 1
            }
        );
        assert_eq!(fixture.rows("cleanup", &key).await.len(), 512);
        assert_eq!(fixture.calls("zremrangebyscore").await, trims + 1);
    }
}

#[tokio::test]
async fn redis_usage_cleanup_chunked_copy_matches_legacy_across_regressing_time() {
    for protocol in ["resp2", "resp3"] {
        let Some(mut fixture) = Fixture::start(protocol).await else {
            return;
        };
        let rules = [
            rule("usage:{history}:short", 512, 10),
            rule("usage:{history}:long", 9000, 60),
        ];
        for prefix in ["cleanup", "legacy"] {
            for rule in &rules {
                fixture.seed(prefix, rule.key, 8192, 100_000).await;
                fixture.seed_live(prefix, rule.key, 512).await;
            }
            ::redis::cmd("PERSIST")
                .arg(format!("{prefix}:{}", rules[0].key))
                .query_async::<bool>(&mut fixture.admin)
                .await
                .unwrap();
        }
        for (event, now) in [
            ("denied", 110_000),
            ("live-00000000", 109_000),
            ("old-00000000", 1000),
            ("expired", 110_001),
            ("backdated", 102_000),
            ("new", 160_001),
        ] {
            let expected = fixture.legacy(&rules, event, now).await;
            assert_eq!(fixture.consume(&rules, event, now).await, expected);
            for rule in &rules {
                assert_eq!(
                    fixture.rows("cleanup", rule.key).await,
                    fixture.rows("legacy", rule.key).await
                );
            }
        }
        assert!(fixture.calls("watch").await > 0);
    }
}

#[tokio::test]
async fn redis_usage_cleanup_copy_refuses_existing_temporary_and_denied_expiration() {
    let Some(mut fixture) = Fixture::start("resp3").await else {
        return;
    };
    let source = "cleanup:usage:{copy}:source";
    let target = "cleanup:usage:{copy}:existing";
    fixture
        .seed_live("cleanup", "usage:{copy}:source", 512)
        .await;
    ::redis::cmd("SET")
        .arg(target)
        .arg("unrelated")
        .query_async::<()>(&mut fixture.admin)
        .await
        .unwrap();
    let mut connection = redis_test_connection(&format!(
        "redis://usage-cleanup:usage-cleanup-test-password@127.0.0.1:{}/6",
        fixture._server.port,
    ))
    .await;
    let script = ::redis::Script::new(include_str!("usage_copy.lua"));
    let result = script
        .key(source)
        .key(target)
        .arg(0)
        .arg(511)
        .arg(0)
        .invoke_async::<i64>(&mut connection)
        .await;
    assert!(result.is_err());
    let value: String = ::redis::cmd("GET")
        .arg(target)
        .query_async(&mut fixture.admin)
        .await
        .unwrap();
    assert_eq!(value, "unrelated");
    ::redis::cmd("ACL")
        .arg("SETUSER")
        .arg("usage-cleanup")
        .arg("-pexpire")
        .query_async::<()>(&mut fixture.admin)
        .await
        .unwrap();
    let result = script
        .key(source)
        .key("cleanup:usage:{copy}:denied")
        .arg(0)
        .arg(511)
        .arg(0)
        .invoke_async::<i64>(&mut connection)
        .await;
    assert!(result.is_err());
    let exists: bool = ::redis::cmd("EXISTS")
        .arg("cleanup:usage:{copy}:denied")
        .query_async(&mut fixture.admin)
        .await
        .unwrap();
    assert!(!exists);
    assert_eq!(
        fixture.rows("cleanup", "usage:{copy}:source").await.len(),
        512
    );
}
