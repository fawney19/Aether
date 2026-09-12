use crate::{DataLayerError, PostgresPoolConfig};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use sqlx::PgPool;
use std::str::FromStr;
use std::time::Duration;

const STATEMENT_TIMEOUT_ENV: &str = "AETHER_GATEWAY_DATA_POSTGRES_STATEMENT_TIMEOUT_MS";
const LOCK_TIMEOUT_ENV: &str = "AETHER_GATEWAY_DATA_POSTGRES_LOCK_TIMEOUT_MS";

#[derive(Debug, Clone, Copy)]
struct PostgresSessionTimeouts {
    statement_ms: u32,
    lock_ms: u32,
}

impl PostgresSessionTimeouts {
    fn from_env() -> Result<Self, DataLayerError> {
        Ok(Self {
            statement_ms: read_timeout_env(STATEMENT_TIMEOUT_ENV, 30_000)?,
            lock_ms: read_timeout_env(LOCK_TIMEOUT_ENV, 3_000)?,
        })
    }

    fn apply(self, options: PgConnectOptions) -> PgConnectOptions {
        options.options([
            ("statement_timeout", self.statement_ms),
            ("lock_timeout", self.lock_ms),
        ])
    }
}

fn parse_timeout_ms(name: &str, value: &str) -> Result<u32, DataLayerError> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value <= i32::MAX as u32)
        .ok_or_else(|| {
            DataLayerError::InvalidConfiguration(format!(
                "{name} must be milliseconds in 0..=2147483647 (0 disables the timeout)"
            ))
        })
}

fn read_timeout_env(name: &str, default: u32) -> Result<u32, DataLayerError> {
    match std::env::var(name) {
        Ok(value) => parse_timeout_ms(name, &value),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(DataLayerError::InvalidConfiguration(
            format!("{name} must contain a valid integer"),
        )),
    }
}

/// Migration and historical backfill connections are discarded on every exit path,
/// including cancellation, so their relaxed deadlines cannot escape into request work.
pub async fn acquire_postgres_migration_connection(
    pool: &PgPool,
) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    conn.close_on_drop();
    sqlx::query("SET statement_timeout = 0")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SET lock_timeout = 0")
        .execute(&mut *conn)
        .await?;
    Ok(conn)
}

fn connect_options(config: &PostgresPoolConfig) -> Result<PgConnectOptions, DataLayerError> {
    config.validate()?;
    let options = PgConnectOptions::from_str(config.database_url.trim()).map_err(|err| {
        DataLayerError::InvalidConfiguration(format!("invalid postgres database_url: {err}"))
    })?;

    // Preserve an explicit verification mode from the URL. `require_ssl` is
    // a minimum transport guarantee, so it may upgrade Disable/Allow/Prefer
    // to Require but must never silently weaken VerifyCa/VerifyFull.
    let ssl_mode = if config.require_ssl
        && !matches!(
            options.get_ssl_mode(),
            PgSslMode::VerifyCa | PgSslMode::VerifyFull
        ) {
        PgSslMode::Require
    } else {
        options.get_ssl_mode()
    };

    Ok(options
        .ssl_mode(ssl_mode)
        .statement_cache_capacity(config.statement_cache_capacity))
}

pub type PostgresPool = PgPool;

#[derive(Debug, Clone)]
pub struct PostgresPoolFactory {
    config: PostgresPoolConfig,
    timeouts: PostgresSessionTimeouts,
}

impl PostgresPoolFactory {
    pub fn new(config: PostgresPoolConfig) -> Result<Self, DataLayerError> {
        config.validate()?;
        Ok(Self {
            config,
            timeouts: PostgresSessionTimeouts::from_env()?,
        })
    }

    pub fn config(&self) -> &PostgresPoolConfig {
        &self.config
    }

    pub fn connect_lazy(&self) -> Result<PostgresPool, DataLayerError> {
        let options = self.timeouts.apply(connect_options(&self.config)?);
        Ok(PgPoolOptions::new()
            .min_connections(self.config.min_connections)
            .max_connections(self.config.max_connections)
            .acquire_timeout(Duration::from_millis(self.config.acquire_timeout_ms))
            .idle_timeout(Duration::from_millis(self.config.idle_timeout_ms))
            .max_lifetime(Duration::from_millis(self.config.max_lifetime_ms))
            .connect_lazy_with(options))
    }
}

#[cfg(test)]
mod tests {
    use super::{connect_options, parse_timeout_ms, PostgresPoolFactory, PostgresSessionTimeouts};
    use crate::PostgresPoolConfig;
    use sqlx::postgres::PgSslMode;

    #[tokio::test]
    async fn migration_connection_future_is_send() {
        fn assert_send(_: impl Send) {}

        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/aether")
            .unwrap();
        assert_send(super::acquire_postgres_migration_connection(&pool));
    }

    #[test]
    fn validates_session_timeout_milliseconds() {
        assert_eq!(parse_timeout_ms("timeout", "0").unwrap(), 0);
        assert_eq!(parse_timeout_ms("timeout", " 3000 ").unwrap(), 3_000);
        assert_eq!(
            parse_timeout_ms("timeout", "2147483647").unwrap(),
            i32::MAX as u32
        );
        for invalid in ["", "-1", "3s", "2147483648", "4294967296"] {
            assert!(parse_timeout_ms("timeout", invalid).is_err());
        }
    }

