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
}

/** The newest events first, whatever order the cache holds them in. */
export function eventRows(
  events: readonly AnyEvent[],
  now: number,
): EventRow[] {
  return [...events]
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
  worker: string;
  /** `P-4 v2 in ws/review-p-4`. */
  where: string;
  /** `ran cargo nextest, 48 passed · 2 min ago`; the word before the first command. */
  last: string;
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
      checkout.last_command === null
        ? "no command yet"
        : `${checkout.last_command.result} · ${formatSince(now, checkout.last_command.at)}`,
  }));
}
