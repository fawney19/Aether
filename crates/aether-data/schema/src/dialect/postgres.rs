use crate::dialect::{
    column_default, column_nullable, default_sql_bool_keywords, postgres_type,
    referential_action_sql,
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
        out.push_str(&format!(
            "CREATE TABLE IF NOT EXISTS public.{table_name} (\n"
        ));
        for (index, column) in table.columns.iter().enumerate() {
            let comma = if index + 1 == table.columns.len() {
                ""
            } else {
                ","
            };
            out.push_str("    ");
            out.push_str(&column.name);
            out.push(' ');
            out.push_str(&postgres_type(column));
            if let Some(default) = column_default(column, column.driver.postgres.as_ref()) {
                out.push_str(" DEFAULT ");
                out.push_str(&default_sql_bool_keywords(default));
            }
            if !column_nullable(column, column.driver.postgres.as_ref()) {
                out.push_str(" NOT NULL");
            }
            out.push_str(comma);
            out.push('\n');
        }
        out.push_str(");\n\n");

        if !table.primary_key.is_empty() {
            out.push_str(&format!(
                "ALTER TABLE ONLY public.{table_name} ADD CONSTRAINT {table_name}_pkey PRIMARY KEY ({});\n",
                table.primary_key.join(", ")
            ));
        }
        for unique in &table.uniques {
            out.push_str(&format!(
                "ALTER TABLE ONLY public.{table_name} ADD CONSTRAINT {} UNIQUE ({});\n",
                unique.name,
                unique.columns.join(", ")
            ));
        }
        for index in &table.indexes {
            let unique = if index.unique { "UNIQUE " } else { "" };
            out.push_str(&format!(
                "CREATE {unique}INDEX IF NOT EXISTS {} ON public.{table_name} USING btree ({});\n",
                index.name,
                index.columns.join(", ")
            ));
        }
        for foreign_key in &table.foreign_keys {
            out.push_str(&format!(
                "ALTER TABLE ONLY public.{table_name} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES public.{}({})",
                foreign_key.name,
                foreign_key.columns.join(", "),
                foreign_key.references_table,
                foreign_key.references_columns.join(", ")
            ));
            if let Some(action) = &foreign_key.on_delete {
                out.push_str(" ON DELETE ");
                out.push_str(referential_action_sql(action));
            }
            out.push_str(";\n");
        }
        out.push('\n');
    }
    out
}
