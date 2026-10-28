import type { ProposalState, ProposalSummary } from "@/api/proposals";
import { projectPath, withQuery } from "@/routes/paths";

export const STATES: readonly ProposalState[] = [
  "open",
  "integrated",
  "closed",
];

export type ClaimFilter = "claimed" | "unclaimed";

/** The list's address: the state segment, the claim filter and the conflicted toggle. */
export interface ListSearch {
  /** `open` when absent. */
  state?: ProposalState;
  claim?: ClaimFilter;
  conflicted?: true;
  /** Only the proposals whose current version waits for your review. */
  waiting?: true;
}

function isState(value: unknown): value is ProposalState {
  return STATES.some((state) => state === value);
}

/** Unknown values are dropped, so an old or edited address still opens the list. */
export function parseListSearch(search: Record<string, unknown>): ListSearch {
  const { state, claim, conflicted, waiting } = search;
  return {
    ...(isState(state) && state !== "open" ? { state } : {}),
    ...(claim === "claimed" || claim === "unclaimed" ? { claim } : {}),
    ...(conflicted === true || conflicted === "true"
      ? { conflicted: true as const }
      : {}),
    ...(waiting === true || waiting === "true"
      ? { waiting: true as const }
      : {}),
  };
}

export function shownState(search: ListSearch): ProposalState {
  return search.state ?? "open";
}

/** `waitingForYou` holds the ids the daemon names as waiting for your review; the waiting filter keeps only those. */
export function filterProposals(
  proposals: readonly ProposalSummary[],
  search: ListSearch,
  waitingForYou: readonly string[] = [],
): ProposalSummary[] {
  const state = shownState(search);
  return proposals
    .filter((p) => p.state === state)
    .filter((p) => !search.waiting || waitingForYou.includes(p.id))
    .filter((p) => {
      if (search.claim === "claimed") return p.review_claim !== null;
      if (search.claim === "unclaimed") return p.review_claim === null;
      return true;
    })
    .filter((p) => !search.conflicted || p.conflicted)
    .sort((a, b) => b.updated_at - a.updated_at);
}

export function countByState(
  proposals: readonly ProposalSummary[],
): Record<ProposalState, number> {
  const counts: Record<ProposalState, number> = {
    open: 0,
    integrated: 0,
    closed: 0,
  };
  for (const p of proposals) counts[p.state] += 1;
  return counts;
}

/** True when a filter other than the state segment narrows the list. */
export function isNarrowed(search: ListSearch): boolean {
  return (
    search.claim !== undefined ||
    search.conflicted === true ||
    search.waiting === true
  );
}

export function listHref(slug: string, search: ListSearch): string {
  const params = new URLSearchParams();
  if (search.state && search.state !== "open")
    params.set("state", search.state);
  if (search.claim) params.set("claim", search.claim);
  if (search.conflicted) params.set("conflicted", "true");
  if (search.waiting) params.set("waiting", "true");
  return withQuery(projectPath(slug, "proposals"), params);
}

export function claimLabel(claim: ClaimFilter | undefined): string {
  if (claim === "claimed") return "Claimed";
  if (claim === "unclaimed") return "Unclaimed";
  return "Any claim";
}

/** A filter the list applies besides the state segment, with the address that drops it. */
export interface ActiveFilter {
  key: "waiting" | "conflicted" | "claim";
  label: string;
  without: ListSearch;
}

/** The filters that narrow the list, in the order the Filter menu lists them, each with the address that removes it. */
export function activeFilters(search: ListSearch): ActiveFilter[] {
  const out: ActiveFilter[] = [];
  if (search.waiting) {
    out.push({
      key: "waiting",
      label: "Waits for you",
      without: { ...search, waiting: undefined },
    });
  }
  if (search.conflicted) {
    out.push({
      key: "conflicted",
      label: "Conflicted",
      without: { ...search, conflicted: undefined },
    });
  }
  if (search.claim !== undefined) {
    out.push({
      key: "claim",
      label: claimLabel(search.claim),
      without: { ...search, claim: undefined },
    });
  }
  return out;
}

/** The open proposals each toggle of the Filter menu would keep, with the claim filter as it is. */
export function menuCounts(
  proposals: readonly ProposalSummary[],
  search: ListSearch,
  waitingForYou: readonly string[],
): { waiting: number; conflicted: number } {
  const base = { state: search.state, claim: search.claim };
  return {
    waiting: filterProposals(
      proposals,
      { ...base, waiting: true },
      waitingForYou,
    ).length,
    conflicted: filterProposals(
      proposals,
      { ...base, conflicted: true },
      waitingForYou,
    ).length,
  };
}
