import { parseId } from "@/format/ids";
import type { ThreadFilter } from "./filters";

export interface MessagesSearch extends ThreadFilter {
  /** A thread id, or the id of a message in it. */
  thread?: string;
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 && value.length <= 80
    ? value
    : undefined;
}

function idOf(value: unknown, kind: string): string | undefined {
  const raw = text(value);
  return raw !== undefined && parseId(raw)?.kind === kind ? raw : undefined;
}

/** Keeps the search fields that hold a well-formed value; the URL is user input. */
export function normalizeSearch(raw: Record<string, unknown>): MessagesSearch {
  const search: MessagesSearch = {};
  const agent = text(raw.agent);
  const task = idOf(raw.task, "task");
  const proposal = idOf(raw.proposal, "proposal");
  const thread = idOf(raw.thread, "message");
  if (agent !== undefined) search.agent = agent;
  if (task !== undefined) search.task = task;
  if (proposal !== undefined) search.proposal = proposal;
  if (thread !== undefined) search.thread = thread;
  return search;
}

/** The messages path for a search, with only the set fields. */
export function messagesHref(slug: string, search: MessagesSearch): string {
  const params = new URLSearchParams();
  for (const key of ["agent", "task", "proposal", "thread"] as const) {
    const value = search[key];
    if (value !== undefined) params.set(key, value);
  }
  const query = params.toString();
  return `/p/${slug}/messages${query ? `?${query}` : ""}`;
}
