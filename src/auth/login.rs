use serde_json::Value;
use socketioxide::extract::{Data, SocketRef, State};
use tracing::{error, info};

use crate::auth::verify_password;
use crate::protocol::codes;
use crate::protocol::events;
use crate::protocol::AccountLoginRequest;
use crate::state::OnlineAccount;
use crate::util::common_time;
use crate::AppState;

pub async fn handle_account_login(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }

    let parsed: Result<AccountLoginRequest, _> = serde_json::from_value(data);
    let Ok(req) = parsed else {
        let _ = socket.emit(events::LOGIN_RESPONSE, &codes::INVALID_NAME_PASSWORD);
        return;
    };

    let socket_id = socket.id.to_string();

    // Prevent duplicate pending logins for same socket
    {
        let mut world = state.world.write();
        if world.login_pending.contains(&socket_id) {
            return;
        }
        world.login_pending.insert(socket_id.clone());
        // Track queue length for LoginQueue event
        world.login_queue_len += 1;
        let qlen = world.login_queue_len;
        if qlen > 16 {
            drop(world);
            let _ = socket.emit(events::LOGIN_QUEUE, &qlen);
        }
    }

    // Serial login processing (Node: loginQueue + 50ms between jobs)
    let _guard = state.login_mutex.lock().await;

    // 50ms inter-login delay after acquiring lock (except first is fine)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    if !socket.connected() {
        finish_queue_slot(&state, &socket_id);
        return;
    }

    let account_name = req.account_name.to_uppercase();

    let doc = match state.db.find_by_account_name(&account_name).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            finish_queue_slot(&state, &socket_id);
            let _ = socket.emit(events::LOGIN_RESPONSE, &codes::INVALID_NAME_PASSWORD);
            return;
        }
        Err(e) => {
            error!(error = %e, "login DB error");
            finish_queue_slot(&state, &socket_id);
            let _ = socket.emit(events::LOGIN_RESPONSE, &codes::INVALID_NAME_PASSWORD);
            return;
        }
    };

    let password_hash = doc
        .get("Password")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let ok = match verify_password(&req.password, &password_hash) {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, "bcrypt verify failed");
            false
        }
    };

    if !socket.connected() {
        finish_queue_slot(&state, &socket_id);
        return;
    }

    if !ok {
        finish_queue_slot(&state, &socket_id);
        let _ = socket.emit(events::LOGIN_RESPONSE, &codes::INVALID_NAME_PASSWORD);
        return;
    }

    // Kick duplicate session (Node: ForceDisconnect + AccountRemove → ChatRoomRemove ServerDisconnect)
    let dup_socket = state.world.read().find_duplicate_login(&account_name);
    if let Some(old_id) = dup_socket {
        if let Some(io) = state.io.get() {
            crate::socket_util::emit_to(
                io,
                &old_id,
                events::FORCE_DISCONNECT,
                &codes::ERROR_DUPLICATED_LOGIN,
            );
        }
        {
            let mut world = state.world.write();
            if let Some(old) = world.get_by_socket(&old_id) {
                if let Some(ref rid) = old.chat_room_id.clone() {
                    let member = old.member_number;
                    crate::room::leave_room_on_disconnect(
                        &mut world,
                        state.io.get(),
                        rid,
                        member,
                        "ServerDisconnect",
                    );
                }
            }
            let _ = world.remove_account(&old_id);
        }
        if let Some(io) = state.io.get() {
            crate::socket_util::disconnect_socket(io, &old_id);
        }
    }

    let environment = crate::auth::account_environment(&socket, &state.config);
    let mut account = OnlineAccount::from_db_doc(socket_id.clone(), environment, doc);
    account.ensure_defaults();

    // Assign MemberNumber if missing
    if account.member_number == 0 {
        let n = state.world.read().allocate_member_number();
        account.member_number = n;
        info!(
            member = n,
            account = %account.account_name,
            "Assigning missing member number"
        );
        if let Err(e) = state.db.set_member_number(&account.account_name, n).await {
            error!(error = %e, "failed to set member number");
        }
    }

    let now = common_time();
    if let Err(e) = state.db.set_last_login(&account.account_name, now).await {
        error!(error = %e, "failed to set last login");
    }

    let member = account.member_number;
    // Build LoginResponse before AccountPurgeInfo-equivalent purge (Node order).
    let response = account.to_login_response();
    account.purge_after_login();
    {
        let mut world = state.world.write();
        world.remove_member_from_all_rooms(member);
        world.login_pending.remove(&socket_id);
        world.login_queue_len = world.login_queue_len.saturating_sub(1);
        world.insert_account(account);
    }

    crate::handlers::on_login(&socket, &state);

    let _ = socket.emit(events::LOGIN_RESPONSE, &response);
    crate::handlers::send_server_info_to(&socket, &state);
}

fn finish_queue_slot(state: &AppState, socket_id: &str) {
    let mut world = state.world.write();
    world.login_pending.remove(socket_id);
    world.login_queue_len = world.login_queue_len.saturating_sub(1);
}
