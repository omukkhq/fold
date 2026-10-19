import type { ComparisonTotals, ProposalSummary } from "@/api/proposals";
import type { ReviewBatch, ReviewBatchMember } from "@/api/reviews";
import type { WorkerProcess } from "@/api/workers";
import { reviewerActor } from "@/people/names";
import {
  contextText,
  costText,
  roleName,
  type PillTone,
} from "@/features/workers/team";

export type VerdictKind =
  | "approved"
  | "changes"
  | "rejected"
  | "commented"
  | "conflict"
  | "reviewing"
  | "left"
  | "none";

export interface MemberRow {
  proposal: string;
  title: string | null;
  task: string;
  author: string | null;
  version: string;
  verdict: VerdictKind;
  /** The human a marked proposal still waits for, as an actor; empty when any human may approve. */
  needsHuman: string | null;
}

const VERDICTS = {
  approve: "approved",
  request_changes: "changes",
  reject: "rejected",
  comment: "commented",
} as const;

export const VERDICT_TEXT: Record<VerdictKind, string> = {
  approved: "approved",
  changes: "changes requested",
  rejected: "rejected",
  commented: "commented",
  conflict: "conflicts with main",
  reviewing: "reviewing",
  left: "left the batch",
  none: "no verdict",
};

function verdictOf(
  batch: ReviewBatch,
  member: ReviewBatchMember,
  summary: ProposalSummary | undefined,
): VerdictKind {
  const given = batch.verdicts.find(
    (v) => v.proposal === member.proposal && v.version === member.version,
  );
  if (given) return VERDICTS[given.verdict];
  if (summary?.conflicted && member.in_batch) return "conflict";
  if (batch.state !== "open") return "none";
  return member.in_batch ? "reviewing" : "left";
}

function needsHumanOf(summary: ProposalSummary | undefined): string | null {
  if (!summary?.human_review || summary.human_approved) return null;
  return reviewerActor(summary.human_review.reviewer) ?? "";
}

/** One row per member in merge order, with the proposal's title and author from the proposal list. */
export function memberRows(
  batch: ReviewBatch,
  proposals: readonly ProposalSummary[],
): MemberRow[] {
  return [...batch.members]
    .sort((a, b) => a.position - b.position)
    .map((member) => {
      const summary = proposals.find((p) => p.id === member.proposal);
      return {
        proposal: member.proposal,
        title: summary?.title ?? null,
        task: member.task,
        author: summary?.author ?? null,
        version: `v${String(member.version)} of ${String(member.current_version)}`,
        verdict: verdictOf(batch, member, summary),
        needsHuman: needsHumanOf(summary),
      };
    });
}

/** The change switch of the batch view: each member's full change, or only what changed after the newest earlier version that got a verdict, by anyone. */
export type ChangeMode = "full" | "since";

/** Only a batch with a member that has such an earlier version has something to switch. */
export function hasSince(batch: ReviewBatch): boolean {
  return batch.members.some((m) => m.since !== null);
}

/** `1 file · +3 −1`. */
export function countsText(totals: ComparisonTotals): string {
  const files = `${String(totals.files)} file${totals.files === 1 ? "" : "s"}`;
  return `${files} · +${String(totals.added_lines)} −${String(totals.removed_lines)}`;
}

export interface MemberChange {
  /** Null while the full change of the member is not read yet. */
  counts: string | null;
  note: string;
  /** The earlier version the change is read since; null for the full change. */
  since: number | null;
}

/** A member shows its change since the earlier version in `since` mode when it has one, and its full change otherwise. */
export function memberChange(
  member: ReviewBatchMember,
  mode: ChangeMode,
  full: ComparisonTotals | undefined,
): MemberChange {
  if (mode === "since" && member.since !== null) {
    return {
      counts: countsText(member.since.totals),
      note: `since v${String(member.since.version)}`,
      since: member.since.version,
    };
  }
  return {
    counts: full === undefined ? null : countsText(full),
    note: "full change",
    since: null,
  };
}

/** The proposal screen opens on the change the row shows. */
export function memberSearch(change: MemberChange | undefined): {
  since?: number;
} {
  return change === undefined || change.since === null
    ? {}
    : { since: change.since };
}

/** Each member's change by proposal; `full` holds the full changes read so far, by proposal. */
export function memberChanges(
  batch: ReviewBatch,
  mode: ChangeMode,
  full: ReadonlyMap<string, ComparisonTotals>,
): Map<string, MemberChange> {
  return new Map(
    batch.members.map((m) => [
      m.proposal,
      memberChange(m, mode, full.get(m.proposal)),
    ]),
  );
}

/** The members whose proposal conflicts with main, in merge order. */
export function conflictedMembers(rows: readonly MemberRow[]): string[] {
  return rows
    .filter((row) => row.verdict === "conflict")
    .map((r) => r.proposal);
}

/** `2 proposals, one verdict each`, counting the members still in an open batch. */
export function memberCountText(batch: ReviewBatch): string {
  const count =
    batch.state === "open"
      ? batch.members.filter((m) => m.in_batch).length
      : batch.members.length;
  return `${String(count)} proposal${count === 1 ? "" : "s"}, one verdict each`;
}

export interface CheckoutView {
  label: string;
  tone: PillTone;
  /** `ws-12 · main r17 + P-3 + P-4`; null without a checkout. */
  line: string | null;
  path: string | null;
}

export function checkoutView(batch: ReviewBatch): CheckoutView {
  const members = batch.members
    .filter((m) => m.in_batch)
    .map((m) => m.proposal);
  const checkout = batch.checkout;
  const line = checkout
    ? [`${checkout.workspace} · main ${checkout.base}`, ...members].join(" + ")
    : null;
  const path = checkout?.path ?? null;
  if (batch.state === "submitted") {
    return { label: "submitted", tone: "stopped", line, path };
  }
  if (batch.state === "closed") {
    return { label: "closed", tone: "stopped", line, path };
  }
  return checkout
    ? { label: "ready", tone: "ready", line, path }
    : { label: "no checkout", tone: "starting", line, path };
}

/** `21 turns · $1.64 · context 84k`. */
export function workerTotals(worker: WorkerProcess): string {
  return [
    `${String(worker.turns)} turn${worker.turns === 1 ? "" : "s"}`,
    costText(worker.cost_micro_usd),
    `context ${contextText(worker.context_tokens)}`,
  ].join(" · ");
}

/** `Reviewer · sonnet`, leaving out what the daemon does not know. */
export function reviewerLine(worker: WorkerProcess): string {
  return [roleName(worker.role), worker.model]
    .filter((part) => part !== null)
    .join(" · ");
}

/** The review lead that groups proposals into batches: the newest one fold runs or ran. */
export function reviewLead(workers: readonly WorkerProcess[]): string | null {
  return workers.filter((w) => w.role === "review_lead").at(-1)?.name ?? null;
}
