import { describe, expect, it } from "vitest";
import {
  proposalDetail,
  proposalSummary,
  proposalVersion,
  review,
} from "@/test/proposals";
import type { ProposalDetail } from "@/api/proposals";
import {
  changeOptions,
  commentsOnSide,
  defaultSince,
  detailHref,
  emptyChangeText,
  parseDetailSearch,
  resolveView,
  shownVersion,
  versionTabs,
} from "./view";

/** P-4 v2 on r8 after a request for changes on v1 and an approval of v2; P-3 has one version with changes requested. */
function proposal(id: "P-3" | "P-4") {
  if (id === "P-3") {
    return proposalDetail(proposalSummary("P-3"), {
      reviews: [review(1, "review/security", "request_changes")],
    });
  }
  return proposalDetail(proposalSummary("P-4", { current_version: 2 }), {
    versions: [
      proposalVersion(1, { base: "r7", changeset: "C-11" }),
      proposalVersion(2, { base: "r8", changeset: "C-18" }),
    ],
    reviews: [
      review(1, "review/security", "request_changes"),
      review(2, "review/quality", "approve"),
    ],
  });
}

describe("parseDetailSearch", () => {
  it("keeps positive whole versions and a path", () => {
    expect(
      parseDetailSearch({ version: 1, since: "1", path: " a.sql " }),
    ).toEqual({
      version: 1,
      since: 1,
      path: "a.sql",
    });
  });

  it("drops anything else", () => {
    expect(parseDetailSearch({ version: 0, since: 1.5, path: "" })).toEqual({});
    expect(parseDetailSearch({ version: "x", path: 3 })).toEqual({});
  });
});

describe("resolveView", () => {
  const p = proposal("P-4");

  it("shows the current version's full change by default, and an earlier one when asked", () => {
    expect(resolveView(p, {})).toEqual({
      kind: "version",
      version: 2,
      current: true,
      range: { version: 2, since: null },
      label: "P-4 v2",
    });
    expect(resolveView(p, { version: 1 })).toEqual({
      kind: "version",
      version: 1,
      current: false,
      range: { version: 1, since: null },
      label: "P-4 v1",
    });
  });

  it("falls back to the current version for a version the proposal does not have", () => {
    expect(resolveView(p, { version: 7 })).toMatchObject({
      kind: "version",
      version: 2,
    });
  });

  it("reads the change since an earlier version only", () => {
    expect(resolveView(p, { since: 1 })).toEqual({
      kind: "since",
      version: 2,
      current: true,
      range: { version: 2, since: 1 },
      label: "P-4 v2",
    });
    expect(resolveView(p, { version: 1, since: 1 })).toMatchObject({
      kind: "version",
      version: 1,
    });
    expect(resolveView(p, { since: 5 })).toMatchObject({ kind: "version" });
  });

  it("gives comments the shown version in either view", () => {
    expect(shownVersion(resolveView(p, {}))).toBe(2);
    expect(shownVersion(resolveView(p, { since: 1 }))).toBe(2);
  });

  it("takes comments on both sides of a full change and on the new side only since an earlier version", () => {
    expect(commentsOnSide(resolveView(p, {}), "old")).toBe(true);
    expect(commentsOnSide(resolveView(p, { since: 1 }), "old")).toBe(false);
    expect(commentsOnSide(resolveView(p, { since: 1 }), "new")).toBe(true);
  });

  it("names what an empty range holds", () => {
    expect(emptyChangeText(resolveView(p, {}))).toBe(
      "P-4 v2 holds the same files as its base.",
    );
    expect(emptyChangeText(resolveView(p, { since: 1 }))).toBe(
      "P-4 v2 holds the same files as v1.",
    );
  });
});

describe("defaultSince", () => {
  const versions = [1, 2, 3].map((n) => proposalVersion(n));
  const p3 = (fields: Partial<ProposalDetail>) =>
    proposalDetail(proposalSummary("P-5", { current_version: 3 }), {
      versions,
      ...fields,
    });

  it("is the newest earlier version anyone gave a verdict on, a comment not counted", () => {
    const p = p3({
      reviews: [
        review(1, "reviewer", "request_changes"),
        review(2, "review", "comment"),
        review(3, "review", "approve"),
      ],
    });
    expect(defaultSince(p, 3)).toBe(1);
    expect(defaultSince(p, 2)).toBe(1);
    expect(defaultSince(p, 1)).toBeNull();
  });

  it("is nothing when no earlier version has a verdict", () => {
    expect(defaultSince(p3({}), 3)).toBeNull();
    expect(
      defaultSince(p3({ reviews: [review(2, "review", "comment")] }), 3),
    ).toBeNull();
  });
});

describe("changeOptions", () => {
  const p = proposal("P-4");

  it("offers the full change and the change since the version the review asked changes on", () => {
    expect(changeOptions(p, resolveView(p, {}))).toEqual([
      { key: "full", label: "Full change", search: {}, selected: true },
      {
        key: "since",
        label: "Since v1",
        search: { since: 1 },
        selected: false,
      },
    ]);
    expect(
      changeOptions(p, resolveView(p, { since: 1 }))?.map((o) => o.selected),
    ).toEqual([false, true]);
  });

  it("keeps an earlier version on screen, and offers nothing for a first version", () => {
    expect(changeOptions(p, resolveView(p, { version: 1 }))).toBeNull();
    const p3 = proposalDetail(proposalSummary("P-5", { current_version: 3 }), {
      versions: [1, 2, 3].map((n) => proposalVersion(n)),
      reviews: [review(1, "review", "request_changes")],
    });
    expect(
      changeOptions(p3, resolveView(p3, { version: 2 }))?.map((o) => o.search),
    ).toEqual([{ version: 2 }, { version: 2, since: 1 }]);
    const unreviewed = { ...p3, reviews: [] };
    expect(changeOptions(unreviewed, resolveView(unreviewed, {}))).toBeNull();
  });
});

describe("versionTabs", () => {
  it("lists the newest version first with its verdict, the shown version selected in either view", () => {
    const p = proposal("P-4");
    const tabs = versionTabs(p, resolveView(p, {}));
    expect(tabs.map((t) => [t.label, t.selected])).toEqual([
      ["v2 · approved", true],
      ["v1 · changes requested", false],
    ]);
    expect(tabs.map((t) => t.search)).toEqual([{}, { version: 1 }]);
    expect(
      versionTabs(p, resolveView(p, { since: 1 })).map((t) => t.selected),
    ).toEqual([true, false]);
  });
});

describe("detailHref", () => {
  it("builds the detail address", () => {
    expect(detailHref("demo", "P-4", {})).toBe("/p/demo/proposals/P-4");
    expect(
      detailHref("demo", "P-4", { version: 1, since: 1, path: "a b.sql" }),
    ).toBe("/p/demo/proposals/P-4?version=1&since=1&path=a+b.sql");
  });
});
