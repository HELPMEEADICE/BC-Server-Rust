use serde_json::Value;
use socketioxide::extract::{Data, SocketRef, State};
use tracing::{error, info};

use crate::auth::{hash_password, verify_password};
use crate::protocol::codes;
use crate::protocol::events;
use crate::protocol::PasswordResetProcessRequest;
use crate::state::PasswordResetEntry;
use crate::util::{common_time, is_account_name, is_account_password, is_email_valid, is_reset_number};
use crate::AppState;

pub async fn handle_password_reset(
    socket: SocketRef,
    Data(data): Data<Value>,
    State(state): State<AppState>,
) {
    if !crate::handlers::check_message_rate(&socket) {
        return;
    }
    // Rate limit resets (NextPasswordReset style — 1 per few seconds globally simplified)
    let now = common_time();
    {
        let world = state.world.read();
        if now < world.next_password_reset_at.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = socket.emit(events::PASSWORD_RESET_RESPONSE, &codes::RETRY_LATER);
            return;
        }
    }
    state
        .world
        .read()
        .next_password_reset_at
        .store(now + 5000, std::sync::atomic::Ordering::SeqCst);

    let email = match data {
        Value::String(s) => s,
        other => other
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_default(),
    };

    if email.is_empty() || !is_email_valid(&email) {
        let _ = socket.emit(events::PASSWORD_RESET_RESPONSE, &codes::NO_ACCOUNT_ON_EMAIL);
        return;
    }

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

    // Generate reset numbers for each account
    let mut sent_any = false;
    for account_name in names {
        let reset_number: String = (0..7)
            .map(|_| (rand::random::<u8>() % 10).to_string())
            .collect::<String>();

        {
            let mut world = state.world.write();
            // Replace existing entry for account
            world
                .password_resets
                .retain(|e| e.account_name != account_name);
            world.password_resets.push(PasswordResetEntry {
                account_name: account_name.clone(),
                reset_number: reset_number.clone(),
            });
        }

        match state
            .mailer
            .send_password_reset(&email, &account_name, &reset_number)
            .await
        {
            Ok(()) => sent_any = true,
            Err(e) => error!(error = %e, account = %account_name, "send reset email failed"),
        }
    }

    if sent_any {
        info!(%email, "Password reset email sent");
        let _ = socket.emit(events::PASSWORD_RESET_RESPONSE, &codes::EMAIL_SENT);
    } else {
        let _ = socket.emit(events::PASSWORD_RESET_RESPONSE, &codes::EMAIL_SENT_ERROR);
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

    if !is_account_name(&req.account_name)
        || !is_reset_number(&req.reset_number)
        || !is_account_password(&req.new_password)
    {
        let _ = socket.emit(
            events::PASSWORD_RESET_RESPONSE,
            &codes::INVALID_PASSWORD_RESET,
        );
        return;
    }

    let account_name = req.account_name.to_uppercase();

    let valid = {
        let world = state.world.read();
        world.password_resets.iter().any(|e| {
            e.account_name == account_name && e.reset_number == req.reset_number
        })
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

    {
        let mut world = state.world.write();
        world
            .password_resets
            .retain(|e| e.account_name != account_name);
    }

    // silence unused import if verify not needed
    let _ = verify_password as fn(&str, &str) -> anyhow::Result<bool>;

    info!(account = %account_name, "Password reset successful");
    let _ = socket.emit(
        events::PASSWORD_RESET_RESPONSE,
        &codes::PASSWORD_RESET_SUCCESS,
    );
}
