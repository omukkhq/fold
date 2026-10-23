import type { MessageView } from "@/api/messages";
import { bareName, isHumanName, isYou } from "@/people/names";
import { actorSegment, joinSegments, type Segment } from "@/people/segments";

/** Where a message sits: the signed-in human's own on the right, another human's and an agent's on the left, each with its own mark. */
export type MessageSide = "own" | "human" | "agent";

export function messageSide(sender: string, you: string | null): MessageSide {
  if (isYou(sender, you)) return "own";
  return isHumanName(sender) ? "human" : "agent";
}

/** The first letter of a person's name, upper case, for a round avatar. */
export function initial(actor: string): string {
  return bareName(actor).charAt(0).toUpperCase();
}

/** `to you and @rafeed`: the recipients with the signed-in human first. */
export function recipientsPhrase(
  message: MessageView,
  you: string | null,
): Segment[] {
  if (message.recipients.length === 0) return [];
  const names = message.recipients.map((r) => r.name);
  const ordered = [
    ...names.filter((n) => isYou(n, you)),
    ...names.filter((n) => !isYou(n, you)),
  ];
  return ["to ", ...joinSegments(ordered.map((n) => [actorSegment(n)]))];
}
