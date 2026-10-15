import { formatAsOf } from "@/format/time";
import type { MessageView, RecipientView } from "@/api/messages";
import { bareName } from "@/people/names";
import { isHuman } from "./filters";

const shown = bareName;

/** `acknowledged by frontend/ui · 14:09:00`, `read by tanim · 14:12:00` for a human, `delivered to backend · 14:08:05` or `not delivered to review`. */
export function recipientState(r: RecipientView, now: number): string {
  const name = shown(r.name);
  if (r.acknowledged_at !== null) {
    const verb = isHuman(r.name) ? "read by" : "acknowledged by";
    return `${verb} ${name} · ${formatAsOf(now, r.acknowledged_at)}`;
  }
  if (r.delivered_at !== null) {
    return `delivered to ${name} · ${formatAsOf(now, r.delivered_at)}`;
  }
  return `not delivered to ${name}`;
}

/** One line per recipient, in the order the daemon lists them. */
export function deliveryLines(message: MessageView, now: number): string[] {
  return message.recipients.map((r) => recipientState(r, now));
}

/** The recipients as the thread shows them: `frontend/ui, backend`. */
export function recipientNames(message: MessageView): string {
  return message.recipients.map((r) => shown(r.name)).join(", ");
}

/** `tanim → frontend/ui, backend`. */
export function routeLine(message: MessageView): string {
  return `${shown(message.sender)} → ${recipientNames(message)}`;
}

/** The last message of a thread as `frontend/ui: Using the short versions now.` */
export function previewLine(message: MessageView): string {
  return `${shown(message.sender)}: ${message.body}`;
}
