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
let invalidated: string[];

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
    expect(invalidated).toEqual([]);
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
    expect(invalidated).toEqual([]);
  });
});

describe("invalidation rules", () => {
  it("invalidates the repository keys for remote.pulled", () => {
    applyEvent(client, slug, ev("remote.pulled"));
    expect(hit(keys.repository(slug))).toBe(true);
    expect(hit(keys.board(slug))).toBe(false);
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
      expect(invalidated.filter((k) => k.includes("proposal"))).toEqual([]);
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
      observe(keys.board("other")),
      observe(keys.projects()),
    ];
    await vi.waitFor(() => {
      expect(client.isFetching()).toBe(0);
    });
    expect(fetched).toHaveLength(5);
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
