use std::time::Duration;

use fold_core::projects::notify::{EVENT_HEADER, Notice, SIGNATURE_HEADER, SmtpSecurity, sign};
use fold_core::{FoldError, codes};
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};

use super::settings::{Channel, EmailChannel, Secret};

/// The longest a send waits for its receiver or SMTP server.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(20);

fn failed(message: String) -> FoldError {
    FoldError::new(codes::NOTIFY_SEND_FAILED, message)
}

/// Sends `notice` through `channel`, blocking until the receiver or the SMTP server took it. The error says what failed in words of fold's own and never repeats the URL, an address, a secret or a server's answer text.
pub fn deliver(channel: &Channel, notice: &Notice) -> Result<(), FoldError> {
    match channel {
        Channel::Webhook { url, secret } => post(url, secret, notice),
        Channel::Email(email) => send_email(email, notice),
    }
}

/// Posts the JSON body with its signature. A redirect is not followed, so the signed body goes only to the URL a human gave.
fn post(url: &str, secret: &Secret, notice: &Notice) -> Result<(), FoldError> {
    let body = notice.body()?;
    let signature = sign(secret.expose().as_bytes(), &body)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(SEND_TIMEOUT))
        .max_redirects(0)
        .build()
        .into();
    agent
        .post(url)
        .header("content-type", "application/json")
        .header("user-agent", concat!("fold/", env!("CARGO_PKG_VERSION")))
        .header(SIGNATURE_HEADER, &signature)
        .header(EVENT_HEADER, notice.event.as_str())
        .send(&body[..])
        .map(|_| ())
        .map_err(|error| {
            failed(match error {
                ureq::Error::StatusCode(status) => {
                    format!("the webhook receiver answered HTTP {status}")
                }
                ureq::Error::Timeout(_) => "the webhook receiver did not answer in time".to_owned(),
                ureq::Error::HostNotFound => "the webhook host name was not found".to_owned(),
                ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => {
                    "the TLS connection to the webhook receiver failed".to_owned()
                }
                _ => "the webhook receiver could not be reached".to_owned(),
            })
        })
}

fn send_email(email: &EmailChannel, notice: &Notice) -> Result<(), FoldError> {
    let address = |text: &str| {
        text.parse::<Mailbox>()
            .map_err(|_| failed("an email address of the target is not valid".to_owned()))
    };
    let message = Message::builder()
        .from(address(&email.from)?)
        .to(address(&email.to)?)
        .subject(notice.subject())
        .header(ContentType::TEXT_PLAIN)
        .body(notice.text())
        .map_err(|_| failed("the email could not be built".to_owned()))?;
    let builder = match email.security {
        SmtpSecurity::Tls => SmtpTransport::relay(&email.smtp_host),
        SmtpSecurity::Starttls => SmtpTransport::starttls_relay(&email.smtp_host),
        SmtpSecurity::None => Ok(SmtpTransport::builder_dangerous(email.smtp_host.as_str())),
    }
    .map_err(|_| failed("the SMTP host is not valid for TLS".to_owned()))?
    .port(email.smtp_port)
    .timeout(Some(SEND_TIMEOUT));
    let builder = match (&email.username, &email.password) {
        (Some(user), Some(password)) => {
            builder.credentials(Credentials::new(user.clone(), password.expose().to_owned()))
        }
        _ => builder,
    };
    builder.build().send(&message).map(|_| ()).map_err(|error| {
        failed(match error.status() {
            Some(code) => format!("the SMTP server answered {code}"),
            None if error.is_timeout() => "the SMTP server did not answer in time".to_owned(),
            None if error.is_tls() => "the TLS connection to the SMTP server failed".to_owned(),
            None => "the SMTP server could not be reached".to_owned(),
        })
    })
}
