use mongodb::bson::doc;
use serde_json::Value;
use socketioxide::extract::{Data, SocketRef, State};
use tracing::{error, info};

use crate::auth::hash_password;
use crate::config::Config;
use crate::limits::extract_ip;
use crate::protocol::codes;
use crate::protocol::events;
use crate::protocol::{AccountCreateRequest, AccountCreateSuccess};
use crate::util::{
    common_time, is_account_name, is_account_password, is_character_name, is_email_valid,
};
use crate::AppState;

pub async fn handle_account_create(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let parsed: Result<AccountCreateRequest, _> = serde_json::from_value(data.clone());
    let Ok(req) = parsed else {
        let _ = socket.emit(events::CREATION_RESPONSE, &codes::INVALID_ACCOUNT_INFO);
        return;
    };

    if !is_character_name(&req.name)
        || !is_account_name(&req.account_name)
        || !is_account_password(&req.password)
        || !(req.email.is_empty() || is_email_valid(&req.email))
        || req.email.len() > 100
    {
        let _ = socket.emit(events::CREATION_RESPONSE, &codes::INVALID_ACCOUNT_INFO);
        return;
    }

    let ip = client_ip(&socket);
    {
        let mut world = state.world.write();
        world.prune_old_creation_records();
        let (total, hour) = world.count_creations_for_ip(&ip);
        if total >= state.config.max_ip_account_per_day
            || hour >= state.config.max_ip_account_per_hour
        {
            let _ = socket.emit(events::CREATION_RESPONSE, &codes::NEW_ACCOUNTS_EXCEEDED);
            return;
        }
        world.account_creation_ip.push(crate::state::AccountCreationRecord {
            address: ip.clone(),
            time: common_time(),
        });
    }

    let account_name = req.account_name.to_uppercase();

    match state.db.find_by_account_name(&account_name).await {
        Ok(Some(_)) => {
            let _ = socket.emit(events::CREATION_RESPONSE, &codes::ACCOUNT_ALREADY_EXISTS);
            return;
        }
        Ok(None) => {}
        Err(e) => {
            error!(error = %e, "DB error on account create lookup");
            let _ = socket.emit(events::CREATION_RESPONSE, &codes::INVALID_ACCOUNT_INFO);
            return;
        }
    }

    let hash = match hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "bcrypt hash failed");
            let _ = socket.emit(events::CREATION_RESPONSE, &codes::INVALID_ACCOUNT_INFO);
            return;
        }
    };

    let environment = account_environment(&socket, &state.config);
    let member_number = state.world.read().allocate_member_number();
    let now = common_time();
    let socket_id = socket.id.to_string();

    let doc = doc! {
        "AccountName": &account_name,
        "Email": &req.email,
        "Password": &hash,
        "MemberNumber": member_number,
        "Name": &req.name,
        "Money": 100i64,
        "Creation": now,
        "LastLogin": now,
        "Lovership": [],
        "ItemPermission": 2i32,
        "FriendList": [],
        "WhiteList": [],
        "BlackList": [],
    };

    if let Err(e) = state.db.insert_account(doc).await {
        error!(error = %e, "insert account failed");
        // roll back member number is not critical
        let _ = socket.emit(events::CREATION_RESPONSE, &codes::INVALID_ACCOUNT_INFO);
        return;
    }

    let account = new_online_from_create(
        socket_id.clone(),
        account_name.clone(),
        member_number,
        req.name.clone(),
        environment,
        now,
    );

    {
        let mut world = state.world.write();
        world.insert_account(account);
    }

    info!(account = %account_name, id = %socket_id, "Creating new account");

    crate::handlers::on_login(&socket, &state);

    let resp = AccountCreateSuccess {
        server_answer: codes::ACCOUNT_CREATED,
        online_id: socket_id,
        member_number,
    };
    let _ = socket.emit(events::CREATION_RESPONSE, &resp);
    crate::handlers::send_server_info_to(&socket, &state);
}

pub fn client_ip(socket: &SocketRef) -> String {
    let remote = socket.req_parts().extensions.get::<String>().cloned();
    // Prefer peer from transport if available via headers
    let xff = socket
        .req_parts()
        .headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok());
    // socketioxide may not expose remote address the same way; use XFF or fallback
    let remote_str = remote.as_deref();
    extract_ip(remote_str, xff)
}

pub fn account_environment(socket: &SocketRef, config: &Config) -> String {
    let origin = socket
        .req_parts()
        .headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    if origin.is_empty() {
        return format!("{}", rand::random::<u64>() % 1_000_000_000_000);
    }
    if config.production_origins.iter().any(|p| p == &origin) {
        "PROD".into()
    } else {
        "DEV".into()
    }
}

// helper used by create — re-export style
use crate::state::OnlineAccount;

fn new_online_from_create(
    socket_id: String,
    account_name: String,
    member_number: i64,
    name: String,
    environment: String,
    creation: i64,
) -> OnlineAccount {
    OnlineAccount {
        socket_id,
        account_name,
        member_number,
        name,
        environment,
        creation,
        item_permission: 2,
        white_list: vec![],
        black_list: vec![],
        friend_list: vec![],
        ownership: None,
        owner: String::new(),
        lovership: vec![],
        difficulty: None,
        appearance: None,
        inventory_data: None,
        arousal_settings: None,
        online_shared_settings: None,
        game: None,
        map_data: None,
        label_color: None,
        reputation: None,
        description: None,
        block_items: None,
        limited_items: None,
        favorite_items: None,
        title: None,
        nickname: None,
        crafting: None,
        pose: None,
        active_pose: None,
        chat_room_id: None,
        delayed_appearance: None,
        delayed_skill: None,
        delayed_game: None,
        extra: Default::default(),
        logged_in_at: creation,
    }
}
