use crate::DataLayerError;

pub(crate) trait SqlResultExt<T> {
    fn map_sql_err(self) -> Result<T, DataLayerError>;
}

impl<T> SqlResultExt<T> for Result<T, sqlx::Error> {
    fn map_sql_err(self) -> Result<T, DataLayerError> {
        self.map_err(DataLayerError::sql)
    }
}
