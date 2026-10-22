import { describe, expect, it } from "vitest";
import { proposalSummary, revision } from "@/test/proposals";
import { integratedRevisions, integrationOf, mainHead } from "./integration";

const integrated = proposalSummary("P-3", {
  state: "integrated",
  turn: null,
  current_version: 2,
  closed_at: 7,
  updated_at: 6,
});
const revisions = [revision(9, "P-3"), revision(8, "P-2"), revision(1)];

describe("integrationOf", () => {
  it("finds the revision of main that names the integrated proposal", () => {
    expect(integrationOf(integrated, revisions)).toEqual({
      revision: "r9",
      commit: null,
      version: 2,
      at: revision(9).created_at,
      actor: null,
      batch: null,
    });
  });

  it("names the review batch the proposal integrated with", () => {
    const batched = [{ ...revision(9, "P-3"), batch: "rb-2" }];
    expect(integrationOf(integrated, batched)?.batch).toBe("rb-2");
  });

  it("carries the Git commit of the revision once it is written", () => {
    const written = [{ ...revision(9, "P-3"), commit: "4f2a9c1e" }];
    expect(integrationOf(integrated, written)?.commit).toBe("4f2a9c1e");
  });

  it("takes the human's own integrate answer until the revisions name the proposal", () => {
    const answer = {
      proposal: "P-3",
      result: "succeeded" as const,
      revision: "r9",
      run: null,
    };
    expect(integrationOf(integrated, [], answer)).toEqual({
      revision: "r9",
      commit: null,
      version: 2,
      at: 7,
      actor: null,
      batch: null,
    });
    expect(
      integrationOf({ ...integrated, closed_at: null }, [], answer)?.at,
    ).toBe(6);
    expect(
      integrationOf(integrated, [], { ...answer, proposal: "P-4" }),
    ).toBeNull();
    expect(integrationOf(integrated, [])).toBeNull();
    expect(
      integrationOf(integrated, [], {
        ...answer,
        result: "queued",
        revision: null,
      }),
    ).toBeNull();
  });

  it("is null for a proposal that is not integrated", () => {
    expect(integrationOf(proposalSummary("P-3"), revisions)).toBeNull();
  });
});

describe("mainHead and integratedRevisions", () => {
  it("reads the head of main and each integrated proposal's newest revision", () => {
    expect(mainHead(revisions)).toBe("r9");
    expect(mainHead([])).toBeNull();
    expect([
      ...integratedRevisions([...revisions, revision(5, "P-3")]).entries(),
    ]).toEqual([
      ["P-3", "r9"],
      ["P-2", "r8"],
    ]);
  });
});
