use sqlx::{Database, Encode, QueryBuilder, Type};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlDialect {
    Postgres,
    MySql,
    Sqlite,
}

impl SqlDialect {
    pub fn quote_ident(self, ident: &str) -> String {
        let quote = match self {
            Self::Postgres | Self::Sqlite => '"',
            Self::MySql => '`',
        };
        let escaped = ident.replace(quote, &format!("{quote}{quote}"));
        format!("{quote}{escaped}{quote}")
    }

    pub fn quote_path(self, parts: &[&str]) -> String {
        parts
            .iter()
            .map(|part| self.quote_ident(part))
            .collect::<Vec<_>>()
            .join(".")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialectSql<'a> {
    common: Option<&'a str>,
    postgres: Option<&'a str>,
    mysql: Option<&'a str>,
    sqlite: Option<&'a str>,
}

impl<'a> DialectSql<'a> {
    pub const fn common(sql: &'a str) -> Self {
        Self {
            common: Some(sql),
            postgres: None,
            mysql: None,
            sqlite: None,
        }
    }

    pub const fn dialect(postgres: &'a str, sqlite: &'a str) -> Self {
        Self {
            common: None,
            postgres: Some(postgres),
            mysql: None,
            sqlite: Some(sqlite),
        }
    }

    pub fn with_postgres(mut self, sql: &'a str) -> Self {
        self.postgres = Some(sql);
        self
    }

    pub fn with_mysql(mut self, sql: &'a str) -> Self {
        self.mysql = Some(sql);
        self
    }

    pub fn with_sqlite(mut self, sql: &'a str) -> Self {
        self.sqlite = Some(sql);
        self
    }

    pub fn sql(self, dialect: SqlDialect) -> &'a str {
        match dialect {
            SqlDialect::Postgres => self.postgres.or(self.common),
            SqlDialect::MySql => self.mysql.or(self.common),
            SqlDialect::Sqlite => self.sqlite.or(self.common),
        }
        .expect("dialect SQL expression is missing for selected dialect")
    }
}

