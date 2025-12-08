use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use tokio_postgres::{Client as PgClient, NoTls, Error};
use std::sync::Arc;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use futures_util::task::Context;
use std::pin::Pin;
use futures_util::task::Poll;
#[derive(Debug)]
pub enum ByteOrmError {
    Pool(String),
    Query(tokio_postgres::Error),
    MissingField(String),
    NotFound(String),
    Validation(String),
    Serialization(String),
}
impl std::fmt::Display for ByteOrmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ByteOrmError::Pool(msg) => write!(f, "Pool error: {}", msg),
            ByteOrmError::Query(e) => write!(f, "Query error: {}", e),
            ByteOrmError::MissingField(field) => {
                write!(f, "Missing required field: {}", field)
            }
            ByteOrmError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ByteOrmError::Validation(msg) => write!(f, "Validation error: {}", msg),
            ByteOrmError::Serialization(msg) => write!(f, "Serialization error: {}", msg),
        }
    }
}
impl std::error::Error for ByteOrmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ByteOrmError::Query(e) => Some(e),
            _ => None,
        }
    }
}
impl From<tokio_postgres::Error> for ByteOrmError {
    fn from(e: tokio_postgres::Error) -> Self {
        ByteOrmError::Query(e)
    }
}
impl From<serde_json::Error> for ByteOrmError {
    fn from(e: serde_json::Error) -> Self {
        ByteOrmError::Serialization(e.to_string())
    }
}
pub fn expect_keys<T: Copy>(
    map: &std::collections::HashMap<String, T>,
    keys: &[&str],
) -> Result<Vec<T>, &'static str> {
    keys.iter().map(|k| map.get(*k).copied().ok_or("missing key")).collect()
}
pub mod debug {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);
    pub fn enable_debug() {
        DEBUG_ENABLED.store(true, Ordering::Relaxed);
    }
    pub fn disable_debug() {
        DEBUG_ENABLED.store(false, Ordering::Relaxed);
    }
    pub fn is_debug_enabled() -> bool {
        DEBUG_ENABLED.load(Ordering::Relaxed)
    }
    pub fn log_query(sql: &str, params_count: usize) {
        if is_debug_enabled() {
            eprintln!("[ByteORM Debug] Executing SQL: {}", sql);
            eprintln!("[ByteORM Debug] Parameters count: {}", params_count);
        }
    }
    pub fn log_result(operation: &str, rows_affected: u64) {
        if is_debug_enabled() {
            eprintln!(
                "[ByteORM Debug] {} - Rows affected: {}", operation, rows_affected
            );
        }
    }
    pub fn log_error(operation: &str, error: &str) {
        if is_debug_enabled() {
            eprintln!("[ByteORM Debug] Error in {}: {}", operation, error);
        }
    }
}
pub trait JsonbExt {
    fn get_value<T>(
        &self,
        key: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        T: serde::de::DeserializeOwned;
    fn get_string(
        &self,
        key: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
    fn get_i64(
        &self,
        key: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>>;
    fn get_bool(
        &self,
        key: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>;
    fn get_or_default<T>(&self, key: &str, default: T) -> T
    where
        T: serde::de::DeserializeOwned;
    fn has_key(&self, key: &str) -> bool;
}
impl JsonbExt for serde_json::Value {
    fn get_value<T>(
        &self,
        key: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        T: serde::de::DeserializeOwned,
    {
        let value = self
            .get(key)
            .ok_or_else(|| {
                Box::<
                    dyn std::error::Error + Send + Sync,
                >::from(format!("Key '{}' not found", key))
            })?;
        serde_json::from_value(value.clone())
            .map_err(|e| Box::<
                dyn std::error::Error + Send + Sync,
            >::from(format!("Failed to parse key '{}': {}", key, e)))
    }
    fn get_string(
        &self,
        key: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match self.get(key) {
            Some(serde_json::Value::String(s)) => Ok(s.clone()),
            Some(v) => {
                serde_json::from_value(v.clone())
                    .map_err(|e| Box::<
                        dyn std::error::Error + Send + Sync,
                    >::from(format!("Failed to parse '{}' as string: {}", key, e)))
            }
            None => {
                Err(
                    Box::<
                        dyn std::error::Error + Send + Sync,
                    >::from(format!("Key '{}' not found", key)),
                )
            }
        }
    }
    fn get_i64(
        &self,
        key: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        match self.get(key) {
            Some(serde_json::Value::Number(n)) => {
                n.as_i64()
                    .ok_or_else(|| {
                        Box::<
                            dyn std::error::Error + Send + Sync,
                        >::from(format!("Key '{}' is not a valid i64", key))
                    })
            }
            Some(v) => {
                serde_json::from_value(v.clone())
                    .map_err(|e| Box::<
                        dyn std::error::Error + Send + Sync,
                    >::from(format!("Failed to parse '{}' as i64: {}", key, e)))
            }
            None => {
                Err(
                    Box::<
                        dyn std::error::Error + Send + Sync,
                    >::from(format!("Key '{}' not found", key)),
                )
            }
        }
    }
    fn get_bool(
        &self,
        key: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        match self.get(key) {
            Some(serde_json::Value::Bool(b)) => Ok(*b),
            Some(v) => {
                serde_json::from_value(v.clone())
                    .map_err(|e| Box::<
                        dyn std::error::Error + Send + Sync,
                    >::from(format!("Failed to parse '{}' as bool: {}", key, e)))
            }
            None => {
                Err(
                    Box::<
                        dyn std::error::Error + Send + Sync,
                    >::from(format!("Key '{}' not found", key)),
                )
            }
        }
    }
    fn get_or_default<T>(&self, key: &str, default: T) -> T
    where
        T: serde::de::DeserializeOwned,
    {
        self.get_value(key).unwrap_or(default)
    }
    fn has_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
}
#[derive(Clone)]
pub struct UsersAccessor {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
}
impl std::fmt::Debug for UsersAccessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(UsersAccessor)).field("pool", &"<bb8::Pool>").finish()
    }
}
impl UsersAccessor {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self { pool: pool.clone() }
    }
    pub async fn find_many<F>(
        &self,
        f: F,
    ) -> Result<Vec<Users>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(UsersWhereBuilder) -> UsersWhereBuilder,
    {
        let builder = f(UsersWhereBuilder::new());
        UsersQuery::from_builder(self.pool.clone(), builder).await
    }
    pub async fn find_first<F>(
        &self,
        f: F,
    ) -> Result<Option<Users>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(UsersWhereBuilder) -> UsersWhereBuilder,
    {
        let builder = f(UsersWhereBuilder::new());
        UsersQuery::from_builder(self.pool.clone(), builder).first().await
    }
    pub async fn sum<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(UsersWhereBuilder) -> UsersWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let builder = f(UsersWhereBuilder::new());
        UsersQuery::from_builder(self.pool.clone(), builder).sum(field).await
    }
    pub async fn count<F>(
        &self,
        f: F,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(UsersWhereBuilder) -> UsersWhereBuilder,
    {
        let builder = f(UsersWhereBuilder::new());
        UsersQuery::from_builder(self.pool.clone(), builder).count().await
    }
    pub async fn avg<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(UsersWhereBuilder) -> UsersWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(UsersWhereBuilder::new());
        UsersQuery::from_builder(self.pool.clone(), builder).avg::<T>(field).await
    }
    pub async fn min<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(UsersWhereBuilder) -> UsersWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(UsersWhereBuilder::new());
        UsersQuery::from_builder(self.pool.clone(), builder).min::<T>(field).await
    }
    pub async fn max<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(UsersWhereBuilder) -> UsersWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(UsersWhereBuilder::new());
        UsersQuery::from_builder(self.pool.clone(), builder).max::<T>(field).await
    }
    pub async fn sum_flags<F>(
        &self,
        f: F,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(UsersWhereBuilder) -> UsersWhereBuilder,
    {
        let builder = f(UsersWhereBuilder::new());
        UsersQuery::from_builder(self.pool.clone(), builder).sum::<i64>("flags").await
    }
    pub fn update<F>(&self, f: F) -> UsersUpdate
    where
        F: FnOnce(UsersUpdate) -> UsersUpdate,
    {
        let builder = UsersUpdate::new(self.pool.clone());
        f(builder)
    }
    pub fn upsert<F>(&self, f: F) -> UsersUpsert
    where
        F: FnOnce(UsersUpsert) -> UsersUpsert,
    {
        let builder = UsersUpsert::new(self.pool.clone());
        f(builder)
    }
    pub fn create<F>(&self, f: F) -> UsersCreate
    where
        F: FnOnce(UsersCreate) -> UsersCreate,
    {
        let builder = UsersCreate::new(self.pool.clone());
        f(builder)
    }
    pub fn delete<F>(&self, f: F) -> UsersDelete
    where
        F: FnOnce(UsersDelete) -> UsersDelete,
    {
        let builder = UsersDelete::new(self.pool.clone());
        f(builder)
    }
    pub async fn create_many(
        &self,
        records: Vec<
            std::collections::HashMap<
                &'static str,
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            >,
        >,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        if records.is_empty() {
            return Ok(0);
        }
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let first = &records[0];
        let mut columns: Vec<&str> = first.keys().copied().collect();
        columns.sort();
        let columns_str = columns.join(", ");
        let mut all_values: Vec<String> = Vec::with_capacity(records.len());
        let mut all_params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
        let mut param_idx = 1;
        for record in records {
            let placeholders: Vec<String> = columns
                .iter()
                .map(|_| {
                    let p = format!("${}", param_idx);
                    param_idx += 1;
                    p
                })
                .collect();
            all_values.push(format!("({})", placeholders.join(", ")));
            for col in &columns {
                if let Some(val) = record.get(col) {
                    all_params.push(unsafe { std::ptr::read(val) });
                }
            }
        }
        let sql = format!(
            "INSERT INTO {} ({}) VALUES {}", "users", columns_str, all_values.join(", ")
        );
        debug::log_query(&sql, all_params.len());
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let result = client.execute(&sql, &params[..]).await?;
        Ok(result)
    }
    pub async fn find_unique(
        &self,
        id: String,
    ) -> Result<Option<Users>, Box<dyn std::error::Error + Send + Sync>> {
        Users::find_by_id(self.pool.clone(), id).await
    }
    pub async fn find_or_create(
        &self,
        id: String,
    ) -> Result<Users, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ($1) ON CONFLICT ({}) DO NOTHING RETURNING *",
            "users", "id", "id"
        );
        debug::log_query(&sql, 1);
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let rows = client.query(&sql, &[&id]).await?;
        if let Some(row) = rows.first() {
            Ok(Users {
                id: row.get("id"),
                username: row.get("username"),
                tag: row.get("tag"),
                created_at: row.get("created_at"),
                bot: row.get("bot"),
                status: row.get("status"),
                flags: row.get("flags"),
                bio: row.get("bio"),
                avatar: row.get("avatar"),
                banner: row.get("banner"),
            })
        } else {
            self.find_unique(id)
                .await?
                .ok_or("Record should exist after find_or_create".into())
        }
    }
    pub async fn get_client(
        &self,
    ) -> Result<
        bb8::PooledConnection<
            '_,
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
        tokio_postgres::Error,
    > {
        self.pool.get().await.map_err(|_| tokio_postgres::Error::__private_api_timeout())
    }
    pub fn pool(
        &self,
    ) -> &bb8::Pool<
        bb8_postgres::PostgresConnectionManager<tokio_postgres_rustls::MakeRustlsConnect>,
    > {
        &self.pool
    }
}
#[derive(Clone)]
pub struct AccountsAccessor {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
}
impl std::fmt::Debug for AccountsAccessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(AccountsAccessor))
            .field("pool", &"<bb8::Pool>")
            .finish()
    }
}
impl AccountsAccessor {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self { pool: pool.clone() }
    }
    pub async fn find_many<F>(
        &self,
        f: F,
    ) -> Result<Vec<Accounts>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(AccountsWhereBuilder) -> AccountsWhereBuilder,
    {
        let builder = f(AccountsWhereBuilder::new());
        AccountsQuery::from_builder(self.pool.clone(), builder).await
    }
    pub async fn find_first<F>(
        &self,
        f: F,
    ) -> Result<Option<Accounts>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(AccountsWhereBuilder) -> AccountsWhereBuilder,
    {
        let builder = f(AccountsWhereBuilder::new());
        AccountsQuery::from_builder(self.pool.clone(), builder).first().await
    }
    pub async fn sum<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(AccountsWhereBuilder) -> AccountsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let builder = f(AccountsWhereBuilder::new());
        AccountsQuery::from_builder(self.pool.clone(), builder).sum(field).await
    }
    pub async fn count<F>(
        &self,
        f: F,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(AccountsWhereBuilder) -> AccountsWhereBuilder,
    {
        let builder = f(AccountsWhereBuilder::new());
        AccountsQuery::from_builder(self.pool.clone(), builder).count().await
    }
    pub async fn avg<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(AccountsWhereBuilder) -> AccountsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(AccountsWhereBuilder::new());
        AccountsQuery::from_builder(self.pool.clone(), builder).avg::<T>(field).await
    }
    pub async fn min<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(AccountsWhereBuilder) -> AccountsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(AccountsWhereBuilder::new());
        AccountsQuery::from_builder(self.pool.clone(), builder).min::<T>(field).await
    }
    pub async fn max<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(AccountsWhereBuilder) -> AccountsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(AccountsWhereBuilder::new());
        AccountsQuery::from_builder(self.pool.clone(), builder).max::<T>(field).await
    }
    pub fn update<F>(&self, f: F) -> AccountsUpdate
    where
        F: FnOnce(AccountsUpdate) -> AccountsUpdate,
    {
        let builder = AccountsUpdate::new(self.pool.clone());
        f(builder)
    }
    pub fn upsert<F>(&self, f: F) -> AccountsUpsert
    where
        F: FnOnce(AccountsUpsert) -> AccountsUpsert,
    {
        let builder = AccountsUpsert::new(self.pool.clone());
        f(builder)
    }
    pub fn create<F>(&self, f: F) -> AccountsCreate
    where
        F: FnOnce(AccountsCreate) -> AccountsCreate,
    {
        let builder = AccountsCreate::new(self.pool.clone());
        f(builder)
    }
    pub fn delete<F>(&self, f: F) -> AccountsDelete
    where
        F: FnOnce(AccountsDelete) -> AccountsDelete,
    {
        let builder = AccountsDelete::new(self.pool.clone());
        f(builder)
    }
    pub async fn create_many(
        &self,
        records: Vec<
            std::collections::HashMap<
                &'static str,
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            >,
        >,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        if records.is_empty() {
            return Ok(0);
        }
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let first = &records[0];
        let mut columns: Vec<&str> = first.keys().copied().collect();
        columns.sort();
        let columns_str = columns.join(", ");
        let mut all_values: Vec<String> = Vec::with_capacity(records.len());
        let mut all_params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
        let mut param_idx = 1;
        for record in records {
            let placeholders: Vec<String> = columns
                .iter()
                .map(|_| {
                    let p = format!("${}", param_idx);
                    param_idx += 1;
                    p
                })
                .collect();
            all_values.push(format!("({})", placeholders.join(", ")));
            for col in &columns {
                if let Some(val) = record.get(col) {
                    all_params.push(unsafe { std::ptr::read(val) });
                }
            }
        }
        let sql = format!(
            "INSERT INTO {} ({}) VALUES {}", "accounts", columns_str, all_values
            .join(", ")
        );
        debug::log_query(&sql, all_params.len());
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let result = client.execute(&sql, &params[..]).await?;
        Ok(result)
    }
    pub async fn find_unique(
        &self,
        id: String,
    ) -> Result<Option<Accounts>, Box<dyn std::error::Error + Send + Sync>> {
        Accounts::find_by_id(self.pool.clone(), id).await
    }
    pub async fn find_or_create(
        &self,
        id: String,
    ) -> Result<Accounts, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ($1) ON CONFLICT ({}) DO NOTHING RETURNING *",
            "accounts", "id", "id"
        );
        debug::log_query(&sql, 1);
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let rows = client.query(&sql, &[&id]).await?;
        if let Some(row) = rows.first() {
            Ok(Accounts {
                id: row.get("id"),
                email: row.get("email"),
                password: row.get("password"),
                user_id: row.get("user_id"),
                email_verified: row.get("email_verified"),
                locale: row.get("locale"),
                token: row.get("token"),
            })
        } else {
            self.find_unique(id)
                .await?
                .ok_or("Record should exist after find_or_create".into())
        }
    }
    pub async fn get_client(
        &self,
    ) -> Result<
        bb8::PooledConnection<
            '_,
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
        tokio_postgres::Error,
    > {
        self.pool.get().await.map_err(|_| tokio_postgres::Error::__private_api_timeout())
    }
    pub fn pool(
        &self,
    ) -> &bb8::Pool<
        bb8_postgres::PostgresConnectionManager<tokio_postgres_rustls::MakeRustlsConnect>,
    > {
        &self.pool
    }
}
#[derive(Clone)]
pub struct GuildsAccessor {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
}
impl std::fmt::Debug for GuildsAccessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(GuildsAccessor)).field("pool", &"<bb8::Pool>").finish()
    }
}
impl GuildsAccessor {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self { pool: pool.clone() }
    }
    pub async fn find_many<F>(
        &self,
        f: F,
    ) -> Result<Vec<Guilds>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildsWhereBuilder) -> GuildsWhereBuilder,
    {
        let builder = f(GuildsWhereBuilder::new());
        GuildsQuery::from_builder(self.pool.clone(), builder).await
    }
    pub async fn find_first<F>(
        &self,
        f: F,
    ) -> Result<Option<Guilds>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildsWhereBuilder) -> GuildsWhereBuilder,
    {
        let builder = f(GuildsWhereBuilder::new());
        GuildsQuery::from_builder(self.pool.clone(), builder).first().await
    }
    pub async fn sum<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildsWhereBuilder) -> GuildsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let builder = f(GuildsWhereBuilder::new());
        GuildsQuery::from_builder(self.pool.clone(), builder).sum(field).await
    }
    pub async fn count<F>(
        &self,
        f: F,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildsWhereBuilder) -> GuildsWhereBuilder,
    {
        let builder = f(GuildsWhereBuilder::new());
        GuildsQuery::from_builder(self.pool.clone(), builder).count().await
    }
    pub async fn avg<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildsWhereBuilder) -> GuildsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(GuildsWhereBuilder::new());
        GuildsQuery::from_builder(self.pool.clone(), builder).avg::<T>(field).await
    }
    pub async fn min<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildsWhereBuilder) -> GuildsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(GuildsWhereBuilder::new());
        GuildsQuery::from_builder(self.pool.clone(), builder).min::<T>(field).await
    }
    pub async fn max<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildsWhereBuilder) -> GuildsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(GuildsWhereBuilder::new());
        GuildsQuery::from_builder(self.pool.clone(), builder).max::<T>(field).await
    }
    pub fn update<F>(&self, f: F) -> GuildsUpdate
    where
        F: FnOnce(GuildsUpdate) -> GuildsUpdate,
    {
        let builder = GuildsUpdate::new(self.pool.clone());
        f(builder)
    }
    pub fn upsert<F>(&self, f: F) -> GuildsUpsert
    where
        F: FnOnce(GuildsUpsert) -> GuildsUpsert,
    {
        let builder = GuildsUpsert::new(self.pool.clone());
        f(builder)
    }
    pub fn create<F>(&self, f: F) -> GuildsCreate
    where
        F: FnOnce(GuildsCreate) -> GuildsCreate,
    {
        let builder = GuildsCreate::new(self.pool.clone());
        f(builder)
    }
    pub fn delete<F>(&self, f: F) -> GuildsDelete
    where
        F: FnOnce(GuildsDelete) -> GuildsDelete,
    {
        let builder = GuildsDelete::new(self.pool.clone());
        f(builder)
    }
    pub async fn create_many(
        &self,
        records: Vec<
            std::collections::HashMap<
                &'static str,
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            >,
        >,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        if records.is_empty() {
            return Ok(0);
        }
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let first = &records[0];
        let mut columns: Vec<&str> = first.keys().copied().collect();
        columns.sort();
        let columns_str = columns.join(", ");
        let mut all_values: Vec<String> = Vec::with_capacity(records.len());
        let mut all_params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
        let mut param_idx = 1;
        for record in records {
            let placeholders: Vec<String> = columns
                .iter()
                .map(|_| {
                    let p = format!("${}", param_idx);
                    param_idx += 1;
                    p
                })
                .collect();
            all_values.push(format!("({})", placeholders.join(", ")));
            for col in &columns {
                if let Some(val) = record.get(col) {
                    all_params.push(unsafe { std::ptr::read(val) });
                }
            }
        }
        let sql = format!(
            "INSERT INTO {} ({}) VALUES {}", "guilds", columns_str, all_values.join(", ")
        );
        debug::log_query(&sql, all_params.len());
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let result = client.execute(&sql, &params[..]).await?;
        Ok(result)
    }
    pub async fn find_unique(
        &self,
        id: String,
    ) -> Result<Option<Guilds>, Box<dyn std::error::Error + Send + Sync>> {
        Guilds::find_by_id(self.pool.clone(), id).await
    }
    pub async fn find_or_create(
        &self,
        id: String,
    ) -> Result<Guilds, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ($1) ON CONFLICT ({}) DO NOTHING RETURNING *",
            "guilds", "id", "id"
        );
        debug::log_query(&sql, 1);
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let rows = client.query(&sql, &[&id]).await?;
        if let Some(row) = rows.first() {
            Ok(Guilds {
                id: row.get("id"),
                name: row.get("name"),
                brief: row.get("brief"),
                icon: row.get("icon"),
                created_at: row.get("created_at"),
                owner_id: row.get("owner_id"),
            })
        } else {
            self.find_unique(id)
                .await?
                .ok_or("Record should exist after find_or_create".into())
        }
    }
    pub async fn get_client(
        &self,
    ) -> Result<
        bb8::PooledConnection<
            '_,
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
        tokio_postgres::Error,
    > {
        self.pool.get().await.map_err(|_| tokio_postgres::Error::__private_api_timeout())
    }
    pub fn pool(
        &self,
    ) -> &bb8::Pool<
        bb8_postgres::PostgresConnectionManager<tokio_postgres_rustls::MakeRustlsConnect>,
    > {
        &self.pool
    }
}
#[derive(Clone)]
pub struct GuildMembersAccessor {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
}
impl std::fmt::Debug for GuildMembersAccessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(GuildMembersAccessor))
            .field("pool", &"<bb8::Pool>")
            .finish()
    }
}
impl GuildMembersAccessor {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self { pool: pool.clone() }
    }
    pub async fn find_many<F>(
        &self,
        f: F,
    ) -> Result<Vec<GuildMembers>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildMembersWhereBuilder) -> GuildMembersWhereBuilder,
    {
        let builder = f(GuildMembersWhereBuilder::new());
        GuildMembersQuery::from_builder(self.pool.clone(), builder).await
    }
    pub async fn find_first<F>(
        &self,
        f: F,
    ) -> Result<Option<GuildMembers>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildMembersWhereBuilder) -> GuildMembersWhereBuilder,
    {
        let builder = f(GuildMembersWhereBuilder::new());
        GuildMembersQuery::from_builder(self.pool.clone(), builder).first().await
    }
    pub async fn sum<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildMembersWhereBuilder) -> GuildMembersWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let builder = f(GuildMembersWhereBuilder::new());
        GuildMembersQuery::from_builder(self.pool.clone(), builder).sum(field).await
    }
    pub async fn count<F>(
        &self,
        f: F,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildMembersWhereBuilder) -> GuildMembersWhereBuilder,
    {
        let builder = f(GuildMembersWhereBuilder::new());
        GuildMembersQuery::from_builder(self.pool.clone(), builder).count().await
    }
    pub async fn avg<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildMembersWhereBuilder) -> GuildMembersWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(GuildMembersWhereBuilder::new());
        GuildMembersQuery::from_builder(self.pool.clone(), builder).avg::<T>(field).await
    }
    pub async fn min<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildMembersWhereBuilder) -> GuildMembersWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(GuildMembersWhereBuilder::new());
        GuildMembersQuery::from_builder(self.pool.clone(), builder).min::<T>(field).await
    }
    pub async fn max<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildMembersWhereBuilder) -> GuildMembersWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(GuildMembersWhereBuilder::new());
        GuildMembersQuery::from_builder(self.pool.clone(), builder).max::<T>(field).await
    }
    pub async fn sum_id<F>(
        &self,
        f: F,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(GuildMembersWhereBuilder) -> GuildMembersWhereBuilder,
    {
        let builder = f(GuildMembersWhereBuilder::new());
        GuildMembersQuery::from_builder(self.pool.clone(), builder)
            .sum::<i64>("id")
            .await
    }
    pub fn update<F>(&self, f: F) -> GuildMembersUpdate
    where
        F: FnOnce(GuildMembersUpdate) -> GuildMembersUpdate,
    {
        let builder = GuildMembersUpdate::new(self.pool.clone());
        f(builder)
    }
    pub fn upsert<F>(&self, f: F) -> GuildMembersUpsert
    where
        F: FnOnce(GuildMembersUpsert) -> GuildMembersUpsert,
    {
        let builder = GuildMembersUpsert::new(self.pool.clone());
        f(builder)
    }
    pub fn create<F>(&self, f: F) -> GuildMembersCreate
    where
        F: FnOnce(GuildMembersCreate) -> GuildMembersCreate,
    {
        let builder = GuildMembersCreate::new(self.pool.clone());
        f(builder)
    }
    pub fn delete<F>(&self, f: F) -> GuildMembersDelete
    where
        F: FnOnce(GuildMembersDelete) -> GuildMembersDelete,
    {
        let builder = GuildMembersDelete::new(self.pool.clone());
        f(builder)
    }
    pub async fn create_many(
        &self,
        records: Vec<
            std::collections::HashMap<
                &'static str,
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            >,
        >,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        if records.is_empty() {
            return Ok(0);
        }
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let first = &records[0];
        let mut columns: Vec<&str> = first.keys().copied().collect();
        columns.sort();
        let columns_str = columns.join(", ");
        let mut all_values: Vec<String> = Vec::with_capacity(records.len());
        let mut all_params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
        let mut param_idx = 1;
        for record in records {
            let placeholders: Vec<String> = columns
                .iter()
                .map(|_| {
                    let p = format!("${}", param_idx);
                    param_idx += 1;
                    p
                })
                .collect();
            all_values.push(format!("({})", placeholders.join(", ")));
            for col in &columns {
                if let Some(val) = record.get(col) {
                    all_params.push(unsafe { std::ptr::read(val) });
                }
            }
        }
        let sql = format!(
            "INSERT INTO {} ({}) VALUES {}", "guildmembers", columns_str, all_values
            .join(", ")
        );
        debug::log_query(&sql, all_params.len());
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let result = client.execute(&sql, &params[..]).await?;
        Ok(result)
    }
    pub async fn find_unique(
        &self,
        id: i32,
    ) -> Result<Option<GuildMembers>, Box<dyn std::error::Error + Send + Sync>> {
        GuildMembers::find_by_id(self.pool.clone(), id).await
    }
    pub async fn find_or_create(
        &self,
        id: i32,
    ) -> Result<GuildMembers, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ($1) ON CONFLICT ({}) DO NOTHING RETURNING *",
            "guildmembers", "id", "id"
        );
        debug::log_query(&sql, 1);
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let rows = client.query(&sql, &[&id]).await?;
        if let Some(row) = rows.first() {
            Ok(GuildMembers {
                id: row.get("id"),
                guild_id: row.get("guild_id"),
                user_id: row.get("user_id"),
                nickname: row.get("nickname"),
                joined_at: row.get("joined_at"),
            })
        } else {
            self.find_unique(id)
                .await?
                .ok_or("Record should exist after find_or_create".into())
        }
    }
    pub async fn get_client(
        &self,
    ) -> Result<
        bb8::PooledConnection<
            '_,
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
        tokio_postgres::Error,
    > {
        self.pool.get().await.map_err(|_| tokio_postgres::Error::__private_api_timeout())
    }
    pub fn pool(
        &self,
    ) -> &bb8::Pool<
        bb8_postgres::PostgresConnectionManager<tokio_postgres_rustls::MakeRustlsConnect>,
    > {
        &self.pool
    }
}
#[derive(Clone)]
pub struct ChannelsAccessor {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
}
impl std::fmt::Debug for ChannelsAccessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(ChannelsAccessor))
            .field("pool", &"<bb8::Pool>")
            .finish()
    }
}
impl ChannelsAccessor {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self { pool: pool.clone() }
    }
    pub async fn find_many<F>(
        &self,
        f: F,
    ) -> Result<Vec<Channels>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(ChannelsWhereBuilder) -> ChannelsWhereBuilder,
    {
        let builder = f(ChannelsWhereBuilder::new());
        ChannelsQuery::from_builder(self.pool.clone(), builder).await
    }
    pub async fn find_first<F>(
        &self,
        f: F,
    ) -> Result<Option<Channels>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(ChannelsWhereBuilder) -> ChannelsWhereBuilder,
    {
        let builder = f(ChannelsWhereBuilder::new());
        ChannelsQuery::from_builder(self.pool.clone(), builder).first().await
    }
    pub async fn sum<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(ChannelsWhereBuilder) -> ChannelsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let builder = f(ChannelsWhereBuilder::new());
        ChannelsQuery::from_builder(self.pool.clone(), builder).sum(field).await
    }
    pub async fn count<F>(
        &self,
        f: F,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(ChannelsWhereBuilder) -> ChannelsWhereBuilder,
    {
        let builder = f(ChannelsWhereBuilder::new());
        ChannelsQuery::from_builder(self.pool.clone(), builder).count().await
    }
    pub async fn avg<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(ChannelsWhereBuilder) -> ChannelsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(ChannelsWhereBuilder::new());
        ChannelsQuery::from_builder(self.pool.clone(), builder).avg::<T>(field).await
    }
    pub async fn min<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(ChannelsWhereBuilder) -> ChannelsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(ChannelsWhereBuilder::new());
        ChannelsQuery::from_builder(self.pool.clone(), builder).min::<T>(field).await
    }
    pub async fn max<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(ChannelsWhereBuilder) -> ChannelsWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(ChannelsWhereBuilder::new());
        ChannelsQuery::from_builder(self.pool.clone(), builder).max::<T>(field).await
    }
    pub async fn sum_rate_limit_per_user<F>(
        &self,
        f: F,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(ChannelsWhereBuilder) -> ChannelsWhereBuilder,
    {
        let builder = f(ChannelsWhereBuilder::new());
        ChannelsQuery::from_builder(self.pool.clone(), builder)
            .sum::<i64>("rate_limit_per_user")
            .await
    }
    pub fn update<F>(&self, f: F) -> ChannelsUpdate
    where
        F: FnOnce(ChannelsUpdate) -> ChannelsUpdate,
    {
        let builder = ChannelsUpdate::new(self.pool.clone());
        f(builder)
    }
    pub fn upsert<F>(&self, f: F) -> ChannelsUpsert
    where
        F: FnOnce(ChannelsUpsert) -> ChannelsUpsert,
    {
        let builder = ChannelsUpsert::new(self.pool.clone());
        f(builder)
    }
    pub fn create<F>(&self, f: F) -> ChannelsCreate
    where
        F: FnOnce(ChannelsCreate) -> ChannelsCreate,
    {
        let builder = ChannelsCreate::new(self.pool.clone());
        f(builder)
    }
    pub fn delete<F>(&self, f: F) -> ChannelsDelete
    where
        F: FnOnce(ChannelsDelete) -> ChannelsDelete,
    {
        let builder = ChannelsDelete::new(self.pool.clone());
        f(builder)
    }
    pub async fn create_many(
        &self,
        records: Vec<
            std::collections::HashMap<
                &'static str,
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            >,
        >,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        if records.is_empty() {
            return Ok(0);
        }
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let first = &records[0];
        let mut columns: Vec<&str> = first.keys().copied().collect();
        columns.sort();
        let columns_str = columns.join(", ");
        let mut all_values: Vec<String> = Vec::with_capacity(records.len());
        let mut all_params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
        let mut param_idx = 1;
        for record in records {
            let placeholders: Vec<String> = columns
                .iter()
                .map(|_| {
                    let p = format!("${}", param_idx);
                    param_idx += 1;
                    p
                })
                .collect();
            all_values.push(format!("({})", placeholders.join(", ")));
            for col in &columns {
                if let Some(val) = record.get(col) {
                    all_params.push(unsafe { std::ptr::read(val) });
                }
            }
        }
        let sql = format!(
            "INSERT INTO {} ({}) VALUES {}", "channels", columns_str, all_values
            .join(", ")
        );
        debug::log_query(&sql, all_params.len());
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let result = client.execute(&sql, &params[..]).await?;
        Ok(result)
    }
    pub async fn find_unique(
        &self,
        id: String,
    ) -> Result<Option<Channels>, Box<dyn std::error::Error + Send + Sync>> {
        Channels::find_by_id(self.pool.clone(), id).await
    }
    pub async fn find_or_create(
        &self,
        id: String,
    ) -> Result<Channels, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ($1) ON CONFLICT ({}) DO NOTHING RETURNING *",
            "channels", "id", "id"
        );
        debug::log_query(&sql, 1);
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let rows = client.query(&sql, &[&id]).await?;
        if let Some(row) = rows.first() {
            Ok(Channels {
                id: row.get("id"),
                name: row.get("name"),
                guild_id: row.get("guild_id"),
                created_at: row.get("created_at"),
                rate_limit_per_user: row.get("rate_limit_per_user"),
            })
        } else {
            self.find_unique(id)
                .await?
                .ok_or("Record should exist after find_or_create".into())
        }
    }
    pub async fn get_client(
        &self,
    ) -> Result<
        bb8::PooledConnection<
            '_,
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
        tokio_postgres::Error,
    > {
        self.pool.get().await.map_err(|_| tokio_postgres::Error::__private_api_timeout())
    }
    pub fn pool(
        &self,
    ) -> &bb8::Pool<
        bb8_postgres::PostgresConnectionManager<tokio_postgres_rustls::MakeRustlsConnect>,
    > {
        &self.pool
    }
}
#[derive(Clone)]
pub struct MessagesAccessor {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
}
impl std::fmt::Debug for MessagesAccessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(MessagesAccessor))
            .field("pool", &"<bb8::Pool>")
            .finish()
    }
}
impl MessagesAccessor {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self { pool: pool.clone() }
    }
    pub async fn find_many<F>(
        &self,
        f: F,
    ) -> Result<Vec<Messages>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(MessagesWhereBuilder) -> MessagesWhereBuilder,
    {
        let builder = f(MessagesWhereBuilder::new());
        MessagesQuery::from_builder(self.pool.clone(), builder).await
    }
    pub async fn find_first<F>(
        &self,
        f: F,
    ) -> Result<Option<Messages>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(MessagesWhereBuilder) -> MessagesWhereBuilder,
    {
        let builder = f(MessagesWhereBuilder::new());
        MessagesQuery::from_builder(self.pool.clone(), builder).first().await
    }
    pub async fn sum<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(MessagesWhereBuilder) -> MessagesWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let builder = f(MessagesWhereBuilder::new());
        MessagesQuery::from_builder(self.pool.clone(), builder).sum(field).await
    }
    pub async fn count<F>(
        &self,
        f: F,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(MessagesWhereBuilder) -> MessagesWhereBuilder,
    {
        let builder = f(MessagesWhereBuilder::new());
        MessagesQuery::from_builder(self.pool.clone(), builder).count().await
    }
    pub async fn avg<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(MessagesWhereBuilder) -> MessagesWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(MessagesWhereBuilder::new());
        MessagesQuery::from_builder(self.pool.clone(), builder).avg::<T>(field).await
    }
    pub async fn min<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(MessagesWhereBuilder) -> MessagesWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(MessagesWhereBuilder::new());
        MessagesQuery::from_builder(self.pool.clone(), builder).min::<T>(field).await
    }
    pub async fn max<F, T>(
        &self,
        f: F,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(MessagesWhereBuilder) -> MessagesWhereBuilder,
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let builder = f(MessagesWhereBuilder::new());
        MessagesQuery::from_builder(self.pool.clone(), builder).max::<T>(field).await
    }
    pub fn update<F>(&self, f: F) -> MessagesUpdate
    where
        F: FnOnce(MessagesUpdate) -> MessagesUpdate,
    {
        let builder = MessagesUpdate::new(self.pool.clone());
        f(builder)
    }
    pub fn upsert<F>(&self, f: F) -> MessagesUpsert
    where
        F: FnOnce(MessagesUpsert) -> MessagesUpsert,
    {
        let builder = MessagesUpsert::new(self.pool.clone());
        f(builder)
    }
    pub fn create<F>(&self, f: F) -> MessagesCreate
    where
        F: FnOnce(MessagesCreate) -> MessagesCreate,
    {
        let builder = MessagesCreate::new(self.pool.clone());
        f(builder)
    }
    pub fn delete<F>(&self, f: F) -> MessagesDelete
    where
        F: FnOnce(MessagesDelete) -> MessagesDelete,
    {
        let builder = MessagesDelete::new(self.pool.clone());
        f(builder)
    }
    pub async fn create_many(
        &self,
        records: Vec<
            std::collections::HashMap<
                &'static str,
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            >,
        >,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        if records.is_empty() {
            return Ok(0);
        }
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let first = &records[0];
        let mut columns: Vec<&str> = first.keys().copied().collect();
        columns.sort();
        let columns_str = columns.join(", ");
        let mut all_values: Vec<String> = Vec::with_capacity(records.len());
        let mut all_params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
        let mut param_idx = 1;
        for record in records {
            let placeholders: Vec<String> = columns
                .iter()
                .map(|_| {
                    let p = format!("${}", param_idx);
                    param_idx += 1;
                    p
                })
                .collect();
            all_values.push(format!("({})", placeholders.join(", ")));
            for col in &columns {
                if let Some(val) = record.get(col) {
                    all_params.push(unsafe { std::ptr::read(val) });
                }
            }
        }
        let sql = format!(
            "INSERT INTO {} ({}) VALUES {}", "messages", columns_str, all_values
            .join(", ")
        );
        debug::log_query(&sql, all_params.len());
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let result = client.execute(&sql, &params[..]).await?;
        Ok(result)
    }
    pub async fn find_unique(
        &self,
        id: String,
    ) -> Result<Option<Messages>, Box<dyn std::error::Error + Send + Sync>> {
        Messages::find_by_id(self.pool.clone(), id).await
    }
    pub async fn find_or_create(
        &self,
        id: String,
    ) -> Result<Messages, Box<dyn std::error::Error + Send + Sync>> {
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ($1) ON CONFLICT ({}) DO NOTHING RETURNING *",
            "messages", "id", "id"
        );
        debug::log_query(&sql, 1);
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let rows = client.query(&sql, &[&id]).await?;
        if let Some(row) = rows.first() {
            Ok(Messages {
                id: row.get("id"),
                author_id: row.get("author_id"),
                channel_id: row.get("channel_id"),
                guild_id: row.get("guild_id"),
                content: row.get("content"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
                message_type: row.get("message_type"),
                nonce: row.get("nonce"),
            })
        } else {
            self.find_unique(id)
                .await?
                .ok_or("Record should exist after find_or_create".into())
        }
    }
    pub async fn get_client(
        &self,
    ) -> Result<
        bb8::PooledConnection<
            '_,
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
        tokio_postgres::Error,
    > {
        self.pool.get().await.map_err(|_| tokio_postgres::Error::__private_api_timeout())
    }
    pub fn pool(
        &self,
    ) -> &bb8::Pool<
        bb8_postgres::PostgresConnectionManager<tokio_postgres_rustls::MakeRustlsConnect>,
    > {
        &self.pool
    }
}
#[derive(Clone)]
pub struct Client {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    pub users: UsersAccessor,
    pub accounts: AccountsAccessor,
    pub guilds: GuildsAccessor,
    pub guild_members: GuildMembersAccessor,
    pub channels: ChannelsAccessor,
    pub messages: MessagesAccessor,
}
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("pool", &"<bb8::Pool>")
            .field("users", &self.users)
            .field("accounts", &self.accounts)
            .field("guilds", &self.guilds)
            .field("guild_members", &self.guild_members)
            .field("channels", &self.channels)
            .field("messages", &self.messages)
            .finish()
    }
}
impl Client {
    pub async fn new(connection_string: &str) -> Result<Self, Error> {
        let root_store = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.iter().cloned().collect(),
        };
        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
        let manager = bb8_postgres::PostgresConnectionManager::new_from_stringlike(
            connection_string,
            tls,
        )?;
        let pool = bb8::Pool::builder().max_size(20).build(manager).await?;
        let pool = Arc::new(pool);
        Ok(Self {
            pool: pool.clone(),
            users: UsersAccessor::new(pool.clone()),
            accounts: AccountsAccessor::new(pool.clone()),
            guilds: GuildsAccessor::new(pool.clone()),
            guild_members: GuildMembersAccessor::new(pool.clone()),
            channels: ChannelsAccessor::new(pool.clone()),
            messages: MessagesAccessor::new(pool.clone()),
        })
    }
    pub async fn get_client(
        &self,
    ) -> Result<
        bb8::PooledConnection<
            '_,
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
        Error,
    > {
        self.pool.get().await.map_err(|_| Error::__private_api_timeout())
    }
    pub fn pool(
        &self,
    ) -> &bb8::Pool<
        bb8_postgres::PostgresConnectionManager<tokio_postgres_rustls::MakeRustlsConnect>,
    > {
        &self.pool
    }
    pub async fn transaction<F, T, E>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(
                Transaction<'_>,
            ) -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<T, E>> + Send + '_>,
                > + Send,
        E: From<tokio_postgres::Error> + Send,
        T: Send,
    {
        let mut client = self
            .pool
            .get()
            .await
            .map_err(|_| tokio_postgres::Error::__private_api_timeout())?;
        let tx = client.transaction().await?;
        let transaction = Transaction { inner: tx };
        let result = f(transaction).await;
        match &result {
            Ok(_) => {
                client.transaction().await?.commit().await.ok();
            }
            Err(_) => {}
        }
        result
    }
    pub async fn execute_raw(
        &self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<u64, Error> {
        let client = self.pool.get().await.map_err(|_| Error::__private_api_timeout())?;
        client.execute(sql, params).await
    }
    pub async fn query_raw(
        &self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<tokio_postgres::Row>, Error> {
        let client = self.pool.get().await.map_err(|_| Error::__private_api_timeout())?;
        client.query(sql, params).await
    }
}
pub struct Transaction<'a> {
    inner: tokio_postgres::Transaction<'a>,
}
impl<'a> Transaction<'a> {
    pub async fn execute(
        &self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<u64, Error> {
        self.inner.execute(sql, params).await
    }
    pub async fn query(
        &self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<tokio_postgres::Row>, Error> {
        self.inner.query(sql, params).await
    }
    pub async fn commit(self) -> Result<(), Error> {
        self.inner.commit().await
    }
    pub async fn rollback(self) -> Result<(), Error> {
        self.inner.rollback().await
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Users {
    pub id: String,
    pub username: String,
    pub tag: String,
    pub created_at: DateTime<Utc>,
    pub bot: bool,
    pub status: String,
    pub flags: i32,
    pub bio: Option<String>,
    pub avatar: Option<String>,
    pub banner: Option<String>,
}
pub struct UsersWhereBuilder {
    where_clauses: Vec<String>,
    order_by: Vec<(String, String)>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
}
impl UsersWhereBuilder {
    pub fn new() -> Self {
        Self {
            where_clauses: vec![],
            order_by: vec![],
            args: vec![],
            limit: None,
            offset: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_username(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "username", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_username_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "username", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_tag(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "tag", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_tag_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "tag", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "created_at", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} > ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} < ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} >= ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} <= ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_bot(mut self, value: bool) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "bot", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_bot_in(mut self, values: Vec<bool>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "bot", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_status(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "status", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_status_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "status", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_flags(mut self, value: i32) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "flags", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_flags_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "flags", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_bio(mut self, value: Option<String>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "bio", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_bio_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "bio", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_bio_is_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NULL", "bio"));
        self
    }
    pub fn where_bio_is_not_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NOT NULL", "bio"));
        self
    }
    pub fn where_avatar(mut self, value: Option<String>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "avatar", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_avatar_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "avatar", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_avatar_is_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NULL", "avatar"));
        self
    }
    pub fn where_avatar_is_not_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NOT NULL", "avatar"));
        self
    }
    pub fn where_banner(mut self, value: Option<String>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "banner", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_banner_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "banner", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_banner_is_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NULL", "banner"));
        self
    }
    pub fn where_banner_is_not_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NOT NULL", "banner"));
        self
    }
    pub fn order_by_id_asc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_id_desc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_username_asc(mut self) -> Self {
        self.order_by.push(("username".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_username_desc(mut self) -> Self {
        self.order_by.push(("username".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_tag_asc(mut self) -> Self {
        self.order_by.push(("tag".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_tag_desc(mut self) -> Self {
        self.order_by.push(("tag".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_created_at_asc(mut self) -> Self {
        self.order_by.push(("created_at".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_created_at_desc(mut self) -> Self {
        self.order_by.push(("created_at".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_bot_asc(mut self) -> Self {
        self.order_by.push(("bot".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_bot_desc(mut self) -> Self {
        self.order_by.push(("bot".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_status_asc(mut self) -> Self {
        self.order_by.push(("status".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_status_desc(mut self) -> Self {
        self.order_by.push(("status".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_flags_asc(mut self) -> Self {
        self.order_by.push(("flags".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_flags_desc(mut self) -> Self {
        self.order_by.push(("flags".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_bio_asc(mut self) -> Self {
        self.order_by.push(("bio".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_bio_desc(mut self) -> Self {
        self.order_by.push(("bio".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_avatar_asc(mut self) -> Self {
        self.order_by.push(("avatar".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_avatar_desc(mut self) -> Self {
        self.order_by.push(("avatar".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_banner_asc(mut self) -> Self {
        self.order_by.push(("banner".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_banner_desc(mut self) -> Self {
        self.order_by.push(("banner".to_string(), "DESC".to_string()));
        self
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
}
pub struct UsersQuery {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_clauses: Vec<String>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
    order_by: Vec<(String, String)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Vec<Users>, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for UsersQuery {}
impl UsersQuery {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "users".to_string(),
            where_clauses: vec![],
            args: vec![],
            limit: None,
            offset: None,
            order_by: vec![],
            fut: None,
        }
    }
    pub fn from_builder(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        builder: UsersWhereBuilder,
    ) -> Self {
        Self {
            pool,
            table: "users".to_string(),
            where_clauses: builder.where_clauses,
            args: builder.args,
            limit: builder.limit,
            offset: builder.offset,
            order_by: builder.order_by,
            fut: None,
        }
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
    pub async fn first(
        self,
    ) -> Result<Option<Users>, Box<dyn std::error::Error + Send + Sync>> {
        let result = self.limit(1).await?;
        Ok(result.into_iter().next())
    }
    pub async fn count(self) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn aggregate<T>(
        self,
        field: &str,
        func: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let func_upper = func.to_uppercase();
        let mut sql = format!("SELECT {}({}) FROM {}", func_upper, field, self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum<T>(
        self,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let default_val = T::default();
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(SUM({}), ${}) FROM {}", field, default_idx, self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum_cast_i64(
        self,
        field: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let default_val: i64 = 0;
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(CAST(SUM({}) AS BIGINT), ${}) FROM {}", field, default_idx,
            self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn avg<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "AVG").await
    }
    pub async fn min<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MIN").await
    }
    pub async fn max<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MAX").await
    }
}
impl Future for UsersQuery {
    type Output = Result<Vec<Users>, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_clauses = std::mem::take(&mut me.where_clauses);
            let limit = me.limit;
            let offset = me.offset;
            let order_by = std::mem::take(&mut me.order_by);
            let args = std::mem::take(&mut me.args);
            let fut = async move {
                let mut sql = format!("SELECT * FROM {}", table);
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = args
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                if !where_clauses.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&where_clauses.join(" AND "));
                }
                if !order_by.is_empty() {
                    let order_clauses: Vec<String> = order_by
                        .iter()
                        .map(|(col, dir)| format!("{} {}", col, dir))
                        .collect();
                    sql.push_str(" ORDER BY ");
                    sql.push_str(&order_clauses.join(", "));
                }
                if let Some(limit) = limit {
                    sql.push_str(&format!(" LIMIT {}", limit));
                }
                if let Some(offset) = offset {
                    sql.push_str(&format!(" OFFSET {}", offset));
                }
                debug::log_query(&sql, params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let rows = client.query(&sql, &params[..]).await?;
                Ok(
                    rows
                        .into_iter()
                        .map(|row| Users {
                            id: row.get("id"),
                            username: row.get("username"),
                            tag: row.get("tag"),
                            created_at: row.get("created_at"),
                            bot: row.get("bot"),
                            status: row.get("status"),
                            flags: row.get("flags"),
                            bio: row.get("bio"),
                            avatar: row.get("avatar"),
                            banner: row.get("banner"),
                        })
                        .collect(),
                )
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct UsersUpdate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_fragments: Vec<&'static str>,
    set_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    inc_ops: Vec<(&'static str, &'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Users, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for UsersUpdate {}
impl UsersUpdate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "users".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_fragments: vec![],
            set_args: vec![],
            inc_ops: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_username(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("username".to_string(), self.where_args.len()));
        self
    }
    pub fn where_username_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "username", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_tag(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("tag".to_string(), self.where_args.len()));
        self
    }
    pub fn where_tag_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "tag", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_bot(mut self, value: bool) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("bot".to_string(), self.where_args.len()));
        self
    }
    pub fn where_bot_in(mut self, values: Vec<bool>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "bot", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_status(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("status".to_string(), self.where_args.len()));
        self
    }
    pub fn where_status_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "status", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_flags(mut self, value: i32) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("flags".to_string(), self.where_args.len()));
        self
    }
    pub fn where_flags_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "flags", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_bio(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("bio".to_string(), self.where_args.len()));
        self
    }
    pub fn where_bio_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "bio", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_bio_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "bio"), 0));
        self
    }
    pub fn where_bio_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "bio"), 0));
        self
    }
    pub fn where_avatar(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("avatar".to_string(), self.where_args.len()));
        self
    }
    pub fn where_avatar_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "avatar", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_avatar_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "avatar"), 0));
        self
    }
    pub fn where_avatar_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "avatar"), 0));
        self
    }
    pub fn where_banner(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("banner".to_string(), self.where_args.len()));
        self
    }
    pub fn where_banner_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "banner", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_banner_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "banner"), 0));
        self
    }
    pub fn where_banner_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "banner"), 0));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("id");
        self
    }
    pub fn set_username(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("username");
        self
    }
    pub fn set_tag(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("tag");
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("created_at");
        self
    }
    pub fn set_bot(mut self, value: bool) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("bot");
        self
    }
    pub fn set_status(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("status");
        self
    }
    pub fn set_flags(mut self, value: i32) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("flags");
        self
    }
    pub fn set_bio(mut self, value: Option<String>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("bio");
        self
    }
    pub fn set_avatar(mut self, value: Option<String>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("avatar");
        self
    }
    pub fn set_banner(mut self, value: Option<String>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("banner");
        self
    }
    pub fn inc_flags(mut self, amount: i64) -> Self {
        self.inc_ops.push(("flags", "inc", amount));
        self
    }
    pub fn dec_flags(mut self, amount: i64) -> Self {
        self.inc_ops.push(("flags", "dec", amount));
        self
    }
    pub fn mul_flags(mut self, factor: i64) -> Self {
        self.inc_ops.push(("flags", "mul", factor));
        self
    }
    pub fn div_flags(mut self, divisor: i64) -> Self {
        self.inc_ops.push(("flags", "div", divisor));
        self
    }
}
impl std::future::Future for UsersUpdate {
    type Output = Result<Users, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.set_fragments.is_empty() && me.inc_ops.is_empty() {
                return std::task::Poll::Ready(Err("No fields to update".into()));
            }
            let mut sql = format!("UPDATE {} SET ", me.table);
            let mut set_clauses: Vec<String> = vec![];
            let mut param_idx = 1;
            for col in me.set_fragments.iter() {
                set_clauses.push(format!("{} = ${}", col, param_idx));
                param_idx += 1;
            }
            for (field, op, _) in &me.inc_ops {
                let clause = match *op {
                    "inc" => format!("{} = {} + ${}", field, field, param_idx),
                    "dec" => format!("{} = {} - ${}", field, field, param_idx),
                    "mul" => format!("{} = {} * ${}", field, field, param_idx),
                    "div" => format!("{} = {} / ${}", field, field, param_idx),
                    _ => continue,
                };
                set_clauses.push(clause);
                param_idx += 1;
            }
            sql.push_str(&set_clauses.join(", "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.set_args) {
                all_params.push(arg);
            }
            for (_, _, val) in &me.inc_ops {
                all_params.push(Box::new(*val));
            }
            if !me.where_fragments.is_empty() {
                let where_clauses: Vec<String> = me
                    .where_fragments
                    .iter()
                    .enumerate()
                    .map(|(i, (col, _))| format!("{} = ${}", col, param_idx + i))
                    .collect();
                sql.push_str(" WHERE ");
                sql.push_str(&where_clauses.join(" AND "));
                for arg in std::mem::take(&mut me.where_args) {
                    all_params.push(arg);
                }
            }
            sql.push_str(" RETURNING *");
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Users {
                    id: row.get("id"),
                    username: row.get("username"),
                    tag: row.get("tag"),
                    created_at: row.get("created_at"),
                    bot: row.get("bot"),
                    status: row.get("status"),
                    flags: row.get("flags"),
                    bio: row.get("bio"),
                    avatar: row.get("avatar"),
                    banner: row.get("banner"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct UsersUpsert {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    pk_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    inc_ops: std::collections::HashMap<&'static str, (&'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Users, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for UsersUpsert {}
impl UsersUpsert {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "users".to_string(),
            pk_values: std::collections::HashMap::new(),
            set_values: std::collections::HashMap::new(),
            inc_ops: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.pk_values.insert("id", Box::new(value));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_username(mut self, value: String) -> Self {
        self.set_values.insert("username", Box::new(value));
        self
    }
    pub fn set_tag(mut self, value: String) -> Self {
        self.set_values.insert("tag", Box::new(value));
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_values.insert("created_at", Box::new(value));
        self
    }
    pub fn set_bot(mut self, value: bool) -> Self {
        self.set_values.insert("bot", Box::new(value));
        self
    }
    pub fn set_status(mut self, value: String) -> Self {
        self.set_values.insert("status", Box::new(value));
        self
    }
    pub fn set_flags(mut self, value: i32) -> Self {
        self.set_values.insert("flags", Box::new(value));
        self
    }
    pub fn set_bio(mut self, value: Option<String>) -> Self {
        self.set_values.insert("bio", Box::new(value));
        self
    }
    pub fn set_avatar(mut self, value: Option<String>) -> Self {
        self.set_values.insert("avatar", Box::new(value));
        self
    }
    pub fn set_banner(mut self, value: Option<String>) -> Self {
        self.set_values.insert("banner", Box::new(value));
        self
    }
    pub fn inc_flags(mut self, amount: i64) -> Self {
        self.inc_ops.insert("flags", ("inc", amount));
        self.set_values.insert("flags", Box::new(amount));
        self
    }
    pub fn dec_flags(mut self, amount: i64) -> Self {
        self.inc_ops.insert("flags", ("dec", amount));
        self.set_values.insert("flags", Box::new(-amount));
        self
    }
    pub fn mul_flags(mut self, factor: i64) -> Self {
        self.inc_ops.insert("flags", ("mul", factor));
        self.set_values.insert("flags", Box::new(0));
        self
    }
    pub fn div_flags(mut self, divisor: i64) -> Self {
        self.inc_ops.insert("flags", ("div", divisor));
        self.set_values.insert("flags", Box::new(0));
        self
    }
}
impl std::future::Future for UsersUpsert {
    type Output = Result<Users, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pk_columns = vec!["id"];
            for pk_col in &pk_columns {
                if !me.pk_values.contains_key(pk_col)
                    && !me.set_values.contains_key(pk_col)
                {
                    return std::task::Poll::Ready(
                        Err(format!("Missing primary key field: {}", pk_col).into()),
                    );
                }
            }
            let mut all_values = std::mem::take(&mut me.pk_values);
            for (k, v) in std::mem::take(&mut me.set_values) {
                all_values.insert(k, v);
            }
            if all_values.is_empty() {
                return std::task::Poll::Ready(Err("No fields to upsert".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let inc_ops = std::mem::take(&mut me.inc_ops);
            let conflict_clause = "id".to_string();
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let mut columns: Vec<&str> = all_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(all_values.get(col).unwrap().as_ref());
                }
                let update_columns: Vec<&str> = columns
                    .iter()
                    .filter(|col| !pk_columns.iter().any(|pk| pk == *col))
                    .copied()
                    .collect();
                let sql = if update_columns.is_empty() && inc_ops.is_empty() {
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause
                    )
                } else {
                    let mut update_clauses: Vec<String> = vec![];
                    for col in update_columns {
                        if let Some((op, value)) = inc_ops.get(col) {
                            let clause = match *op {
                                "inc" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) + {}", col, table, col, value
                                    )
                                }
                                "dec" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) - {}", col, table, col, value.abs()
                                    )
                                }
                                "mul" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) * {}", col, table, col, value
                                    )
                                }
                                "div" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) / {}", col, table, col, value
                                    )
                                }
                                _ => format!("{} = EXCLUDED.{}", col, col),
                            };
                            update_clauses.push(clause);
                        } else {
                            update_clauses.push(format!("{} = EXCLUDED.{}", col, col));
                        }
                    }
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {} RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause,
                        update_clauses.join(", ")
                    )
                };
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Users {
                    id: row.get("id"),
                    username: row.get("username"),
                    tag: row.get("tag"),
                    created_at: row.get("created_at"),
                    bot: row.get("bot"),
                    status: row.get("status"),
                    flags: row.get("flags"),
                    bio: row.get("bio"),
                    avatar: row.get("avatar"),
                    banner: row.get("banner"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct UsersCreate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Users, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for UsersCreate {}
impl UsersCreate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "users".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_values: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_username(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("username".to_string(), self.where_args.len()));
        self
    }
    pub fn where_username_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "username", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_tag(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("tag".to_string(), self.where_args.len()));
        self
    }
    pub fn where_tag_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "tag", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_bot(mut self, value: bool) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("bot".to_string(), self.where_args.len()));
        self
    }
    pub fn where_bot_in(mut self, values: Vec<bool>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "bot", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_status(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("status".to_string(), self.where_args.len()));
        self
    }
    pub fn where_status_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "status", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_flags(mut self, value: i32) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("flags".to_string(), self.where_args.len()));
        self
    }
    pub fn where_flags_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "flags", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_bio(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("bio".to_string(), self.where_args.len()));
        self
    }
    pub fn where_bio_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "bio", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_bio_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "bio"), 0));
        self
    }
    pub fn where_bio_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "bio"), 0));
        self
    }
    pub fn where_avatar(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("avatar".to_string(), self.where_args.len()));
        self
    }
    pub fn where_avatar_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "avatar", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_avatar_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "avatar"), 0));
        self
    }
    pub fn where_avatar_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "avatar"), 0));
        self
    }
    pub fn where_banner(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("banner".to_string(), self.where_args.len()));
        self
    }
    pub fn where_banner_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "banner", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_banner_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "banner"), 0));
        self
    }
    pub fn where_banner_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "banner"), 0));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_username(mut self, value: String) -> Self {
        self.set_values.insert("username", Box::new(value));
        self
    }
    pub fn set_tag(mut self, value: String) -> Self {
        self.set_values.insert("tag", Box::new(value));
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_values.insert("created_at", Box::new(value));
        self
    }
    pub fn set_bot(mut self, value: bool) -> Self {
        self.set_values.insert("bot", Box::new(value));
        self
    }
    pub fn set_status(mut self, value: String) -> Self {
        self.set_values.insert("status", Box::new(value));
        self
    }
    pub fn set_flags(mut self, value: i32) -> Self {
        self.set_values.insert("flags", Box::new(value));
        self
    }
    pub fn set_bio(mut self, value: Option<String>) -> Self {
        self.set_values.insert("bio", Box::new(value));
        self
    }
    pub fn set_avatar(mut self, value: Option<String>) -> Self {
        self.set_values.insert("avatar", Box::new(value));
        self
    }
    pub fn set_banner(mut self, value: Option<String>) -> Self {
        self.set_values.insert("banner", Box::new(value));
        self
    }
}
impl std::future::Future for UsersCreate {
    type Output = Result<Users, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let required_fields = vec!["id", "username", "tag"];
            for req in &required_fields {
                if !me.set_values.contains_key(req) {
                    return std::task::Poll::Ready(
                        Err(format!("Missing required field: {}", req).into()),
                    );
                }
            }
            if me.set_values.is_empty() && !required_fields.is_empty() {
                return std::task::Poll::Ready(Err("No fields to create".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_fragments = std::mem::take(&mut me.where_fragments);
            let where_args = std::mem::take(&mut me.where_args);
            let set_values = std::mem::take(&mut me.set_values);
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                if !where_fragments.is_empty() {
                    let mut sql = format!("SELECT COUNT(*) FROM {}", table);
                    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                    let conds: Vec<String> = where_fragments
                        .iter()
                        .enumerate()
                        .map(|(i, (col, idx))| format!("{} = ${}", col, i + 1))
                        .collect();
                    sql.push_str(" WHERE ");
                    sql.push_str(&conds.join(" AND "));
                    for arg in &where_args {
                        params.push(arg.as_ref());
                    }
                    let row = client.query_one(&sql, &params[..]).await?;
                    let count: i64 = row.get(0);
                    if count > 0 {
                        return Err("Record already exists".into());
                    }
                }
                let mut columns: Vec<&str> = set_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(set_values.get(col).unwrap().as_ref());
                }
                let sql = format!(
                    "INSERT INTO {} ({}) VALUES ({}) RETURNING *", table, columns_str,
                    placeholders_str
                );
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Users {
                    id: row.get("id"),
                    username: row.get("username"),
                    tag: row.get("tag"),
                    created_at: row.get("created_at"),
                    bot: row.get("bot"),
                    status: row.get("status"),
                    flags: row.get("flags"),
                    bio: row.get("bio"),
                    avatar: row.get("avatar"),
                    banner: row.get("banner"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct UsersDelete {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for UsersDelete {}
impl UsersDelete {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "users".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_username(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("username".to_string(), self.where_args.len()));
        self
    }
    pub fn where_username_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "username", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_tag(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("tag".to_string(), self.where_args.len()));
        self
    }
    pub fn where_tag_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "tag", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_bot(mut self, value: bool) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("bot".to_string(), self.where_args.len()));
        self
    }
    pub fn where_bot_in(mut self, values: Vec<bool>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "bot", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_status(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("status".to_string(), self.where_args.len()));
        self
    }
    pub fn where_status_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "status", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_flags(mut self, value: i32) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("flags".to_string(), self.where_args.len()));
        self
    }
    pub fn where_flags_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "flags", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_bio(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("bio".to_string(), self.where_args.len()));
        self
    }
    pub fn where_bio_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "bio", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_bio_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "bio"), 0));
        self
    }
    pub fn where_bio_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "bio"), 0));
        self
    }
    pub fn where_avatar(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("avatar".to_string(), self.where_args.len()));
        self
    }
    pub fn where_avatar_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "avatar", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_avatar_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "avatar"), 0));
        self
    }
    pub fn where_avatar_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "avatar"), 0));
        self
    }
    pub fn where_banner(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("banner".to_string(), self.where_args.len()));
        self
    }
    pub fn where_banner_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "banner", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_banner_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "banner"), 0));
        self
    }
    pub fn where_banner_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "banner"), 0));
        self
    }
}
impl std::future::Future for UsersDelete {
    type Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.where_fragments.is_empty() {
                return std::task::Poll::Ready(
                    Err("DELETE without WHERE clause is not allowed".into()),
                );
            }
            let mut sql = format!("DELETE FROM {}", me.table);
            let conds: Vec<String> = me
                .where_fragments
                .iter()
                .enumerate()
                .map(|(i, (col, idx))| { format!("{} = ${}", col, i + 1) })
                .collect();
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.where_args) {
                all_params.push(arg);
            }
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let count = client.execute(&sql, &params[..]).await?;
                Ok(count)
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
impl Users {
    pub async fn find_by_id(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        id: String,
    ) -> Result<Option<Users>, Box<dyn std::error::Error + Send + Sync>> {
        let client = pool.get().await.map_err(|_| "Failed to get connection from pool")?;
        let sql = format!(
            "SELECT * FROM {} WHERE {} = $1", stringify!(Users) .to_lowercase(), "id"
        );
        debug::log_query(&sql, 1);
        let row_opt = client.query_opt(&sql, &[&id]).await?;
        Ok(
            row_opt
                .map(|row| Users {
                    id: row.get("id"),
                    username: row.get("username"),
                    tag: row.get("tag"),
                    created_at: row.get("created_at"),
                    bot: row.get("bot"),
                    status: row.get("status"),
                    flags: row.get("flags"),
                    bio: row.get("bio"),
                    avatar: row.get("avatar"),
                    banner: row.get("banner"),
                }),
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Accounts {
    pub id: String,
    pub email: String,
    pub password: String,
    pub user_id: String,
    pub email_verified: bool,
    pub locale: String,
    pub token: String,
}
pub struct AccountsWhereBuilder {
    where_clauses: Vec<String>,
    order_by: Vec<(String, String)>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
}
impl AccountsWhereBuilder {
    pub fn new() -> Self {
        Self {
            where_clauses: vec![],
            order_by: vec![],
            args: vec![],
            limit: None,
            offset: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_email(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "email", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_email_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "email", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_password(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "password", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_password_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "password", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_user_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "user_id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_user_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "user_id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_email_verified(mut self, value: bool) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "email_verified", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_email_verified_in(mut self, values: Vec<bool>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "email_verified", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_locale(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "locale", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_locale_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "locale", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_token(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "token", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_token_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "token", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn order_by_id_asc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_id_desc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_email_asc(mut self) -> Self {
        self.order_by.push(("email".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_email_desc(mut self) -> Self {
        self.order_by.push(("email".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_password_asc(mut self) -> Self {
        self.order_by.push(("password".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_password_desc(mut self) -> Self {
        self.order_by.push(("password".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_user_id_asc(mut self) -> Self {
        self.order_by.push(("user_id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_user_id_desc(mut self) -> Self {
        self.order_by.push(("user_id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_email_verified_asc(mut self) -> Self {
        self.order_by.push(("email_verified".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_email_verified_desc(mut self) -> Self {
        self.order_by.push(("email_verified".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_locale_asc(mut self) -> Self {
        self.order_by.push(("locale".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_locale_desc(mut self) -> Self {
        self.order_by.push(("locale".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_token_asc(mut self) -> Self {
        self.order_by.push(("token".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_token_desc(mut self) -> Self {
        self.order_by.push(("token".to_string(), "DESC".to_string()));
        self
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
}
pub struct AccountsQuery {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_clauses: Vec<String>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
    order_by: Vec<(String, String)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<
                        Vec<Accounts>,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for AccountsQuery {}
impl AccountsQuery {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "accounts".to_string(),
            where_clauses: vec![],
            args: vec![],
            limit: None,
            offset: None,
            order_by: vec![],
            fut: None,
        }
    }
    pub fn from_builder(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        builder: AccountsWhereBuilder,
    ) -> Self {
        Self {
            pool,
            table: "accounts".to_string(),
            where_clauses: builder.where_clauses,
            args: builder.args,
            limit: builder.limit,
            offset: builder.offset,
            order_by: builder.order_by,
            fut: None,
        }
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
    pub async fn first(
        self,
    ) -> Result<Option<Accounts>, Box<dyn std::error::Error + Send + Sync>> {
        let result = self.limit(1).await?;
        Ok(result.into_iter().next())
    }
    pub async fn count(self) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn aggregate<T>(
        self,
        field: &str,
        func: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let func_upper = func.to_uppercase();
        let mut sql = format!("SELECT {}({}) FROM {}", func_upper, field, self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum<T>(
        self,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let default_val = T::default();
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(SUM({}), ${}) FROM {}", field, default_idx, self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum_cast_i64(
        self,
        field: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let default_val: i64 = 0;
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(CAST(SUM({}) AS BIGINT), ${}) FROM {}", field, default_idx,
            self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn avg<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "AVG").await
    }
    pub async fn min<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MIN").await
    }
    pub async fn max<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MAX").await
    }
}
impl Future for AccountsQuery {
    type Output = Result<Vec<Accounts>, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_clauses = std::mem::take(&mut me.where_clauses);
            let limit = me.limit;
            let offset = me.offset;
            let order_by = std::mem::take(&mut me.order_by);
            let args = std::mem::take(&mut me.args);
            let fut = async move {
                let mut sql = format!("SELECT * FROM {}", table);
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = args
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                if !where_clauses.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&where_clauses.join(" AND "));
                }
                if !order_by.is_empty() {
                    let order_clauses: Vec<String> = order_by
                        .iter()
                        .map(|(col, dir)| format!("{} {}", col, dir))
                        .collect();
                    sql.push_str(" ORDER BY ");
                    sql.push_str(&order_clauses.join(", "));
                }
                if let Some(limit) = limit {
                    sql.push_str(&format!(" LIMIT {}", limit));
                }
                if let Some(offset) = offset {
                    sql.push_str(&format!(" OFFSET {}", offset));
                }
                debug::log_query(&sql, params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let rows = client.query(&sql, &params[..]).await?;
                Ok(
                    rows
                        .into_iter()
                        .map(|row| Accounts {
                            id: row.get("id"),
                            email: row.get("email"),
                            password: row.get("password"),
                            user_id: row.get("user_id"),
                            email_verified: row.get("email_verified"),
                            locale: row.get("locale"),
                            token: row.get("token"),
                        })
                        .collect(),
                )
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct AccountsUpdate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_fragments: Vec<&'static str>,
    set_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    inc_ops: Vec<(&'static str, &'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Accounts, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for AccountsUpdate {}
impl AccountsUpdate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "accounts".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_fragments: vec![],
            set_args: vec![],
            inc_ops: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_email(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("email".to_string(), self.where_args.len()));
        self
    }
    pub fn where_email_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "email", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_password(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("password".to_string(), self.where_args.len()));
        self
    }
    pub fn where_password_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "password", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_user_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("user_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_user_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "user_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_email_verified(mut self, value: bool) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("email_verified".to_string(), self.where_args.len()));
        self
    }
    pub fn where_email_verified_in(mut self, values: Vec<bool>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "email_verified", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_locale(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("locale".to_string(), self.where_args.len()));
        self
    }
    pub fn where_locale_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "locale", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_token(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("token".to_string(), self.where_args.len()));
        self
    }
    pub fn where_token_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "token", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("id");
        self
    }
    pub fn set_email(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("email");
        self
    }
    pub fn set_password(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("password");
        self
    }
    pub fn set_user_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("user_id");
        self
    }
    pub fn set_email_verified(mut self, value: bool) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("email_verified");
        self
    }
    pub fn set_locale(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("locale");
        self
    }
    pub fn set_token(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("token");
        self
    }
}
impl std::future::Future for AccountsUpdate {
    type Output = Result<Accounts, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.set_fragments.is_empty() && me.inc_ops.is_empty() {
                return std::task::Poll::Ready(Err("No fields to update".into()));
            }
            let mut sql = format!("UPDATE {} SET ", me.table);
            let mut set_clauses: Vec<String> = vec![];
            let mut param_idx = 1;
            for col in me.set_fragments.iter() {
                set_clauses.push(format!("{} = ${}", col, param_idx));
                param_idx += 1;
            }
            for (field, op, _) in &me.inc_ops {
                let clause = match *op {
                    "inc" => format!("{} = {} + ${}", field, field, param_idx),
                    "dec" => format!("{} = {} - ${}", field, field, param_idx),
                    "mul" => format!("{} = {} * ${}", field, field, param_idx),
                    "div" => format!("{} = {} / ${}", field, field, param_idx),
                    _ => continue,
                };
                set_clauses.push(clause);
                param_idx += 1;
            }
            sql.push_str(&set_clauses.join(", "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.set_args) {
                all_params.push(arg);
            }
            for (_, _, val) in &me.inc_ops {
                all_params.push(Box::new(*val));
            }
            if !me.where_fragments.is_empty() {
                let where_clauses: Vec<String> = me
                    .where_fragments
                    .iter()
                    .enumerate()
                    .map(|(i, (col, _))| format!("{} = ${}", col, param_idx + i))
                    .collect();
                sql.push_str(" WHERE ");
                sql.push_str(&where_clauses.join(" AND "));
                for arg in std::mem::take(&mut me.where_args) {
                    all_params.push(arg);
                }
            }
            sql.push_str(" RETURNING *");
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Accounts {
                    id: row.get("id"),
                    email: row.get("email"),
                    password: row.get("password"),
                    user_id: row.get("user_id"),
                    email_verified: row.get("email_verified"),
                    locale: row.get("locale"),
                    token: row.get("token"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct AccountsUpsert {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    pk_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    inc_ops: std::collections::HashMap<&'static str, (&'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Accounts, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for AccountsUpsert {}
impl AccountsUpsert {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "accounts".to_string(),
            pk_values: std::collections::HashMap::new(),
            set_values: std::collections::HashMap::new(),
            inc_ops: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.pk_values.insert("id", Box::new(value));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_email(mut self, value: String) -> Self {
        self.set_values.insert("email", Box::new(value));
        self
    }
    pub fn set_password(mut self, value: String) -> Self {
        self.set_values.insert("password", Box::new(value));
        self
    }
    pub fn set_user_id(mut self, value: String) -> Self {
        self.set_values.insert("user_id", Box::new(value));
        self
    }
    pub fn set_email_verified(mut self, value: bool) -> Self {
        self.set_values.insert("email_verified", Box::new(value));
        self
    }
    pub fn set_locale(mut self, value: String) -> Self {
        self.set_values.insert("locale", Box::new(value));
        self
    }
    pub fn set_token(mut self, value: String) -> Self {
        self.set_values.insert("token", Box::new(value));
        self
    }
}
impl std::future::Future for AccountsUpsert {
    type Output = Result<Accounts, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pk_columns = vec!["id"];
            for pk_col in &pk_columns {
                if !me.pk_values.contains_key(pk_col)
                    && !me.set_values.contains_key(pk_col)
                {
                    return std::task::Poll::Ready(
                        Err(format!("Missing primary key field: {}", pk_col).into()),
                    );
                }
            }
            let mut all_values = std::mem::take(&mut me.pk_values);
            for (k, v) in std::mem::take(&mut me.set_values) {
                all_values.insert(k, v);
            }
            if all_values.is_empty() {
                return std::task::Poll::Ready(Err("No fields to upsert".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let inc_ops = std::mem::take(&mut me.inc_ops);
            let conflict_clause = "id".to_string();
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let mut columns: Vec<&str> = all_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(all_values.get(col).unwrap().as_ref());
                }
                let update_columns: Vec<&str> = columns
                    .iter()
                    .filter(|col| !pk_columns.iter().any(|pk| pk == *col))
                    .copied()
                    .collect();
                let sql = if update_columns.is_empty() && inc_ops.is_empty() {
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause
                    )
                } else {
                    let mut update_clauses: Vec<String> = vec![];
                    for col in update_columns {
                        if let Some((op, value)) = inc_ops.get(col) {
                            let clause = match *op {
                                "inc" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) + {}", col, table, col, value
                                    )
                                }
                                "dec" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) - {}", col, table, col, value.abs()
                                    )
                                }
                                "mul" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) * {}", col, table, col, value
                                    )
                                }
                                "div" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) / {}", col, table, col, value
                                    )
                                }
                                _ => format!("{} = EXCLUDED.{}", col, col),
                            };
                            update_clauses.push(clause);
                        } else {
                            update_clauses.push(format!("{} = EXCLUDED.{}", col, col));
                        }
                    }
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {} RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause,
                        update_clauses.join(", ")
                    )
                };
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Accounts {
                    id: row.get("id"),
                    email: row.get("email"),
                    password: row.get("password"),
                    user_id: row.get("user_id"),
                    email_verified: row.get("email_verified"),
                    locale: row.get("locale"),
                    token: row.get("token"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct AccountsCreate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Accounts, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for AccountsCreate {}
impl AccountsCreate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "accounts".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_values: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_email(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("email".to_string(), self.where_args.len()));
        self
    }
    pub fn where_email_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "email", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_password(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("password".to_string(), self.where_args.len()));
        self
    }
    pub fn where_password_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "password", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_user_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("user_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_user_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "user_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_email_verified(mut self, value: bool) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("email_verified".to_string(), self.where_args.len()));
        self
    }
    pub fn where_email_verified_in(mut self, values: Vec<bool>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "email_verified", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_locale(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("locale".to_string(), self.where_args.len()));
        self
    }
    pub fn where_locale_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "locale", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_token(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("token".to_string(), self.where_args.len()));
        self
    }
    pub fn where_token_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "token", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_email(mut self, value: String) -> Self {
        self.set_values.insert("email", Box::new(value));
        self
    }
    pub fn set_password(mut self, value: String) -> Self {
        self.set_values.insert("password", Box::new(value));
        self
    }
    pub fn set_user_id(mut self, value: String) -> Self {
        self.set_values.insert("user_id", Box::new(value));
        self
    }
    pub fn set_email_verified(mut self, value: bool) -> Self {
        self.set_values.insert("email_verified", Box::new(value));
        self
    }
    pub fn set_locale(mut self, value: String) -> Self {
        self.set_values.insert("locale", Box::new(value));
        self
    }
    pub fn set_token(mut self, value: String) -> Self {
        self.set_values.insert("token", Box::new(value));
        self
    }
}
impl std::future::Future for AccountsCreate {
    type Output = Result<Accounts, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let required_fields = vec!["id", "email", "password", "user_id", "token"];
            for req in &required_fields {
                if !me.set_values.contains_key(req) {
                    return std::task::Poll::Ready(
                        Err(format!("Missing required field: {}", req).into()),
                    );
                }
            }
            if me.set_values.is_empty() && !required_fields.is_empty() {
                return std::task::Poll::Ready(Err("No fields to create".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_fragments = std::mem::take(&mut me.where_fragments);
            let where_args = std::mem::take(&mut me.where_args);
            let set_values = std::mem::take(&mut me.set_values);
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                if !where_fragments.is_empty() {
                    let mut sql = format!("SELECT COUNT(*) FROM {}", table);
                    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                    let conds: Vec<String> = where_fragments
                        .iter()
                        .enumerate()
                        .map(|(i, (col, idx))| format!("{} = ${}", col, i + 1))
                        .collect();
                    sql.push_str(" WHERE ");
                    sql.push_str(&conds.join(" AND "));
                    for arg in &where_args {
                        params.push(arg.as_ref());
                    }
                    let row = client.query_one(&sql, &params[..]).await?;
                    let count: i64 = row.get(0);
                    if count > 0 {
                        return Err("Record already exists".into());
                    }
                }
                let mut columns: Vec<&str> = set_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(set_values.get(col).unwrap().as_ref());
                }
                let sql = format!(
                    "INSERT INTO {} ({}) VALUES ({}) RETURNING *", table, columns_str,
                    placeholders_str
                );
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Accounts {
                    id: row.get("id"),
                    email: row.get("email"),
                    password: row.get("password"),
                    user_id: row.get("user_id"),
                    email_verified: row.get("email_verified"),
                    locale: row.get("locale"),
                    token: row.get("token"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct AccountsDelete {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for AccountsDelete {}
impl AccountsDelete {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "accounts".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_email(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("email".to_string(), self.where_args.len()));
        self
    }
    pub fn where_email_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "email", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_password(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("password".to_string(), self.where_args.len()));
        self
    }
    pub fn where_password_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "password", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_user_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("user_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_user_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "user_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_email_verified(mut self, value: bool) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("email_verified".to_string(), self.where_args.len()));
        self
    }
    pub fn where_email_verified_in(mut self, values: Vec<bool>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "email_verified", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_locale(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("locale".to_string(), self.where_args.len()));
        self
    }
    pub fn where_locale_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "locale", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_token(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("token".to_string(), self.where_args.len()));
        self
    }
    pub fn where_token_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "token", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
}
impl std::future::Future for AccountsDelete {
    type Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.where_fragments.is_empty() {
                return std::task::Poll::Ready(
                    Err("DELETE without WHERE clause is not allowed".into()),
                );
            }
            let mut sql = format!("DELETE FROM {}", me.table);
            let conds: Vec<String> = me
                .where_fragments
                .iter()
                .enumerate()
                .map(|(i, (col, idx))| { format!("{} = ${}", col, i + 1) })
                .collect();
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.where_args) {
                all_params.push(arg);
            }
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let count = client.execute(&sql, &params[..]).await?;
                Ok(count)
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
impl Accounts {
    pub async fn find_by_id(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        id: String,
    ) -> Result<Option<Accounts>, Box<dyn std::error::Error + Send + Sync>> {
        let client = pool.get().await.map_err(|_| "Failed to get connection from pool")?;
        let sql = format!(
            "SELECT * FROM {} WHERE {} = $1", stringify!(Accounts) .to_lowercase(), "id"
        );
        debug::log_query(&sql, 1);
        let row_opt = client.query_opt(&sql, &[&id]).await?;
        Ok(
            row_opt
                .map(|row| Accounts {
                    id: row.get("id"),
                    email: row.get("email"),
                    password: row.get("password"),
                    user_id: row.get("user_id"),
                    email_verified: row.get("email_verified"),
                    locale: row.get("locale"),
                    token: row.get("token"),
                }),
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Guilds {
    pub id: String,
    pub name: String,
    pub brief: String,
    pub icon: Option<String>,
    pub created_at: DateTime<Utc>,
    pub owner_id: String,
}
pub struct GuildsWhereBuilder {
    where_clauses: Vec<String>,
    order_by: Vec<(String, String)>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
}
impl GuildsWhereBuilder {
    pub fn new() -> Self {
        Self {
            where_clauses: vec![],
            order_by: vec![],
            args: vec![],
            limit: None,
            offset: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_name(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "name", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_name_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "name", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_brief(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "brief", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_brief_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "brief", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_icon(mut self, value: Option<String>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "icon", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_icon_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "icon", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_icon_is_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NULL", "icon"));
        self
    }
    pub fn where_icon_is_not_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NOT NULL", "icon"));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "created_at", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} > ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} < ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} >= ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} <= ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_owner_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "owner_id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_owner_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "owner_id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn order_by_id_asc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_id_desc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_name_asc(mut self) -> Self {
        self.order_by.push(("name".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_name_desc(mut self) -> Self {
        self.order_by.push(("name".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_brief_asc(mut self) -> Self {
        self.order_by.push(("brief".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_brief_desc(mut self) -> Self {
        self.order_by.push(("brief".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_icon_asc(mut self) -> Self {
        self.order_by.push(("icon".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_icon_desc(mut self) -> Self {
        self.order_by.push(("icon".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_created_at_asc(mut self) -> Self {
        self.order_by.push(("created_at".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_created_at_desc(mut self) -> Self {
        self.order_by.push(("created_at".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_owner_id_asc(mut self) -> Self {
        self.order_by.push(("owner_id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_owner_id_desc(mut self) -> Self {
        self.order_by.push(("owner_id".to_string(), "DESC".to_string()));
        self
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
}
pub struct GuildsQuery {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_clauses: Vec<String>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
    order_by: Vec<(String, String)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<
                        Vec<Guilds>,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for GuildsQuery {}
impl GuildsQuery {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "guilds".to_string(),
            where_clauses: vec![],
            args: vec![],
            limit: None,
            offset: None,
            order_by: vec![],
            fut: None,
        }
    }
    pub fn from_builder(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        builder: GuildsWhereBuilder,
    ) -> Self {
        Self {
            pool,
            table: "guilds".to_string(),
            where_clauses: builder.where_clauses,
            args: builder.args,
            limit: builder.limit,
            offset: builder.offset,
            order_by: builder.order_by,
            fut: None,
        }
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
    pub async fn first(
        self,
    ) -> Result<Option<Guilds>, Box<dyn std::error::Error + Send + Sync>> {
        let result = self.limit(1).await?;
        Ok(result.into_iter().next())
    }
    pub async fn count(self) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn aggregate<T>(
        self,
        field: &str,
        func: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let func_upper = func.to_uppercase();
        let mut sql = format!("SELECT {}({}) FROM {}", func_upper, field, self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum<T>(
        self,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let default_val = T::default();
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(SUM({}), ${}) FROM {}", field, default_idx, self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum_cast_i64(
        self,
        field: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let default_val: i64 = 0;
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(CAST(SUM({}) AS BIGINT), ${}) FROM {}", field, default_idx,
            self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn avg<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "AVG").await
    }
    pub async fn min<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MIN").await
    }
    pub async fn max<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MAX").await
    }
}
impl Future for GuildsQuery {
    type Output = Result<Vec<Guilds>, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_clauses = std::mem::take(&mut me.where_clauses);
            let limit = me.limit;
            let offset = me.offset;
            let order_by = std::mem::take(&mut me.order_by);
            let args = std::mem::take(&mut me.args);
            let fut = async move {
                let mut sql = format!("SELECT * FROM {}", table);
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = args
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                if !where_clauses.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&where_clauses.join(" AND "));
                }
                if !order_by.is_empty() {
                    let order_clauses: Vec<String> = order_by
                        .iter()
                        .map(|(col, dir)| format!("{} {}", col, dir))
                        .collect();
                    sql.push_str(" ORDER BY ");
                    sql.push_str(&order_clauses.join(", "));
                }
                if let Some(limit) = limit {
                    sql.push_str(&format!(" LIMIT {}", limit));
                }
                if let Some(offset) = offset {
                    sql.push_str(&format!(" OFFSET {}", offset));
                }
                debug::log_query(&sql, params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let rows = client.query(&sql, &params[..]).await?;
                Ok(
                    rows
                        .into_iter()
                        .map(|row| Guilds {
                            id: row.get("id"),
                            name: row.get("name"),
                            brief: row.get("brief"),
                            icon: row.get("icon"),
                            created_at: row.get("created_at"),
                            owner_id: row.get("owner_id"),
                        })
                        .collect(),
                )
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct GuildsUpdate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_fragments: Vec<&'static str>,
    set_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    inc_ops: Vec<(&'static str, &'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Guilds, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for GuildsUpdate {}
impl GuildsUpdate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "guilds".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_fragments: vec![],
            set_args: vec![],
            inc_ops: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_name(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("name".to_string(), self.where_args.len()));
        self
    }
    pub fn where_name_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "name", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_brief(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("brief".to_string(), self.where_args.len()));
        self
    }
    pub fn where_brief_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "brief", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_icon(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("icon".to_string(), self.where_args.len()));
        self
    }
    pub fn where_icon_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "icon", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_icon_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "icon"), 0));
        self
    }
    pub fn where_icon_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "icon"), 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_owner_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("owner_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_owner_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "owner_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("id");
        self
    }
    pub fn set_name(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("name");
        self
    }
    pub fn set_brief(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("brief");
        self
    }
    pub fn set_icon(mut self, value: Option<String>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("icon");
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("created_at");
        self
    }
    pub fn set_owner_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("owner_id");
        self
    }
}
impl std::future::Future for GuildsUpdate {
    type Output = Result<Guilds, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.set_fragments.is_empty() && me.inc_ops.is_empty() {
                return std::task::Poll::Ready(Err("No fields to update".into()));
            }
            let mut sql = format!("UPDATE {} SET ", me.table);
            let mut set_clauses: Vec<String> = vec![];
            let mut param_idx = 1;
            for col in me.set_fragments.iter() {
                set_clauses.push(format!("{} = ${}", col, param_idx));
                param_idx += 1;
            }
            for (field, op, _) in &me.inc_ops {
                let clause = match *op {
                    "inc" => format!("{} = {} + ${}", field, field, param_idx),
                    "dec" => format!("{} = {} - ${}", field, field, param_idx),
                    "mul" => format!("{} = {} * ${}", field, field, param_idx),
                    "div" => format!("{} = {} / ${}", field, field, param_idx),
                    _ => continue,
                };
                set_clauses.push(clause);
                param_idx += 1;
            }
            sql.push_str(&set_clauses.join(", "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.set_args) {
                all_params.push(arg);
            }
            for (_, _, val) in &me.inc_ops {
                all_params.push(Box::new(*val));
            }
            if !me.where_fragments.is_empty() {
                let where_clauses: Vec<String> = me
                    .where_fragments
                    .iter()
                    .enumerate()
                    .map(|(i, (col, _))| format!("{} = ${}", col, param_idx + i))
                    .collect();
                sql.push_str(" WHERE ");
                sql.push_str(&where_clauses.join(" AND "));
                for arg in std::mem::take(&mut me.where_args) {
                    all_params.push(arg);
                }
            }
            sql.push_str(" RETURNING *");
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Guilds {
                    id: row.get("id"),
                    name: row.get("name"),
                    brief: row.get("brief"),
                    icon: row.get("icon"),
                    created_at: row.get("created_at"),
                    owner_id: row.get("owner_id"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct GuildsUpsert {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    pk_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    inc_ops: std::collections::HashMap<&'static str, (&'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Guilds, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for GuildsUpsert {}
impl GuildsUpsert {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "guilds".to_string(),
            pk_values: std::collections::HashMap::new(),
            set_values: std::collections::HashMap::new(),
            inc_ops: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.pk_values.insert("id", Box::new(value));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_name(mut self, value: String) -> Self {
        self.set_values.insert("name", Box::new(value));
        self
    }
    pub fn set_brief(mut self, value: String) -> Self {
        self.set_values.insert("brief", Box::new(value));
        self
    }
    pub fn set_icon(mut self, value: Option<String>) -> Self {
        self.set_values.insert("icon", Box::new(value));
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_values.insert("created_at", Box::new(value));
        self
    }
    pub fn set_owner_id(mut self, value: String) -> Self {
        self.set_values.insert("owner_id", Box::new(value));
        self
    }
}
impl std::future::Future for GuildsUpsert {
    type Output = Result<Guilds, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pk_columns = vec!["id"];
            for pk_col in &pk_columns {
                if !me.pk_values.contains_key(pk_col)
                    && !me.set_values.contains_key(pk_col)
                {
                    return std::task::Poll::Ready(
                        Err(format!("Missing primary key field: {}", pk_col).into()),
                    );
                }
            }
            let mut all_values = std::mem::take(&mut me.pk_values);
            for (k, v) in std::mem::take(&mut me.set_values) {
                all_values.insert(k, v);
            }
            if all_values.is_empty() {
                return std::task::Poll::Ready(Err("No fields to upsert".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let inc_ops = std::mem::take(&mut me.inc_ops);
            let conflict_clause = "id".to_string();
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let mut columns: Vec<&str> = all_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(all_values.get(col).unwrap().as_ref());
                }
                let update_columns: Vec<&str> = columns
                    .iter()
                    .filter(|col| !pk_columns.iter().any(|pk| pk == *col))
                    .copied()
                    .collect();
                let sql = if update_columns.is_empty() && inc_ops.is_empty() {
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause
                    )
                } else {
                    let mut update_clauses: Vec<String> = vec![];
                    for col in update_columns {
                        if let Some((op, value)) = inc_ops.get(col) {
                            let clause = match *op {
                                "inc" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) + {}", col, table, col, value
                                    )
                                }
                                "dec" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) - {}", col, table, col, value.abs()
                                    )
                                }
                                "mul" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) * {}", col, table, col, value
                                    )
                                }
                                "div" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) / {}", col, table, col, value
                                    )
                                }
                                _ => format!("{} = EXCLUDED.{}", col, col),
                            };
                            update_clauses.push(clause);
                        } else {
                            update_clauses.push(format!("{} = EXCLUDED.{}", col, col));
                        }
                    }
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {} RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause,
                        update_clauses.join(", ")
                    )
                };
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Guilds {
                    id: row.get("id"),
                    name: row.get("name"),
                    brief: row.get("brief"),
                    icon: row.get("icon"),
                    created_at: row.get("created_at"),
                    owner_id: row.get("owner_id"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct GuildsCreate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Guilds, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for GuildsCreate {}
impl GuildsCreate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "guilds".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_values: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_name(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("name".to_string(), self.where_args.len()));
        self
    }
    pub fn where_name_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "name", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_brief(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("brief".to_string(), self.where_args.len()));
        self
    }
    pub fn where_brief_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "brief", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_icon(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("icon".to_string(), self.where_args.len()));
        self
    }
    pub fn where_icon_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "icon", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_icon_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "icon"), 0));
        self
    }
    pub fn where_icon_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "icon"), 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_owner_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("owner_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_owner_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "owner_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_name(mut self, value: String) -> Self {
        self.set_values.insert("name", Box::new(value));
        self
    }
    pub fn set_brief(mut self, value: String) -> Self {
        self.set_values.insert("brief", Box::new(value));
        self
    }
    pub fn set_icon(mut self, value: Option<String>) -> Self {
        self.set_values.insert("icon", Box::new(value));
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_values.insert("created_at", Box::new(value));
        self
    }
    pub fn set_owner_id(mut self, value: String) -> Self {
        self.set_values.insert("owner_id", Box::new(value));
        self
    }
}
impl std::future::Future for GuildsCreate {
    type Output = Result<Guilds, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let required_fields = vec!["id", "owner_id"];
            for req in &required_fields {
                if !me.set_values.contains_key(req) {
                    return std::task::Poll::Ready(
                        Err(format!("Missing required field: {}", req).into()),
                    );
                }
            }
            if me.set_values.is_empty() && !required_fields.is_empty() {
                return std::task::Poll::Ready(Err("No fields to create".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_fragments = std::mem::take(&mut me.where_fragments);
            let where_args = std::mem::take(&mut me.where_args);
            let set_values = std::mem::take(&mut me.set_values);
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                if !where_fragments.is_empty() {
                    let mut sql = format!("SELECT COUNT(*) FROM {}", table);
                    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                    let conds: Vec<String> = where_fragments
                        .iter()
                        .enumerate()
                        .map(|(i, (col, idx))| format!("{} = ${}", col, i + 1))
                        .collect();
                    sql.push_str(" WHERE ");
                    sql.push_str(&conds.join(" AND "));
                    for arg in &where_args {
                        params.push(arg.as_ref());
                    }
                    let row = client.query_one(&sql, &params[..]).await?;
                    let count: i64 = row.get(0);
                    if count > 0 {
                        return Err("Record already exists".into());
                    }
                }
                let mut columns: Vec<&str> = set_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(set_values.get(col).unwrap().as_ref());
                }
                let sql = format!(
                    "INSERT INTO {} ({}) VALUES ({}) RETURNING *", table, columns_str,
                    placeholders_str
                );
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Guilds {
                    id: row.get("id"),
                    name: row.get("name"),
                    brief: row.get("brief"),
                    icon: row.get("icon"),
                    created_at: row.get("created_at"),
                    owner_id: row.get("owner_id"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct GuildsDelete {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for GuildsDelete {}
impl GuildsDelete {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "guilds".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_name(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("name".to_string(), self.where_args.len()));
        self
    }
    pub fn where_name_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "name", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_brief(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("brief".to_string(), self.where_args.len()));
        self
    }
    pub fn where_brief_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "brief", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_icon(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("icon".to_string(), self.where_args.len()));
        self
    }
    pub fn where_icon_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "icon", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_icon_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "icon"), 0));
        self
    }
    pub fn where_icon_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "icon"), 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_owner_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("owner_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_owner_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "owner_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
}
impl std::future::Future for GuildsDelete {
    type Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.where_fragments.is_empty() {
                return std::task::Poll::Ready(
                    Err("DELETE without WHERE clause is not allowed".into()),
                );
            }
            let mut sql = format!("DELETE FROM {}", me.table);
            let conds: Vec<String> = me
                .where_fragments
                .iter()
                .enumerate()
                .map(|(i, (col, idx))| { format!("{} = ${}", col, i + 1) })
                .collect();
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.where_args) {
                all_params.push(arg);
            }
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let count = client.execute(&sql, &params[..]).await?;
                Ok(count)
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
impl Guilds {
    pub async fn find_by_id(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        id: String,
    ) -> Result<Option<Guilds>, Box<dyn std::error::Error + Send + Sync>> {
        let client = pool.get().await.map_err(|_| "Failed to get connection from pool")?;
        let sql = format!(
            "SELECT * FROM {} WHERE {} = $1", stringify!(Guilds) .to_lowercase(), "id"
        );
        debug::log_query(&sql, 1);
        let row_opt = client.query_opt(&sql, &[&id]).await?;
        Ok(
            row_opt
                .map(|row| Guilds {
                    id: row.get("id"),
                    name: row.get("name"),
                    brief: row.get("brief"),
                    icon: row.get("icon"),
                    created_at: row.get("created_at"),
                    owner_id: row.get("owner_id"),
                }),
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuildMembers {
    pub id: i32,
    pub guild_id: String,
    pub user_id: String,
    pub nickname: Option<String>,
    pub joined_at: DateTime<Utc>,
}
pub struct GuildMembersWhereBuilder {
    where_clauses: Vec<String>,
    order_by: Vec<(String, String)>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
}
impl GuildMembersWhereBuilder {
    pub fn new() -> Self {
        Self {
            where_clauses: vec![],
            order_by: vec![],
            args: vec![],
            limit: None,
            offset: None,
        }
    }
    pub fn where_id(mut self, value: i32) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_id_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "guild_id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "guild_id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_user_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "user_id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_user_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "user_id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_nickname(mut self, value: Option<String>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "nickname", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_nickname_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "nickname", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_nickname_is_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NULL", "nickname"));
        self
    }
    pub fn where_nickname_is_not_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NOT NULL", "nickname"));
        self
    }
    pub fn where_joined_at(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "joined_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_joined_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "joined_at", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_joined_at_gt(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} > ${}", "joined_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_joined_at_lt(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} < ${}", "joined_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_joined_at_gte(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} >= ${}", "joined_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_joined_at_lte(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} <= ${}", "joined_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn order_by_id_asc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_id_desc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_guild_id_asc(mut self) -> Self {
        self.order_by.push(("guild_id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_guild_id_desc(mut self) -> Self {
        self.order_by.push(("guild_id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_user_id_asc(mut self) -> Self {
        self.order_by.push(("user_id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_user_id_desc(mut self) -> Self {
        self.order_by.push(("user_id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_nickname_asc(mut self) -> Self {
        self.order_by.push(("nickname".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_nickname_desc(mut self) -> Self {
        self.order_by.push(("nickname".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_joined_at_asc(mut self) -> Self {
        self.order_by.push(("joined_at".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_joined_at_desc(mut self) -> Self {
        self.order_by.push(("joined_at".to_string(), "DESC".to_string()));
        self
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
}
pub struct GuildMembersQuery {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_clauses: Vec<String>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
    order_by: Vec<(String, String)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<
                        Vec<GuildMembers>,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for GuildMembersQuery {}
impl GuildMembersQuery {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "guildmembers".to_string(),
            where_clauses: vec![],
            args: vec![],
            limit: None,
            offset: None,
            order_by: vec![],
            fut: None,
        }
    }
    pub fn from_builder(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        builder: GuildMembersWhereBuilder,
    ) -> Self {
        Self {
            pool,
            table: "guildmembers".to_string(),
            where_clauses: builder.where_clauses,
            args: builder.args,
            limit: builder.limit,
            offset: builder.offset,
            order_by: builder.order_by,
            fut: None,
        }
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
    pub async fn first(
        self,
    ) -> Result<Option<GuildMembers>, Box<dyn std::error::Error + Send + Sync>> {
        let result = self.limit(1).await?;
        Ok(result.into_iter().next())
    }
    pub async fn count(self) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn aggregate<T>(
        self,
        field: &str,
        func: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let func_upper = func.to_uppercase();
        let mut sql = format!("SELECT {}({}) FROM {}", func_upper, field, self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum<T>(
        self,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let default_val = T::default();
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(SUM({}), ${}) FROM {}", field, default_idx, self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum_cast_i64(
        self,
        field: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let default_val: i64 = 0;
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(CAST(SUM({}) AS BIGINT), ${}) FROM {}", field, default_idx,
            self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn avg<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "AVG").await
    }
    pub async fn min<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MIN").await
    }
    pub async fn max<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MAX").await
    }
}
impl Future for GuildMembersQuery {
    type Output = Result<Vec<GuildMembers>, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_clauses = std::mem::take(&mut me.where_clauses);
            let limit = me.limit;
            let offset = me.offset;
            let order_by = std::mem::take(&mut me.order_by);
            let args = std::mem::take(&mut me.args);
            let fut = async move {
                let mut sql = format!("SELECT * FROM {}", table);
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = args
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                if !where_clauses.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&where_clauses.join(" AND "));
                }
                if !order_by.is_empty() {
                    let order_clauses: Vec<String> = order_by
                        .iter()
                        .map(|(col, dir)| format!("{} {}", col, dir))
                        .collect();
                    sql.push_str(" ORDER BY ");
                    sql.push_str(&order_clauses.join(", "));
                }
                if let Some(limit) = limit {
                    sql.push_str(&format!(" LIMIT {}", limit));
                }
                if let Some(offset) = offset {
                    sql.push_str(&format!(" OFFSET {}", offset));
                }
                debug::log_query(&sql, params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let rows = client.query(&sql, &params[..]).await?;
                Ok(
                    rows
                        .into_iter()
                        .map(|row| GuildMembers {
                            id: row.get("id"),
                            guild_id: row.get("guild_id"),
                            user_id: row.get("user_id"),
                            nickname: row.get("nickname"),
                            joined_at: row.get("joined_at"),
                        })
                        .collect(),
                )
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct GuildMembersUpdate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_fragments: Vec<&'static str>,
    set_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    inc_ops: Vec<(&'static str, &'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<
                        GuildMembers,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for GuildMembersUpdate {}
impl GuildMembersUpdate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "guildmembers".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_fragments: vec![],
            set_args: vec![],
            inc_ops: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: i32) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("guild_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "guild_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_user_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("user_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_user_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "user_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_nickname(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("nickname".to_string(), self.where_args.len()));
        self
    }
    pub fn where_nickname_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "nickname", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_nickname_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "nickname"), 0));
        self
    }
    pub fn where_nickname_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "nickname"), 0));
        self
    }
    pub fn where_joined_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("joined_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_joined_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "joined_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_joined_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push((format!("{} >", "joined_at"), self.where_args.len()));
        self
    }
    pub fn where_joined_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments.push((format!("{} <", "joined_at"), self.where_args.len()));
        self
    }
    pub fn where_joined_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "joined_at"), self.where_args.len()));
        self
    }
    pub fn where_joined_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "joined_at"), self.where_args.len()));
        self
    }
    pub fn set_id(mut self, value: i32) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("id");
        self
    }
    pub fn set_guild_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("guild_id");
        self
    }
    pub fn set_user_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("user_id");
        self
    }
    pub fn set_nickname(mut self, value: Option<String>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("nickname");
        self
    }
    pub fn set_joined_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("joined_at");
        self
    }
    pub fn inc_id(mut self, amount: i64) -> Self {
        self.inc_ops.push(("id", "inc", amount));
        self
    }
    pub fn dec_id(mut self, amount: i64) -> Self {
        self.inc_ops.push(("id", "dec", amount));
        self
    }
    pub fn mul_id(mut self, factor: i64) -> Self {
        self.inc_ops.push(("id", "mul", factor));
        self
    }
    pub fn div_id(mut self, divisor: i64) -> Self {
        self.inc_ops.push(("id", "div", divisor));
        self
    }
}
impl std::future::Future for GuildMembersUpdate {
    type Output = Result<GuildMembers, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.set_fragments.is_empty() && me.inc_ops.is_empty() {
                return std::task::Poll::Ready(Err("No fields to update".into()));
            }
            let mut sql = format!("UPDATE {} SET ", me.table);
            let mut set_clauses: Vec<String> = vec![];
            let mut param_idx = 1;
            for col in me.set_fragments.iter() {
                set_clauses.push(format!("{} = ${}", col, param_idx));
                param_idx += 1;
            }
            for (field, op, _) in &me.inc_ops {
                let clause = match *op {
                    "inc" => format!("{} = {} + ${}", field, field, param_idx),
                    "dec" => format!("{} = {} - ${}", field, field, param_idx),
                    "mul" => format!("{} = {} * ${}", field, field, param_idx),
                    "div" => format!("{} = {} / ${}", field, field, param_idx),
                    _ => continue,
                };
                set_clauses.push(clause);
                param_idx += 1;
            }
            sql.push_str(&set_clauses.join(", "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.set_args) {
                all_params.push(arg);
            }
            for (_, _, val) in &me.inc_ops {
                all_params.push(Box::new(*val));
            }
            if !me.where_fragments.is_empty() {
                let where_clauses: Vec<String> = me
                    .where_fragments
                    .iter()
                    .enumerate()
                    .map(|(i, (col, _))| format!("{} = ${}", col, param_idx + i))
                    .collect();
                sql.push_str(" WHERE ");
                sql.push_str(&where_clauses.join(" AND "));
                for arg in std::mem::take(&mut me.where_args) {
                    all_params.push(arg);
                }
            }
            sql.push_str(" RETURNING *");
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(GuildMembers {
                    id: row.get("id"),
                    guild_id: row.get("guild_id"),
                    user_id: row.get("user_id"),
                    nickname: row.get("nickname"),
                    joined_at: row.get("joined_at"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct GuildMembersUpsert {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    pk_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    inc_ops: std::collections::HashMap<&'static str, (&'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<
                        GuildMembers,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for GuildMembersUpsert {}
impl GuildMembersUpsert {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "guildmembers".to_string(),
            pk_values: std::collections::HashMap::new(),
            set_values: std::collections::HashMap::new(),
            inc_ops: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: i32) -> Self {
        self.pk_values.insert("id", Box::new(value));
        self
    }
    pub fn set_id(mut self, value: i32) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_guild_id(mut self, value: String) -> Self {
        self.set_values.insert("guild_id", Box::new(value));
        self
    }
    pub fn set_user_id(mut self, value: String) -> Self {
        self.set_values.insert("user_id", Box::new(value));
        self
    }
    pub fn set_nickname(mut self, value: Option<String>) -> Self {
        self.set_values.insert("nickname", Box::new(value));
        self
    }
    pub fn set_joined_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_values.insert("joined_at", Box::new(value));
        self
    }
    pub fn inc_id(mut self, amount: i64) -> Self {
        self.inc_ops.insert("id", ("inc", amount));
        self.set_values.insert("id", Box::new(amount));
        self
    }
    pub fn dec_id(mut self, amount: i64) -> Self {
        self.inc_ops.insert("id", ("dec", amount));
        self.set_values.insert("id", Box::new(-amount));
        self
    }
    pub fn mul_id(mut self, factor: i64) -> Self {
        self.inc_ops.insert("id", ("mul", factor));
        self.set_values.insert("id", Box::new(0));
        self
    }
    pub fn div_id(mut self, divisor: i64) -> Self {
        self.inc_ops.insert("id", ("div", divisor));
        self.set_values.insert("id", Box::new(0));
        self
    }
}
impl std::future::Future for GuildMembersUpsert {
    type Output = Result<GuildMembers, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pk_columns = vec!["id"];
            for pk_col in &pk_columns {
                if !me.pk_values.contains_key(pk_col)
                    && !me.set_values.contains_key(pk_col)
                {
                    return std::task::Poll::Ready(
                        Err(format!("Missing primary key field: {}", pk_col).into()),
                    );
                }
            }
            let mut all_values = std::mem::take(&mut me.pk_values);
            for (k, v) in std::mem::take(&mut me.set_values) {
                all_values.insert(k, v);
            }
            if all_values.is_empty() {
                return std::task::Poll::Ready(Err("No fields to upsert".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let inc_ops = std::mem::take(&mut me.inc_ops);
            let conflict_clause = "id".to_string();
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let mut columns: Vec<&str> = all_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(all_values.get(col).unwrap().as_ref());
                }
                let update_columns: Vec<&str> = columns
                    .iter()
                    .filter(|col| !pk_columns.iter().any(|pk| pk == *col))
                    .copied()
                    .collect();
                let sql = if update_columns.is_empty() && inc_ops.is_empty() {
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause
                    )
                } else {
                    let mut update_clauses: Vec<String> = vec![];
                    for col in update_columns {
                        if let Some((op, value)) = inc_ops.get(col) {
                            let clause = match *op {
                                "inc" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) + {}", col, table, col, value
                                    )
                                }
                                "dec" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) - {}", col, table, col, value.abs()
                                    )
                                }
                                "mul" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) * {}", col, table, col, value
                                    )
                                }
                                "div" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) / {}", col, table, col, value
                                    )
                                }
                                _ => format!("{} = EXCLUDED.{}", col, col),
                            };
                            update_clauses.push(clause);
                        } else {
                            update_clauses.push(format!("{} = EXCLUDED.{}", col, col));
                        }
                    }
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {} RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause,
                        update_clauses.join(", ")
                    )
                };
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(GuildMembers {
                    id: row.get("id"),
                    guild_id: row.get("guild_id"),
                    user_id: row.get("user_id"),
                    nickname: row.get("nickname"),
                    joined_at: row.get("joined_at"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct GuildMembersCreate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<
                        GuildMembers,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for GuildMembersCreate {}
impl GuildMembersCreate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "guildmembers".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_values: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: i32) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("guild_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "guild_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_user_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("user_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_user_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "user_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_nickname(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("nickname".to_string(), self.where_args.len()));
        self
    }
    pub fn where_nickname_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "nickname", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_nickname_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "nickname"), 0));
        self
    }
    pub fn where_nickname_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "nickname"), 0));
        self
    }
    pub fn where_joined_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("joined_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_joined_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "joined_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_joined_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push((format!("{} >", "joined_at"), self.where_args.len()));
        self
    }
    pub fn where_joined_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments.push((format!("{} <", "joined_at"), self.where_args.len()));
        self
    }
    pub fn where_joined_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "joined_at"), self.where_args.len()));
        self
    }
    pub fn where_joined_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "joined_at"), self.where_args.len()));
        self
    }
    pub fn set_id(mut self, value: i32) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_guild_id(mut self, value: String) -> Self {
        self.set_values.insert("guild_id", Box::new(value));
        self
    }
    pub fn set_user_id(mut self, value: String) -> Self {
        self.set_values.insert("user_id", Box::new(value));
        self
    }
    pub fn set_nickname(mut self, value: Option<String>) -> Self {
        self.set_values.insert("nickname", Box::new(value));
        self
    }
    pub fn set_joined_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_values.insert("joined_at", Box::new(value));
        self
    }
}
impl std::future::Future for GuildMembersCreate {
    type Output = Result<GuildMembers, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let required_fields = vec!["guild_id", "user_id"];
            for req in &required_fields {
                if !me.set_values.contains_key(req) {
                    return std::task::Poll::Ready(
                        Err(format!("Missing required field: {}", req).into()),
                    );
                }
            }
            if me.set_values.is_empty() && !required_fields.is_empty() {
                return std::task::Poll::Ready(Err("No fields to create".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_fragments = std::mem::take(&mut me.where_fragments);
            let where_args = std::mem::take(&mut me.where_args);
            let set_values = std::mem::take(&mut me.set_values);
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                if !where_fragments.is_empty() {
                    let mut sql = format!("SELECT COUNT(*) FROM {}", table);
                    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                    let conds: Vec<String> = where_fragments
                        .iter()
                        .enumerate()
                        .map(|(i, (col, idx))| format!("{} = ${}", col, i + 1))
                        .collect();
                    sql.push_str(" WHERE ");
                    sql.push_str(&conds.join(" AND "));
                    for arg in &where_args {
                        params.push(arg.as_ref());
                    }
                    let row = client.query_one(&sql, &params[..]).await?;
                    let count: i64 = row.get(0);
                    if count > 0 {
                        return Err("Record already exists".into());
                    }
                }
                let mut columns: Vec<&str> = set_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(set_values.get(col).unwrap().as_ref());
                }
                let sql = format!(
                    "INSERT INTO {} ({}) VALUES ({}) RETURNING *", table, columns_str,
                    placeholders_str
                );
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(GuildMembers {
                    id: row.get("id"),
                    guild_id: row.get("guild_id"),
                    user_id: row.get("user_id"),
                    nickname: row.get("nickname"),
                    joined_at: row.get("joined_at"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct GuildMembersDelete {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for GuildMembersDelete {}
impl GuildMembersDelete {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "guildmembers".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: i32) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("guild_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "guild_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_user_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("user_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_user_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "user_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_nickname(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("nickname".to_string(), self.where_args.len()));
        self
    }
    pub fn where_nickname_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "nickname", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_nickname_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "nickname"), 0));
        self
    }
    pub fn where_nickname_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "nickname"), 0));
        self
    }
    pub fn where_joined_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("joined_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_joined_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "joined_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_joined_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push((format!("{} >", "joined_at"), self.where_args.len()));
        self
    }
    pub fn where_joined_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments.push((format!("{} <", "joined_at"), self.where_args.len()));
        self
    }
    pub fn where_joined_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "joined_at"), self.where_args.len()));
        self
    }
    pub fn where_joined_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "joined_at"), self.where_args.len()));
        self
    }
}
impl std::future::Future for GuildMembersDelete {
    type Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.where_fragments.is_empty() {
                return std::task::Poll::Ready(
                    Err("DELETE without WHERE clause is not allowed".into()),
                );
            }
            let mut sql = format!("DELETE FROM {}", me.table);
            let conds: Vec<String> = me
                .where_fragments
                .iter()
                .enumerate()
                .map(|(i, (col, idx))| { format!("{} = ${}", col, i + 1) })
                .collect();
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.where_args) {
                all_params.push(arg);
            }
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let count = client.execute(&sql, &params[..]).await?;
                Ok(count)
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
impl GuildMembers {
    pub async fn find_by_id(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        id: i32,
    ) -> Result<Option<GuildMembers>, Box<dyn std::error::Error + Send + Sync>> {
        let client = pool.get().await.map_err(|_| "Failed to get connection from pool")?;
        let sql = format!(
            "SELECT * FROM {} WHERE {} = $1", stringify!(GuildMembers) .to_lowercase(),
            "id"
        );
        debug::log_query(&sql, 1);
        let row_opt = client.query_opt(&sql, &[&id]).await?;
        Ok(
            row_opt
                .map(|row| GuildMembers {
                    id: row.get("id"),
                    guild_id: row.get("guild_id"),
                    user_id: row.get("user_id"),
                    nickname: row.get("nickname"),
                    joined_at: row.get("joined_at"),
                }),
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channels {
    pub id: String,
    pub name: String,
    pub guild_id: String,
    pub created_at: DateTime<Utc>,
    pub rate_limit_per_user: i32,
}
pub struct ChannelsWhereBuilder {
    where_clauses: Vec<String>,
    order_by: Vec<(String, String)>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
}
impl ChannelsWhereBuilder {
    pub fn new() -> Self {
        Self {
            where_clauses: vec![],
            order_by: vec![],
            args: vec![],
            limit: None,
            offset: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_name(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "name", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_name_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "name", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "guild_id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "guild_id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "created_at", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} > ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} < ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} >= ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} <= ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_rate_limit_per_user(mut self, value: i32) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "rate_limit_per_user", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_rate_limit_per_user_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "rate_limit_per_user", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn order_by_id_asc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_id_desc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_name_asc(mut self) -> Self {
        self.order_by.push(("name".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_name_desc(mut self) -> Self {
        self.order_by.push(("name".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_guild_id_asc(mut self) -> Self {
        self.order_by.push(("guild_id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_guild_id_desc(mut self) -> Self {
        self.order_by.push(("guild_id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_created_at_asc(mut self) -> Self {
        self.order_by.push(("created_at".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_created_at_desc(mut self) -> Self {
        self.order_by.push(("created_at".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_rate_limit_per_user_asc(mut self) -> Self {
        self.order_by.push(("rate_limit_per_user".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_rate_limit_per_user_desc(mut self) -> Self {
        self.order_by.push(("rate_limit_per_user".to_string(), "DESC".to_string()));
        self
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
}
pub struct ChannelsQuery {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_clauses: Vec<String>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
    order_by: Vec<(String, String)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<
                        Vec<Channels>,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for ChannelsQuery {}
impl ChannelsQuery {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "channels".to_string(),
            where_clauses: vec![],
            args: vec![],
            limit: None,
            offset: None,
            order_by: vec![],
            fut: None,
        }
    }
    pub fn from_builder(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        builder: ChannelsWhereBuilder,
    ) -> Self {
        Self {
            pool,
            table: "channels".to_string(),
            where_clauses: builder.where_clauses,
            args: builder.args,
            limit: builder.limit,
            offset: builder.offset,
            order_by: builder.order_by,
            fut: None,
        }
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
    pub async fn first(
        self,
    ) -> Result<Option<Channels>, Box<dyn std::error::Error + Send + Sync>> {
        let result = self.limit(1).await?;
        Ok(result.into_iter().next())
    }
    pub async fn count(self) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn aggregate<T>(
        self,
        field: &str,
        func: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let func_upper = func.to_uppercase();
        let mut sql = format!("SELECT {}({}) FROM {}", func_upper, field, self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum<T>(
        self,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let default_val = T::default();
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(SUM({}), ${}) FROM {}", field, default_idx, self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum_cast_i64(
        self,
        field: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let default_val: i64 = 0;
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(CAST(SUM({}) AS BIGINT), ${}) FROM {}", field, default_idx,
            self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn avg<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "AVG").await
    }
    pub async fn min<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MIN").await
    }
    pub async fn max<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MAX").await
    }
}
impl Future for ChannelsQuery {
    type Output = Result<Vec<Channels>, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_clauses = std::mem::take(&mut me.where_clauses);
            let limit = me.limit;
            let offset = me.offset;
            let order_by = std::mem::take(&mut me.order_by);
            let args = std::mem::take(&mut me.args);
            let fut = async move {
                let mut sql = format!("SELECT * FROM {}", table);
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = args
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                if !where_clauses.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&where_clauses.join(" AND "));
                }
                if !order_by.is_empty() {
                    let order_clauses: Vec<String> = order_by
                        .iter()
                        .map(|(col, dir)| format!("{} {}", col, dir))
                        .collect();
                    sql.push_str(" ORDER BY ");
                    sql.push_str(&order_clauses.join(", "));
                }
                if let Some(limit) = limit {
                    sql.push_str(&format!(" LIMIT {}", limit));
                }
                if let Some(offset) = offset {
                    sql.push_str(&format!(" OFFSET {}", offset));
                }
                debug::log_query(&sql, params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let rows = client.query(&sql, &params[..]).await?;
                Ok(
                    rows
                        .into_iter()
                        .map(|row| Channels {
                            id: row.get("id"),
                            name: row.get("name"),
                            guild_id: row.get("guild_id"),
                            created_at: row.get("created_at"),
                            rate_limit_per_user: row.get("rate_limit_per_user"),
                        })
                        .collect(),
                )
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct ChannelsUpdate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_fragments: Vec<&'static str>,
    set_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    inc_ops: Vec<(&'static str, &'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Channels, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for ChannelsUpdate {}
impl ChannelsUpdate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "channels".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_fragments: vec![],
            set_args: vec![],
            inc_ops: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_name(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("name".to_string(), self.where_args.len()));
        self
    }
    pub fn where_name_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "name", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("guild_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "guild_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_rate_limit_per_user(mut self, value: i32) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push(("rate_limit_per_user".to_string(), self.where_args.len()));
        self
    }
    pub fn where_rate_limit_per_user_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!(
            "{} IN ({})", "rate_limit_per_user", placeholders.join(", ")
        );
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("id");
        self
    }
    pub fn set_name(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("name");
        self
    }
    pub fn set_guild_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("guild_id");
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("created_at");
        self
    }
    pub fn set_rate_limit_per_user(mut self, value: i32) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("rate_limit_per_user");
        self
    }
    pub fn inc_rate_limit_per_user(mut self, amount: i64) -> Self {
        self.inc_ops.push(("rate_limit_per_user", "inc", amount));
        self
    }
    pub fn dec_rate_limit_per_user(mut self, amount: i64) -> Self {
        self.inc_ops.push(("rate_limit_per_user", "dec", amount));
        self
    }
    pub fn mul_rate_limit_per_user(mut self, factor: i64) -> Self {
        self.inc_ops.push(("rate_limit_per_user", "mul", factor));
        self
    }
    pub fn div_rate_limit_per_user(mut self, divisor: i64) -> Self {
        self.inc_ops.push(("rate_limit_per_user", "div", divisor));
        self
    }
}
impl std::future::Future for ChannelsUpdate {
    type Output = Result<Channels, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.set_fragments.is_empty() && me.inc_ops.is_empty() {
                return std::task::Poll::Ready(Err("No fields to update".into()));
            }
            let mut sql = format!("UPDATE {} SET ", me.table);
            let mut set_clauses: Vec<String> = vec![];
            let mut param_idx = 1;
            for col in me.set_fragments.iter() {
                set_clauses.push(format!("{} = ${}", col, param_idx));
                param_idx += 1;
            }
            for (field, op, _) in &me.inc_ops {
                let clause = match *op {
                    "inc" => format!("{} = {} + ${}", field, field, param_idx),
                    "dec" => format!("{} = {} - ${}", field, field, param_idx),
                    "mul" => format!("{} = {} * ${}", field, field, param_idx),
                    "div" => format!("{} = {} / ${}", field, field, param_idx),
                    _ => continue,
                };
                set_clauses.push(clause);
                param_idx += 1;
            }
            sql.push_str(&set_clauses.join(", "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.set_args) {
                all_params.push(arg);
            }
            for (_, _, val) in &me.inc_ops {
                all_params.push(Box::new(*val));
            }
            if !me.where_fragments.is_empty() {
                let where_clauses: Vec<String> = me
                    .where_fragments
                    .iter()
                    .enumerate()
                    .map(|(i, (col, _))| format!("{} = ${}", col, param_idx + i))
                    .collect();
                sql.push_str(" WHERE ");
                sql.push_str(&where_clauses.join(" AND "));
                for arg in std::mem::take(&mut me.where_args) {
                    all_params.push(arg);
                }
            }
            sql.push_str(" RETURNING *");
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Channels {
                    id: row.get("id"),
                    name: row.get("name"),
                    guild_id: row.get("guild_id"),
                    created_at: row.get("created_at"),
                    rate_limit_per_user: row.get("rate_limit_per_user"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct ChannelsUpsert {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    pk_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    inc_ops: std::collections::HashMap<&'static str, (&'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Channels, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for ChannelsUpsert {}
impl ChannelsUpsert {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "channels".to_string(),
            pk_values: std::collections::HashMap::new(),
            set_values: std::collections::HashMap::new(),
            inc_ops: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.pk_values.insert("id", Box::new(value));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_name(mut self, value: String) -> Self {
        self.set_values.insert("name", Box::new(value));
        self
    }
    pub fn set_guild_id(mut self, value: String) -> Self {
        self.set_values.insert("guild_id", Box::new(value));
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_values.insert("created_at", Box::new(value));
        self
    }
    pub fn set_rate_limit_per_user(mut self, value: i32) -> Self {
        self.set_values.insert("rate_limit_per_user", Box::new(value));
        self
    }
    pub fn inc_rate_limit_per_user(mut self, amount: i64) -> Self {
        self.inc_ops.insert("rate_limit_per_user", ("inc", amount));
        self.set_values.insert("rate_limit_per_user", Box::new(amount));
        self
    }
    pub fn dec_rate_limit_per_user(mut self, amount: i64) -> Self {
        self.inc_ops.insert("rate_limit_per_user", ("dec", amount));
        self.set_values.insert("rate_limit_per_user", Box::new(-amount));
        self
    }
    pub fn mul_rate_limit_per_user(mut self, factor: i64) -> Self {
        self.inc_ops.insert("rate_limit_per_user", ("mul", factor));
        self.set_values.insert("rate_limit_per_user", Box::new(0));
        self
    }
    pub fn div_rate_limit_per_user(mut self, divisor: i64) -> Self {
        self.inc_ops.insert("rate_limit_per_user", ("div", divisor));
        self.set_values.insert("rate_limit_per_user", Box::new(0));
        self
    }
}
impl std::future::Future for ChannelsUpsert {
    type Output = Result<Channels, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pk_columns = vec!["id"];
            for pk_col in &pk_columns {
                if !me.pk_values.contains_key(pk_col)
                    && !me.set_values.contains_key(pk_col)
                {
                    return std::task::Poll::Ready(
                        Err(format!("Missing primary key field: {}", pk_col).into()),
                    );
                }
            }
            let mut all_values = std::mem::take(&mut me.pk_values);
            for (k, v) in std::mem::take(&mut me.set_values) {
                all_values.insert(k, v);
            }
            if all_values.is_empty() {
                return std::task::Poll::Ready(Err("No fields to upsert".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let inc_ops = std::mem::take(&mut me.inc_ops);
            let conflict_clause = "id".to_string();
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let mut columns: Vec<&str> = all_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(all_values.get(col).unwrap().as_ref());
                }
                let update_columns: Vec<&str> = columns
                    .iter()
                    .filter(|col| !pk_columns.iter().any(|pk| pk == *col))
                    .copied()
                    .collect();
                let sql = if update_columns.is_empty() && inc_ops.is_empty() {
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause
                    )
                } else {
                    let mut update_clauses: Vec<String> = vec![];
                    for col in update_columns {
                        if let Some((op, value)) = inc_ops.get(col) {
                            let clause = match *op {
                                "inc" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) + {}", col, table, col, value
                                    )
                                }
                                "dec" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) - {}", col, table, col, value.abs()
                                    )
                                }
                                "mul" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) * {}", col, table, col, value
                                    )
                                }
                                "div" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) / {}", col, table, col, value
                                    )
                                }
                                _ => format!("{} = EXCLUDED.{}", col, col),
                            };
                            update_clauses.push(clause);
                        } else {
                            update_clauses.push(format!("{} = EXCLUDED.{}", col, col));
                        }
                    }
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {} RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause,
                        update_clauses.join(", ")
                    )
                };
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Channels {
                    id: row.get("id"),
                    name: row.get("name"),
                    guild_id: row.get("guild_id"),
                    created_at: row.get("created_at"),
                    rate_limit_per_user: row.get("rate_limit_per_user"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct ChannelsCreate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Channels, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for ChannelsCreate {}
impl ChannelsCreate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "channels".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_values: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_name(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("name".to_string(), self.where_args.len()));
        self
    }
    pub fn where_name_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "name", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("guild_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "guild_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_rate_limit_per_user(mut self, value: i32) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push(("rate_limit_per_user".to_string(), self.where_args.len()));
        self
    }
    pub fn where_rate_limit_per_user_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!(
            "{} IN ({})", "rate_limit_per_user", placeholders.join(", ")
        );
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_name(mut self, value: String) -> Self {
        self.set_values.insert("name", Box::new(value));
        self
    }
    pub fn set_guild_id(mut self, value: String) -> Self {
        self.set_values.insert("guild_id", Box::new(value));
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_values.insert("created_at", Box::new(value));
        self
    }
    pub fn set_rate_limit_per_user(mut self, value: i32) -> Self {
        self.set_values.insert("rate_limit_per_user", Box::new(value));
        self
    }
}
impl std::future::Future for ChannelsCreate {
    type Output = Result<Channels, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let required_fields = vec!["id", "guild_id"];
            for req in &required_fields {
                if !me.set_values.contains_key(req) {
                    return std::task::Poll::Ready(
                        Err(format!("Missing required field: {}", req).into()),
                    );
                }
            }
            if me.set_values.is_empty() && !required_fields.is_empty() {
                return std::task::Poll::Ready(Err("No fields to create".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_fragments = std::mem::take(&mut me.where_fragments);
            let where_args = std::mem::take(&mut me.where_args);
            let set_values = std::mem::take(&mut me.set_values);
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                if !where_fragments.is_empty() {
                    let mut sql = format!("SELECT COUNT(*) FROM {}", table);
                    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                    let conds: Vec<String> = where_fragments
                        .iter()
                        .enumerate()
                        .map(|(i, (col, idx))| format!("{} = ${}", col, i + 1))
                        .collect();
                    sql.push_str(" WHERE ");
                    sql.push_str(&conds.join(" AND "));
                    for arg in &where_args {
                        params.push(arg.as_ref());
                    }
                    let row = client.query_one(&sql, &params[..]).await?;
                    let count: i64 = row.get(0);
                    if count > 0 {
                        return Err("Record already exists".into());
                    }
                }
                let mut columns: Vec<&str> = set_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(set_values.get(col).unwrap().as_ref());
                }
                let sql = format!(
                    "INSERT INTO {} ({}) VALUES ({}) RETURNING *", table, columns_str,
                    placeholders_str
                );
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Channels {
                    id: row.get("id"),
                    name: row.get("name"),
                    guild_id: row.get("guild_id"),
                    created_at: row.get("created_at"),
                    rate_limit_per_user: row.get("rate_limit_per_user"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct ChannelsDelete {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for ChannelsDelete {}
impl ChannelsDelete {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "channels".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_name(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("name".to_string(), self.where_args.len()));
        self
    }
    pub fn where_name_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "name", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("guild_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "guild_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_rate_limit_per_user(mut self, value: i32) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push(("rate_limit_per_user".to_string(), self.where_args.len()));
        self
    }
    pub fn where_rate_limit_per_user_in(mut self, values: Vec<i32>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!(
            "{} IN ({})", "rate_limit_per_user", placeholders.join(", ")
        );
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
}
impl std::future::Future for ChannelsDelete {
    type Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.where_fragments.is_empty() {
                return std::task::Poll::Ready(
                    Err("DELETE without WHERE clause is not allowed".into()),
                );
            }
            let mut sql = format!("DELETE FROM {}", me.table);
            let conds: Vec<String> = me
                .where_fragments
                .iter()
                .enumerate()
                .map(|(i, (col, idx))| { format!("{} = ${}", col, i + 1) })
                .collect();
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.where_args) {
                all_params.push(arg);
            }
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let count = client.execute(&sql, &params[..]).await?;
                Ok(count)
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
impl Channels {
    pub async fn find_by_id(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        id: String,
    ) -> Result<Option<Channels>, Box<dyn std::error::Error + Send + Sync>> {
        let client = pool.get().await.map_err(|_| "Failed to get connection from pool")?;
        let sql = format!(
            "SELECT * FROM {} WHERE {} = $1", stringify!(Channels) .to_lowercase(), "id"
        );
        debug::log_query(&sql, 1);
        let row_opt = client.query_opt(&sql, &[&id]).await?;
        Ok(
            row_opt
                .map(|row| Channels {
                    id: row.get("id"),
                    name: row.get("name"),
                    guild_id: row.get("guild_id"),
                    created_at: row.get("created_at"),
                    rate_limit_per_user: row.get("rate_limit_per_user"),
                }),
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Messages {
    pub id: String,
    pub author_id: String,
    pub channel_id: String,
    pub guild_id: String,
    pub content: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub message_type: String,
    pub nonce: String,
}
pub struct MessagesWhereBuilder {
    where_clauses: Vec<String>,
    order_by: Vec<(String, String)>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
}
impl MessagesWhereBuilder {
    pub fn new() -> Self {
        Self {
            where_clauses: vec![],
            order_by: vec![],
            args: vec![],
            limit: None,
            offset: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_author_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "author_id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_author_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "author_id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_channel_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "channel_id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_channel_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "channel_id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "guild_id", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "guild_id", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_content(mut self, value: Option<String>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "content", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_content_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "content", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_content_is_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NULL", "content"));
        self
    }
    pub fn where_content_is_not_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NOT NULL", "content"));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "created_at", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} > ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} < ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} >= ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} <= ${}", "created_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_updated_at(mut self, value: Option<DateTime<Utc>>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "updated_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_updated_at_in(mut self, values: Vec<Option<DateTime<Utc>>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "updated_at", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_updated_at_is_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NULL", "updated_at"));
        self
    }
    pub fn where_updated_at_is_not_null(mut self) -> Self {
        self.where_clauses.push(format!("{} IS NOT NULL", "updated_at"));
        self
    }
    pub fn where_updated_at_gt(mut self, value: Option<DateTime<Utc>>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} > ${}", "updated_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_updated_at_lt(mut self, value: Option<DateTime<Utc>>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} < ${}", "updated_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_updated_at_gte(mut self, value: Option<DateTime<Utc>>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} >= ${}", "updated_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_updated_at_lte(mut self, value: Option<DateTime<Utc>>) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} <= ${}", "updated_at", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_message_type(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "message_type", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_message_type_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses
            .push(format!("{} IN ({})", "message_type", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn where_nonce(mut self, value: String) -> Self {
        let param_idx = self.args.len() + 1;
        self.where_clauses.push(format!("{} = ${}", "nonce", param_idx));
        self.args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self
    }
    pub fn where_nonce_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        self.where_clauses.push(format!("{} IN ({})", "nonce", placeholders.join(", ")));
        for value in values {
            self.args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self
    }
    pub fn order_by_id_asc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_id_desc(mut self) -> Self {
        self.order_by.push(("id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_author_id_asc(mut self) -> Self {
        self.order_by.push(("author_id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_author_id_desc(mut self) -> Self {
        self.order_by.push(("author_id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_channel_id_asc(mut self) -> Self {
        self.order_by.push(("channel_id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_channel_id_desc(mut self) -> Self {
        self.order_by.push(("channel_id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_guild_id_asc(mut self) -> Self {
        self.order_by.push(("guild_id".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_guild_id_desc(mut self) -> Self {
        self.order_by.push(("guild_id".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_content_asc(mut self) -> Self {
        self.order_by.push(("content".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_content_desc(mut self) -> Self {
        self.order_by.push(("content".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_created_at_asc(mut self) -> Self {
        self.order_by.push(("created_at".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_created_at_desc(mut self) -> Self {
        self.order_by.push(("created_at".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_updated_at_asc(mut self) -> Self {
        self.order_by.push(("updated_at".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_updated_at_desc(mut self) -> Self {
        self.order_by.push(("updated_at".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_message_type_asc(mut self) -> Self {
        self.order_by.push(("message_type".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_message_type_desc(mut self) -> Self {
        self.order_by.push(("message_type".to_string(), "DESC".to_string()));
        self
    }
    pub fn order_by_nonce_asc(mut self) -> Self {
        self.order_by.push(("nonce".to_string(), "ASC".to_string()));
        self
    }
    pub fn order_by_nonce_desc(mut self) -> Self {
        self.order_by.push(("nonce".to_string(), "DESC".to_string()));
        self
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
}
pub struct MessagesQuery {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_clauses: Vec<String>,
    args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    limit: Option<usize>,
    offset: Option<usize>,
    order_by: Vec<(String, String)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<
                        Vec<Messages>,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for MessagesQuery {}
impl MessagesQuery {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "messages".to_string(),
            where_clauses: vec![],
            args: vec![],
            limit: None,
            offset: None,
            order_by: vec![],
            fut: None,
        }
    }
    pub fn from_builder(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        builder: MessagesWhereBuilder,
    ) -> Self {
        Self {
            pool,
            table: "messages".to_string(),
            where_clauses: builder.where_clauses,
            args: builder.args,
            limit: builder.limit,
            offset: builder.offset,
            order_by: builder.order_by,
            fut: None,
        }
    }
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
    pub async fn first(
        self,
    ) -> Result<Option<Messages>, Box<dyn std::error::Error + Send + Sync>> {
        let result = self.limit(1).await?;
        Ok(result.into_iter().next())
    }
    pub async fn count(self) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let mut sql = format!("SELECT COUNT(*) FROM {}", self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn aggregate<T>(
        self,
        field: &str,
        func: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        let func_upper = func.to_uppercase();
        let mut sql = format!("SELECT {}({}) FROM {}", func_upper, field, self.table);
        let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum<T>(
        self,
        field: &str,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a> + Default
            + tokio_postgres::types::ToSql + Sync,
    {
        let default_val = T::default();
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(SUM({}), ${}) FROM {}", field, default_idx, self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn sum_cast_i64(
        self,
        field: &str,
    ) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let default_val: i64 = 0;
        let default_idx = self.args.len() + 1;
        let mut sql = format!(
            "SELECT COALESCE(CAST(SUM({}) AS BIGINT), ${}) FROM {}", field, default_idx,
            self.table
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = self
            .args
            .iter()
            .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        if !self.where_clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.where_clauses.join(" AND "));
        }
        params.push(&default_val);
        debug::log_query(&sql, params.len());
        let client = self
            .pool
            .get()
            .await
            .map_err(|_| "Failed to get connection from pool")?;
        let row = client.query_one(&sql, &params[..]).await?;
        Ok(row.get(0))
    }
    pub async fn avg<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "AVG").await
    }
    pub async fn min<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MIN").await
    }
    pub async fn max<T>(
        self,
        field: &str,
    ) -> Result<Option<T>, Box<dyn std::error::Error + Send + Sync>>
    where
        T: for<'a> tokio_postgres::types::FromSql<'a>,
    {
        self.aggregate(field, "MAX").await
    }
}
impl Future for MessagesQuery {
    type Output = Result<Vec<Messages>, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_clauses = std::mem::take(&mut me.where_clauses);
            let limit = me.limit;
            let offset = me.offset;
            let order_by = std::mem::take(&mut me.order_by);
            let args = std::mem::take(&mut me.args);
            let fut = async move {
                let mut sql = format!("SELECT * FROM {}", table);
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = args
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                if !where_clauses.is_empty() {
                    sql.push_str(" WHERE ");
                    sql.push_str(&where_clauses.join(" AND "));
                }
                if !order_by.is_empty() {
                    let order_clauses: Vec<String> = order_by
                        .iter()
                        .map(|(col, dir)| format!("{} {}", col, dir))
                        .collect();
                    sql.push_str(" ORDER BY ");
                    sql.push_str(&order_clauses.join(", "));
                }
                if let Some(limit) = limit {
                    sql.push_str(&format!(" LIMIT {}", limit));
                }
                if let Some(offset) = offset {
                    sql.push_str(&format!(" OFFSET {}", offset));
                }
                debug::log_query(&sql, params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let rows = client.query(&sql, &params[..]).await?;
                Ok(
                    rows
                        .into_iter()
                        .map(|row| Messages {
                            id: row.get("id"),
                            author_id: row.get("author_id"),
                            channel_id: row.get("channel_id"),
                            guild_id: row.get("guild_id"),
                            content: row.get("content"),
                            created_at: row.get("created_at"),
                            updated_at: row.get("updated_at"),
                            message_type: row.get("message_type"),
                            nonce: row.get("nonce"),
                        })
                        .collect(),
                )
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct MessagesUpdate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_fragments: Vec<&'static str>,
    set_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    inc_ops: Vec<(&'static str, &'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Messages, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for MessagesUpdate {}
impl MessagesUpdate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "messages".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_fragments: vec![],
            set_args: vec![],
            inc_ops: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_author_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("author_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_author_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "author_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_channel_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("channel_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_channel_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "channel_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("guild_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "guild_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_content(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("content".to_string(), self.where_args.len()));
        self
    }
    pub fn where_content_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "content", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_content_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "content"), 0));
        self
    }
    pub fn where_content_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "content"), 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("updated_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_updated_at_in(mut self, values: Vec<Option<DateTime<Utc>>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "updated_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_updated_at_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "updated_at"), 0));
        self
    }
    pub fn where_updated_at_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "updated_at"), 0));
        self
    }
    pub fn where_updated_at_gt(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at_lt(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at_gte(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at_lte(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_message_type(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("message_type".to_string(), self.where_args.len()));
        self
    }
    pub fn where_message_type_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "message_type", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_nonce(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("nonce".to_string(), self.where_args.len()));
        self
    }
    pub fn where_nonce_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "nonce", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("id");
        self
    }
    pub fn set_author_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("author_id");
        self
    }
    pub fn set_channel_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("channel_id");
        self
    }
    pub fn set_guild_id(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("guild_id");
        self
    }
    pub fn set_content(mut self, value: Option<String>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("content");
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("created_at");
        self
    }
    pub fn set_updated_at(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("updated_at");
        self
    }
    pub fn set_message_type(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("message_type");
        self
    }
    pub fn set_nonce(mut self, value: String) -> Self {
        self.set_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.set_fragments.push("nonce");
        self
    }
}
impl std::future::Future for MessagesUpdate {
    type Output = Result<Messages, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.set_fragments.is_empty() && me.inc_ops.is_empty() {
                return std::task::Poll::Ready(Err("No fields to update".into()));
            }
            let mut sql = format!("UPDATE {} SET ", me.table);
            let mut set_clauses: Vec<String> = vec![];
            let mut param_idx = 1;
            for col in me.set_fragments.iter() {
                set_clauses.push(format!("{} = ${}", col, param_idx));
                param_idx += 1;
            }
            for (field, op, _) in &me.inc_ops {
                let clause = match *op {
                    "inc" => format!("{} = {} + ${}", field, field, param_idx),
                    "dec" => format!("{} = {} - ${}", field, field, param_idx),
                    "mul" => format!("{} = {} * ${}", field, field, param_idx),
                    "div" => format!("{} = {} / ${}", field, field, param_idx),
                    _ => continue,
                };
                set_clauses.push(clause);
                param_idx += 1;
            }
            sql.push_str(&set_clauses.join(", "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.set_args) {
                all_params.push(arg);
            }
            for (_, _, val) in &me.inc_ops {
                all_params.push(Box::new(*val));
            }
            if !me.where_fragments.is_empty() {
                let where_clauses: Vec<String> = me
                    .where_fragments
                    .iter()
                    .enumerate()
                    .map(|(i, (col, _))| format!("{} = ${}", col, param_idx + i))
                    .collect();
                sql.push_str(" WHERE ");
                sql.push_str(&where_clauses.join(" AND "));
                for arg in std::mem::take(&mut me.where_args) {
                    all_params.push(arg);
                }
            }
            sql.push_str(" RETURNING *");
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Messages {
                    id: row.get("id"),
                    author_id: row.get("author_id"),
                    channel_id: row.get("channel_id"),
                    guild_id: row.get("guild_id"),
                    content: row.get("content"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    message_type: row.get("message_type"),
                    nonce: row.get("nonce"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct MessagesUpsert {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    pk_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    inc_ops: std::collections::HashMap<&'static str, (&'static str, i64)>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Messages, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for MessagesUpsert {}
impl MessagesUpsert {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "messages".to_string(),
            pk_values: std::collections::HashMap::new(),
            set_values: std::collections::HashMap::new(),
            inc_ops: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.pk_values.insert("id", Box::new(value));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_author_id(mut self, value: String) -> Self {
        self.set_values.insert("author_id", Box::new(value));
        self
    }
    pub fn set_channel_id(mut self, value: String) -> Self {
        self.set_values.insert("channel_id", Box::new(value));
        self
    }
    pub fn set_guild_id(mut self, value: String) -> Self {
        self.set_values.insert("guild_id", Box::new(value));
        self
    }
    pub fn set_content(mut self, value: Option<String>) -> Self {
        self.set_values.insert("content", Box::new(value));
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_values.insert("created_at", Box::new(value));
        self
    }
    pub fn set_updated_at(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.set_values.insert("updated_at", Box::new(value));
        self
    }
    pub fn set_message_type(mut self, value: String) -> Self {
        self.set_values.insert("message_type", Box::new(value));
        self
    }
    pub fn set_nonce(mut self, value: String) -> Self {
        self.set_values.insert("nonce", Box::new(value));
        self
    }
}
impl std::future::Future for MessagesUpsert {
    type Output = Result<Messages, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let pk_columns = vec!["id"];
            for pk_col in &pk_columns {
                if !me.pk_values.contains_key(pk_col)
                    && !me.set_values.contains_key(pk_col)
                {
                    return std::task::Poll::Ready(
                        Err(format!("Missing primary key field: {}", pk_col).into()),
                    );
                }
            }
            let mut all_values = std::mem::take(&mut me.pk_values);
            for (k, v) in std::mem::take(&mut me.set_values) {
                all_values.insert(k, v);
            }
            if all_values.is_empty() {
                return std::task::Poll::Ready(Err("No fields to upsert".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let inc_ops = std::mem::take(&mut me.inc_ops);
            let conflict_clause = "id".to_string();
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let mut columns: Vec<&str> = all_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(all_values.get(col).unwrap().as_ref());
                }
                let update_columns: Vec<&str> = columns
                    .iter()
                    .filter(|col| !pk_columns.iter().any(|pk| pk == *col))
                    .copied()
                    .collect();
                let sql = if update_columns.is_empty() && inc_ops.is_empty() {
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause
                    )
                } else {
                    let mut update_clauses: Vec<String> = vec![];
                    for col in update_columns {
                        if let Some((op, value)) = inc_ops.get(col) {
                            let clause = match *op {
                                "inc" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) + {}", col, table, col, value
                                    )
                                }
                                "dec" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) - {}", col, table, col, value.abs()
                                    )
                                }
                                "mul" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) * {}", col, table, col, value
                                    )
                                }
                                "div" => {
                                    format!(
                                        "{} = COALESCE({}.{}, 0) / {}", col, table, col, value
                                    )
                                }
                                _ => format!("{} = EXCLUDED.{}", col, col),
                            };
                            update_clauses.push(clause);
                        } else {
                            update_clauses.push(format!("{} = EXCLUDED.{}", col, col));
                        }
                    }
                    format!(
                        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {} RETURNING *",
                        table, columns_str, placeholders_str, conflict_clause,
                        update_clauses.join(", ")
                    )
                };
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Messages {
                    id: row.get("id"),
                    author_id: row.get("author_id"),
                    channel_id: row.get("channel_id"),
                    guild_id: row.get("guild_id"),
                    content: row.get("content"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    message_type: row.get("message_type"),
                    nonce: row.get("nonce"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct MessagesCreate {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    set_values: std::collections::HashMap<
        &'static str,
        Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
    >,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<Messages, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for MessagesCreate {}
impl MessagesCreate {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "messages".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            set_values: std::collections::HashMap::new(),
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_author_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("author_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_author_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "author_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_channel_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("channel_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_channel_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "channel_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("guild_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "guild_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_content(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("content".to_string(), self.where_args.len()));
        self
    }
    pub fn where_content_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "content", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_content_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "content"), 0));
        self
    }
    pub fn where_content_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "content"), 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("updated_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_updated_at_in(mut self, values: Vec<Option<DateTime<Utc>>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "updated_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_updated_at_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "updated_at"), 0));
        self
    }
    pub fn where_updated_at_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "updated_at"), 0));
        self
    }
    pub fn where_updated_at_gt(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at_lt(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at_gte(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at_lte(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_message_type(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("message_type".to_string(), self.where_args.len()));
        self
    }
    pub fn where_message_type_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "message_type", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_nonce(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("nonce".to_string(), self.where_args.len()));
        self
    }
    pub fn where_nonce_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "nonce", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn set_id(mut self, value: String) -> Self {
        self.set_values.insert("id", Box::new(value));
        self
    }
    pub fn set_author_id(mut self, value: String) -> Self {
        self.set_values.insert("author_id", Box::new(value));
        self
    }
    pub fn set_channel_id(mut self, value: String) -> Self {
        self.set_values.insert("channel_id", Box::new(value));
        self
    }
    pub fn set_guild_id(mut self, value: String) -> Self {
        self.set_values.insert("guild_id", Box::new(value));
        self
    }
    pub fn set_content(mut self, value: Option<String>) -> Self {
        self.set_values.insert("content", Box::new(value));
        self
    }
    pub fn set_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.set_values.insert("created_at", Box::new(value));
        self
    }
    pub fn set_updated_at(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.set_values.insert("updated_at", Box::new(value));
        self
    }
    pub fn set_message_type(mut self, value: String) -> Self {
        self.set_values.insert("message_type", Box::new(value));
        self
    }
    pub fn set_nonce(mut self, value: String) -> Self {
        self.set_values.insert("nonce", Box::new(value));
        self
    }
}
impl std::future::Future for MessagesCreate {
    type Output = Result<Messages, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            let required_fields = vec!["id", "author_id", "channel_id", "guild_id"];
            for req in &required_fields {
                if !me.set_values.contains_key(req) {
                    return std::task::Poll::Ready(
                        Err(format!("Missing required field: {}", req).into()),
                    );
                }
            }
            if me.set_values.is_empty() && !required_fields.is_empty() {
                return std::task::Poll::Ready(Err("No fields to create".into()));
            }
            let pool = me.pool.clone();
            let table = me.table.clone();
            let where_fragments = std::mem::take(&mut me.where_fragments);
            let where_args = std::mem::take(&mut me.where_args);
            let set_values = std::mem::take(&mut me.set_values);
            let fut = async move {
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                if !where_fragments.is_empty() {
                    let mut sql = format!("SELECT COUNT(*) FROM {}", table);
                    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                    let conds: Vec<String> = where_fragments
                        .iter()
                        .enumerate()
                        .map(|(i, (col, idx))| format!("{} = ${}", col, i + 1))
                        .collect();
                    sql.push_str(" WHERE ");
                    sql.push_str(&conds.join(" AND "));
                    for arg in &where_args {
                        params.push(arg.as_ref());
                    }
                    let row = client.query_one(&sql, &params[..]).await?;
                    let count: i64 = row.get(0);
                    if count > 0 {
                        return Err("Record already exists".into());
                    }
                }
                let mut columns: Vec<&str> = set_values.keys().copied().collect();
                columns.sort();
                let columns_str = columns.join(", ");
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("${}", i))
                    .collect();
                let placeholders_str = placeholders.join(", ");
                let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![];
                for col in &columns {
                    params.push(set_values.get(col).unwrap().as_ref());
                }
                let sql = format!(
                    "INSERT INTO {} ({}) VALUES ({}) RETURNING *", table, columns_str,
                    placeholders_str
                );
                debug::log_query(&sql, params.len());
                let row = client.query_one(&sql, &params[..]).await?;
                Ok(Messages {
                    id: row.get("id"),
                    author_id: row.get("author_id"),
                    channel_id: row.get("channel_id"),
                    guild_id: row.get("guild_id"),
                    content: row.get("content"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    message_type: row.get("message_type"),
                    nonce: row.get("nonce"),
                })
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
pub struct MessagesDelete {
    pool: Arc<
        bb8::Pool<
            bb8_postgres::PostgresConnectionManager<
                tokio_postgres_rustls::MakeRustlsConnect,
            >,
        >,
    >,
    table: String,
    where_fragments: Vec<(String, usize)>,
    where_args: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>>,
    fut: Option<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
            >,
        >,
    >,
}
unsafe impl Send for MessagesDelete {}
impl MessagesDelete {
    pub fn new(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
    ) -> Self {
        Self {
            pool,
            table: "messages".to_string(),
            where_fragments: vec![],
            where_args: vec![],
            fut: None,
        }
    }
    pub fn where_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_author_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("author_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_author_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "author_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_channel_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("channel_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_channel_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "channel_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_guild_id(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("guild_id".to_string(), self.where_args.len()));
        self
    }
    pub fn where_guild_id_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "guild_id", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_content(mut self, value: Option<String>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("content".to_string(), self.where_args.len()));
        self
    }
    pub fn where_content_in(mut self, values: Vec<Option<String>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "content", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_content_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "content"), 0));
        self
    }
    pub fn where_content_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "content"), 0));
        self
    }
    pub fn where_created_at(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("created_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_created_at_in(mut self, values: Vec<DateTime<Utc>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "created_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_created_at_gt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lt(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_gte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_created_at_lte(mut self, value: DateTime<Utc>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "created_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("updated_at".to_string(), self.where_args.len()));
        self
    }
    pub fn where_updated_at_in(mut self, values: Vec<Option<DateTime<Utc>>>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "updated_at", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_updated_at_is_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NULL", "updated_at"), 0));
        self
    }
    pub fn where_updated_at_is_not_null(mut self) -> Self {
        self.where_fragments.push((format!("{} IS NOT NULL", "updated_at"), 0));
        self
    }
    pub fn where_updated_at_gt(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at_lt(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Send + Sync>,
            );
        self.where_fragments
            .push((format!("{} <", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at_gte(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} >=", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_updated_at_lte(mut self, value: Option<DateTime<Utc>>) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments
            .push((format!("{} <=", "updated_at"), self.where_args.len()));
        self
    }
    pub fn where_message_type(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("message_type".to_string(), self.where_args.len()));
        self
    }
    pub fn where_message_type_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "message_type", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
    pub fn where_nonce(mut self, value: String) -> Self {
        self.where_args
            .push(
                Box::new(value) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            );
        self.where_fragments.push(("nonce".to_string(), self.where_args.len()));
        self
    }
    pub fn where_nonce_in(mut self, values: Vec<String>) -> Self {
        if values.is_empty() {
            return self;
        }
        let start_idx = self.where_args.len() + 1;
        let placeholders: Vec<String> = (start_idx..start_idx + values.len())
            .map(|i| format!("${}", i))
            .collect();
        let in_clause = format!("{} IN ({})", "nonce", placeholders.join(", "));
        for value in values {
            self.where_args
                .push(
                    Box::new(value)
                        as Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
                );
        }
        self.where_fragments.push((in_clause, 0));
        self
    }
}
impl std::future::Future for MessagesDelete {
    type Output = Result<u64, Box<dyn std::error::Error + Send + Sync>>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let me = &mut *self;
        if me.fut.is_none() {
            if me.where_fragments.is_empty() {
                return std::task::Poll::Ready(
                    Err("DELETE without WHERE clause is not allowed".into()),
                );
            }
            let mut sql = format!("DELETE FROM {}", me.table);
            let conds: Vec<String> = me
                .where_fragments
                .iter()
                .enumerate()
                .map(|(i, (col, idx))| { format!("{} = ${}", col, i + 1) })
                .collect();
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
            let mut all_params: Vec<
                Box<dyn tokio_postgres::types::ToSql + Sync + Send>,
            > = vec![];
            for arg in std::mem::take(&mut me.where_args) {
                all_params.push(arg);
            }
            let pool = me.pool.clone();
            let fut = async move {
                debug::log_query(&sql, all_params.len());
                let client = pool
                    .get()
                    .await
                    .map_err(|_| "Failed to get connection from pool")?;
                let params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = all_params
                    .iter()
                    .map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
                    .collect();
                let count = client.execute(&sql, &params[..]).await?;
                Ok(count)
            };
            me.fut = Some(Box::pin(fut));
        }
        me.fut.as_mut().unwrap().as_mut().poll(cx)
    }
}
impl Messages {
    pub async fn find_by_id(
        pool: Arc<
            bb8::Pool<
                bb8_postgres::PostgresConnectionManager<
                    tokio_postgres_rustls::MakeRustlsConnect,
                >,
            >,
        >,
        id: String,
    ) -> Result<Option<Messages>, Box<dyn std::error::Error + Send + Sync>> {
        let client = pool.get().await.map_err(|_| "Failed to get connection from pool")?;
        let sql = format!(
            "SELECT * FROM {} WHERE {} = $1", stringify!(Messages) .to_lowercase(), "id"
        );
        debug::log_query(&sql, 1);
        let row_opt = client.query_opt(&sql, &[&id]).await?;
        Ok(
            row_opt
                .map(|row| Messages {
                    id: row.get("id"),
                    author_id: row.get("author_id"),
                    channel_id: row.get("channel_id"),
                    guild_id: row.get("guild_id"),
                    content: row.get("content"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    message_type: row.get("message_type"),
                    nonce: row.get("nonce"),
                }),
        )
    }
}
