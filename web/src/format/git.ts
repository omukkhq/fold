import type { GitRemote, ProjectGit } from "../api/git";

const SHORT_COMMIT_LENGTH = 7;

/** What a project without a remote shows to add one. */
export const ADD_REMOTE_COMMAND = "fld remote add <name> <url>";

/** The first seven characters of a Git commit id. */
export function shortCommit(commit: string): string {
  return commit.slice(0, SHORT_COMMIT_LENGTH);
}

const FAILURES: Record<string, string> = {
  remote_ahead: "remote has newer commits",
  remote_not_set: "the remote is no longer set",
};

/** The caption of the first row: fold works on this branch of the repository, not on the remote's. */
export const LOCAL_BRANCH_LABEL = "Local branch";

/** What a remote row says about where fold pushes: the remote and its branch, as Git names them. */
export function pushTarget(remote: Pick<GitRemote, "name" | "branch">): string {
  return `Pushes to ${remote.name}/${remote.branch}`;
}

export interface RemoteLine {
  name: string;
  location: string;
  branch: string;
  /** "Pushes to origin/dev". */
  target: string;
  /** True when fold does not push after an integration; the row then says "push off". */
  pushOff: boolean;
  /** The Git commit of the newest revision on the remote; null before the first push or while the revision has none. */
  commit: string | null;
  tone: "ok" | "failed" | "idle";
  status: string;
}

/** A failed push after the last good one wins over that push, because the daemon drops the failure when a push succeeds. */
export function remoteLine(remote: GitRemote): RemoteLine {
  const line = {
    name: remote.name,
    location: remote.location,
    branch: remote.branch,
    target: pushTarget(remote),
    pushOff: !remote.push_on_integrate,
    commit: remote.pushed?.commit ?? null,
  };
  if (remote.failed) {
    const reason = FAILURES[remote.failed.code];
    return {
      ...line,
      tone: "failed",
      status: reason ? `push failed: ${reason}` : "push failed",
    };
  }
  if (remote.pushed) {
    return { ...line, tone: "ok", status: `pushed ${remote.pushed.revision}` };
  }
  return { ...line, tone: "idle", status: "not pushed yet" };
}

export interface GitSummary {
  repository: string;
  branch: string;
  branchLabel: string;
  remotes: RemoteLine[];
  /** The command to add a remote, shown only when there is none. */
  addRemote: string | null;
}

export function gitSummary(git: ProjectGit): GitSummary {
  return {
    repository: git.repository,
    branch: git.branch,
    branchLabel: LOCAL_BRANCH_LABEL,
    remotes: git.remotes.map(remoteLine),
    addRemote: git.remotes.length === 0 ? ADD_REMOTE_COMMAND : null,
  };
}
