use once_cell::sync::Lazy;
use regex::Regex;

static ACCOUNT_EMAIL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-zA-Z0-9@.!#$%&'*+/=?^_`{|}~-]{5,100}$").unwrap());
static ACCOUNT_NAME: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-zA-Z0-9]{1,20}$").unwrap());
static ACCOUNT_PASSWORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-zA-Z0-9]{1,20}$").unwrap());
static ACCOUNT_RESET_NUMBER: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[0-9]{1,20}$").unwrap());
static CHARACTER_NAME: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-zA-Z ]{1,20}$").unwrap());
static CHARACTER_NICKNAME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[\p{L}\p{Nd}\p{Z}'-]+$").unwrap());
static CHAT_ROOM_NAME: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[\x20-\x7E]{1,20}$").unwrap());

pub const CHAT_MESSAGE_MAX_LENGTH: usize = 2000;
pub const CHAT_ROOM_DESCRIPTION_MAX_LENGTH: usize = 300;
pub const OWNERSHIP_DELAY_MS: i64 = 604_800_000;
pub const LOVERSHIP_DELAY_MS: i64 = 604_800_000;
pub const DIFFICULTY_DELAY_MS: i64 = 604_800_000;
pub const ROOM_LIMIT_DEFAULT: i32 = 10;
pub const ROOM_LIMIT_MINIMUM: i32 = 2;
pub const ROOM_LIMIT_MAXIMUM: i32 = 20;

pub fn is_account_name(s: &str) -> bool {
    ACCOUNT_NAME.is_match(s)
}

pub fn is_account_password(s: &str) -> bool {
    ACCOUNT_PASSWORD.is_match(s)
}

pub fn is_character_name(s: &str) -> bool {
    CHARACTER_NAME.is_match(s)
}

pub fn is_character_nickname(s: &str) -> bool {
    CHARACTER_NICKNAME.is_match(s)
}

pub fn is_chat_room_name(s: &str) -> bool {
    CHAT_ROOM_NAME.is_match(s)
}

pub fn is_reset_number(s: &str) -> bool {
    ACCOUNT_RESET_NUMBER.is_match(s)
}

/// Matches Node `CommonEmailIsValid`.
pub fn is_email_valid(email: &str) -> bool {
    if !ACCOUNT_EMAIL.is_match(email) {
        return false;
    }
    let parts: Vec<&str> = email.split('@').collect();
    if parts.len() != 2 {
        return false;
    }
    parts[1].contains('.')
}

pub fn is_simple_object(value: &serde_json::Value) -> bool {
    value.is_object()
}
