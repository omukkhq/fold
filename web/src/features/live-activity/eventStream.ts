import type { LucideIcon } from "lucide-react";
import type { ReviewCheckout } from "../../api/reviews";
import { formatAsOf, formatSince } from "../../format/time";
import { EVENT_LIMIT } from "../../live/eventList";
import type { ConnectionState } from "../../live/status";
import { eventLine, type AnyEvent } from "./eventLine";

export interface EventRow {
  seq: number;
  actor: string;
  /** Local clock time, `14:12:31`, with the date in front when it is not today. */
  time: string;
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
        time: formatAsOf(now, event.created_at),
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
  /** `P-4 v2 in ws/review-p-4`. */
  where: string;
  /** `ran cargo nextest, 48 passed · 2 min ago`, or `no command yet`; null when the read carries no command field, as the daemon records none. */
  last: string | null;
}

export function checkoutRows(
  checkouts: readonly ReviewCheckout[],
  now: number,
): CheckoutRow[] {
  return checkouts.map((checkout) => ({
    workspace: checkout.workspace,
    worker: checkout.worker,
    where: `${checkout.proposal} v${String(checkout.version)} in ${checkout.path}`,
    last:
      checkout.last_command === undefined
        ? null
        : checkout.last_command === null
          ? "no command yet"
          : `${checkout.last_command.result} · ${formatSince(now, checkout.last_command.at)}`,
  }));
}

/** The text after the worker: where the checkout is, the last command when known, and that nothing in it is recorded. */
export function checkoutText(row: CheckoutRow): string {
  return [row.where, row.last, "not recordable"]
    .filter((part): part is string => part !== null)
    .join(" · ");
}
