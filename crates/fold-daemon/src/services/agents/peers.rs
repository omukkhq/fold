use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use fold_core::policy::{self, SessionPeer};
use fold_core::{AgentName, FoldError, codes};
use fold_store::agents;

use super::AgentError;
use crate::projects::Projects;
use crate::support::lock;

/// The most parents a walk follows. A real process tree is far shallower; the bound only ends a walk through a table that changed under it.
const MAX_DEPTH: usize = 64;

/// A worker that fold started, of any open project, whose process is the socket peer or one of its ancestors, or a check command that fold runs. A check acts for the worker that asked for its run, and `agent` is `None` for a check that fold runs for no worker, such as an integration check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerWorker {
    pub project: String,
    pub agent: Option<AgentName>,
}

/// The check commands this daemon runs, by the pid of each command's shell, which leads the command's process group: the registration's token and the worker the check acts for.
static CHECKS: LazyLock<Mutex<HashMap<u32, (u64, PeerWorker)>>> = LazyLock::new(Mutex::default);

static NEXT_CHECK: AtomicU64 = AtomicU64::new(0);

/// A check command's place in the registry the peer checks read, from its spawn until it is dropped. A check runs commands from a file that a worker can change, so a process under it must not pass for the user's terminal. Dropping removes only this registration, so a later check whose shell got the same pid keeps its own.
#[derive(Debug)]
pub(crate) struct CheckPeer {
    pid: u32,
    token: u64,
}

impl Drop for CheckPeer {
    fn drop(&mut self) {
        let mut checks = lock(&CHECKS);
        if checks
            .get(&self.pid)
            .is_some_and(|(token, _)| *token == self.token)
        {
            checks.remove(&self.pid);
        }
    }
}

/// Registers the shell `pid` of a check command that this daemon spawned, acting for `owner`.
pub(crate) fn register_check(pid: u32, owner: PeerWorker) -> CheckPeer {
    let token = NEXT_CHECK.fetch_add(1, Ordering::Relaxed);
    lock(&CHECKS).insert(pid, (token, owner));
    CheckPeer { pid, token }
}

/// The check that `chain`, a process and its ancestors nearest first, runs under. A registered pid counts only while this daemon is its parent, so a pid that an ended check's shell left and another process took never matches.
pub fn check_peer(chain: &[u32]) -> Option<PeerWorker> {
    let checks = lock(&CHECKS);
    let daemon = std::process::id();
    chain.iter().enumerate().find_map(|(at, pid)| {
        let parent = chain.get(at + 1).copied().unwrap_or(1);
        checks
            .get(pid)
            .filter(|_| parent == daemon)
            .map(|(_, owner)| owner.clone())
    })
}

/// The worker whose process tree holds the process `pid`, or the check command it runs under, if any. Without a live worker process or a running check on the host it reads no process table. A process table that cannot be read fails the lookup, so a peer is never counted outside every worker only because its parents are unknown. A peer that forked twice to leave the tree is not found, whether it left a worker or a check.
pub async fn peer_worker(
    projects: &Projects,
    pid: Option<u32>,
) -> Result<Option<PeerWorker>, AgentError> {
    let Some(pid) = pid else {
        return Ok(None);
    };
    let mut workers = HashMap::new();
    for handle in projects.handles() {
        for (worker, agent) in agents::live_worker_pids(handle.store.read()).await? {
            workers.insert(
                worker,
                PeerWorker {
                    project: handle.slug.clone(),
                    agent: Some(agent),
                },
            );
        }
    }
    if workers.is_empty() && lock(&CHECKS).is_empty() {
        return Ok(None);
    }
    let chain = tokio::task::spawn_blocking(move || process_chain(pid))
        .await
        .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "the process walk failed"))?
        .ok_or_else(|| {
            FoldError::new(
                codes::INTERNAL_ERROR,
                "the daemon could not read the parent processes of the socket peer",
            )
            .with_hint("run the command again")
        })?;
    let worker = chain.iter().find_map(|pid| workers.remove(pid));
    Ok(worker.or_else(|| check_peer(&chain)))
}

/// Refuses `agent` to a socket peer under a worker of another agent, for a socket route that names its agent itself instead of a bound session.
pub async fn require_peer_agent(
    projects: &Projects,
    pid: Option<u32>,
    project: &str,
    agent: &AgentName,
) -> Result<(), AgentError> {
    let peer = session_peer(peer_worker(projects, pid).await?, project);
    if let Err(denied) = policy::can_bind_agent(&peer, agent) {
        let worker = peer.worker().map(AgentName::as_str);
        tracing::warn!(agent = %agent, worker, "refused a hook from a worker's or a check's process tree");
        return Err(FoldError::from(denied).into());
    }
    Ok(())
}

/// What a session in project `project` may act as, given the worker or the check its peer runs under.
pub fn session_peer(worker: Option<PeerWorker>, project: &str) -> SessionPeer {
    match worker {
        None => SessionPeer::Unsupervised,
        Some(PeerWorker { agent: None, .. }) => SessionPeer::Check,
        Some(PeerWorker {
            project: own,
            agent: Some(agent),
        }) if own == project => SessionPeer::Worker(agent),
        Some(_) => SessionPeer::OtherProjectWorker,
    }
}

