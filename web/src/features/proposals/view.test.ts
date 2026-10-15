import { describe, expect, it } from "vitest";
import type { ProposalDetail } from "@/api/proposals";
import { detail } from "../../../fixtures/proposals";
import {
  detailHref,
  parseDetailSearch,
  resolveView,
  shownVersion,
  versionTabs,
} from "./view";

function proposal(id: string): ProposalDetail {
  const found = detail("demo", id);
  if (!found) throw new Error(`no fixture ${id}`);
  return structuredClone(found);
}

describe("parseDetailSearch", () => {
  it("keeps positive whole versions and a path", () => {
    expect(
      parseDetailSearch({ version: 1, compare: "1", path: " a.sql " }),
    ).toEqual({
      version: 1,
      compare: 1,
      path: "a.sql",
    });
  });

  it("drops anything else", () => {
    expect(parseDetailSearch({ version: 0, compare: 1.5, path: "" })).toEqual(
      {},
    );
    expect(parseDetailSearch({ version: "x", path: 3 })).toEqual({});
  });
});

describe("resolveView", () => {
  const p = proposal("P-4");

  it("shows the current version by default and an earlier one when asked", () => {
    expect(resolveView(p, {})).toEqual({
      kind: "version",
      version: 2,
      current: true,
      to: "P-4 v2",
    });
    expect(resolveView(p, { version: 1 })).toEqual({
      kind: "version",
      version: 1,
      current: false,
      to: "P-4 v1",
    });
  });

  it("falls back to the current version for a version the proposal does not have", () => {
    expect(resolveView(p, { version: 7 })).toMatchObject({
      kind: "version",
      version: 2,
    });
  });

  it("compares an earlier version with the version shown", () => {
    expect(resolveView(p, { compare: 1 })).toEqual({
      kind: "compare",
      from: 1,
      version: 2,
      fromRef: "P-4 v1",
      to: "P-4 v2",
    });
    expect(resolveView(p, { version: 1, compare: 1 })).toMatchObject({
      kind: "version",
      version: 1,
    });
    expect(resolveView(p, { compare: 5 })).toMatchObject({ kind: "version" });
  });

  it("gives comments a version only when one version is shown", () => {
    expect(shownVersion(resolveView(p, {}))).toBe(2);
    expect(shownVersion(resolveView(p, { compare: 1 }))).toBeNull();
  });
});

describe("versionTabs", () => {
  it("lists the newest version first with its verdict, then the comparison", () => {
    const p = proposal("P-4");
    const tabs = versionTabs(p, resolveView(p, {}));
    expect(tabs.map((t) => [t.label, t.selected])).toEqual([
      ["v2 · approved", true],
      ["v1 · changes requested", false],
      ["Compare v1 → v2", false],
    ]);
    expect(tabs.map((t) => t.search)).toEqual([
      {},
      { version: 1 },
      { compare: 1 },
    ]);
    expect(
      versionTabs(p, resolveView(p, { compare: 1 })).at(-1)?.selected,
    ).toBe(true);
  });

  it("offers no comparison for a proposal with one version", () => {
    const p = proposal("P-3");
    expect(versionTabs(p, resolveView(p, {})).map((t) => t.label)).toEqual([
      "v1 · changes requested",
    ]);
  });
});

describe("detailHref", () => {
  it("builds the detail address", () => {
    expect(detailHref("demo", "P-4", {})).toBe("/p/demo/proposals/P-4");
    expect(
      detailHref("demo", "P-4", { version: 1, compare: 1, path: "a b.sql" }),
    ).toBe("/p/demo/proposals/P-4?version=1&compare=1&path=a+b.sql");
  });
});
