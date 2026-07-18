mod mongo;
mod sqlite;

use anyhow::{bail, Result};
use serde_json::{Map, Value};
use tracing::info;

use crate::config::{Config, DbBackend};
use crate::protocol::MemberNumber;

use self::mongo::MongoDb;
use self::sqlite::SqliteDb;

/// Account store (MongoDB by default, optional SQLite via `.env`).
#[derive(Clone)]
pub struct Db {
    inner: DbInner,
}

#[derive(Clone)]
enum DbInner {
    Mongo(MongoDb),
    Sqlite(SqliteDb),
}

impl Db {
    pub async fn connect(config: &Config) -> Result<Self> {
        let inner = match config.db_backend {
            DbBackend::Mongo => {
                info!(
                    backend = "mongodb",
                    db = %config.database_name,
                    collection = %config.account_collection,
                    "Connecting database"
                );
                DbInner::Mongo(MongoDb::connect(config).await?)
            }
            DbBackend::Sqlite => {
                info!(backend = "sqlite", url = %config.database_url, "Connecting database");
                DbInner::Sqlite(SqliteDb::connect(config).await?)
            }
        };
        Ok(Self { inner })
    }

    pub async fn close(&self) -> Result<()> {
        match &self.inner {
            DbInner::Mongo(_) => Ok(()),
            DbInner::Sqlite(db) => db.close().await,
        }
    }

    pub async fn next_member_number(&self) -> Result<MemberNumber> {
        match &self.inner {
            DbInner::Mongo(db) => db.next_member_number().await,
            DbInner::Sqlite(db) => db.next_member_number().await,
        }
    }

    pub async fn find_by_account_name(&self, account_name: &str) -> Result<Option<Value>> {
        match &self.inner {
            DbInner::Mongo(db) => db.find_by_account_name(account_name).await,
            DbInner::Sqlite(db) => db.find_by_account_name(account_name).await,
        }
    }

    pub async fn find_by_member_number(
        &self,
        member_number: MemberNumber,
    ) -> Result<Option<Value>> {
        match &self.inner {
            DbInner::Mongo(db) => db.find_by_member_number(member_number).await,
            DbInner::Sqlite(db) => db.find_by_member_number(member_number).await,
        }
    }

    /// Returns only non-sensitive fields required to render public prison listings.
    pub async fn list_public_prison_data(&self) -> Result<Vec<Value>> {
        match &self.inner {
            DbInner::Mongo(db) => db.list_public_prison_data().await,
            DbInner::Sqlite(db) => db.list_public_prison_data().await,
        }
    }

    pub async fn find_email_by_account_name(&self, account_name: &str) -> Result<Option<String>> {
        match &self.inner {
            DbInner::Mongo(db) => db.find_email_by_account_name(account_name).await,
            DbInner::Sqlite(db) => db.find_email_by_account_name(account_name).await,
        }
    }

    pub async fn find_account_names_by_email(&self, email: &str) -> Result<Vec<String>> {
        match &self.inner {
            DbInner::Mongo(db) => db.find_account_names_by_email(email).await,
            DbInner::Sqlite(db) => db.find_account_names_by_email(email).await,
        }
    }

    pub async fn insert_account(&self, account: Value) -> Result<()> {
        match &self.inner {
            DbInner::Mongo(db) => db.insert_account(account).await,
            DbInner::Sqlite(db) => db.insert_account(account).await,
        }
    }

    pub async fn update_fields(&self, account_name: &str, set: Map<String, Value>) -> Result<()> {
        if set.is_empty() {
            return Ok(());
        }
        match &self.inner {
            DbInner::Mongo(db) => db.update_fields(account_name, set).await,
            DbInner::Sqlite(db) => db.update_fields(account_name, set).await,
        }
    }

    pub async fn update_fields_by_member_number(
        &self,
        member_number: MemberNumber,
        set: Map<String, Value>,
    ) -> Result<()> {
        if set.is_empty() {
            return Ok(());
        }
        match &self.inner {
            DbInner::Mongo(db) => db.update_fields_by_member_number(member_number, set).await,
            DbInner::Sqlite(db) => db.update_fields_by_member_number(member_number, set).await,
        }
    }

    pub async fn clear_ownership(&self, account_name: &str) -> Result<()> {
        let mut set = Map::new();
        set.insert("Owner".into(), Value::String(String::new()));
        set.insert("Ownership".into(), Value::Null);
        self.update_fields(account_name, set).await
    }

    pub async fn pull_lovership_member(
        &self,
        target_member: MemberNumber,
        source_member: MemberNumber,
    ) -> Result<()> {
        match &self.inner {
            DbInner::Mongo(db) => db.pull_lovership_member(target_member, source_member).await,
            DbInner::Sqlite(db) => db.pull_lovership_member(target_member, source_member).await,
        }
    }

    pub async fn set_password(&self, account_name: &str, password_hash: &str) -> Result<()> {
        let mut set = Map::new();
        set.insert("Password".into(), Value::String(password_hash.to_string()));
        self.update_fields(account_name, set).await
    }

    pub async fn set_member_number(&self, account_name: &str, n: MemberNumber) -> Result<()> {
        let mut set = Map::new();
        set.insert("MemberNumber".into(), Value::Number(n.into()));
        self.update_fields(account_name, set).await
    }

    pub async fn set_last_login(&self, account_name: &str, t: i64) -> Result<()> {
        let mut set = Map::new();
        set.insert("LastLogin".into(), Value::Number(t.into()));
        self.update_fields(account_name, set).await
    }

    pub async fn set_email(&self, account_name: &str, email: &str) -> Result<()> {
        let mut set = Map::new();
        set.insert("Email".into(), Value::String(email.to_string()));
        self.update_fields(account_name, set).await
    }
}

/// Build a partial update map from a JSON object (for AccountUpdate).
/// Skips `_id` and `MapData` (never persisted).
pub fn json_object_to_set_map(value: &Value) -> Result<Map<String, Value>> {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if k == "_id" || k == "MapData" {
                    continue;
                }
                out.insert(k.clone(), v.clone());
            }
            Ok(out)
        }
        _ => bail!("expected object"),
    }
}
