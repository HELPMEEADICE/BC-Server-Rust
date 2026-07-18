use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde_json::{json, Value};
use socketioxide::extract::{Data, SocketRef, State};
use tracing::info;

use crate::protocol::events;
use crate::protocol::{ChatRoomCreateRequest, ChatRoomJoinRequest};
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
    // Node requires a plain object with a string Query.
    let Some(query_raw) = data.get("Query").and_then(|v| v.as_str()) else {
        return;
    };
    if query_raw.len() > 20 {
        return;
    }

    let socket_id = socket.id.to_string();
    let results = {
        let world = state.world.read();
        let Some(acc) = world.get_by_socket(&socket_id) else {
            return;
        };

        // Node: Query is trim only; matching uses uppercased room name includes(Query)
        let query = query_raw.trim().to_string();
        let query_upper = query.to_uppercase();
        let include_full = data
            .get("FullRooms")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
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
            _ => {}
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
            _ => {}
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

        let game_filter = data
            .get("Game")
            .and_then(|v| v.as_str())
            .filter(|game| game.len() <= 100)
            .unwrap_or("");

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
            // Node: Spaces.includes(room.Space) — empty Spaces matches nothing
            if !spaces.iter().any(|s| s == &room.space) {
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
                // Node: roomName UPPER, Query as-is after trim — term.includes(Query)
                let mut matched = room_name_upper.contains(query.as_str());
                if !matched && search_descs {
                    matched = room.description.to_uppercase().contains(query.as_str());
                }
                if !matched {
                    continue;
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

            // Node filter uses room.MapData?.Type (undefined if missing) then result MapType defaults Never
            let map_type_raw = room
                .map_data
                .as_ref()
                .and_then(|m| m.get("Type"))
                .and_then(|v| v.as_str());
            if !map_types.is_empty() {
                match map_type_raw {
                    Some(t) if map_types.iter().any(|x| x == t) => {}
                    _ => continue,
                }
            }
            let map_type = map_type_raw.unwrap_or("Never");

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
    // Node requires Name, Description, Background as strings
    let name_raw = data.get("Name").and_then(|v| v.as_str()).map(str::trim);
    let description = data.get("Description").and_then(|v| v.as_str());
    let background = data.get("Background").and_then(|v| v.as_str());
    let (Some(name_raw), Some(description), Some(background)) = (name_raw, description, background)
    else {
        let _ = socket.emit(events::CHAT_ROOM_CREATE_RESPONSE, &"InvalidRoomData");
        return;
    };

    // Node Private/Visibility XOR + Access/Locked compatibility
    let has_visibility = data
        .get("Visibility")
        .map(|v| v.is_array())
        .unwrap_or(false);
    let has_private = data.get("Private").map(|v| v.is_boolean()).unwrap_or(false);
    if has_visibility == has_private {
        let _ = socket.emit(events::CHAT_ROOM_CREATE_RESPONSE, &"InvalidRoomData");
        return;
    }
    let has_access = data.get("Access").map(|v| v.is_array()).unwrap_or(false);
    let has_locked = data.get("Locked").map(|v| v.is_boolean()).unwrap_or(false);
    if has_access && has_locked {
        let _ = socket.emit(events::CHAT_ROOM_CREATE_RESPONSE, &"InvalidRoomData");
        return;
    }

    if !is_chat_room_name(name_raw)
        || description.chars().count() > CHAT_ROOM_DESCRIPTION_MAX_LENGTH
        || background.len() > 100
    {
        let _ = socket.emit(events::CHAT_ROOM_CREATE_RESPONSE, &"InvalidRoomData");
        return;
    }

    // Node normalizes malformed optional lists instead of rejecting the entire room.
    let mut normalized = data.clone();
    let Some(fields) = normalized.as_object_mut() else {
        let _ = socket.emit(events::CHAT_ROOM_CREATE_RESPONSE, &"InvalidRoomData");
        return;
    };
    for key in ["Admin", "Ban", "Whitelist"] {
        if !fields.get(key).is_some_and(|value| {
            value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_i64))
        }) {
            fields.insert(key.to_string(), Value::Null);
        }
    }
    for key in ["Space", "Game", "Language"] {
        if fields.get(key).is_some_and(|value| !value.is_string()) {
            fields.insert(key.to_string(), Value::Null);
        }
    }
    if fields.get("BlockCategory").is_some_and(|value| {
        !value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
    }) {
        fields.insert("BlockCategory".to_string(), json!([]));
    }
    let Ok(mut req) = serde_json::from_value::<ChatRoomCreateRequest>(normalized) else {
        let _ = socket.emit(events::CHAT_ROOM_CREATE_RESPONSE, &"InvalidRoomData");
        return;
    };
    req.name = name_raw.to_string();

    // Fill Private from Visibility / Access from Locked (Node compat block)
    if has_visibility {
        if let Some(ref v) = req.visibility {
            req.private = Some(!v.iter().any(|x| x == "All"));
        }
    } else if let Some(p) = req.private {
        req.visibility = Some(if p {
            vec!["Admin".into()]
        } else {
            vec!["All".into()]
        });
    }
    if has_access {
        if let Some(ref a) = req.access {
            req.locked = Some(!a.iter().any(|x| x == "All"));
        }
    } else if let Some(l) = req.locked {
        req.access = Some(if l {
            vec!["Admin".into(), "Whitelist".into()]
        } else {
            vec!["All".into()]
        });
    } else {
        req.locked = Some(false);
        req.access = Some(vec!["All".into()]);
    }

    let socket_id = socket.id.to_string();

    let created: Result<_, &'static str> = (|| {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket(&socket_id).cloned() else {
            return Err("AccountError");
        };

        // Node: name unique globally (all environments)
        if world.room_name_exists_any(&req.name) {
            return Err("RoomAlreadyExist");
        }
        if let Some(ref rid) = acc.chat_room_id {
            leave_room_inner(&mut world, &socket, rid, acc.member_number);
        }

        let mut room = ChatRoom::new(
            generate_room_id(),
            req.name.clone(),
            acc.environment.clone(),
            acc.name.clone(),
            acc.member_number,
        );

        room.description = description
            .chars()
            .take(CHAT_ROOM_DESCRIPTION_MAX_LENGTH)
            .collect();
        room.background = background.to_string();
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
            if s.len() <= 100 {
                room.space = s;
            }
        }
        if let Some(g) = req.game {
            if g.len() <= 100 {
                room.game = g;
            }
        }
        // Node: invalid Admin → [creator]; valid Admin used as-is (no force-add)
        if let Some(a) = req.admin {
            room.admin = a;
        } else {
            room.admin = vec![acc.member_number];
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
            room.limit = if (ROOM_LIMIT_MINIMUM..=ROOM_LIMIT_MAXIMUM).contains(&n) {
                n
            } else {
                ROOM_LIMIT_DEFAULT
            };
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
        // Node: InvalidRoomData
        let _ = socket.emit(events::CHAT_ROOM_SEARCH_RESPONSE, &"InvalidRoomData");
        return;
    };
    if req.name.is_empty() {
        let _ = socket.emit(events::CHAT_ROOM_SEARCH_RESPONSE, &"InvalidRoomData");
        return;
    }

    let socket_id = socket.id.to_string();

    let joined: Result<_, &'static str> = (|| {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket(&socket_id).cloned() else {
            return Err("AccountError");
        };

        let room_id = {
            let Some(room) = world.get_room_by_name(&acc.environment, &req.name) else {
                // Node: CannotFindRoom
                return Err("CannotFindRoom");
            };
            if room.is_full() {
                return Err("RoomFull");
            }
            if room.ban.contains(&acc.member_number) {
                return Err("RoomBanned");
            }
            if !has_any_role(&acc, room, &room.access) {
                return Err("RoomLocked");
            }
            // Node: already in this room → AlreadyInRoom (no leave/rejoin)
            if acc.chat_room_id.as_deref() == Some(room.id.as_str()) {
                return Err("AlreadyInRoom");
            }
            room.id.clone()
        };

        // Leave previous room if any (Node ChatRoomRemove before push)
        if let Some(ref rid) = acc.chat_room_id {
            leave_room_inner(&mut world, &socket, rid, acc.member_number);
        }

        // Guard: still online after leave (Node Account.find check)
        if world.get_by_member(acc.member_number).is_none() {
            return Err("AccountError");
        }

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

        let member_name = acc.name.clone();

        Ok((
            room_id,
            room_socket_name,
            acc.member_number,
            member_name,
            join_payload,
        ))
    })();

    let (room_id, room_socket_name, member, member_name, join_payload) = match joined {
        Ok(v) => v,
        Err(code) => {
            let _ = socket.emit(events::CHAT_ROOM_SEARCH_RESPONSE, &code);
            return;
        }
    };

    // Node order: join socket room → JoinedRoom → ChatRoomSyncMemberJoin → ChatRoomSync → ServerEnter
    socket.join(room_socket_name.clone());
    let _ = socket.emit(events::CHAT_ROOM_SEARCH_RESPONSE, &"JoinedRoom");
    // Node: Character.Socket.to("chatroom-…") — others only
    crate::socket_util::emit_to_room(
        &socket,
        room_socket_name.clone(),
        events::CHAT_ROOM_SYNC_MEMBER_JOIN,
        &join_payload,
    );
    sync_room_to_member(&socket, &state, &room_id, member);
    // Node: ChatRoomMessage ServerEnter to whole room
    room_message(
        &socket,
        &state,
        &room_socket_name,
        member,
        "ServerEnter",
        "Action",
        None,
        Some(json!([{
            "Tag": "SourceCharacter",
            "Text": member_name,
            "MemberNumber": member
        }])),
    );
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
    leave_room_inner_reason(
        world,
        Some(socket),
        None,
        room_id,
        member,
        Some("ServerLeave"),
        None,
    );
}

