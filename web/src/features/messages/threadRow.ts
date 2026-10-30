import type { MessageView } from "@/api/messages";
import { isYou } from "@/people/names";
import { actorSegment, seg, type Segment } from "@/people/segments";
import {
  chatName,
  newestMessage,
  unreadMessages,
  type AgentChat,
} from "./chats";
import { chatSets } from "./sets";

/** The newest message in one line: `You: …`, the body alone when the row already names its sender, else `@sender: …`. */
export function rowPreview(
  message: MessageView,
  you: string | null,
  named: string | null,
): Segment[] {
  if (isYou(message.sender, you)) return seg`You: ${message.body}`;
  if (message.sender === named) return [message.body];
  return seg`${actorSegment(message.sender)}: ${message.body}`;
}

/** A chat row: its title, the newest message, and how many question sets and other messages wait for the signed-in human. */
export interface ChatRow {
  /** The agent's name for a chat with an agent, else the title of its one thread. */
  title: { agent: string } | { text: string };
  preview: Segment[];
  /** The question sets that wait for the signed-in human's answer, one for each set. */
  questions: number;
  /** The unread messages that are not such a set, which the questions count already holds. */
  unread: number;
}

/** `open` holds the asks that wait for the signed-in human, as Needs you lists them. */
export function chatRow(
  chat: AgentChat,
  you: string | null,
  open: ReadonlySet<string>,
): ChatRow {
  const newest = newestMessage(chat);
  const asks = chatSets(chat, open);
  return {
    title:
      chat.agent === null ? { text: chatName(chat) } : { agent: chat.agent },
    preview: newest === null ? [] : rowPreview(newest, you, chat.agent),
    questions: asks.length,
    unread: unreadMessages(chat, you).filter((m) => !open.has(m.id)).length,
  };
}

/** `1 question set waits for you`, `2 question sets wait for you`. */
export function questionsLabel(count: number): string {
  return count === 1
    ? "1 question set waits for you"
    : `${String(count)} question sets wait for you`;
}

/** `1 unread message`, `3 unread messages`. */
export function unreadLabel(count: number): string {
  return `${String(count)} unread ${count === 1 ? "message" : "messages"}`;
}
