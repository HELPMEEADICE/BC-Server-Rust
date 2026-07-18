use std::collections::HashMap;

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

    let (account_name, should_persist, should_room_sync, should_sector_sync, member) = {
        let mut world = state.world.write();
        let Some(acc) = world.get_by_socket_mut(&socket_id) else {
            return;
        };

        // NPC Lover: Lover string starting with "NPC-"
        let mut npc_lovership_for_db: Option<Value> = None;
        if let Some(obj) = data.as_object_mut() {
            if let Some(Value::String(lover)) = obj.get("Lover").cloned() {
                if lover.starts_with("NPC-") && acc.lovership.len() < 5 {
                    let present = acc
                        .lovership
                        .iter()
                        .any(|l| l.get("Name").and_then(|n| n.as_str()) == Some(lover.as_str()));
                    if !present {
                        acc.lovership.push(json!({ "Name": lover }));
                        let mut clean = acc.lovership.clone();
                        clean.retain(|l| l.get("BeginDatingOfferedByMemberNumber").is_none());
                        for entry in &mut clean {
                            if let Some(o) = entry.as_object_mut() {
                                o.remove("BeginEngagementOfferedByMemberNumber");
                                o.remove("BeginWeddingOfferedByMemberNumber");
                            }
                        }
                        acc.lovership = clean.clone();
                        npc_lovership_for_db = Some(Value::Array(clean.clone()));
                        let _ =
                            socket.emit(events::ACCOUNT_LOVERSHIP, &json!({ "Lovership": clean }));
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

        let should_room_sync =
            acc.chat_room_id.is_some() && ROOM_SYNC_KEYS.iter().any(|k| obj.contains_key(*k));
        let should_sector_sync = obj.contains_key("Appearance")
            || obj.contains_key("ActivePose")
            || obj.contains_key("Title");
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

        (
            account_name,
            should_persist,
            should_room_sync,
            should_sector_sync,
            member,
        )
    };

    if should_room_sync {
        crate::room::sync_character(&socket, &state, member, member);
    }
    if should_sector_sync {
        emit_prison_sector_syncs_for_member(&state, member);
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

    if !current.is_empty() && current.trim().to_lowercase() != req.email_old.trim().to_lowercase() {
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
        "PublicPrisons" => {
            let include_own = req
                .extra
                .get("IncludeOwn")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let (requester_member, online) = {
                let world = state.world.read();
                let Some(acc) = world.get_by_socket(&socket_id) else {
                    return;
                };
                (
                    acc.member_number,
                    world
                        .accounts_by_socket
                        .values()
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            };

            // Offline prisons must be visible as well. The DB layer supplies only
            // rendering-safe prison fields, never credentials or account data.
            let mut by_member: HashMap<i64, Value> = HashMap::new();
            match state.db.list_public_prison_data().await {
                Ok(accounts) => {
                    for account in accounts {
                        let member = account.get("MemberNumber").and_then(|v| v.as_i64());
                        if member == Some(requester_member) && !include_own {
                            continue;
                        }
                        if let Some(entry) = public_prison_summary_from_document(&account, false) {
                            if let Some(member) = entry.get("MemberNumber").and_then(|v| v.as_i64())
                            {
                                by_member.insert(member, entry);
                            }
                        }
                    }
                }
                Err(e) => error!(error = %e, "public prison listing lookup failed"),
            }
            // In-memory sessions are fresher than their most recent persisted save.
            for account in &online {
                if account.member_number == requester_member && !include_own {
                    continue;
                }
                if let Some(entry) = public_prison_summary_online(account) {
                    by_member.insert(account.member_number, entry);
                }
            }
            let mut prisons: Vec<Value> = by_member.into_values().collect();
            prisons.sort_by(|a, b| {
                a.get("MemberName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .cmp(b.get("MemberName").and_then(|v| v.as_str()).unwrap_or(""))
            });
            let _ = socket.emit(
                events::ACCOUNT_QUERY_RESULT,
                &json!({ "Query": "PublicPrisons", "Result": prisons }),
            );
        }
        "PrisonRent" => {
            handle_prison_rent(socket, &state, &socket_id, &req.extra).await;
        }
        "PrisonRelease" => {
            handle_prison_release(socket, &state, &socket_id, &req.extra).await;
        }
        "PrisonWorkRoom" => {
            handle_prison_work_room(socket, &state, &socket_id, &req.extra).await;
        }
        _ => {}
    }
}

const PRISON_RENT_FEE: i64 = 500;
const PRISON_RENT_DURATION_MS: i64 = 24 * 60 * 60 * 1000;
const PRISON_RANSOM_MIN: i64 = 100;

#[derive(Clone)]
struct PrisonHost {
    account_name: String,
    extension_settings: Value,
}

fn prison_host_from_online(account: &crate::state::OnlineAccount) -> PrisonHost {
    PrisonHost {
        account_name: account.account_name.clone(),
        extension_settings: account
            .extra
            .get("ExtensionSettings")
            .cloned()
            .unwrap_or_else(|| json!({})),
    }
}

fn prison_host_from_document(document: &Value) -> Option<PrisonHost> {
    Some(PrisonHost {
        account_name: document.get("AccountName")?.as_str()?.to_string(),
        extension_settings: document
            .get("ExtensionSettings")
            .cloned()
            .unwrap_or_else(|| json!({})),
    })
}

async fn load_prison_host(state: &AppState, member_number: i64) -> Option<PrisonHost> {
    if let Some(account) = {
        let world = state.world.read();
        world.get_by_member(member_number).cloned()
    } {
        return Some(prison_host_from_online(&account));
    }
    state
        .db
        .find_by_member_number(member_number)
        .await
        .ok()
        .flatten()
        .and_then(|document| prison_host_from_document(&document))
}

fn prison_from_extension(extension_settings: &Value) -> Option<Value> {
    extension_settings.get("UndergroundPrison").cloned()
}

fn extension_with_prison(extension_settings: &Value, prison: Value) -> Value {
    let mut extension_settings = extension_settings.clone();
    if !extension_settings.is_object() {
        extension_settings = json!({});
    }
    extension_settings["UndergroundPrison"] = prison;
    extension_settings
}

fn value_i64(value: Option<&Value>) -> i64 {
    value
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|n| n.floor() as i64)))
        .unwrap_or(0)
}

fn rental_role(value: &Value) -> &str {
    value
        .get("RentRole")
        .and_then(|v| v.as_str())
        .unwrap_or("Owner")
}

fn rental_is_active(rental: &Value, now: i64) -> bool {
    rental
        .get("Until")
        .and_then(|v| v.as_i64())
        .map(|until| until <= 0 || until > now)
        .unwrap_or(true)
}

/// Visitor rents self or a private-room slave into an online or offline public prison.
async fn handle_prison_rent(socket: SocketRef, state: &AppState, socket_id: &str, extra: &Value) {
    let Some(host_member_number) = extra.get("HostMemberNumber").and_then(|v| v.as_i64()) else {
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "BadRequest" }),
        );
        return;
    };
    let sector_index = value_i64(extra.get("SectorIndex"));
    if sector_index < 0 {
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "BadRequest" }),
        );
        return;
    }
    let sector_index = sector_index as usize;
    let Some(renter) = ({
        let world = state.world.read();
        world.get_by_socket(socket_id).cloned()
    }) else {
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "Offline" }),
        );
        return;
    };
    if renter.member_number == host_member_number {
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "BadRequest" }),
        );
        return;
    }
    let Some(host) = load_prison_host(state, host_member_number).await else {
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "NotFound" }),
        );
        return;
    };
    let money = value_i64(renter.extra.get("Money"));
    if money < PRISON_RENT_FEE {
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "InsufficientFunds" }),
        );
        return;
    }

    let mut prison = match prison_from_extension(&host.extension_settings) {
        Some(prison)
            if prison.get("Owned").and_then(|v| v.as_bool()) == Some(true)
                && prison.get("Permission").and_then(|v| v.as_str()) == Some("Public") =>
        {
            prison
        }
        _ => {
            emit_prison_result(
                &socket,
                "PrisonRent",
                json!({ "Ok": false, "Error": "Private" }),
            );
            return;
        }
    };
    let now = common_time();
    let until = now + PRISON_RENT_DURATION_MS;
    let rent_role = if extra.get("Role").and_then(|v| v.as_str()) == Some("Self") {
        "Self"
    } else {
        "Slave"
    };
    let strength = value_i64(extra.get("Strength")).clamp(1, 100);
    let willpower = value_i64(extra.get("Willpower")).clamp(1, 100);
    let (name, private_index, asset_family, appearance) = if rent_role == "Self" {
        (
            renter.name.clone(),
            -1,
            renter
                .extra
                .get("AssetFamily")
                .and_then(|v| v.as_str())
                .unwrap_or("Female3DCG")
                .to_string(),
            renter
                .appearance
                .clone()
                .or_else(|| extra.get("Appearance").cloned())
                .unwrap_or_else(|| Value::Array(vec![])),
        )
    } else {
        let private_index = value_i64(
            extra
                .get("RenterPrivateIndex")
                .or_else(|| extra.get("PrivateIndex")),
        );
        let npc = renter
            .extra
            .get("PrivateCharacter")
            .and_then(|v| v.as_array())
            .and_then(|characters| {
                private_index
                    .checked_sub(1)
                    .and_then(|i| characters.get(i as usize))
            });
        let Some(npc) = npc else {
            emit_prison_result(
                &socket,
                "PrisonRent",
                json!({ "Ok": false, "Error": "BadSlave" }),
            );
            return;
        };
        (
            npc.get("Name")
                .and_then(|v| v.as_str())
                .unwrap_or("Worker")
                .chars()
                .take(40)
                .collect(),
            private_index,
            npc.get("AssetFamily")
                .and_then(|v| v.as_str())
                .unwrap_or("Female3DCG")
                .to_string(),
            npc.get("Appearance")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![])),
        )
    };

    let sectors = match prison.get_mut("Sectors").and_then(|v| v.as_array_mut()) {
        Some(sectors) => sectors,
        None => {
            emit_prison_result(
                &socket,
                "PrisonRent",
                json!({ "Ok": false, "Error": "Full" }),
            );
            return;
        }
    };
    let Some(sector) = sectors.get_mut(sector_index) else {
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "Full" }),
        );
        return;
    };
    let capacity = value_i64(sector.get("BaseSlaveCapacity")).max(5)
        + value_i64(sector.get("ExtraCapacity")).max(0);
    let has_overseer = sector
        .get("Overseers")
        .and_then(|v| v.as_array())
        .map(|overseers| !overseers.is_empty())
        .unwrap_or(false);
    if sector.get("Unlocked").and_then(|v| v.as_bool()) != Some(true) || !has_overseer {
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "Full" }),
        );
        return;
    }
    if !sector.get("Slaves").map(|v| v.is_array()).unwrap_or(false) {
        sector["Slaves"] = Value::Array(vec![]);
    }
    let slaves = sector
        .get_mut("Slaves")
        .and_then(|v| v.as_array_mut())
        .expect("array just created");
    slaves.retain(|rental| rental_is_active(rental, now));
    if slaves.len() as i64 >= capacity {
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "Full" }),
        );
        return;
    }
    slaves.push(json!({
        "Name": name,
        "PrivateIndex": -1,
        "CharacterID": "",
        "AssetFamily": asset_family,
        "Appearance": appearance,
        "AssignedTime": now,
        "Strength": strength,
        "Willpower": willpower,
        "RenterMemberNumber": renter.member_number,
        "RenterName": renter.name,
        "RenterPrivateIndex": private_index,
        "Until": until,
        "RentRole": rent_role,
    }));
    let vault = prison.get("Vault").and_then(|v| v.as_f64()).unwrap_or(0.0);
    prison["Vault"] = json!(vault + PRISON_RENT_FEE as f64);

    if !persist_prison(state, &host, &prison).await {
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "Error" }),
        );
        return;
    }
    if !persist_money(state, renter.member_number, money - PRISON_RENT_FEE).await {
        error!(
            member = renter.member_number,
            "PrisonRent money persist failed after prison persist"
        );
        emit_prison_result(
            &socket,
            "PrisonRent",
            json!({ "Ok": false, "Error": "Error" }),
        );
        return;
    }
    apply_prison_and_money_to_online(
        state,
        host_member_number,
        &host.extension_settings,
        &prison,
        renter.member_number,
        money - PRISON_RENT_FEE,
    );
    emit_prison_rental_sync(
        state,
        &[host_member_number, renter.member_number],
        json!({
            "Action": "Rented",
            "HostMemberNumber": host_member_number,
            "SectorIndex": sector_index,
            "RenterMemberNumber": renter.member_number,
            "RentRole": rent_role,
            "RenterPrivateIndex": private_index,
            "Until": until,
        }),
    );
    emit_prison_result(
        &socket,
        "PrisonRent",
        json!({
            "Ok": true,
            "HostMemberNumber": host_member_number,
            "SectorIndex": sector_index,
            "RentRole": rent_role,
            "RenterPrivateIndex": private_index,
            "Until": until,
            "Fee": PRISON_RENT_FEE,
        }),
    );
}

