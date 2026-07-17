use serde_json::Value;
use std::collections::HashMap;

use crate::protocol::MemberNumber;
use crate::util::common_time;

/// Online session account (in-memory). Large/sensitive fields are purged after login.
#[derive(Debug, Clone)]
pub struct OnlineAccount {
    pub socket_id: String,
    pub account_name: String,
    pub member_number: MemberNumber,
    pub name: String,
    pub environment: String,
    pub creation: i64,
    pub item_permission: i32,
    pub white_list: Vec<MemberNumber>,
    pub black_list: Vec<MemberNumber>,
    pub friend_list: Vec<MemberNumber>,
    pub ownership: Option<Value>,
    /// Display owner name (Node `Owner` field).
    pub owner: String,
    pub lovership: Vec<Value>,
    pub difficulty: Option<Value>,
    pub appearance: Option<Value>,
    pub inventory_data: Option<Value>,
    pub arousal_settings: Option<Value>,
    pub online_shared_settings: Option<Value>,
    pub game: Option<Value>,
    pub map_data: Option<Value>,
    pub label_color: Option<String>,
    pub reputation: Option<Value>,
    pub description: Option<String>,
    pub block_items: Option<Value>,
    pub limited_items: Option<Value>,
    pub favorite_items: Option<Value>,
    pub title: Option<String>,
    pub nickname: Option<String>,
    pub crafting: Option<Value>,
    pub pose: Option<Value>,
    pub active_pose: Option<Value>,
    pub chat_room_id: Option<String>,
    pub delayed_appearance: Option<Value>,
    pub delayed_skill: Option<Value>,
    pub delayed_game: Option<Value>,
    /// Full document fields kept for LoginResponse / room sync (JSON blob).
    pub extra: HashMap<String, Value>,
    pub logged_in_at: i64,
}

impl OnlineAccount {
    pub fn from_db_doc(socket_id: String, environment: String, mut doc: Value) -> Self {
        let obj = doc.as_object_mut().expect("account doc must be object");

        let account_name = take_string(obj, "AccountName").unwrap_or_default();
        let member_number = take_i64(obj, "MemberNumber").unwrap_or(0);
        let name = take_string(obj, "Name").unwrap_or_default();
        let creation = take_i64(obj, "Creation").unwrap_or_else(common_time);
        let item_permission = take_i64(obj, "ItemPermission").unwrap_or(2) as i32;

        let white_list = take_member_list(obj, "WhiteList");
        let black_list = take_member_list(obj, "BlackList");
        let friend_list = take_member_list(obj, "FriendList");

        let ownership = obj.remove("Ownership");
        let owner = take_string(obj, "Owner").unwrap_or_default();
        let lovership = match obj.remove("Lovership") {
            Some(Value::Array(arr)) => arr,
            Some(v) if !v.is_null() => vec![v],
            _ => vec![],
        };
        let difficulty = obj.remove("Difficulty");
        let appearance = obj.remove("Appearance");
        let inventory_data = obj.remove("InventoryData");
        let arousal_settings = obj.remove("ArousalSettings");
        let online_shared_settings = obj.remove("OnlineSharedSettings");
        let game = obj.remove("Game");
        let map_data = obj.remove("MapData");
        let label_color = take_string(obj, "LabelColor");
        let reputation = obj.remove("Reputation");
        let description = take_string(obj, "Description");
        let block_items = obj.remove("BlockItems");
        let limited_items = obj.remove("LimitedItems");
        let favorite_items = obj.remove("FavoriteItems");
        let title = take_string(obj, "Title");
        let nickname = take_string(obj, "Nickname");
        let crafting = obj.remove("Crafting");
        let pose = obj.remove("Pose");
        let active_pose = obj.remove("ActivePose");

        // Never keep secrets / mongo id in the online session blob.
        // Node deletes Password/Email before LoginResponse; bulky fields are only
        // purged from memory AFTER LoginResponse (AccountPurgeInfo) so clients still
        // receive Log (ToS), Skill, Wardrobe, ChatSettings, etc.
        for key in ["Password", "Email", "_id"] {
            obj.remove(key);
        }

        let keys: Vec<String> = obj.keys().cloned().collect();
        let mut extra = HashMap::new();
        for k in keys {
            if let Some(v) = obj.remove(&k) {
                extra.insert(k, v);
            }
        }

        Self {
            socket_id,
            account_name,
            member_number,
            name,
            environment,
            creation,
            item_permission,
            white_list,
            black_list,
            friend_list,
            ownership,
            owner,
            lovership,
            difficulty,
            appearance,
            inventory_data,
            arousal_settings,
            online_shared_settings,
            game,
            map_data,
            label_color,
            reputation,
            description,
            block_items,
            limited_items,
            favorite_items,
            title,
            nickname,
            crafting,
            pose,
            active_pose,
            chat_room_id: None,
            delayed_appearance: None,
            delayed_skill: None,
            delayed_game: None,
            extra,
            logged_in_at: common_time(),
        }
    }

