import type { BadgeTone } from "@/components/badge/badges";
import { presenceSpec } from "@/components/avatar/presence";
import { formatAsOf, formatSince } from "@/format/time";
import { bareName } from "@/people/names";
import type {
  AgentAttention,
  AgentView,
  ClaimView,
  SessionView,
  WorkerView,
  WorkspaceView,
} from "@/api/agents";

export const INTEGRATOR_TAG = "integrator";

function plural(count: number, noun: string): string {
  return `${String(count)} ${noun}${count === 1 ? "" : "s"}`;
}

/** The presence line under an agent's name. The state is the daemon's; this only words it. */
export function presenceLine(agent: AgentView, now: number): string {
  const state = presenceSpec(agent.presence).label.toLowerCase();
  const sessions = plural(agent.sessions, "session");
  if (agent.last_call_at === null) {
    return `${state} · ${sessions} · no call yet`;
  }
  return `${state} · ${sessions} · last call ${formatSince(now, agent.last_call_at)}`;
}

/** `session since 13:30:00 · pid 81231 · /home/tanim/ledger`; a session that sent no working directory ends at its pid. */
export function sessionLine(session: SessionView, now: number): string {
  const line = `session since ${formatAsOf(now, session.started_at)} · pid ${String(session.pid)}`;
  return session.cwd === null ? line : `${line} · ${session.cwd}`;
}

/** The session lines of the agent, oldest first. */
export function sessionLines(agent: AgentView): SessionView[] {
  return agent.open_sessions;
}

/** A session has no public id; one adapter process is one session, so its pid and start name it. */
export function sessionKey(session: SessionView): string {
  return `${String(session.pid)}:${String(session.started_at)}`;
}

function hasItem(
  attention: AgentAttention[],
  kind: AgentAttention["kind"],
  task: string,
): boolean {
  return attention.some((i) => i.kind === kind && i.task === task);
}

/** `stalled` for a worker whose claimed task has a stalled item. The web compares no times. */
export function workerBadges(
  worker: WorkerView,
  attention: AgentAttention[],
): BadgeTone[] {
  return worker.claims.some((claim) =>
    hasItem(attention, "stalled", claim.task),
  )
    ? ["stalled"]
    : [];
}

/** `stale` for a claim whose lease ran out, or whose task has a stale_claim item. */
export function claimBadges(
  claim: ClaimView,
  attention: AgentAttention[],
): BadgeTone[] {
  return claim.stale || hasItem(attention, "stale_claim", claim.task)
    ? ["stale"]
    : [];
}

/** What the worker did last, for example `edit_file · 12 s ago`. The daemon words the activity; the time is its last call. */
export function activityText(worker: WorkerView, now: number): string {
  if (worker.last_call_at === null) {
    return "no call yet";
  }
  const since = formatSince(now, worker.last_call_at);
  return worker.activity ? `${worker.activity} · ${since}` : since;
}

/** A claim with the worker that holds it. */
export interface HeldClaimRow {
  worker: string;
  claim: ClaimView;
}

/** Every claim of the agent's workers, in the order of the workers. */
export function agentClaims(agent: AgentView): HeldClaimRow[] {
  return agent.workers.flatMap((worker) =>
    worker.claims.map((claim) => ({ worker: worker.name, claim })),
  );
}

/** The workspace of an implementation claim: the worker's workspace on the claimed task. */
export function claimWorkspace(
  worker: WorkerView,
  claim: ClaimView,
): WorkspaceView | null {
  return claim.kind === "implementation"
    ? (worker.workspaces.find((ws) => ws.task === claim.task) ?? null)
    : null;
}

/** `T-12 · ws-7` for an implementation claim with a workspace, `T-12 review` for a review claim, `T-12` otherwise; null for a worker with no claim. Several claims are joined with a comma. */
export function claimText(worker: WorkerView): string | null {
  if (worker.claims.length === 0) {
    return null;
  }
  return worker.claims
    .map((claim) => {
      if (claim.kind === "review") {
        return `${claim.task} review`;
      }
      const workspace = claimWorkspace(worker, claim);
      return workspace ? `${claim.task} · ${workspace.ws}` : claim.task;
    })
    .join(", ");
}

/** Every claim links to its task: a review claim names the task of the reviewed proposal. */
export function claimTarget(
  slug: string,
  claim: ClaimView,
): { id: string; href: string } {
  return { id: claim.task, href: `/p/${slug}/t/${claim.task}` };
}

/** Every workspace of the agent's workers, in the order of the workers. */
export function agentWorkspaces(
  agent: AgentView,
): { worker: string; workspace: WorkspaceView }[] {
  return agent.workers.flatMap((worker) =>
    worker.workspaces.map((workspace) => ({ worker: worker.name, workspace })),
  );
}

/** `3 changed files`; the daemon's cached count for the workspace. */
export function changedFilesText(workspace: WorkspaceView): string {
  return plural(workspace.changed_files, "changed file");
}

/** A human edits no tag but `integrator`. */
export function isEditableTag(tag: string): boolean {
  return tag === INTEGRATOR_TAG;
}

export function hasIntegrator(agent: AgentView): boolean {
  return agent.tags.includes(INTEGRATOR_TAG);
}

/** The agent's tags without `integrator`, which has its own control. */
export function plainTags(agent: AgentView): string[] {
  return agent.tags.filter((tag) => !isEditableTag(tag));
}

/** `2 claims · 2 workspaces · 1 open proposal`, or `1 review claim · integrator granted by tanim`; a count of zero is left out, and so is an unknown granter. */
export function footLine(agent: AgentView): string {
  const claims = agentClaims(agent).map((row) => row.claim);
  const implementation = claims.filter((c) => c.kind === "implementation");
  const counts: [number, string][] = [
    [implementation.length, "claim"],
    [claims.length - implementation.length, "review claim"],
    [agentWorkspaces(agent).length, "workspace"],
    [
      agent.workers.reduce((n, w) => n + w.proposals.length, 0),
      "open proposal",
    ],
  ];
  const parts = counts
    .filter(([count]) => count > 0)
    .map(([count, noun]) => plural(count, noun));
  const granter = agent.integrator_granted_by;
  if (hasIntegrator(agent) && granter) {
    parts.push(`integrator granted by ${bareName(granter)}`);
  }
  return parts.length > 0 ? parts.join(" · ") : "no claims";
}

/** Each worker after its parent, with its depth, so the list reads as a tree. */
export function orderWorkers(
  workers: WorkerView[],
): { worker: WorkerView; depth: number }[] {
  const out: { worker: WorkerView; depth: number }[] = [];
  const seen = new Set<string>();
  const visit = (parent: string | null, depth: number) => {
    for (const worker of workers) {
      if (worker.parent === parent && !seen.has(worker.name)) {
        seen.add(worker.name);
        out.push({ worker, depth });
        visit(worker.name, depth + 1);
      }
    }
  };
  visit(null, 0);
  // A worker whose parent is missing from the payload still shows, at the top.
  for (const worker of workers) {
    if (!seen.has(worker.name)) {
      seen.add(worker.name);
      out.push({ worker, depth: 0 });
      visit(worker.name, 1);
    }
  }
  return out;
}
