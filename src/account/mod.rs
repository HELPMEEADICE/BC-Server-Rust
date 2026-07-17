use serde_json::{json, Value};
use socketioxide::extract::{Data, SocketRef, State};
use tracing::{error, info};

use crate::db::json_object_to_set_map;
use crate::protocol::events;
use crate::protocol::{AccountBeepRequest, AccountQueryRequest, AccountUpdateEmailRequest};
use crate::room::role_list_is_restrictive;
use crate::util::{
	apply_dotted_path, common_time, is_email_valid, is_simple_object, DIFFICULTY_DELAY_MS,
};
use crate::AppState;

const IMMUTABLE_KEYS: &[&str] = &[
    "Name",
    "AccountName",
    "Password",
    "Email",
    "Creation",
    "LastLogin",
    "Pose",
    "ActivePose",
    "ChatRoom",
    "ID",
    "Socket",
    "Inventory",
    "_id",
    "MemberNumber",
    "Environment",
    "Ownership",
    "Lovership",
    "Difficulty",
    "AssetFamily",
    "DelayedAppearanceUpdate",
    "DelayedSkillUpdate",
    "DelayedGameUpdate",
];

const ROOM_SYNC_KEYS: &[&str] = &[
    "MapData",
    "Title",
    "Nickname",
    "Crafting",
    "Reputation",
    "Description",
    "LabelColor",
    "ItemPermission",
    "InventoryData",
    "BlockItems",
    "LimitedItems",
    "FavoriteItems",
    "OnlineSharedSettings",
    "WhiteList",
    "BlackList",
];

