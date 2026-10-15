import { describe, expect, it } from "vitest";
import type { AgentView, ClaimView, WorkerView } from "@/api/agents";
import {
  activityText,
  claimBadges,
  claimTarget,
  claimText,
  footLine,
  hasIntegrator,
  isEditableTag,
  orderWorkers,
  plainTags,
  presenceLine,
  sessionLine,
  workerBadges,
} from "./presence";

const NOW = Date.UTC(2026, 9, 1, 12, 0, 0);

function worker(label: string, fields: Partial<WorkerView> = {}): WorkerView {
  const slash = label.indexOf("/");
  return {
    label,
    parent: slash < 0 ? null : label.slice(0, slash),
    presence: "active",
    last_call_at: null,
    activity: null,
    claim: null,
    ...fields,
  };
}

function agent(fields: Partial<AgentView> = {}): AgentView {
  return {
    name: "frontend",
    tags: [],
    presence: "active",
    last_call_at: NOW - 12_000,
    sessions: [],
    workers: [],
    claims: [],
    workspaces: [],
    events: [],
    attention: [],
    integrator_granted_by: null,
    ...fields,
  };
}

const claim: ClaimView = {
  task: "T-12",
  kind: "implementation",
  worker: "frontend/ui",
  lease: "held",
  workspace: "ws-7",
  proposal: null,
};

describe("presenceLine", () => {
  it("words the state the payload gives", () => {
    const session = {
      id: "s",
      started_at: NOW,
      last_call_at: NOW,
      pid: 1,
      cwd: "~",
    };
    expect(presenceLine(agent({ sessions: [session] }), NOW)).toBe(
      "active · 1 session · last call 12 s ago",
    );
    expect(presenceLine(agent({ presence: "idle" }), NOW)).toBe(
      "idle · 0 sessions · last call 12 s ago",
    );
  });
  it("says so when the agent never called", () => {
    expect(
      presenceLine(agent({ presence: "offline", last_call_at: null }), NOW),
    ).toBe("offline · 0 sessions · no call yet");
  });
});

describe("sessionLine", () => {
  it("shows start, pid and directory", () => {
    const line = sessionLine(
      { id: "s", started_at: NOW, last_call_at: NOW, pid: 81231, cwd: "~/x" },
      NOW,
    );
    expect(line).toMatch(/^session since \d\d:\d\d:\d\d · pid 81231 · ~\/x$/);
  });
});

describe("activityText", () => {
  it("names the tool, its target and the age", () => {
    const w = worker("frontend/ui", {
      claim: "T-12",
      activity: { tool: "editing", detail: "a.tsx", at: NOW - 12_000 },
    });
    expect(activityText(w, NOW)).toBe("editing a.tsx · 12 s ago");
  });
  it("leaves out a missing target", () => {
    const w = worker("frontend", {
      activity: { tool: "orchestrating", detail: null, at: NOW - 60_000 },
    });
    expect(activityText(w, NOW)).toBe("orchestrating · 1 min ago");
  });
  it("tells a worker without activity apart by its claim", () => {
    expect(activityText(worker("a"), NOW)).toBe("no claim of its own");
    expect(activityText(worker("a/b", { claim: "T-1" }), NOW)).toBe(
      "no call yet",
    );
  });
});

const review: ClaimView = {
  task: "T-10",
  kind: "review",
  worker: "review/quality",
  lease: "held",
  workspace: null,
  proposal: "P-4",
};

describe("claimText", () => {
  it("names the task and the workspace", () => {
    expect(claimText(worker("w", { claim: "T-12" }), [claim])).toBe(
      "T-12 · ws-7",
    );
  });
  it("names the proposal of a review claim", () => {
    expect(claimText(worker("w", { claim: "T-10" }), [review])).toBe(
      "P-4 review",
    );
    expect(
      claimText(worker("w", { claim: "T-10" }), [
        { ...review, proposal: null },
      ]),
    ).toBe("T-10 review");
  });
  it("shows a claim without a workspace and a worker without a claim", () => {
    expect(claimText(worker("w", { claim: "T-9" }), [])).toBe("T-9");
    expect(claimText(worker("w"), [claim])).toBeNull();
  });
});

