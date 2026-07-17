use serde_json::{json, Value};
use socketioxide::extract::{Data, SocketRef, State};

use crate::protocol::codes;
use crate::protocol::events;
use crate::protocol::ChatRoomMessage;
use crate::util::CHAT_MESSAGE_MAX_LENGTH;
use crate::AppState;

pub async fn handle_chat_room_chat(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let Ok(msg) = serde_json::from_value::<ChatRoomMessage>(data.clone()) else {
        return;
    };

    let msg_type = msg.msg_type.as_deref().unwrap_or("");
    if !codes::CHAT_TYPES.contains(&msg_type) {
        return;
    }

    // Node: data.Content.trim() before length check
    let content = msg.content.as_deref().unwrap_or("").trim().to_string();
    if content.len() > CHAT_MESSAGE_MAX_LENGTH {
        return;
    }

    let socket_id = socket.id.to_string();
    let world = state.world.read();
    let Some(acc) = world.get_by_socket(&socket_id) else {
        return;
    };
    let Some(ref room_id) = acc.chat_room_id else {
        return;
    };
    let Some(room) = world.chat_rooms.get(room_id) else {
        return;
    };

    let payload = json!({
        "Content": content,
        "Type": msg_type,
        "Sender": acc.member_number,
        "Dictionary": msg.dictionary,
    });

    let room_name = room.socket_room_name();

    // Node ChatRoomMessage: any Target → only that member (no room broadcast, no sender echo)
    if let Some(target_mn) = msg.target {
        if let Some(target) = world.get_by_member(target_mn) {
            // Target should be in same room (Node iterates room accounts)
            if room.members.contains(&target_mn) {
                let tid = target.socket_id.clone();
                drop(world);
                if let Some(io) = state.io.get() {
                    crate::socket_util::emit_to(io, &tid, events::CHAT_ROOM_MESSAGE, &payload);
                }
            }
        }
        return;
    }

    drop(world);
    crate::socket_util::emit_within(&socket, room_name, events::CHAT_ROOM_MESSAGE, &payload);
}

pub async fn handle_chat_room_game(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let socket_id = socket.id.to_string();
    let world = state.world.read();
    let Some(acc) = world.get_by_socket(&socket_id) else {
        return;
    };
    let Some(ref room_id) = acc.chat_room_id else {
        return;
    };
    let Some(room) = world.chat_rooms.get(room_id) else {
        return;
    };

    let payload = json!({
        "Sender": acc.member_number,
        "Data": data,
        "RNG": rand::random::<f64>(),
    });
    let room_name = room.socket_room_name();
    drop(world);

    crate::socket_util::emit_within(&socket, room_name, events::CHAT_ROOM_GAME_RESPONSE, &payload);
}

/// Node `ChatRoomCharacterUpdate`: target by socket `ID`, AllowItem gate, ban check.
pub async fn handle_character_update(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }

    let target_id = match data.get("ID").and_then(|v| v.as_str()) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return,
    };
    let Some(appearance) = data.get("Appearance") else {
        return;
    };
    if !appearance.is_array() {
        return;
    }

    let socket_id = socket.id.to_string();
    let (character, source_member, room_name) = {
        let mut world = state.world.write();
        let Some(src) = world.get_by_socket(&socket_id).cloned() else {
            return;
        };
        let Some(ref room_id) = src.chat_room_id else {
            return;
        };
        let Some(room) = world.chat_rooms.get(room_id) else {
            return;
        };
        if room.ban.contains(&src.member_number) {
            return;
        }
        if !room.members.iter().any(|&m| {
            world
                .get_by_member(m)
                .is_some_and(|a| a.socket_id == target_id)
        }) {
            return;
        }
        let Some(target) = world.get_by_socket(&target_id).cloned() else {
            return;
        };
        if !chat_room_get_allow_item(&src, &target) {
            return;
        }

        let room_name = room.socket_room_name();
        let members = room.members.clone();
        let source_member = src.member_number;
        if let Some(t) = world.get_by_socket_mut(&target_id) {
            t.appearance = Some(appearance.clone());
            t.delayed_appearance = Some(appearance.clone());
            if let Some(pose) = data.get("ActivePose") {
                t.active_pose = Some(pose.clone());
            }
            let character = t.to_synced_character_for_room(&members);
            (character, source_member, room_name)
        } else {
            return;
        }
    };

    let payload = json!({
        "Character": character,
        "SourceMemberNumber": source_member,
    });
    crate::socket_util::emit_within(&socket, room_name, events::CHAT_ROOM_SYNC_SINGLE, &payload);
}