pub async fn handle_account_update(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    if !is_simple_object(&data) {
        return;
    }

    let socket_id = socket.id.to_string();
    let mut data = data;

    if let Some(obj) = data.as_object_mut() {
        for k in IMMUTABLE_KEYS {
            obj.remove(*k);
        }
    }

    let (account_name, should_persist, should_room_sync, member) = {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket_mut(&socket_id) else {
            return;
        };

        // NPC Lover: Lover string starting with "NPC-"
        let mut npc_lovership_for_db: Option<Value> = None;
        if let Some(obj) = data.as_object_mut() {
            if let Some(Value::String(lover)) = obj.get("Lover").cloned() {
                if lover.starts_with("NPC-") && acc.lovership.len() < 5 {
                    let present = acc.lovership.iter().any(|l| {
                        l.get("Name").and_then(|n| n.as_str()) == Some(lover.as_str())
                    });
                    if !present {
                        acc.lovership.push(json!({ "Name": lover }));
                        let mut clean = acc.lovership.clone();
                        clean.retain(|l| {
                            l.get("BeginDatingOfferedByMemberNumber").is_none()
                        });
                        for entry in &mut clean {
                            if let Some(o) = entry.as_object_mut() {
                                o.remove("BeginEngagementOfferedByMemberNumber");
                                o.remove("BeginWeddingOfferedByMemberNumber");
                            }
                        }
                        acc.lovership = clean.clone();
                        npc_lovership_for_db = Some(Value::Array(clean.clone()));
                        let _ = socket.emit(
                            events::ACCOUNT_LOVERSHIP,
                            &json!({ "Lovership": clean }),
                        );
                    }
                }
                obj.remove("Lover");
            }
        }

        let obj = match data.as_object() {
            Some(o) => o,
            None => return,
        };

        if let Some(v) = obj.get("InventoryData") {
            acc.inventory_data = Some(v.clone());
        }
        if let Some(v) = obj.get("ItemPermission").and_then(|v| v.as_i64()) {
            acc.item_permission = v as i32;
        }
        if let Some(v) = obj.get("ArousalSettings") {
            acc.arousal_settings = Some(v.clone());
        }
        if let Some(v) = obj.get("OnlineSharedSettings") {
            acc.online_shared_settings = Some(v.clone());
        }
        if let Some(v) = obj.get("Game") {
            acc.game = Some(v.clone());
        }
        if let Some(v) = obj.get("MapData") {
            acc.map_data = Some(v.clone());
        }
        if let Some(v) = obj.get("LabelColor").and_then(|v| v.as_str()) {
            acc.label_color = Some(v.to_string());
        }
        if let Some(v) = obj.get("Appearance") {
            acc.appearance = Some(v.clone());
        }
        if let Some(v) = obj.get("Reputation") {
            acc.reputation = Some(v.clone());
        }
        if let Some(v) = obj.get("Description").and_then(|v| v.as_str()) {
            acc.description = Some(v.to_string());
        }
        if let Some(v) = obj.get("BlockItems") {
            acc.block_items = Some(v.clone());
        }
        if let Some(v) = obj.get("LimitedItems") {
            acc.limited_items = Some(v.clone());
        }
        if let Some(v) = obj.get("FavoriteItems") {
            acc.favorite_items = Some(v.clone());
        }
        if let Some(Value::Array(arr)) = obj.get("WhiteList") {
            acc.white_list = arr.iter().filter_map(|v| v.as_i64()).collect();
        }
        if let Some(Value::Array(arr)) = obj.get("BlackList") {
            acc.black_list = arr.iter().filter_map(|v| v.as_i64()).collect();
        }
        if let Some(Value::Array(arr)) = obj.get("FriendList") {
            acc.friend_list = arr.iter().filter_map(|v| v.as_i64()).collect();
        }
        if let Some(v) = obj.get("Title").and_then(|v| v.as_str()) {
            acc.title = Some(v.to_string());
        }
        if let Some(v) = obj.get("Nickname").and_then(|v| v.as_str()) {
            acc.nickname = Some(v.to_string());
        }
        if let Some(v) = obj.get("Crafting") {
            acc.crafting = Some(v.clone());
        }
        // Private room NPCs are stored as account data. Keep the latest client
        // snapshot in memory too, so subsequent session operations cannot retain
        // the stale login-time version until the player reconnects.
        if let Some(v) = obj.get("PrivateCharacter") {
            acc.extra.insert("PrivateCharacter".into(), v.clone());
        }

        // Client may send Mongo-style dotted keys such as
        // `ExtensionSettings.UndergroundPrison`. Nest them into `extra` so the
        // online session and LoginResponse stay consistent with DB storage.
        for (k, v) in obj.iter() {
            if k.contains('.') {
                // Build nested structure inside a temp map of extra keys.
                let mut nested = serde_json::Map::new();
                // Seed with existing top-level extra values that match the first segment.
                if let Some(first) = k.split('.').next() {
                    if let Some(existing) = acc.extra.get(first) {
                        nested.insert(first.to_string(), existing.clone());
                    }
                }
                apply_dotted_path(&mut nested, k, v.clone());
                for (nk, nv) in nested {
                    acc.extra.insert(nk, nv);
                }
            } else if k == "ExtensionSettings" {
                acc.extra.insert("ExtensionSettings".into(), v.clone());
            } else if k == "Money" {
                acc.extra.insert("Money".into(), v.clone());
            } else if k == "Log" {
                acc.extra.insert("Log".into(), v.clone());
            }
        }

        let should_room_sync = acc.chat_room_id.is_some()
            && ROOM_SYNC_KEYS.iter().any(|k| obj.contains_key(*k));
        let member = acc.member_number;
        let account_name = acc.account_name.clone();

        let should_persist = if obj.len() == 1 {
            if obj.contains_key("Appearance") {
                acc.delayed_appearance = obj.get("Appearance").cloned();
                false
            } else if obj.contains_key("Skill") {
                acc.delayed_skill = obj.get("Skill").cloned();
                false
            } else if obj.contains_key("Game") {
                acc.delayed_game = obj.get("Game").cloned();
                false
            } else {
                true
            }
        } else {
            if obj.len() > 1 {
                if obj.contains_key("Appearance") {
                    acc.delayed_appearance = None;
                }
                if obj.contains_key("Skill") {
                    acc.delayed_skill = None;
                }
                if obj.contains_key("Game") {
                    acc.delayed_game = None;
                }
            }
            true
        };

        // Merge NPC lovership into persist payload
        if let Some(ls) = npc_lovership_for_db {
            if let Some(o) = data.as_object_mut() {
                o.insert("Lovership".into(), ls);
            }
        }

        (account_name, should_persist, should_room_sync, member)
    };

    if should_room_sync {
        crate::room::sync_character(&socket, &state, member, member);
    }

    if !should_persist {
        return;
    }

    if let Some(obj) = data.as_object_mut() {
        obj.remove("MapData");
        obj.remove("Lover");
    }

    if data.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return;
    }

    match json_object_to_set_map(&data) {
        Ok(set) if !set.is_empty() => {
            if let Err(e) = state.db.update_fields(&account_name, set).await {
                error!(error = %e, account = %account_name, "AccountUpdate DB error");
            }
        }
        Ok(_) => {}
        Err(e) => error!(error = %e, "AccountUpdate serialize error"),
    }
}

