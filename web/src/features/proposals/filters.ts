import type { ProposalState, ProposalSummary } from "@/api/proposals";

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
}

function isState(value: unknown): value is ProposalState {
  return STATES.some((state) => state === value);
}

/** Unknown values are dropped, so an old or edited address still opens the list. */
export function parseListSearch(search: Record<string, unknown>): ListSearch {
  const { state, claim, conflicted } = search;
  return {
    ...(isState(state) && state !== "open" ? { state } : {}),
    ...(claim === "claimed" || claim === "unclaimed" ? { claim } : {}),
    ...(conflicted === true || conflicted === "true"
      ? { conflicted: true as const }
      : {}),
  };
}

export function shownState(search: ListSearch): ProposalState {
  return search.state ?? "open";
}

export function filterProposals(
  proposals: readonly ProposalSummary[],
  search: ListSearch,
): ProposalSummary[] {
  const state = shownState(search);
  return proposals
    .filter((p) => p.state === state)
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
  return search.claim !== undefined || search.conflicted === true;
}

export function listHref(slug: string, search: ListSearch): string {
  const params = new URLSearchParams();
  if (search.state && search.state !== "open")
    params.set("state", search.state);
  if (search.claim) params.set("claim", search.claim);
  if (search.conflicted) params.set("conflicted", "true");
  const query = params.toString();
  return `/p/${slug}/proposals${query ? `?${query}` : ""}`;
}

export function claimLabel(claim: ClaimFilter | undefined): string {
  if (claim === "claimed") return "Claimed";
  if (claim === "unclaimed") return "Unclaimed";
  return "Any claim";
}
