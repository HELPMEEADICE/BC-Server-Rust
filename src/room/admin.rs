//! ChatRoomAdmin — full parity with Node `ChatRoomAdmin`.

use serde_json::{json, Value};
use socketioxide::extract::{Data, SocketRef, State};

use crate::protocol::events;
use crate::room::{leave_room_inner_reason, room_message};
use crate::state::OnlineAccount;
use crate::util::{
    is_chat_room_name, CHAT_ROOM_DESCRIPTION_MAX_LENGTH, ROOM_LIMIT_DEFAULT, ROOM_LIMIT_MAXIMUM,
    ROOM_LIMIT_MINIMUM,
};
use crate::AppState;

struct AdminEffects {
    kick: Option<(String, &'static str)>,
    props: Option<(String, Value)>,
    reorder: Option<(String, Value)>,
    action: Option<(String, i64, String, Value)>,
    update_resp: Option<&'static str>,
}

impl AdminEffects {
    fn empty() -> Self {
        Self {
            kick: None,
            props: None,
            reorder: None,
            action: None,
            update_resp: None,
        }
    }
}

pub async fn handle_chat_room_admin(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let action = match data.get("Action").and_then(|v| v.as_str()) {
        Some(a) => a.to_string(),
        None => return,
    };
    let Some(target_mn) = data.get("MemberNumber").and_then(|v| v.as_i64()) else {
        return;
    };

    let socket_id = socket.id.to_string();
    let effects = {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket(&socket_id).cloned() else {
            return;
        };
        let Some(room_id) = acc.chat_room_id.clone() else {
            return;
        };
        {
            let Some(room) = world.chat_rooms.get(&room_id) else {
                return;
            };
            if !room.admin.contains(&acc.member_number) {
                return;
            }
        }

        // Only Swap / MoveLeft / MoveRight allowed on self
        if acc.member_number == target_mn
            && action != "Swap"
            && action != "MoveLeft"
            && action != "MoveRight"
        {
            return;
        }

        if action == "Update" {
            apply_update(&mut world, &acc, &room_id, &data)
        } else if action == "Swap" {
            apply_swap(&mut world, &acc, &room_id, &data)
        } else {
            apply_member_or_offline(&mut world, &socket, &acc, &room_id, &action, target_mn, &data)
        }
    };

    emit_effects(&socket, &state, effects);
}

fn emit_effects(socket: &SocketRef, state: &AppState, e: AdminEffects) {
    if let Some(code) = e.update_resp {
        let _ = socket.emit(events::CHAT_ROOM_UPDATE_RESPONSE, &code);
    }
    // Kick/Ban response to target first (Node order), then action already sent in leave_room_inner_reason
    if let Some((sid, code)) = e.kick {
        if let Some(io) = state.io.get() {
            crate::socket_util::emit_to(io, &sid, events::CHAT_ROOM_SEARCH_RESPONSE, &code);
        }
    }
    if let Some((room_name, order)) = e.reorder {
        let _ = socket
            .within(room_name)
            .emit(events::CHAT_ROOM_SYNC_REORDER_PLAYERS, &order);
    }
    if let Some((room_name, sender, content, dict)) = e.action {
        room_message(
            socket,
            &room_name,
            sender,
            &content,
            "Action",
            None,
            Some(dict),
        );
    }
    if let Some((room_name, payload)) = e.props {
        let _ = socket
            .within(room_name)
            .emit(events::CHAT_ROOM_SYNC_ROOM_PROPERTIES, &payload);
    }
}

fn apply_update(
    world: &mut crate::state::World,
    acc: &OnlineAccount,
    room_id: &str,
    data: &Value,
) -> AdminEffects {
    let room_data = data.get("Room").cloned().unwrap_or_else(|| data.clone());

    let name = room_data
        .get("Name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    let description = room_data
        .get("Description")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let background = room_data
        .get("Background")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let (Some(name), Some(description), Some(background)) = (name, description, background) else {
        return AdminEffects {
            update_resp: Some("InvalidRoomData"),
            ..AdminEffects::empty()
        };
    };

    if !is_chat_room_name(&name)
        || description.len() > CHAT_ROOM_DESCRIPTION_MAX_LENGTH
        || background.len() > 100
    {
        return AdminEffects {
            update_resp: Some("InvalidRoomData"),
            ..AdminEffects::empty()
        };
    }

    let admin = match room_data.get("Admin") {
        Some(Value::Array(arr)) if arr.iter().all(|v| v.as_i64().is_some()) => {
            arr.iter().filter_map(|v| v.as_i64()).collect::<Vec<_>>()
        }
        _ => {
            return AdminEffects {
                update_resp: Some("InvalidRoomData"),
                ..AdminEffects::empty()
            };
        }
    };
    let ban = match room_data.get("Ban") {
        Some(Value::Array(arr)) if arr.iter().all(|v| v.as_i64().is_some()) => {
            arr.iter().filter_map(|v| v.as_i64()).collect::<Vec<_>>()
        }
        _ => {
            return AdminEffects {
                update_resp: Some("InvalidRoomData"),
                ..AdminEffects::empty()
            };
        }
    };

    let old_name = world
        .chat_rooms
        .get(room_id)
        .map(|r| r.name.clone())
        .unwrap_or_default();
    if old_name.to_uppercase() != name.to_uppercase()
        && world.get_room_by_name(&acc.environment, &name).is_some()
    {
        return AdminEffects {
            update_resp: Some("RoomAlreadyExist"),
            ..AdminEffects::empty()
        };
    }

    let mut visibility = room_data.get("Visibility").and_then(|v| {
        v.as_array().map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
    });
    let mut access = room_data.get("Access").and_then(|v| {
        v.as_array().map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
    });
    let mut private = room_data.get("Private").and_then(|v| v.as_bool());
    let mut locked = room_data.get("Locked").and_then(|v| v.as_bool());

    if let Some(ref vis) = visibility {
        private = Some(!vis.iter().any(|v| v == "All"));
    } else if let Some(p) = private {
        visibility = Some(if p {
            vec!["Admin".into()]
        } else {
            vec!["All".into()]
        });
    }
    if let Some(ref accs) = access {
        locked = Some(!accs.iter().any(|v| v == "All"));
    } else if let Some(l) = locked {
        access = Some(if l {
            vec!["Admin".into(), "Whitelist".into()]
        } else {
            vec!["All".into()]
        });
    }

    let Some(room) = world.chat_rooms.get_mut(room_id) else {
        return AdminEffects::empty();
    };

    let old_key = crate::state::World::room_key(&room.environment, &room.name);
    room.name = name.clone();
    room.description = description;
    room.background = background;
    room.admin = admin;
    room.ban = ban;
    if let Some(lang) = room_data.get("Language").and_then(|v| v.as_str()) {
        room.language = lang.to_string();
    }
    if let Some(c) = room_data.get("Custom") {
        room.custom = Some(c.clone());
    }
    if let Some(Value::Array(bc)) = room_data.get("BlockCategory") {
        room.block_category = bc
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect();
    } else {
        room.block_category = vec![];
    }
    if let Some(Value::Array(w)) = room_data.get("Whitelist") {
        room.whitelist = w.iter().filter_map(|v| v.as_i64()).collect();
    }
    room.game = room_data
        .get("Game")
        .and_then(|v| v.as_str())
        .filter(|g| g.len() <= 100)
        .unwrap_or("")
        .to_string();

    let limit = {
        let lim = room_data.get("Limit");
        let n = lim
            .and_then(|v| v.as_i64())
            .or_else(|| lim.and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
            .unwrap_or(ROOM_LIMIT_DEFAULT as i64) as i32;
        if n < ROOM_LIMIT_MINIMUM || n > ROOM_LIMIT_MAXIMUM {
            ROOM_LIMIT_DEFAULT
        } else {
            n
        }
    };
    room.limit = limit;
    if let Some(v) = visibility {
        room.visibility = v;
    }
    if let Some(a) = access {
        room.access = a;
    }
    if let Some(p) = private {
        room.private = p;
    }
    if let Some(l) = locked {
        room.locked = l;
    }
    if let Some(m) = room_data.get("MapData") {
        room.map_data = if m.is_null() {
            None
        } else {
            Some(m.clone())
        };
    }

    let new_key = crate::state::World::room_key(&room.environment, &room.name);
    if old_key != new_key {
        world.room_by_name_env.remove(&old_key);
        world.room_by_name_env.insert(new_key, room_id.to_string());
    }

    let room = world.chat_rooms.get(room_id).unwrap();
    let room_name = room.socket_room_name();
    let mut props = room.to_properties_json();
    if let Some(obj) = props.as_object_mut() {
        obj.insert("SourceMemberNumber".into(), json!(acc.member_number));
    }
    let privacy = if room.visibility.iter().any(|v| v == "All") {
        "Public"
    } else {
        "Private"
    };
    let lock_label = if room.access.iter().any(|v| v == "All") {
        "Unlocked"
    } else {
        "Locked"
    };
    let dict = json!([
        { "Tag": "SourceCharacter", "Text": acc.name, "MemberNumber": acc.member_number },
        { "Tag": "ChatRoomName", "Text": room.name },
        { "Tag": "ChatRoomLimit", "Text": room.limit.to_string() },
        { "Tag": "ChatRoomPrivacy", "TextToLookUp": privacy },
        { "Tag": "ChatRoomLocked", "TextToLookUp": lock_label },
    ]);

    AdminEffects {
        update_resp: Some("Updated"),
        props: Some((room_name.clone(), props)),
        action: Some((
            room_name,
            acc.member_number,
            "ServerUpdateRoom".into(),
            dict,
        )),
        ..AdminEffects::empty()
    }
}

fn apply_swap(
    world: &mut crate::state::World,
    acc: &OnlineAccount,
    room_id: &str,
    data: &Value,
) -> AdminEffects {
    let target = data.get("TargetMemberNumber").and_then(|v| v.as_i64());
    let dest = data.get("DestinationMemberNumber").and_then(|v| v.as_i64());
    let (Some(t), Some(d)) = (target, dest) else {
        return AdminEffects::empty();
    };
    if t == d {
        return AdminEffects::empty();
    }
    let Some(room) = world.chat_rooms.get_mut(room_id) else {
        return AdminEffects::empty();
    };
    let ti = room.members.iter().position(|&m| m == t);
    let di = room.members.iter().position(|&m| m == d);
    let (Some(ti), Some(di)) = (ti, di) else {
        return AdminEffects::empty();
    };
    room.members.swap(ti, di);
    let order = room.members.clone();
    let room_name = room.socket_room_name();
    let dict = json!([
        { "SourceCharacter": acc.member_number },
        { "TargetCharacter": t },
        { "TargetCharacter": d, "Index": 1 },
    ]);
    AdminEffects {
        reorder: Some((room_name.clone(), json!({ "PlayerOrder": order }))),
        action: Some((room_name, acc.member_number, "ServerSwap".into(), dict)),
        ..AdminEffects::empty()
    }
}

fn apply_member_or_offline(
    world: &mut crate::state::World,
    socket: &SocketRef,
    acc: &OnlineAccount,
    room_id: &str,
    action: &str,
    target_mn: i64,
    data: &Value,
) -> AdminEffects {
    let idx = world
        .chat_rooms
        .get(room_id)
        .and_then(|r| r.members.iter().position(|&m| m == target_mn));

    if let Some(a) = idx {
        let target_name = world
            .get_by_member(target_mn)
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let target_sid = world.get_by_member(target_mn).map(|t| t.socket_id.clone());

        match action {
            "Ban" => {
                // Node: Ban.push → RoomBanned → ChatRoomRemove(ServerBan) → SyncRoomProperties
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    if !r.ban.contains(&target_mn) {
                        r.ban.push(target_mn);
                    }
                }
                let dict = source_target_dict(acc, target_mn, &target_name);
                leave_room_inner_reason(
                    world,
                    socket,
                    room_id,
                    target_mn,
                    Some("ServerBan"),
                    Some(dict),
                );
                let mut e = props_effect(world, room_id, acc.member_number);
                e.kick = target_sid.map(|sid| (sid, "RoomBanned"));
                e
            }
            "Kick" => {
                // Node: RoomKicked → ChatRoomRemove(ServerKick) — no room props sync
                let dict = source_target_dict(acc, target_mn, &target_name);
                leave_room_inner_reason(
                    world,
                    socket,
                    room_id,
                    target_mn,
                    Some("ServerKick"),
                    Some(dict),
                );
                let mut e = AdminEffects::empty();
                e.kick = target_sid.map(|sid| (sid, "RoomKicked"));
                e
            }
            "MoveLeft" if a > 0 => {
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    r.members.swap(a, a - 1);
                }
                move_effect(world, room_id, acc, target_mn, &target_name, "ServerMoveLeft", data)
            }
            "MoveRight" => {
                let len = world
                    .chat_rooms
                    .get(room_id)
                    .map(|r| r.members.len())
                    .unwrap_or(0);
                if a + 1 < len {
                    if let Some(r) = world.chat_rooms.get_mut(room_id) {
                        r.members.swap(a, a + 1);
                    }
                    move_effect(
                        world,
                        room_id,
                        acc,
                        target_mn,
                        &target_name,
                        "ServerMoveRight",
                        data,
                    )
                } else {
                    AdminEffects::empty()
                }
            }
            "Shuffle" => {
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    use rand::seq::SliceRandom;
                    r.members.shuffle(&mut rand::thread_rng());
                }
                let Some(room) = world.chat_rooms.get(room_id) else {
                    return AdminEffects::empty();
                };
                AdminEffects {
                    reorder: Some((
                        room.socket_room_name(),
                        json!({ "PlayerOrder": room.members }),
                    )),
                    action: Some((
                        room.socket_room_name(),
                        acc.member_number,
                        "ServerShuffle".into(),
                        json!([{ "Tag": "SourceCharacter", "Text": acc.name, "MemberNumber": acc.member_number }]),
                    )),
                    ..AdminEffects::empty()
                }
            }
            "Promote" => {
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    if !r.admin.contains(&target_mn) {
                        r.admin.push(target_mn);
                    }
                }
                promote_demote_effect(
                    world,
                    room_id,
                    acc,
                    target_mn,
                    &target_name,
                    "ServerPromoteAdmin",
                )
            }
            "Demote" => {
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    r.admin.retain(|&m| m != target_mn);
                }
                promote_demote_effect(
                    world,
                    room_id,
                    acc,
                    target_mn,
                    &target_name,
                    "ServerDemoteAdmin",
                )
            }
            "Whitelist" => {
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    if !r.whitelist.contains(&target_mn) {
                        r.whitelist.push(target_mn);
                    }
                }
                promote_demote_effect(
                    world,
                    room_id,
                    acc,
                    target_mn,
                    &target_name,
                    "ServerRoomWhitelist",
                )
            }
            "Unwhitelist" => {
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    r.whitelist.retain(|&m| m != target_mn);
                }
                promote_demote_effect(
                    world,
                    room_id,
                    acc,
                    target_mn,
                    &target_name,
                    "ServerRoomUnwhitelist",
                )
            }
            _ => AdminEffects::empty(),
        }
    } else {
        // Not in room: Ban / Unban / Whitelist / Unwhitelist silent
        match action {
            "Ban" => {
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    if !r.ban.contains(&target_mn) {
                        r.ban.push(target_mn);
                    }
                }
                props_effect(world, room_id, acc.member_number)
            }
            "Unban" => {
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    r.ban.retain(|&m| m != target_mn);
                }
                props_effect(world, room_id, acc.member_number)
            }
            "Whitelist" => {
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    if !r.whitelist.contains(&target_mn) {
                        r.whitelist.push(target_mn);
                    }
                }
                props_effect(world, room_id, acc.member_number)
            }
            "Unwhitelist" => {
                if let Some(r) = world.chat_rooms.get_mut(room_id) {
                    r.whitelist.retain(|&m| m != target_mn);
                }
                props_effect(world, room_id, acc.member_number)
            }
            _ => AdminEffects::empty(),
        }
    }
}

