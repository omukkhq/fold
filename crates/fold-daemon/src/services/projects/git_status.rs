use std::path::PathBuf;

use fold_store::workspaces::remote_states;
pub use fold_store::workspaces::{RemoteFailed as PushFailed, RemotePushed as Pushed};

use super::{ProjectError, load_config};
use crate::projects::ProjectHandle;

/// The project's Git repository and what fold last did with each remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatus {
    /// The project folder, which holds the repository.
    pub repository: PathBuf,
    pub branch: String,
    /// Ordered by name.
    pub remotes: Vec<RemoteStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStatus {
    pub name: String,
    /// The remote's URL or path without credentials.
    pub location: String,
    pub branch: String,
    pub push_on_integrate: bool,
    /// The newest push that reached the remote.
    pub pushed: Option<Pushed>,
    /// The failed push after the newest successful one; `None` once a push succeeded after it.
    pub failed: Option<PushFailed>,
}

/// Reads the repository, the branch and the remotes from the configuration, and the last push of each remote from `remote_states`.
pub async fn git_status(handle: &ProjectHandle) -> Result<GitStatus, ProjectError> {
    let config = load_config(&handle.paths).await?;
    let mut states = remote_states(handle.store.read()).await?;
    let remotes = config
        .remotes
        .into_iter()
        .map(|(name, remote)| {
            let state = states
                .iter()
                .position(|state| state.name == name)
                .map(|index| states.swap_remove(index));
            let (pushed, failed) = state.map_or((None, None), |state| (state.pushed, state.failed));
            RemoteStatus {
                location: public_location(&remote.url),
                name,
                branch: remote.branch,
                push_on_integrate: remote.push_on_integrate,
                pushed,
                failed,
            }
        })
        .collect();
    Ok(GitStatus {
        repository: handle.paths.root().to_owned(),
        branch: config.git.branch,
        remotes,
    })
}

/// `url` without anything that can hold a secret: the user and password of a `scheme://user:password@host/path` URL, its query and its fragment, and the `user:password@` of an `user:password@host:path` address. A path and an `user@host:path` address carry no secret and stay as they are.
pub fn public_location(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return match url.split_once('@') {
            Some((user, address)) if user.contains(':') && !user.contains('/') => {
                address.to_owned()
            }
            _ => url.to_owned(),
        };
    };
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(authority_end);
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    format!("{scheme}://{host}{path}")
}

#[cfg(test)]
mod tests {
    use super::public_location;

    #[test]
    fn a_shown_location_keeps_no_credential() {
        for (url, shown) in [
            ("https://user:token@host/x", "https://host/x"),
            ("https://token@host/x.git", "https://host/x.git"),
            ("https://u:p@host:8443/x", "https://host:8443/x"),
            ("ssh://git@host/x", "ssh://host/x"),
            ("https://host/x?private_token=abc", "https://host/x"),
            ("https://u:p@host#frag", "https://host"),
            ("file:///srv/x.git", "file:///srv/x.git"),
            ("/mnt/backup/x.git", "/mnt/backup/x.git"),
            ("git@host:x.git", "git@host:x.git"),
            ("ana:secret@host:x.git", "host:x.git"),
            ("../shared/x.git", "../shared/x.git"),
        ] {
            assert_eq!(public_location(url), shown, "{url}");
        }
    }
}
