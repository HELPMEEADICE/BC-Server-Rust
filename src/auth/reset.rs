use serde_json::Value;
use socketioxide::extract::{Data, SocketRef, State};
use tracing::{error, info};

use crate::auth::hash_password;
use crate::protocol::codes;
use crate::protocol::events;
use crate::protocol::PasswordResetProcessRequest;
use crate::state::PasswordResetEntry;
use crate::util::{common_time, is_account_name, is_account_password, is_email_valid};
use crate::AppState;

/// Node: `(crypto.randomBytes(6).readUIntBE(0, 6) % 1000000000000).toString()`
fn generate_reset_number() -> String {
    let mut bytes = [0u8; 6];
    for b in &mut bytes {
        *b = rand::random();
    }
    let mut n: u64 = 0;
    for b in bytes {
        n = (n << 8) | u64::from(b);
    }
    (n % 1_000_000_000_000).to_string()
}

pub async fn handle_password_reset(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    // Node: only string email; invalid/non-string → silent no response
    let email = match data {
        Value::String(s) if !s.is_empty() && is_email_valid(&s) => s,
        _ => return,
    };

    // One email reset password per 5 seconds
    let now = common_time();
    {
        let world = state.world.read();
        if now
            < world
                .next_password_reset_at
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            let _ = socket.emit(events::PASSWORD_RESET_RESPONSE, &codes::RETRY_LATER);
            return;
        }
    }
    state
        .world
        .read()
        .next_password_reset_at
        .store(now + 5000, std::sync::atomic::Ordering::SeqCst);

    let names = match state.db.find_account_names_by_email(&email).await {
        Ok(n) => n,
        Err(e) => {
            error!(error = %e, "password reset lookup failed");
            let _ = socket.emit(events::PASSWORD_RESET_RESPONSE, &codes::EMAIL_SENT_ERROR);
            return;
        }
    };

    if names.is_empty() {
        let _ = socket.emit(events::PASSWORD_RESET_RESPONSE, &codes::NO_ACCOUNT_ON_EMAIL);
        return;
    }

    // Build reset numbers for each account (Node: one email body for all)
    let mut pairs: Vec<(String, String)> = Vec::with_capacity(names.len());
    {
        let mut world = state.world.write();
        for account_name in names {
            let reset_number = generate_reset_number();
            world
                .password_resets
                .retain(|e| e.account_name != account_name);
            world.password_resets.push(PasswordResetEntry {
                account_name: account_name.clone(),
                reset_number: reset_number.clone(),
            });
            pairs.push((account_name, reset_number));
        }
    }

    if !state.mailer.smtp_configured() {
        error!(%email, "Password reset requested but SMTP is not configured");
        let _ = socket.emit(events::PASSWORD_RESET_RESPONSE, &codes::EMAIL_SENT_ERROR);
        return;
    }

    match state.mailer.send_password_reset_multi(&email, &pairs).await {
        Ok(()) => {
            info!(%email, count = pairs.len(), "Password reset email sent");
            let _ = socket.emit(events::PASSWORD_RESET_RESPONSE, &codes::EMAIL_SENT);
        }
        Err(e) => {
            error!(error = %e, %email, "send reset email failed");
            let _ = socket.emit(events::PASSWORD_RESET_RESPONSE, &codes::EMAIL_SENT_ERROR);
        }
    }
}

pub async fn handle_password_reset_process(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    let Ok(req) = serde_json::from_value::<PasswordResetProcessRequest>(data) else {
        let _ = socket.emit(
            events::PASSWORD_RESET_RESPONSE,
            &codes::INVALID_PASSWORD_RESET,
        );
        return;
    };

    // Node validates AccountName + NewPassword regex only; ResetNumber is string match
    if !is_account_name(&req.account_name) || !is_account_password(&req.new_password) {
        let _ = socket.emit(
            events::PASSWORD_RESET_RESPONSE,
            &codes::INVALID_PASSWORD_RESET,
        );
        return;
    }

    // Node compares AccountName as sent (create stores uppercased)
    let account_name = req.account_name.clone();

    let valid = {
        let world = state.world.read();
        world
            .password_resets
            .iter()
            .any(|e| e.account_name == account_name && e.reset_number == req.reset_number)
    };

    if !valid {
        let _ = socket.emit(
            events::PASSWORD_RESET_RESPONSE,
            &codes::INVALID_PASSWORD_RESET,
        );
        return;
    }

    let hash = match hash_password(&req.new_password) {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "hash new password failed");
            let _ = socket.emit(
                events::PASSWORD_RESET_RESPONSE,
                &codes::INVALID_PASSWORD_RESET,
            );
            return;
        }
    };

    if let Err(e) = state.db.set_password(&account_name, &hash).await {
        error!(error = %e, "set password failed");
        let _ = socket.emit(
            events::PASSWORD_RESET_RESPONSE,
            &codes::INVALID_PASSWORD_RESET,
        );
        return;
    }

    // Node leaves reset entry in memory; clearing is safer and still accepts one use
    {
        let mut world = state.world.write();
        world
            .password_resets
            .retain(|e| e.account_name != account_name);
    }

    info!(account = %account_name, "Password reset successful");
    let _ = socket.emit(
        events::PASSWORD_RESET_RESPONSE,
        &codes::PASSWORD_RESET_SUCCESS,
    );
}
