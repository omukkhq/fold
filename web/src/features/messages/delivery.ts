import { formatAsOf } from "@/format/time";
import type { MessageView, RecipientView } from "@/api/messages";
import { isHumanName } from "@/people/names";
import { actorSegment, seg, type Segment } from "@/people/segments";

/** One recipient's delivery: `acknowledged by` / `read by` (a human) / `delivered to` / `not delivered to`, the recipient, the time the daemon recorded, if any, and a note when fold cannot tell the agent that the message waits. */
export interface Delivery {
  lead: string;
  actor: string;
  at: string | null;
  note: string | null;
}

export function recipientState(r: RecipientView, now: number): Delivery {
  if (r.acknowledged_at !== null) {
    return {
      lead: isHumanName(r.name) ? "read by" : "acknowledged by",
      actor: r.name,
      at: formatAsOf(now, r.acknowledged_at),
      note: null,
    };
  }
  if (r.delivered_at !== null) {
    return {
      lead: "delivered to",
      actor: r.name,
      at: formatAsOf(now, r.delivered_at),
      note: null,
    };
  }
  return {
    lead: "not delivered to",
    actor: r.name,
    at: null,
    note:
      r.reachable === false
        ? "not reachable now · it gets the message when it reads its inbox"
        : null,
  };
}

/** One line per recipient, in the order the daemon lists them. */
export function deliveryLines(message: MessageView, now: number): Delivery[] {
  return message.recipients.map((r) => recipientState(r, now));
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