async fn handle_prison_release(
    socket: SocketRef,
    state: &AppState,
    socket_id: &str,
    extra: &Value,
) {
    let action = extra.get("Action").and_then(|v| v.as_str()).unwrap_or("");
    let Some(host_member_number) = extra.get("HostMemberNumber").and_then(|v| v.as_i64()) else {
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "BadRequest" }),
        );
        return;
    };
    let sector_index = value_i64(extra.get("SectorIndex"));
    if sector_index < 0 {
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "BadRequest" }),
        );
        return;
    }
    let sector_index = sector_index as usize;
    let Some(requester) = ({
        let world = state.world.read();
        world.get_by_socket(socket_id).cloned()
    }) else {
        return;
    };
    let role = if extra.get("Role").and_then(|v| v.as_str()) == Some("Slave") {
        "Slave"
    } else {
        "Self"
    };
    let renter_private_index = value_i64(
        extra
            .get("TargetPrivateIndex")
            .or_else(|| extra.get("RenterPrivateIndex"))
            .or_else(|| extra.get("PrivateIndex")),
    );
    let Some(host) = load_prison_host(state, host_member_number).await else {
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "NotFound" }),
        );
        return;
    };
    let Some(mut prison) = prison_from_extension(&host.extension_settings) else {
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "NotFound" }),
        );
        return;
    };
    let target_renter = extra
        .get("TargetMemberNumber")
        .or_else(|| extra.get("RenterMemberNumber"))
        .and_then(|value| value.as_i64())
        .unwrap_or(requester.member_number);
    if action == "OwnerDismiss" && requester.member_number != host_member_number {
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "NotOwner" }),
        );
        return;
    }
    if action != "OwnerDismiss" && action != "SelfRansom" {
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "BadRequest" }),
        );
        return;
    }
    if action == "SelfRansom" && (role != "Self" || target_renter != requester.member_number) {
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "NotRenter" }),
        );
        return;
    }
    let Some(removed) = remove_prison_rental(
        &mut prison,
        sector_index,
        target_renter,
        role,
        renter_private_index,
    ) else {
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "NotFound" }),
        );
        return;
    };
    let until = value_i64(removed.get("Until"));
    let fee = if action == "SelfRansom" {
        ransom_fee(until, common_time())
    } else {
        0
    };
    let requester_money = value_i64(requester.extra.get("Money"));
    if fee > requester_money {
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "InsufficientFunds" }),
        );
        return;
    }
    if !persist_prison(state, &host, &prison).await {
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "Error" }),
        );
        return;
    }
    if fee > 0 && !persist_money(state, requester.member_number, requester_money - fee).await {
        error!(
            member = requester.member_number,
            "PrisonRelease ransom persist failed after prison persist"
        );
        emit_prison_result(
            &socket,
            "PrisonRelease",
            json!({ "Ok": false, "Error": "Error" }),
        );
        return;
    }
    apply_prison_and_money_to_online(
        state,
        host_member_number,
        &host.extension_settings,
        &prison,
        requester.member_number,
        requester_money - fee,
    );

    // A dismissed/redeemed real player is removed from the in-sector session;
    // the client receives the rental sync and returns to the hub itself.
    remove_prison_sector_member(state, host_member_number, sector_index, target_renter);
    let sync_action = if action == "OwnerDismiss" {
        "Dismissed"
    } else {
        "Redeemed"
    };
    emit_prison_rental_sync(
        state,
        &[host_member_number, target_renter],
        json!({
            "Action": sync_action,
            "HostMemberNumber": host_member_number,
            "SectorIndex": sector_index,
            "RenterMemberNumber": target_renter,
            "RentRole": rental_role(&removed),
            "RenterPrivateIndex": value_i64(removed.get("RenterPrivateIndex")),
            "Until": until,
        }),
    );
    emit_prison_result(
        &socket,
        "PrisonRelease",
        json!({
            "Ok": true,
            "Action": action,
            "HostMemberNumber": host_member_number,
            "SectorIndex": sector_index,
            "RenterMemberNumber": target_renter,
            "RentRole": rental_role(&removed),
            "RenterPrivateIndex": value_i64(removed.get("RenterPrivateIndex")),
            "Until": until,
            "Fee": fee,
        }),
    );
}

