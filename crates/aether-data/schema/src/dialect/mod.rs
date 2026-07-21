use crate::{Column, DefaultValue, DriverColumnOverride, LogicalType, ReferentialAction};

pub mod mysql;
pub mod postgres;
pub mod sqlite;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
    Mysql,
    Sqlite,
}

impl Dialect {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Sqlite => "sqlite",
        }
    }
}

fn quote_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn default_sql(default: &DefaultValue) -> String {
    match default {
        DefaultValue::String(value) => quote_string(value),
        DefaultValue::Integer(value) => value.to_string(),
        DefaultValue::Bool(value) => {
            if *value {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        DefaultValue::Raw { raw } => raw.clone(),
    }
}

fn default_sql_bool_keywords(default: &DefaultValue) -> String {
    match default {
        DefaultValue::Bool(true) => "true".to_string(),
        DefaultValue::Bool(false) => "false".to_string(),
        _ => default_sql(default),
    }
}

fn column_default<'a>(
    column: &'a Column,
    override_: Option<&'a DriverColumnOverride>,
) -> Option<&'a DefaultValue> {
    override_
        .and_then(|override_| override_.default.as_ref())
        .or(column.default.as_ref())
}

fn column_nullable(column: &Column, override_: Option<&DriverColumnOverride>) -> bool {
    override_
        .and_then(|override_| override_.nullable)
        .unwrap_or(column.nullable)
}

fn override_type(override_: Option<&DriverColumnOverride>) -> Option<&str> {
    override_?.sql_type.as_deref()
}

fn postgres_type(column: &Column) -> String {
    let override_ = column.driver.postgres.as_ref();
    if let Some(sql_type) = override_type(override_) {
        return sql_type.to_string();
    }
    if column.auto_increment {
        return "bigserial".to_string();
    }
    match column.logical_type {
        LogicalType::TextId | LogicalType::Text => match column.length {
            Some(length) => format!("character varying({length})"),
            None => "text".to_string(),
        },
        LogicalType::LongText => "text".to_string(),
        LogicalType::Bool => "boolean".to_string(),
        LogicalType::Int32 => "integer".to_string(),
        LogicalType::Int64 | LogicalType::UnixSeconds | LogicalType::UnixMillis => {
            "bigint".to_string()
        }
        LogicalType::Float64 => "double precision".to_string(),
        LogicalType::DecimalMoney => "numeric".to_string(),
        LogicalType::Timestamp => "timestamp with time zone".to_string(),
        LogicalType::Json => "jsonb".to_string(),
        LogicalType::Bytes => "bytea".to_string(),
    }
}

fn mysql_type(column: &Column) -> String {
    let override_ = column.driver.mysql.as_ref();
    if let Some(sql_type) = override_type(override_) {
        return sql_type.to_string();
    }
    match column.logical_type {
        LogicalType::TextId | LogicalType::Text => match column.length {
            Some(length) => format!("VARCHAR({length})"),
            None => "TEXT".to_string(),
        },
        LogicalType::LongText => "LONGTEXT".to_string(),
        LogicalType::Bool => "TINYINT(1)".to_string(),
        LogicalType::Int32 => "INT".to_string(),
        LogicalType::Int64 | LogicalType::UnixSeconds | LogicalType::UnixMillis => {
            "BIGINT".to_string()
        }
        LogicalType::Float64 => "DOUBLE".to_string(),
        LogicalType::DecimalMoney => "DECIMAL(20,8)".to_string(),
        LogicalType::Timestamp => "BIGINT".to_string(),
        LogicalType::Json => "JSON".to_string(),
        LogicalType::Bytes => "LONGBLOB".to_string(),
    }
}

fn sqlite_type(column: &Column) -> String {
    let override_ = column.driver.sqlite.as_ref();
    if let Some(sql_type) = override_type(override_) {
        return sql_type.to_string();
    }
    match column.logical_type {
        LogicalType::TextId | LogicalType::Text | LogicalType::LongText | LogicalType::Json => {
            "TEXT".to_string()
        }
        LogicalType::Bool
        | LogicalType::Int32
        | LogicalType::Int64
        | LogicalType::UnixSeconds
        | LogicalType::UnixMillis
        | LogicalType::Timestamp => "INTEGER".to_string(),
        LogicalType::Float64 | LogicalType::DecimalMoney => "REAL".to_string(),
        LogicalType::Bytes => "BLOB".to_string(),
    }
}

fn referential_action_sql(action: &ReferentialAction) -> &'static str {
    match action {
        ReferentialAction::Cascade => "CASCADE",
        ReferentialAction::SetNull => "SET NULL",
        ReferentialAction::Restrict => "RESTRICT",
        ReferentialAction::NoAction => "NO ACTION",
    }
}
