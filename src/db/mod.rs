use anyhow::{Context, Result};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::ClientOptions;
use mongodb::{Client, Collection, Database};
use serde_json::Value;
use tracing::info;

use crate::config::Config;
use crate::protocol::MemberNumber;

#[derive(Clone)]
pub struct Db {
    pub database: Database,
    pub accounts: Collection<Document>,
}

impl Db {
    pub async fn connect(config: &Config) -> Result<Self> {
        let mut opts = ClientOptions::parse(&config.database_url)
            .await
            .context("parse DATABASE_URL")?;
        opts.app_name = Some("bondage-club-server-rs".into());

        let client = Client::with_options(opts).context("create mongo client")?;
        let database = client.database(&config.database_name);
        let accounts = database.collection::<Document>(&config.account_collection);

        // Ping
        database
            .run_command(doc! { "ping": 1 })
            .await
            .context("mongo ping")?;

        info!(
            db = %config.database_name,
            collection = %config.account_collection,
            "Database connected"
        );

        Ok(Self { database, accounts })
    }

    pub async fn next_member_number(&self) -> Result<MemberNumber> {
        let mut cursor = self
            .accounts
            .find(doc! { "MemberNumber": { "$exists": true, "$ne": Bson::Null } })
            .sort(doc! { "MemberNumber": -1 })
            .limit(1)
            .await?;

        if let Some(doc) = cursor.try_next().await? {
            let n = doc
                .get_i64("MemberNumber")
                .or_else(|_| doc.get_i32("MemberNumber").map(|v| v as i64))
                .unwrap_or(0);
            Ok(n + 1)
        } else {
            Ok(1)
        }
    }

    pub async fn find_by_account_name(&self, account_name: &str) -> Result<Option<Value>> {
        let result = self
            .accounts
            .find_one(doc! { "AccountName": account_name })
            .await?;
        Ok(result.map(document_to_json))
    }

    pub async fn find_email_by_account_name(&self, account_name: &str) -> Result<Option<String>> {
        let result = self
            .accounts
            .find_one(doc! { "AccountName": account_name })
            .projection(doc! { "Email": 1, "_id": 0 })
            .await?;
        Ok(result.and_then(|d| d.get_str("Email").ok().map(|s| s.to_string())))
    }

    pub async fn find_account_names_by_email(&self, email: &str) -> Result<Vec<String>> {
        // Case-insensitive match via regex (legacy behavior approximates trim+lower compare on update;
        // password reset uses exact email field match in Node — we try case-insensitive).
        let filter = doc! {
            "Email": {
                "$regex": format!("^{}$", regex_escape(email.trim())),
                "$options": "i"
            }
        };
        let mut cursor = self.accounts.find(filter).await?;
        let mut names = Vec::new();
        while let Some(doc) = cursor.try_next().await? {
            if let Ok(name) = doc.get_str("AccountName") {
                names.push(name.to_string());
            }
        }
        Ok(names)
    }

    pub async fn insert_account(&self, account: Document) -> Result<()> {
        self.accounts.insert_one(account).await?;
        Ok(())
    }

    pub async fn update_fields(&self, account_name: &str, set: Document) -> Result<()> {
        if set.is_empty() {
            return Ok(());
        }
        self.accounts
            .update_one(doc! { "AccountName": account_name }, doc! { "$set": set })
            .await?;
        Ok(())
    }

    pub async fn set_password(&self, account_name: &str, password_hash: &str) -> Result<()> {
        self.update_fields(account_name, doc! { "Password": password_hash })
            .await
    }

    pub async fn set_member_number(&self, account_name: &str, n: MemberNumber) -> Result<()> {
        self.update_fields(account_name, doc! { "MemberNumber": n })
            .await
    }

    pub async fn set_last_login(&self, account_name: &str, t: i64) -> Result<()> {
        self.update_fields(account_name, doc! { "LastLogin": t })
            .await
    }

    pub async fn set_email(&self, account_name: &str, email: &str) -> Result<()> {
        self.update_fields(account_name, doc! { "Email": email })
            .await
    }
}

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '.' | '+' | '*' | '?' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Convert BSON Document to serde_json::Value for flexible account handling.
pub fn document_to_json(doc: Document) -> Value {
    let bson = Bson::Document(doc);
    bson_to_json(bson)
}

pub fn json_to_document(value: &Value) -> Result<Document> {
    let bson = json_to_bson(value)?;
    match bson {
        Bson::Document(d) => Ok(d),
        _ => anyhow::bail!("expected object document"),
    }
}

fn bson_to_json(bson: Bson) -> Value {
    match bson {
        Bson::Double(f) => {
            if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                Value::Number((f as i64).into())
            } else {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        Bson::String(s) => Value::String(s),
        Bson::Array(arr) => Value::Array(arr.into_iter().map(bson_to_json).collect()),
        Bson::Document(doc) => {
            let mut map = serde_json::Map::new();
            for (k, v) in doc {
                // Skip raw binary ObjectId presentation issues — serialize ObjectId as hex string
                map.insert(k, bson_to_json(v));
            }
            Value::Object(map)
        }
        Bson::Boolean(b) => Value::Bool(b),
        Bson::Null => Value::Null,
        Bson::Int32(i) => Value::Number(i.into()),
        Bson::Int64(i) => Value::Number(i.into()),
        Bson::ObjectId(oid) => Value::String(oid.to_hex()),
        Bson::DateTime(dt) => Value::Number(dt.timestamp_millis().into()),
        Bson::Decimal128(d) => Value::String(d.to_string()),
        other => Value::String(other.to_string()),
    }
}

fn json_to_bson(value: &Value) -> Result<Bson> {
    Ok(match value {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Bson::Int64(i)
            } else if let Some(u) = n.as_u64() {
                Bson::Int64(u as i64)
            } else if let Some(f) = n.as_f64() {
                Bson::Double(f)
            } else {
                Bson::Null
            }
        }
        Value::String(s) => Bson::String(s.clone()),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for v in arr {
                out.push(json_to_bson(v)?);
            }
            Bson::Array(out)
        }
        Value::Object(map) => {
            let mut doc = Document::new();
            for (k, v) in map {
                if k == "_id" {
                    continue;
                }
                doc.insert(k.clone(), json_to_bson(v)?);
            }
            Bson::Document(doc)
        }
    })
}

/// Build a $set document from a JSON object (for AccountUpdate).
pub fn json_object_to_set_doc(value: &Value) -> Result<Document> {
    match value {
        Value::Object(map) => {
            let mut doc = Document::new();
            for (k, v) in map {
                if k == "_id" || k == "MapData" {
                    continue;
                }
                doc.insert(k.clone(), json_to_bson(v)?);
            }
            Ok(doc)
        }
        _ => anyhow::bail!("expected object"),
    }
}
