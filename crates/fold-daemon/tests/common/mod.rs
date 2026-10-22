pub mod acceptance;
pub mod invariants;
pub mod roles;
pub mod sse;

/// The questions to humans that no human has answered in their thread yet, in number order, each with its task, as the needs-you list reads them.
pub async fn awaiting_human(store: &fold_store::Store) -> Vec<fold_store::messages::MessageRow> {
    let mut conn = store.read().acquire().await.unwrap();
    fold_store::messages::awaiting_human(&mut conn)
        .await
        .unwrap()
}

/// The human every init of a test runs as.
pub fn ana() -> fold_core::HumanName {
    fold_core::HumanName::parse("ana").unwrap()
}

/// `fld init --with-git` in a new folder `root` that holds a `.gitignore` that ignores `.fold/` already: the folder's files, the `.gitignore` among them, become `r1`, and Git sync is on with no other revision.
pub async fn init_with_git(
    host: &fold_core::HostPaths,
    root: &std::path::Path,
    clock: &dyn fold_core::Clock,
) -> Result<
    fold_daemon::services::projects::InitializedProject,
    fold_daemon::services::projects::ProjectError,
> {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join(".gitignore"), ".fold/\n").unwrap();
    fold_daemon::services::projects::init_new_project(
        host,
        root,
        None,
        &ana(),
        fold_vcs::git::Git::default(),
        None,
        Some(Box::new(|_: &std::path::Path| {})),
        clock,
    )
    .await
}

/// The `Cookie` value of a new web session of `human` in the registry of `state`, made as a sign-in makes one: a token, then a session from it.
pub async fn session_cookie(state: &fold_daemon::daemon::AppState, human: &str) -> String {
    let human = fold_core::HumanName::parse(human).unwrap();
    let token = fold_daemon::services::projects::create_web_token(
        &state.registry,
        &human,
        None,
        state.clock.now(),
    )
    .await
    .unwrap();
    let signed_in = fold_daemon::services::projects::create_web_session(
        &state.registry,
        state.clock.as_ref(),
        state.session_lifetime,
        &token,
    )
    .await
    .unwrap()
    .unwrap();
    format!(
        "{}={}",
        fold_daemon::http::auth::SESSION_COOKIE,
        signed_in.session
    )
}

/// The TCP router of `state` with every request signed in as `human`: each request gets the cookie of one real session, as a browser sends it after a sign-in.
pub async fn signed_in_router(state: &fold_daemon::daemon::AppState, human: &str) -> axum::Router {
    let cookie = axum::http::HeaderValue::from_str(&session_cookie(state, human).await).unwrap();
    fold_daemon::http::router(state.clone()).layer(axum::middleware::map_request(
        move |mut request: axum::extract::Request| {
            let cookie = cookie.clone();
            async move {
                request
                    .headers_mut()
                    .append(axum::http::header::COOKIE, cookie);
                request
            }
        },
    ))
}