async fn handle_prison_work_room(
    socket: SocketRef,
    state: &AppState,
    socket_id: &str,
    extra: &Value,
) {
    let action = extra.get("Action").and_then(|v| v.as_str()).unwrap_or("");
    if action != "Join" && action != "Leave" && action != "Sync" {
        emit_prison_result(
            &socket,
            "PrisonWorkRoom",
            json!({ "Ok": false, "Error": "BadRequest" }),
        );
        return;
    }
    let Some(host_member_number) = extra.get("HostMemberNumber").and_then(|v| v.as_i64()) else {
        emit_prison_result(
            &socket,
            "PrisonWorkRoom",
            json!({ "Ok": false, "Error": "BadRequest" }),
        );
        return;
    };
    let sector_index = value_i64(extra.get("SectorIndex"));
    if sector_index < 0 {
        emit_prison_result(
            &socket,
            "PrisonWorkRoom",
            json!({ "Ok": false, "Error": "BadRequest" }),
        );
        return;
    }
    let sector_index = sector_index as usize;
    let Some(requester) = ({
        let world = state.world.read();
        world.get_by_socket(socket_id).cloned()
    }) else {
        return;
    };
    if action == "Leave" {
        remove_prison_sector_member(
            state,
            host_member_number,
            sector_index,
            requester.member_number,
        );
        emit_prison_result(
            &socket,
            "PrisonWorkRoom",
            json!({
                "Ok": true,
                "Action": "Leave",
                "HostMemberNumber": host_member_number,
                "SectorIndex": sector_index,
                "Mode": "AI",
            }),
        );
        return;
    }
    let Some(host) = load_prison_host(state, host_member_number).await else {
        emit_prison_result(
            &socket,
            "PrisonWorkRoom",
            json!({ "Ok": false, "Error": "NotFound" }),
        );
        return;
    };
    let Some(prison) = prison_from_extension(&host.extension_settings) else {
        emit_prison_result(
            &socket,
            "PrisonWorkRoom",
            json!({ "Ok": false, "Error": "NotFound" }),
        );
        return;
    };
    let Some(sector) = prison
        .get("Sectors")
        .and_then(|v| v.as_array())
        .and_then(|sectors| sectors.get(sector_index))
    else {
        emit_prison_result(
            &socket,
            "PrisonWorkRoom",
            json!({ "Ok": false, "Error": "NotFound" }),
        );
        return;
    };
    if sector.get("Unlocked").and_then(|v| v.as_bool()) != Some(true) {
        emit_prison_result(
            &socket,
            "PrisonWorkRoom",
            json!({ "Ok": false, "Error": "NotFound" }),
        );
        return;
    }
    let is_host = requester.member_number == host_member_number;
    let has_active_self_rental = sector
        .get("Slaves")
        .and_then(|v| v.as_array())
        .map(|rentals| {
            rentals.iter().any(|rental| {
                rental.get("RenterMemberNumber").and_then(|v| v.as_i64())
                    == Some(requester.member_number)
                    && rental_role(rental) == "Self"
                    && rental_is_active(rental, common_time())
            })
        })
        .unwrap_or(false);
    if !is_host && !has_active_self_rental {
        emit_prison_result(
            &socket,
            "PrisonWorkRoom",
            json!({ "Ok": false, "Error": "NotRented" }),
        );
        return;
    }
    if action == "Sync" {
        let snapshot = {
            let world = state.world.read();
            let joined = world
                .prison_sector_sessions
                .get(&prison_sector_key(host_member_number, sector_index))
                .map(|session| session.members.contains(&requester.member_number))
                .unwrap_or(false);
            if joined {
                prison_sector_snapshot(&world, host_member_number, sector_index)
            } else {
                None
            }
        };
        if snapshot.is_none() {
            emit_prison_result(
                &socket,
                "PrisonWorkRoom",
                json!({ "Ok": false, "Error": "NotJoined" }),
            );
            return;
        }
        emit_prison_result(
            &socket,
            "PrisonWorkRoom",
            json!({
                "Ok": true,
                "Action": "Sync",
                "HostMemberNumber": host_member_number,
                "SectorIndex": sector_index,
                "Mode": "AI",
                "Members": snapshot,
            }),
        );
        return;
    }
    let (members, previous_sessions) = {
        let mut world = state.world.write();
        // One online player can only render one prison work sector at a time.
        let mut previous_sessions = Vec::new();
        for session in world.prison_sector_sessions.values_mut() {
            if session.members.remove(&requester.member_number) {
                previous_sessions.push((session.host_member_number, session.sector_index));
            }
        }
        world
            .prison_sector_sessions
            .retain(|_, session| !session.members.is_empty());
        let key = prison_sector_key(host_member_number, sector_index);
        let session = world.prison_sector_sessions.entry(key).or_insert_with(|| {
            crate::state::PrisonSectorSession {
                host_member_number,
                sector_index,
                members: std::collections::HashSet::new(),
            }
        });
        session.members.insert(requester.member_number);
        (
            prison_sector_snapshot(&world, host_member_number, sector_index).unwrap_or_default(),
            previous_sessions,
        )
    };
    for (previous_host, previous_sector) in previous_sessions {
        if previous_host != host_member_number || previous_sector != sector_index {
            emit_prison_sector_sync(state, previous_host, previous_sector);
        }
    }
    emit_prison_sector_sync(state, host_member_number, sector_index);
    emit_prison_result(
        &socket,
        "PrisonWorkRoom",
        json!({
            "Ok": true,
            "Action": "Join",
            "HostMemberNumber": host_member_number,
            "SectorIndex": sector_index,
            "Mode": "AI",
            "Members": members,
        }),
    );
}

