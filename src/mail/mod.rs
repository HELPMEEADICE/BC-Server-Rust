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
                    Some(builder.port(config.email_port).credentials(creds).build())
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

    pub fn smtp_configured(&self) -> bool {
        self.transport.is_some()
    }

    pub async fn send(&self, to: &str, subject: &str, html: &str) -> Result<()> {
        let Some(transport) = &self.transport else {
            anyhow::bail!("SMTP not configured");
        };
        if to.is_empty() {
            anyhow::bail!("empty recipient");
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

    /// Node: one email listing all AccountName + ResetNumber pairs for that address.
    pub async fn send_password_reset_multi(
        &self,
        to: &str,
        accounts: &[(String, String)],
    ) -> Result<()> {
        let mut html = String::from(
            "To reset your account password, enter your account name and the reset number included in this email.  You need to put these in the Bondage Club password reset screen, with your new password.<br /><br />",
        );
        for (account_name, reset_number) in accounts {
            html.push_str(&format!("Account Name: {}<br />", account_name));
            html.push_str(&format!("Reset Number: {}<br /><br />", reset_number));
        }
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