impl<'a> From<&'a str> for DialectSql<'a> {
    fn from(value: &'a str) -> Self {
        Self::common(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectColumn<'a> {
    expr: DialectSql<'a>,
    alias: Option<&'a str>,
}

impl<'a> SelectColumn<'a> {
    pub fn expr(expr: impl Into<DialectSql<'a>>) -> Self {
        Self {
            expr: expr.into(),
            alias: None,
        }
    }

    pub fn alias(mut self, alias: &'a str) -> Self {
        self.alias = Some(alias);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectQuery<'a> {
    distinct: bool,
    columns: Vec<SelectColumn<'a>>,
    from: DialectSql<'a>,
    joins: Vec<DialectSql<'a>>,
}

impl<'a> SelectQuery<'a> {
    pub fn new(from: impl Into<DialectSql<'a>>) -> Self {
        Self {
            distinct: false,
            columns: Vec::new(),
            from: from.into(),
            joins: Vec::new(),
        }
    }

    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    pub fn select(mut self, column: SelectColumn<'a>) -> Self {
        self.columns.push(column);
        self
    }

    pub fn select_columns<I>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = SelectColumn<'a>>,
    {
        self.columns.extend(columns);
        self
    }

    pub fn join(mut self, join_sql: impl Into<DialectSql<'a>>) -> Self {
        self.joins.push(join_sql.into());
        self
    }

    pub fn render(&self, dialect: SqlDialect) -> String {
        let mut sql = String::from("SELECT ");
        if self.distinct {
            sql.push_str("DISTINCT ");
        }

        if self.columns.is_empty() {
            sql.push('*');
        } else {
            for (index, column) in self.columns.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                sql.push_str(column.expr.sql(dialect));
                if let Some(alias) = column.alias {
                    sql.push_str(" AS ");
                    sql.push_str(&dialect.quote_ident(alias));
                }
            }
        }

        sql.push_str(" FROM ");
        sql.push_str(self.from.sql(dialect));
        for join in &self.joins {
            sql.push(' ');
            sql.push_str(join.sql(dialect));
        }
        sql
    }

    pub fn statement<'args, DB>(&self, dialect: SqlDialect) -> SelectStatement<'args, DB>
    where
        DB: Database,
    {
        SelectStatement {
            dialect,
            builder: QueryBuilder::<DB>::new(self.render(dialect)),
            where_clause: WhereClause::new(),
        }
    }
}

pub struct SelectStatement<'args, DB>
where
    DB: Database,
{
    dialect: SqlDialect,
    builder: QueryBuilder<'args, DB>,
    where_clause: WhereClause,
}

impl<'args, DB> SelectStatement<'args, DB>
where
    DB: Database,
{
    pub fn where_eq<T>(&mut self, column_sql: &str, value: T) -> &mut Self
    where
        T: 'args + Encode<'args, DB> + Type<DB>,
    {
        push_eq(&mut self.builder, &mut self.where_clause, column_sql, value);
        self
    }

    pub fn where_optional_eq<T>(&mut self, column_sql: &str, value: Option<T>) -> &mut Self
    where
        T: 'args + Encode<'args, DB> + Type<DB>,
    {
        push_optional_eq(&mut self.builder, &mut self.where_clause, column_sql, value);
        self
    }

    pub fn where_in<T>(&mut self, column_sql: &str, values: &[T]) -> &mut Self
    where
        T: Clone + 'args + Encode<'args, DB> + Type<DB>,
    {
        push_in(
            &mut self.builder,
            &mut self.where_clause,
            column_sql,
            values,
        );
        self
    }

    pub fn where_ci_contains(&mut self, column_sql: &str, value: &str) -> &mut Self
    where
        String: 'args + Encode<'args, DB> + Type<DB>,
    {
        push_ci_contains(
            &mut self.builder,
            &mut self.where_clause,
            self.dialect,
            column_sql,
            value,
        );
        self
    }

    pub fn where_ci_contains_any(&mut self, column_sqls: &[&str], value: &str) -> &mut Self
    where
        String: 'args + Encode<'args, DB> + Type<DB>,
    {
        push_ci_contains_any(
            &mut self.builder,
            &mut self.where_clause,
            self.dialect,
            column_sqls,
            value,
        );
        self
    }

    pub fn where_raw(&mut self, predicate_sql: &str) -> &mut Self {
        if !predicate_sql.trim().is_empty() {
            self.where_clause.push_next(&mut self.builder);
            self.builder.push(predicate_sql);
        }
        self
    }

    pub fn order_by_sql(&mut self, order_sql: &str) -> &mut Self {
        if !order_sql.trim().is_empty() {
            self.builder.push(" ORDER BY ").push(order_sql);
        }
        self
    }

    pub fn order_by(
        &mut self,
        requested_key: Option<&str>,
        direction: SortDirection,
        allowed: &[OrderByColumn<'_>],
        default_key: &str,
    ) -> &mut Self {
        push_order_by(
            &mut self.builder,
            requested_key,
            direction,
            allowed,
            default_key,
        );
        self
    }

    pub fn limit(&mut self, limit: i64) -> &mut Self
    where
        i64: 'args + Encode<'args, DB> + Type<DB>,
    {
        push_limit(&mut self.builder, limit);
        self
    }

    pub fn limit_offset(&mut self, limit: i64, offset: i64) -> &mut Self
    where
        i64: 'args + Encode<'args, DB> + Type<DB>,
    {
        push_limit_offset(&mut self.builder, limit, offset);
        self
    }

    pub fn finish(self) -> QueryBuilder<'args, DB> {
        self.builder
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WhereClause {
    has_clause: bool,
}

impl WhereClause {
    pub fn new() -> Self {
        Self { has_clause: false }
    }

    pub fn with_existing_clause() -> Self {
        Self { has_clause: true }
    }

    pub fn is_empty(self) -> bool {
        !self.has_clause
    }

    pub fn push_next<DB>(&mut self, builder: &mut QueryBuilder<'_, DB>)
    where
        DB: Database,
    {
        if self.has_clause {
            builder.push(" AND ");
        } else {
            builder.push(" WHERE ");
            self.has_clause = true;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub fn sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderByColumn<'a> {
    pub key: &'a str,
    pub sql: &'a str,
}

pub fn push_eq<'args, DB, T>(
    builder: &mut QueryBuilder<'args, DB>,
    where_clause: &mut WhereClause,
    column_sql: &str,
    value: T,
) where
    DB: Database,
    T: 'args + Encode<'args, DB> + Type<DB>,
{
    where_clause.push_next(builder);
    builder.push(column_sql).push(" = ").push_bind(value);
}

pub fn push_optional_eq<'args, DB, T>(
    builder: &mut QueryBuilder<'args, DB>,
    where_clause: &mut WhereClause,
    column_sql: &str,
    value: Option<T>,
) where
    DB: Database,
    T: 'args + Encode<'args, DB> + Type<DB>,
{
    if let Some(value) = value {
        push_eq(builder, where_clause, column_sql, value);
    }
}

pub fn push_in<'args, DB, T>(
    builder: &mut QueryBuilder<'args, DB>,
    where_clause: &mut WhereClause,
    column_sql: &str,
    values: &[T],
) where
    DB: Database,
    T: Clone + 'args + Encode<'args, DB> + Type<DB>,
{
    where_clause.push_next(builder);
    builder.push(column_sql).push(" IN (");
    {
        let mut separated = builder.separated(", ");
        for value in values {
            separated.push_bind(value.clone());
        }
    }
    builder.push(")");
}

pub fn push_ci_contains<'args, DB>(
    builder: &mut QueryBuilder<'args, DB>,
    where_clause: &mut WhereClause,
    dialect: SqlDialect,
    column_sql: &str,
    value: &str,
) where
    DB: Database,
    String: 'args + Encode<'args, DB> + Type<DB>,
{
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }

    where_clause.push_next(builder);
    push_ci_contains_predicate(builder, dialect, column_sql, trimmed);
}

pub fn push_ci_contains_any<'args, DB>(
    builder: &mut QueryBuilder<'args, DB>,
    where_clause: &mut WhereClause,
    dialect: SqlDialect,
    column_sqls: &[&str],
    value: &str,
) where
    DB: Database,
    String: 'args + Encode<'args, DB> + Type<DB>,
{
    let trimmed = value.trim();
    if trimmed.is_empty() || column_sqls.is_empty() {
        return;
    }

    where_clause.push_next(builder);
    builder.push("(");
    for (index, column_sql) in column_sqls.iter().enumerate() {
        if index > 0 {
            builder.push(" OR ");
        }
        push_ci_contains_predicate(builder, dialect, column_sql, trimmed);
    }
    builder.push(")");
}

