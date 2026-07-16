//! Wire protocol types shared with the Bondage Club client (`Messages.d.ts`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type MemberNumber = i64;

// ---------------------------------------------------------------------------
// Client → Server request payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct AccountLoginRequest {
    #[serde(rename = "AccountName")]
    pub account_name: String,
    #[serde(rename = "Password")]
    pub password: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountCreateRequest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "AccountName")]
    pub account_name: String,
    #[serde(rename = "Password")]
    pub password: String,
    #[serde(rename = "Email")]
    pub email: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PasswordResetProcessRequest {
    #[serde(rename = "AccountName")]
    pub account_name: String,
    #[serde(rename = "ResetNumber")]
    pub reset_number: String,
    #[serde(rename = "NewPassword")]
    pub new_password: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountUpdateEmailRequest {
    #[serde(rename = "EmailOld")]
    pub email_old: String,
    #[serde(rename = "EmailNew")]
    pub email_new: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountQueryRequest {
    #[serde(rename = "Query")]
    pub query: String,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountBeepRequest {
    #[serde(rename = "MemberNumber")]
    pub member_number: Option<MemberNumber>,
    #[serde(rename = "BeepType")]
    pub beep_type: Option<String>,
    #[serde(rename = "Message")]
    pub message: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRoomSearchRequest {
    #[serde(rename = "Query")]
    pub query: Option<String>,
    #[serde(rename = "Space")]
    pub space: Option<String>,
    #[serde(rename = "Game")]
    pub game: Option<String>,
    #[serde(rename = "FullRooms")]
    pub full_rooms: Option<bool>,
    #[serde(rename = "Language")]
    pub language: Option<String>,
    #[serde(rename = "Map")]
    pub map: Option<String>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRoomCreateRequest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Description")]
    pub description: Option<String>,
    #[serde(rename = "Background")]
    pub background: Option<String>,
    #[serde(rename = "Private")]
    pub private: Option<bool>,
    #[serde(rename = "Locked")]
    pub locked: Option<bool>,
    #[serde(rename = "Visibility")]
    pub visibility: Option<Vec<String>>,
    #[serde(rename = "Access")]
    pub access: Option<Vec<String>>,
    #[serde(rename = "Space")]
    pub space: Option<String>,
    #[serde(rename = "Game")]
    pub game: Option<String>,
    #[serde(rename = "Admin")]
    pub admin: Option<Vec<MemberNumber>>,
    #[serde(rename = "Ban")]
    pub ban: Option<Vec<MemberNumber>>,
    #[serde(rename = "Whitelist")]
    pub whitelist: Option<Vec<MemberNumber>>,
    #[serde(rename = "Limit")]
    pub limit: Option<Value>,
    #[serde(rename = "BlockCategory")]
    pub block_category: Option<Vec<String>>,
    #[serde(rename = "Language")]
    pub language: Option<String>,
    #[serde(rename = "MapData")]
    pub map_data: Option<Value>,
    #[serde(rename = "Custom")]
    pub custom: Option<Value>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRoomJoinRequest {
    #[serde(rename = "Name")]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRoomMessage {
    #[serde(rename = "Content")]
    pub content: Option<String>,
    #[serde(rename = "Type")]
    pub msg_type: Option<String>,
    #[serde(rename = "Target")]
    pub target: Option<MemberNumber>,
    #[serde(rename = "Dictionary")]
    pub dictionary: Option<Value>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRoomAdminRequest {
    #[serde(rename = "Action")]
    pub action: String,
    #[serde(rename = "MemberNumber")]
    pub member_number: Option<MemberNumber>,
    #[serde(rename = "MemberNumberList")]
    pub member_number_list: Option<Vec<MemberNumber>>,
    #[serde(flatten)]
    pub extra: Value,
}

// ---------------------------------------------------------------------------
// Server → Client response helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ServerInfoMessage {
    #[serde(rename = "Time")]
    pub time: i64,
    #[serde(rename = "OnlinePlayers")]
    pub online_players: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountCreateSuccess {
    #[serde(rename = "ServerAnswer")]
    pub server_answer: &'static str,
    #[serde(rename = "OnlineID")]
    pub online_id: String,
    #[serde(rename = "MemberNumber")]
    pub member_number: MemberNumber,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountQueryResult {
    #[serde(rename = "Query")]
    pub query: String,
    #[serde(rename = "Result")]
    pub result: Value,
}

// ---------------------------------------------------------------------------
// Stable string response codes (wire-compatible with Node)
// ---------------------------------------------------------------------------

pub mod codes {
    pub const INVALID_NAME_PASSWORD: &str = "InvalidNamePassword";
    pub const ERROR_RATE_LIMITED: &str = "ErrorRateLimited";
    pub const ERROR_DUPLICATED_LOGIN: &str = "ErrorDuplicatedLogin";
    pub const ACCOUNT_ALREADY_EXISTS: &str = "Account already exists";
    pub const INVALID_ACCOUNT_INFO: &str = "Invalid account information";
    pub const NEW_ACCOUNTS_EXCEEDED: &str = "New accounts per day exceeded";
    pub const ACCOUNT_CREATED: &str = "AccountCreated";

    pub const RETRY_LATER: &str = "RetryLater";
    pub const EMAIL_SENT: &str = "EmailSent";
    pub const EMAIL_SENT_ERROR: &str = "EmailSentError";
    pub const NO_ACCOUNT_ON_EMAIL: &str = "NoAccountOnEmail";
    pub const PASSWORD_RESET_SUCCESS: &str = "PasswordResetSuccessful";
    pub const INVALID_PASSWORD_RESET: &str = "InvalidPasswordResetInfo";

    pub const CHAT_TYPES: &[&str] = &[
        "Chat", "Action", "Activity", "Emote", "Whisper", "Hidden", "Status",
    ];
}

// ---------------------------------------------------------------------------
// Event name constants
// ---------------------------------------------------------------------------

pub mod events {
    // Server → Client
    pub const SERVER_INFO: &str = "ServerInfo";
    pub const SERVER_MESSAGE: &str = "ServerMessage";
    pub const FORCE_DISCONNECT: &str = "ForceDisconnect";
    pub const CREATION_RESPONSE: &str = "CreationResponse";
    pub const PASSWORD_RESET_RESPONSE: &str = "PasswordResetResponse";
    pub const LOGIN_RESPONSE: &str = "LoginResponse";
    pub const LOGIN_QUEUE: &str = "LoginQueue";
    pub const ACCOUNT_QUERY_RESULT: &str = "AccountQueryResult";
    pub const ACCOUNT_LOVERSHIP: &str = "AccountLovership";
    pub const ACCOUNT_OWNERSHIP: &str = "AccountOwnership";
    pub const ACCOUNT_BEEP: &str = "AccountBeep";
    pub const CHAT_ROOM_SEARCH_RESULT: &str = "ChatRoomSearchResult";
    pub const CHAT_ROOM_CREATE_RESPONSE: &str = "ChatRoomCreateResponse";
    pub const CHAT_ROOM_SEARCH_RESPONSE: &str = "ChatRoomSearchResponse";
    pub const CHAT_ROOM_MESSAGE: &str = "ChatRoomMessage";
    pub const CHAT_ROOM_GAME_RESPONSE: &str = "ChatRoomGameResponse";
    pub const CHAT_ROOM_SYNC: &str = "ChatRoomSync";
    pub const CHAT_ROOM_SYNC_CHARACTER: &str = "ChatRoomSyncCharacter";
    pub const CHAT_ROOM_SYNC_MEMBER_JOIN: &str = "ChatRoomSyncMemberJoin";
    pub const CHAT_ROOM_SYNC_MEMBER_LEAVE: &str = "ChatRoomSyncMemberLeave";
    pub const CHAT_ROOM_SYNC_ROOM_PROPERTIES: &str = "ChatRoomSyncRoomProperties";
    pub const CHAT_ROOM_SYNC_REORDER_PLAYERS: &str = "ChatRoomSyncReorderPlayers";
    pub const CHAT_ROOM_SYNC_SINGLE: &str = "ChatRoomSyncSingle";
    pub const CHAT_ROOM_SYNC_EXPRESSION: &str = "ChatRoomSyncExpression";
    pub const CHAT_ROOM_SYNC_POSE: &str = "ChatRoomSyncPose";
    pub const CHAT_ROOM_SYNC_AROUSAL: &str = "ChatRoomSyncArousal";
    pub const CHAT_ROOM_SYNC_ITEM: &str = "ChatRoomSyncItem";
    pub const CHAT_ROOM_SYNC_MAP_DATA: &str = "ChatRoomSyncMapData";
    pub const CHAT_ROOM_UPDATE_RESPONSE: &str = "ChatRoomUpdateResponse";
    pub const CHAT_ROOM_ALLOW_ITEM: &str = "ChatRoomAllowItem";

    // Client → Server
    pub const ACCOUNT_LOGIN: &str = "AccountLogin";
    pub const ACCOUNT_CREATE: &str = "AccountCreate";
    pub const PASSWORD_RESET: &str = "PasswordReset";
    pub const PASSWORD_RESET_PROCESS: &str = "PasswordResetProcess";
    pub const ACCOUNT_UPDATE: &str = "AccountUpdate";
    pub const ACCOUNT_UPDATE_EMAIL: &str = "AccountUpdateEmail";
    pub const ACCOUNT_QUERY: &str = "AccountQuery";
    pub const ACCOUNT_BEEP_EVT: &str = "AccountBeep";
    pub const ACCOUNT_OWNERSHIP_EVT: &str = "AccountOwnership";
    pub const ACCOUNT_LOVERSHIP_EVT: &str = "AccountLovership";
    pub const ACCOUNT_DIFFICULTY: &str = "AccountDifficulty";
    pub const ACCOUNT_DISCONNECT: &str = "AccountDisconnect";
    pub const CHAT_ROOM_SEARCH: &str = "ChatRoomSearch";
    pub const CHAT_ROOM_CREATE: &str = "ChatRoomCreate";
    pub const CHAT_ROOM_JOIN: &str = "ChatRoomJoin";
    pub const CHAT_ROOM_LEAVE: &str = "ChatRoomLeave";
    pub const CHAT_ROOM_CHAT: &str = "ChatRoomChat";
    pub const CHAT_ROOM_CHARACTER_UPDATE: &str = "ChatRoomCharacterUpdate";
    pub const CHAT_ROOM_CHARACTER_EXPRESSION_UPDATE: &str = "ChatRoomCharacterExpressionUpdate";
    pub const CHAT_ROOM_CHARACTER_POSE_UPDATE: &str = "ChatRoomCharacterPoseUpdate";
    pub const CHAT_ROOM_CHARACTER_AROUSAL_UPDATE: &str = "ChatRoomCharacterArousalUpdate";
    pub const CHAT_ROOM_CHARACTER_ITEM_UPDATE: &str = "ChatRoomCharacterItemUpdate";
    pub const CHAT_ROOM_CHARACTER_MAP_DATA_UPDATE: &str = "ChatRoomCharacterMapDataUpdate";
    pub const CHAT_ROOM_ADMIN: &str = "ChatRoomAdmin";
    pub const CHAT_ROOM_ALLOW_ITEM_EVT: &str = "ChatRoomAllowItem";
    pub const CHAT_ROOM_GAME: &str = "ChatRoomGame";
}
