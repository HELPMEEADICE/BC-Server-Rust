use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde_json::{json, Value};
use socketioxide::extract::{Data, SocketRef, State};
use tracing::info;

use crate::protocol::events;
use crate::protocol::{ChatRoomCreateRequest, ChatRoomJoinRequest, ChatRoomSearchRequest};
use crate::state::{ChatRoom, OnlineAccount};
use crate::util::{
    is_chat_room_name, CHAT_ROOM_DESCRIPTION_MAX_LENGTH, ROOM_LIMIT_DEFAULT, ROOM_LIMIT_MAXIMUM,
    ROOM_LIMIT_MINIMUM,
};
use crate::AppState;

const CHAT_ROOM_SEARCH_MAX_RESULTS: usize = 240;

/// Node `ChatRoomRoleListIsRestrictive`
pub fn role_list_is_restrictive(roles: &[String]) -> bool {
    !roles.iter().any(|r| r == "All")
}

/// Node `ChatRoomAccountHasAnyRole`
pub fn has_any_role(acc: &OnlineAccount, room: &ChatRoom, roles: &[String]) -> bool {
    if roles.iter().any(|r| r == "All") {
        return true;
    }
    if roles.iter().any(|r| r == "Admin") && room.admin.contains(&acc.member_number) {
        return true;
    }
    if roles.iter().any(|r| r == "Whitelist") && room.whitelist.contains(&acc.member_number) {
        return true;
    }
    false
}

