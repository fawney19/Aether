use aether_data_contracts::repository::users::StoredUserPreferenceRecord;
use aether_data_postgres::SqlxUserReadRepository;
use sqlx::{query, query_scalar, PgPool};

use super::{ManagedPostgresServer, POSTGRES_MIGRATOR};

#[tokio::test]
async fn wallet_overage_preference_upgrade_defaults_off_and_round_trips() {
    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres should start")
    else {
        return;
    };
    let pool = PgPool::connect(server.database_url())
        .await
        .expect("database should connect");
    sqlx::raw_sql(
        r#"
CREATE TABLE providers (id text PRIMARY KEY, name text NOT NULL);
CREATE TABLE user_preferences (
    id text PRIMARY KEY, user_id text UNIQUE NOT NULL,
    avatar_url text, bio text, default_provider_id text,
    theme text NOT NULL DEFAULT 'light', language text NOT NULL DEFAULT 'zh-CN',
    timezone text NOT NULL DEFAULT 'Asia/Shanghai',
    email_notifications boolean NOT NULL DEFAULT true,
    usage_alerts boolean NOT NULL DEFAULT true,
    announcement_notifications boolean NOT NULL DEFAULT true,
    created_at timestamptz DEFAULT now(), updated_at timestamptz DEFAULT now()
);
INSERT INTO user_preferences (id, user_id, theme) VALUES ('legacy-pref', 'legacy-user', 'dark');
"#,
    )
    .execute(&pool)
    .await
    .expect("legacy preferences should seed");
    let migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20261009000000)
        .expect("wallet overage migration should be embedded");
    for _ in 0..2 {
        sqlx::raw_sql(&migration.sql)
            .execute(&pool)
            .await
            .expect("migration should apply idempotently");
    }
    let repository = SqlxUserReadRepository::new(pool.clone());
    let mut preferences = repository
        .read_user_preferences("legacy-user")
        .await
        .expect("legacy preferences should read")
        .expect("legacy preferences should exist");
    assert!(!preferences.allow_wallet_overage);
    assert_eq!(preferences.theme, "dark");
    for enabled in [true, false] {
        preferences.allow_wallet_overage = enabled;
        assert_eq!(
            repository
                .write_user_preferences(&preferences)
                .await
                .expect("preferences should update"),
            Some(preferences.clone())
        );
        assert_eq!(
            repository
                .read_user_preferences("legacy-user")
                .await
                .expect("preferences should read"),
            Some(preferences.clone())
        );
    }
    query("INSERT INTO user_preferences (id, user_id) VALUES ('new-pref', 'new-user')")
        .execute(&pool)
        .await
        .expect("new preferences should insert");
    let default: bool = query_scalar(
        "SELECT allow_wallet_overage FROM user_preferences WHERE user_id = 'new-user'",
    )
    .fetch_one(&pool)
    .await
    .expect("new default should read");
    assert!(!default);
    let mut enabled = StoredUserPreferenceRecord::default_for_user("enabled-user");
    enabled.allow_wallet_overage = true;
    assert_eq!(
        repository
            .write_user_preferences(&enabled)
            .await
            .expect("new enabled preferences should insert"),
        Some(enabled)
    );
    pool.close().await;
}
