import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { keys } from "../api/keys";
import {
  applyEvent,
  domainOf,
  invalidateProject,
  patchBoard,
  patchPresence,
  patchNamedStatus,
} from "./apply";
import {
  isEventEnvelope,
  type EventEnvelope,
  type LiveWorkspace,
} from "./envelope";

const slug = "demo";

function board() {
  return {
    columns: [
      {
        status: "ready",
        cards: [{ task: "T-1", status: "ready", blocked: false }],
      },
      {
        status: "in_progress",
        cards: [{ task: "T-2", status: "in_progress", blocked: false }],
      },
      { status: "done", cards: [] },
    ],
  };
}

let client: QueryClient;
/** A filter without a key, such as a predicate, records `undefined`. */
let invalidated: (string | undefined)[];

beforeEach(() => {
  client = new QueryClient();
  invalidated = [];
  const original = client.invalidateQueries.bind(client);
  client.invalidateQueries = (filters, options) => {
    invalidated.push(JSON.stringify(filters?.queryKey));
    return original(filters, options);
  };
});

const hit = (key: readonly unknown[]) =>
  invalidated.includes(JSON.stringify(key));
/** A stored event is always a new row of the activity list. */
const onlyActivity = [JSON.stringify(keys.activity(slug))];

/** Every frame reaches `applyEvent` through the guard, as it does from the stream. */
function guarded(value: unknown): EventEnvelope {
  if (!isEventEnvelope(value)) {
    throw new Error(`not a frame: ${JSON.stringify(value)}`);
  }
  return value;
}

function ev(type: string, extra: Record<string, unknown> = {}): EventEnvelope {
  return guarded({
    kind: "event",
    seq: 1,
    type,
    actor: "frontend/ui",
    task: null,
    proposal: null,
    workspace: null,
    revision: null,
    data: {},
    created_at: 1_760_000_000_000,
    ...extra,
  });
}

function live(id: string, summary: string): LiveWorkspace {
  return {
    workspace: id,
    task: "T-1",
    worker: "frontend/ui",
    base: "r1",
    files: [
      {
        path: "src/a.ts",
        previous_path: null,
        change: "modified",
        added_lines: 2,
        removed_lines: 0,
        binary: false,
      },
    ],
    summary,
    last_activity_at: 1_760_000_000_000,
    checkpoints: [
      {
        id: "ck-1",
        label: null,
        automatic: true,
        created_at: 1_760_000_000_000,
      },
    ],
    proposal: null,
    stalled: false,
  };
}

describe("domainOf", () => {
  it("returns the part before the dot for known domains only", () => {
    expect(domainOf("review.comment_added")).toBe("review");
    expect(domainOf("future.thing")).toBeNull();
    expect(domainOf("nodot")).toBeNull();
    expect(domainOf(".x")).toBeNull();
  });
});