async fn persist_prison(state: &AppState, host: &PrisonHost, prison: &Value) -> bool {
    let mut set = serde_json::Map::new();
    set.insert("ExtensionSettings.UndergroundPrison".into(), prison.clone());
    if let Err(error) = state.db.update_fields(&host.account_name, set).await {
        error!(error = %error, account = %host.account_name, "underground prison persist failed");
        return false;
    }
    true
}

async fn persist_money(state: &AppState, member_number: i64, money: i64) -> bool {
    let mut set = serde_json::Map::new();
    set.insert("Money".into(), json!(money));
    if let Err(error) = state
        .db
        .update_fields_by_member_number(member_number, set)
        .await
    {
        error!(error = %error, member = member_number, "underground prison money persist failed");
        return false;
    }
    true
}

fn apply_prison_and_money_to_online(
    state: &AppState,
    host_member_number: i64,
    previous_extension_settings: &Value,
    prison: &Value,
    renter_member_number: i64,
    renter_money: i64,
) {
    apply_prison_to_online(
        state,
        host_member_number,
        previous_extension_settings,
        prison,
    );
    apply_money_to_online(state, renter_member_number, renter_money);
}

fn apply_prison_to_online(
    state: &AppState,
    host_member_number: i64,
    previous_extension_settings: &Value,
    prison: &Value,
) {
    let mut world = state.world.write();
    if let Some(host) = world
        .get_by_member(host_member_number)
        .map(|account| account.socket_id.clone())
    {
        if let Some(account) = world.get_by_socket_mut(&host) {
            account.extra.insert(
                "ExtensionSettings".into(),
                extension_with_prison(previous_extension_settings, prison.clone()),
            );
        }
    }
}