pub async fn handle_account_update_email(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let fail = || {
        let _ = socket.emit(
            events::ACCOUNT_QUERY_RESULT,
            &json!({ "Query": "EmailUpdate", "Result": false }),
        );
    };

    let Ok(req) = serde_json::from_value::<AccountUpdateEmailRequest>(data) else {
        fail();
        return;
    };

    let socket_id = socket.id.to_string();
    let account_name = {
        let world = state.world.read();
        match world.get_by_socket(&socket_id) {
            Some(a) => a.account_name.clone(),
            None => {
                fail();
                return;
            }
        }
    };

    if !req.email_new.is_empty() && !is_email_valid(&req.email_new) {
        fail();
        return;
    }

    let current = match state.db.find_email_by_account_name(&account_name).await {
        Ok(e) => e.unwrap_or_default(),
        Err(e) => {
            error!(error = %e, "email lookup failed");
            fail();
            return;
        }
    };

    if !current.is_empty()
        && current.trim().to_lowercase() != req.email_old.trim().to_lowercase()
    {
        fail();
        return;
    }

    if let Err(e) = state.db.set_email(&account_name, &req.email_new).await {
        error!(error = %e, "email update failed");
        fail();
        return;
    }

    info!(account = %account_name, "Updated email");
    let _ = socket.emit(
        events::ACCOUNT_QUERY_RESULT,
        &json!({ "Query": "EmailUpdate", "Result": true }),
    );
}

pub async fn handle_account_query(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let Ok(req) = serde_json::from_value::<AccountQueryRequest>(data) else {
        return;
    };

    let socket_id = socket.id.to_string();

    match req.query.as_str() {
        "EmailStatus" => {
            let account_name = {
                let world = state.world.read();
                match world.get_by_socket(&socket_id) {
                    Some(a) => a.account_name.clone(),
                    None => return,
                }
            };
            let email = state
                .db
                .find_email_by_account_name(&account_name)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let result = !email.is_empty();
            let _ = socket.emit(
                events::ACCOUNT_QUERY_RESULT,
                &json!({ "Query": "EmailStatus", "Result": result }),
            );
        }
        "OnlineFriends" => {
            let friends = {
                let world = state.world.read();
                let Some(acc) = world.get_by_socket(&socket_id) else {
                    return;
                };
                // Node requires FriendList != null (we always have a vec)
                let mut friends = Vec::new();
                let mut index: Vec<i64> = Vec::new();

                // Pass 1: submissives owned by player + lovers
                for other in world.accounts_by_socket.values() {
                    if other.environment != acc.environment {
                        continue;
                    }
                    let lovers_numbers: Vec<i64> = other
                        .lovership
                        .iter()
                        .filter_map(|l| l.get("MemberNumber").and_then(|v| v.as_i64()))
                        .collect();
                    let is_owned = other
                        .ownership
                        .as_ref()
                        .and_then(|o| o.get("MemberNumber"))
                        .and_then(|v| v.as_i64())
                        == Some(acc.member_number);
                    let is_lover = lovers_numbers.contains(&acc.member_number);
                    if is_owned || is_lover {
                        let ftype = if is_owned { "Submissive" } else { "Lover" };
                        friends.push(query_friend_info(ftype, other, &world));
                        index.push(other.member_number);
                    }
                }

                // Pass 2: mutual friends not already indexed
                for &mn in &acc.friend_list {
                    if index.contains(&mn) {
                        continue;
                    }
                    if let Some(other) = world.get_by_member(mn) {
                        if other.environment == acc.environment
                            && other.friend_list.contains(&acc.member_number)
                        {
                            friends.push(query_friend_info("Friend", other, &world));
                        }
                    }
                }
                friends
            };
            let _ = socket.emit(
                events::ACCOUNT_QUERY_RESULT,
                &json!({ "Query": "OnlineFriends", "Result": friends }),
            );
        }
        _ => {}
    }
}

fn query_friend_info(
    ftype: &str,
    account: &crate::state::OnlineAccount,
    world: &crate::state::World,
) -> Value {
    let mut info = json!({
        "Type": ftype,
        "MemberNumber": account.member_number,
        "MemberName": account.name,
        "MemberNickname": account.nickname,
    });
    if let Some(ref room_id) = account.chat_room_id {
        if let Some(room) = world.chat_rooms.get(room_id) {
            if role_list_is_restrictive(&room.visibility) {
                info["Private"] = json!(true);
                if ftype == "Friend" {
                    // Hide room details for normal friends in private rooms
                    return info;
                }
            }
            info["ChatRoomSpace"] = json!(room.space);
            info["ChatRoomName"] = json!(room.name);
            info["ChatRoomMemberCount"] = json!(room.members.len());
            info["ChatRoomLimit"] = json!(room.limit);
        }
    }
    info
}

