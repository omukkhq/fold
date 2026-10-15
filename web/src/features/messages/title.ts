import type { ThreadView } from "@/api/messages";

export const MAX_TITLE = 80;

/** Messages have no subject, so a thread is named by the first line of its first message, cut at `MAX_TITLE` characters. */
export function threadTitle(thread: ThreadView): string {
  const first = thread.messages[0]?.body ?? "";
  const line =
    first
      .split("\n")
      .map((l) => l.trim())
      .find((l) => l.length > 0) ?? "";
  if (line === "") {
    return `Thread ${thread.id}`;
  }
  return line.length > MAX_TITLE ? `${line.slice(0, MAX_TITLE - 1)}…` : line;
}
