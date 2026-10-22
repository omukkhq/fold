use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use fold_cli::daemon_client::{DaemonClient, Response};
use fold_core::Timestamp;
use hyper::Method;

use super::remote::{RemoteError, refusal};
use crate::output;
use crate::serve::{self, Dirs};

#[derive(Args, Debug)]
pub struct WebArgs {
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
}

/// Asks the daemon, starting it when none runs, for a one-time sign-in link for the human in the environment, and prints it.
pub fn run(args: &WebArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = sign_in_link(
        args.socket.as_deref(),
        &Dirs::from_env(),
        crate::fold_name::current(),
    )?;
    for line in summary(&response)? {
        output::line(out, &line)?;
    }
    Ok(())
}

/// Sends `POST /api/v1/sign-in-links` over the socket as `human`.
pub(crate) fn sign_in_link(
    socket: Option<&Path>,
    dirs: &Dirs,
    human: Option<String>,
) -> Result<Response, RemoteError> {
    let config = serve::client_config(socket, dirs)?;
    let client = DaemonClient::new(config, human);
    let runtime = serve::current_thread_runtime().map_err(RemoteError::Runtime)?;
    Ok(runtime.block_on(client.request(Method::POST, "/api/v1/sign-in-links", None))?)
}

/// The link on a line of its own, so a script can take it, then what it does.
pub(crate) fn summary(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let url = response.body["url"].as_str().ok_or(RemoteError::Answer)?;
    let expiry = expiry(&response.body).ok_or(RemoteError::Answer)?;
    Ok(vec![
        url.to_owned(),
        format!("open the link in a browser on this machine {expiry}; it signs you in once"),
    ])
}

/// How long the sign-in link in `body` lasts, by the daemon's clock, and when it expires in UTC to the second, such as `within 5 minutes, before 2026-10-22 10:05:00 UTC`.
pub(crate) fn expiry(body: &serde_json::Value) -> Option<String> {
    let at = body["expires_at"]
        .as_i64()
        .and_then(|millis| Timestamp::from_millis(millis).ok())?
        .to_string();
    let minutes = body["expires_in_secs"].as_u64()?.div_ceil(60);
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    Some(format!(
        "within {minutes} {unit}, before {} {} UTC",
        at.get(..10)?,
        at.get(11..19)?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_expiry_names_the_minutes_left_and_the_utc_time_to_the_second() {
        let body = json!({"expires_at": 1_792_663_500_123_i64, "expires_in_secs": 300});
        assert_eq!(
            expiry(&body).unwrap(),
            "within 5 minutes, before 2026-10-22 10:05:00 UTC"
        );
        let body = json!({"expires_at": 1_792_663_500_123_i64, "expires_in_secs": 30});
        assert_eq!(
            expiry(&body).unwrap(),
            "within 1 minute, before 2026-10-22 10:05:00 UTC"
        );
        assert_eq!(expiry(&json!({"expires_at": 1})), None);
        assert_eq!(expiry(&json!({"expires_in_secs": 300})), None);
    }
}
