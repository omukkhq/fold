use fold_core::{ProjectConfig, ProjectPaths};
use fold_vcs::git::Git;
use fold_vcs::git_export::{Signature, repository_identity};

use super::{Check, on_path};
use crate::output;
use crate::platform::Host;

const IDENTITY: &str = "git identity";
const UPSTREAM: &str = "git upstream";

/// The upstream Git's config names for a local branch: `branch.<b>.remote` and `branch.<b>.merge`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Upstream {
    pub(crate) remote: String,
    pub(crate) merge: String,
}

/// Whether the Git commits of fold main carry a person's identity. fold reads it as the integration does, so the check names the author the next commit gets.
pub(super) fn identity_check(paths: &ProjectPaths, host: &Host) -> Check {
    let Some(program) = on_path("git", host.path.as_deref()) else {
        return Check::not_applicable(IDENTITY);
    };
    if !paths.git_dir().is_dir() {
        return Check::not_applicable(IDENTITY);
    }
    match repository_identity(&Git::new(program), &paths.git_dir()) {
        Ok(identity) => identity_verdict(&identity),
        Err(_) => Check::warn(
            IDENTITY,
            "git cannot read user.name and user.email of the project folder",
            "run `git config user.email` in the project folder",
        ),
    }
}

/// `identity` is the author and committer of the next Git commit of fold main.
pub(crate) fn identity_verdict(identity: &Signature) -> Check {
    if *identity == Signature::fold() {
        let fold = Signature::fold();
        return Check::warn(
            IDENTITY,
            format!(
                "fold writes the Git commits of fold main as `{} <{}>`, because Git names no valid user.name and user.email for the project folder",
                fold.name, fold.email
            ),
            "run `git config --global user.name \"Your Name\"` and `git config --global user.email you@example.com`, or the same with --local in the project folder; the next integration commits with them",
        );
    }
    Check::ok(
        IDENTITY,
        format!(
            "fold writes the Git commits of fold main as {} <{}>",
            output::escape_control(&identity.name),
            output::escape_control(&identity.email)
        ),
    )
}

/// Whether the upstream that plain `git pull` and `git push` use for the local branch is a remote branch that fold syncs it with.
pub(super) fn upstream_check(paths: &ProjectPaths, host: &Host) -> Check {
    let Some(program) = on_path("git", host.path.as_deref()) else {
        return Check::not_applicable(UPSTREAM);
    };
    if !paths.git_dir().is_dir() {
        return Check::not_applicable(UPSTREAM);
    }
    let config = std::fs::read_to_string(paths.config())
        .ok()
        .or_else(|| (!paths.config().exists()).then(String::new))
        .and_then(|text| ProjectConfig::from_toml(&text).ok());
    let Some(config) = config else {
        return Check::warn(
            UPSTREAM,
            "config.toml cannot be read, so doctor cannot compare the upstream of the local branch",
            "fix .fold/config.toml, then run fld doctor again",
        );
    };
    let git = Git::new(program);
    let local = &config.git.branch;
    let value = |key: &str| {
        git.config_get(&paths.git_dir(), &format!("branch.{local}.{key}"))
            .map(|value| value.filter(|value| !value.is_empty()))
    };
    let upstream = match (value("remote"), value("merge")) {
        (Ok(Some(remote)), Ok(Some(merge))) => Some(Upstream { remote, merge }),
        (Ok(_), Ok(_)) => None,
        _ => {
            return Check::warn(
                UPSTREAM,
                "git cannot read the upstream of the local branch",
                "run `git branch -vv` in the project folder",
            );
        }
    };
    let remotes: Vec<(String, String)> = config
        .remotes
        .iter()
        .map(|(name, remote)| (name.clone(), remote.branch.clone()))
        .collect();
    upstream_verdict(local, upstream.as_ref(), &remotes)
}

/// `local` is the `[git] branch`, `upstream` what Git's config names for it and `remotes` each remote of the config with its branch. fold never sets an upstream, so only one that is set and names no remote branch fold syncs with is a problem: plain `git pull` and `git push` would then use another branch than `fld pull` and `fld push`.
pub(crate) fn upstream_verdict(
    local: &str,
    upstream: Option<&Upstream>,
    remotes: &[(String, String)],
) -> Check {
    let local_shown = output::escape_control(local);
    let Some(upstream) = upstream else {
        return Check::ok(
            UPSTREAM,
            format!(
                "the local branch `{local_shown}` has no upstream; fld push and fld pull sync it"
            ),
        );
    };
    let merge = upstream
        .merge
        .strip_prefix("refs/heads/")
        .unwrap_or(&upstream.merge);
    let tracked = format!(
        "{}/`{}`",
        output::escape_control(&upstream.remote),
        output::escape_control(merge)
    );
    let synced = remotes
        .iter()
        .find(|(name, _)| *name == upstream.remote)
        .map(|(_, branch)| branch);
    match synced {
        Some(branch) if branch == merge => Check::ok(
            UPSTREAM,
            format!("the local branch `{local_shown}` tracks {tracked}, which fold syncs it with"),
        ),
        Some(branch) => {
            let remote = output::escape_control(&upstream.remote);
            let branch = output::escape_control(branch);
            Check::warn(
                UPSTREAM,
                format!(
                    "the local branch `{local_shown}` tracks {tracked}, but fold syncs it with {remote}/`{branch}`, so git pull and git push use another branch than fld pull and fld push"
                ),
                format!(
                    "run `fld pull {remote}`, then `git branch --set-upstream-to={remote}/{branch} {local_shown}` in the project folder, or `git branch --unset-upstream {local_shown}`"
                ),
            )
        }
        None => Check::warn(
            UPSTREAM,
            format!(
                "the local branch `{local_shown}` tracks {tracked}, which no remote of .fold/config.toml names, so git pull and git push use another branch than fld pull and fld push"
            ),
            format!(
                "run `git branch --unset-upstream {local_shown}` in the project folder, or `fld remote add` for that remote"
            ),
        ),
    }
}
