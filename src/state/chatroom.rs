use serde_json::Value;

use crate::protocol::MemberNumber;
use crate::util::{common_time, ROOM_LIMIT_DEFAULT};

#[derive(Debug, Clone)]
pub struct ChatRoom {
    pub id: String,
    pub name: String,
    pub description: String,
    pub background: String,
    pub environment: String,
    pub creator: String,
    pub creator_member_number: MemberNumber,
    pub creation: i64,
    pub admin: Vec<MemberNumber>,
    pub whitelist: Vec<MemberNumber>,
    pub ban: Vec<MemberNumber>,
    pub limit: i32,
    pub game: String,
    pub visibility: Vec<String>,
    pub access: Vec<String>,
    /// Legacy dual field (deprecated, kept for wire compat).
    pub private: bool,
    pub locked: bool,
    pub block_category: Vec<String>,
    pub language: String,
    pub space: String,
    pub map_data: Option<Value>,
    pub custom: Option<Value>,
    /// Member numbers currently in the room, order matters.
    pub members: Vec<MemberNumber>,
}

impl ChatRoom {
    pub fn new(
        id: String,
        name: String,
        environment: String,
        creator: String,
        creator_member_number: MemberNumber,
    ) -> Self {
        Self {
            id,
            name,
            description: String::new(),
            background: "Introduction".into(),
            environment,
            creator,
            creator_member_number,
            creation: common_time(),
            admin: vec![creator_member_number],
            whitelist: vec![],
            ban: vec![],
            limit: ROOM_LIMIT_DEFAULT,
            game: String::new(),
            visibility: vec!["All".into()],
            access: vec!["All".into()],
            private: false,
            locked: false,
            block_category: vec![],
            language: "EN".into(),
            space: String::new(),
            map_data: None,
            custom: None,
            members: vec![],
        }
    }

    pub fn is_full(&self) -> bool {
        self.members.len() as i32 >= self.limit
    }

    pub fn socket_room_name(&self) -> String {
        format!("chatroom-{}", self.id)
    }

    pub fn to_properties_json(&self) -> Value {
        serde_json::json!({
            "Name": self.name,
            "Description": self.description,
            "Admin": self.admin,
            "Whitelist": self.whitelist,
            "Ban": self.ban,
            "Background": self.background,
            "Limit": self.limit,
            "Game": self.game,
            "Visibility": self.visibility,
            "Access": self.access,
            "Private": self.private,
            "Locked": self.locked,
            "BlockCategory": self.block_category,
            "Language": self.language,
            "Space": self.space,
            "MapData": self.map_data,
            "Custom": self.custom,
        })
    }
}