/// Like `leave_room_inner`, optionally emitting a room Action message before leave sync.
/// Prefer `socket` when available; use `io` for disconnect / dup-login (Node `IO.to`).
pub fn leave_room_inner_reason(
    world: &mut crate::state::World,
    socket: Option<&SocketRef>,
    io: Option<&socketioxide::SocketIo>,
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
            if let Some(s) = socket {
                if let Some(ts) = crate::socket_util::get_socket_from_ref(s, sid) {
                    ts.leave(name.clone());
                } else if s.id.to_string() == *sid {
                    s.leave(name.clone());
                }
            } else if let Some(io) = io {
                if let Some(ts) = crate::socket_util::get_socket(io, sid) {
                    ts.leave(name.clone());
                }
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
                let dict = dictionary.unwrap_or_else(|| {
                    json!([{
                        "Tag": "SourceCharacter",
                        "Text": member_name,
                        "MemberNumber": member
                    }])
                });
                let payload = json!({
                    "Sender": member,
                    "Content": content,
                    "Type": "Action",
                    "Dictionary": dict,
                });
                if let Some(s) = socket {
                    crate::socket_util::emit_within(
                        s,
                        name.clone(),
                        events::CHAT_ROOM_MESSAGE,
                        &payload,
                    );
                } else if let Some(io) = io {
                    crate::socket_util::emit_io_within(
                        io,
                        name.clone(),
                        events::CHAT_ROOM_MESSAGE,
                        &payload,
                    );
                }
            }
            let payload = json!({ "SourceMemberNumber": member });
            if let Some(s) = socket {
                crate::socket_util::emit_within(
                    s,
                    name,
                    events::CHAT_ROOM_SYNC_MEMBER_LEAVE,
                    &payload,
                );
            } else if let Some(io) = io {
                crate::socket_util::emit_io_within(
                    io,
                    name,
                    events::CHAT_ROOM_SYNC_MEMBER_LEAVE,
                    &payload,
                );
            }
        }
    }

    if let Some(room) = world.chat_rooms.get(room_id) {
        if room.members.is_empty() {
            world.remove_room(room_id);
        }
    }
}

