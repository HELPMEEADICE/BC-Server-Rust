use socketioxide::extract::{SocketRef, State};
use socketioxide::SocketIo;
use tracing::{info, warn};

use crate::account;
use crate::auth;
use crate::chat;
use crate::limits::{check_ip_connection_limit, extract_ip, on_ip_disconnect, MessageRateLimiter};
use crate::protocol::codes;
use crate::protocol::events;
use crate::protocol::ServerInfoMessage;
use crate::relations;
use crate::room;
use crate::util::common_time;
use crate::AppState;

/// Register pre-auth handlers and rate limits on new connection.
pub fn on_connection(socket: SocketRef, state: AppState) {
    let ip = {
        let xff = socket
            .req_parts()
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok());
        // Prefer last hop of XFF; fall back to connection peer if available via extensions later
        extract_ip(None, xff)
    };

    {
        let mut world = state.world.write();
        if check_ip_connection_limit(
            &mut world,
            &ip,
            state.config.ip_connection_limit,
            state.config.ip_connection_rate_limit,
        ) {
            warn!(%ip, "Rejecting connection (IP limit)");
            let _ = socket.emit(events::FORCE_DISCONNECT, &codes::ERROR_RATE_LIMITED);
            let _ = socket.clone().disconnect();
            return;
        }
    }

    let limiter = MessageRateLimiter::new(state.config.client_message_rate_limit);
    socket.extensions.insert(limiter);
    socket.extensions.insert(ClientIp(ip.clone()));

    {
        let st = state.clone();
        let sid = socket.id.to_string();
        let ip2 = ip.clone();
        socket.on_disconnect(move || {
            let st = st.clone();
            let sid = sid.clone();
            let ip2 = ip2.clone();
            async move {
                on_disconnect_cleanup(&st, &sid, &ip2).await;
            }
        });
    }

    // Pre-login only (Node parity). Rate limit applied inside each handler.
    socket.on(events::ACCOUNT_CREATE, rate_limited(auth::handle_account_create));
    socket.on(events::ACCOUNT_LOGIN, rate_limited(auth::handle_account_login));
    socket.on(events::PASSWORD_RESET, rate_limited(auth::handle_password_reset));
    socket.on(
        events::PASSWORD_RESET_PROCESS,
        rate_limited(auth::handle_password_reset_process),
    );

    send_server_info_to(&socket, &state);
    info!(id = %socket.id, %ip, "Client connected");
}

/// After successful login/create: strip pre-auth handlers, register post-login handlers.
pub fn on_login(socket: &SocketRef, _state: &AppState) {
    // Node removes pre-login listeners. socketioxide replaces handlers on re-register;
    // re-bind pre-login events to no-ops so they cannot re-auth after login.
    socket.on(events::ACCOUNT_CREATE, noop_handler);
    socket.on(events::ACCOUNT_LOGIN, noop_handler);
    socket.on(events::PASSWORD_RESET, noop_handler);
    socket.on(events::PASSWORD_RESET_PROCESS, noop_handler);

    socket.on(
        events::ACCOUNT_UPDATE,
        rate_limited(account::handle_account_update),
    );
    socket.on(
        events::ACCOUNT_UPDATE_EMAIL,
        rate_limited(account::handle_account_update_email),
    );
    socket.on(
        events::ACCOUNT_QUERY,
        rate_limited(account::handle_account_query),
    );
    socket.on(
        events::ACCOUNT_BEEP_EVT,
        rate_limited(account::handle_account_beep),
    );
    socket.on(
        events::ACCOUNT_OWNERSHIP_EVT,
        rate_limited(relations::handle_account_ownership),
    );
    socket.on(
        events::ACCOUNT_LOVERSHIP_EVT,
        rate_limited(relations::handle_account_lovership),
    );
    socket.on(
        events::ACCOUNT_DIFFICULTY,
        rate_limited(account::handle_account_difficulty),
    );
    socket.on(events::ACCOUNT_DISCONNECT, handle_account_disconnect);
    socket.on(
        events::CHAT_ROOM_SEARCH,
        rate_limited(room::handle_chat_room_search),
    );
    socket.on(
        events::CHAT_ROOM_CREATE,
        rate_limited(room::handle_chat_room_create),
    );
    socket.on(
        events::CHAT_ROOM_JOIN,
        rate_limited(room::handle_chat_room_join),
    );
    socket.on(events::CHAT_ROOM_LEAVE, rate_limited_leave);
    socket.on(
        events::CHAT_ROOM_CHAT,
        rate_limited(chat::handle_chat_room_chat),
    );
    socket.on(
        events::CHAT_ROOM_CHARACTER_UPDATE,
        rate_limited(chat::handle_character_update),
    );
    socket.on(
        events::CHAT_ROOM_CHARACTER_EXPRESSION_UPDATE,
        rate_limited(chat::handle_expression_update),
    );
    socket.on(
        events::CHAT_ROOM_CHARACTER_POSE_UPDATE,
        rate_limited(chat::handle_pose_update),
    );
    socket.on(
        events::CHAT_ROOM_CHARACTER_AROUSAL_UPDATE,
        rate_limited(chat::handle_arousal_update),
    );
    socket.on(
        events::CHAT_ROOM_CHARACTER_ITEM_UPDATE,
        rate_limited(chat::handle_item_update),
    );
    socket.on(
        events::CHAT_ROOM_CHARACTER_MAP_DATA_UPDATE,
        rate_limited(chat::handle_map_data_update),
    );
    socket.on(
        events::CHAT_ROOM_ADMIN,
        rate_limited(room::admin::handle_chat_room_admin),
    );
    socket.on(
        events::CHAT_ROOM_ALLOW_ITEM_EVT,
        rate_limited(chat::handle_allow_item),
    );
    socket.on(
        events::CHAT_ROOM_GAME,
        rate_limited(chat::handle_chat_room_game),
    );
}