    /// Build a LoginResponse-compatible JSON object (password/email already purged).
    pub fn to_login_response(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("ID".into(), Value::String(self.socket_id.clone()));
        map.insert("AccountName".into(), Value::String(self.account_name.clone()));
        map.insert("MemberNumber".into(), json_num(self.member_number));
        map.insert("Name".into(), Value::String(self.name.clone()));
        map.insert("Creation".into(), json_num(self.creation));
        map.insert("ItemPermission".into(), json_num(self.item_permission as i64));
        // Node always includes AssetFamily on shared/login character data
        if !self.extra.contains_key("AssetFamily") {
            map.insert(
                "AssetFamily".into(),
                Value::String("Female3DCG".into()),
            );
        }
        map.insert("WhiteList".into(), member_list_json(&self.white_list));
        map.insert("BlackList".into(), member_list_json(&self.black_list));
        map.insert("FriendList".into(), member_list_json(&self.friend_list));
        map.insert("Lovership".into(), Value::Array(self.lovership.clone()));
        if let Some(ref o) = self.ownership {
            map.insert("Ownership".into(), o.clone());
        }
        if !self.owner.is_empty() {
            map.insert("Owner".into(), Value::String(self.owner.clone()));
        }
        if let Some(ref d) = self.difficulty {
            map.insert("Difficulty".into(), d.clone());
        }
        if let Some(ref a) = self.appearance {
            map.insert("Appearance".into(), a.clone());
        }
        if let Some(ref v) = self.inventory_data {
            map.insert("InventoryData".into(), v.clone());
        }
        if let Some(ref v) = self.arousal_settings {
            map.insert("ArousalSettings".into(), v.clone());
        }
        if let Some(ref v) = self.online_shared_settings {
            map.insert("OnlineSharedSettings".into(), v.clone());
        }
        if let Some(ref v) = self.game {
            map.insert("Game".into(), v.clone());
        }
        if let Some(ref v) = self.map_data {
            map.insert("MapData".into(), v.clone());
        }
        if let Some(ref v) = self.label_color {
            map.insert("LabelColor".into(), Value::String(v.clone()));
        }
        if let Some(ref v) = self.reputation {
            map.insert("Reputation".into(), v.clone());
        }
        if let Some(ref v) = self.description {
            map.insert("Description".into(), Value::String(v.clone()));
        }
        if let Some(ref v) = self.block_items {
            map.insert("BlockItems".into(), v.clone());
        }
        if let Some(ref v) = self.limited_items {
            map.insert("LimitedItems".into(), v.clone());
        }
        if let Some(ref v) = self.favorite_items {
            map.insert("FavoriteItems".into(), v.clone());
        }
        if let Some(ref v) = self.title {
            map.insert("Title".into(), Value::String(v.clone()));
        }
        if let Some(ref v) = self.nickname {
            map.insert("Nickname".into(), Value::String(v.clone()));
        }
        if let Some(ref v) = self.crafting {
            map.insert("Crafting".into(), v.clone());
        }
        if let Some(ref v) = self.pose {
            map.insert("Pose".into(), v.clone());
        }
        if let Some(ref v) = self.active_pose {
            map.insert("ActivePose".into(), v.clone());
        }
        for (k, v) in &self.extra {
            map.entry(k.clone()).or_insert_with(|| v.clone());
        }
        Value::Object(map)
    }