/// Notify room of disconnect/leave without a live peer socket (Node `ChatRoomRemove` via IO).
pub fn leave_room_on_disconnect(
    world: &mut crate::state::World,
    io: Option<&socketioxide::SocketIo>,
    room_id: &str,
    member: i64,
    reason: &str,
) {
    leave_room_inner_reason(world, None, io, room_id, member, Some(reason), None);
}

/// Broadcast a ChatRoomMessage to a socket.io room, or only to a target member (Node).
/// `target` is MemberNumber; when set, message is delivered only to that socket.
pub fn room_message(
    socket: &SocketRef,
    state: &AppState,
    room_socket_name: &str,
    sender: i64,
    content: &str,
    msg_type: &str,
    target: Option<i64>,
    dictionary: Option<Value>,
) {
    let mut payload = json!({
        "Sender": sender,
        "Content": content,
        "Type": msg_type,
        "Dictionary": dictionary,
    });
    if dictionary.is_none() {
        payload.as_object_mut().unwrap().remove("Dictionary");
    }
    if let Some(target_mn) = target {
        // Node: only target socket; no room broadcast, no sender echo
        let sid = state
            .world
            .read()
            .get_by_member(target_mn)
            .map(|a| a.socket_id.clone());
        if let Some(sid) = sid {
            if let Some(io) = state.io.get() {
                crate::socket_util::emit_to(io, &sid, events::CHAT_ROOM_MESSAGE, &payload);
            } else if let Some(ts) = crate::socket_util::get_socket_from_ref(socket, &sid) {
                let _ = ts.emit(events::CHAT_ROOM_MESSAGE, &payload);
            }
        }
        return;
    }
    crate::socket_util::emit_within(
        socket,
        room_socket_name.to_string(),
        events::CHAT_ROOM_MESSAGE,
        &payload,
    );
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
    crate::socket_util::emit_within(socket, name, events::CHAT_ROOM_SYNC_ROOM_PROPERTIES, &props);
}

/// Sync a single character to the room (ChatRoomSyncCharacter).
/// Node sends through the source account's socket, excluding that account.
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
    let source_socket_id = world
        .get_by_member(source)
        .map(|account| account.socket_id.clone());
    drop(world);
    if let Some(source_socket_id) = source_socket_id {
        if let Some(io) = state.io.get() {
            if let Some(source_socket) = crate::socket_util::get_socket(io, &source_socket_id) {
                crate::socket_util::emit_to_room(
                    &source_socket,
                    name,
                    events::CHAT_ROOM_SYNC_CHARACTER,
                    &payload,
                );
                return;
            }
        }
        if let Some(source_socket) =
            crate::socket_util::get_socket_from_ref(socket, &source_socket_id)
        {
            crate::socket_util::emit_to_room(
                &source_socket,
                name,
                events::CHAT_ROOM_SYNC_CHARACTER,
                &payload,
            );
            return;
        }
    }
    crate::socket_util::emit_to_room(socket, name, events::CHAT_ROOM_SYNC_CHARACTER, &payload);
}

pub fn sync_room_to_member(
    socket: &SocketRef,
    state: &AppState,
    room_id: &str,
    source_member: i64,
) {
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