    #[test]
    fn session_deadlines_preserve_unrelated_connection_options() {
        let options = PostgresSessionTimeouts {
            statement_ms: 30_000,
            lock_ms: 3_000,
        }
        .apply(sqlx::postgres::PgConnectOptions::new().options([("search_path", "audit")]));
        assert_eq!(
            options.get_options(),
            Some("-c search_path=audit -c statement_timeout=30000 -c lock_timeout=3000")
        );
    }

    #[tokio::test]
    #[ignore = "requires an isolated AETHER_TEST_DATABASE_URL"]
    async fn live_session_deadlines_rollback_transactions_and_isolate_migration_overrides() {
        use crate::error::SqlxResultExt;
        use crate::{PostgresTransactionOptions, PostgresTransactionRunner};

        let factory = PostgresPoolFactory {
            config: PostgresPoolConfig {
                database_url: std::env::var("AETHER_TEST_DATABASE_URL").expect("test database URL"),
                min_connections: 0,
                max_connections: 2,
                ..PostgresPoolConfig::default()
            },
            timeouts: PostgresSessionTimeouts {
                statement_ms: 100,
                lock_ms: 40,
            },
        };
        let pool = factory.connect_lazy().unwrap();
        let table = format!("deadline_test_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!(
            "CREATE TABLE {table} (id INTEGER PRIMARY KEY, value INTEGER NOT NULL)"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(&format!("INSERT INTO {table} VALUES (1, 0)"))
            .execute(&pool)
            .await
            .unwrap();
        let mut blocker = pool.begin().await.unwrap();
        sqlx::query(&format!("UPDATE {table} SET value = 7 WHERE id = 1"))
            .execute(&mut *blocker)
            .await
            .unwrap();
        let runner = PostgresTransactionRunner::new(pool.clone());
        let insert = format!("INSERT INTO {table} VALUES (2, 2)");
        let update = format!("UPDATE {table} SET value = 9 WHERE id = 1");
        let started = std::time::Instant::now();
        let error = runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    sqlx::query(&insert)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    sqlx::query(&update)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    Ok(())
                })
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("SQLSTATE 55P03"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        blocker.rollback().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i32>(&format!("SELECT value FROM {table} WHERE id = 1"))
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );

        let error = sqlx::query("SELECT pg_sleep(0.3)")
            .execute(&pool)
            .await
            .map_postgres_err()
            .unwrap_err();
        assert!(error.to_string().contains("SQLSTATE 57014"), "{error}");
        runner
            .run(
                PostgresTransactionOptions {
                    statement_timeout_ms: Some(1_000),
                    ..PostgresTransactionOptions::read_write()
                },
                |tx| {
                    Box::pin(async move {
                        sqlx::query("SELECT pg_sleep(0.15)")
                            .execute(&mut **tx)
                            .await
                            .map_postgres_err()?;
                        Ok(())
                    })
                },
            )
            .await
            .unwrap();

        let mut migration = super::acquire_postgres_migration_connection(&pool)
            .await
            .unwrap();
        sqlx::query("SELECT pg_sleep(0.15)")
            .execute(&mut *migration)
            .await
            .unwrap();
        drop(migration);
        for _ in 0..2 {
            let configured: i64 = sqlx::query_scalar(
                "SELECT setting::BIGINT FROM pg_settings WHERE name = 'statement_timeout'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                configured, 100,
                "relaxed/local overrides must not leak into pooled requests"
            );
        }
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    fn ssl_mode(url: &str, require_ssl: bool) -> PgSslMode {
        connect_options(&PostgresPoolConfig {
            database_url: url.to_string(),
            require_ssl,
            ..PostgresPoolConfig::default()
        })
        .expect("postgres options should parse")
        .get_ssl_mode()
    }

    #[test]
    fn preserves_explicit_postgres_verification_modes() {
        assert!(matches!(
            ssl_mode("postgres://localhost/aether?sslmode=verify-full", false),
            PgSslMode::VerifyFull
        ));
        assert!(matches!(
            ssl_mode("postgres://localhost/aether?sslmode=verify-ca", true),
            PgSslMode::VerifyCa
        ));
    }

    #[test]
    fn require_ssl_only_upgrades_weak_postgres_modes() {
        for mode in ["disable", "allow", "prefer"] {
            let url = format!("postgres://localhost/aether?sslmode={mode}");
            assert!(matches!(ssl_mode(&url, true), PgSslMode::Require));
        }
        assert!(matches!(
            ssl_mode("postgres://localhost/aether", false),
            PgSslMode::Prefer
        ));
    }

    #[tokio::test]
    async fn factory_builds_lazy_pool_from_valid_config() {
        let config = PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        };

        let factory = PostgresPoolFactory::new(config).expect("factory should build");
        let _pool = factory.connect_lazy().expect("lazy pool should build");
    }
}