describe("claimTarget", () => {
  it("links an implementation claim to its task", () => {
    expect(claimTarget("demo", claim)).toEqual({
      id: "T-12",
      href: "/p/demo/t/T-12",
    });
  });
  it("links a review claim to its proposal", () => {
    expect(claimTarget("demo", review)).toEqual({
      id: "P-4",
      href: "/p/demo/proposals/P-4",
    });
    expect(claimTarget("demo", { ...review, proposal: null }).href).toBe(
      "/p/demo/t/T-10",
    );
  });
});

describe("badges", () => {
  const attention = [
    { kind: "stalled" as const, task: "T-16" },
    { kind: "stale_claim" as const, task: "T-12" },
  ];
  it("gives a worker stalled only for its task's stalled item", () => {
    expect(workerBadges(worker("w", { claim: "T-16" }), attention)).toEqual([
      "stalled",
    ]);
    expect(workerBadges(worker("w", { claim: "T-12" }), attention)).toEqual([]);
    expect(workerBadges(worker("w"), attention)).toEqual([]);
  });
  it("gives a claim stale only for its task's stale_claim item", () => {
    expect(claimBadges(claim, attention)).toEqual(["stale"]);
    expect(claimBadges({ ...claim, task: "T-16" }, attention)).toEqual([]);
  });
  it("keeps stalled and stale apart for one task", () => {
    const both = [
      { kind: "stalled" as const, task: "T-1" },
      { kind: "stale_claim" as const, task: "T-1" },
    ];
    expect(workerBadges(worker("w", { claim: "T-1" }), both)).toEqual([
      "stalled",
    ]);
    expect(claimBadges({ ...claim, task: "T-1" }, both)).toEqual(["stale"]);
  });
});

describe("tags", () => {
  it("lets a human edit only integrator", () => {
    expect(isEditableTag("integrator")).toBe(true);
    expect(isEditableTag("frontend")).toBe(false);
  });
  it("splits integrator from the plain tags", () => {
    const a = agent({ tags: ["frontend", "integrator"] });
    expect(hasIntegrator(a)).toBe(true);
    expect(plainTags(a)).toEqual(["frontend"]);
    expect(hasIntegrator(agent())).toBe(false);
  });
});

describe("footLine", () => {
  const workspace = {
    workspace: "ws-7",
    task: "T-12",
    worker: "frontend/ui",
    summary: "1 file",
    last_activity_at: NOW,
  };
  it("counts claims, workspaces and open proposals", () => {
    expect(
      footLine(
        agent({
          claims: [claim, { ...claim, task: "T-11", proposal: "P-3" }],
          workspaces: [workspace, { ...workspace, workspace: "ws-5" }],
        }),
      ),
    ).toBe("2 claims · 2 workspaces · 1 open proposal");
  });
  it("counts review claims apart and names who granted integrator", () => {
    expect(
      footLine(
        agent({
          claims: [review],
          tags: ["integrator"],
          integrator_granted_by: "human:tanim",
        }),
      ),
    ).toBe("1 review claim · integrator granted by tanim");
  });
  it("says so when the agent holds nothing", () => {
    expect(footLine(agent())).toBe("no claims");
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
    expect(ordered.map((o) => [o.worker.label, o.depth])).toEqual([
      ["a", 0],
      ["a/x", 1],
      ["a/x/deep", 2],
      ["a/y", 1],
    ]);
  });
  it("still shows a worker whose parent is missing", () => {
    const ordered = orderWorkers([worker("orphan", { parent: "gone" })]);
    expect(ordered.map((o) => [o.worker.label, o.depth])).toEqual([
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
