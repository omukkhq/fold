import { type CostRange, isCostRange } from "@/features/cost/cost";

const QUESTION = /^pq-[1-9][0-9]{0,9}$/;
const BATCH = /^rb-[1-9][0-9]{0,9}$/;

export interface AgentsSearch {
  /** The permission question the side panel shows, such as `pq-3`. */
  question?: string;
  /** The Cost tab; the Team tab without it. */
  tab?: "cost";
  /** The range of the Cost tab; all time without it. */
  range?: CostRange;
}

/** Keeps a well-formed question id, the Cost tab and a known range; the URL is user input. */
export function parseAgentsSearch(raw: Record<string, unknown>): AgentsSearch {
  const search: AgentsSearch = {};
  const { question, tab, range } = raw;
  if (typeof question === "string" && QUESTION.test(question)) {
    search.question = question;
  }
  if (tab === "cost") {
    search.tab = tab;
  }
  if (isCostRange(range) && range !== "all") {
    search.range = range;
  }
  return search;
}

export function isBatchId(id: string): boolean {
  return BATCH.test(id);
}
