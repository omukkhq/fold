use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use fold_cli::daemon_client::{DaemonClient, Response};
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
    Ok(vec![
        url.to_owned(),
        "open the link in a browser on this machine within 5 minutes; it signs you in once"
            .to_owned(),
    ])
}
