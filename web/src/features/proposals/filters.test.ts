import { describe, expect, it } from "vitest";
import type { ProposalSummary } from "@/api/proposals";
import {
  claimLabel,
  countByState,
  filterProposals,
  isNarrowed,
  listHref,
  parseListSearch,
  shownState,
} from "./filters";

function summary(
  id: string,
  fields: Partial<ProposalSummary> = {},
): ProposalSummary {
  return {
    id,
    title: id,
    task: "T-1",
    author: "backend",
    state: "open",
    current_version: 1,
    with: "reviewer",
    waiting_on: null,
    latest_verdict: null,
    human_review: null,
    human_approved: false,
    review_claim: null,
    conflicted: false,
    integrated_revision: null,
    updated_at: 0,
    ...fields,
  };
}

const all = [
  summary("P-1", { state: "integrated", updated_at: 1 }),
  summary("P-2", { review_claim: "review/quality", updated_at: 5 }),
  summary("P-3", {
    conflicted: true,
    review_claim: "review/quality",
    updated_at: 9,
  }),
  summary("P-4", { updated_at: 7 }),
  summary("P-5", { state: "closed", updated_at: 2 }),
];

const ids = (list: ProposalSummary[]) => list.map((p) => p.id);

describe("filterProposals", () => {
  it("shows open proposals by default, the most recently updated first", () => {
    expect(ids(filterProposals(all, {}))).toEqual(["P-3", "P-4", "P-2"]);
  });

  it("shows integrated and closed proposals under their segments", () => {
    expect(ids(filterProposals(all, { state: "integrated" }))).toEqual(["P-1"]);
    expect(ids(filterProposals(all, { state: "closed" }))).toEqual(["P-5"]);
  });

  it("filters by the review claim", () => {
    expect(ids(filterProposals(all, { claim: "claimed" }))).toEqual([
      "P-3",
      "P-2",
    ]);
    expect(ids(filterProposals(all, { claim: "unclaimed" }))).toEqual(["P-4"]);
  });

  it("keeps only conflicted proposals when the toggle is on", () => {
    expect(ids(filterProposals(all, { conflicted: true }))).toEqual(["P-3"]);
    expect(
      filterProposals(all, { conflicted: true, claim: "unclaimed" }),
    ).toEqual([]);
  });
});

describe("parseListSearch", () => {
  it("keeps known values and drops the default state", () => {
    expect(
      parseListSearch({ state: "closed", claim: "claimed", conflicted: true }),
    ).toEqual({
      state: "closed",
      claim: "claimed",
      conflicted: true,
    });
    expect(parseListSearch({ state: "open", conflicted: "true" })).toEqual({
      conflicted: true,
    });
  });

  it("drops unknown values", () => {
    expect(
      parseListSearch({ state: "merged", claim: "mine", conflicted: "no" }),
    ).toEqual({});
  });
});

describe("list helpers", () => {
  it("counts proposals per state", () => {
    expect(countByState(all)).toEqual({ open: 3, integrated: 1, closed: 1 });
  });

  it("says when a filter other than the state narrows the list", () => {
    expect(isNarrowed({ state: "closed" })).toBe(false);
    expect(isNarrowed({ claim: "unclaimed" })).toBe(true);
    expect(isNarrowed({ conflicted: true })).toBe(true);
  });

  it("builds the list address without the default state", () => {
    expect(listHref("demo", {})).toBe("/p/demo/proposals");
    expect(listHref("demo", { state: "open", claim: "claimed" })).toBe(
      "/p/demo/proposals?claim=claimed",
    );
    expect(listHref("demo", { state: "integrated", conflicted: true })).toBe(
      "/p/demo/proposals?state=integrated&conflicted=true",
    );
  });

  it("names the state and claim filters", () => {
    expect(shownState({})).toBe("open");
    expect(claimLabel(undefined)).toBe("Any claim");
    expect(claimLabel("claimed")).toBe("Claimed");
    expect(claimLabel("unclaimed")).toBe("Unclaimed");
  });
});
