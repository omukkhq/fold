import type { RequestState, RequestSummary } from "@/api/requests";
import { isRequester } from "./actions";
import { kindOf, type KindKey } from "./present";
import { projectPath, withQuery } from "@/routes/paths";

export const STATES: readonly RequestState[] = [
  "open",
  "drafting",
  "draft_ready",
  "changes_requested",
  "approved",
  "closed",
];

export const KINDS: readonly KindKey[] = [
  "bug",
  "feature",
  "question",
  "unsorted",
];

/** The list's address: one state segment (all when absent), one kind toggle and the Mine toggle. */
export interface ListSearch {
  state?: RequestState;
  kind?: KindKey;
  mine?: true;
}

function isState(value: unknown): value is RequestState {
  return STATES.some((s) => s === value);
}

function isKind(value: unknown): value is KindKey {
  return KINDS.some((k) => k === value);
}

/** Unknown values are dropped, so an old or edited address still opens the list. */
export function parseListSearch(search: Record<string, unknown>): ListSearch {
  const { state, kind, mine } = search;
  return {
    ...(isState(state) ? { state } : {}),
    ...(isKind(kind) ? { kind } : {}),
    ...(mine === true || mine === "true" ? { mine: true as const } : {}),
  };
}

/** The signed-in human's own requests; with no name nothing is theirs. */
export const isMine = isRequester;

/** The kind and Mine filters, which the state counts respect. */
function narrowed(
  requests: readonly RequestSummary[],
  search: ListSearch,
  human: string | null,
): RequestSummary[] {
  return requests
    .filter((r) => search.kind === undefined || kindOf(r) === search.kind)
    .filter((r) => !search.mine || isMine(r, human));
}

/** The rows in the daemon's order, newest first. */
export function filterRequests(
  requests: readonly RequestSummary[],
  search: ListSearch,
  human: string | null,
): RequestSummary[] {
  return narrowed(requests, search, human).filter(
    (r) => search.state === undefined || r.state === search.state,
  );
}

export type StateCounts = Record<RequestState | "all", number>;

export function countByState(
  requests: readonly RequestSummary[],
  search: ListSearch,
  human: string | null,
): StateCounts {
  const counts: StateCounts = {
    all: 0,
    open: 0,
    drafting: 0,
    draft_ready: 0,
    changes_requested: 0,
    approved: 0,
    closed: 0,
  };
  for (const r of narrowed(requests, search, human)) {
    counts.all += 1;
    counts[r.state] += 1;
  }
  return counts;
}

export function isNarrowed(search: ListSearch): boolean {
  return (
    search.kind !== undefined ||
    search.mine === true ||
    search.state !== undefined
  );
}

export function listHref(slug: string, search: ListSearch): string {
  const params = new URLSearchParams();
  if (search.state) params.set("state", search.state);
  if (search.kind) params.set("kind", search.kind);
  if (search.mine) params.set("mine", "true");
  return withQuery(projectPath(slug, "requests"), params);
}

/** Pressing the active kind clears it; pressing another replaces it. */
export function toggleKind(search: ListSearch, kind: KindKey): ListSearch {
  const { kind: current, ...rest } = search;
  return current === kind ? rest : { ...rest, kind };
}

export function toggleMine(search: ListSearch): ListSearch {
  const { mine, ...rest } = search;
  return mine ? rest : { ...rest, mine: true };
}
