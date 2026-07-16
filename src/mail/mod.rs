use anyhow::Result;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use tracing::{error, info};

use crate::config::Config;

#[derive(Clone)]
pub struct Mailer {
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    from: String,
    admin: String,
}

impl Mailer {
    pub fn new(config: &Config) -> Self {
        let transport = if config.email_password.is_empty() {
            None
        } else {
            match AsyncSmtpTransport::<Tokio1Executor>::relay(&config.email_host) {
                Ok(builder) => {
                    let creds =
                        Credentials::new(config.email_user.clone(), config.email_password.clone());
                    Some(
                        builder
                            .port(config.email_port)
                            .credentials(creds)
                            .build(),
                    )
                }
                Err(e) => {
                    error!(error = %e, "Failed to build SMTP transport");
                    None
                }
            }
        };

        Self {
            transport,
            from: config.email_from.clone(),
            admin: config.email_admin.clone(),
        }
    }

    pub async fn send(&self, to: &str, subject: &str, html: &str) -> Result<()> {
        let Some(transport) = &self.transport else {
            info!(%to, %subject, "Email skipped (no SMTP credentials)");
            return Ok(());
        };
        if to.is_empty() {
            return Ok(());
        }

        let email = Message::builder()
            .from(self.from.parse::<Mailbox>()?)
            .to(to.parse::<Mailbox>()?)
            .subject(subject)
            .header(lettre::message::header::ContentType::TEXT_HTML)
            .body(html.to_string())?;

        transport.send(email).await?;
        info!(%to, %subject, "Email sent");
        Ok(())
    }

    pub async fn send_password_reset(&self, to: &str, account_name: &str, reset_number: &str) -> Result<()> {
        let html = format!(
            "Password reset code for account <b>{}</b>:<br/><br/><b>{}</b><br/><br/>If you did not request this, ignore this email.",
            account_name, reset_number
        );
        self.send(to, "Bondage Club Password Reset", &html).await
    }

    pub async fn send_admin_alert(&self, subject: &str, html: &str) {
        if self.admin.is_empty() {
            return;
        }
        if let Err(e) = self.send(&self.admin, subject, html).await {
            error!(error = %e, "Failed to send admin email");
        }
    }
}