pub async fn handle_expression_update(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let group = match data.get("Group").and_then(|v| v.as_str()) {
        Some(g) if !g.is_empty() => g.to_string(),
        _ => return,
    };
    let name_expr = data.get("Name").cloned().unwrap_or(Value::Null);
    let socket_id = socket.id.to_string();
    let (member, room_name) = {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket_mut(&socket_id) else {
            return;
        };
        // Node may also update Appearance array on expression (local only)
        if let Some(app) = data.get("Appearance") {
            if app.as_array().is_some_and(|a| a.len() >= 5) {
                acc.appearance = Some(app.clone());
            }
        }
        let member = acc.member_number;
        let room_id = acc.chat_room_id.clone();
        let room_name =
            room_id.and_then(|id| world.chat_rooms.get(&id).map(|r| r.socket_room_name()));
        (member, room_name)
    };
    if let Some(room_name) = room_name {
        // Node: socket.to → exclude source; only { MemberNumber, Name, Group }
        let payload = json!({
            "MemberNumber": member,
            "Name": name_expr,
            "Group": group,
        });
        crate::socket_util::emit_to_room(
            &socket,
            room_name,
            events::CHAT_ROOM_SYNC_EXPRESSION,
            &payload,
        );
    }
}

pub async fn handle_pose_update(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    // Node rejects array root payload
    if data.is_array() {
        return;
    }
    let socket_id = socket.id.to_string();
    let pose = match data.get("Pose") {
        Some(Value::Array(arr)) => {
            let filtered: Vec<Value> = arr.iter().filter(|p| p.is_string()).cloned().collect();
            Value::Array(filtered)
        }
        Some(Value::String(s)) => json!([s]),
        _ => json!([]),
    };
    let (member, room_name) = {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket_mut(&socket_id) else {
            return;
        };
        acc.active_pose = Some(pose.clone());
        acc.pose = Some(pose.clone());
        let member = acc.member_number;
        let room_id = acc.chat_room_id.clone();
        let room_name =
            room_id.and_then(|id| world.chat_rooms.get(&id).map(|r| r.socket_room_name()));
        (member, room_name)
    };
    if let Some(name) = room_name {
        // Node: socket.to → exclude source; payload Pose is string[]
        let payload = json!({
            "MemberNumber": member,
            "Pose": pose,
        });
        crate::socket_util::emit_to_room(&socket, name, events::CHAT_ROOM_SYNC_POSE, &payload);
    }
}

pub async fn handle_arousal_update(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let socket_id = socket.id.to_string();
    let (member, room_name, emit) = {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket_mut(&socket_id) else {
            return;
        };
        // Node: only if ArousalSettings already exists
        let Some(settings) = acc.arousal_settings.as_mut().and_then(|v| v.as_object_mut()) else {
            return;
        };
        if let Some(v) = data.get("OrgasmTimer") {
            settings.insert("OrgasmTimer".into(), v.clone());
        }
        if let Some(v) = data.get("OrgasmCount") {
            settings.insert("OrgasmCount".into(), v.clone());
        }
        if let Some(v) = data.get("Progress") {
            settings.insert("Progress".into(), v.clone());
        }
        if let Some(v) = data.get("ProgressTimer") {
            settings.insert("ProgressTimer".into(), v.clone());
        }
        let member = acc.member_number;
        let room_id = acc.chat_room_id.clone();
        let room_name =
            room_id.and_then(|id| world.chat_rooms.get(&id).map(|r| r.socket_room_name()));
        (member, room_name, true)
    };
    if emit {
        if let Some(name) = room_name {
            let payload = json!({
                "MemberNumber": member,
                "OrgasmTimer": data.get("OrgasmTimer"),
                "OrgasmCount": data.get("OrgasmCount"),
                "Progress": data.get("Progress"),
                "ProgressTimer": data.get("ProgressTimer"),
            });
            crate::socket_util::emit_to_room(
                &socket,
                name,
                events::CHAT_ROOM_SYNC_AROUSAL,
                &payload,
            );
        }
    }
}

