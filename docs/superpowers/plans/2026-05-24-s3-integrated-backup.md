# S3 一体化备份 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在系统设置页增加可配置的 S3-compatible 备份能力，备份范围与现有三种导出一致，并支持手动备份、周期备份和按范围保留最新 N 份。

**Architecture:** 后端新增 `backup` 模块，负责配置解析、周期计算、导出 payload 选择、压缩、上传、保留清理和 worker 调度。配置继续存入 `system_configs`，敏感 `Secret Access Key` 使用现有加密和遮蔽模式。前端在数据管理附近新增“S3 备份”区域，核心字段默认展示，高级字段折叠。

**Tech Stack:** Rust、Axum、Aether background task runtime、`system_configs`、`object_store` with `aws` feature、Vue 3、TypeScript、现有 shadcn-like UI 组件。

---

### 文件结构

后端新增 `apps/aether-gateway/src/backup/mod.rs` 作为模块入口，`config.rs` 负责从系统配置读取和校验 S3 备份配置，`schedule.rs` 负责小时/天/周/月周期校验和到期槽位计算，`scopes.rs` 负责三种备份范围到现有导出 payload 和对象命名的映射，`store.rs` 定义本地对象存储 trait、`object_store` 适配器和 fake 测试实现，`executor.rs` 负责一次备份 run 的生成、压缩、上传和保留清理，`worker.rs` 负责周期 worker。

现有后端会修改 `crates/aether-admin/src/system.rs` 增加 S3 备份配置默认值和敏感 key，修改 `apps/aether-gateway/src/task_runtime/mod.rs` 注册新 task key，修改 `apps/aether-gateway/src/state/core.rs` 启动周期 worker，修改 `apps/aether-gateway/src/control/route/admin/system_families.rs` 和 `apps/aether-gateway/src/handlers/admin/system/core/system_routes.rs` 增加手动备份接口。依赖修改在根 `Cargo.toml` 和 `apps/aether-gateway/Cargo.toml`。

前端新增 `frontend/src/views/admin/system-settings/S3BackupSection.vue`，新增 `frontend/src/views/admin/system-settings/composables/useS3BackupConfig.ts`，修改 `frontend/src/views/admin/SystemSettings.vue` 接入这个区域，修改 `frontend/src/api/admin.ts` 增加手动备份 API 类型和方法。

### Task 1: 配置 key、敏感遮蔽和默认值

**Files:**

Modify: `crates/aether-admin/src/system.rs`

Test: `crates/aether-admin/src/system.rs`

- [ ] **Step 1: 写失败测试**

在 `crates/aether-admin/src/system.rs` 的现有测试模块里增加测试，先证明 S3 Secret 是敏感配置、默认备份范围是完整备份、Secret 详情读取不回显值。

