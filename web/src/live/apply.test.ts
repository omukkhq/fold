import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { keys } from "../api/keys";
import { applyEvent, domainOf, invalidateProject, patchBoard } from "./apply";
import type { EventEnvelope } from "./envelope";

const slug = "demo";

function board() {
  return {
    columns: [
      {
        status: "ready",
        cards: [{ task: 1, status: "ready", blocked: false }],
      },
      {
        status: "in_progress",
        cards: [{ task: 2, status: "in_progress", blocked: false }],
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

function ev(type: string, extra: Partial<EventEnvelope> = {}): EventEnvelope {
  return { seq: 1, type, ...extra };
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
      ev("task.status_changed", { task: 1, data: { status: "in_progress" } }),
    );
    const next = client.getQueryData<ReturnType<typeof board>>(
      keys.board(slug),
    );
    expect(next?.columns[0]?.cards).toEqual([]);
    expect(next?.columns[1]?.cards.map((c) => c.task)).toEqual([2, 1]);
    expect(next?.columns[1]?.cards[1]?.status).toBe("in_progress");
    expect(invalidated).toEqual(onlyActivity);
  });

  it("patches blocked on the board and the task cache", () => {
    client.setQueryData(keys.board(slug), board());
    client.setQueryData(keys.task(slug, 1), {
      task: 1,
      blocked: false,
      title: "x",
    });
    applyEvent(
      client,
      slug,
      ev("task.blocked_changed", { task: 1, data: { blocked: true } }),
    );
    expect(client.getQueryData(keys.task(slug, 1))).toEqual({
      task: 1,
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
      ev("task.status_changed", { task: 1, data: { status: "done" } }),
    );
    expect(client.getQueryData(keys.task(slug, 1))).toBeUndefined();
    expect(client.getQueryData(keys.board(slug))).toBeUndefined();
    expect(invalidated).toEqual(onlyActivity);
  });

  it("invalidates the board when the card or its column is not cached", () => {
    client.setQueryData(keys.board(slug), board());
    applyEvent(
      client,
      slug,
      ev("task.status_changed", { task: 9, data: { status: "done" } }),
    );
    expect(hit(keys.board(slug))).toBe(true);
    invalidated = [];
    applyEvent(
      client,
      slug,
      ev("task.status_changed", { task: 1, data: { status: "archived" } }),
    );
    expect(hit(keys.board(slug))).toBe(true);
    invalidated = [];
    client.setQueryData(keys.board(slug), { rows: [] });
    applyEvent(
      client,
      slug,
      ev("task.status_changed", { task: 1, data: { status: "done" } }),
    );
    expect(hit(keys.board(slug))).toBe(true);
  });

  it("keeps a non-record task cache as it is", () => {
    client.setQueryData(keys.task(slug, 1), "odd");
    applyEvent(
      client,
      slug,
      ev("task.blocked_changed", { task: 1, data: { blocked: true } }),
    );
    expect(client.getQueryData(keys.task(slug, 1))).toBe("odd");
  });

  it("falls back to invalidation when the patch data is malformed or the task id is absent", () => {
    applyEvent(
      client,
      slug,
      ev("task.status_changed", { task: 1, data: { status: 3 } }),
    );
    expect(hit(keys.board(slug))).toBe(true);
    invalidated = [];
    applyEvent(client, slug, ev("task.blocked_changed", { task: 1 }));
    expect(hit(keys.board(slug))).toBe(true);
    invalidated = [];
    client.setQueryData(keys.board(slug), board());
    applyEvent(
      client,
      slug,
      ev("task.status_changed", { data: { status: "done" } }),
    );
    expect(client.getQueryData(keys.board(slug))).toEqual(board());
  });

  it("replaces one workspace live state wholesale", () => {
    client.setQueryData(keys.workspaceLive(slug, 1), { changed: 1, old: true });
    applyEvent(
      client,
      slug,
      ev("workspace.changed", {
        workspace: 1,
        data: { state: { changed: 4 } },
      }),
    );
    expect(client.getQueryData(keys.workspaceLive(slug, 1))).toEqual({
      changed: 4,
    });
    expect(invalidated).toEqual([]);
  });

  it("ignores a workspace.changed with no workspace or state", () => {
    applyEvent(
      client,
      slug,
      ev("workspace.changed", { data: { state: { a: 1 } } }),
    );
    applyEvent(client, slug, ev("workspace.changed", { workspace: 1 }));
    expect(client.getQueryCache().getAll()).toHaveLength(0);
  });

  it("replaces every live state on a snapshot and drops absent ones", () => {
    client.setQueryData(keys.workspaceLive(slug, 1), { changed: 1 });
    client.setQueryData(keys.workspaceLive(slug, 2), { changed: 2 });
    client.setQueryData(keys.workspaceLive("other", 2), { changed: 9 });
    client.setQueryData(keys.workspace(slug, 2), { id: 2 });
    applyEvent(client, slug, {
      seq: 5,
      type: "snapshot",
      workspaces: [
        { workspace: 1, state: { changed: 7 } },
        { workspace: 3, state: { changed: 3 } },
      ],
    });
    expect(client.getQueryData(keys.workspaceLive(slug, 1))).toEqual({
      changed: 7,
    });
    expect(client.getQueryData(keys.workspaceLive(slug, 2))).toBeUndefined();
    expect(client.getQueryData(keys.workspaceLive(slug, 3))).toEqual({
      changed: 3,
    });
    expect(client.getQueryData(keys.workspaceLive("other", 2))).toEqual({
      changed: 9,
    });
    expect(client.getQueryData(keys.workspace(slug, 2))).toEqual({ id: 2 });
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
      ev("integration.succeeded", { proposal: 4, task: 2 }),
    );
    expect(hit(keys.proposal(slug, 4))).toBe(true);
    expect(hit(keys.proposals(slug))).toBe(true);
    expect(hit(keys.task(slug, 2))).toBe(true);
    expect(hit(keys.repository(slug))).toBe(true);
    invalidated = [];
    applyEvent(
      client,
      slug,
      ev("proposal.integrated", { proposal: 4, task: 2 }),
    );
    expect(hit(keys.repository(slug))).toBe(true);
    expect(hit(keys.proposal(slug, 4))).toBe(true);
    expect(hit(keys.board(slug))).toBe(true);
  });

  it("invalidates the board with the attention list for proposal, review and message events", () => {
    for (const type of [
      "proposal.opened",
      "review.comment_added",
      "message.sent",
    ]) {
      invalidated = [];
      applyEvent(client, slug, ev(type, { proposal: 4 }));
      expect(hit(keys.board(slug))).toBe(true);
    }
    invalidated = [];
    applyEvent(client, slug, ev("review.submitted", { proposal: 4 }));
    expect(hit(keys.proposal(slug, 4))).toBe(true);
    invalidated = [];
    applyEvent(client, slug, ev("message.acknowledged"));
    expect(hit(keys.messages(slug))).toBe(true);
    invalidated = [];
    applyEvent(
      client,
      slug,
      ev("proposal.version_added", { proposal: 4, task: 1 }),
    );
    expect(hit(keys.proposals(slug))).toBe(true);
    expect(hit(keys.repository(slug))).toBe(false);
  });

  it("invalidates board, workspace and agents keys for claim.stale", () => {
    applyEvent(client, slug, ev("claim.stale", { task: 2, workspace: 6 }));
    expect(hit(keys.board(slug))).toBe(true);
    expect(hit(keys.workspaces(slug))).toBe(true);
    expect(hit(keys.workspace(slug, 6))).toBe(true);
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
    applyEvent(client, slug, ev("task.updated", { task: 3 }));
    expect(hit(keys.task(slug, 3))).toBe(true);
  });

  it("refreshes only the activity list for an event of an unknown type", () => {
    applyEvent(client, slug, ev("future.thing", { task: 1, proposal: 2 }));
    expect(invalidated).toEqual(onlyActivity);
    invalidated = [];
    applyEvent(client, slug, ev("nodot"));
    expect(invalidated).toEqual(onlyActivity);
  });

  it("invalidates the domain of an unknown type in a known domain", () => {
    applyEvent(client, slug, ev("task.renamed", { task: 3 }));
    expect(hit(keys.board(slug))).toBe(true);
    expect(hit(keys.task(slug, 3))).toBe(true);
  });

  it("drops the live state of an abandoned or archived workspace", () => {
    for (const type of ["workspace.abandoned", "workspace.archived"]) {
      client.setQueryData(keys.workspaceLive(slug, 4), { changed: 1 });
      client.setQueryData(keys.workspaceLive(slug, 5), { changed: 1 });
      applyEvent(client, slug, ev(type, { workspace: 4 }));
      expect(client.getQueryData(keys.workspaceLive(slug, 4))).toBeUndefined();
      expect(client.getQueryData(keys.workspaceLive(slug, 5))).toEqual({
        changed: 1,
      });
      expect(hit(keys.workspace(slug, 4))).toBe(true);
    }
    applyEvent(client, slug, ev("workspace.archived"));
    expect(client.getQueryData(keys.workspaceLive(slug, 5))).toEqual({
      changed: 1,
    });
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
      observe(keys.task(slug, 1)),
      observe(keys.workspaceLive(slug, 1)),
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
        [keys.board(slug), keys.task(slug, 1)]
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
    expect(patchBoard(undefined, 1, { status: "done" })).toBeNull();
    expect(
      patchBoard({ columns: [{ status: "x", cards: [1] }] }, 1, {}),
    ).toBeNull();
  });

  it("patches in place when the status is unchanged", () => {
    const next = patchBoard(board(), 1, { blocked: true });
    expect(next?.columns[0]?.cards[0]).toEqual({
      task: 1,
      status: "ready",
      blocked: true,
    });
  });
});
