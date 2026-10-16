const QUESTION = /^pq-[1-9][0-9]{0,9}$/;
const BATCH = /^rb-[1-9][0-9]{0,9}$/;

export interface AgentsSearch {
  /** The permission question the side panel shows, such as `pq-3`. */
  question?: string;
}

/** Keeps a well-formed question id; the URL is user input. */
export function parseAgentsSearch(raw: Record<string, unknown>): AgentsSearch {
  const question = raw.question;
  return typeof question === "string" && QUESTION.test(question)
    ? { question }
    : {};
}

export function isBatchId(id: string): boolean {
  return BATCH.test(id);
}