fn generate_room_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub async fn handle_chat_room_search(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    // Query length guard (Node: Query string length > 20 → return)
    let query_raw = data
        .get("Query")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if query_raw.len() > 20 {
        return;
    }

    let req: ChatRoomSearchRequest = serde_json::from_value(data.clone()).unwrap_or(
        ChatRoomSearchRequest {
            query: None,
            space: None,
            game: None,
            full_rooms: None,
            language: None,
            map: None,
            extra: Value::Null,
        },
    );

    let socket_id = socket.id.to_string();
    let results = {
        let world = state.world.read();
        let Some(acc) = world.get_by_socket(&socket_id) else {
            return;
        };

        // Node: Query is trim only; matching uses uppercased room name includes(Query)
        let query = req.query.as_deref().unwrap_or("").trim().to_string();
        let query_upper = query.to_uppercase();
        let include_full = req.full_rooms.unwrap_or(false);
        let show_locked = data
            .get("ShowLocked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let search_descs = data
            .get("SearchDescs")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Spaces: string or array
        let mut spaces: Vec<String> = Vec::new();
        match data.get("Space") {
            Some(Value::String(s)) if s.len() <= 100 => spaces.push(s.clone()),
            Some(Value::Array(arr)) => {
                for s in arr {
                    if let Some(st) = s.as_str() {
                        if st.len() <= 100 {
                            spaces.push(st.to_string());
                        }
                    }
                }
            }
            _ => {
                if let Some(s) = req.space.clone() {
                    spaces.push(s);
                }
            }
        }

        // Languages: string or array
        let mut languages: Vec<String> = Vec::new();
        match data.get("Language") {
            Some(Value::String(s)) if !s.is_empty() => languages.push(s.clone()),
            Some(Value::Array(arr)) => {
                for s in arr {
                    if let Some(st) = s.as_str() {
                        languages.push(st.to_string());
                    }
                }
            }
            _ => {
                if let Some(l) = req.language.clone() {
                    if !l.is_empty() {
                        languages.push(l);
                    }
                }
            }
        }

        // Ignore list (uppercased valid room names)
        let mut ignored: Vec<String> = Vec::new();
        if let Some(Value::Array(arr)) = data.get("Ignore") {
            for r in arr {
                if let Some(name) = r.as_str() {
                    if is_chat_room_name(name) {
                        ignored.push(name.to_uppercase());
                    }
                }
            }
        }

        // MapTypes filter
        let mut map_types: Vec<String> = Vec::new();
        if let Some(Value::Array(arr)) = data.get("MapTypes") {
            for t in arr {
                if let Some(s) = t.as_str() {
                    if s.len() < 20 {
                        map_types.push(s.to_string());
                    }
                }
            }
        }

        let game_filter = req.game.clone().unwrap_or_default();

        // Newest first (Node reverses ChatRoom array)
        let mut rooms: Vec<&ChatRoom> = world.chat_rooms.values().collect();
        rooms.sort_by(|a, b| b.creation.cmp(&a.creation));

        let mut results = Vec::new();
        for room in rooms {
            if room.environment != acc.environment {
                continue;
            }
            if !game_filter.is_empty() && room.game != game_filter {
                continue;
            }
            if room.is_full() && !include_full {
                continue;
            }
            if !spaces.is_empty() && !spaces.iter().any(|s| s == &room.space) {
                continue;
            }
            if room.ban.contains(&acc.member_number) {
                continue;
            }
            if !languages.is_empty() && !languages.iter().any(|l| l == &room.language) {
                continue;
            }

            let room_name_upper = room.name.to_uppercase();
            if !query.is_empty() {
                let mut terms = vec![room_name_upper.clone()];
                if search_descs {
                    terms.push(room.description.to_uppercase());
                }
                // Node: term.includes(Query) — Query not uppercased; room name is upper
                if !terms.iter().any(|t| t.contains(&query) || t.contains(&query_upper)) {
                    // Match Node closely: roomName is upper, Query is as-is after trim
                    if !room_name_upper.contains(&query) {
                        if !(search_descs && room.description.to_uppercase().contains(&query)) {
                            continue;
                        }
                    }
                }
            }

            // Exact name bypasses visibility; otherwise need role
            if room_name_upper != query_upper && !has_any_role(acc, room, &room.visibility) {
                continue;
            }

            // Locked rooms hidden unless ShowLocked or has access
            if !show_locked && !has_any_role(acc, room, &room.access) {
                continue;
            }

            if ignored.iter().any(|n| n == &room_name_upper) {
                continue;
            }

            let map_type = room
                .map_data
                .as_ref()
                .and_then(|m| m.get("Type"))
                .and_then(|v| v.as_str())
                .unwrap_or("Never");
            if !map_types.is_empty() && !map_types.iter().any(|t| t == map_type) {
                continue;
            }

            // Friends list: Submissive if owned, Friend if mutual
            let mut friends = Vec::new();
            for &mn in &room.members {
                if let Some(room_acc) = world.get_by_member(mn) {
                    if room_acc
                        .ownership
                        .as_ref()
                        .and_then(|o| o.get("MemberNumber"))
                        .and_then(|v| v.as_i64())
                        == Some(acc.member_number)
                    {
                        friends.push(json!({
                            "Type": "Submissive",
                            "MemberNumber": room_acc.member_number,
                            "MemberName": room_acc.name,
                        }));
                    } else if acc.friend_list.contains(&room_acc.member_number)
                        && room_acc.friend_list.contains(&acc.member_number)
                    {
                        friends.push(json!({
                            "Type": "Friend",
                            "MemberNumber": room_acc.member_number,
                            "MemberName": room_acc.name,
                        }));
                    }
                }
            }

            results.push(json!({
                "Name": room.name,
                "Language": room.language,
                "Creator": room.creator,
                "CreatorMemberNumber": room.creator_member_number,
                "Creation": room.creation,
                "MemberCount": room.members.len(),
                "MemberLimit": room.limit,
                "Description": room.description,
                "BlockCategory": room.block_category,
                "Game": room.game,
                "Friends": friends,
                "Space": room.space,
                "Visibility": room.visibility,
                "Access": room.access,
                "Locked": room.locked,
                "Private": room.private,
                "MapType": map_type,
                "CanJoin": has_any_role(acc, room, &room.access),
            }));

            if results.len() >= CHAT_ROOM_SEARCH_MAX_RESULTS {
                break;
            }
        }
        results
    };

    let _ = socket.emit(events::CHAT_ROOM_SEARCH_RESULT, &results);
}

pub async fn handle_chat_room_create(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let Ok(req) = serde_json::from_value::<ChatRoomCreateRequest>(data) else {
        let _ = socket.emit(events::CHAT_ROOM_CREATE_RESPONSE, &"InvalidRoomData");
        return;
    };

    if !is_chat_room_name(&req.name) {
        let _ = socket.emit(events::CHAT_ROOM_CREATE_RESPONSE, &"InvalidName");
        return;
    }

    let socket_id = socket.id.to_string();

    let created: Result<_, &'static str> = (|| {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket(&socket_id).cloned() else {
            return Err("__NO_ACC__");
        };

        if let Some(ref rid) = acc.chat_room_id {
            leave_room_inner(&mut world, &socket, rid, acc.member_number);
        }

        if world.get_room_by_name(&acc.environment, &req.name).is_some() {
            return Err("RoomAlreadyExist");
        }

        let mut room = ChatRoom::new(
            generate_room_id(),
            req.name.clone(),
            acc.environment.clone(),
            acc.name.clone(),
            acc.member_number,
        );

        if let Some(d) = req.description {
            room.description = d.chars().take(CHAT_ROOM_DESCRIPTION_MAX_LENGTH).collect();
        }
        if let Some(bg) = req.background {
            room.background = bg;
        }
        if let Some(p) = req.private {
            room.private = p;
        }
        if let Some(l) = req.locked {
            room.locked = l;
        }
        if let Some(v) = req.visibility {
            room.visibility = v;
            room.private = !room.visibility.iter().any(|x| x == "All");
        }
        if let Some(a) = req.access {
            room.access = a;
            room.locked = !room.access.iter().any(|x| x == "All");
        }
        if let Some(s) = req.space {
            room.space = s;
        }
        if let Some(g) = req.game {
            room.game = g;
        }
        if let Some(a) = req.admin {
            room.admin = a;
            if !room.admin.contains(&acc.member_number) {
                room.admin.push(acc.member_number);
            }
        }
        if let Some(b) = req.ban {
            room.ban = b;
        }
        if let Some(w) = req.whitelist {
            room.whitelist = w;
        }
        if let Some(lim) = req.limit {
            let n = lim
                .as_i64()
                .or_else(|| lim.as_str().and_then(|s| s.parse().ok()))
                .unwrap_or(ROOM_LIMIT_DEFAULT as i64) as i32;
            room.limit = n.clamp(ROOM_LIMIT_MINIMUM, ROOM_LIMIT_MAXIMUM);
        }
        if let Some(bc) = req.block_category {
            room.block_category = bc;
        }
        if let Some(lang) = req.language {
            room.language = lang;
        }
        room.map_data = req.map_data;
        room.custom = req.custom;

        room.members.push(acc.member_number);
        let room_id = room.id.clone();
        let room_socket_name = room.socket_room_name();
        let member = acc.member_number;
        let account_name = acc.account_name.clone();
        let room_name = req.name.clone();

        world.insert_room(room);
        if let Some(a) = world.get_by_socket_mut(&socket_id) {
            a.chat_room_id = Some(room_id.clone());
        }

        Ok((room_id, room_socket_name, member, account_name, room_name))
    })();

    let (room_id, room_socket_name, member, account_name, room_name) = match created {
        Ok(v) => v,
        Err("__NO_ACC__") => return,
        Err(code) => {
            let _ = socket.emit(events::CHAT_ROOM_CREATE_RESPONSE, &code);
            return;
        }
    };

    socket.join(room_socket_name);
    info!(room = %room_name, account = %account_name, "Chat room created");

    let _ = socket.emit(events::CHAT_ROOM_CREATE_RESPONSE, &"ChatRoomCreated");
    sync_room_to_member(&socket, &state, &room_id, member);
}