```rust
#[test]
fn s3_backup_secret_access_key_is_sensitive() {
    assert!(is_sensitive_admin_system_config_key("backup_s3_secret_access_key"));
    assert!(is_sensitive_admin_system_config_key("BACKUP_S3_SECRET_ACCESS_KEY"));
    assert!(!is_sensitive_admin_system_config_key("backup_s3_bucket"));
}

#[test]
fn s3_backup_defaults_match_admin_ui_contract() {
    assert_eq!(
        admin_system_config_default_value("backup_s3_scope"),
        Some(json!("data"))
    );
    assert_eq!(
        admin_system_config_default_value("backup_s3_schedule_unit"),
        Some(json!("days"))
    );
    assert_eq!(
        admin_system_config_default_value("backup_s3_schedule_interval"),
        Some(json!(1))
    );
    assert_eq!(
        admin_system_config_default_value("backup_s3_retention_count"),
        Some(json!(7))
    );
}

#[test]
fn s3_backup_secret_detail_is_write_only() {
    let payload = build_admin_system_config_detail_payload(
        "backup_s3_secret_access_key",
        Some(json!("encrypted-secret")),
    )
    .expect("sensitive backup key should render");

    assert_eq!(payload["key"], json!("backup_s3_secret_access_key"));
    assert_eq!(payload["value"], serde_json::Value::Null);
    assert_eq!(payload["is_set"], json!(true));
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `rtk cargo test -p aether-admin s3_backup --lib`

Expected: FAIL，错误包含找不到默认值或敏感 key 断言失败。

- [ ] **Step 3: 实现最小代码**

在 `SENSITIVE_SYSTEM_CONFIG_KEYS` 增加：

```rust
"backup_s3_secret_access_key",
```

在 `admin_system_config_default_value()` 增加：

```rust
"backup_s3_enabled" => Some(json!(false)),
"backup_s3_scope" => Some(json!("data")),
"backup_s3_endpoint" => Some(serde_json::Value::Null),
"backup_s3_region" => Some(json!("auto")),
"backup_s3_bucket" => Some(serde_json::Value::Null),
"backup_s3_prefix" => Some(json!("aether/backups/")),
"backup_s3_access_key_id" => Some(serde_json::Value::Null),
"backup_s3_secret_access_key" => Some(serde_json::Value::Null),
"backup_s3_path_style" => Some(json!(false)),
"backup_s3_compression" => Some(json!("zstd")),
"backup_s3_schedule_unit" => Some(json!("days")),
"backup_s3_schedule_interval" => Some(json!(1)),
"backup_s3_schedule_minute" => Some(json!(0)),
"backup_s3_schedule_hour" => Some(json!(3)),
"backup_s3_schedule_weekday" => Some(json!(1)),
"backup_s3_schedule_month_day" => Some(json!(1)),
"backup_s3_retention_count" => Some(json!(7)),
"backup_s3_last_slot" => Some(serde_json::Value::Null),
```

- [ ] **Step 4: 运行测试确认通过**

Run: `rtk cargo test -p aether-admin s3_backup --lib`

Expected: PASS。

- [ ] **Step 5: 提交**

Run:

```bash
rtk git add crates/aether-admin/src/system.rs
rtk git commit -m "feat: add s3 backup system config keys"
```

### Task 2: 备份配置解析和周期计算

**Files:**

Create: `apps/aether-gateway/src/backup/mod.rs`

Create: `apps/aether-gateway/src/backup/config.rs`

Create: `apps/aether-gateway/src/backup/schedule.rs`

Create: `apps/aether-gateway/src/backup/scopes.rs`

Modify: `apps/aether-gateway/src/lib.rs`

Test: `apps/aether-gateway/src/backup/config.rs`

Test: `apps/aether-gateway/src/backup/schedule.rs`

- [ ] **Step 1: 写失败测试**

在 `config.rs` 测试配置范围和必填项解析，在 `schedule.rs` 测试小时/天/周/月的到期槽位。

```rust
#[test]
fn parses_minimal_valid_s3_backup_config() {
    let entries = serde_json::json!({
        "backup_s3_enabled": true,
        "backup_s3_scope": "data",
        "backup_s3_endpoint": "https://s3.example.com",
        "backup_s3_region": "auto",
        "backup_s3_bucket": "aether-backups",
        "backup_s3_prefix": "prod/",
        "backup_s3_access_key_id": "access",
        "backup_s3_secret_access_key": "secret",
        "backup_s3_path_style": true,
        "backup_s3_compression": "zstd",
        "backup_s3_schedule_unit": "days",
        "backup_s3_schedule_interval": 1,
        "backup_s3_schedule_hour": 3,
        "backup_s3_schedule_minute": 15,
        "backup_s3_retention_count": 7
    });

    let config = S3BackupConfig::from_json_map(entries.as_object().unwrap())
        .expect("config should parse");

    assert_eq!(config.scope, BackupScope::Data);
    assert_eq!(config.bucket, "aether-backups");
    assert_eq!(config.prefix, "prod/");
    assert_eq!(config.schedule.unit, BackupScheduleUnit::Days);
    assert_eq!(config.retention_count, 7);
}

#[test]
fn rejects_missing_bucket_for_backup() {
    let entries = serde_json::json!({
        "backup_s3_endpoint": "https://s3.example.com",
        "backup_s3_access_key_id": "access",
        "backup_s3_secret_access_key": "secret"
    });

    let err = S3BackupConfig::from_json_map(entries.as_object().unwrap())
        .expect_err("bucket is required");

    assert!(err.to_string().contains("Bucket"));
}