/// Node `ChatRoomCharacterItemUpdate`: ban + AllowItem if target in room; emit excluding source.
pub async fn handle_item_update(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }

    let target_mn = match data.get("Target").and_then(|v| v.as_i64()) {
        Some(n) => n,
        None => return,
    };
    if data.get("Group").and_then(|v| v.as_str()).is_none() {
        return;
    }

    let socket_id = socket.id.to_string();
    let (source_mn, room_name) = {
        let world = state.world.read();
        let Some(acc) = world.get_by_socket(&socket_id) else {
            return;
        };
        let Some(ref room_id) = acc.chat_room_id else {
            return;
        };
        let Some(room) = world.chat_rooms.get(room_id) else {
            return;
        };
        if room.ban.contains(&acc.member_number) {
            return;
        }
        // If target is in room, require AllowItem; if not in room, still broadcast (Node edge case)
        if room.members.contains(&target_mn) {
            if let Some(target) = world.get_by_member(target_mn) {
                if !chat_room_get_allow_item(acc, target) {
                    return;
                }
            }
        }
        (acc.member_number, room.socket_room_name())
    };

    let payload = json!({
        "Source": source_mn,
        "Item": data,
    });
    // Node: socket.to(room) — exclude source
    crate::socket_util::emit_to_room(&socket, room_name, events::CHAT_ROOM_SYNC_ITEM, &payload);
}

pub async fn handle_map_data_update(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let socket_id = socket.id.to_string();
    let (member, room_name) = {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket_mut(&socket_id) else {
            return;
        };
        acc.map_data = Some(data.clone());
        let member = acc.member_number;
        let room_id = acc.chat_room_id.clone();
        let room_name = room_id
            .as_ref()
            .and_then(|id| world.chat_rooms.get(id).map(|r| r.socket_room_name()));
        (member, room_name)
    };

    if let Some(name) = room_name {
        let payload = json!({
            "MemberNumber": member,
            "MapData": data,
        });
        // Node: socket.to → exclude source
        crate::socket_util::emit_to_room(&socket, name, events::CHAT_ROOM_SYNC_MAP_DATA, &payload);
    }
}

pub async fn handle_allow_item(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let target_mn = data
        .get("MemberNumber")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let socket_id = socket.id.to_string();
    let world = state.world.read();
    let Some(src) = world.get_by_socket(&socket_id) else {
        return;
    };
    // Node only checks accounts in the same room
    let allow = if let Some(ref room_id) = src.chat_room_id {
        if let Some(room) = world.chat_rooms.get(room_id) {
            if room.members.contains(&target_mn) {
                world
                    .get_by_member(target_mn)
                    .map(|t| chat_room_get_allow_item(src, t))
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };
    drop(world);
    let _ = socket.emit(
        events::CHAT_ROOM_ALLOW_ITEM,
        &json!({ "MemberNumber": target_mn, "AllowItem": allow }),
    );
}

/// Item permission levels 0–5 (mirrors Node `ChatRoomGetAllowItem`).
pub fn chat_room_get_allow_item(
    source: &crate::state::OnlineAccount,
    target: &crate::state::OnlineAccount,
) -> bool {
    if source.member_number == target.member_number {
        return true;
    }

    // Owner always allowed
    if let Some(ref ownership) = target.ownership {
        if ownership
            .get("MemberNumber")
            .and_then(|v| v.as_i64())
            == Some(source.member_number)
        {
            return true;
        }
    }

    let item_perm = if target.item_permission < 0 {
        2
    } else {
        target.item_permission
    };

    // At zero permission level, allow
    if item_perm <= 0 {
        return true;
    }

    // At one: allow if source not on blacklist
    if item_perm == 1 {
        return !target.black_list.contains(&source.member_number);
    }

    let lovers: Vec<i64> = target
        .lovership
        .iter()
        .filter_map(|l| l.get("MemberNumber").and_then(|v| v.as_i64()))
        .collect();

    // At two: not blacklisted AND (Dom+25 >= Target Dom OR whitelist OR lover)
    if item_perm == 2 {
        if target.black_list.contains(&source.member_number) {
            return false;
        }
        return dominant_value(source) + 25 >= dominant_value(target)
            || target.white_list.contains(&source.member_number)
            || lovers.contains(&source.member_number);
    }

    // At three: whitelist or lover
    if item_perm == 3 {
        return target.white_list.contains(&source.member_number)
            || lovers.contains(&source.member_number);
    }

    // At four: lover only
    if item_perm == 4 {
        return lovers.contains(&source.member_number);
    }

    false
}

fn dominant_value(acc: &crate::state::OnlineAccount) -> i64 {
    acc.reputation
        .as_ref()
        .and_then(|r| r.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|e| {
                if e.get("Type").and_then(|t| t.as_str()) == Some("Dominant") {
                    e.get("Value").and_then(|v| v.as_i64()).or_else(|| {
                        e.get("Value")
                            .and_then(|v| v.as_f64())
                            .map(|f| f as i64)
                    })
                } else {
                    None
                }
            })
        })
        .unwrap_or(0)
}


