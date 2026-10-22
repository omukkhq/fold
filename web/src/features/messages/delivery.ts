import { formatAsOfMinute } from "@/format/time";
import type { MessageView, RecipientView } from "@/api/messages";
import { atName, humanActor, isHumanName } from "@/people/names";
import { actorSegment, seg, type Segment } from "@/people/segments";

/** How far a message got to one recipient, worst first: `failed` (not delivered), `waiting` (the notice waits in a busy session), `forwarded` (a stopped worker's message went on to its lead), `delivered` (an agent read its inbox, or a human's Messages screen shows the message) and `read` (acknowledged or read). */
export type DeliveryTone =
  | "failed"
  | "waiting"
  | "forwarded"
  | "delivered"
  | "read";

/** One recipient's delivery: `acknowledged by` / `read by` (a human) / `delivered to` (the agent read its inbox, or a human's Messages screen shows the message from the time it was sent) / `waiting for` (the notice reached the agent's session, which takes it when its current step ends) / `not delivered to`, the recipient, the time the daemon recorded, if any, and a note that says why the message waits. A stopped task worker that could not start for the message has no lead word, and its note names the lead the daemon sent the message on to. */
export interface Delivery {
  tone: DeliveryTone;
  lead: string;
  actor: string;
  at: string | null;
  note: string | null;
}

export function recipientState(
  r: RecipientView,
  now: number,
  sender: string | null = null,
  sentAt: number | null = null,
): Delivery {
  if (r.acknowledged_at !== null) {
    return {
      tone: "read",
      lead: isHumanName(r.name) ? "read by" : "acknowledged by",
      actor: r.name,
      at: formatAsOfMinute(now, r.acknowledged_at),
      note: null,
    };
  }
  // A human's message shows in their Messages screen as soon as it is stored; no inbox read or notice applies to them.
  if (isHumanName(r.name)) {
    return {
      tone: "delivered",
      lead: "delivered to",
      actor: r.name,
      at: formatAsOfMinute(now, r.delivered_at ?? sentAt ?? now),
      note: null,
    };
  }
  if (r.delivered_at !== null) {
    return {
      tone: "delivered",
      lead: "delivered to",
      actor: r.name,
      at: formatAsOfMinute(now, r.delivered_at),
      note: null,
    };
  }
  if (r.forwarded_to !== null) {
    return {
      tone: "forwarded",
      lead: "",
      actor: r.name,
      at: null,
      note:
        r.forwarded_to === sender
          ? CANNOT_START
          : `${CANNOT_START} · sent on to ${atName(r.forwarded_to)}`,
    };
  }
  const unreachable = r.reachable === false ? UNREACHABLE_NOTE : null;
  // A notice that got through stays in the session even when a later post to the agent fails.
  if (r.pushed_at !== null) {
    return {
      tone: "waiting",
      lead: "waiting for",
      actor: r.name,
      at: formatAsOfMinute(now, r.pushed_at),
      note: unreachable ?? "it is busy until its current step ends",
    };
  }
  return {
    tone: "failed",
    lead: "not delivered to",
    actor: r.name,
    at: null,
    note: unreachable,
  };
}

const CANNOT_START = "cannot start now";

const UNREACHABLE_NOTE =
  "not reachable now · it gets the message when it reads its inbox";

/** One line per recipient, in the order the daemon lists them. */
export function deliveryLines(message: MessageView, now: number): Delivery[] {
  return message.recipients.map((r) =>
    recipientState(r, now, message.sender, message.sent_at),
  );
}

/** The recipients' actors, for the head of a message. */
export function recipientActors(message: MessageView): string[] {
  return message.recipients.map((r) => r.name);
}

function joinRecipients(message: MessageView): Segment[] {
  return message.recipients.flatMap((r, index) => [
    ...(index > 0 ? [", "] : []),
    actorSegment(r.name),
  ]);
}

/** `@tanim → @frontend/ui, @backend`. */
export function routeLine(message: MessageView): Segment[] {
  return seg`${actorSegment(message.sender)} → ${joinRecipients(message)}`;
}

/** The last message of a thread as `@frontend/ui: Using the short versions now.` */
export function previewLine(message: MessageView): Segment[] {
  return seg`${actorSegment(message.sender)}: ${message.body}`;
}

/** The newest message of the thread that waits for `human` to read it, the key of one mark-read call; null when nothing waits or no human is signed in. */
export function unreadBy(
  messages: readonly MessageView[],
  human: string | null,
): string | null {
  if (human === null) return null;
  const actor = humanActor(human);
  const waiting = messages.filter((message) =>
    message.recipients.some(
      (r) => r.name === actor && r.acknowledged_at === null,
    ),
  );
  return waiting.at(-1)?.id ?? null;
}