describe("patch rules", () => {
  it("moves a card between cached columns with no refetch", () => {
    client.setQueryData(keys.board(slug), board());
    applyEvent(
      client,
      slug,
      ev("task.status_changed", {
        task: "T-1",
        data: { from: "todo", to: "in_progress" },
      }),
    );
    const next = client.getQueryData<ReturnType<typeof board>>(
      keys.board(slug),
    );
    expect(next?.columns[0]?.cards).toEqual([]);
    expect(next?.columns[1]?.cards.map((c) => c.task)).toEqual(["T-2", "T-1"]);
    expect(next?.columns[1]?.cards[1]?.status).toBe("in_progress");
    expect(invalidated).toEqual(onlyActivity);
  });

  it("patches blocked on the board and the task cache", () => {
    client.setQueryData(keys.board(slug), board());
    client.setQueryData(keys.task(slug, "T-1"), {
      task: "T-1",
      blocked: false,
      title: "x",
    });
    applyEvent(
      client,
      slug,
      ev("task.blocked_changed", {
        task: "T-1",
        data: { blocked: true, blocked_by: [] },
      }),
    );
    expect(client.getQueryData(keys.task(slug, "T-1"))).toEqual({
      task: "T-1",
      blocked: true,
      title: "x",
    });
    const next = client.getQueryData<ReturnType<typeof board>>(
      keys.board(slug),
    );
    expect(next?.columns[0]?.cards[0]?.blocked).toBe(true);
    expect(invalidated).toEqual(onlyActivity);
  });

  it("leaves an uncached task and board alone", () => {
    applyEvent(
      client,
      slug,
      ev("task.status_changed", {
        task: "T-1",
        data: { from: "todo", to: "done" },
      }),
    );
    expect(client.getQueryData(keys.task(slug, "T-1"))).toBeUndefined();
    expect(client.getQueryData(keys.board(slug))).toBeUndefined();
    expect(invalidated).toEqual(onlyActivity);
  });

  it("invalidates the board when the card or its column is not cached", () => {
    client.setQueryData(keys.board(slug), board());
    applyEvent(
      client,
      slug,
      ev("task.status_changed", {
        task: "T-9",
        data: { from: "todo", to: "done" },
      }),
    );
    expect(hit(keys.board(slug))).toBe(true);
    invalidated = [];
    applyEvent(
      client,
      slug,
      ev("task.status_changed", {
        task: "T-1",
        data: { from: "todo", to: "archived" },
      }),
    );
    expect(hit(keys.board(slug))).toBe(true);
    invalidated = [];
    client.setQueryData(keys.board(slug), { rows: [] });
    applyEvent(
      client,
      slug,
      ev("task.status_changed", {
        task: "T-1",
        data: { from: "todo", to: "done" },
      }),
    );
    expect(hit(keys.board(slug))).toBe(true);
  });

  it("keeps a non-record task cache as it is", () => {
    client.setQueryData(keys.task(slug, "T-1"), "odd");
    applyEvent(
      client,
      slug,
      ev("task.blocked_changed", {
        task: "T-1",
        data: { blocked: true, blocked_by: [] },
      }),
    );
    expect(client.getQueryData(keys.task(slug, "T-1"))).toBe("odd");
  });

  it("falls back to invalidation when the patch data is malformed or the task id is absent", () => {
    applyEvent(
      client,
      slug,
      ev("task.status_changed", { task: "T-1", data: { from: "todo", to: 3 } }),
    );
    expect(hit(keys.board(slug))).toBe(true);
    invalidated = [];
    applyEvent(client, slug, ev("task.blocked_changed", { task: "T-1" }));
    expect(hit(keys.board(slug))).toBe(true);
    invalidated = [];
    client.setQueryData(keys.board(slug), board());
    applyEvent(
      client,
      slug,
      ev("task.status_changed", { data: { from: "todo", to: "done" } }),
    );
    expect(client.getQueryData(keys.board(slug))).toEqual(board());
  });

  it("replaces one workspace live state wholesale from a workspace_changed frame", () => {
    client.setQueryData(keys.workspaceLive(slug, "ws-1"), live("ws-1", "old"));
    const next = live("ws-1", "1 file, +2 -0");
    applyEvent(client, slug, guarded({ kind: "workspace_changed", ...next }));
    expect(client.getQueryData(keys.workspaceLive(slug, "ws-1"))).toEqual(next);
    // Only the workspace diffs, picked by a predicate rather than a key, are read again.
    expect(invalidated.every((key) => key === undefined)).toBe(true);
  });

  it("leaves the workspace list and the activity list alone on a workspace_changed frame", () => {
    client.setQueryData(keys.workspaces(slug), [{ workspace: "ws-1" }]);
    applyEvent(
      client,
      slug,
      guarded({ kind: "workspace_changed", ...live("ws-1", "1 file, +2 -0") }),
    );
    expect(client.getQueryData(keys.workspaces(slug))).toEqual([
      { workspace: "ws-1" },
    ]);
    expect(hit(keys.workspaces(slug))).toBe(false);
    expect(hit(keys.activity(slug))).toBe(false);
  });

  it("patches the presence of the named agent in every agent list with no refetch", () => {
    client.setQueryData(keys.agents(slug), [
      { name: "frontend", presence: "active", tags: [] },
      { name: "backend", presence: "active", tags: [] },
    ]);
    client.setQueryData(keys.agentBar(slug), [
      { name: "frontend", presence: "active", working: 1, stalled: 0 },
    ]);
    const targets = { agents: ["frontend"], tags: [] };
    client.setQueryData(keys.messageTargets(slug), targets);
    applyEvent(
      client,
      slug,
      guarded({
        kind: "agent_presence_changed",
        agent: "frontend",
        presence: "idle",
      }),
    );
    expect(client.getQueryData(keys.agents(slug))).toEqual([
      { name: "frontend", presence: "idle", tags: [] },
      { name: "backend", presence: "active", tags: [] },
    ]);
    expect(client.getQueryData(keys.agentBar(slug))).toEqual([
      { name: "frontend", presence: "idle", working: 1, stalled: 0 },
    ]);
    expect(client.getQueryData(keys.messageTargets(slug))).toBe(targets);
    expect(invalidated).toEqual([]);
  });

  it("records a worker_output frame as the worker's latest step and refetches nothing", () => {
    const agents = [{ name: "web-coder", presence: "idle", tags: [] }];
    client.setQueryData(keys.agents(slug), agents);
    const frame = (agent: string, step: string, tool: string | null) =>
      guarded({ kind: "worker_output", agent, step, tool, text_length: null });
    applyEvent(client, slug, frame("web-coder", "text", null));
    applyEvent(client, slug, frame("web-lead", "tool_use", "Agent"));
    applyEvent(client, slug, frame("web-coder", "tool_use", "Edit"));
    expect(client.getQueryData(keys.workerSteps(slug))).toEqual({
      "web-coder": { step: "tool_use", tool: "Edit" },
      "web-lead": { step: "tool_use", tool: "Agent" },
    });
    expect(client.getQueryData(keys.agents(slug))).toBe(agents);
    expect(invalidated).toEqual([]);
  });

  it("refetches an agent list that does not hold the agent and leaves other projects alone", () => {
    client.setQueryData(keys.agents(slug), [
      { name: "backend", presence: "idle" },
    ]);
    client.setQueryData(keys.agents("other"), [
      { name: "frontend", presence: "active" },
    ]);
    applyEvent(
      client,
      slug,
      guarded({
        kind: "agent_presence_changed",
        agent: "frontend",
        presence: "active",
      }),
    );
    expect(hit(keys.agents(slug))).toBe(true);
    expect(hit(keys.activity(slug))).toBe(false);
    expect(client.getQueryData(keys.agents("other"))).toEqual([
      { name: "frontend", presence: "active" },
    ]);
  });

  it("patches only a list of records that holds the agent", () => {
    const frame = { agent: "frontend", presence: "offline" } as const;
    expect(patchPresence(undefined, frame)).toBeNull();
    expect(patchPresence({ agents: [] }, frame)).toBeNull();
    expect(patchPresence(["frontend"], frame)).toBeNull();
    expect(patchPresence([{ name: "backend" }], frame)).toBeNull();
    expect(
      patchPresence([{ name: "frontend", presence: "idle" }], frame),
    ).toEqual([{ name: "frontend", presence: "offline" }]);
  });

  it("replaces every live state on a snapshot and drops absent ones", () => {
    client.setQueryData(keys.workspaceLive(slug, "ws-1"), live("ws-1", "a"));
    client.setQueryData(keys.workspaceLive(slug, "ws-2"), live("ws-2", "b"));
    client.setQueryData(keys.workspaceLive("other", "ws-2"), live("ws-2", "c"));
    client.setQueryData(keys.workspace(slug, "ws-2"), { id: "ws-2" });
    const one = live("ws-1", "seven");
    const three = live("ws-3", "three");
    applyEvent(
      client,
      slug,
      guarded({ kind: "snapshot", seq: 5, workspaces: [one, three] }),
    );
    expect(client.getQueryData(keys.workspaceLive(slug, "ws-1"))).toEqual(one);
    expect(
      client.getQueryData(keys.workspaceLive(slug, "ws-2")),
    ).toBeUndefined();
    expect(client.getQueryData(keys.workspaceLive(slug, "ws-3"))).toEqual(
      three,
    );
    expect(client.getQueryData(keys.workspaceLive("other", "ws-2"))).toEqual(
      live("ws-2", "c"),
    );
    expect(client.getQueryData(keys.workspace(slug, "ws-2"))).toEqual({
      id: "ws-2",
    });
    // Only the workspace diffs, picked by a predicate rather than a key, are read again.
    expect(invalidated.every((key) => key === undefined)).toBe(true);
  });
});