fn push_ci_contains_predicate<'args, DB>(
    builder: &mut QueryBuilder<'args, DB>,
    dialect: SqlDialect,
    column_sql: &str,
    trimmed: &str,
) where
    DB: Database,
    String: 'args + Encode<'args, DB> + Type<DB>,
{
    match dialect {
        SqlDialect::Postgres => {
            builder
                .push(column_sql)
                .push(" ILIKE ")
                .push_bind(format!("%{trimmed}%"));
        }
        SqlDialect::MySql | SqlDialect::Sqlite => {
            builder
                .push("LOWER(")
                .push(column_sql)
                .push(") LIKE ")
                .push_bind(format!("%{}%", trimmed.to_ascii_lowercase()));
        }
    }
}

pub fn push_limit<'args, DB>(builder: &mut QueryBuilder<'args, DB>, limit: i64)
where
    DB: Database,
    i64: 'args + Encode<'args, DB> + Type<DB>,
{
    builder.push(" LIMIT ").push_bind(limit);
}

pub fn push_limit_offset<'args, DB>(builder: &mut QueryBuilder<'args, DB>, limit: i64, offset: i64)
where
    DB: Database,
    i64: 'args + Encode<'args, DB> + Type<DB>,
{
    push_limit(builder, limit);
    builder.push(" OFFSET ").push_bind(offset);
}

