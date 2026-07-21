pub mod dialect;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("failed to read schema file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse schema file {path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("failed to write generated schema file {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("schema validation failed: {0}")]
    Validation(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalSchema {
    pub tables: BTreeMap<String, Table>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSchema {
    pub schema: LogicalSchema,
    pub sources: Vec<SchemaSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSource {
    pub name: String,
    pub tables: Vec<String>,
}

impl LogicalSchema {
    pub fn ordered_table_names(&self) -> Vec<String> {
        let mut names = self.tables.keys().cloned().collect::<Vec<_>>();
        names.sort_by(|left, right| {
            let left_table = self
                .tables
                .get(left)
                .expect("ordered table should exist in schema");
            let right_table = self
                .tables
                .get(right)
                .expect("ordered table should exist in schema");
            left_table
                .order
                .unwrap_or(u32::MAX)
                .cmp(&right_table.order.unwrap_or(u32::MAX))
                .then_with(|| left.cmp(right))
        });
        names
    }
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct SchemaFile {
    #[serde(default)]
    pub table: BTreeMap<String, Table>,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Table {
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub order: Option<u32>,
    #[serde(default)]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub primary_key: Vec<String>,
    #[serde(default)]
    pub uniques: Vec<UniqueConstraint>,
    #[serde(default)]
    pub indexes: Vec<Index>,
    #[serde(default)]
    pub foreign_keys: Vec<ForeignKey>,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub logical_type: LogicalType,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub auto_increment: bool,
    #[serde(default)]
    pub default: Option<DefaultValue>,
    #[serde(default)]
    pub length: Option<u32>,
    #[serde(default)]
    pub driver: DriverColumnOverrides,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogicalType {
    TextId,
    Text,
    LongText,
    Bool,
    Int32,
    Int64,
    Float64,
    DecimalMoney,
    UnixSeconds,
    UnixMillis,
    Timestamp,
    Json,
    Bytes,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum DefaultValue {
    String(String),
    Integer(i64),
    Bool(bool),
    Raw { raw: String },
}

#[derive(Debug, Clone, Default, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DriverColumnOverrides {
    #[serde(default)]
    pub postgres: Option<DriverColumnOverride>,
    #[serde(default)]
    pub mysql: Option<DriverColumnOverride>,
    #[serde(default)]
    pub sqlite: Option<DriverColumnOverride>,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DriverColumnOverride {
    #[serde(rename = "type")]
    #[serde(default)]
    pub sql_type: Option<String>,
    #[serde(default)]
    pub default: Option<DefaultValue>,
    #[serde(default)]
    pub nullable: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UniqueConstraint {
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    #[serde(default)]
    pub unique: bool,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub references_table: String,
    pub references_columns: Vec<String>,
    #[serde(default)]
    pub on_delete: Option<ReferentialAction>,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReferentialAction {
    Cascade,
    SetNull,
    Restrict,
    NoAction,
}

pub fn load_schema_dir(root: impl AsRef<Path>) -> Result<LogicalSchema, SchemaError> {
    Ok(load_schema_sources(root)?.schema)
}

pub fn load_schema_sources(root: impl AsRef<Path>) -> Result<LoadedSchema, SchemaError> {
    let mut paths = fs::read_dir(root.as_ref())
        .map_err(|source| SchemaError::Read {
            path: root.as_ref().to_path_buf(),
            source,
        })?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| SchemaError::Read {
            path: root.as_ref().to_path_buf(),
            source,
        })?;
    paths.sort();
    paths.retain(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"));

    let mut tables = BTreeMap::new();
    let mut sources = Vec::new();
    for path in paths {
        let text = fs::read_to_string(&path).map_err(|source| SchemaError::Read {
            path: path.clone(),
            source,
        })?;
        let file: SchemaFile = toml::from_str(&text).map_err(|source| SchemaError::Parse {
            path: path.clone(),
            source: Box::new(source),
        })?;
        let mut source_tables = Vec::new();
        for (name, table) in file.table {
            source_tables.push(name.clone());
            if tables.insert(name.clone(), table).is_some() {
                return Err(SchemaError::Validation(format!(
                    "duplicate table definition: {name}"
                )));
            }
        }
        source_tables.sort_by(|left, right| {
            let left_table = tables
                .get(left)
                .expect("source table should exist in merged schema");
            let right_table = tables
                .get(right)
                .expect("source table should exist in merged schema");
            left_table
                .order
                .unwrap_or(u32::MAX)
                .cmp(&right_table.order.unwrap_or(u32::MAX))
                .then_with(|| left.cmp(right))
        });
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| {
                SchemaError::Validation(format!("invalid logical schema filename: {path:?}"))
            })?
            .to_string();
        sources.push(SchemaSource {
            name,
            tables: source_tables,
        });
    }

    let schema = LogicalSchema { tables };
    validate_schema(&schema)?;
    Ok(LoadedSchema { schema, sources })
}

pub fn validate_schema(schema: &LogicalSchema) -> Result<(), SchemaError> {
    if schema.tables.is_empty() {
        return Err(SchemaError::Validation(
            "logical schema must define at least one table".to_string(),
        ));
    }

    for (table_name, table) in &schema.tables {
        if table.columns.is_empty() {
            return Err(SchemaError::Validation(format!(
                "table {table_name} must define at least one column"
            )));
        }

        let mut columns = BTreeSet::new();
        for column in &table.columns {
            validate_identifier(&column.name, "column", table_name)?;
            if !columns.insert(column.name.as_str()) {
                return Err(SchemaError::Validation(format!(
                    "table {table_name} defines duplicate column {}",
                    column.name
                )));
            }
        }

        for column_name in &table.primary_key {
            require_column(table_name, &columns, column_name, "primary key")?;
        }

        let mut names = BTreeSet::new();
        for unique in &table.uniques {
            validate_named_columns(
                table_name,
                &columns,
                &mut names,
                &unique.name,
                &unique.columns,
            )?;
        }
        for index in &table.indexes {
            validate_named_columns(
                table_name,
                &columns,
                &mut names,
                &index.name,
                &index.columns,
            )?;
        }
        for foreign_key in &table.foreign_keys {
            validate_named_columns(
                table_name,
                &columns,
                &mut names,
                &foreign_key.name,
                &foreign_key.columns,
            )?;
            let Some(referenced) = schema.tables.get(&foreign_key.references_table) else {
                return Err(SchemaError::Validation(format!(
                    "foreign key {} on {table_name} references unknown table {}",
                    foreign_key.name, foreign_key.references_table
                )));
            };
            let referenced_columns = referenced
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<BTreeSet<_>>();
            if foreign_key.columns.len() != foreign_key.references_columns.len() {
                return Err(SchemaError::Validation(format!(
                    "foreign key {} on {table_name} has mismatched local/reference column count",
                    foreign_key.name
                )));
            }
            for column_name in &foreign_key.references_columns {
                require_column(
                    &foreign_key.references_table,
                    &referenced_columns,
                    column_name,
                    "foreign key reference",
                )?;
            }
        }
    }

    Ok(())
}

fn validate_named_columns(
    table_name: &str,
    columns: &BTreeSet<&str>,
    names: &mut BTreeSet<String>,
    name: &str,
    column_names: &[String],
) -> Result<(), SchemaError> {
    validate_identifier(name, "constraint/index", table_name)?;
    if !names.insert(name.to_string()) {
        return Err(SchemaError::Validation(format!(
            "table {table_name} defines duplicate constraint/index name {name}"
        )));
    }
    if column_names.is_empty() {
        return Err(SchemaError::Validation(format!(
            "table {table_name} constraint/index {name} must reference at least one column"
        )));
    }
    for column_name in column_names {
        require_column(table_name, columns, column_name, name)?;
    }
    Ok(())
}

fn require_column(
    table_name: &str,
    columns: &BTreeSet<&str>,
    column_name: &str,
    context: &str,
) -> Result<(), SchemaError> {
    if columns.contains(column_name) {
        Ok(())
    } else {
        Err(SchemaError::Validation(format!(
            "table {table_name} {context} references unknown column {column_name}"
        )))
    }
}

fn validate_identifier(value: &str, kind: &str, table_name: &str) -> Result<(), SchemaError> {
    let mut chars = value.chars();
    let valid = matches!(chars.next(), Some(ch) if ch.is_ascii_lowercase() || ch == '_')
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(SchemaError::Validation(format!(
            "invalid {kind} identifier {value} in table {table_name}"
        )))
    }
}

pub fn generate_to_dir(
    schema: &LogicalSchema,
    output_root: impl AsRef<Path>,
) -> Result<(), SchemaError> {
    let source = SchemaSource {
        name: "schema".to_string(),
        tables: schema.ordered_table_names(),
    };
    generate_sources_to_dir(schema, &[source], output_root)
}

pub fn generate_loaded_to_dir(
    loaded: &LoadedSchema,
    output_root: impl AsRef<Path>,
) -> Result<(), SchemaError> {
    generate_sources_to_dir(&loaded.schema, &loaded.sources, output_root)
}

pub fn generate_sources_to_dir(
    schema: &LogicalSchema,
    sources: &[SchemaSource],
    output_root: impl AsRef<Path>,
) -> Result<(), SchemaError> {
    let output_root = output_root.as_ref();
    write_generated_readme(output_root)?;
    write_driver_sources(output_root, "postgres", sources, |tables| {
        dialect::postgres::emit_named_schema(schema, tables)
    })?;
    write_driver_sources(output_root, "mysql", sources, |tables| {
        dialect::mysql::emit_named_schema(schema, tables)
    })?;
    write_driver_sources(output_root, "sqlite", sources, |tables| {
        dialect::sqlite::emit_named_schema(schema, tables)
    })?;
    Ok(())
}

pub fn check_generated_dir(
    loaded: &LoadedSchema,
    output_root: impl AsRef<Path>,
) -> Result<(), SchemaError> {
    let output_root = output_root.as_ref();
    assert_file_contents(output_root.join("README.md"), &generated_readme())?;
    assert_generated_root_files(output_root)?;
    check_driver_sources(output_root, "postgres", &loaded.sources, |tables| {
        dialect::postgres::emit_named_schema(&loaded.schema, tables)
    })?;
    check_driver_sources(output_root, "mysql", &loaded.sources, |tables| {
        dialect::mysql::emit_named_schema(&loaded.schema, tables)
    })?;
    check_driver_sources(output_root, "sqlite", &loaded.sources, |tables| {
        dialect::sqlite::emit_named_schema(&loaded.schema, tables)
    })?;
    Ok(())
}

pub fn check_required_tables(
    schema: &LogicalSchema,
    sql_paths: &[PathBuf],
) -> Result<(), SchemaError> {
    for path in sql_paths {
        let text = fs::read_to_string(path).map_err(|source| SchemaError::Read {
            path: path.clone(),
            source,
        })?;
        let required = extract_table_shapes(&text);
        let mut missing_tables = Vec::new();
        let mut missing_columns = Vec::new();
        for (table_name, required_columns) in required {
            let Some(table) = schema.tables.get(&table_name) else {
                missing_tables.push(table_name);
                continue;
            };
            let defined_columns = table
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<BTreeSet<_>>();
            let missing = required_columns
                .iter()
                .filter(|column| !defined_columns.contains(column.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                missing_columns.push(format!("{table_name}.{}", missing.join("|")));
            }
        }
        if !missing_tables.is_empty() || !missing_columns.is_empty() {
            let mut details = Vec::new();
            if !missing_tables.is_empty() {
                details.push(format!("missing tables: {}", missing_tables.join(", ")));
            }
            if !missing_columns.is_empty() {
                details.push(format!("missing columns: {}", missing_columns.join(", ")));
            }
            return Err(SchemaError::Validation(format!(
                "logical schema is missing definitions required by {}: {}",
                path.display(),
                details.join("; ")
            )));
        }
    }
    Ok(())
}

pub fn extract_create_table_names(sql: &str) -> BTreeSet<String> {
    extract_table_shapes(sql).into_keys().collect()
}

pub fn extract_table_shapes(sql: &str) -> BTreeMap<String, BTreeSet<String>> {
    const PREFIX: &str = "CREATE TABLE IF NOT EXISTS ";
    let mut tables = BTreeMap::<String, BTreeSet<String>>::new();
    let mut current_table = None::<String>;
    let mut skipping_table_constraint = false;
    for line in sql.lines() {
        let trimmed = line.trim_start();
        let upper = trimmed.to_ascii_uppercase();
        if !upper.starts_with(PREFIX) {
            if let Some(table_name) = &current_table {
                if trimmed.starts_with(");") || (trimmed.starts_with(')') && trimmed.ends_with(';'))
                {
                    current_table = None;
                    skipping_table_constraint = false;
                } else if trimmed == ")" || trimmed == ")," {
                    skipping_table_constraint = false;
                } else if skipping_table_constraint {
                    if table_definition_entry_ends(trimmed) {
                        skipping_table_constraint = false;
                    }
                } else if is_table_constraint_start(trimmed) {
                    if !table_definition_entry_ends(trimmed) {
                        skipping_table_constraint = true;
                    }
                } else if let Some(column_name) = parse_create_table_column_name(trimmed) {
                    tables
                        .entry(table_name.clone())
                        .or_default()
                        .insert(column_name);
                }
            } else if let Some((table_name, column_name)) = parse_alter_table_add_column(trimmed) {
                tables.entry(table_name).or_default().insert(column_name);
            }
            continue;
        }
        let rest = &trimmed[PREFIX.len()..];
        if let Some(table) = rest
            .split_whitespace()
            .next()
            .and_then(normalize_table_token)
        {
            tables.entry(table.clone()).or_default();
            current_table = Some(table);
            skipping_table_constraint = false;
        }
    }
    tables
}

fn is_table_constraint_start(line: &str) -> bool {
    let Some(first) = line.split_whitespace().next() else {
        return false;
    };
    let first = first.trim_end_matches(',').to_ascii_uppercase();
    matches!(
        first.as_str(),
        "CHECK" | "CONSTRAINT" | "FOREIGN" | "KEY" | "PRIMARY" | "UNIQUE"
    )
}

fn table_definition_entry_ends(line: &str) -> bool {
    let line = line.trim_end();
    line.ends_with(',') || line == ")" || line == ");"
}

fn parse_create_table_column_name(line: &str) -> Option<String> {
    let first = line.split_whitespace().next()?;
    let column_name = normalize_identifier_token(first)?;
    let upper = column_name.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CONSTRAINT" | "FOREIGN" | "KEY" | "PRIMARY" | "UNIQUE" | "INDEX" | "CHECK"
    ) {
        None
    } else {
        Some(column_name)
    }
}

fn parse_alter_table_add_column(line: &str) -> Option<(String, String)> {
    let upper = line.to_ascii_uppercase();
    if !upper.starts_with("ALTER TABLE ") || !upper.contains(" ADD COLUMN ") {
        return None;
    }
    let tokens = line.split_whitespace().collect::<Vec<_>>();
    let table_name = normalize_table_token(tokens.get(2)?)?;
    let add_index = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case("ADD"))?;
    let mut column_index = add_index + 2;
    if tokens
        .get(column_index)
        .is_some_and(|token| token.eq_ignore_ascii_case("IF"))
    {
        column_index += 3;
    }
    let column_name = normalize_identifier_token(tokens.get(column_index)?)?;
    Some((table_name, column_name))
}

fn normalize_table_token(token: &str) -> Option<String> {
    let mut value = token
        .trim()
        .trim_end_matches('(')
        .trim_end_matches(';')
        .trim()
        .to_string();
    if let Some((_, table)) = value.rsplit_once('.') {
        value = table.to_string();
    }
    value = value
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('[')
        .trim_matches(']')
        .to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn normalize_identifier_token(token: &str) -> Option<String> {
    let value = token
        .trim()
        .trim_end_matches(',')
        .trim_end_matches('(')
        .trim_end_matches(';')
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('[')
        .trim_matches(']')
        .to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn write_driver_sources(
    output_root: &Path,
    driver: &str,
    sources: &[SchemaSource],
    emit: impl Fn(&[String]) -> String,
) -> Result<(), SchemaError> {
    let baseline_dir = output_root.join(driver).join("baseline");
    if baseline_dir.exists() {
        fs::remove_dir_all(&baseline_dir).map_err(|source| SchemaError::Write {
            path: baseline_dir.clone(),
            source,
        })?;
    }
    fs::create_dir_all(&baseline_dir).map_err(|source| SchemaError::Write {
        path: baseline_dir.clone(),
        source,
    })?;

    let mut manifest = generated_header();
    for source in sources {
        let filename = format!("{}.sql", source.name);
        manifest.push_str(&filename);
        manifest.push('\n');
        let mut output = generated_header();
        output.push_str(&emit(&source.tables));
        write_generated(baseline_dir.join(filename), &output)?;
    }
    write_generated(baseline_dir.join("manifest.txt"), &manifest)?;
    Ok(())
}

fn write_generated_readme(output_root: &Path) -> Result<(), SchemaError> {
    write_generated(output_root.join("README.md"), &generated_readme())
}

fn assert_generated_root_files(output_root: &Path) -> Result<(), SchemaError> {
    let expected = BTreeSet::from([
        "README.md".to_string(),
        "mysql".to_string(),
        "postgres".to_string(),
        "sqlite".to_string(),
    ]);
    let actual = fs::read_dir(output_root)
        .map_err(|source| SchemaError::Read {
            path: output_root.to_path_buf(),
            source,
        })?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().to_string()))
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|source| SchemaError::Read {
            path: output_root.to_path_buf(),
            source,
        })?;
    if actual == expected {
        Ok(())
    } else {
        Err(SchemaError::Validation(format!(
            "generated root file set mismatch: expected {:?}, got {:?}",
            expected, actual
        )))
    }
}

fn check_driver_sources(
    output_root: &Path,
    driver: &str,
    sources: &[SchemaSource],
    emit: impl Fn(&[String]) -> String,
) -> Result<(), SchemaError> {
    let baseline_dir = output_root.join(driver).join("baseline");
    let expected_manifest = generated_header()
        + &sources
            .iter()
            .map(|source| format!("{}.sql\n", source.name))
            .collect::<String>();
    assert_file_contents(baseline_dir.join("manifest.txt"), &expected_manifest)?;

    let expected_files = sources
        .iter()
        .map(|source| format!("{}.sql", source.name))
        .chain(std::iter::once("manifest.txt".to_string()))
        .collect::<BTreeSet<_>>();
    let actual_files = fs::read_dir(&baseline_dir)
        .map_err(|source| SchemaError::Read {
            path: baseline_dir.clone(),
            source,
        })?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().to_string()))
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|source| SchemaError::Read {
            path: baseline_dir.clone(),
            source,
        })?;
    if actual_files != expected_files {
        return Err(SchemaError::Validation(format!(
            "generated {driver} file set mismatch: expected {:?}, got {:?}",
            expected_files, actual_files
        )));
    }

    for source in sources {
        let expected = generated_header() + &emit(&source.tables);
        assert_file_contents(baseline_dir.join(format!("{}.sql", source.name)), &expected)?;
    }
    Ok(())
}

fn generated_header() -> String {
    "-- Generated by aether-data-schema from crates/aether-data/runtime/schema/logical/*.toml.\n\
     -- Do not edit generated files directly; edit logical schema or explicit overrides instead.\n\n"
        .to_string()
}

fn generated_readme() -> String {
    "# Generated Schema\n\n\
     This directory is generated by `aether-data-schema` from `../logical/*.toml`.\n\
     It is checked in only as an auditable compiler output and drift-detection fixture.\n\n\
     Do not edit files in this directory by hand. Update `../logical/*.toml`, then run:\n\n\
     ```bash\n\
     bash crates/aether-data/runtime/schema/compose_schema.sh generate\n\
     ```\n\n\
     Runtime migrations are not loaded from this directory. The executable SQL lives under \
     `crates/aether-data/adapters/{postgres,mysql,sqlite}/migrations`, and the Postgres bootstrap snapshot \
     is generated at build time from `crates/aether-data/runtime/schema/bootstrap/postgres` into the crate \
     build output until a generated fragment is deliberately promoted into the driver-specific \
     schema manifests.\n"
        .to_string()
}

fn assert_file_contents(path: PathBuf, expected: &str) -> Result<(), SchemaError> {
    let actual = fs::read_to_string(&path).map_err(|source| SchemaError::Read {
        path: path.clone(),
        source,
    })?;
    if actual == expected {
        Ok(())
    } else {
        Err(SchemaError::Validation(format!(
            "generated schema file is stale: {path:?}"
        )))
    }
}

fn write_generated(path: PathBuf, contents: &str) -> Result<(), SchemaError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| SchemaError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(&path, contents).map_err(|source| SchemaError::Write { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{mysql, postgres, sqlite};

    fn announcements_schema() -> LogicalSchema {
        LogicalSchema {
            tables: BTreeMap::from([(
                "announcements".to_string(),
                Table {
                    domain: None,
                    order: Some(1),
                    columns: vec![
                        Column {
                            name: "id".to_string(),
                            logical_type: LogicalType::TextId,
                            nullable: false,
                            auto_increment: false,
                            default: None,
                            length: Some(64),
                            driver: DriverColumnOverrides::default(),
                        },
                        Column {
                            name: "is_active".to_string(),
                            logical_type: LogicalType::Bool,
                            nullable: false,
                            auto_increment: false,
                            default: Some(DefaultValue::Bool(true)),
                            length: None,
                            driver: DriverColumnOverrides::default(),
                        },
                    ],
                    primary_key: vec!["id".to_string()],
                    uniques: vec![],
                    indexes: vec![Index {
                        name: "announcements_is_active_idx".to_string(),
                        columns: vec!["is_active".to_string()],
                        unique: false,
                    }],
                    foreign_keys: vec![],
                },
            )]),
        }
    }

    #[test]
    fn validator_rejects_unknown_index_column() {
        let mut schema = announcements_schema();
        schema
            .tables
            .get_mut("announcements")
            .expect("fixture table exists")
            .indexes
            .push(Index {
                name: "ix_missing".to_string(),
                columns: vec!["missing".to_string()],
                unique: false,
            });

        let err = validate_schema(&schema).expect_err("invalid schema should fail");
        assert!(err.to_string().contains("unknown column missing"));
    }

    #[test]
    fn emitters_generate_expected_driver_types() {
        let schema = announcements_schema();
        validate_schema(&schema).expect("fixture schema should be valid");

        let postgres_sql = postgres::emit_schema(&schema);
        assert!(postgres_sql.contains("id character varying(64) NOT NULL"));
        assert!(postgres_sql.contains("is_active boolean DEFAULT true NOT NULL"));
        assert!(postgres_sql.contains(
            "CREATE INDEX IF NOT EXISTS announcements_is_active_idx ON public.announcements USING btree (is_active);"
        ));

        let mysql_sql = mysql::emit_schema(&schema);
        assert!(mysql_sql.contains("`id` VARCHAR(64) NOT NULL"));
        assert!(mysql_sql.contains("`is_active` TINYINT(1) NOT NULL DEFAULT 1"));
        assert!(mysql_sql.contains("KEY announcements_is_active_idx (`is_active`)"));

        let sqlite_sql = sqlite::emit_schema(&schema);
        assert!(sqlite_sql.contains("id TEXT PRIMARY KEY NOT NULL"));
        assert!(sqlite_sql.contains("is_active INTEGER NOT NULL DEFAULT 1"));
        assert!(sqlite_sql.contains(
            "CREATE INDEX IF NOT EXISTS announcements_is_active_idx ON announcements (is_active);"
        ));
    }

    #[test]
    fn extracts_create_table_names_across_driver_quoting() {
        let tables = extract_create_table_names(
            r#"
CREATE TABLE IF NOT EXISTS public.users (
CREATE TABLE IF NOT EXISTS `usage` (
CREATE TABLE IF NOT EXISTS "stats_daily" (
            "#,
        );

        assert_eq!(
            tables,
            BTreeSet::from([
                "stats_daily".to_string(),
                "usage".to_string(),
                "users".to_string()
            ])
        );
    }

    #[test]
    fn extracts_table_shapes_from_create_and_alter_sql() {
        let shapes = extract_table_shapes(
            r#"
CREATE TABLE IF NOT EXISTS public.users (
    id VARCHAR(64) PRIMARY KEY,
    email VARCHAR(320),
    CHECK (
        (id IS NOT NULL AND email IS NULL)
        OR (id IS NULL AND email IS NOT NULL)
    )
);
CREATE INDEX IF NOT EXISTS idx_users_email
    ON users (email);
ALTER TABLE users ADD COLUMN ldap_dn VARCHAR(1024);
            "#,
        );

        assert_eq!(
            shapes.get("users"),
            Some(&BTreeSet::from([
                "email".to_string(),
                "id".to_string(),
                "ldap_dn".to_string()
            ]))
        );
    }

    #[test]
    fn required_table_check_reports_missing_tables() {
        let schema = announcements_schema();
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let path = manifest_dir.join("target/required-table-fixture.sql");
        std::fs::create_dir_all(path.parent().expect("fixture should have parent"))
            .expect("fixture dir should be writable");
        std::fs::write(
            &path,
            "CREATE TABLE IF NOT EXISTS announcements (\n);\nCREATE TABLE IF NOT EXISTS users (\n);\n",
        )
        .expect("fixture should be writable");

        let err = check_required_tables(&schema, std::slice::from_ref(&path))
            .expect_err("missing required table should fail");
        std::fs::remove_file(&path).expect("fixture should be removable");
        assert!(err.to_string().contains("users"));
    }

    #[test]
    fn required_table_check_reports_missing_columns() {
        let schema = announcements_schema();
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let path = manifest_dir.join("target/required-column-fixture.sql");
        std::fs::create_dir_all(path.parent().expect("fixture should have parent"))
            .expect("fixture dir should be writable");
        std::fs::write(
            &path,
            "CREATE TABLE IF NOT EXISTS announcements (\n    id TEXT PRIMARY KEY,\n    title TEXT\n);\n",
        )
        .expect("fixture should be writable");

        let err = check_required_tables(&schema, std::slice::from_ref(&path))
            .expect_err("missing required column should fail");
        std::fs::remove_file(&path).expect("fixture should be removable");
        assert!(err.to_string().contains("announcements.title"));
    }

    #[test]
    fn workspace_logical_schema_generated_artifacts_are_current() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest_dir
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .expect("crate should live under workspace/crates/aether-data");
        let schema_dir = workspace.join("crates/aether-data/runtime/schema/logical");
        let generated_dir = workspace.join("crates/aether-data/runtime/schema/generated");

        let loaded = load_schema_sources(schema_dir).expect("workspace logical schema should load");
        check_generated_dir(&loaded, generated_dir)
            .expect("workspace generated schema should be current");
    }

    #[test]
    fn workspace_logical_schema_covers_required_sql_tables() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace = manifest_dir
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .expect("crate should live under workspace/crates/aether-data");
        let schema_dir = workspace.join("crates/aether-data/runtime/schema/logical");
        let mut required_sql_paths = vec![workspace
            .join("crates/aether-data/adapters/postgres/migrations/20260403000000_baseline.sql")];
        for driver in ["mysql", "sqlite"] {
            let driver_dir =
                workspace.join(format!("crates/aether-data/adapters/{driver}/migrations"));
            let mut paths = std::fs::read_dir(&driver_dir)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", driver_dir.display()))
                .map(|entry| entry.expect("migration entry should be readable").path())
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sql"))
                .collect::<Vec<_>>();
            paths.sort();
            required_sql_paths.extend(paths);
        }

        let loaded = load_schema_sources(schema_dir).expect("workspace logical schema should load");
        check_required_tables(&loaded.schema, &required_sql_paths)
            .expect("portable SQL tables should be defined in logical schema");
    }
}
