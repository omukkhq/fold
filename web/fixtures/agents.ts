import type { AgentView, DurableEvent, WorkerView } from "../src/api/agents";
import { ApiError } from "../src/api/errors";

const NOW = Date.now();
const SEC = 1000;
const MIN = 60 * SEC;

function event(
  seq: number,
  type: DurableEvent["type"],
  actor: string,
  ago: number,
  related: Partial<Pick<DurableEvent, "task" | "proposal" | "workspace">>,
): DurableEvent {
  return {
    seq,
    type,
    actor,
    task: null,
    proposal: null,
    workspace: null,
    revision: null,
    data: {},
    created_at: NOW - ago,
    ...related,
  };
}

function worker(
  label: string,
  fields: Partial<WorkerView> & Pick<WorkerView, "presence">,
): WorkerView {
  const slash = label.indexOf("/");
  return {
    label,
    parent: slash < 0 ? null : label.slice(0, slash),
    last_call_at: null,
    activity: null,
    claim: null,
    ...fields,
  };
}

export const agents: AgentView[] = [
  {
    name: "frontend",
    tags: ["frontend", "ui"],
    presence: "active",
    last_call_at: NOW - 12 * SEC,
    sessions: [
      {
        id: "s-1",
        started_at: NOW - 40 * MIN,
        last_call_at: NOW - 12 * SEC,
        pid: 81231,
        cwd: "~/ledger",
      },
    ],
    workers: [
      worker("frontend", {
        presence: "active",
        last_call_at: NOW - 30 * SEC,
        activity: { tool: "orchestrating", detail: null, at: NOW - 30 * SEC },
      }),
      worker("frontend/ui", {
        presence: "active",
        last_call_at: NOW - 12 * SEC,
        activity: {
          tool: "editing",
          detail: "routes/login.tsx",
          at: NOW - 12 * SEC,
        },
        claim: "T-12",
      }),
      worker("frontend/styles", {
        presence: "idle",
        last_call_at: NOW - MIN,
        activity: { tool: "reading", detail: "P-3 review", at: NOW - MIN },
        claim: "T-11",
      }),
    ],
    claims: [
      {
        task: "T-12",
        kind: "implementation",
        worker: "frontend/ui",
        lease: "held",
        workspace: "ws-7",
        proposal: null,
      },
      {
        task: "T-11",
        kind: "implementation",
        worker: "frontend/styles",
        lease: "held",
        workspace: "ws-5",
        proposal: "P-3",
      },
    ],
    workspaces: [
      {
        workspace: "ws-7",
        task: "T-12",
        worker: "frontend/ui",
        summary: "3 files, +40 -12",
        last_activity_at: NOW - 12 * SEC,
      },
      {
        workspace: "ws-5",
        task: "T-11",
        worker: "frontend/styles",
        summary: "1 file, +8 -2",
        last_activity_at: NOW - MIN,
      },
    ],
    events: [
      event(41, "claim.acquired", "frontend/ui", 38 * MIN, {
        task: "T-12",
        workspace: "ws-7",
      }),
      event(44, "checkpoint.recorded", "frontend/ui", 9 * MIN, {
        task: "T-12",
        workspace: "ws-7",
      }),
      event(47, "proposal.version_added", "frontend/styles", 2 * MIN, {
        task: "T-11",
        proposal: "P-3",
      }),
    ],
    attention: [],
    integrator_granted_by: null,
  },
  {
    name: "backend",
    tags: ["backend"],
    presence: "active",
    last_call_at: NOW - 4 * SEC,
    sessions: [
      {
        id: "s-2",
        started_at: NOW - 39 * MIN,
        last_call_at: NOW - 4 * SEC,
        pid: 81302,
        cwd: "~/ledger",
      },
    ],
    workers: [
      worker("backend", {
        presence: "active",
        last_call_at: NOW - 20 * SEC,
        activity: { tool: "orchestrating", detail: null, at: NOW - 20 * SEC },
      }),
      worker("backend/api", {
        presence: "active",
        last_call_at: NOW - 4 * SEC,
        activity: {
          tool: "editing",
          detail: "reports/filter.rs",
          at: NOW - 4 * SEC,
        },
        claim: "T-14",
      }),
      worker("backend/db", {
        presence: "idle",
        last_call_at: NOW - 6 * MIN,
        activity: {
          tool: "editing",
          detail: "migrations/0004_reports.sql",
          at: NOW - 6 * MIN,
        },
        claim: "T-16",
      }),
    ],
    claims: [
      {
        task: "T-14",
        kind: "implementation",
        worker: "backend/api",
        lease: "held",
        workspace: "ws-8",
        proposal: null,
      },
      {
        task: "T-16",
        kind: "implementation",
        worker: "backend/db",
        lease: "held",
        workspace: "ws-9",
        proposal: null,
      },
    ],
    workspaces: [
      {
        workspace: "ws-8",
        task: "T-14",
        worker: "backend/api",
        summary: "2 files, +61 -4",
        last_activity_at: NOW - 4 * SEC,
      },
      {
        workspace: "ws-9",
        task: "T-16",
        worker: "backend/db",
        summary: "1 file, +22 -0",
        last_activity_at: NOW - 6 * MIN,
      },
    ],
    events: [
      event(42, "claim.acquired", "backend/api", 37 * MIN, {
        task: "T-14",
        workspace: "ws-8",
      }),
      event(45, "claim.stale", "fold", 6 * MIN, { task: "T-16" }),
    ],
    attention: [
      { kind: "stalled", task: "T-16" },
      { kind: "stale_claim", task: "T-16" },
    ],
    integrator_granted_by: null,
  },
  {
    name: "review",
    tags: ["integrator"],
    presence: "idle",
    last_call_at: NOW - 40 * SEC,
    sessions: [
      {
        id: "s-3",
        started_at: NOW - 38 * MIN,
        last_call_at: NOW - 40 * SEC,
        pid: 81377,
        cwd: "~/ledger",
      },
    ],
    workers: [
      worker("review", {
        presence: "idle",
        last_call_at: NOW - 40 * SEC,
        activity: {
          tool: "wait_for_events",
          detail: "proposals",
          at: NOW - 40 * SEC,
        },
      }),
      worker("review/quality", {
        presence: "idle",
        last_call_at: NOW - MIN,
        activity: { tool: "approved", detail: "P-4 v2", at: NOW - MIN },
        claim: "T-10",
      }),
      worker("review/security", { presence: "offline" }),
    ],
    claims: [
      {
        task: "T-10",
        kind: "review",
        worker: "review/quality",
        lease: "held",
        workspace: null,
        proposal: "P-4",
      },
    ],
    workspaces: [],
    events: [
      event(46, "review.submitted", "review/quality", MIN, {
        proposal: "P-4",
      }),
    ],
    attention: [],
    integrator_granted_by: "human:tanim",
  },
];

/** Stands in for the daemon: the tag list and the grant record change together. */
export function setIntegrator(agent: string, enabled: boolean): true {
  const found = agents.find((a) => a.name === agent);
  if (!found) {
    throw new ApiError("agent_not_found", `no agent is named ${agent}`);
  }
  const others = found.tags.filter((tag) => tag !== "integrator");
  found.tags = enabled ? [...others, "integrator"] : others;
  found.integrator_granted_by = enabled ? "human:tanim" : null;
  return true;
}
