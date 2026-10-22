import type {
  AgentView,
  ClaimView,
  WorkerView,
  WorkspaceView,
} from "@/api/agents";

export function claim(fields: Partial<ClaimView> = {}): ClaimView {
  return {
    task: "T-12",
    kind: "implementation",
    acquired_at: 0,
    renewed_at: 0,
    stale: false,
    ...fields,
  };
}

export function workspace(fields: Partial<WorkspaceView> = {}): WorkspaceView {
  return {
    ws: "ws-7",
    task: "T-12",
    base: "r5",
    changed_files: 2,
    last_activity_at: null,
    latest_checkpoint: null,
    state: "active",
    ...fields,
  };
}

/** A worker of the given name. */
export function worker(
  name: string,
  fields: Partial<WorkerView> = {},
): WorkerView {
  return {
    name,
    last_call_at: null,
    activity: null,
    claims: [],
    workspaces: [],
    proposals: [],
    ...fields,
  };
}

export function agent(
  name: string,
  fields: Partial<AgentView> = {},
): AgentView {
  return {
    name,
    tags: [],
    presence: "active",
    last_call_at: null,
    sessions: 1,
    open_sessions: [],
    attention: [],
    workers: [worker(name)],
    events: [],
    process: null,
    notices: "socket",
    ...fields,
  };
}