    /// Match Node `AccountPurgeInfo`: drop bulky/sensitive fields from RAM after login.
    /// LoginResponse must be built first so Log/Skill/settings still reach the client.
    pub fn purge_after_login(&mut self) {
        for key in [
            "Log",
            "Skill",
            "Wardrobe",
            "WardrobeCharacterNames",
            "ChatSettings",
            "VisualSettings",
            "AudioSettings",
            "GameplaySettings",
            "GhostList",
            "HiddenItems",
            "LastLogin",
            "Password",
            "Email",
        ] {
            self.extra.remove(key);
        }
    }

    /// Character data shared with room peers (omit money / friend list / account name).
    pub fn to_synced_character(&self) -> Value {
        self.to_synced_character_for_room(&[])
    }

    /// Room-scoped sync: WhiteList/BlackList only include members present in room.
    /// BlackList is only sent when ItemPermission is 1 or 2 (Node `AccountShouldSendBlackList`).
    pub fn to_synced_character_for_room(&self, room_members: &[MemberNumber]) -> Value {
        let mut v = self.to_login_response();
        if let Some(obj) = v.as_object_mut() {
            obj.entry("AssetFamily".to_string())
                .or_insert_with(|| Value::String("Female3DCG".into()));
            obj.remove("Money");
            obj.remove("FriendList");
            obj.remove("AccountName");
            obj.remove("Password");
            obj.remove("Email");
            // Never broadcast private client-only blobs to room peers.
            for key in [
                "Log",
                "Skill",
                "Wardrobe",
                "WardrobeCharacterNames",
                "ChatSettings",
                "VisualSettings",
                "AudioSettings",
                "GameplaySettings",
                "GhostList",
                "HiddenItems",
                "LastLogin",
                "OnlineSettings",
                "GraphicsSettings",
                "NotificationSettings",
                "ControllerSettings",
                "ImmersionSettings",
                "RestrictionSettings",
                "GenderSettings",
                "ExtensionSettings",
                "FriendNames",
                "SubmissivesList",
            ] {
                obj.remove(key);
            }

            if room_members.is_empty() {
                // Full lists only when no room context
            } else {
                let wl: Vec<Value> = self
                    .white_list
                    .iter()
                    .filter(|m| room_members.contains(m))
                    .map(|&n| Value::Number(n.into()))
                    .collect();
                obj.insert("WhiteList".into(), Value::Array(wl));

                if Self::should_send_blacklist(self.item_permission) {
                    let bl: Vec<Value> = self
                        .black_list
                        .iter()
                        .filter(|m| room_members.contains(m))
                        .map(|&n| Value::Number(n.into()))
                        .collect();
                    obj.insert("BlackList".into(), Value::Array(bl));
                } else {
                    obj.insert("BlackList".into(), Value::Array(vec![]));
                }
            }
        }
        v
    }

    pub fn should_send_blacklist(item_permission: i32) -> bool {
        item_permission == 1 || item_permission == 2
    }

    pub fn ensure_defaults(&mut self) {
        if self.item_permission < 0 || self.item_permission > 5 {
            self.item_permission = 2;
        }
    }
}

fn take_string(obj: &mut serde_json::Map<String, Value>, key: &str) -> Option<String> {
    match obj.remove(key) {
        Some(Value::String(s)) => Some(s),
        Some(v) => Some(v.to_string().trim_matches('"').to_string()),
        None => None,
    }
}

fn take_i64(obj: &mut serde_json::Map<String, Value>, key: &str) -> Option<i64> {
    match obj.remove(key) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    }
}

fn take_member_list(obj: &mut serde_json::Map<String, Value>, key: &str) -> Vec<MemberNumber> {
    match obj.remove(key) {
        Some(Value::Array(arr)) => arr
            .into_iter()
            .filter_map(|v| match v {
                Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

fn member_list_json(list: &[MemberNumber]) -> Value {
    Value::Array(list.iter().map(|&n| json_num(n)).collect())
}

fn json_num(n: i64) -> Value {
    Value::Number(n.into())
}
