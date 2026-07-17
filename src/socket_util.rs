use serde::Serialize;
use serde_json::Value;
use socketioxide::extract::SocketRef;
use socketioxide::SocketIo;

/// Look up a connected socket by its string id.
pub fn get_socket(io: &SocketIo, id: &str) -> Option<SocketRef> {
    io.sockets().into_iter().find(|s| s.id.to_string() == id)
}

/// Look up socket via any peer socket's room operators (same namespace).
pub fn get_socket_from_ref(peer: &SocketRef, id: &str) -> Option<SocketRef> {
    peer.broadcast()
        .sockets()
        .into_iter()
        .find(|s| s.id.to_string() == id)
}

pub fn emit_to(io: &SocketIo, id: &str, event: &str, data: &impl Serialize) {
    if let Some(s) = get_socket(io, id) {
        let _ = s.emit(event, data);
    }
}

pub fn disconnect_socket(io: &SocketIo, id: &str) {
    if let Some(s) = get_socket(io, id) {
        let _ = s.disconnect();
    }
}

fn owned_payload(data: &impl Serialize) -> Value {
    serde_json::to_value(data).unwrap_or(Value::Null)
}

/// Broadcast to everyone in `room` (including the calling socket).
/// socketioxide broadcast `emit` returns a Future that must be polled.
pub fn emit_within(socket: &SocketRef, room: impl Into<String>, event: &str, data: &impl Serialize) {
    let socket = socket.clone();
    let room = room.into();
    let event = event.to_string();
    let data = owned_payload(data);
    tokio::spawn(async move {
        let _ = socket.within(room).emit(event.as_str(), &data).await;
    });
}

/// Broadcast to everyone in `room` except the calling socket (Node `socket.to(room)`).
pub fn emit_to_room(socket: &SocketRef, room: impl Into<String>, event: &str, data: &impl Serialize) {
    let socket = socket.clone();
    let room = room.into();
    let event = event.to_string();
    let data = owned_payload(data);
    tokio::spawn(async move {
        let _ = socket.to(room).emit(event.as_str(), &data).await;
    });
}

/// Namespace-wide broadcast (must be polled).
pub fn emit_io(io: &SocketIo, event: &str, data: &impl Serialize) {
    let io = io.clone();
    let event = event.to_string();
    let data = owned_payload(data);
    tokio::spawn(async move {
        let _ = io.emit(event.as_str(), &data).await;
    });
}
