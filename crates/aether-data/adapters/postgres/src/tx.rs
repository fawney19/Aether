use futures_util::future::BoxFuture;
use sqlx::{Postgres, Transaction};

use crate::error::{postgres_error, SqlxResultExt};
use crate::DataLayerError;
use crate::PostgresPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransactionMode {
    ReadOnly,
    #[default]
    ReadWrite,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PostgresTransactionOptions {
    pub mode: TransactionMode,
    pub statement_timeout_ms: Option<u64>,
    pub lock_timeout_ms: Option<u64>,
}

impl PostgresTransactionOptions {
    pub fn read_only() -> Self {
        Self {
            mode: TransactionMode::ReadOnly,
            ..Self::default()
        }
    }

    pub fn read_write() -> Self {
        Self {
            mode: TransactionMode::ReadWrite,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), DataLayerError> {
        if matches!(self.statement_timeout_ms, Some(0)) {
            return Err(DataLayerError::InvalidConfiguration(
                "postgres statement_timeout_ms must be positive".to_string(),
            ));
        }
        if matches!(self.lock_timeout_ms, Some(0)) {
            return Err(DataLayerError::InvalidConfiguration(
                "postgres lock_timeout_ms must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

pub type PostgresTransaction = Transaction<'static, Postgres>;

#[derive(Debug, Clone)]
pub struct PostgresTransactionRunner {
    pool: PostgresPool,
}

impl PostgresTransactionRunner {
    pub fn new(pool: PostgresPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PostgresPool {
        &self.pool
    }

    pub async fn begin(
        &self,
        options: PostgresTransactionOptions,
    ) -> Result<PostgresTransaction, DataLayerError> {
        options.validate()?;

        let mut tx = self.pool.begin().await.map_postgres_err()?;
        for statement in build_transaction_setup_statements(options) {
            sqlx::query(statement.as_str())
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        Ok(tx)
    }

    pub async fn run<T, F>(
        &self,
        options: PostgresTransactionOptions,
        f: F,
    ) -> Result<T, DataLayerError>
    where
        F: for<'tx> FnOnce(
            &'tx mut PostgresTransaction,
        ) -> BoxFuture<'tx, Result<T, DataLayerError>>,
    {
        let mut tx = self.begin(options).await?;
        match f(&mut tx).await {
            Ok(value) => {
                tx.commit().await.map_err(postgres_error)?;
                Ok(value)
            }
            Err(err) => {
                let _ = tx.rollback().await;
                Err(err)
            }
        }
    }

    pub async fn run_read_only<T, F>(&self, f: F) -> Result<T, DataLayerError>
    where
        F: for<'tx> FnOnce(
            &'tx mut PostgresTransaction,
        ) -> BoxFuture<'tx, Result<T, DataLayerError>>,
    {
        self.run(PostgresTransactionOptions::read_only(), f).await
    }

    pub async fn run_read_write<T, F>(&self, f: F) -> Result<T, DataLayerError>
    where
        F: for<'tx> FnOnce(
            &'tx mut PostgresTransaction,
        ) -> BoxFuture<'tx, Result<T, DataLayerError>>,
    {
        self.run(PostgresTransactionOptions::read_write(), f).await
    }
}

pub(crate) fn build_transaction_setup_statements(
    options: PostgresTransactionOptions,
) -> Vec<String> {
    let mut statements = Vec::new();
    if options.mode == TransactionMode::ReadOnly {
        statements.push("SET TRANSACTION READ ONLY".to_string());
    }
    if let Some(statement_timeout_ms) = options.statement_timeout_ms {
        statements.push(format!(
            "SET LOCAL statement_timeout = {}",
            statement_timeout_ms
        ));
    }
    if let Some(lock_timeout_ms) = options.lock_timeout_ms {
        statements.push(format!("SET LOCAL lock_timeout = {}", lock_timeout_ms));
    }
    statements
}

#[cfg(test)]
mod tests {
    use super::{
        build_transaction_setup_statements, PostgresTransactionOptions, PostgresTransactionRunner,
        TransactionMode,
    };
    use crate::{PostgresPoolConfig, PostgresPoolFactory};

    #[test]
    fn validates_transaction_options() {
        assert!(PostgresTransactionOptions {
            statement_timeout_ms: Some(0),
            ..PostgresTransactionOptions::default()
        }
        .validate()
        .is_err());
        assert!(PostgresTransactionOptions {
            lock_timeout_ms: Some(0),
            ..PostgresTransactionOptions::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn builds_expected_setup_statements() {
        let statements = build_transaction_setup_statements(PostgresTransactionOptions {
            mode: TransactionMode::ReadOnly,
            statement_timeout_ms: Some(1_500),
            lock_timeout_ms: Some(750),
        });

        assert_eq!(
            statements,
            vec![
                "SET TRANSACTION READ ONLY".to_string(),
                "SET LOCAL statement_timeout = 1500".to_string(),
                "SET LOCAL lock_timeout = 750".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn runner_reuses_lazy_pool() {
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

        let runner = PostgresTransactionRunner::new(pool.clone());

        let _pool_ref = runner.pool();
    }
}
