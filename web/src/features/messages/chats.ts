import type { MessageView, ThreadView } from "@/api/messages";
import type { IdKind } from "@/format/ids";
import { atName, humanActor, isHumanName } from "@/people/names";
import { threadTitle } from "./title";

/**
 * Everything the person and one agent wrote to each other, across the threads of tasks, proposals and requests.
 * Threads stay as fold stores them; a chat only shows them together.
 */
export interface AgentChat {
  /** The agent's name, or the thread id of a thread that has no agent. */
  key: string;
  /** The agent the person talks with; null for a thread that has no agent. */
  agent: string | null;
  /** Newest first. */
  threads: ThreadView[];
  /** UTC milliseconds of the newest message. */
  last_at: number;
}

/** fold writes only its own notes and reads no reply, so a chat is never with fold. */
function isChatAgent(actor: string): boolean {
  return !isHumanName(actor) && actor !== "fold";
}

/**
 * The agent a thread belongs to: the first agent that wrote in it, so a thread with several agents shows in the chat of the agent that started it.
 * A thread the person started belongs to the first agent its first message went to.
 */
export function threadAgent(thread: ThreadView): string | null {
  const writer = thread.messages.find((m) => isChatAgent(m.sender));
  const first = thread.messages[0];
  if (first !== undefined && isHumanName(first.sender)) {
    const to = first.recipients.find((r) => isChatAgent(r.name));
    if (to !== undefined) return to.name;
  }
  return writer?.sender ?? thread.participants.find(isChatAgent) ?? null;
}

/** The threads as one chat per agent, newest chat first; a thread without an agent is a chat of its own. */
export function groupChats(threads: readonly ThreadView[]): AgentChat[] {
  const chats = new Map<string, AgentChat>();
  for (const thread of threads) {
    const agent = threadAgent(thread);
    const key = agent ?? thread.id;
    const chat = chats.get(key) ?? { key, agent, threads: [], last_at: 0 };
    if (!chat.threads.some((t) => t.id === thread.id)) {
      chat.threads.push(thread);
    }
    chat.last_at = Math.max(chat.last_at, thread.last_at);
    chats.set(key, chat);
  }
  const list = [...chats.values()];
  for (const chat of list) {
    chat.threads.sort((a, b) => b.last_at - a.last_at);
  }
  return list.sort((a, b) => b.last_at - a.last_at);
}

/** The chat with this key. */
export function chatByKey(
  chats: readonly AgentChat[],
  key: string | undefined,
): AgentChat | null {
  return key === undefined ? null : (chats.find((c) => c.key === key) ?? null);
}

/** The thread with this id, or the thread that holds a message with it. */
export function threadIn(
  chat: AgentChat,
  id: string | undefined,
): ThreadView | null {
  if (id === undefined) return null;
  return (
    chat.threads.find((t) => t.id === id) ??
    chat.threads.find((t) => t.messages.some((m) => m.id === id)) ??
    null
  );
}

/** The chat that holds a thread with this id, or a message with it. */
export function chatHolding(
  chats: readonly AgentChat[],
  id: string | undefined,
): AgentChat | null {
  return chats.find((c) => threadIn(c, id) !== null) ?? null;
}

function idNumber(id: string): number {
  return Number(id.slice(id.indexOf("-") + 1));
}

/** Every message of the chat in time order; two messages of the same millisecond keep the order of their numbers. */
export function chatMessages(chat: AgentChat): MessageView[] {
  return chat.threads
    .flatMap((t) => t.messages)
    .sort((a, b) => a.sent_at - b.sent_at || idNumber(a.id) - idNumber(b.id));
}

/** The thread of each message of the chat, by message id. */
export function threadsByMessage(chat: AgentChat): Map<string, ThreadView> {
  const map = new Map<string, ThreadView>();
  for (const thread of chat.threads) {
    for (const message of thread.messages) {
      map.set(message.id, thread);
    }
  }
  return map;
}

/** The newest message of the chat. */
export function newestMessage(chat: AgentChat): MessageView | null {
  return chatMessages(chat).at(-1) ?? null;
}

/** What a message is about, as the id its chip shows: its request, else its proposal, else its task; the thread's own link fills in what a reply does not carry. */
export interface MessageContext {
  kind: IdKind;
  id: string;
}

export function messageContext(
  message: MessageView,
  thread: ThreadView,
): MessageContext | null {
  const request = message.request ?? thread.request;
  if (request !== null) return { kind: "request", id: request };
  const proposal = message.proposal ?? thread.proposal;
  if (proposal !== null) return { kind: "proposal", id: proposal };
  const task = message.task ?? thread.task;
  if (task !== null) return { kind: "task", id: task };
  return null;
}

/** What a thread is about: its request, else its proposal, else its task. */
export function threadContext(thread: ThreadView): MessageContext | null {
  if (thread.request !== null) return { kind: "request", id: thread.request };
  if (thread.proposal !== null) {
    return { kind: "proposal", id: thread.proposal };
  }
  if (thread.task !== null) return { kind: "task", id: thread.task };
  return null;
}

/** The distinct requests, proposals and tasks of the chat's threads, in the order of the threads, newest first. */
export function chatContexts(chat: AgentChat): MessageContext[] {
  const seen = new Set<string>();
  const list: MessageContext[] = [];
  for (const thread of chat.threads) {
    const items: MessageContext[] = [
      ...(thread.request === null
        ? []
        : [{ kind: "request" as const, id: thread.request }]),
      ...(thread.proposal === null
        ? []
        : [{ kind: "proposal" as const, id: thread.proposal }]),
      ...(thread.task === null
        ? []
        : [{ kind: "task" as const, id: thread.task }]),
    ];
    for (const item of items) {
      if (!seen.has(item.id)) {
        seen.add(item.id);
        list.push(item);
      }
    }
  }
  return list;
}

/** How many of a chat's tasks, proposals and requests its header names; the rest it counts as `+N`. */
export const HEADER_CONTEXTS = 3;

/** The newest contexts the chat's header names, and how many more the chat has. */
export function headerContexts(
  chat: AgentChat,
  max = HEADER_CONTEXTS,
): { shown: MessageContext[]; more: number } {
  const all = chatContexts(chat);
  return { shown: all.slice(0, max), more: Math.max(all.length - max, 0) };
}

/** Every person and agent of the chat's threads, in the order they first appear. */
export function chatParticipants(chat: AgentChat): string[] {
  return [
    ...new Set([...chat.threads].reverse().flatMap((t) => t.participants)),
  ];
}

/** The messages that wait for the signed-in human to read them; none when no human is signed in. */
export function unreadMessages(
  chat: AgentChat,
  you: string | null,
): MessageView[] {
  if (you === null) return [];
  const actor = humanActor(you);
  return chatMessages(chat).filter((m) =>
    m.recipients.some((r) => r.name === actor && r.acknowledged_at === null),
  );
}

/** The chat's name: `@frontend`, or the title of the one thread of a chat without an agent. */
export function chatName(chat: AgentChat): string {
  const first = chat.threads[0];
  if (chat.agent !== null) return atName(chat.agent);
  return first === undefined ? chat.key : threadTitle(first);
}
