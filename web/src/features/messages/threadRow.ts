import type { MessageView, ThreadView } from "@/api/messages";
import { isYou } from "@/people/names";
import { actorSegment, seg, type Segment } from "@/people/segments";
import { askWaits, asksYou } from "./replyTarget";

/** A thread row's second line: who asks you, or who wrote to whom first. */
export interface RowLine {
  segments: Segment[];
  /** The sender the line names, whose name the preview then leaves out. */
  sender: string | null;
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
      };
    }
  }
  const first = thread.messages[0];
  if (!first) return { segments: [], sender: null };
  const from = isYou(first.sender, you) ? "You" : actorSegment(first.sender);
  const to = first.recipients.length > 0 ? seg` to ${recipientsOf(first)}` : [];
  return { segments: seg`${from}${to}`, sender: first.sender };
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