pub async fn handle_account_beep(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let Ok(req) = serde_json::from_value::<AccountBeepRequest>(data) else {
        return;
    };
    let Some(target_mn) = req.member_number else {
        return;
    };

    let is_secret = req
        .extra
        .get("IsSecret")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let beep_type = req.beep_type.clone();
    let is_leash = beep_type.as_deref() == Some("Leash");

    let socket_id = socket.id.to_string();
    let (target_socket_id, payload) = {
        let world = state.world.read();
        let Some(src) = world.get_by_socket(&socket_id) else {
            return;
        };
        let Some(target) = world.get_by_member(target_mn) else {
            return;
        };

        if target.environment != src.environment {
            return;
        }

        // Node: target has source on FriendList OR source owns target OR BeepType Leash
        let target_friends_src = target.friend_list.contains(&src.member_number);
        let src_owns_target = target
            .ownership
            .as_ref()
            .and_then(|o| o.get("MemberNumber"))
            .and_then(|v| v.as_i64())
            == Some(src.member_number);

        if !(target_friends_src || src_owns_target || is_leash) {
            return;
        }

        let (room_space, room_name, private) = if src.chat_room_id.is_none() || is_secret {
            (Value::Null, Value::Null, Value::Null)
        } else if let Some(ref rid) = src.chat_room_id {
            if let Some(room) = world.chat_rooms.get(rid) {
                (
                    json!(room.space),
                    json!(room.name),
                    json!(role_list_is_restrictive(&room.visibility)),
                )
            } else {
                (Value::Null, Value::Null, Value::Null)
            }
        } else {
            (Value::Null, Value::Null, Value::Null)
        };

        let payload = json!({
            "MemberNumber": src.member_number,
            "MemberName": src.name,
            "ChatRoomSpace": room_space,
            "ChatRoomName": room_name,
            "Private": private,
            "BeepType": beep_type,
            "Message": req.message,
        });
        (target.socket_id.clone(), payload)
    };

    if let Some(io) = state.io.get() {
        crate::socket_util::emit_to(io, &target_socket_id, events::ACCOUNT_BEEP, &payload);
    }
}

pub async fn handle_account_difficulty(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let level = match data {
        Value::Number(n) => n.as_i64().unwrap_or(-1),
        _ => -1,
    };
    if !(0..=3).contains(&level) {
        return;
    }

    let socket_id = socket.id.to_string();
    let (account_name, can_change) = {
        let world = state.world.read();
        let Some(acc) = world.get_by_socket(&socket_id) else {
            return;
        };
        // Node: can set to 2 or 3 only if no change for 1 week; lower always ok
        let can = if level <= 1 {
            true
        } else {
            let last_change = match &acc.difficulty {
                Some(d) => d
                    .get("LastChange")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(acc.creation),
                None => acc.creation,
            };
            last_change + DIFFICULTY_DELAY_MS < common_time()
        };
        (acc.account_name.clone(), can)
    };

    if !can_change {
        return;
    }

    let difficulty = json!({
        "Level": level,
        "LastChange": common_time(),
    });

    {
        let mut world = state.world.write();
        if let Some(acc) = world.get_by_socket_mut(&socket_id) {
            acc.difficulty = Some(difficulty.clone());
        }
    }

    if let Ok(set) = json_object_to_set_map(&json!({ "Difficulty": difficulty })) {
        if let Err(e) = state.db.update_fields(&account_name, set).await {
            error!(error = %e, "difficulty update failed");
        }
    }
}

pub async fn flush_delayed_updates(state: &AppState) {
    let pending: Vec<(String, Option<Value>, Option<Value>, Option<Value>)> = {
        let mut world = state.world.write();
        let mut out = Vec::new();
        for acc in world.accounts_by_socket.values_mut() {
            if acc.delayed_appearance.is_some()
                || acc.delayed_skill.is_some()
                || acc.delayed_game.is_some()
            {
                out.push((
                    acc.account_name.clone(),
                    acc.delayed_appearance.take(),
                    acc.delayed_skill.take(),
                    acc.delayed_game.take(),
                ));
            }
        }
        out
    };

    for (name, appearance, skill, game) in pending {
        let mut set_json = serde_json::Map::new();
        if let Some(a) = appearance {
            set_json.insert("Appearance".into(), a);
        }
        if let Some(s) = skill {
            set_json.insert("Skill".into(), s);
        }
        if let Some(g) = game {
            set_json.insert("Game".into(), g);
        }
        if set_json.is_empty() {
            continue;
        }
        match json_object_to_set_map(&Value::Object(set_json)) {
            Ok(doc) => {
                if let Err(e) = state.db.update_fields(&name, doc).await {
                    error!(error = %e, account = %name, "delayed update failed");
                }
            }
            Err(e) => error!(error = %e, "delayed update serialize failed"),
        }
    }
}
