import type { MessageView, ThreadView } from "@/api/messages";
import { isYou } from "@/people/names";
import { actorSegment, seg, type Segment } from "@/people/segments";
import { askWaits, asksYou } from "./replyTarget";

/** A thread row's second line: who asks you, or who wrote to whom first. */
export interface RowLine {
  segments: Segment[];
  /** The sender the line names, whose name the preview then leaves out. */
  sender: string | null;
  /** The line says who asks you. */
  asks: boolean;
}

function recipientsOf(message: MessageView): Segment[] {
  return message.recipients.flatMap((r, index) => [
    ...(index > 0 ? [", "] : []),
    actorSegment(r.name),
  ]);
}

/** `@web-worker-39 asks you` while the thread waits for you, else `@main to @backend-lead, @web-lead` or `You to @frontend`. */
export function rowLine(thread: ThreadView, you: string | null): RowLine {
  if (thread.awaiting_human) {
    const ask = thread.messages.findLast(
      (m) => askWaits(thread, m) && asksYou(m, you),
    );
    if (ask) {
      return {
        segments: seg`${actorSegment(ask.sender)} asks you`,
        sender: ask.sender,
        asks: true,
      };
    }
  }
  const first = thread.messages[0];
  if (!first) return { segments: [], sender: null, asks: false };
  const from = isYou(first.sender, you) ? "You" : actorSegment(first.sender);
  const to = first.recipients.length > 0 ? seg` to ${recipientsOf(first)}` : [];
  return { segments: seg`${from}${to}`, sender: first.sender, asks: false };
}

/** The newest message in one line: `You: …`, the body alone when the row's line already names its sender, else `@sender: …`. */
export function rowPreview(
  message: MessageView,
  you: string | null,
  named: string | null,
): Segment[] {
  if (isYou(message.sender, you)) return seg`You: ${message.body}`;
  if (message.sender === named) return [message.body];
  return seg`${actorSegment(message.sender)}: ${message.body}`;
}

/** A thread row's second line, after the title. */
export interface RowSecond {
  segments: Segment[];
  /** The line says who asks you, so the row draws it in the waiting tone. */
  asks: boolean;
}

/** `@web-worker-39 asks you` while the thread waits for you; the route of a thread whose one message is already its title; else the newest message with its sender. */
export function rowSecond(thread: ThreadView, you: string | null): RowSecond {
  const line = rowLine(thread, you);
  const last = thread.messages.at(-1);
  if (line.asks || last === undefined || thread.messages.length === 1) {
    return { segments: line.segments, asks: line.asks };
  }
  return { segments: rowPreview(last, you, null), asks: false };
}
