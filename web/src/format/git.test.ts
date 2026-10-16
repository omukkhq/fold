import { describe, expect, it } from "vitest";
import type { GitRemote, ProjectGit } from "../api/git";
import { ADD_REMOTE_COMMAND, gitSummary, remoteLine, shortCommit } from "./git";

const remote = (extra: Partial<GitRemote> = {}): GitRemote => ({
  name: "origin",
  location: "git@example.com:ledger.git",
  branch: "main",
  push_on_integrate: true,
  pushed: null,
  failed: null,
  ...extra,
});

describe("shortCommit", () => {
  it("keeps the first seven characters", () => {
    expect(shortCommit("4f2a9c1e4f2a9c1e4f2a9c1e4f2a9c1e4f2a9c1e")).toBe(
      "4f2a9c1",
    );
    expect(shortCommit("abc")).toBe("abc");
  });
});

describe("remoteLine", () => {
  it("says a remote that never received a push has none yet", () => {
    expect(remoteLine(remote())).toMatchObject({
      tone: "idle",
      status: "not pushed yet",
      commit: null,
    });
  });

  it("names the newest pushed revision and its commit", () => {
    const line = remoteLine(
      remote({ pushed: { revision: "r8", commit: "4f2a9c1e", at: 1 } }),
    );
    expect(line).toMatchObject({
      tone: "ok",
      status: "pushed r8",
      commit: "4f2a9c1e",
    });
  });

  it("keeps the commit null while the pushed revision has none", () => {
    const line = remoteLine(
      remote({ pushed: { revision: "r8", commit: null, at: 1 } }),
    );
    expect(line.commit).toBeNull();
  });

  it("shows a failure over the last good push and words known codes", () => {
    const pushed = { revision: "r8", commit: "4f2a9c1e", at: 1 };
    expect(
      remoteLine(remote({ pushed, failed: { code: "remote_ahead", at: 2 } })),
    ).toMatchObject({
      tone: "failed",
      status: "push failed: remote has newer commits",
      commit: "4f2a9c1e",
    });
    expect(
      remoteLine(remote({ failed: { code: "remote_not_set", at: 2 } })).status,
    ).toBe("push failed: the remote is no longer set");
    expect(
      remoteLine(remote({ failed: { code: "other", at: 2 } })).status,
    ).toBe("push failed");
  });
});

describe("gitSummary", () => {
  const base: ProjectGit = {
    repository: "/home/ana/ledger",
    branch: "main",
    remotes: [],
  };

  it("offers the add command when there is no remote", () => {
    expect(gitSummary(base)).toMatchObject({
      repository: "/home/ana/ledger",
      branch: "main",
      remotes: [],
      addRemote: ADD_REMOTE_COMMAND,
    });
  });

  it("lists the remotes and offers nothing to add", () => {
    const summary = gitSummary({ ...base, remotes: [remote()] });
    expect(summary.remotes.map((r) => r.name)).toEqual(["origin"]);
    expect(summary.addRemote).toBeNull();
  });
});