async fn noop_handler(_socket: SocketRef) {}

async fn rate_limited_leave(socket: SocketRef, State(state): State<AppState>) {
    if !check_message_rate(&socket) {
        return;
    }
    room::handle_chat_room_leave(socket, State(state)).await;
}

/// Wrap is not possible generically without type erasure; handlers call check_message_rate directly.
/// This helper is a no-op identity for registration clarity when handler already rates itself.
fn rate_limited<H>(handler: H) -> H {
    handler
}

async fn handle_account_disconnect(socket: SocketRef, State(state): State<AppState>) {
    if !check_message_rate(&socket) {
        return;
    }
    let sid = socket.id.to_string();
    let ip = socket
        .extensions
        .get::<ClientIp>()
        .map(|c| c.0)
        .unwrap_or_else(|| "unknown".into());
    on_disconnect_cleanup(&state, &sid, &ip).await;
    let _ = socket.clone().disconnect();
}

async fn on_disconnect_cleanup(state: &AppState, socket_id: &str, ip: &str) {
    let delayed = {
        let mut world = state.world.write();
        if let Some(acc) = world.get_by_socket_mut(socket_id) {
            Some((
                acc.account_name.clone(),
                acc.delayed_appearance.take(),
                acc.delayed_skill.take(),
                acc.delayed_game.take(),
            ))
        } else {
            None
        }
    };

    if let Some((name, appearance, skill, game)) = delayed {
        let mut map = serde_json::Map::new();
        if let Some(a) = appearance {
            map.insert("Appearance".into(), a);
        }
        if let Some(s) = skill {
            map.insert("Skill".into(), s);
        }
        if let Some(g) = game {
            map.insert("Game".into(), g);
        }
        if !map.is_empty() {
            if let Ok(doc) = crate::db::json_object_to_set_map(&serde_json::Value::Object(map)) {
                let _ = state.db.update_fields(&name, doc).await;
            }
        }
    }

    {
        let mut world = state.world.write();
        if let Some(acc) = world.remove_account(socket_id) {
            if let Some(ref rid) = acc.chat_room_id {
                if let Some(room) = world.chat_rooms.get_mut(rid) {
                    room.members.retain(|&m| m != acc.member_number);
                    let empty = room.members.is_empty();
                    if empty {
                        world.remove_room(rid);
                    }
                }
            }
            info!(account = %acc.account_name, "Account disconnected");
        }
        on_ip_disconnect(&mut world, ip);
    }
}

pub fn send_server_info_to(socket: &SocketRef, state: &AppState) {
    let online = state.world.read().online_count();
    let info = ServerInfoMessage {
        time: common_time(),
        online_players: online,
    };
    let _ = socket.emit(events::SERVER_INFO, &info);
}

pub fn broadcast_server_info(io: &SocketIo, state: &AppState) {
    let online = state.world.read().online_count();
    let info = ServerInfoMessage {
        time: common_time(),
        online_players: online,
    };
    crate::socket_util::emit_io(io, events::SERVER_INFO, &info);
}

pub async fn graceful_shutdown_message(io: &SocketIo) {
    let _ = io
        .emit(
            events::SERVER_MESSAGE,
            &"Server will reboot in 30 seconds.",
        )
        .await;
}

/// Per-socket client IP stored in extensions.
#[derive(Clone)]
pub struct ClientIp(pub String);

/// Check message rate limit; disconnect and return false if exceeded.
pub fn check_message_rate(socket: &SocketRef) -> bool {
    if let Some(mut lim) = socket.extensions.get::<MessageRateLimiter>() {
        if lim.check() {
            let _ = socket.emit(events::FORCE_DISCONNECT, &codes::ERROR_RATE_LIMITED);
            let _ = socket.clone().disconnect();
            return false;
        }
        socket.extensions.insert(lim);
    }
    true
}
