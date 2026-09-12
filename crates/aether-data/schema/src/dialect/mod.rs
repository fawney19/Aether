use crate::{Column, DefaultValue, DriverColumnOverride, LogicalType, ReferentialAction};

pub mod postgres;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
}

impl Dialect {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
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
        DefaultValue::Bool(value) => value.to_string(),
        DefaultValue::Raw { raw } => raw.clone(),
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

fn referential_action_sql(action: &ReferentialAction) -> &'static str {
    match action {
        ReferentialAction::Cascade => "CASCADE",
        ReferentialAction::SetNull => "SET NULL",
        ReferentialAction::Restrict => "RESTRICT",
        ReferentialAction::NoAction => "NO ACTION",
    }
}
