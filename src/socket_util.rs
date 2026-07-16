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

pub fn emit_to(io: &SocketIo, id: &str, event: &str, data: &impl serde::Serialize) {
    if let Some(s) = get_socket(io, id) {
        let _ = s.emit(event, data);
    }
}

pub fn disconnect_socket(io: &SocketIo, id: &str) {
    if let Some(s) = get_socket(io, id) {
        let _ = s.disconnect();
    }
}
