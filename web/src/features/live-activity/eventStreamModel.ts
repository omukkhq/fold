import type { LucideIcon } from "lucide-react";
import type { ReviewCheckout } from "../../api/reviews";
import { formatAsOfMinute } from "../../format/time";
import { EVENT_LIMIT } from "../../live/eventList";
import type { ConnectionState } from "../../live/status";
import { eventLine, type AnyEvent } from "./eventLine";

export interface EventRow {
  seq: number;
  actor: string;
  /** The local minute, `14:12`, with the date in front when it is not today; the rows of one minute share it. */
  minute: string;
  icon: LucideIcon;
  text: string;
  emphasis: boolean;
  /** The Git commit of the revision an integration line names, once `git.committed` has reported it; null otherwise. */
  commit: string | null;
}

/** The integration lines that show the commit of their revision. */
const COMMIT_LINES = new Set(["proposal.integrated", "integration.succeeded"]);

/** The Git commit of each revision the events report, by revision id. Each commit is written once, so a repeat keeps the first. */
export function commitsByRevision(
  events: readonly AnyEvent[],
): ReadonlyMap<string, string> {
  const commits = new Map<string, string>();
  for (const event of events) {
    const commit = event.data.commit;
    if (
      event.type === "git.committed" &&
      event.revision !== null &&
      typeof commit === "string" &&
      !commits.has(event.revision)
    ) {
      commits.set(event.revision, commit);
    }
  }
  return commits;
}

/** The newest events first, whatever order the cache holds them in. `git.committed` has no row of its own: it adds the commit to the integration line of its revision. */
export function eventRows(
  events: readonly AnyEvent[],
  now: number,
): EventRow[] {
  const commits = commitsByRevision(events);
  return [...events]
    .filter((event) => event.type !== "git.committed")
    .sort((a, b) => b.seq - a.seq)
    .slice(0, EVENT_LIMIT)
    .map((event) => {
      const line = eventLine(event);
      return {
        seq: event.seq,
        actor: event.actor,
        minute: formatAsOfMinute(now, event.created_at),
        icon: line.icon,
        text: line.text,
        emphasis: line.emphasis,
        commit:
          COMMIT_LINES.has(event.type) && event.revision !== null
            ? (commits.get(event.revision) ?? null)
            : null,
      };
    });
}

export interface EventGroup {
  minute: string;
  rows: EventRow[];
}

/** The rows in runs of one minute each, so the stream says each time once; the rows keep their order. */
export function eventGroups(rows: readonly EventRow[]): EventGroup[] {
  const groups: EventGroup[] = [];
  for (const row of rows) {
    const last = groups.at(-1);
    if (last?.minute === row.minute) {
      last.rows.push(row);
    } else {
      groups.push({ minute: row.minute, rows: [row] });
    }
  }
  return groups;
}

export interface LiveDot {
  label: string;
  tone: "live" | "connecting" | "reconnecting";
}

/** The dot beside the stream title follows the connection: only a connected stream is "live". */
export function liveDot(state: ConnectionState): LiveDot {
  switch (state) {
    case "live":
      return { label: "live", tone: "live" };
    case "reconnecting":
      return { label: "reconnecting", tone: "reconnecting" };
    case "connecting":
      return { label: "connecting", tone: "connecting" };
  }
}

export interface CheckoutRow {
  workspace: string;
  /** Null for a checkout whose worker is gone. */
  worker: string | null;
  /** `P-4 v2 in ws/review-p-4`, or for a batch `rb-3 · P-2 v1 + P-3 v1 in ws/review-rb-3`. */
  where: string;
}

/** The proposal versions a checkout's files hold: one, or a batch's members in merge order. */
function held(target: ReviewCheckout["target"]): string {
  const version = (proposal: string, v: number) => `${proposal} v${String(v)}`;
  if (target.kind === "proposal") {
    return version(target.proposal, target.version);
  }
  const members = target.members
    .map((member) => version(member.proposal, member.version))
    .join(" + ");
  return `${target.batch} · ${members}`;
}

export function checkoutRows(
  checkouts: readonly ReviewCheckout[],
): CheckoutRow[] {
  return checkouts.map((checkout) => ({
    workspace: checkout.workspace,
    worker: checkout.worker,
    where: `${held(checkout.target)} in ${checkout.path}`,
  }));
}

/** The text after the worker: where the checkout is, and that nothing in it is recorded. */
export function checkoutText(row: CheckoutRow): string {
  return `${row.where} · not recordable`;
}