pub async fn handle_chat_room_join(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let Ok(req) = serde_json::from_value::<ChatRoomJoinRequest>(data) else {
        let _ = socket.emit(events::CHAT_ROOM_SEARCH_RESPONSE, &"InvalidRoom");
        return;
    };

    let socket_id = socket.id.to_string();

    let joined: Result<_, &'static str> = (|| {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket(&socket_id).cloned() else {
            return Err("__NO_ACC__");
        };

        if let Some(ref rid) = acc.chat_room_id {
            leave_room_inner(&mut world, &socket, rid, acc.member_number);
        }

        let room_id = {
            let Some(room) = world.get_room_by_name(&acc.environment, &req.name) else {
                return Err("RoomNotFound");
            };
            if room.ban.contains(&acc.member_number) {
                return Err("RoomBanned");
            }
            if room.is_full() {
                return Err("RoomFull");
            }
            if !has_any_role(&acc, room, &room.access) {
                return Err("RoomLocked");
            }
            room.id.clone()
        };

        if let Some(room) = world.chat_rooms.get_mut(&room_id) {
            room.members.push(acc.member_number);
        }
        if let Some(a) = world.get_by_socket_mut(&socket_id) {
            a.chat_room_id = Some(room_id.clone());
        }

        let room_socket_name = world
            .chat_rooms
            .get(&room_id)
            .map(|r| r.socket_room_name())
            .unwrap_or_default();

        let members = world
            .chat_rooms
            .get(&room_id)
            .map(|r| r.members.clone())
            .unwrap_or_default();

        let character = world
            .get_by_member(acc.member_number)
            .map(|a| a.to_synced_character_for_room(&members))
            .unwrap_or(Value::Null);

        // WhiteListedBy / BlackListedBy from other members (Node ChatRoomSyncMemberJoin)
        let mut white_listed_by = Vec::new();
        let mut black_listed_by = Vec::new();
        for &mn in &members {
            if mn == acc.member_number {
                continue;
            }
            if let Some(other) = world.get_by_member(mn) {
                if other.white_list.contains(&acc.member_number) {
                    white_listed_by.push(other.member_number);
                }
                if OnlineAccount::should_send_blacklist(other.item_permission)
                    && other.black_list.contains(&acc.member_number)
                {
                    black_listed_by.push(other.member_number);
                }
            }
        }

        let join_payload = json!({
            "SourceMemberNumber": acc.member_number,
            "Character": character,
            "WhiteListedBy": white_listed_by,
            "BlackListedBy": black_listed_by,
        });

        Ok((
            room_id,
            room_socket_name,
            acc.member_number,
            join_payload,
        ))
    })();

    let (room_id, room_socket_name, member, join_payload) = match joined {
        Ok(v) => v,
        Err("__NO_ACC__") => return,
        Err(code) => {
            let _ = socket.emit(events::CHAT_ROOM_SEARCH_RESPONSE, &code);
            return;
        }
    };

    socket.join(room_socket_name.clone());
    let _ = socket
        .within(room_socket_name)
        .except(socket.id)
        .emit(events::CHAT_ROOM_SYNC_MEMBER_JOIN, &join_payload);

    let _ = socket.emit(events::CHAT_ROOM_SEARCH_RESPONSE, &"JoinedRoom");
    sync_room_to_member(&socket, &state, &room_id, member);
}