fn apply_money_to_online(state: &AppState, renter_member_number: i64, renter_money: i64) {
    let mut world = state.world.write();
    if let Some(renter) = world
        .get_by_member(renter_member_number)
        .map(|account| account.socket_id.clone())
    {
        if let Some(account) = world.get_by_socket_mut(&renter) {
            account.extra.insert("Money".into(), json!(renter_money));
        }
    }
}

fn remove_prison_rental(
    prison: &mut Value,
    sector_index: usize,
    renter_member_number: i64,
    role: &str,
    renter_private_index: i64,
) -> Option<Value> {
    let slaves = prison
        .get_mut("Sectors")?
        .as_array_mut()?
        .get_mut(sector_index)?
        .get_mut("Slaves")?
        .as_array_mut()?;
    let index = slaves.iter().position(|rental| {
        rental.get("RenterMemberNumber").and_then(|v| v.as_i64()) == Some(renter_member_number)
            && rental_role(rental) == role
            && (role == "Self"
                || value_i64(rental.get("RenterPrivateIndex")) == renter_private_index)
    })?;
    Some(slaves.remove(index))
}

fn ransom_fee(until: i64, now: i64) -> i64 {
    if until <= now {
        return 0;
    }
    let remaining = until - now;
    ((PRISON_RENT_FEE * remaining + PRISON_RENT_DURATION_MS - 1) / PRISON_RENT_DURATION_MS)
        .max(PRISON_RANSOM_MIN)
}

