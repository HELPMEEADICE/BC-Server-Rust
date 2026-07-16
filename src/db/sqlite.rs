use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use tracing::info;

use crate::config::Config;
use crate::protocol::MemberNumber;

#[derive(Clone)]
pub struct SqliteDb {
    pool: Arc<SqlitePool>,
}

impl SqliteDb {
    pub async fn connect(config: &Config) -> Result<Self> {
        let connect_url = normalize_sqlite_url(&config.database_url)?;
        ensure_sqlite_parent_dir(&connect_url)?;

        let options = connect_url
            .parse::<SqliteConnectOptions>()
            .context("parse SQLite DATABASE_URL")?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await
            .context("connect SQLite")?;

        let db = Self {
            pool: Arc::new(pool),
        };
        db.migrate().await?;
        info!(url = %config.database_url, "SQLite connected");
        Ok(db)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS accounts (
                account_name  TEXT PRIMARY KEY COLLATE NOCASE NOT NULL,
                email         TEXT,
                password      TEXT NOT NULL,
                member_number INTEGER UNIQUE,
                name          TEXT,
                last_login    INTEGER,
                creation      INTEGER,
                data_json     TEXT NOT NULL
            )
            "#,
        )
        .execute(self.pool.as_ref())
        .await
        .context("sqlite create accounts")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_accounts_email ON accounts(email COLLATE NOCASE)",
        )
        .execute(self.pool.as_ref())
        .await
        .context("sqlite index email")?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_accounts_member ON accounts(member_number)")
            .execute(self.pool.as_ref())
            .await
            .context("sqlite index member")?;

        Ok(())
    }

    pub async fn next_member_number(&self) -> Result<MemberNumber> {
        let row = sqlx::query("SELECT MAX(member_number) AS m FROM accounts")
            .fetch_one(self.pool.as_ref())
            .await?;
        let max: Option<i64> = row.try_get("m")?;
        Ok(max.unwrap_or(0) + 1)
    }

    pub async fn find_by_account_name(&self, account_name: &str) -> Result<Option<Value>> {
        let row = sqlx::query(
            r#"
            SELECT account_name, email, password, member_number, name, last_login, creation, data_json
            FROM accounts WHERE account_name = ?1 COLLATE NOCASE
            "#,
        )
        .bind(account_name)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|r| row_to_json(&r)).transpose()?)
    }

    pub async fn find_by_member_number(&self, member_number: MemberNumber) -> Result<Option<Value>> {
        let row = sqlx::query(
            r#"
            SELECT account_name, email, password, member_number, name, last_login, creation, data_json
            FROM accounts WHERE member_number = ?1
            "#,
        )
        .bind(member_number)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|r| row_to_json(&r)).transpose()?)
    }

    pub async fn find_email_by_account_name(&self, account_name: &str) -> Result<Option<String>> {
        let row = sqlx::query(
            "SELECT email FROM accounts WHERE account_name = ?1 COLLATE NOCASE",
        )
        .bind(account_name)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.and_then(|r| r.try_get::<Option<String>, _>("email").ok().flatten()))
    }

    pub async fn find_account_names_by_email(&self, email: &str) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT account_name FROM accounts
            WHERE email IS NOT NULL AND lower(trim(email)) = lower(trim(?1))
            "#,
        )
        .bind(email)
        .fetch_all(self.pool.as_ref())
        .await?;
        let mut names = Vec::with_capacity(rows.len());
        for r in rows {
            names.push(r.try_get::<String, _>("account_name")?);
        }
        Ok(names)
    }

    pub async fn insert_account(&self, account: Value) -> Result<()> {
        let obj = account
            .as_object()
            .context("insert_account expects object")?;
        let account_name = obj
            .get("AccountName")
            .and_then(|v| v.as_str())
            .context("AccountName required")?
            .to_string();
        let email = obj
            .get("Email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let password = obj
            .get("Password")
            .and_then(|v| v.as_str())
            .context("Password required")?
            .to_string();
        let member_number = obj.get("MemberNumber").and_then(|v| v.as_i64());
        let name = obj
            .get("Name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let last_login = obj.get("LastLogin").and_then(|v| v.as_i64());
        let creation = obj.get("Creation").and_then(|v| v.as_i64());
        let data_json = serde_json::to_string(&account).context("serialize account json")?;

        sqlx::query(
            r#"
            INSERT INTO accounts
                (account_name, email, password, member_number, name, last_login, creation, data_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
        )
        .bind(&account_name)
        .bind(email)
        .bind(password)
        .bind(member_number)
        .bind(name)
        .bind(last_login)
        .bind(creation)
        .bind(data_json)
        .execute(self.pool.as_ref())
        .await
        .context("sqlite insert account")?;
        Ok(())
    }

    pub async fn update_fields(&self, account_name: &str, set: Map<String, Value>) -> Result<()> {
        let mut tx = self.pool.begin().await.context("sqlite begin")?;
        let row = sqlx::query(
            r#"
            SELECT account_name, email, password, member_number, name, last_login, creation, data_json
            FROM accounts WHERE account_name = ?1 COLLATE NOCASE
            "#,
        )
        .bind(account_name)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            return Ok(());
        };

        let (merged, cols) = merge_row_with_set(&row, &set)?;
        sqlx::query(
            r#"
            UPDATE accounts SET
                email = ?2,
                password = ?3,
                member_number = ?4,
                name = ?5,
                last_login = ?6,
                creation = ?7,
                data_json = ?8
            WHERE account_name = ?1 COLLATE NOCASE
            "#,
        )
        .bind(&cols.account_name)
        .bind(&cols.email)
        .bind(&cols.password)
        .bind(cols.member_number)
        .bind(&cols.name)
        .bind(cols.last_login)
        .bind(cols.creation)
        .bind(merged)
        .execute(&mut *tx)
        .await
        .context("sqlite update_fields")?;
        tx.commit().await.context("sqlite commit")?;
        Ok(())
    }

    pub async fn update_fields_by_member_number(
        &self,
        member_number: MemberNumber,
        set: Map<String, Value>,
    ) -> Result<()> {
        let row = sqlx::query(
            "SELECT account_name FROM accounts WHERE member_number = ?1",
        )
        .bind(member_number)
        .fetch_optional(self.pool.as_ref())
        .await?;
        let Some(row) = row else {
            return Ok(());
        };
        let account_name: String = row.try_get("account_name")?;
        self.update_fields(&account_name, set).await
    }

    pub async fn pull_lovership_member(
        &self,
        target_member: MemberNumber,
        source_member: MemberNumber,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.context("sqlite begin")?;
        let row = sqlx::query(
            r#"
            SELECT account_name, email, password, member_number, name, last_login, creation, data_json
            FROM accounts WHERE member_number = ?1
            "#,
        )
        .bind(target_member)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = row else {
            return Ok(());
        };

        let mut data = row_to_json(&row)?;
        let obj = data
            .as_object_mut()
            .context("account json must be object")?;
        if let Some(Value::Array(list)) = obj.get_mut("Lovership") {
            list.retain(|entry| {
                entry
                    .get("MemberNumber")
                    .and_then(|v| v.as_i64())
                    != Some(source_member)
            });
        }

        let account_name = obj
            .get("AccountName")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let email = obj
            .get("Email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let password = obj
            .get("Password")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let member_number = obj.get("MemberNumber").and_then(|v| v.as_i64());
        let name = obj
            .get("Name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let last_login = obj.get("LastLogin").and_then(|v| v.as_i64());
        let creation = obj.get("Creation").and_then(|v| v.as_i64());
        let data_json = serde_json::to_string(&data)?;

        sqlx::query(
            r#"
            UPDATE accounts SET
                email = ?2,
                password = ?3,
                member_number = ?4,
                name = ?5,
                last_login = ?6,
                creation = ?7,
                data_json = ?8
            WHERE account_name = ?1 COLLATE NOCASE
            "#,
        )
        .bind(&account_name)
        .bind(email)
        .bind(password)
        .bind(member_number)
        .bind(name)
        .bind(last_login)
        .bind(creation)
        .bind(data_json)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

struct KeyColumns {
    account_name: String,
    email: Option<String>,
    password: String,
    member_number: Option<i64>,
    name: Option<String>,
    last_login: Option<i64>,
    creation: Option<i64>,
}

fn merge_row_with_set(row: &sqlx::sqlite::SqliteRow, set: &Map<String, Value>) -> Result<(String, KeyColumns)> {
    let mut data = row_to_json(row)?;
    let obj = data
        .as_object_mut()
        .context("account json must be object")?;

    for (k, v) in set {
        if k == "_id" || k == "MapData" {
            continue;
        }
        obj.insert(k.clone(), v.clone());
    }

    let cols = KeyColumns {
        account_name: obj
            .get("AccountName")
            .and_then(|v| v.as_str())
            .context("AccountName missing after merge")?
            .to_string(),
        email: obj
            .get("Email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        password: obj
            .get("Password")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        member_number: obj.get("MemberNumber").and_then(|v| v.as_i64()),
        name: obj
            .get("Name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        last_login: obj.get("LastLogin").and_then(|v| v.as_i64()),
        creation: obj.get("Creation").and_then(|v| v.as_i64()),
    };

    let data_json = serde_json::to_string(&data)?;
    Ok((data_json, cols))
}

fn row_to_json(row: &sqlx::sqlite::SqliteRow) -> Result<Value> {
    let data_json: String = row.try_get("data_json")?;
    let mut data: Value = serde_json::from_str(&data_json).context("parse data_json")?;
    let obj = match data.as_object_mut() {
        Some(o) => o,
        None => {
            data = Value::Object(Map::new());
            data.as_object_mut().unwrap()
        }
    };

    let account_name: String = row.try_get("account_name")?;
    let email: Option<String> = row.try_get("email").ok().flatten();
    let password: String = row.try_get("password")?;
    let member_number: Option<i64> = row.try_get("member_number").ok().flatten();
    let name: Option<String> = row.try_get("name").ok().flatten();
    let last_login: Option<i64> = row.try_get("last_login").ok().flatten();
    let creation: Option<i64> = row.try_get("creation").ok().flatten();

    obj.insert("AccountName".into(), Value::String(account_name));
    if let Some(e) = email {
        obj.insert("Email".into(), Value::String(e));
    } else {
        obj.entry("Email".to_string())
            .or_insert(Value::String(String::new()));
    }
    obj.insert("Password".into(), Value::String(password));
    if let Some(n) = member_number {
        obj.insert("MemberNumber".into(), Value::Number(n.into()));
    }
    if let Some(n) = name {
        obj.insert("Name".into(), Value::String(n));
    }
    if let Some(t) = last_login {
        obj.insert("LastLogin".into(), Value::Number(t.into()));
    }
    if let Some(t) = creation {
        obj.insert("Creation".into(), Value::Number(t.into()));
    }

    Ok(data)
}

/// Accepts `sqlite:path`, `sqlite://path`, `sqlite:///abs`, or a bare `.db` path.
fn normalize_sqlite_url(url: &str) -> Result<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        bail!("empty SQLite DATABASE_URL");
    }

    if trimmed == "sqlite::memory:" || trimmed == ":memory:" || trimmed == "sqlite://:memory:" {
        return Ok("sqlite::memory:".into());
    }

    if let Some(rest) = trimmed.strip_prefix("sqlite://") {
        let path = rest.trim_start_matches('/');
        // sqlite:///C:/foo on Windows may become C:/foo after one strip; keep absolute if possible
        if rest.starts_with('/') && !cfg!(windows) {
            return Ok(format!("sqlite:/{rest}"));
        }
        if rest.len() >= 2 && rest.as_bytes()[0] == b'/' && rest.as_bytes()[2] == b':' {
            // sqlite:///C:/path
            return Ok(format!("sqlite:{}", &rest[1..]));
        }
        return Ok(format!("sqlite:{path}"));
    }

    if let Some(rest) = trimmed.strip_prefix("sqlite:") {
        return Ok(format!("sqlite:{rest}"));
    }

    // Bare path
    Ok(format!("sqlite:{trimmed}"))
}

fn ensure_sqlite_parent_dir(connect_url: &str) -> Result<()> {
    if connect_url.contains(":memory:") {
        return Ok(());
    }
    let path_str = connect_url.strip_prefix("sqlite:").unwrap_or(connect_url);
    let path = Path::new(path_str);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create SQLite directory {}", parent.display()))?;
        }
    }
    Ok(())
}
