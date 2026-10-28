import { describe, expect, it } from "vitest";
import { requestSummary } from "@/test/requests";
import {
  countByState,
  filterRequests,
  isMine,
  isNarrowed,
  listHref,
  parseListSearch,
  visibleStates,
  withKind,
  toggleMine,
} from "./filters";

const rows = [
  requestSummary("RQ-7"),
  requestSummary("RQ-6", {
    kind: "bug",
    state: "drafting",
    requester: "frontend",
  }),
  requestSummary("RQ-5", { kind: "question", state: "draft_ready" }),
  requestSummary("RQ-4", { kind: "feature", state: "approved" }),
];

describe("parseListSearch", () => {
  it("keeps known values and drops the rest", () => {
    expect(
      parseListSearch({ state: "draft_ready", kind: "unsorted", mine: "true" }),
    ).toEqual({
      state: "draft_ready",
      kind: "unsorted",
      mine: true,
    });
    expect(
      parseListSearch({ state: "ready", kind: "idea", mine: "yes" }),
    ).toEqual({});
    expect(parseListSearch({ mine: true })).toEqual({ mine: true });
  });
});

describe("filterRequests", () => {
  it("narrows by state, kind and the human's own requests", () => {
    const ids = (
      search: Parameters<typeof filterRequests>[1],
      human: string | null = "tanim",
    ) => filterRequests(rows, search, human).map((r) => r.id);
    expect(ids({})).toEqual(["RQ-7", "RQ-6", "RQ-5", "RQ-4"]);
    expect(ids({ state: "drafting" })).toEqual(["RQ-6"]);
    expect(ids({ kind: "unsorted" })).toEqual(["RQ-7"]);
    expect(ids({ mine: true })).toEqual(["RQ-7", "RQ-5", "RQ-4"]);
    expect(ids({ mine: true }, null)).toEqual([]);
  });

  it("knows the human's own requests", () => {
    expect(isMine(rows[0] ?? requestSummary("x"), "tanim")).toBe(true);
    expect(isMine(rows[1] ?? requestSummary("x"), "tanim")).toBe(false);
  });
});

describe("countByState", () => {
  it("counts every state under the kind and Mine filters", () => {
    expect(countByState(rows, {}, "tanim")).toEqual({
      all: 4,
      open: 1,
      drafting: 1,
      draft_ready: 1,
      changes_requested: 0,
      approved: 1,
      closed: 0,
    });
    expect(countByState(rows, { mine: true, state: "open" }, "tanim").all).toBe(
      3,
    );
  });
});

describe("addresses and toggles", () => {
  it("writes the list address", () => {
    expect(listHref("demo", {})).toBe("/p/demo/requests");
    expect(listHref("demo", { state: "open", kind: "bug", mine: true })).toBe(
      "/p/demo/requests?state=open&kind=bug&mine=true",
    );
  });

  it("sets one kind or none, and toggles Mine", () => {
    expect(withKind({}, "bug")).toEqual({ kind: "bug" });
    expect(withKind({ kind: "bug", mine: true }, "feature")).toEqual({
      kind: "feature",
      mine: true,
    });
    expect(withKind({ kind: "bug", state: "open" }, undefined)).toEqual({
      state: "open",
    });
    expect(toggleMine({})).toEqual({ mine: true });
    expect(toggleMine({ mine: true })).toEqual({});
  });

  it("shows only the states that have requests, and the chosen one", () => {
    const counts = countByState(
      [requestSummary("RQ-1"), requestSummary("RQ-2", { state: "approved" })],
      {},
      null,
    );
    expect(visibleStates(counts, undefined)).toEqual(["open", "approved"]);
    expect(visibleStates(counts, "closed")).toEqual([
      "open",
      "approved",
      "closed",
    ]);
  });

  it("says when a filter narrows the list", () => {
    expect(isNarrowed({})).toBe(false);
    expect(isNarrowed({ kind: "bug" })).toBe(true);
    expect(isNarrowed({ mine: true })).toBe(true);
    expect(isNarrowed({ state: "closed" })).toBe(true);
  });
});