pub async fn handle_chat_room_leave(socket: SocketRef, State(state): State<AppState>) {
    // rate limit checked by caller or rate_limited_leave
    let socket_id = socket.id.to_string();
    let mut world = state.world.write();
    let Some(acc) = world.get_by_socket(&socket_id).cloned() else {
        return;
    };
    if let Some(ref rid) = acc.chat_room_id {
        leave_room_inner(&mut world, &socket, rid, acc.member_number);
    }
}

/// Remove `member` from the room. `socket` is used for room broadcast (not necessarily the member).
/// Default reason is `ServerLeave` (Node `ChatRoomLeave` / `ChatRoomRemove`).
pub fn leave_room_inner(
    world: &mut crate::state::World,
    socket: &SocketRef,
    room_id: &str,
    member: i64,
) {
    leave_room_inner_reason(world, socket, room_id, member, Some("ServerLeave"), None);
}

/// Like `leave_room_inner`, optionally emitting a room Action message before leave sync.
pub fn leave_room_inner_reason(
    world: &mut crate::state::World,
    socket: &SocketRef,
    room_id: &str,
    member: i64,
    reason: Option<&str>,
    dictionary: Option<Value>,
) {
    let room_socket_name = world.chat_rooms.get(room_id).map(|r| r.socket_room_name());
    let target_sid = world.get_by_member(member).map(|a| a.socket_id.clone());
    let member_name = world
        .get_by_member(member)
        .map(|a| a.name.clone())
        .unwrap_or_default();

    if let Some(room) = world.chat_rooms.get_mut(room_id) {
        room.members.retain(|&m| m != member);
    }

    if let Some(acc) = world
        .accounts_by_socket
        .values_mut()
        .find(|a| a.member_number == member)
    {
        acc.chat_room_id = None;
    }

    if let Some(name) = room_socket_name {
        // Target leaves the Socket.IO room if still connected
        if let Some(ref sid) = target_sid {
            if let Some(ts) = crate::socket_util::get_socket_from_ref(socket, sid) {
                ts.leave(name.clone());
            } else if socket.id.to_string() == *sid {
                socket.leave(name.clone());
            }
        }

        let remaining = world
            .chat_rooms
            .get(room_id)
            .map(|r| r.members.len())
            .unwrap_or(0);
        // Node: only message + leave sync when room is not empty
        if remaining > 0 {
            if let Some(content) = reason {
                // Node fills SourceCharacter if Dictionary empty
                let dict = dictionary.unwrap_or_else(|| {
                    json!([{
                        "Tag": "SourceCharacter",
                        "Text": member_name,
                        "MemberNumber": member
                    }])
                });
                room_message(socket, &name, member, content, "Action", None, Some(dict));
            }
            let payload = json!({ "SourceMemberNumber": member });
            let _ = socket
                .within(name)
                .emit(events::CHAT_ROOM_SYNC_MEMBER_LEAVE, &payload);
        }
    }

    if let Some(room) = world.chat_rooms.get(room_id) {
        if room.members.is_empty() {
            world.remove_room(room_id);
        }
    }
}

