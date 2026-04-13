use crate::error::SqlxResultExt;
use crate::postgres::{DatabaseRecordId, PostgresTransactionOptions, PostgresTransactionRunner};
use crate::DataLayerError;
use futures_util::{FutureExt, TryStreamExt};
use sqlx::query_scalar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostgresLeaseClaimOptions {
    pub batch_size: usize,
    pub lease_ms: u64,
}

impl PostgresLeaseClaimOptions {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        if self.batch_size == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "postgres lease batch_size must be positive".to_string(),
            ));
        }
        if self.lease_ms == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "postgres lease lease_ms must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresLeaseClaimSpec {
    pub table: &'static str,
    pub id_column: &'static str,
    pub lease_owner_column: &'static str,
    pub lease_expires_at_column: &'static str,
    pub eligibility_predicate_sql: &'static str,
    pub order_by_sql: &'static str,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PostgresLeaseRunnerConfig {
    pub statement_timeout_ms: Option<u64>,
    pub lock_timeout_ms: Option<u64>,
}

impl PostgresLeaseRunnerConfig {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        if matches!(self.statement_timeout_ms, Some(0)) {
            return Err(DataLayerError::InvalidConfiguration(
                "postgres lease statement_timeout_ms must be positive".to_string(),
            ));
        }
        if matches!(self.lock_timeout_ms, Some(0)) {
            return Err(DataLayerError::InvalidConfiguration(
                "postgres lease lock_timeout_ms must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PostgresLeaseRunner {
    transaction_runner: PostgresTransactionRunner,
    config: PostgresLeaseRunnerConfig,
}

impl PostgresLeaseRunner {
    pub fn new(
        transaction_runner: PostgresTransactionRunner,
        config: PostgresLeaseRunnerConfig,
    ) -> Result<Self, DataLayerError> {
        config.validate()?;
        Ok(Self {
            transaction_runner,
            config,
        })
    }

    pub fn config(&self) -> PostgresLeaseRunnerConfig {
        self.config
    }

    pub fn transaction_runner(&self) -> &PostgresTransactionRunner {
        &self.transaction_runner
    }

    pub async fn claim_ids(
        &self,
        spec: &PostgresLeaseClaimSpec,
        options: PostgresLeaseClaimOptions,
        owner: &str,
    ) -> Result<Vec<DatabaseRecordId>, DataLayerError> {
        validate_lease_owner(owner)?;
        let sql = build_postgres_lease_claim_sql(spec, options)?;
        let owner = owner.trim().to_string();
        let lease_ms = i64::try_from(options.lease_ms).map_err(|_| {
            DataLayerError::InvalidInput("postgres lease lease_ms exceeds i64 range".to_string())
        })?;
        let tx_options = PostgresTransactionOptions {
            statement_timeout_ms: self.config.statement_timeout_ms,
            lock_timeout_ms: self.config.lock_timeout_ms,
            ..PostgresTransactionOptions::read_write()
        };

        self.transaction_runner
            .run(tx_options, |tx| {
                async move {
                    let mut rows = query_scalar::<_, String>(&sql)
                        .bind(owner)
                        .bind(lease_ms)
                        .fetch(&mut **tx);
                    let mut ids = Vec::new();
                    while let Some(id) = rows.try_next().await.map_postgres_err()? {
                        ids.push(DatabaseRecordId(id));
                    }
                    Ok(ids)
                }
                .boxed()
            })
            .await
    }

    pub async fn release_ids(
        &self,
        spec: &PostgresLeaseClaimSpec,
        ids: &[DatabaseRecordId],
        owner: &str,
    ) -> Result<Vec<DatabaseRecordId>, DataLayerError> {
        validate_lease_owner(owner)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = build_postgres_lease_release_sql(spec)?;
        let owner = owner.trim().to_string();
        let ids = ids.iter().map(|id| id.0.clone()).collect::<Vec<_>>();
        let tx_options = PostgresTransactionOptions {
            statement_timeout_ms: self.config.statement_timeout_ms,
            lock_timeout_ms: self.config.lock_timeout_ms,
            ..PostgresTransactionOptions::read_write()
        };

        self.transaction_runner
            .run(tx_options, |tx| {
                async move {
                    let mut rows = query_scalar::<_, String>(&sql)
                        .bind(ids)
                        .bind(owner)
                        .fetch(&mut **tx);
                    let mut released = Vec::new();
                    while let Some(id) = rows.try_next().await.map_postgres_err()? {
                        released.push(DatabaseRecordId(id));
                    }
                    Ok(released)
                }
                .boxed()
            })
            .await
    }

    pub async fn renew_ids(
        &self,
        spec: &PostgresLeaseClaimSpec,
        ids: &[DatabaseRecordId],
        owner: &str,
        lease_ms: u64,
    ) -> Result<Vec<DatabaseRecordId>, DataLayerError> {
        validate_lease_owner(owner)?;
        if lease_ms == 0 {
            return Err(DataLayerError::InvalidInput(
                "postgres lease lease_ms must be positive".to_string(),
            ));
        }
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = build_postgres_lease_renew_sql(spec)?;
        let owner = owner.trim().to_string();
        let lease_ms = i64::try_from(lease_ms).map_err(|_| {
            DataLayerError::InvalidInput("postgres lease lease_ms exceeds i64 range".to_string())
        })?;
        let ids = ids.iter().map(|id| id.0.clone()).collect::<Vec<_>>();
        let tx_options = PostgresTransactionOptions {
            statement_timeout_ms: self.config.statement_timeout_ms,
            lock_timeout_ms: self.config.lock_timeout_ms,
            ..PostgresTransactionOptions::read_write()
        };

        self.transaction_runner
            .run(tx_options, |tx| {
                async move {
                    let mut rows = query_scalar::<_, String>(&sql)
                        .bind(ids)
                        .bind(owner)
                        .bind(lease_ms)
                        .fetch(&mut **tx);
                    let mut renewed = Vec::new();
                    while let Some(id) = rows.try_next().await.map_postgres_err()? {
                        renewed.push(DatabaseRecordId(id));
                    }
                    Ok(renewed)
                }
                .boxed()
            })
            .await
    }
}

pub fn build_postgres_lease_claim_sql(
    spec: &PostgresLeaseClaimSpec,
    options: PostgresLeaseClaimOptions,
) -> Result<String, DataLayerError> {
    options.validate()?;
    validate_lease_spec(spec)?;

    Ok(format!(
        "WITH claimable AS (\
         SELECT {id_column} \
         FROM {table} \
         WHERE ({eligibility_predicate_sql}) \
           AND ({lease_expires_at_column} IS NULL OR {lease_expires_at_column} <= NOW()) \
         ORDER BY {order_by_sql} \
         FOR UPDATE SKIP LOCKED \
         LIMIT {batch_size}\
         ) \
         UPDATE {table} AS target \
         SET {lease_owner_column} = $1, \
             {lease_expires_at_column} = NOW() + ($2::bigint * INTERVAL '1 millisecond') \
         FROM claimable \
         WHERE target.{id_column} = claimable.{id_column} \
         RETURNING target.{id_column}",
        id_column = spec.id_column,
        table = spec.table,
        eligibility_predicate_sql = spec.eligibility_predicate_sql,
        lease_expires_at_column = spec.lease_expires_at_column,
        order_by_sql = spec.order_by_sql,
        batch_size = options.batch_size,
        lease_owner_column = spec.lease_owner_column,
    ))
}

pub fn build_postgres_lease_release_sql(
    spec: &PostgresLeaseClaimSpec,
) -> Result<String, DataLayerError> {
    validate_lease_spec(spec)?;

    Ok(format!(
        "UPDATE {table} \
         SET {lease_owner_column} = NULL, \
             {lease_expires_at_column} = NULL \
         WHERE {id_column} = ANY($1) \
           AND {lease_owner_column} = $2 \
         RETURNING {id_column}",
        table = spec.table,
        id_column = spec.id_column,
        lease_owner_column = spec.lease_owner_column,
        lease_expires_at_column = spec.lease_expires_at_column,
    ))
}

pub fn build_postgres_lease_renew_sql(
    spec: &PostgresLeaseClaimSpec,
) -> Result<String, DataLayerError> {
    validate_lease_spec(spec)?;

    Ok(format!(
        "UPDATE {table} \
         SET {lease_expires_at_column} = NOW() + ($3::bigint * INTERVAL '1 millisecond') \
         WHERE {id_column} = ANY($1) \
           AND {lease_owner_column} = $2 \
         RETURNING {id_column}",
        table = spec.table,
        id_column = spec.id_column,
        lease_owner_column = spec.lease_owner_column,
        lease_expires_at_column = spec.lease_expires_at_column,
    ))
}

fn validate_lease_spec(spec: &PostgresLeaseClaimSpec) -> Result<(), DataLayerError> {
    for (field, value) in [
        ("table", spec.table),
        ("id_column", spec.id_column),
        ("lease_owner_column", spec.lease_owner_column),
        ("lease_expires_at_column", spec.lease_expires_at_column),
        ("eligibility_predicate_sql", spec.eligibility_predicate_sql),
        ("order_by_sql", spec.order_by_sql),
    ] {
        if value.trim().is_empty() {
            return Err(DataLayerError::InvalidConfiguration(format!(
                "postgres lease {field} cannot be empty"
            )));
        }
    }
    Ok(())
}

fn validate_lease_owner(owner: &str) -> Result<(), DataLayerError> {
    if owner.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "postgres lease owner cannot be empty".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_postgres_lease_claim_sql, build_postgres_lease_release_sql,
        build_postgres_lease_renew_sql, PostgresLeaseClaimOptions, PostgresLeaseClaimSpec,
        PostgresLeaseRunner, PostgresLeaseRunnerConfig,
    };
    use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory, PostgresTransactionRunner};

    fn sample_spec() -> PostgresLeaseClaimSpec {
        PostgresLeaseClaimSpec {
            table: "video_tasks",
            id_column: "id",
            lease_owner_column: "lease_owner",
            lease_expires_at_column: "lease_expires_at",
            eligibility_predicate_sql: "status IN ('submitted', 'processing')",
            order_by_sql: "updated_at ASC",
        }
    }

    #[test]
    fn builds_skip_locked_claim_sql() {
        let sql = build_postgres_lease_claim_sql(
            &sample_spec(),
            PostgresLeaseClaimOptions {
                batch_size: 16,
                lease_ms: 15_000,
            },
        )
        .expect("claim SQL should build");

        assert!(sql.contains("FOR UPDATE SKIP LOCKED"));
        assert!(sql.contains("LIMIT 16"));
        assert!(sql.contains("lease_owner = $1"));
        assert!(sql.contains("NOW() + ($2::bigint * INTERVAL '1 millisecond')"));
    }

    #[test]
    fn builds_release_sql() {
        let sql =
            build_postgres_lease_release_sql(&sample_spec()).expect("release SQL should build");

        assert!(sql.contains("id = ANY($1)"));
        assert!(sql.contains("lease_owner = $2"));
        assert!(sql.contains("lease_expires_at = NULL"));
    }

    #[test]
    fn builds_renew_sql() {
        let sql = build_postgres_lease_renew_sql(&sample_spec()).expect("renew SQL should build");

        assert!(sql.contains("id = ANY($1)"));
        assert!(sql.contains("lease_owner = $2"));
        assert!(sql.contains("NOW() + ($3::bigint * INTERVAL '1 millisecond')"));
    }

    #[tokio::test]
    async fn lease_runner_reuses_transaction_runner() {
        let factory = PostgresPoolFactory::new(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("factory should build");
        let pool = factory.connect_lazy().expect("lazy pool should build");
        let transaction_runner = PostgresTransactionRunner::new(pool);

        let lease_runner = PostgresLeaseRunner::new(
            transaction_runner.clone(),
            PostgresLeaseRunnerConfig {
                statement_timeout_ms: Some(2_000),
                lock_timeout_ms: Some(500),
            },
        )
        .expect("lease runner should build");

        assert_eq!(
            lease_runner.config(),
            PostgresLeaseRunnerConfig {
                statement_timeout_ms: Some(2_000),
                lock_timeout_ms: Some(500),
            }
        );
        let _runner_ref = lease_runner.transaction_runner();
    }

    #[tokio::test]
    async fn release_and_renew_empty_ids_short_circuit() {
        let factory = PostgresPoolFactory::new(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("factory should build");
        let pool = factory.connect_lazy().expect("lazy pool should build");
        let runner = PostgresLeaseRunner::new(
            PostgresTransactionRunner::new(pool),
            PostgresLeaseRunnerConfig::default(),
        )
        .expect("lease runner should build");

        assert!(runner
            .release_ids(&sample_spec(), &[], "worker-1")
            .await
            .expect("empty release should succeed")
            .is_empty());
        assert!(runner
            .renew_ids(&sample_spec(), &[], "worker-1", 5_000)
            .await
            .expect("empty renew should succeed")
            .is_empty());
    }
}