fn move_effect(
    world: &crate::state::World,
    room_id: &str,
    acc: &OnlineAccount,
    target_mn: i64,
    target_name: &str,
    content: &str,
    data: &Value,
) -> AdminEffects {
    let Some(room) = world.chat_rooms.get(room_id) else {
        return AdminEffects::empty();
    };
    let publish = data.get("Publish").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut e = AdminEffects {
        reorder: Some((
            room.socket_room_name(),
            json!({ "PlayerOrder": room.members }),
        )),
        ..AdminEffects::empty()
    };
    if publish {
        e.action = Some((
            room.socket_room_name(),
            acc.member_number,
            content.into(),
            source_target_dict(acc, target_mn, target_name),
        ));
    }
    e
}

fn promote_demote_effect(
    world: &crate::state::World,
    room_id: &str,
    acc: &OnlineAccount,
    target_mn: i64,
    target_name: &str,
    content: &str,
) -> AdminEffects {
    let Some(room) = world.chat_rooms.get(room_id) else {
        return AdminEffects::empty();
    };
    let mut props = room.to_properties_json();
    if let Some(obj) = props.as_object_mut() {
        obj.insert("SourceMemberNumber".into(), json!(acc.member_number));
    }
    AdminEffects {
        props: Some((room.socket_room_name(), props)),
        action: Some((
            room.socket_room_name(),
            acc.member_number,
            content.into(),
            source_target_dict(acc, target_mn, target_name),
        )),
        ..AdminEffects::empty()
    }
}

fn props_effect(world: &crate::state::World, room_id: &str, source: i64) -> AdminEffects {
    let Some(room) = world.chat_rooms.get(room_id) else {
        return AdminEffects::empty();
    };
    let mut props = room.to_properties_json();
    if let Some(obj) = props.as_object_mut() {
        obj.insert("SourceMemberNumber".into(), json!(source));
    }
    AdminEffects {
        props: Some((room.socket_room_name(), props)),
        ..AdminEffects::empty()
    }
}

fn source_target_dict(acc: &OnlineAccount, target_mn: i64, target_name: &str) -> Value {
    json!([
        { "Tag": "SourceCharacter", "Text": acc.name, "MemberNumber": acc.member_number },
        { "Tag": "TargetCharacterName", "Text": target_name, "MemberNumber": target_mn },
    ])
}