#[test]
fn hourly_schedule_returns_stable_slot_once_per_due_hour() {
    let schedule = BackupSchedule {
        unit: BackupScheduleUnit::Hours,
        interval: 6,
        minute: 10,
        hour: 0,
        weekday: 1,
        month_day: 1,
    };
    let now = chrono::DateTime::parse_from_rfc3339("2026-05-24T12:10:30+08:00")
        .unwrap()
        .with_timezone(&chrono::Utc);

    assert_eq!(
        schedule.due_slot(now).as_deref(),
        Some("hours:2026-05-24T04:10:00Z")
    );
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `rtk cargo test -p aether-gateway backup::`

Expected: FAIL，错误包含 `S3BackupConfig` 或 `BackupSchedule` 未定义。

- [ ] **Step 3: 实现最小代码**

`apps/aether-gateway/src/backup/mod.rs`:

```rust
pub(crate) mod config;
pub(crate) mod schedule;
pub(crate) mod scopes;
```

`config.rs` 定义 `S3BackupConfig`、`BackupConfigError` 和 `from_json_map()`；`schedule.rs` 定义 `BackupSchedule`、`BackupScheduleUnit` 和 `due_slot()`。`due_slot()` 返回字符串槽位，只有当前时间命中配置分钟/小时/星期/月日时才返回 `Some(slot)`，未命中返回 `None`。

`scopes.rs` 先定义最小 `BackupScope` 枚举和配置值解析：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupScope {
    Config,
    Users,
    Data,
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `rtk cargo test -p aether-gateway backup::`

Expected: PASS。

- [ ] **Step 5: 提交**

Run:

```bash
rtk git add apps/aether-gateway/src/backup apps/aether-gateway/src/lib.rs
rtk git commit -m "feat: parse s3 backup config and schedule"
```

### Task 3: 备份范围和对象命名

**Files:**

Modify: `apps/aether-gateway/src/backup/mod.rs`

Modify: `apps/aether-gateway/src/backup/scopes.rs`

Test: `apps/aether-gateway/src/backup/scopes.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn backup_scope_matches_export_routes_and_object_prefixes() {
    assert_eq!(BackupScope::Config.route_kind(), "config_export");
    assert_eq!(BackupScope::Users.route_kind(), "users_export");
    assert_eq!(BackupScope::Data.route_kind(), "data_export");

    assert_eq!(
        BackupScope::Config.object_key("prod/", "20260524-031500"),
        "prod/aether-config-backup-20260524-031500.json.zst"
    );
    assert_eq!(
        BackupScope::Users.object_key("prod/", "20260524-031500"),
        "prod/aether-users-backup-20260524-031500.json.zst"
    );
    assert_eq!(
        BackupScope::Data.object_key("prod/", "20260524-031500"),
        "prod/aether-data-backup-20260524-031500.json.zst"
    );
}

#[test]
fn retention_filter_only_matches_same_scope() {
    let keys = vec![
        "prod/aether-config-backup-20260524-010000.json.zst".to_string(),
        "prod/aether-users-backup-20260524-010000.json.zst".to_string(),
        "prod/aether-data-backup-20260524-010000.json.zst".to_string(),
        "prod/random.json.zst".to_string(),
    ];

    let matched = BackupScope::Users.matching_backup_keys("prod/", keys);

    assert_eq!(
        matched,
        vec!["prod/aether-users-backup-20260524-010000.json.zst"]
    );
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `rtk cargo test -p aether-gateway backup::scopes`

Expected: FAIL，错误包含 `route_kind`、`object_key` 或 `matching_backup_keys` 未定义。

- [ ] **Step 3: 实现最小代码**

在已有 `BackupScope` 上补齐 `from_config_value()`、`as_config_value()`、`route_kind()`、`file_stem()`、`object_key()`、`matching_backup_keys()`。合法配置值使用 `config`、`users`、`data`。

- [ ] **Step 4: 运行测试确认通过**

Run: `rtk cargo test -p aether-gateway backup::scopes`

Expected: PASS。

- [ ] **Step 5: 提交**

Run:

```bash
rtk git add apps/aether-gateway/src/backup/scopes.rs apps/aether-gateway/src/backup/mod.rs
rtk git commit -m "feat: define s3 backup scopes"
```

### Task 4: 对象存储 trait、fake client 和 `object_store` 适配器

**Files:**

Modify: `Cargo.toml`

Modify: `apps/aether-gateway/Cargo.toml`

Create: `apps/aether-gateway/src/backup/store.rs`

Modify: `apps/aether-gateway/src/backup/mod.rs`

Test: `apps/aether-gateway/src/backup/store.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn fake_backup_object_store_puts_lists_and_deletes() {
    let store = FakeBackupObjectStore::default();
    store.put_object("prod/aether-data-backup-20260524-010000.json.zst", bytes::Bytes::from_static(b"one")).await.unwrap();
    store.put_object("prod/aether-data-backup-20260524-020000.json.zst", bytes::Bytes::from_static(b"two")).await.unwrap();

    let keys = store.list_keys("prod/").await.unwrap();
    assert_eq!(keys.len(), 2);

    store.delete_object("prod/aether-data-backup-20260524-010000.json.zst").await.unwrap();
    let keys = store.list_keys("prod/").await.unwrap();
    assert_eq!(keys, vec!["prod/aether-data-backup-20260524-020000.json.zst"]);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `rtk cargo test -p aether-gateway backup::store`

Expected: FAIL，错误包含 `FakeBackupObjectStore` 未定义。

- [ ] **Step 3: 实现最小代码**

根 `Cargo.toml` workspace dependencies 增加：

```toml
object_store = { version = "0.12", default-features = false, features = ["aws"] }
```

`apps/aether-gateway/Cargo.toml` 增加：

```toml
object_store.workspace = true
```

`store.rs` 定义：

```rust
#[async_trait::async_trait]
pub(crate) trait BackupObjectStore: Send + Sync {
    async fn put_object(&self, key: &str, bytes: bytes::Bytes) -> Result<(), BackupStoreError>;
    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, BackupStoreError>;
    async fn delete_object(&self, key: &str) -> Result<(), BackupStoreError>;
}
```

实现 `FakeBackupObjectStore` 用于测试。实现 `ObjectStoreS3BackupStore` 时用 `object_store::aws::AmazonS3Builder`，从 `S3BackupConfig` 填 endpoint、region、bucket、access key、secret、path style。

- [ ] **Step 4: 运行测试确认通过**

Run: `rtk cargo test -p aether-gateway backup::store`

Expected: PASS。

- [ ] **Step 5: 提交**

Run:

```bash
rtk git add Cargo.toml apps/aether-gateway/Cargo.toml apps/aether-gateway/src/backup/store.rs apps/aether-gateway/src/backup/mod.rs
rtk git commit -m "feat: add s3 backup object store"
```

### Task 5: 一次备份执行器和保留清理

**Files:**

Create: `apps/aether-gateway/src/backup/executor.rs`

Modify: `apps/aether-gateway/src/backup/mod.rs`

Test: `apps/aether-gateway/src/backup/executor.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn backup_executor_uploads_payload_and_prunes_same_scope_only() {
    let store = FakeBackupObjectStore::default();
    store.put_object("prod/aether-data-backup-20260524-010000.json.zst", bytes::Bytes::from_static(b"old")).await.unwrap();
    store.put_object("prod/aether-config-backup-20260524-010000.json.zst", bytes::Bytes::from_static(b"keep-config")).await.unwrap();

    let config = sample_backup_config(BackupScope::Data, 1);
    let payload = serde_json::json!({
        "version": "1.0",
        "exported_at": "2026-05-24T03:15:00Z",
        "config_data": {},
        "user_data": {}
    });

    let result = run_backup_with_store(
        &config,
        &store,
        payload,
        chrono::DateTime::parse_from_rfc3339("2026-05-24T03:15:00+08:00").unwrap().with_timezone(&chrono::Utc),
    )
    .await
    .expect("backup should succeed");

    assert_eq!(result.scope, BackupScope::Data);
    assert!(result.object_key.ends_with("aether-data-backup-20260523-191500.json.zst"));
    assert_eq!(result.deleted_old_objects, 1);

    let keys = store.list_keys("prod/").await.unwrap();
    assert!(keys.iter().any(|key| key.contains("aether-config-backup")));
    assert!(!keys.iter().any(|key| key.ends_with("010000.json.zst") && key.contains("aether-data")));
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `rtk cargo test -p aether-gateway backup::executor`

Expected: FAIL，错误包含 `run_backup_with_store` 未定义。

- [ ] **Step 3: 实现最小代码**

实现 `BackupRunResult`，字段包含 `scope`、`bucket`、`object_key`、`bytes`、`sha256`、`export_version`、`exported_at`、`compression`、`deleted_old_objects`。实现 `run_backup_with_store()`：序列化 JSON，使用 `zstd::stream::encode_all()` 压缩，计算 sha256，上传，列出同 prefix 对象，按范围过滤，按文件名倒序保留 `retention_count` 个，其余删除。

- [ ] **Step 4: 运行测试确认通过**

Run: `rtk cargo test -p aether-gateway backup::executor`

Expected: PASS。

- [ ] **Step 5: 提交**

Run:

```bash
rtk git add apps/aether-gateway/src/backup/executor.rs apps/aether-gateway/src/backup/mod.rs
rtk git commit -m "feat: execute s3 backup uploads"
```

### Task 6: 手动备份接口和任务记录

**Files:**

Modify: `apps/aether-gateway/src/task_runtime/mod.rs`

Modify: `apps/aether-gateway/src/control/route/admin/system_families.rs`

Modify: `apps/aether-gateway/src/control/tests/admin_core.rs`

Modify: `apps/aether-gateway/src/handlers/admin/system/core/system_routes.rs`

Create: `apps/aether-gateway/src/backup/task.rs`

Test: `apps/aether-gateway/src/control/tests/admin_core.rs`

Test: `apps/aether-gateway/src/tests/control/admin/system.rs`

- [ ] **Step 1: 写失败测试**

在 route 测试里增加：

```rust
#[test]
fn classifies_admin_system_s3_backup_start_as_admin_proxy_route() {
    let headers = headers(&[]);
    let uri: Uri = "/api/admin/system/backups/s3/run".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("admin_proxy"));
    assert_eq!(decision.route_family.as_deref(), Some("system_manage"));
    assert_eq!(decision.route_kind.as_deref(), Some("s3_backup_run"));
    assert_eq!(decision.auth_endpoint_signature.as_deref(), Some("admin:system"));
}
```

在 admin system handler 测试里增加一个未配置时返回明确错误的测试：

```rust
#[tokio::test]
async fn gateway_rejects_s3_backup_run_when_storage_config_missing() {
    let upstream = Router::new().route(
        "/api/admin/system/backups/s3/run",
        any(|_request: Request| async { (StatusCode::OK, Body::from("unexpected upstream hit")) }),
    );
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/backups/s3/run"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], json!("S3 备份配置缺少 Bucket"));

    gateway_handle.abort();
    upstream_handle.abort();
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `rtk cargo test -p aether-gateway s3_backup`

Expected: FAIL，路由无法分类或 handler 没有实现。

- [ ] **Step 3: 实现最小代码**

`task_runtime/mod.rs` 增加：

```rust
pub(crate) const TASK_KEY_SYSTEM_S3_BACKUP: &str = "system.s3.backup";
```

并在 `TASK_DEFINITIONS` 注册为 `TaskKind::Scheduled`，这样同一个 key 可以记录周期 worker，也可以让手动 run 的 `trigger` 写 `manual`。

`system_families.rs` 增加 `POST /api/admin/system/backups/s3/run` 的 `s3_backup_run` 分类。`system_routes.rs` 在 system handler 中调用 `crate::backup::task::start_s3_backup_task(state.cloned_app(), "manual").await`，返回 `{ "message": "...", "task": ... }` 并写审计。

`backup/task.rs` 创建 background task run，异步执行：读取配置，按 scope 调用 `build_admin_system_config_export_payload()`、`build_admin_system_users_export_payload()` 或 `build_admin_system_data_export_payload()`，再调用 executor。运行中更新状态和事件。

- [ ] **Step 4: 运行测试确认通过**

Run: `rtk cargo test -p aether-gateway s3_backup`

Expected: PASS。

- [ ] **Step 5: 提交**

Run:

```bash
rtk git add apps/aether-gateway/src/task_runtime/mod.rs apps/aether-gateway/src/control/route/admin/system_families.rs apps/aether-gateway/src/control/tests/admin_core.rs apps/aether-gateway/src/handlers/admin/system/core/system_routes.rs apps/aether-gateway/src/backup/task.rs
rtk git commit -m "feat: add manual s3 backup task"
```

### Task 7: 周期 worker 和重复触发保护

**Files:**

Create: `apps/aether-gateway/src/backup/worker.rs`

Modify: `apps/aether-gateway/src/backup/mod.rs`

Modify: `apps/aether-gateway/src/state/core.rs`

Test: `apps/aether-gateway/src/backup/worker.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn backup_worker_skips_already_recorded_slot() {
    let schedule = BackupSchedule {
        unit: BackupScheduleUnit::Days,
        interval: 1,
        minute: 0,
        hour: 3,
        weekday: 1,
        month_day: 1,
    };
    let now = chrono::DateTime::parse_from_rfc3339("2026-05-24T03:00:30+08:00")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let slot = schedule.due_slot(now).expect("slot should be due");

    assert!(should_start_scheduled_backup(Some("days:2026-05-23T19:00:00Z"), &slot));
    assert!(!should_start_scheduled_backup(Some(&slot), &slot));
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `rtk cargo test -p aether-gateway backup::worker`

Expected: FAIL，错误包含 `should_start_scheduled_backup` 未定义。

- [ ] **Step 3: 实现最小代码**

`worker.rs` 实现 `should_start_scheduled_backup()` 和 `spawn_s3_backup_worker(app: AppState) -> Option<JoinHandle<()>>`。worker 每 60 秒读取配置；未启用自动备份时跳过；到期且 `backup_s3_last_slot` 不等于当前槽位时，先写入当前槽位，再启动 `start_s3_backup_task(app.clone(), "scheduled")`。如果启动失败，记录 tracing error；不回滚 slot，避免失败时每分钟重复打爆 S3。

- [ ] **Step 4: 运行测试确认通过**

Run: `rtk cargo test -p aether-gateway backup::worker`

Expected: PASS。

- [ ] **Step 5: 接入 `spawn_background_tasks()` 并提交**

在 `state/core.rs` 的 `spawn_background_tasks()` 增加：

```rust
supervise_worker(
    crate::task_runtime::TASK_KEY_SYSTEM_S3_BACKUP,
    crate::backup::worker::spawn_s3_backup_worker(self.clone()),
);
```

Run:

```bash
rtk git add apps/aether-gateway/src/backup/worker.rs apps/aether-gateway/src/backup/mod.rs apps/aether-gateway/src/state/core.rs
rtk git commit -m "feat: schedule s3 backups"
```

### Task 8: 前端 API、配置 composable 和 S3 备份区域

**Files:**

Modify: `frontend/src/api/admin.ts`

Create: `frontend/src/views/admin/system-settings/composables/useS3BackupConfig.ts`

Create: `frontend/src/views/admin/system-settings/S3BackupSection.vue`

Modify: `frontend/src/views/admin/SystemSettings.vue`

Test: `frontend/src/views/admin/system-settings/composables/useS3BackupConfig.spec.ts`

- [ ] **Step 1: 写失败测试**

新增 composable 测试，覆盖默认范围、Secret 不回显、保存 payload 和高级字段。

```ts
import { describe, expect, it, vi } from 'vitest'
import { useS3BackupConfig } from './useS3BackupConfig'
import { adminApi } from '@/api/admin'

vi.mock('@/api/admin', () => ({
  adminApi: {
    getSystemConfig: vi.fn(),
    updateSystemConfig: vi.fn(),
    runS3Backup: vi.fn(),
  },
}))

describe('useS3BackupConfig', () => {
  it('loads write-only secret as configured without exposing the value', async () => {
    vi.mocked(adminApi.getSystemConfig).mockImplementation(async (key: string) => {
      if (key === 'backup_s3_secret_access_key') {
        return { key, value: null, is_set: true }
      }
      if (key === 'backup_s3_scope') {
        return { key, value: 'data' }
      }
      return { key, value: null }
    })

    const backup = useS3BackupConfig()
    await backup.loadS3BackupConfig()

    expect(backup.config.value.scope).toBe('data')
    expect(backup.config.value.secretAccessKey).toBe('')
    expect(backup.config.value.secretAccessKeyIsSet).toBe(true)
  })
})
```

- [ ] **Step 2: 运行测试确认失败**

Run: `rtk npm --prefix frontend test -- useS3BackupConfig`

Expected: FAIL，文件或 composable 不存在。

- [ ] **Step 3: 实现 API 和 composable**

`admin.ts` 增加：

```ts
export type S3BackupScope = 'config' | 'users' | 'data'

export interface S3BackupRunResponse {
  message: string
  task: {
    id: string
    task_key: string
    status: string
    progress_message?: string
  }
}
```

并在 `adminApi` 增加：

```ts
async runS3Backup(): Promise<S3BackupRunResponse> {
  const response = await apiClient.post<S3BackupRunResponse>('/api/admin/system/backups/s3/run')
  return response.data
}
```

`useS3BackupConfig.ts` 读取所有 `backup_s3_*` key，保存时只在 `secretAccessKey.trim()` 非空时写 `backup_s3_secret_access_key`，清空时写空字符串。

- [ ] **Step 4: 实现 Vue 区域**

`S3BackupSection.vue` 使用现有 `CardSection`、`Input`、`Button`、`Select`、`Switch`。核心区域展示：启用自动备份、备份范围、Bucket、Endpoint、Access Key ID、Secret Access Key、周期单位和间隔、最多保留份数、保存、立即备份。高级区域用一个文本按钮切换展开，展示 Region、Prefix、Path Style、压缩格式和周期锚点字段。

- [ ] **Step 5: 接入系统设置页**

在 `SystemSettings.vue` 引入 `S3BackupSection`，放在 `DataManagementSection` 后面或内部附近。TOC 增加 `section-s3-backup`，onMounted 的配置加载中加入 `loadS3BackupConfig()`。

- [ ] **Step 6: 运行测试确认通过**

Run: `rtk npm --prefix frontend test -- useS3BackupConfig`

Expected: PASS。

- [ ] **Step 7: 提交**

Run:

```bash
rtk git add frontend/src/api/admin.ts frontend/src/views/admin/system-settings/composables/useS3BackupConfig.ts frontend/src/views/admin/system-settings/S3BackupSection.vue frontend/src/views/admin/SystemSettings.vue frontend/src/views/admin/system-settings/composables/useS3BackupConfig.spec.ts
rtk git commit -m "feat: add s3 backup settings UI"
```

### Task 9: 集成验证和收尾

**Files:**

Modify: only files already touched in Tasks 1-8 when verification exposes compile, type, or integration errors.

- [ ] **Step 1: 后端格式化和目标测试**

Run:

```bash
rtk cargo fmt --check
rtk cargo test -p aether-admin s3_backup --lib
rtk cargo test -p aether-gateway backup::
rtk cargo test -p aether-gateway classifies_admin_system_s3_backup_start_as_admin_proxy_route
```

Expected: all PASS。

- [ ] **Step 2: 前端目标测试**

Run:

```bash
rtk npm --prefix frontend test -- useS3BackupConfig
```

Expected: PASS。

- [ ] **Step 3: 构建检查**

Run:

```bash
rtk cargo check -p aether-gateway
rtk npm --prefix frontend run typecheck
```

Expected: both PASS。

- [ ] **Step 4: 代码审查自查**

检查这些点：S3 Secret 不回显；备份范围和三个导出一致；保留清理只按当前范围命名删除；手动备份不要求自动备份启用；自动备份保存 last slot 避免重复触发；没有本地临时备份文件残留。

- [ ] **Step 5: 最终提交**

如果 Step 1-4 有修复，提交：

```bash
rtk git add .
rtk git commit -m "fix: verify s3 backup integration"
```
