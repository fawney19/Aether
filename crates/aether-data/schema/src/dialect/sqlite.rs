use crate::dialect::{
    column_default, column_nullable, default_sql, referential_action_sql, sqlite_type,
};
use crate::LogicalSchema;

pub fn emit_schema(schema: &LogicalSchema) -> String {
    emit_named_schema(schema, &schema.ordered_table_names())
}

pub fn emit_named_schema(schema: &LogicalSchema, table_names: &[String]) -> String {
    let mut out = String::new();
    for table_name in table_names {
        let table = schema
            .tables
            .get(table_name)
            .expect("named schema table should exist");
        let mut definitions = Vec::new();
        for column in &table.columns {
            let mut definition = format!("    {} {}", column.name, sqlite_type(column));
            if table.primary_key.len() == 1 && table.primary_key[0] == column.name {
                definition.push_str(" PRIMARY KEY");
                if column.auto_increment {
                    definition.push_str(" AUTOINCREMENT");
                }
            }
            if !column.auto_increment && !column_nullable(column, column.driver.sqlite.as_ref()) {
                definition.push_str(" NOT NULL");
            }
            if let Some(default) = column_default(column, column.driver.sqlite.as_ref()) {
                definition.push_str(" DEFAULT ");
                definition.push_str(&default_sql(default));
            }
            definitions.push(definition);
        }
        if table.primary_key.len() > 1 {
            definitions.push(format!(
                "    PRIMARY KEY ({})",
                table.primary_key.join(", ")
            ));
        }
        for unique in &table.uniques {
            definitions.push(format!("    UNIQUE ({})", unique.columns.join(", ")));
        }
        for foreign_key in &table.foreign_keys {
            let mut definition = format!(
                "    CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                foreign_key.name,
                foreign_key.columns.join(", "),
                foreign_key.references_table,
                foreign_key.references_columns.join(", ")
            );
            if let Some(action) = &foreign_key.on_delete {
                definition.push_str(" ON DELETE ");
                definition.push_str(referential_action_sql(action));
            }
            definitions.push(definition);
        }

        let quoted_table_name = quote_identifier_if_needed(table_name);
        out.push_str(&format!(
            "CREATE TABLE IF NOT EXISTS {quoted_table_name} (\n"
        ));
        out.push_str(&definitions.join(",\n"));
        out.push_str("\n);\n");
        for index in &table.indexes {
            let unique = if index.unique { "UNIQUE " } else { "" };
            out.push_str(&format!(
                "CREATE {unique}INDEX IF NOT EXISTS {} ON {quoted_table_name} ({});\n",
                index.name,
                index.columns.join(", ")
            ));
        }
        out.push('\n');
    }
    out
}

fn quote_identifier_if_needed(identifier: &str) -> String {
    if needs_quoting(identifier) {
        quote_identifier(identifier)
    } else {
        identifier.to_string()
    }
}

fn needs_quoting(identifier: &str) -> bool {
    matches!(identifier, "date" | "usage")
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