pub fn push_order_by<DB>(
    builder: &mut QueryBuilder<'_, DB>,
    requested_key: Option<&str>,
    direction: SortDirection,
    allowed: &[OrderByColumn<'_>],
    default_key: &str,
) where
    DB: Database,
{
    let key = requested_key.unwrap_or(default_key);
    let column = allowed
        .iter()
        .find(|column| column.key == key)
        .or_else(|| allowed.iter().find(|column| column.key == default_key))
        .expect("default order column must be allowed");
    builder
        .push(" ORDER BY ")
        .push(column.sql)
        .push(" ")
        .push(direction.sql());
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{Execute, MySql, Postgres, QueryBuilder, Sqlite};

    #[test]
    fn quotes_identifiers_by_dialect() {
        assert_eq!(SqlDialect::Postgres.quote_ident("trigger"), "\"trigger\"");
        assert_eq!(SqlDialect::MySql.quote_ident("trigger"), "`trigger`");
        assert_eq!(SqlDialect::MySql.quote_ident("tri`gger"), "`tri``gger`");
        assert_eq!(SqlDialect::Sqlite.quote_ident("trigger"), "\"trigger\"");
        assert_eq!(
            SqlDialect::Postgres.quote_path(&["usage", "id"]),
            "\"usage\".\"id\""
        );
        assert_eq!(
            SqlDialect::MySql.quote_path(&["usage", "item`id"]),
            "`usage`.`item``id`"
        );
    }

    #[test]
    fn where_clause_pushes_where_then_and() {
        let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM items");
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "kind",
            "scheduled".to_string(),
        );
        push_eq(
            &mut builder,
            &mut where_clause,
            "status",
            "running".to_string(),
        );
        let query = builder.build();
        assert!(query.sql().contains(" WHERE kind = ? AND status = ?"));
    }

    #[test]
    fn ci_contains_uses_ilike_for_postgres() {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT * FROM items");
        let mut where_clause = WhereClause::new();
        push_ci_contains(
            &mut builder,
            &mut where_clause,
            SqlDialect::Postgres,
            "task_key",
            " Fetch ",
        );
        let query = builder.build();
        assert!(query.sql().contains(" WHERE task_key ILIKE $1"));
    }

    #[test]
    fn ci_contains_uses_lower_like_for_sqlite() {
        let mut sqlite_builder = QueryBuilder::<Sqlite>::new("SELECT * FROM items");
        let mut sqlite_where = WhereClause::new();
        push_ci_contains(
            &mut sqlite_builder,
            &mut sqlite_where,
            SqlDialect::Sqlite,
            "task_key",
            " Fetch ",
        );
        assert!(sqlite_builder
            .build()
            .sql()
            .contains(" WHERE LOWER(task_key) LIKE ?"));
    }

    #[test]
    fn ci_contains_uses_lower_like_for_mysql() {
        let mut mysql_builder = QueryBuilder::<MySql>::new("SELECT * FROM items");
        let mut mysql_where = WhereClause::new();
        push_ci_contains(
            &mut mysql_builder,
            &mut mysql_where,
            SqlDialect::MySql,
            "task_key",
            " Fetch ",
        );
        assert!(mysql_builder
            .build()
            .sql()
            .contains(" WHERE LOWER(task_key) LIKE ?"));
    }

    #[test]
    fn ci_contains_any_groups_or_predicates() {
        let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM items");
        let mut where_clause = WhereClause::new();
        push_ci_contains_any(
            &mut builder,
            &mut where_clause,
            SqlDialect::Sqlite,
            &["file_name", "COALESCE(display_name, '')"],
            "Avatar",
        );
        let query = builder.build();
        assert!(query.sql().contains(
            " WHERE (LOWER(file_name) LIKE ? OR LOWER(COALESCE(display_name, '')) LIKE ?)"
        ));
    }

    #[test]
    fn in_limit_offset_and_order_are_rendered() {
        let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM items");
        let mut where_clause = WhereClause::new();
        push_in(
            &mut builder,
            &mut where_clause,
            "id",
            &["a".to_string(), "b".to_string()],
        );
        push_order_by(
            &mut builder,
            Some("created"),
            SortDirection::Desc,
            &[OrderByColumn {
                key: "created",
                sql: "created_at",
            }],
            "created",
        );
        push_limit_offset(&mut builder, 10, 5);
        let query = builder.build();
        assert!(query.sql().contains(" WHERE id IN (?, ?)"));
        assert!(query.sql().contains(" ORDER BY created_at DESC"));
        assert!(query.sql().contains(" LIMIT ? OFFSET ?"));
    }

    #[test]
    fn select_query_renders_dialect_specific_projection() {
        let query = SelectQuery::new("providers").select_columns([
            SelectColumn::expr("id").alias("provider_id"),
            SelectColumn::expr(
                DialectSql::dialect(
                    "CAST(monthly_quota_usd AS DOUBLE PRECISION)",
                    "CAST(monthly_quota_usd AS REAL)",
                )
                .with_mysql("CAST(monthly_quota_usd AS DOUBLE)"),
            )
            .alias("monthly_quota_usd"),
        ]);

        assert_eq!(
            query.render(SqlDialect::Postgres),
            "SELECT id AS \"provider_id\", CAST(monthly_quota_usd AS DOUBLE PRECISION) AS \"monthly_quota_usd\" FROM providers"
        );
        assert_eq!(
            query.render(SqlDialect::MySql),
            "SELECT id AS `provider_id`, CAST(monthly_quota_usd AS DOUBLE) AS `monthly_quota_usd` FROM providers"
        );
        assert_eq!(
            query.render(SqlDialect::Sqlite),
            "SELECT id AS \"provider_id\", CAST(monthly_quota_usd AS REAL) AS \"monthly_quota_usd\" FROM providers"
        );
    }

    #[test]
    fn select_statement_keeps_bind_order_and_dialect_search() {
        let query = SelectQuery::new("items")
            .select(SelectColumn::expr("id"))
            .select(SelectColumn::expr("name"));
        let mut statement = query.statement::<Postgres>(SqlDialect::Postgres);
        statement
            .where_eq("kind", "scheduled".to_string())
            .where_ci_contains_any(&["name", "description"], "Fetch")
            .order_by(
                Some("name"),
                SortDirection::Asc,
                &[OrderByColumn {
                    key: "name",
                    sql: "name",
                }],
                "name",
            )
            .limit_offset(20, 40);

        let mut builder = statement.finish();
        let query = builder.build();
        assert_eq!(
            query.sql(),
            "SELECT id, name FROM items WHERE kind = $1 AND (name ILIKE $2 OR description ILIKE $3) ORDER BY name ASC LIMIT $4 OFFSET $5"
        );
    }
}
