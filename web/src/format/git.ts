import type { GitRemote, ProjectGit } from "../api/git";

const SHORT_COMMIT_LENGTH = 7;

/** What a project without a remote shows to add one. */
export const ADD_REMOTE_COMMAND = "fld remote set <name> <url>";

/** The first seven characters of a Git commit id. */
export function shortCommit(commit: string): string {
  return commit.slice(0, SHORT_COMMIT_LENGTH);
}

const FAILURES: Record<string, string> = {
  remote_ahead: "remote has newer commits",
  remote_not_set: "the remote is no longer set",
};

export interface RemoteLine {
  name: string;
  location: string;
  branch: string;
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
  remotes: RemoteLine[];
  /** The command to add a remote, shown only when there is none. */
  addRemote: string | null;
}

export function gitSummary(git: ProjectGit): GitSummary {
  return {
    repository: git.repository,
    branch: git.branch,
    remotes: git.remotes.map(remoteLine),
    addRemote: git.remotes.length === 0 ? ADD_REMOTE_COMMAND : null,
  };
}