describe("invalidation rules", () => {
  it("invalidates the repository keys for remote.pulled", () => {
    applyEvent(client, slug, ev("remote.pulled"));
    expect(hit(keys.repository(slug))).toBe(true);
    expect(hit(keys.board(slug))).toBe(false);
  });

  it("refreshes the Git status for every remote event", () => {
    for (const type of [
      "remote.pushed",
      "remote.push_failed",
      "remote.pulled",
    ]) {
      invalidated = [];
      applyEvent(client, slug, ev(type));
      expect(hit(keys.git(slug))).toBe(true);
    }
  });

  it("refreshes the revisions, proposals and tasks when a Git commit is written", () => {
    applyEvent(client, slug, ev("git.committed", { revision: "r8" }));
    expect(hit(keys.repository(slug))).toBe(true);
    expect(hit(keys.git(slug))).toBe(true);
    expect(hit(keys.proposals(slug))).toBe(true);
    expect(hit(keys.tasks(slug))).toBe(true);
  });

  it("refreshes the Git status and the project for a failed or outside commit", () => {
    for (const type of ["git.commit_failed", "git.outside_commits"]) {
      invalidated = [];
      applyEvent(client, slug, ev(type));
      expect(hit(keys.git(slug))).toBe(true);
      expect(hit(keys.project(slug))).toBe(true);
      expect(hit(keys.proposals(slug))).toBe(false);
    }
  });

  it("invalidates repository and board, and no proposal key, for a pull integration", () => {
    for (const type of [
      "integration.succeeded",
      "integration.conflicted",
      "main.advanced",
    ]) {
      invalidated = [];
      applyEvent(client, slug, ev(type));
      expect(hit(keys.repository(slug))).toBe(true);
      expect(hit(keys.board(slug))).toBe(true);
      expect(invalidated.filter((k) => k?.includes("proposal"))).toEqual([]);
    }
  });

  it("invalidates the proposal, task and repository keys for a proposal integration", () => {
    applyEvent(
      client,
      slug,
      ev("integration.succeeded", { proposal: "P-4", task: "T-2" }),
    );
    expect(hit(keys.proposal(slug, "P-4"))).toBe(true);
    expect(hit(keys.proposals(slug))).toBe(true);
    expect(hit(keys.task(slug, "T-2"))).toBe(true);
    expect(hit(keys.repository(slug))).toBe(true);
    invalidated = [];
    applyEvent(
      client,
      slug,
      ev("proposal.integrated", { proposal: "P-4", task: "T-2" }),
    );
    expect(hit(keys.repository(slug))).toBe(true);
    expect(hit(keys.proposal(slug, "P-4"))).toBe(true);
    expect(hit(keys.board(slug))).toBe(true);
  });

  it("invalidates the board with the attention list for proposal, review and message events", () => {
    for (const type of [
      "proposal.opened",
      "review.comment_added",
      "message.sent",
    ]) {
      invalidated = [];
      applyEvent(client, slug, ev(type, { proposal: "P-4" }));
      expect(hit(keys.board(slug))).toBe(true);
    }
    invalidated = [];
    applyEvent(client, slug, ev("review.submitted", { proposal: "P-4" }));
    expect(hit(keys.proposal(slug, "P-4"))).toBe(true);
    invalidated = [];
    applyEvent(client, slug, ev("message.acknowledged"));
    expect(hit(keys.messages(slug))).toBe(true);
    invalidated = [];
    applyEvent(
      client,
      slug,
      ev("proposal.version_added", { proposal: "P-4", task: "T-1" }),
    );
    expect(hit(keys.proposals(slug))).toBe(true);
    expect(hit(keys.repository(slug))).toBe(false);
  });

  it("refreshes the request list and the named request for request events", () => {
    expect(domainOf("request.created")).toBe("request");
    applyEvent(
      client,
      slug,
      ev("request.created", { data: { request: "RQ-7" } }),
    );
    expect(hit(keys.requests(slug))).toBe(true);
    expect(hit(keys.request(slug, "RQ-7"))).toBe(true);
    expect(hit(keys.board(slug))).toBe(false);
    invalidated = [];
    applyEvent(
      client,
      slug,
      ev("request.draft_submitted", { data: { request: "RQ-4", version: 2 } }),
    );
    expect(hit(keys.request(slug, "RQ-4"))).toBe(true);
    invalidated = [];
    applyEvent(client, slug, ev("request.closed", { data: {} }));
    expect(hit(keys.requests(slug))).toBe(true);
    expect(invalidated.some((k) => k?.includes('"request"'))).toBe(false);
  });

  it("puts the tasks an approval created on the board", () => {
    applyEvent(
      client,
      slug,
      ev("request.approved", {
        data: { request: "RQ-4", version: 2, tasks: ["T-12", "T-13"] },
      }),
    );
    expect(hit(keys.request(slug, "RQ-4"))).toBe(true);
    expect(hit(keys.board(slug))).toBe(true);
    expect(hit(keys.task(slug, "T-12"))).toBe(true);
    expect(hit(keys.task(slug, "T-13"))).toBe(true);
    invalidated = [];
    applyEvent(
      client,
      slug,
      ev("request.approved", { data: { request: "RQ-5", tasks: "T-1" } }),
    );
    expect(hit(keys.board(slug))).toBe(true);
    expect(hit(keys.task(slug, "T-1"))).toBe(false);
  });

  it("refreshes the request whose thread a message joins", () => {
    applyEvent(
      client,
      slug,
      ev("message.sent", { data: { message: "m-3", request: "RQ-4" } }),
    );
    expect(hit(keys.messages(slug))).toBe(true);
    expect(hit(keys.request(slug, "RQ-4"))).toBe(true);
    invalidated = [];
    applyEvent(client, slug, ev("message.sent", { data: { message: "m-4" } }));
    expect(invalidated.some((k) => k?.includes('"request"'))).toBe(false);
  });

  it("invalidates board, workspace and agents keys for claim.stale", () => {
    applyEvent(
      client,
      slug,
      ev("claim.stale", { task: "T-2", workspace: "ws-6" }),
    );
    expect(hit(keys.board(slug))).toBe(true);
    expect(hit(keys.workspaces(slug))).toBe(true);
    expect(hit(keys.workspace(slug, "ws-6"))).toBe(true);
    expect(hit(keys.agents(slug))).toBe(true);
  });

  it("refreshes the workers, their questions and the needs-you list for worker events", () => {
    applyEvent(client, slug, ev("worker.permission_asked"));
    for (const key of [
      keys.agents(slug),
      keys.workers(slug),
      keys.workerQuestions(slug),
      keys.needsYou(slug),
    ]) {
      expect(hit(key)).toBe(true);
    }
  });

  it("refreshes the workers for a change of the worker limits", () => {
    applyEvent(client, slug, ev("workers.limits_changed"));
    expect(hit(keys.workers(slug))).toBe(true);
  });

  it("refreshes the needs-you list for the events that change it", () => {
    const cases: [string, Record<string, unknown>][] = [
      ["message.sent", {}],
      ["review.submitted", { proposal: "P-3" }],
      ["task.human_review_set", { task: "T-3" }],
      ["proposal.opened", { proposal: "P-3" }],
      ["request.draft_submitted", { data: { request: "RQ-4" } }],
      ["request.approved", { data: { request: "RQ-4", tasks: [] } }],
    ];
    for (const [type, extra] of cases) {
      invalidated = [];
      applyEvent(client, slug, ev(type, extra));
      expect(hit(keys.needsYou(slug)), type).toBe(true);
    }
    invalidated = [];
    applyEvent(client, slug, ev("task.updated", { task: "T-3" }));
    expect(hit(keys.needsYou(slug))).toBe(false);
  });

  it("refreshes the batches for batch, proposal and review events", () => {
    applyEvent(client, slug, ev("review.batch_created", { proposal: "P-3" }));
    expect(hit(keys.reviewBatches(slug))).toBe(true);
    expect(hit(keys.workers(slug))).toBe(true);
    expect(hit(keys.proposal(slug, "P-3"))).toBe(true);
    expect(hit(keys.board(slug))).toBe(false);
    for (const type of ["review.submitted", "proposal.version_added"]) {
      invalidated = [];
      applyEvent(client, slug, ev(type));
      expect(hit(keys.reviewBatches(slug)), type).toBe(true);
    }
  });

  it("covers the remaining domains", () => {
    const cases: [string, readonly unknown[]][] = [
      ["agent.registered", keys.agents(slug)],
      ["worker.registered", keys.agents(slug)],
      ["session.ended", keys.agents(slug)],
      ["task.created", keys.board(slug)],
      ["label.created", keys.board(slug)],
      ["dependency.added", keys.board(slug)],
      ["checklist.item_done", keys.board(slug)],
      ["workspace.created", keys.workspaces(slug)],
      ["workspace.created", keys.board(slug)],
      ["workspace.abandoned", keys.board(slug)],
      ["checkpoint.recorded", keys.workspaces(slug)],
      ["changeset.recorded", keys.workspaces(slug)],
    ];
    for (const [type, key] of cases) {
      invalidated = [];
      applyEvent(client, slug, ev(type));
      expect(hit(key)).toBe(true);
      expect(hit(keys.activity(slug))).toBe(true);
    }
    invalidated = [];
    applyEvent(client, slug, ev("task.updated", { task: "T-3" }));
    expect(hit(keys.task(slug, "T-3"))).toBe(true);
  });

  it("refreshes only the activity list for an event of an unknown type", () => {
    applyEvent(
      client,
      slug,
      ev("future.thing", { task: "T-1", proposal: "P-2" }),
    );
    expect(invalidated).toEqual(onlyActivity);
    invalidated = [];
    applyEvent(client, slug, ev("nodot"));
    expect(invalidated).toEqual(onlyActivity);
  });

  it("invalidates the domain of an unknown type in a known domain", () => {
    applyEvent(client, slug, ev("task.renamed", { task: "T-3" }));
    expect(hit(keys.board(slug))).toBe(true);
    expect(hit(keys.task(slug, "T-3"))).toBe(true);
  });

  it("drops the live state of an abandoned or archived workspace", () => {
    for (const type of ["workspace.abandoned", "workspace.archived"]) {
      client.setQueryData(keys.workspaceLive(slug, "ws-4"), live("ws-4", "x"));
      client.setQueryData(keys.workspaceLive(slug, "ws-5"), live("ws-5", "x"));
      applyEvent(client, slug, ev(type, { workspace: "ws-4" }));
      expect(
        client.getQueryData(keys.workspaceLive(slug, "ws-4")),
      ).toBeUndefined();
      expect(client.getQueryData(keys.workspaceLive(slug, "ws-5"))).toEqual(
        live("ws-5", "x"),
      );
      expect(hit(keys.workspace(slug, "ws-4"))).toBe(true);
    }
    applyEvent(client, slug, ev("workspace.archived"));
    expect(client.getQueryData(keys.workspaceLive(slug, "ws-5"))).toEqual(
      live("ws-5", "x"),
    );
  });
});