/// Broadcast a ChatRoomMessage to a socket.io room (or a single target member).
pub fn room_message(
    socket: &SocketRef,
    room_socket_name: &str,
    sender: i64,
    content: &str,
    msg_type: &str,
    target: Option<i64>,
    dictionary: Option<Value>,
) {
    let payload = json!({
        "Sender": sender,
        "Content": content,
        "Type": msg_type,
        "Dictionary": dictionary,
    });
    if target.is_some() {
        // Node emits only to the target socket; room broadcast with Target is an approximation
        // used when caller does not resolve the target socket id.
        let mut p = payload;
        if let Some(obj) = p.as_object_mut() {
            obj.insert("Target".into(), json!(target));
        }
        let _ = socket
            .within(room_socket_name.to_string())
            .emit(events::CHAT_ROOM_MESSAGE, &p);
    } else {
        let _ = socket
            .within(room_socket_name.to_string())
            .emit(events::CHAT_ROOM_MESSAGE, &payload);
    }
}

/// Broadcast room properties to all members.
pub fn sync_room_properties(socket: &SocketRef, state: &AppState, room_id: &str, source: i64) {
    let world = state.world.read();
    let Some(room) = world.chat_rooms.get(room_id) else {
        return;
    };
    let mut props = room.to_properties_json();
    if let Some(obj) = props.as_object_mut() {
        obj.insert("SourceMemberNumber".into(), json!(source));
    }
    let name = room.socket_room_name();
    drop(world);
    let _ = socket
        .within(name)
        .emit(events::CHAT_ROOM_SYNC_ROOM_PROPERTIES, &props);
}

/// Sync a single character to the room (ChatRoomSyncCharacter).
pub fn sync_character(socket: &SocketRef, state: &AppState, member: i64, source: i64) {
    let world = state.world.read();
    let Some(acc) = world.get_by_member(member) else {
        return;
    };
    let Some(ref room_id) = acc.chat_room_id else {
        return;
    };
    let Some(room) = world.chat_rooms.get(room_id) else {
        return;
    };
    let character = acc.to_synced_character_for_room(&room.members);
    let name = room.socket_room_name();
    let payload = json!({
        "Character": character,
        "SourceMemberNumber": source,
    });
    drop(world);
    let _ = socket
        .within(name)
        .emit(events::CHAT_ROOM_SYNC_CHARACTER, &payload);
}

pub fn sync_room_to_member(socket: &SocketRef, state: &AppState, room_id: &str, source_member: i64) {
    let payload = {
        let world = state.world.read();
        let Some(room) = world.chat_rooms.get(room_id) else {
            return;
        };

        let mut characters = Vec::new();
        for &mn in &room.members {
            if let Some(acc) = world.get_by_member(mn) {
                characters.push(acc.to_synced_character_for_room(&room.members));
            }
        }

        let mut payload = room.to_properties_json();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("Character".into(), Value::Array(characters));
            obj.insert("SourceMemberNumber".into(), json!(source_member));
        }
        payload
    };

    let _ = socket.emit(events::CHAT_ROOM_SYNC, &payload);
}

pub mod admin;
