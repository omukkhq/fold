import { describe, expect, it } from "vitest";
import { agent, claim, worker, workspace } from "@/test/agents";
import {
  activityText,
  agentClaims,
  agentWorkspaces,
  changedFilesText,
  claimBadges,
  claimTarget,
  claimText,
  claimWorkspace,
  footLine,
  hasIntegrator,
  isEditableTag,
  orderWorkers,
  plainTags,
  presenceLine,
  sessionKey,
  sessionLine,
  sessionLines,
  workerBadges,
} from "./presence";

const NOW = Date.UTC(2026, 9, 1, 12, 0, 0);

describe("presenceLine", () => {
  it("words the state the payload gives", () => {
    expect(presenceLine(agent("a", { last_call_at: NOW - 12_000 }), NOW)).toBe(
      "active · 1 session · last call 12 s ago",
    );
    expect(
      presenceLine(
        agent("a", {
          presence: "idle",
          sessions: 0,
          last_call_at: NOW - 12_000,
        }),
        NOW,
      ),
    ).toBe("idle · 0 sessions · last call 12 s ago");
  });
  it("says so when the agent never called", () => {
    expect(
      presenceLine(agent("a", { presence: "offline", sessions: 0 }), NOW),
    ).toBe("offline · 0 sessions · no call yet");
  });
});

describe("activityText", () => {
  it("joins the daemon's activity and the age of the last call", () => {
    const w = worker("frontend/ui", {
      activity: "claim_task T-12",
      last_call_at: NOW - 12_000,
    });
    expect(activityText(w, NOW)).toBe("claim_task T-12 · 12 s ago");
  });
  it("shows the age alone when no activity is recorded", () => {
    expect(activityText(worker("a", { last_call_at: NOW - 60_000 }), NOW)).toBe(
      "1 min ago",
    );
  });
  it("says so when the worker never called", () => {
    expect(activityText(worker("a"), NOW)).toBe("no call yet");
  });
});

describe("claims of an agent", () => {
  const review = claim({ task: "T-10", kind: "review" });
  const a = agent("frontend", {
    workers: [
      worker("frontend", { claims: [review] }),
      worker("frontend/ui", {
        claims: [claim()],
        workspaces: [workspace()],
      }),
    ],
  });

  it("lists every claim with the worker that holds it", () => {
    expect(agentClaims(a).map((r) => [r.worker, r.claim.task])).toEqual([
      ["frontend", "T-10"],
      ["frontend/ui", "T-12"],
    ]);
  });

  it("finds the workspace of an implementation claim only", () => {
    const ui = a.workers[1];
    expect(ui && claimWorkspace(ui, claim())?.ws).toBe("ws-7");
    expect(ui && claimWorkspace(ui, claim({ task: "T-99" }))).toBeNull();
    expect(ui && claimWorkspace(ui, review)).toBeNull();
  });

  it("words the claims of a worker", () => {
    const [main, ui] = a.workers;
    expect(main && claimText(main)).toBe("T-10 review");
    expect(ui && claimText(ui)).toBe("T-12 · ws-7");
    expect(claimText(worker("w", { claims: [claim()] }))).toBe("T-12");
    expect(
      claimText(worker("w", { claims: [claim(), claim({ task: "T-13" })] })),
    ).toBe("T-12, T-13");
    expect(claimText(worker("w"))).toBeNull();
  });

  it("links every claim to its task", () => {
    expect(claimTarget("demo", review)).toEqual({
      id: "T-10",
      href: "/p/demo/t/T-10",
    });
  });
});

describe("workspaces of an agent", () => {
  it("lists them with the worker and counts the changed files", () => {
    const a = agent("a", {
      workers: [
        worker("a/x", { workspaces: [workspace({ changed_files: 1 })] }),
      ],
    });
    const [row] = agentWorkspaces(a);
    expect(row?.worker).toBe("a/x");
    expect(row && changedFilesText(row.workspace)).toBe("1 changed file");
    expect(changedFilesText(workspace({ changed_files: 0 }))).toBe(
      "0 changed files",
    );
  });
});

describe("tags", () => {
  it("lets a human edit only integrator", () => {
    expect(isEditableTag("integrator")).toBe(true);
    expect(isEditableTag("frontend")).toBe(false);
  });
  it("splits integrator from the plain tags", () => {
    const a = agent("a", { tags: ["frontend", "integrator"] });
    expect(hasIntegrator(a)).toBe(true);
    expect(plainTags(a)).toEqual(["frontend"]);
    expect(hasIntegrator(agent("a"))).toBe(false);
  });
});