describe("ended workspaces", () => {
  it("drop a change frame that arrives after the stored event that ended the workspace, until the next snapshot", () => {
    applyEvent(
      client,
      slug,
      ev("workspace.abandoned", { workspace: "ws-4", task: "T-1" }),
    );
    applyEvent(client, slug, {
      kind: "workspace_changed",
      ...live("ws-4", "late"),
    });
    expect(
      client.getQueryData(keys.workspaceLive(slug, "ws-4")),
    ).toBeUndefined();
    applyEvent(client, slug, {
      kind: "workspace_changed",
      ...live("ws-5", "other"),
    });
    expect(client.getQueryData(keys.workspaceLive(slug, "ws-5"))).toEqual(
      live("ws-5", "other"),
    );
    applyEvent(
      client,
      slug,
      ev("workspace.archived", { workspace: "ws-4", task: "T-1" }),
    );
    expect(client.getQueryData(keys.endedWorkspaces(slug))).toEqual(["ws-4"]);
    applyEvent(client, slug, { kind: "snapshot", seq: 3, workspaces: [] });
    expect(client.getQueryData(keys.endedWorkspaces(slug))).toBeUndefined();
  });

  it("read every live workspace's diffs again on a snapshot", () => {
    client.setQueryData(keys.comparison(slug, "r1", "ws-5"), { files: [] });
    client.setQueryData(keys.comparison(slug, "ws-5", "r1"), { files: [] });
    client.setQueryData(keys.comparison(slug, "r1", "r2"), { files: [] });
    client.setQueryData(keys.comparison("other", "r1", "ws-5"), {
      files: [],
    });
    applyEvent(client, slug, {
      kind: "snapshot",
      seq: 3,
      workspaces: [live("ws-5", "x")],
    });
    const invalid = (key: readonly unknown[]) =>
      client.getQueryState(key)?.isInvalidated;
    expect(invalid(keys.comparison(slug, "r1", "ws-5"))).toBe(true);
    expect(invalid(keys.comparison(slug, "ws-5", "r1"))).toBe(true);
    expect(invalid(keys.comparison(slug, "r1", "r2"))).toBe(false);
    expect(invalid(keys.comparison("other", "r1", "ws-5"))).toBe(false);
  });
});

