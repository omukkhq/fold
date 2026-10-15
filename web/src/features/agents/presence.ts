import type { BadgeTone } from "@/components/badge/badges";
import { presenceSpec } from "@/components/avatar/presence";
import { formatAsOf, formatSince } from "@/format/time";
import type {
  AgentView,
  AttentionItem,
  ClaimView,
  SessionView,
  WorkerView,
} from "@/api/agents";

export const INTEGRATOR_TAG = "integrator";

function plural(count: number, noun: string): string {
  return `${String(count)} ${noun}${count === 1 ? "" : "s"}`;
}

/** The presence line under an agent's name. The state is the daemon's; this only words it. */
export function presenceLine(agent: AgentView, now: number): string {
  const state = presenceSpec(agent.presence).label.toLowerCase();
  const sessions = plural(agent.sessions.length, "session");
  if (agent.last_call_at === null) {
    return `${state} · ${sessions} · no call yet`;
  }
  return `${state} · ${sessions} · last call ${formatSince(now, agent.last_call_at)}`;
}

/** `session since 13:30:00 · pid 81231 · ~/ledger`. */
export function sessionLine(session: SessionView, now: number): string {
  return `session since ${formatAsOf(now, session.started_at)} · pid ${String(session.pid)} · ${session.cwd}`;
}

/** What the worker did last, for example `editing routes/login.tsx · 12 s ago`. */
export function activityText(worker: WorkerView, now: number): string {
  const { activity } = worker;
  if (!activity) {
    return worker.claim === null ? "no claim of its own" : "no call yet";
  }
  const what = activity.detail
    ? `${activity.tool} ${activity.detail}`
    : activity.tool;
  return `${what} · ${formatSince(now, activity.at)}`;
}

/** `T-12 · ws-7` for an implementation claim, `P-4 review` for a review claim, or null for a worker with no claim. */
export function claimText(
  worker: WorkerView,
  claims: ClaimView[],
): string | null {
  if (worker.claim === null) {
    return null;
  }
  const claim = claims.find((c) => c.task === worker.claim);
  if (claim?.kind === "review") {
    return `${claim.proposal ?? claim.task} review`;
  }
  return claim?.workspace ? `${claim.task} · ${claim.workspace}` : worker.claim;
}

/** A review claim is on a proposal and links to it; an implementation claim links to its task. */
export function claimTarget(
  slug: string,
  claim: ClaimView,
): { id: string; href: string } {
  if (claim.kind === "review" && claim.proposal !== null) {
    return {
      id: claim.proposal,
      href: `/p/${slug}/proposals/${claim.proposal}`,
    };
  }
  return { id: claim.task, href: `/p/${slug}/t/${claim.task}` };
}

function hasItem(
  attention: AttentionItem[],
  kind: AttentionItem["kind"],
  task: string | null,
): boolean {
  return (
    task !== null && attention.some((i) => i.kind === kind && i.task === task)
  );
}

/** `stalled` for a worker whose task has a stalled item. The web compares no times. */
export function workerBadges(
  worker: WorkerView,
  attention: AttentionItem[],
): BadgeTone[] {
  return hasItem(attention, "stalled", worker.claim) ? ["stalled"] : [];
}

/** `stale` for a claim whose task has a stale_claim item. */
export function claimBadges(
  claim: ClaimView,
  attention: AttentionItem[],
): BadgeTone[] {
  return hasItem(attention, "stale_claim", claim.task) ? ["stale"] : [];
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

/** `2 claims · 2 workspaces · 1 open proposal`, or `1 review claim · integrator granted by tanim`; a count of zero is left out. */
export function footLine(agent: AgentView): string {
  const implementation = agent.claims.filter(
    (c) => c.kind === "implementation",
  );
  const review = agent.claims.length - implementation.length;
  const proposals = new Set(
    implementation.flatMap((c) => (c.proposal === null ? [] : [c.proposal])),
  ).size;
  const counts: [number, string][] = [
    [implementation.length, "claim"],
    [review, "review claim"],
    [agent.workspaces.length, "workspace"],
    [proposals, "open proposal"],
  ];
  const parts = counts
    .filter(([count]) => count > 0)
    .map(([count, noun]) => plural(count, noun));
  if (hasIntegrator(agent) && agent.integrator_granted_by !== null) {
    parts.push(
      `integrator granted by ${agent.integrator_granted_by.replace(/^human:/, "")}`,
    );
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
      if (worker.parent === parent && !seen.has(worker.label)) {
        seen.add(worker.label);
        out.push({ worker, depth });
        visit(worker.label, depth + 1);
      }
    }
  };
  visit(null, 0);
  // A worker whose parent is missing from the payload still shows, at the top.
  for (const worker of workers) {
    if (!seen.has(worker.label)) {
      seen.add(worker.label);
      out.push({ worker, depth: 0 });
      visit(worker.label, 1);
    }
  }
  return out;
}
