use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub database_name: String,
    pub account_collection: String,
    pub port: u16,
    pub ip_connection_limit: usize,
    pub ip_connection_rate_limit: usize,
    pub client_message_rate_limit: usize,
    pub max_ip_account_per_day: usize,
    pub max_ip_account_per_hour: usize,
    pub max_heap_usage: u64,
    pub cors_origins: Vec<String>,
    pub production_origins: Vec<String>,
    pub email_password: String,
    pub email_admin: String,
    pub email_host: String,
    pub email_port: u16,
    pub email_user: String,
    pub email_from: String,
}

impl Config {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();

        let mut cors_origins = Vec::new();
        for i in 0..=5 {
            if let Ok(v) = env::var(format!("CORS_ORIGIN{i}")) {
                if !v.is_empty() {
                    cors_origins.push(v);
                }
            }
        }

        let mut production_origins = Vec::new();
        for i in 0..=12 {
            if let Ok(v) = env::var(format!("PRODUCTION{i}")) {
                if !v.is_empty() {
                    production_origins.push(v.to_lowercase());
                }
            }
        }

        Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "mongodb://localhost:27017/BondageClubDatabase".into()),
            database_name: env::var("DATABASE_NAME")
                .unwrap_or_else(|_| "BondageClubDatabase".into()),
            account_collection: env::var("ACCOUNT_COLLECTION").unwrap_or_else(|_| "Accounts".into()),
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(4288),
            ip_connection_limit: parse_usize("IP_CONNECTION_LIMIT", 64),
            ip_connection_rate_limit: parse_usize("IP_CONNECTION_RATE_LIMIT", 2),
            client_message_rate_limit: parse_usize("CLIENT_MESSAGE_RATE_LIMIT", 20),
            max_ip_account_per_day: parse_usize("MAX_IP_ACCOUNT_PER_DAY", 12),
            max_ip_account_per_hour: parse_usize("MAX_IP_ACCOUNT_PER_HOUR", 4),
            max_heap_usage: env::var("MAX_HEAP_USAGE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(16_000_000_000),
            cors_origins,
            production_origins,
            email_password: env::var("EMAIL_PASSWORD").unwrap_or_default(),
            email_admin: env::var("EMAIL_ADMIN").unwrap_or_default(),
            email_host: env::var("EMAIL_HOST")
                .unwrap_or_else(|_| "mail.bondageprojects.com".into()),
            email_port: env::var("EMAIL_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(465),
            email_user: env::var("EMAIL_USER")
                .unwrap_or_else(|_| "donotreply@bondageprojects.com".into()),
            email_from: env::var("EMAIL_FROM")
                .unwrap_or_else(|_| "donotreply@bondageprojects.com".into()),
        }
    }
}

fn parse_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default)
}