describe("invalidateProject", () => {
  it("refetches every observed query of the project except the live workspace state", async () => {
    const fetched: string[] = [];
    const observe = (key: readonly unknown[]) => {
      const observer = new QueryObserver(client, {
        queryKey: key,
        queryFn: () => {
          fetched.push(JSON.stringify(key));
          return Promise.resolve(fetched.length);
        },
        staleTime: Infinity,
      });
      return observer.subscribe(() => undefined);
    };
    const offs = [
      observe(keys.board(slug)),
      observe(keys.task(slug, "T-1")),
      observe(keys.workspaceLive(slug, "ws-1")),
      observe(keys.workerSteps(slug)),
      observe(keys.board("other")),
      observe(keys.projects()),
    ];
    await vi.waitFor(() => {
      expect(client.isFetching()).toBe(0);
    });
    expect(fetched).toHaveLength(6);
    fetched.length = 0;
    invalidateProject(client, slug);
    await vi.waitFor(() => {
      expect(fetched.sort()).toEqual(
        [keys.board(slug), keys.task(slug, "T-1")]
          .map((k) => JSON.stringify(k))
          .sort(),
      );
    });
    for (const off of offs) {
      off();
    }
  });
});

describe("patchBoard", () => {
  it("returns null for a value that is not a board", () => {
    expect(patchBoard(undefined, "T-1", { status: "done" })).toBeNull();
    expect(
      patchBoard({ columns: [{ status: "x", cards: [1] }] }, "T-1", {}),
    ).toBeNull();
  });

  it("patches in place when the status is unchanged", () => {
    const next = patchBoard(board(), "T-1", { blocked: true });
    expect(next?.columns[0]?.cards[0]).toEqual({
      task: "T-1",
      status: "ready",
      blocked: true,
    });
  });
});

describe("patchNamedStatus", () => {
  it("leaves a value that names no task as it was", () => {
    const read = { dependencies: [{ task: "T-1" }], checklists: [null] };
    expect(patchNamedStatus(read, "T-9", "done")).toBe(read);
    expect(patchNamedStatus(null, "T-9", "done")).toBeNull();
    expect(patchNamedStatus({ dependencies: "x" }, "T-9", "done")).toEqual({
      dependencies: "x",
    });
  });
});

describe("event list", () => {
  it("adds a stored event to a loaded list in the shape the list read returns", () => {
    const client = new QueryClient();
    client.setQueryData(keys.events(slug), []);
    applyEvent(client, slug, ev("task.created", { seq: 4, task: "T-4" }));
    expect(client.getQueryData(keys.events(slug))).toEqual([
      {
        seq: 4,
        type: "task.created",
        actor: "frontend/ui",
        task: "T-4",
        proposal: null,
        workspace: null,
        revision: null,
        data: {},
        created_at: 1_760_000_000_000,
      },
    ]);
  });

  it("starts no list before the list read has loaded", () => {
    const client = new QueryClient();
    applyEvent(client, slug, ev("task.created"));
    expect(client.getQueryData(keys.events(slug))).toBeUndefined();
  });
});
