pub use aether_data_contracts::DataLayerError;

#[cfg(feature = "postgres")]
pub(crate) fn postgres_error(error: impl std::fmt::Display) -> DataLayerError {
    DataLayerError::postgres(error)
}

pub(crate) fn sql_error(error: impl std::fmt::Display) -> DataLayerError {
    DataLayerError::sql(error)
}

#[cfg(feature = "postgres")]
pub(crate) trait SqlxResultExt<T> {
    fn map_postgres_err(self) -> Result<T, DataLayerError>;
}

#[cfg(feature = "postgres")]
impl<T> SqlxResultExt<T> for Result<T, sqlx::Error> {
    fn map_postgres_err(self) -> Result<T, DataLayerError> {
        self.map_err(postgres_error)
    }
}

pub(crate) trait SqlResultExt<T> {
    fn map_sql_err(self) -> Result<T, DataLayerError>;
}

impl<T> SqlResultExt<T> for Result<T, sqlx::Error> {
    fn map_sql_err(self) -> Result<T, DataLayerError> {
        self.map_err(sql_error)
    }
}