describe("footLine", () => {
  it("counts claims, workspaces and open proposals", () => {
    const proposal = { id: "P-3", task: "T-11", title: "t", conflicted: false };
    expect(
      footLine(
        agent("a", {
          workers: [
            worker("a", {
              claims: [claim(), claim({ task: "T-11" })],
              workspaces: [workspace(), workspace({ ws: "ws-5" })],
              proposals: [proposal],
            }),
          ],
        }),
      ),
    ).toBe("2 claims · 2 workspaces · 1 open proposal");
  });
  it("counts review claims apart", () => {
    expect(
      footLine(
        agent("a", {
          workers: [worker("a", { claims: [claim({ kind: "review" })] })],
        }),
      ),
    ).toBe("1 review claim");
  });
  it("says so when the agent holds nothing", () => {
    expect(footLine(agent("a"))).toBe("no claims");
  });
});

describe("orderWorkers", () => {
  it("puts each worker after its parent", () => {
    const ordered = orderWorkers([
      worker("a/x"),
      worker("a"),
      worker("a/x/deep", { parent: "a/x" }),
      worker("a/y"),
    ]);
    expect(ordered.map((o) => [o.worker.name, o.depth])).toEqual([
      ["a", 0],
      ["a/x", 1],
      ["a/x/deep", 2],
      ["a/y", 1],
    ]);
  });
  it("still shows a worker whose parent is missing", () => {
    const ordered = orderWorkers([worker("orphan", { parent: "gone" })]);
    expect(ordered.map((o) => [o.worker.name, o.depth])).toEqual([
      ["orphan", 0],
    ]);
  });
  it("shows a worker cycle once", () => {
    const ordered = orderWorkers([
      worker("a", { parent: "b" }),
      worker("b", { parent: "a" }),
    ]);
    expect(ordered).toHaveLength(2);
  });
});

describe("session lines", () => {
  const NOW = new Date(2026, 9, 15, 14, 0, 0).getTime();
  const STARTED = new Date(2026, 9, 15, 13, 30, 0).getTime();
  const session = {
    started_at: STARTED,
    pid: 81231,
    cwd: "/home/tanim/ledger",
  };

  it("words a session with its start, pid and directory", () => {
    expect(sessionLine(session, NOW)).toBe(
      "session since 13:30:00 · pid 81231 · /home/tanim/ledger",
    );
  });

  it("ends at the pid for a session that sent no directory", () => {
    expect(sessionLine({ ...session, cwd: null }, NOW)).toBe(
      "session since 13:30:00 · pid 81231",
    );
  });

  it("keys a session by its pid and start", () => {
    expect(sessionKey(session)).toBe(`81231:${String(STARTED)}`);
  });

  it("lists the sessions the payload carries, and none when it carries none", () => {
    expect(sessionLines(agent("a", { open_sessions: [session] }))).toEqual([
      session,
    ]);
    expect(sessionLines(agent("a"))).toEqual([]);
  });
});

describe("badges", () => {
  const busy = worker("b/db", { claims: [claim({ task: "T-16" })] });

  it("marks a worker stalled when its claimed task has a stalled item", () => {
    expect(workerBadges(busy, [{ kind: "stalled", task: "T-16" }])).toEqual([
      "stalled",
    ]);
    expect(workerBadges(busy, [{ kind: "stalled", task: "T-9" }])).toEqual([]);
  });

  it("marks no worker when the agent has no attention item", () => {
    expect(workerBadges(busy, [])).toEqual([]);
  });

  it("marks a claim stale by its lease or by a stale_claim item", () => {
    expect(claimBadges(claim({ stale: true }), [])).toEqual(["stale"]);
    expect(
      claimBadges(claim({ task: "T-3" }), [
        { kind: "stale_claim", task: "T-3" },
      ]),
    ).toEqual(["stale"]);
    expect(claimBadges(claim(), [])).toEqual([]);
  });
});

describe("footLine with the integrator granter", () => {
  it("names the human that granted integrator when the payload says", () => {
    expect(
      footLine(
        agent("review", {
          tags: ["integrator"],
          integrator_granted_by: "human:tanim",
        }),
      ),
    ).toBe("integrator granted by tanim");
  });

  it("leaves the granter out when it is null or the tag is not held", () => {
    expect(footLine(agent("review", { tags: ["integrator"] }))).toBe(
      "no claims",
    );
    expect(
      footLine(agent("review", { integrator_granted_by: "human:tanim" })),
    ).toBe("no claims");
  });
});
