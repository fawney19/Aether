use crate::dialect::{
    column_default, column_nullable, default_sql, mysql_type, referential_action_sql,
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
            let mut definition = format!("    `{}` {}", column.name, mysql_type(column));
            if !column_nullable(column, column.driver.mysql.as_ref()) {
                definition.push_str(" NOT NULL");
            }
            if column.auto_increment {
                definition.push_str(" AUTO_INCREMENT");
            }
            if let Some(default) = column_default(column, column.driver.mysql.as_ref()) {
                definition.push_str(" DEFAULT ");
                definition.push_str(&default_sql(default));
            }
            definitions.push(definition);
        }
        if !table.primary_key.is_empty() {
            definitions.push(format!(
                "    PRIMARY KEY ({})",
                table
                    .primary_key
                    .iter()
                    .map(|column| format!("`{column}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for unique in &table.uniques {
            definitions.push(format!(
                "    UNIQUE KEY {} ({})",
                unique.name,
                unique
                    .columns
                    .iter()
                    .map(|column| format!("`{column}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for index in &table.indexes {
            let unique = if index.unique { "UNIQUE " } else { "" };
            definitions.push(format!(
                "    {unique}KEY {} ({})",
                index.name,
                index
                    .columns
                    .iter()
                    .map(|column| format!("`{column}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for foreign_key in &table.foreign_keys {
            let mut definition = format!(
                "    CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                foreign_key.name,
                foreign_key
                    .columns
                    .iter()
                    .map(|column| format!("`{column}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                foreign_key.references_table,
                foreign_key
                    .references_columns
                    .iter()
                    .map(|column| format!("`{column}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if let Some(action) = &foreign_key.on_delete {
                definition.push_str(" ON DELETE ");
                definition.push_str(referential_action_sql(action));
            }
            definitions.push(definition);
        }

        out.push_str(&format!(
            "CREATE TABLE IF NOT EXISTS {} (\n",
            quote_identifier_if_needed(table_name)
        ));
        out.push_str(&definitions.join(",\n"));
        out.push_str("\n);\n\n");
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
    format!("`{}`", identifier.replace('`', "``"))
}
