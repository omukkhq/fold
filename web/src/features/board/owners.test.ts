import { describe, expect, it } from "vitest";
import { boardCard } from "@/test/payloads";
import { areaEntry, workerList } from "@/test/workers";
import {
  areaOwner,
  needsOwners,
  ownerChoices,
  ownersOf,
  ownsCard,
} from "./owners";

const list = workerList({
  main_label: "main",
  areas: [
    areaEntry("api", "backend", { lead: "api-lead" }),
    areaEntry("web", "frontend"),
  ],
});
const owners = ownersOf(list);

describe("ownersOf", () => {
  it("lists Human, Main agent and one entry per area, an area without a lead by its name", () => {
    expect(owners.options).toEqual([
      { key: "human", label: "Human" },
      { key: "main", label: "Main agent" },
      { key: "area:api", label: "api-lead", hint: "api" },
      { key: "area:web", label: "web lead" },
    ]);
  });
});

describe("ownsCard", () => {
  const human = boardCard("T-1", { kind: "human" });
  const mainTask = boardCard("T-2", {
    labels: [{ name: "main", color: "ink" }],
  });
  const apiTask = boardCard("T-3", {
    status: "review",
    labels: [{ name: "backend", color: "sage" }],
  });
  const loose = boardCard("T-4", { labels: [{ name: "bug", color: "plum" }] });

  it("tells Human by the task kind, without the workers answer", () => {
    expect(ownsCard("human", human, null)).toBe(true);
    expect(ownsCard("human", mainTask, owners)).toBe(false);
  });

  it("gives Main agent the other tasks with the main label and those in no area, never a human task", () => {
    expect(ownsCard("main", mainTask, owners)).toBe(true);
    expect(ownsCard("main", loose, owners)).toBe(true);
    expect(ownsCard("main", apiTask, owners)).toBe(false);
    expect(ownsCard("main", mainTask, null)).toBe(false);
    const humanMain = boardCard("T-5", {
      kind: "human",
      labels: [{ name: "main", color: "ink" }],
    });
    expect(ownsCard("main", humanMain, owners)).toBe(false);
    expect(ownsCard("main", human, owners)).toBe(false);
    expect(ownsCard("human", humanMain, owners)).toBe(true);
  });

  it("gives an area lead every task with its area's label, a human task too, in any column", () => {
    expect(ownsCard(areaOwner("api"), apiTask, owners)).toBe(true);
    expect(ownsCard(areaOwner("web"), apiTask, owners)).toBe(false);
    const humanApi = boardCard("T-6", {
      kind: "human",
      labels: [{ name: "backend", color: "sage" }],
    });
    expect(ownsCard(areaOwner("api"), humanApi, owners)).toBe(true);
    expect(ownsCard("main", humanApi, owners)).toBe(false);
  });

  it("gives a task in no area no lead, and an unknown key nothing", () => {
    for (const key of ["human", "area:api", "area:web"]) {
      expect(ownsCard(key, loose, owners)).toBe(false);
    }
    expect(ownsCard("area:gone", apiTask, owners)).toBe(false);
    expect(ownsCard("backend", apiTask, owners)).toBe(false);
  });
});

describe("needsOwners", () => {
  it("needs the workers answer for every owner but Human", () => {
    expect(needsOwners(undefined)).toBe(false);
    expect(needsOwners(["human"])).toBe(false);
    expect(needsOwners(["human", "main"])).toBe(true);
    expect(needsOwners([areaOwner("web")])).toBe(true);
  });
});

describe("ownerChoices", () => {
  it("offers Human and Main agent before the workers answer arrives", () => {
    expect(ownerChoices(null, undefined).map((o) => o.key)).toEqual([
      "human",
      "main",
    ]);
  });

  it("keeps a ticked owner the answer does not know, so it can be unticked", () => {
    expect(
      ownerChoices(owners, ["area:gone", "human", "odd"]).slice(4),
    ).toEqual([
      { key: "area:gone", label: "gone lead" },
      { key: "odd", label: "odd" },
    ]);
  });
});