fn emit_prison_result(socket: &SocketRef, query: &str, result: Value) {
    let _ = socket.emit(
        events::ACCOUNT_QUERY_RESULT,
        &json!({ "Query": query, "Result": result }),
    );
}

fn emit_prison_rental_sync(state: &AppState, members: &[i64], payload: Value) {
    let socket_ids = {
        let world = state.world.read();
        members
            .iter()
            .filter_map(|member| {
                world
                    .get_by_member(*member)
                    .map(|account| account.socket_id.clone())
            })
            .collect::<Vec<_>>()
    };
    if let Some(io) = state.io.get() {
        for socket_id in socket_ids {
            crate::socket_util::emit_to(io, &socket_id, events::PRISON_RENTAL_SYNC, &payload);
        }
    }
}

fn prison_sector_key(host_member_number: i64, sector_index: usize) -> String {
    format!("{host_member_number}:{sector_index}")
}

fn prison_sector_snapshot(
    world: &crate::state::World,
    host_member_number: i64,
    sector_index: usize,
) -> Option<Vec<Value>> {
    let session = world
        .prison_sector_sessions
        .get(&prison_sector_key(host_member_number, sector_index))?;
    let mut members = session
        .members
        .iter()
        .filter_map(|member_number| {
            let account = world.get_by_member(*member_number)?;
            Some(json!({
                "MemberNumber": account.member_number,
                "Name": account.name,
                "Role": if account.member_number == host_member_number { "Owner" } else { "Renter" },
                "AssetFamily": account.extra.get("AssetFamily").and_then(|value| value.as_str()).unwrap_or("Female3DCG"),
                "Appearance": account.appearance.clone().unwrap_or_else(|| Value::Array(vec![])),
                "ActivePose": account.active_pose.clone(),
                "Title": account.title.clone(),
            }))
        })
        .collect::<Vec<_>>();
    members.sort_by_key(|member| member.get("MemberNumber").and_then(|value| value.as_i64()));
    Some(members)
}

pub fn emit_prison_sector_sync(state: &AppState, host_member_number: i64, sector_index: usize) {
    let (socket_ids, members) = {
        let world = state.world.read();
        let Some(session) = world
            .prison_sector_sessions
            .get(&prison_sector_key(host_member_number, sector_index))
        else {
            return;
        };
        let socket_ids = session
            .members
            .iter()
            .filter_map(|member_number| {
                world
                    .get_by_member(*member_number)
                    .map(|account| account.socket_id.clone())
            })
            .collect::<Vec<_>>();
        let members =
            prison_sector_snapshot(&world, host_member_number, sector_index).unwrap_or_default();
        (socket_ids, members)
    };
    if let Some(io) = state.io.get() {
        let payload = json!({
            "HostMemberNumber": host_member_number,
            "SectorIndex": sector_index,
            "Mode": "AI",
            "Members": members,
        });
        for socket_id in socket_ids {
            crate::socket_util::emit_to(io, &socket_id, events::PRISON_SECTOR_SYNC, &payload);
        }
    }
}

/// Pushes appearance/pose/title updates to the active sector where a member is
/// currently rendered. The snapshot remains authoritative and includes peers.
fn emit_prison_sector_syncs_for_member(state: &AppState, member_number: i64) {
    let sessions = {
        let world = state.world.read();
        world
            .prison_sector_sessions
            .values()
            .filter(|session| session.members.contains(&member_number))
            .map(|session| (session.host_member_number, session.sector_index))
            .collect::<Vec<_>>()
    };
    for (host_member_number, sector_index) in sessions {
        emit_prison_sector_sync(state, host_member_number, sector_index);
    }
}

fn remove_prison_sector_member(
    state: &AppState,
    host_member_number: i64,
    sector_index: usize,
    member_number: i64,
) {
    let changed = {
        let mut world = state.world.write();
        let key = prison_sector_key(host_member_number, sector_index);
        let changed = world
            .prison_sector_sessions
            .get_mut(&key)
            .map(|session| session.members.remove(&member_number))
            .unwrap_or(false);
        world
            .prison_sector_sessions
            .retain(|_, session| !session.members.is_empty());
        changed
    };
    if changed {
        emit_prison_sector_sync(state, host_member_number, sector_index);
    }
}

