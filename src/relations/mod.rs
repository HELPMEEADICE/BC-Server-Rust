//! Ownership & Lovership state machines — full parity with Node.

use serde_json::{json, Value};
use socketioxide::extract::{Data, SocketRef, State};
use tracing::error;

use crate::db::json_object_to_set_map;
use crate::protocol::events;
use crate::room::{room_message, sync_character};
use crate::util::{common_time, LOVERSHIP_DELAY_MS, OWNERSHIP_DELAY_MS};
use crate::AppState;

// ---------------------------------------------------------------------------
// Ownership (4-step trial → collar)
// ---------------------------------------------------------------------------

pub async fn handle_account_ownership(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let Some(target_mn) = data.get("MemberNumber").and_then(|v| v.as_i64()) else {
        return;
    };
    let action = data
        .get("Action")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let notes = data
        .get("Notes")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let socket_id = socket.id.to_string();

    // --- Break (submissive self-release) ---
    if action.as_deref() == Some("Break") {
        let clear = {
            let mut world = state.world.write();
            let Some(acc) = world.get_by_socket_mut(&socket_id) else {
                return;
            };
            let Some(ref o) = acc.ownership else {
                return;
            };
            let stage = o.get("Stage").and_then(|v| v.as_i64());
            let start = o.get("Start").and_then(|v| v.as_i64());
            let (Some(stage), Some(start)) = (stage, start) else {
                return;
            };
            // Stage 0 always, or after delay
            if stage != 0 && start + OWNERSHIP_DELAY_MS > common_time() {
                return;
            }
            // Extreme (level 3) cannot break full ownership (stage 1)
            let diff = acc
                .difficulty
                .as_ref()
                .and_then(|d| d.get("Level"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if diff > 2 && stage != 0 {
                return;
            }
            acc.owner.clear();
            acc.ownership = None;
            acc.account_name.clone()
        };
        clear_ownership_db(&state, &clear).await;
        let _ = socket.emit(events::ACCOUNT_OWNERSHIP, &json!({ "ClearOwnership": true }));
        return;
    }

    // Snapshot room context (no await while holding lock)
    let room_ctx = {
        let world = state.world.read();
        let Some(acc) = world.get_by_socket(&socket_id) else {
            return;
        };
        match &acc.chat_room_id {
            None => None,
            Some(rid) => world.chat_rooms.get(rid).map(|room| {
                (
                    acc.member_number,
                    acc.name.clone(),
                    acc.account_name.clone(),
                    rid.clone(),
                    room.socket_room_name(),
                    room.members.clone(),
                )
            }),
        }
    };

    // Node requires the owner to be in a chat room, including for an offline release.
    let Some((src_member, src_name, src_account, room_id, room_socket_name, room_members)) =
        room_ctx
    else {
        return;
    };

    // --- Offline Release (target not in room) ---
    if action.as_deref() == Some("Release") && !room_members.contains(&target_mn) {
        release_offline(&socket, &state, &socket_id, target_mn).await;
        return;
    }

    if !room_members.contains(&target_mn) {
        return;
    }

    // Snapshot target
    let target_snap = {
        let world = state.world.read();
        world.get_by_member(target_mn).cloned()
    };
    let Some(target) = target_snap else {
        return;
    };

    // --- UpdateNotes (dominant on fully owned sub) ---
    if action.as_deref() == Some("UpdateNotes") {
        let stage = target
            .ownership
            .as_ref()
            .and_then(|o| o.get("Stage"))
            .and_then(|v| v.as_i64());
        let owner_mn = target
            .ownership
            .as_ref()
            .and_then(|o| o.get("MemberNumber"))
            .and_then(|v| v.as_i64());
        if stage != Some(1) || owner_mn != Some(src_member) {
            return;
        }
        let mut ownership = target.ownership.clone().unwrap_or(json!({}));
        if let Some(obj) = ownership.as_object_mut() {
            if let Some(ref n) = notes {
                if !n.is_empty() {
                    let truncated: String = n.chars().take(4000).collect();
                    obj.insert("Notes".into(), json!(truncated));
                } else {
                    obj.remove("Notes");
                }
            } else {
                obj.remove("Notes");
            }
        }
        let owner_str = target.owner.clone();
        {
            let mut world = state.world.write();
            if let Some(t) = world.get_by_socket_mut(&target.socket_id) {
                t.ownership = Some(ownership.clone());
            }
        }
        let set = json!({ "Ownership": ownership, "Owner": owner_str });
        if let Ok(doc) = json_object_to_set_map(&set) {
            let _ = state.db.update_fields(&target.account_name, doc).await;
        }
        if let Some(io) = state.io.get() {
            crate::socket_util::emit_to(
                io,
                &target.socket_id,
                events::ACCOUNT_OWNERSHIP,
                &set,
            );
        }
        sync_character(&socket, &state, target_mn, target_mn);
        return;
    }

    // --- Release (dominant releases sub in room) ---
    if action.as_deref() == Some("Release") {
        let owner_mn = target
            .ownership
            .as_ref()
            .and_then(|o| o.get("MemberNumber"))
            .and_then(|v| v.as_i64());
        if owner_mn != Some(src_member) {
            return;
        }
        let is_trial = target
            .ownership
            .as_ref()
            .and_then(|o| o.get("Stage"))
            .and_then(|v| v.as_i64())
            .map(|s| s == 0)
            .unwrap_or(true);
        {
            let mut world = state.world.write();
            if let Some(t) = world.get_by_socket_mut(&target.socket_id) {
                t.owner.clear();
                t.ownership = None;
            }
        }
        clear_ownership_db(&state, &target.account_name).await;
        if let Some(io) = state.io.get() {
            crate::socket_util::emit_to(
                io,
                &target.socket_id,
                events::ACCOUNT_OWNERSHIP,
                &json!({ "ClearOwnership": true }),
            );
        }
        let content = if is_trial {
            "EndOwnershipTrial"
        } else {
            "EndOwnership"
        };
        room_message(
            &socket,
            &state,
            &room_socket_name,
            src_member,
            content,
            "ServerMessage",
            None,
            Some(json!([
                { "Tag": "SourceCharacter", "Text": src_name, "MemberNumber": src_member },
                { "Tag": "TargetCharacter", "Text": target.name, "MemberNumber": target_mn },
            ])),
        );
        sync_character(&socket, &state, target_mn, target_mn);
        return;
    }

    // Blacklist check helper
    let blacklisted = |bl: &[i64], mn: i64| bl.contains(&mn);

    // --- Dominant proposes (not owned by target) ---
    let src_owned_by_target = {
        let world = state.world.read();
        world
            .get_by_socket(&socket_id)
            .and_then(|a| a.ownership.as_ref())
            .and_then(|o| o.get("MemberNumber"))
            .and_then(|v| v.as_i64())
            == Some(target_mn)
    };

    if !src_owned_by_target {
        if !blacklisted(&target.black_list, src_member) && target.owner.is_empty() {
            // Step 1: propose start trial
            let target_has_owner = target
                .ownership
                .as_ref()
                .and_then(|o| o.get("MemberNumber"))
                .is_some();
            if !target_has_owner {
                if src_member == target_mn {
                    return;
                }
                if action.as_deref() == Some("Propose") {
                    {
                        let mut world = state.world.write();
                        if let Some(t) = world.get_by_socket_mut(&target.socket_id) {
                            t.owner.clear();
                            t.ownership =
                                Some(json!({ "StartTrialOfferedByMemberNumber": src_member }));
                        }
                    }
                    room_message(
                        &socket,
                        &state,
                        &room_socket_name,
                        src_member,
                        "OfferStartTrial",
                        "ServerMessage",
                        Some(target_mn),
                        Some(json!([
                            { "Tag": "SourceCharacter", "Text": src_name, "MemberNumber": src_member }
                        ])),
                    );
                } else {
                    let _ = socket.emit(
                        events::ACCOUNT_OWNERSHIP,
                        &json!({ "MemberNumber": target_mn, "Result": "CanOfferStartTrial" }),
                    );
                }
            }

            // Step 3: offer end trial after delay
            let can_end = target
                .ownership
                .as_ref()
                .map(|o| {
                    o.get("MemberNumber").and_then(|v| v.as_i64()) == Some(src_member)
                        && o.get("EndTrialOfferedByMemberNumber").is_none()
                        && o.get("Stage").and_then(|v| v.as_i64()) == Some(0)
                        && o
                            .get("Start")
                            .and_then(|v| v.as_i64())
                            .is_some_and(|s| s + OWNERSHIP_DELAY_MS <= common_time())
                })
                .unwrap_or(false);
            if can_end {
                if action.as_deref() == Some("Propose") {
                    {
                        let mut world = state.world.write();
                        if let Some(t) = world.get_by_socket_mut(&target.socket_id) {
                            if let Some(ref mut o) = t.ownership {
                                if let Some(obj) = o.as_object_mut() {
                                    obj.insert(
                                        "EndTrialOfferedByMemberNumber".into(),
                                        json!(src_member),
                                    );
                                }
                            }
                        }
                    }
                    room_message(
                        &socket,
                        &state,
                        &room_socket_name,
                        src_member,
                        "OfferEndTrial",
                        "ServerMessage",
                        None,
                        Some(json!([
                            { "Tag": "SourceCharacter", "Text": src_name, "MemberNumber": src_member }
                        ])),
                    );
                } else {
                    let _ = socket.emit(
                        events::ACCOUNT_OWNERSHIP,
                        &json!({ "MemberNumber": target_mn, "Result": "CanOfferEndTrial" }),
                    );
                }
            }
        }
    }

    // --- Submissive accepts ---
    let src_ownership = {
        let world = state.world.read();
        world
            .get_by_socket(&socket_id)
            .and_then(|a| a.ownership.clone())
    };

    let can_interact = match &src_ownership {
        None => true,
        Some(o) => {
            let mn = o.get("MemberNumber").and_then(|v| v.as_i64());
            mn.is_none() || mn == Some(target_mn)
        }
    };

    if can_interact && !blacklisted(&target.black_list, src_member) {
        // Step 2: accept start trial
        if let Some(ref o) = src_ownership {
            if o.get("StartTrialOfferedByMemberNumber")
                .and_then(|v| v.as_i64())
                == Some(target_mn)
            {
                if action.as_deref() == Some("Accept") {
                    let ownership = json!({
                        "MemberNumber": target_mn,
                        "Name": target.name,
                        "Start": common_time(),
                        "Stage": 0,
                    });
                    {
                        let mut world = state.world.write();
                        if let Some(a) = world.get_by_socket_mut(&socket_id) {
                            a.owner.clear();
                            a.ownership = Some(ownership.clone());
                        }
                    }
                    let set = json!({ "Ownership": ownership, "Owner": "" });
                    if let Ok(doc) = json_object_to_set_map(&set) {
                        let _ = state.db.update_fields(&src_account, doc).await;
                    }
                    let _ = socket.emit(events::ACCOUNT_OWNERSHIP, &set);
                    room_message(
                        &socket,
                        &state,
                        &room_socket_name,
                        src_member,
                        "StartTrial",
                        "ServerMessage",
                        None,
                        Some(json!([
                            { "Tag": "SourceCharacter", "Text": src_name, "MemberNumber": src_member }
                        ])),
                    );
                    sync_character(&socket, &state, src_member, src_member);
                } else {
                    let _ = socket.emit(
                        events::ACCOUNT_OWNERSHIP,
                        &json!({ "MemberNumber": target_mn, "Result": "CanStartTrial" }),
                    );
                }
            }

            // Step 4: accept full collar
            if o.get("Stage").and_then(|v| v.as_i64()) == Some(0)
                && o.get("EndTrialOfferedByMemberNumber")
                    .and_then(|v| v.as_i64())
                    == Some(target_mn)
            {
                if action.as_deref() == Some("Accept") {
                    let ownership = json!({
                        "MemberNumber": target_mn,
                        "Name": target.name,
                        "Start": common_time(),
                        "Stage": 1,
                    });
                    {
                        let mut world = state.world.write();
                        if let Some(a) = world.get_by_socket_mut(&socket_id) {
                            a.owner = target.name.clone();
                            a.ownership = Some(ownership.clone());
                        }
                    }
                    let set = json!({ "Ownership": ownership, "Owner": target.name });
                    if let Ok(doc) = json_object_to_set_map(&set) {
                        let _ = state.db.update_fields(&src_account, doc).await;
                    }
                    let _ = socket.emit(events::ACCOUNT_OWNERSHIP, &set);
                    room_message(
                        &socket,
                        &state,
                        &room_socket_name,
                        src_member,
                        "EndTrial",
                        "ServerMessage",
                        None,
                        Some(json!([
                            { "Tag": "SourceCharacter", "Text": src_name, "MemberNumber": src_member }
                        ])),
                    );
                    sync_character(&socket, &state, src_member, src_member);
                } else {
                    let _ = socket.emit(
                        events::ACCOUNT_OWNERSHIP,
                        &json!({ "MemberNumber": target_mn, "Result": "CanEndTrial" }),
                    );
                }
            }
        }
    }

    let _ = room_id;
}

async fn release_offline(socket: &SocketRef, state: &AppState, socket_id: &str, target_mn: i64) {
    let src_member = {
        let world = state.world.read();
        match world.get_by_socket(socket_id) {
            Some(a) => a.member_number,
            None => return,
        }
    };

    // Load target from DB
    let doc = match state.db.find_by_member_number(target_mn).await {
        Ok(Some(d)) => d,
        _ => {
            emit_release_fail(socket, state, socket_id, src_member);
            return;
        }
    };

    let owner_mn = doc
        .get("Ownership")
        .and_then(|o| o.get("MemberNumber"))
        .and_then(|v| v.as_i64());
    if owner_mn != Some(src_member) {
        // ReleaseFail
        emit_release_fail(socket, state, socket_id, src_member);
        return;
    }

    let account_name = doc
        .get("AccountName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let _ = state.db.clear_ownership(&account_name).await;

    // Online target?
    let (online_target, target_room_name) = {
        let mut world = state.world.write();
        if let Some(sid) = world.socket_by_member.get(&target_mn).cloned() {
            if let Some(t) = world.get_by_socket_mut(&sid) {
                t.owner.clear();
                t.ownership = None;
            }
            if let Some(io) = state.io.get() {
                crate::socket_util::emit_to(
                    io,
                    &sid,
                    events::ACCOUNT_OWNERSHIP,
                    &json!({ "ClearOwnership": true }),
                );
            }
            let target_room_name = world
                .get_by_socket(&sid)
                .and_then(|target| target.chat_room_id.as_ref())
                .and_then(|room_id| world.chat_rooms.get(room_id))
                .map(|room| room.socket_room_name());
            (true, target_room_name)
        } else {
            (false, None)
        }
    };
    if online_target {
        // Node uses the changed account as the sync source, excluding that account.
        sync_character(socket, state, target_mn, target_mn);
        // When the released submissive is in another room, Node also tells that
        // client about the release through their current room channel.
        if let Some(room_name) = target_room_name {
            room_message(
                socket,
                state,
                &room_name,
                target_mn,
                "ReleaseByOwner",
                "ServerMessage",
                Some(target_mn),
                None,
            );
        }
    }

    if let Some(room_name) = {
        let world = state.world.read();
        world
            .get_by_socket(socket_id)
            .and_then(|a| a.chat_room_id.as_ref())
            .and_then(|id| world.chat_rooms.get(id).map(|r| r.socket_room_name()))
    } {
        room_message(
            socket,
            &state,
            &room_name,
            src_member,
            "ReleaseSuccess",
            "ServerMessage",
            Some(src_member),
            None,
        );
    }
}

fn emit_release_fail(socket: &SocketRef, state: &AppState, socket_id: &str, src_member: i64) {
    if let Some(room_name) = {
        let world = state.world.read();
        world
            .get_by_socket(socket_id)
            .and_then(|a| a.chat_room_id.as_ref())
            .and_then(|id| world.chat_rooms.get(id).map(|r| r.socket_room_name()))
    } {
        room_message(socket, state, &room_name, src_member, "ReleaseFail", "ServerMessage", Some(src_member), None);
    }
}

async fn clear_ownership_db(state: &AppState, account_name: &str) {
    if let Err(e) = state.db.clear_ownership(account_name).await {
        error!(error = %e, "clear ownership failed");
    }
}

// ---------------------------------------------------------------------------
// Lovership (6-step dating → engagement → wedding)
// ---------------------------------------------------------------------------

pub async fn handle_account_lovership(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let Some(target_mn) = data.get("MemberNumber").and_then(|v| v.as_i64()) else {
        return;
    };
    let action = data
        .get("Action")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let npc_name = data
        .get("Name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let socket_id = socket.id.to_string();

    // --- Break ---
    if action.as_deref() == Some("Break") {
        handle_lover_break(&socket, &state, &socket_id, target_mn, npc_name).await;
        return;
    }

    // Room required for propose/accept
    let (src, room_socket_name, room_members) = {
        let world = state.world.read();
        let Some(acc) = world.get_by_socket(&socket_id).cloned() else {
            return;
        };
        let Some(ref rid) = acc.chat_room_id else {
            return;
        };
        let Some(room) = world.chat_rooms.get(rid) else {
            return;
        };
        (acc, room.socket_room_name(), room.members.clone())
    };

    if !room_members.contains(&target_mn) {
        return;
    }
    if src.member_number == target_mn {
        return;
    }

    let target = {
        let world = state.world.read();
        world.get_by_member(target_mn).cloned()
    };
    let Some(target) = target else {
        return;
    };
    if target.black_list.contains(&src.member_number) {
        return;
    }

    let al = lover_index(&src.lovership, target_mn);
    let tl = lover_index(&target.lovership, src.member_number);

    // --- Propose paths (dominant-like: either not lovers yet or already) ---
    if (src.lovership.len() < 5 && al.is_none()) || al.is_some() {
        // Step 1: offer begin dating
        if target.lovership.len() < 5 && tl.is_none() {
            if action.as_deref() == Some("Propose") {
                {
                    let mut world = state.world.write();
                    if let Some(t) = world.get_by_socket_mut(&target.socket_id) {
                        t.lovership
                            .push(json!({ "BeginDatingOfferedByMemberNumber": src.member_number }));
                    }
                }
                room_message(
                    &socket,
                    &state,
                    &room_socket_name,
                    src.member_number,
                    "OfferBeginDating",
                    "ServerMessage",
                    Some(target_mn),
                    Some(json!([
                        { "Tag": "SourceCharacter", "Text": src.name, "MemberNumber": src.member_number }
                    ])),
                );
            } else {
                let _ = socket.emit(
                    events::ACCOUNT_LOVERSHIP,
                    &json!({ "MemberNumber": target_mn, "Result": "CanOfferBeginDating" }),
                );
            }
        }

        // Step 3: offer engagement after delay
        if let Some(ti) = tl {
            let entry = &target.lovership[ti];
            if entry.get("BeginEngagementOfferedByMemberNumber").is_none()
                && entry.get("Stage").and_then(|v| v.as_i64()) == Some(0)
                && entry
                    .get("Start")
                    .and_then(|v| v.as_i64())
                    .is_some_and(|s| s + LOVERSHIP_DELAY_MS <= common_time())
            {
                if action.as_deref() == Some("Propose") {
                    {
                        let mut world = state.world.write();
                        if let Some(t) = world.get_by_socket_mut(&target.socket_id) {
                            if let Some(e) = t.lovership.get_mut(ti) {
                                if let Some(obj) = e.as_object_mut() {
                                    obj.insert(
                                        "BeginEngagementOfferedByMemberNumber".into(),
                                        json!(src.member_number),
                                    );
                                }
                            }
                        }
                    }
                    room_message(
                        &socket,
                        &state,
                        &room_socket_name,
                        src.member_number,
                        "OfferBeginEngagement",
                        "ServerMessage",
                        Some(target_mn),
                        Some(json!([
                            { "Tag": "SourceCharacter", "Text": src.name, "MemberNumber": src.member_number }
                        ])),
                    );
                } else {
                    let _ = socket.emit(
                        events::ACCOUNT_LOVERSHIP,
                        &json!({ "MemberNumber": target_mn, "Result": "CanOfferBeginEngagement" }),
                    );
                }
            }

            // Step 5: offer wedding
            if entry.get("BeginWeddingOfferedByMemberNumber").is_none()
                && entry.get("Stage").and_then(|v| v.as_i64()) == Some(1)
                && entry
                    .get("Start")
                    .and_then(|v| v.as_i64())
                    .is_some_and(|s| s + LOVERSHIP_DELAY_MS <= common_time())
            {
                if action.as_deref() == Some("Propose") {
                    {
                        let mut world = state.world.write();
                        if let Some(t) = world.get_by_socket_mut(&target.socket_id) {
                            if let Some(e) = t.lovership.get_mut(ti) {
                                if let Some(obj) = e.as_object_mut() {
                                    obj.insert(
                                        "BeginWeddingOfferedByMemberNumber".into(),
                                        json!(src.member_number),
                                    );
                                }
                            }
                        }
                    }
                    room_message(
                        &socket,
                        &state,
                        &room_socket_name,
                        src.member_number,
                        "OfferBeginWedding",
                        "ServerMessage",
                        Some(target_mn),
                        Some(json!([
                            { "Tag": "SourceCharacter", "Text": src.name, "MemberNumber": src.member_number }
                        ])),
                    );
                } else {
                    let _ = socket.emit(
                        events::ACCOUNT_LOVERSHIP,
                        &json!({ "MemberNumber": target_mn, "Result": "CanOfferBeginWedding" }),
                    );
                }
            }
        }
    }

    // --- Accept paths ---
    let Some(ai) = al else {
        return;
    };
    if src.lovership.len() > 5 {
        return;
    }

    // Re-read src after possible mutations
    let src = {
        let world = state.world.read();
        world.get_by_socket(&socket_id).cloned()
    };
    let Some(src) = src else {
        return;
    };
    let Some(entry) = src.lovership.get(ai).cloned() else {
        return;
    };

    // Step 2: accept dating
    if entry
        .get("BeginDatingOfferedByMemberNumber")
        .and_then(|v| v.as_i64())
        == Some(target_mn)
    {
        let target = {
            let world = state.world.read();
            world.get_by_member(target_mn).cloned()
        };
        let Some(target) = target else {
            return;
        };
        let tl = lover_index(&target.lovership, src.member_number);
        if target.lovership.len() < 5 || tl.is_some() {
            if action.as_deref() == Some("Accept") {
                let now = common_time();
                let src_entry =
                    json!({ "MemberNumber": target_mn, "Name": target.name, "Start": now, "Stage": 0 });
                let tgt_entry =
                    json!({ "MemberNumber": src.member_number, "Name": src.name, "Start": now, "Stage": 0 });
                apply_mutual_lover(
                    &socket,
                    &state,
                    &src,
                    &target,
                    ai,
                    tl,
                    src_entry,
                    tgt_entry,
                    "BeginDating",
                    &room_socket_name,
                )
                .await;
            } else {
                let _ = socket.emit(
                    events::ACCOUNT_LOVERSHIP,
                    &json!({ "MemberNumber": target_mn, "Result": "CanBeginDating" }),
                );
            }
        }
    }

    // Step 4: accept engagement
    if entry.get("Stage").and_then(|v| v.as_i64()) == Some(0)
        && entry
            .get("BeginEngagementOfferedByMemberNumber")
            .and_then(|v| v.as_i64())
            == Some(target_mn)
    {
        if action.as_deref() == Some("Accept") {
            let target = {
                let world = state.world.read();
                world.get_by_member(target_mn).cloned()
            };
            let Some(target) = target else {
                return;
            };
            let tl = lover_index(&target.lovership, src.member_number);
            let now = common_time();
            let src_entry =
                json!({ "MemberNumber": target_mn, "Name": target.name, "Start": now, "Stage": 1 });
            let tgt_entry =
                json!({ "MemberNumber": src.member_number, "Name": src.name, "Start": now, "Stage": 1 });
            apply_mutual_lover(
                &socket,
                &state,
                &src,
                &target,
                ai,
                tl,
                src_entry,
                tgt_entry,
                "BeginEngagement",
                &room_socket_name,
            )
            .await;
        } else {
            let _ = socket.emit(
                events::ACCOUNT_LOVERSHIP,
                &json!({ "MemberNumber": target_mn, "Result": "CanBeginEngagement" }),
            );
        }
    }

    // Step 6: accept wedding
    if entry.get("Stage").and_then(|v| v.as_i64()) == Some(1)
        && entry
            .get("BeginWeddingOfferedByMemberNumber")
            .and_then(|v| v.as_i64())
            == Some(target_mn)
    {
        if action.as_deref() == Some("Accept") {
            let target = {
                let world = state.world.read();
                world.get_by_member(target_mn).cloned()
            };
            let Some(target) = target else {
                return;
            };
            let tl = lover_index(&target.lovership, src.member_number);
            let now = common_time();
            let src_entry =
                json!({ "MemberNumber": target_mn, "Name": target.name, "Start": now, "Stage": 2 });
            let tgt_entry =
                json!({ "MemberNumber": src.member_number, "Name": src.name, "Start": now, "Stage": 2 });
            apply_mutual_lover(
                &socket,
                &state,
                &src,
                &target,
                ai,
                tl,
                src_entry,
                tgt_entry,
                "BeginWedding",
                &room_socket_name,
            )
            .await;
        } else {
            let _ = socket.emit(
                events::ACCOUNT_LOVERSHIP,
                &json!({ "MemberNumber": target_mn, "Result": "CanBeginWedding" }),
            );
        }
    }
}

async fn handle_lover_break(
    socket: &SocketRef,
    state: &AppState,
    socket_id: &str,
    target_mn: i64,
    npc_name: Option<String>,
) {
    // NPC break
    if target_mn < 0 {
        if let Some(name) = npc_name {
            let list = {
                let mut world = state.world.write();
                let Some(acc) = world.get_by_socket_mut(socket_id) else {
                    return;
                };
                // Node uses findIndex followed by splice(index, 1): it removes only the
                // first match and, when absent, its splice(-1, 1) removes the last entry.
                let index = acc
                    .lovership
                    .iter()
                    .position(|lover| lover.get("Name").and_then(|value| value.as_str()) == Some(name.as_str()));
                if let Some(index) = index {
                    acc.lovership.remove(index);
                } else {
                    acc.lovership.pop();
                }
                (acc.account_name.clone(), acc.lovership.clone(), acc.member_number)
            };
            persist_lovership_clean(state, list.2, &list.1).await;
            let clean = clean_lovership(&list.1);
            let _ = socket.emit(events::ACCOUNT_LOVERSHIP, &json!({ "Lovership": clean }));
        }
        return;
    }

    let (src_mn, src_account, can_break, new_src_list) = {
        let world = state.world.read();
        let Some(acc) = world.get_by_socket(socket_id) else {
            return;
        };
        let Some(i) = lover_index(&acc.lovership, target_mn) else {
            return;
        };
        let entry = &acc.lovership[i];
        let stage = entry.get("Stage").and_then(|v| v.as_i64());
        let start = entry.get("Start").and_then(|v| v.as_i64());
        let (Some(stage), Some(start)) = (stage, start) else {
            return;
        };
        // Stage 2 (wedding) needs delay
        if stage == 2 && start + LOVERSHIP_DELAY_MS > common_time() {
            return;
        }
        let mut list = acc.lovership.clone();
        list.remove(i);
        (
            acc.member_number,
            acc.account_name.clone(),
            true,
            list,
        )
    };

    if !can_break {
        return;
    }

    // Update target online + DB
    let target_sid = {
        let mut world = state.world.write();
        // Update source
        if let Some(acc) = world.get_by_socket_mut(socket_id) {
            acc.lovership = new_src_list.clone();
        }
        if let Some(sid) = world.socket_by_member.get(&target_mn).cloned() {
            if let Some(t) = world.get_by_socket_mut(&sid) {
                t.lovership
                    .retain(|l| l.get("MemberNumber").and_then(|v| v.as_i64()) != Some(src_mn));
                let clean = clean_lovership(&t.lovership);
                if let Some(io) = state.io.get() {
                    crate::socket_util::emit_to(
                        io,
                        &sid,
                        events::ACCOUNT_LOVERSHIP,
                        &json!({ "Lovership": clean }),
                    );
                }
                // Will sync character after
            }
            Some(sid)
        } else {
            None
        }
    };

    persist_lovership_clean(state, src_mn, &new_src_list).await;

    // Target list: online snapshot or load/strip from DB
    let tgt_list = {
        let world = state.world.read();
        if let Some(t) = world.get_by_member(target_mn) {
            t.lovership.clone()
        } else {
            Vec::new() // offline: pull from DB below
        }
    };
    if target_sid.is_some() {
        persist_lovership_clean(state, target_mn, &tgt_list).await;
    } else {
        // Offline: remove lover entry with our MemberNumber
        let _ = state
            .db
            .pull_lovership_member(target_mn, src_mn)
            .await;
    }

    let clean = clean_lovership(&new_src_list);
    let _ = socket.emit(events::ACCOUNT_LOVERSHIP, &json!({ "Lovership": clean }));
    sync_character(socket, state, src_mn, src_mn);
    if target_sid.is_some() {
        sync_character(socket, state, target_mn, target_mn);
    }
    let _ = src_account;
}

async fn apply_mutual_lover(
    socket: &SocketRef,
    state: &AppState,
    src: &crate::state::OnlineAccount,
    target: &crate::state::OnlineAccount,
    ai: usize,
    tl: Option<usize>,
    src_entry: Value,
    tgt_entry: Value,
    content: &str,
    room_socket_name: &str,
) {
    let lover_name = src_entry
        .get("Name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let lover_mn = src_entry
        .get("MemberNumber")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    {
        let mut world = state.world.write();
        if let Some(a) = world.get_by_socket_mut(&src.socket_id) {
            if ai < a.lovership.len() {
                a.lovership[ai] = src_entry.clone();
            } else {
                a.lovership.push(src_entry.clone());
            }
        }
        if let Some(t) = world.get_by_socket_mut(&target.socket_id) {
            if let Some(ti) = tl {
                if ti < t.lovership.len() {
                    t.lovership[ti] = tgt_entry.clone();
                } else {
                    t.lovership.push(tgt_entry.clone());
                }
            } else {
                t.lovership.push(tgt_entry.clone());
            }
        }
    }

    let src_list = {
        let world = state.world.read();
        world
            .get_by_socket(&src.socket_id)
            .map(|a| a.lovership.clone())
            .unwrap_or_default()
    };
    let tgt_list = {
        let world = state.world.read();
        world
            .get_by_socket(&target.socket_id)
            .map(|a| a.lovership.clone())
            .unwrap_or_default()
    };

    persist_lovership_clean(state, src.member_number, &src_list).await;
    persist_lovership_clean(state, target.member_number, &tgt_list).await;

    let clean_src = clean_lovership(&src_list);
    let clean_tgt = clean_lovership(&tgt_list);
    let _ = socket.emit(events::ACCOUNT_LOVERSHIP, &json!({ "Lovership": clean_src }));
    if let Some(io) = state.io.get() {
        crate::socket_util::emit_to(
            io,
            &target.socket_id,
            events::ACCOUNT_LOVERSHIP,
            &json!({ "Lovership": clean_tgt }),
        );
    }

    room_message(
        socket,
        &state,
        room_socket_name,
        src.member_number,
        content,
        "ServerMessage",
        None,
        Some(json!([
            { "Tag": "SourceCharacter", "Text": src.name, "MemberNumber": src.member_number },
            { "Tag": "TargetCharacter", "Text": lover_name, "MemberNumber": lover_mn },
        ])),
    );
    sync_character(socket, state, src.member_number, src.member_number);
    sync_character(socket, state, lover_mn, src.member_number);
}

fn lover_index(list: &[Value], member: i64) -> Option<usize> {
    list.iter().position(|l| {
        l.get("MemberNumber").and_then(|v| v.as_i64()) == Some(member)
            || l.get("BeginDatingOfferedByMemberNumber")
                .and_then(|v| v.as_i64())
                == Some(member)
    })
}

/// Strip pending offer fields for DB/client (Node AccountUpdateLovership).
fn clean_lovership(list: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for l in list {
        if l.get("BeginDatingOfferedByMemberNumber").is_some()
            && l.get("MemberNumber").is_none()
        {
            continue; // pending dating offer only — strip
        }
        let mut e = l.clone();
        if let Some(obj) = e.as_object_mut() {
            obj.remove("BeginEngagementOfferedByMemberNumber");
            obj.remove("BeginWeddingOfferedByMemberNumber");
            obj.remove("BeginDatingOfferedByMemberNumber");
        }
        out.push(e);
    }
    out
}

async fn persist_lovership_clean(state: &AppState, member: i64, list: &[Value]) {
    let clean = clean_lovership(list);
    match json_object_to_set_map(&json!({ "Lovership": clean })) {
        Ok(set) => {
            if let Err(e) = state.db.update_fields_by_member_number(member, set).await {
                error!(error = %e, member, "lovership persist failed");
            }
        }
        Err(e) => error!(error = %e, "lovership serialize failed"),
    }
}