/// `pid` and its ancestors, nearest first, up to the first process without a readable parent. Pid 1 and the kernel's pid 0 end the chain, because every process descends from them. `None` when the process table or the entry of `pid` itself cannot be read.
pub fn process_chain(pid: u32) -> Option<Vec<u32>> {
    walk(pid, parents()?)
}

fn walk(pid: u32, mut parent_of: impl FnMut(u32) -> Option<u32>) -> Option<Vec<u32>> {
    let mut chain = vec![pid];
    let mut parent = parent_of(pid)?;
    while parent > 1 && !chain.contains(&parent) && chain.len() < MAX_DEPTH {
        chain.push(parent);
        match parent_of(parent) {
            Some(next) => parent = next,
            None => break,
        }
    }
    Some(chain)
}

/// The fourth field of `/proc/<pid>/stat`. The second field, the command name, sits in parentheses and may hold spaces and parentheses itself, so the fields are counted after its last `)`.
#[cfg(target_os = "linux")]
fn parents() -> Option<impl FnMut(u32) -> Option<u32>> {
    Some(|pid| {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_stat_parent(&stat)
    })
}

#[cfg(any(target_os = "linux", test))]
fn parse_stat_parent(stat: &str) -> Option<u32> {
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(1)?.parse().ok()
}

/// One `ps` listing of every process, read once per walk. macOS has no `/proc`. `None` when `ps` fails or lists nothing.
#[cfg(not(target_os = "linux"))]
fn parents() -> Option<impl FnMut(u32) -> Option<u32>> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=", "-o", "ppid="])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let table = parse_ps_table(&String::from_utf8_lossy(&output.stdout));
    (!table.is_empty()).then_some(move |pid| table.get(&pid).copied())
}

#[cfg(any(not(target_os = "linux"), test))]
fn parse_ps_table(listing: &str) -> HashMap<u32, u32> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_follows_parents_and_stops_at_init_a_loop_or_the_bound() {
        let table = HashMap::from([(40, 30), (30, 20), (20, 1)]);
        let parent = |pid| table.get(&pid).copied();
        assert_eq!(walk(40, parent), Some(vec![40, 30, 20]));
        let looped = HashMap::from([(5, 6), (6, 5)]);
        assert_eq!(walk(5, |pid| looped.get(&pid).copied()), Some(vec![5, 6]));
        let orphan = HashMap::from([(8, 9)]);
        assert_eq!(walk(8, |pid| orphan.get(&pid).copied()), Some(vec![8, 9]));
        let deep = walk(1000, |pid| Some(pid + 1)).unwrap();
        assert_eq!(deep.len(), MAX_DEPTH);
    }

    #[test]
    fn an_unreadable_peer_has_no_chain() {
        assert_eq!(walk(7, |_| None), None);
        let empty = parse_ps_table("");
        assert_eq!(walk(7, |pid| empty.get(&pid).copied()), None);
    }

    #[test]
    fn stat_and_ps_lines_give_the_parent() {
        assert_eq!(
            parse_stat_parent("4242 (we (ird) name) S 17 4242 4242 0"),
            Some(17)
        );
        assert_eq!(parse_stat_parent("4242 no parens"), None);
        let table = parse_ps_table("  1     0\n 812     1\nbad line\n 900   812\n");
        assert_eq!(table.get(&900), Some(&812));
        assert_eq!(table.get(&812), Some(&1));
        assert_eq!(table.len(), 3);
    }

    #[test]
    fn the_chain_of_a_child_holds_this_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let chain = process_chain(child.id()).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(chain[0], child.id());
        assert_eq!(chain.get(1), Some(&std::process::id()));
        assert_eq!(process_chain(child.id()), None, "a reaped peer");
    }

    #[test]
    fn a_session_in_the_workers_project_is_that_worker() {
        let worker = || {
            Some(PeerWorker {
                project: "app".to_owned(),
                agent: Some(AgentName::parse("web-worker").unwrap()),
            })
        };
        assert_eq!(
            session_peer(worker(), "app"),
            SessionPeer::Worker(AgentName::parse("web-worker").unwrap())
        );
        assert_eq!(
            session_peer(worker(), "other"),
            SessionPeer::OtherProjectWorker
        );
        assert_eq!(session_peer(None, "app"), SessionPeer::Unsupervised);
        let check = PeerWorker {
            project: "app".to_owned(),
            agent: None,
        };
        assert_eq!(session_peer(Some(check), "app"), SessionPeer::Check);
    }

    #[test]
    fn a_registered_check_counts_only_while_this_process_is_its_parent() {
        let owner = PeerWorker {
            project: "app".to_owned(),
            agent: Some(AgentName::parse("web-worker").unwrap()),
        };
        let me = std::process::id();
        let (leader, child) = (u32::MAX - 7, u32::MAX - 8);
        let first = register_check(leader, owner.clone());
        assert_eq!(check_peer(&[child, leader, me]), Some(owner.clone()));
        assert_eq!(check_peer(&[leader, me, 1]), Some(owner.clone()));
        assert_eq!(check_peer(&[child, leader, me - 1]), None, "another parent");
        assert_eq!(check_peer(&[child, me]), None);
        let later = PeerWorker {
            project: "app".to_owned(),
            agent: None,
        };
        let second = register_check(leader, later.clone());
        drop(first);
        assert_eq!(
            check_peer(&[leader, me]),
            Some(later),
            "the older registration leaves the newer one"
        );
        drop(second);
        assert_eq!(check_peer(&[leader, me]), None);
    }
}