/// Removes expired rentals from live sector sessions. A modified client cannot
/// keep rendering a player in a sector after the paid term has ended.
pub async fn expire_prison_sector_rentals(state: &AppState) {
    let sectors = {
        let world = state.world.read();
        world
            .prison_sector_sessions
            .iter()
            .map(|(_, session)| (session.host_member_number, session.sector_index))
            .collect::<Vec<_>>()
    };
    let now = common_time();
    for (host_member_number, sector_index) in sectors {
        let Some(host) = load_prison_host(state, host_member_number).await else {
            continue;
        };
        let Some(mut prison) = prison_from_extension(&host.extension_settings) else {
            continue;
        };
        let Some(slaves) = prison
            .get_mut("Sectors")
            .and_then(|value| value.as_array_mut())
            .and_then(|sectors| sectors.get_mut(sector_index))
            .and_then(|sector| sector.get_mut("Slaves"))
            .and_then(|value| value.as_array_mut())
        else {
            continue;
        };
        let mut expired = Vec::new();
        slaves.retain(|rental| {
            if rental_is_active(rental, now) {
                true
            } else {
                expired.push(rental.clone());
                false
            }
        });
        if expired.is_empty() {
            continue;
        }
        if !persist_prison(state, &host, &prison).await {
            continue;
        }
        apply_prison_to_online(state, host_member_number, &host.extension_settings, &prison);
        for rental in expired {
            let Some(renter_member_number) = rental
                .get("RenterMemberNumber")
                .and_then(|value| value.as_i64())
            else {
                continue;
            };
            let role = rental_role(&rental);
            if role != "Self" && role != "Slave" {
                continue;
            }
            remove_prison_sector_member(
                state,
                host_member_number,
                sector_index,
                renter_member_number,
            );
            emit_prison_rental_sync(
                state,
                &[host_member_number, renter_member_number],
                json!({
                    "Action": "Expired",
                    "HostMemberNumber": host_member_number,
                    "SectorIndex": sector_index,
                    "RenterMemberNumber": renter_member_number,
                    "RentRole": role,
                    "RenterPrivateIndex": value_i64(rental.get("RenterPrivateIndex")),
                    "Until": value_i64(rental.get("Until")),
                }),
            );
        }
    }
}

/// Client `PrivateCharacter[0]` is the player and is never synced. Server array
/// index 0 therefore corresponds to client `PrivateCharacter[1]`.
fn resolve_private_appearance(
    private_characters: Option<&Vec<Value>>,
    entry: &Value,
) -> (String, Value, Option<String>) {
    let mut asset_family = entry
        .get("AssetFamily")
        .and_then(|v| v.as_str())
        .unwrap_or("Female3DCG")
        .to_string();
    let mut appearance = entry
        .get("Appearance")
        .cloned()
        .unwrap_or_else(|| Value::Array(vec![]));
    let mut title = entry
        .get("Title")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if appearance
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false)
    {
        return (asset_family, appearance, title);
    }
    let Some(private_characters) = private_characters else {
        return (asset_family, appearance, title);
    };
    let by_index = value_i64(entry.get("PrivateIndex"))
        .checked_sub(1)
        .and_then(|index| private_characters.get(index as usize));
    let by_name = entry.get("Name").and_then(|v| v.as_str()).and_then(|name| {
        private_characters
            .iter()
            .find(|npc| npc.get("Name").and_then(|v| v.as_str()) == Some(name))
    });
    if let Some(npc) = by_index.or(by_name) {
        if let Some(value) = npc.get("Appearance").cloned() {
            if value.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                appearance = value;
            }
        }
        if let Some(value) = npc.get("AssetFamily").and_then(|v| v.as_str()) {
            asset_family = value.to_string();
        }
        if title.is_none() {
            title = npc
                .get("Title")
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
    }
    (asset_family, appearance, title)
}

fn public_prison_summary_online(account: &crate::state::OnlineAccount) -> Option<Value> {
    public_prison_summary(
        account.member_number,
        &account.name,
        account.extra.get("ExtensionSettings")?,
        account
            .extra
            .get("PrivateCharacter")
            .and_then(|v| v.as_array()),
        true,
        common_time(),
    )
}

fn public_prison_summary_from_document(document: &Value, online: bool) -> Option<Value> {
    public_prison_summary(
        document.get("MemberNumber")?.as_i64()?,
        document
            .get("Name")
            .and_then(|v| v.as_str())
            .unwrap_or("Prison owner"),
        document.get("ExtensionSettings")?,
        document.get("PrivateCharacter").and_then(|v| v.as_array()),
        online,
        value_i64(document.get("LastLogin")),
    )
}

