use crate::DataLayerError;

pub(crate) fn postgres_error(error: impl std::fmt::Display) -> DataLayerError {
    DataLayerError::postgres(error)
}

pub(crate) trait SqlxResultExt<T> {
    fn map_postgres_err(self) -> Result<T, DataLayerError>;
}

impl<T> SqlxResultExt<T> for Result<T, sqlx::Error> {
    fn map_postgres_err(self) -> Result<T, DataLayerError> {
        self.map_err(postgres_error)
    }
}