/// Builds a sanitized listing while preserving rental identity fields necessary for
/// other clients to synchronize release, ransom, and forced-room-exit events.
fn public_prison_summary(
    member_number: i64,
    member_name: &str,
    extension_settings: &Value,
    private_characters: Option<&Vec<Value>>,
    online: bool,
    last_active: i64,
) -> Option<Value> {
    let prison = extension_settings.get("UndergroundPrison")?;
    if prison.get("Owned").and_then(|v| v.as_bool()) != Some(true)
        || prison.get("Permission").and_then(|v| v.as_str()) != Some("Public")
    {
        return None;
    }
    const MINE_RATE: f64 = 0.5;
    const FACTORY_RATE: f64 = 0.8;
    const OVERSEER_BONUS: f64 = 1.5;
    const NO_OVERSEER_BONUS: f64 = 0.5;
    let now = common_time();
    let mut sector_count = 0i64;
    let mut slave_count = 0i64;
    let mut hourly = 0.0f64;
    let mut sectors_out = Vec::new();
    for sector in prison
        .get("Sectors")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        if sector.get("Unlocked").and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        sector_count += 1;
        let sector_type = sector
            .get("Type")
            .and_then(|v| v.as_str())
            .unwrap_or("Mine");
        let bonus = if sector
            .get("Overseers")
            .and_then(|v| v.as_array())
            .map(|v| !v.is_empty())
            .unwrap_or(false)
        {
            OVERSEER_BONUS
        } else {
            NO_OVERSEER_BONUS
        };
        let mut slaves_out = Vec::new();
        for slave in sector
            .get("Slaves")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            if !rental_is_active(slave, now) {
                continue;
            }
            let strength = slave
                .get("Strength")
                .and_then(|v| v.as_f64())
                .unwrap_or(50.0);
            let willpower = slave
                .get("Willpower")
                .and_then(|v| v.as_f64())
                .unwrap_or(50.0);
            slave_count += 1;
            hourly += if sector_type == "Factory" {
                (willpower / 10.0) * FACTORY_RATE * bonus * 60.0
            } else {
                (strength / 10.0) * MINE_RATE * bonus * 60.0
            };
            let (asset_family, appearance, title) =
                resolve_private_appearance(private_characters, slave);
            let mut row = json!({
                "Name": slave.get("Name").and_then(|v| v.as_str()).unwrap_or("Slave"),
                "PrivateIndex": value_i64(slave.get("PrivateIndex")),
                "CharacterID": slave.get("CharacterID").and_then(|v| v.as_str()).unwrap_or(""),
                "AssignedTime": value_i64(slave.get("AssignedTime")),
                "Strength": strength.round() as i64,
                "Willpower": willpower.round() as i64,
                "AssetFamily": asset_family,
                "Appearance": appearance,
            });
            for key in [
                "RenterMemberNumber",
                "RenterName",
                "RenterPrivateIndex",
                "Until",
                "RentRole",
            ] {
                if let Some(value) = slave.get(key) {
                    row[key] = value.clone();
                }
            }
            if let Some(title) = title {
                row["Title"] = json!(title);
            }
            slaves_out.push(row);
        }
        let mut overseers_out = Vec::new();
        for overseer in sector
            .get("Overseers")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let (asset_family, appearance, title) =
                resolve_private_appearance(private_characters, overseer);
            let mut row = json!({
                "Name": overseer.get("Name").and_then(|v| v.as_str()).unwrap_or("Overseer"),
                "PrivateIndex": value_i64(overseer.get("PrivateIndex")),
                "AssetFamily": asset_family,
                "Appearance": appearance,
            });
            if let Some(title) = title {
                row["Title"] = json!(title);
            }
            overseers_out.push(row);
        }
        sectors_out.push(json!({
            "Type": sector_type,
            "Unlocked": true,
            "BaseSlaveCapacity": value_i64(sector.get("BaseSlaveCapacity")).max(5),
            "ExtraCapacity": value_i64(sector.get("ExtraCapacity")).max(0),
            "Slaves": slaves_out,
            "Overseers": overseers_out,
        }));
    }
    Some(json!({
        "MemberNumber": member_number,
        "MemberName": member_name,
        "Name": prison.get("Name").and_then(|v| v.as_str()).filter(|v| !v.is_empty()).unwrap_or(member_name),
        "SectorCount": sector_count,
        "SlaveCount": slave_count,
        "Hourly": hourly.floor() as i64,
        "Sectors": sectors_out,
        "Online": online,
        "LastActive": last_active,
        "RentalsAuthoritative": true,
    }))
}

#[cfg(test)]
mod prison_tests {
    use super::*;

    #[test]
    fn ransom_fee_is_zero_after_expiry_and_scales_before_it() {
        assert_eq!(ransom_fee(100, 100), 0);
        assert_eq!(ransom_fee(100, 101), 0);
        assert_eq!(ransom_fee(101, 100), PRISON_RANSOM_MIN);
        assert_eq!(ransom_fee(PRISON_RENT_DURATION_MS, 0), PRISON_RENT_FEE);
    }

    #[test]
    fn offline_public_summary_keeps_remote_rental_identity() {
        let extension = json!({
            "UndergroundPrison": {
                "Owned": true,
                "Permission": "Public",
                "Name": "Offline block",
                "Sectors": [{
                    "Unlocked": true,
                    "Type": "Mine",
                    "BaseSlaveCapacity": 5,
                    "ExtraCapacity": 0,
                    "Overseers": [{ "Name": "Guard" }],
                    "Slaves": [{
                        "Name": "Renter",
                        "Strength": 50,
                        "Willpower": 50,
                        "RenterMemberNumber": 42,
                        "RenterPrivateIndex": -1,
                        "RentRole": "Self",
                        "Until": common_time() + 60_000,
                    }]
                }]
            }
        });
        let summary =
            public_prison_summary(9, "Owner", &extension, None, false, 12).expect("public prison");
        assert_eq!(summary["Online"], json!(false));
        assert_eq!(summary["RentalsAuthoritative"], json!(true));
        assert_eq!(
            summary["Sectors"][0]["Slaves"][0]["RenterMemberNumber"],
            json!(42)
        );
        assert_eq!(
            summary["Sectors"][0]["Slaves"][0]["RentRole"],
            json!("Self")
        );
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
