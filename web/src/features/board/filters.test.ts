import { describe, expect, it } from "vitest";
import type { AttentionKind, BoardCard } from "@/api/board";
import { boardCard } from "@/test/payloads";
import { areaEntry, workerList } from "@/test/workers";
import {
  applyFilters,
  choiceTitle,
  labelOptions,
  needsYouCount,
} from "./filters";
import { areaOwner, ownersOf } from "./owners";

const make = boardCard;

const ui = { name: "ui", color: "sage" };
const data = { name: "data", color: "gold" };
const main = { name: "main", color: "ink" };
const attention = (kind: AttentionKind) => [
  { kind, closable: kind === "question", action: null },
];

const cards = [
  make("T-1", { worker: "web-t1", labels: [ui] }),
  make("T-2", { worker: "api-t2", labels: [data], blocked: true }),
  make("T-3", {
    worker: "main",
    labels: [main],
    attention: attention("question"),
  }),
  make("T-4", { attention: attention("human_review"), labels: [ui, data] }),
  make("T-5", { kind: "human", attention: attention("stalled") }),
  make("T-6", {
    status: "done",
    attention: attention("stale_claim"),
    blocked: true,
    labels: [ui],
  }),
  make("T-7", { title: "Rate-limit the login endpoint" }),
];

const owners = ownersOf(
  workerList({
    areas: [
      areaEntry("api", "data", { lead: "api-lead" }),
      areaEntry("web", "ui", { lead: "web-lead" }),
    ],
  }),
);

const tasks = (filtered: BoardCard[]) => filtered.map((c) => c.task);
const run = (filters: Parameters<typeof applyFilters>[1]) =>
  tasks(applyFilters(cards, filters, owners));

describe("applyFilters", () => {
  it("keeps every card with no filter", () => {
    expect(applyFilters(cards, {}, null)).toHaveLength(cards.length);
  });

  it("matches any ticked label", () => {
    expect(run({ label: ["data"] })).toEqual(["T-2", "T-4"]);
    expect(run({ label: ["data", "main"] })).toEqual(["T-2", "T-3", "T-4"]);
    expect(run({ label: ["gone"] })).toEqual([]);
  });

  it("matches any ticked owner, an area lead in every column and Main agent with the tasks in no area", () => {
    expect(run({ owner: [areaOwner("web")] })).toEqual(["T-1", "T-4", "T-6"]);
    expect(run({ owner: ["human", "main"] })).toEqual(["T-3", "T-5", "T-7"]);
    expect(run({ owner: ["main"] })).toEqual(["T-3", "T-7"]);
    expect(run({ owner: [areaOwner("api"), "human"] })).toEqual([
      "T-2",
      "T-4",
      "T-5",
    ]);
  });

  it("matches only human tasks for an owner it cannot tell before the workers answer", () => {
    expect(
      tasks(applyFilters(cards, { owner: ["human", areaOwner("web")] }, null)),
    ).toEqual(["T-5"]);
  });

  it("keeps only blocked cards", () => {
    expect(run({ blocked: true })).toEqual(["T-2", "T-6"]);
  });

  it("matches Attention on any attention item", () => {
    expect(run({ needs: true })).toEqual(["T-3", "T-4", "T-5", "T-6"]);
  });

  it("searches the title and the exact id", () => {
    expect(run({ q: "rate-LIMIT" })).toEqual(["T-7"]);
    expect(run({ q: "t-2" })).toEqual(["T-2"]);
    expect(run({ q: "   " })).toHaveLength(cards.length);
  });

  it("needs every filter to match, while one tick inside a filter is enough", () => {
    expect(
      run({ owner: [areaOwner("web"), "main"], label: ["data", "main"] }),
    ).toEqual(["T-3", "T-4"]);
    expect(
      run({
        owner: [areaOwner("web"), areaOwner("api")],
        label: ["ui", "data"],
        blocked: true,
        needs: true,
      }),
    ).toEqual(["T-6"]);
    expect(run({ label: ["ui", "data"], needs: true, q: "T-4" })).toEqual([
      "T-4",
    ]);
  });
});

describe("labelOptions", () => {
  it("lists each label once, by name", () => {
    expect(labelOptions(cards).map((l) => l.name)).toEqual([
      "data",
      "main",
      "ui",
    ]);
  });

  it("keeps a ticked label no card carries, so it can be unticked", () => {
    expect(labelOptions(cards, ["ui", "gone"]).map((l) => l.name)).toEqual([
      "data",
      "main",
      "ui",
      "gone",
    ]);
  });
});

describe("choiceTitle", () => {
  const title = { none: "Labels", one: "Label", many: "Labels" };
  it("names one ticked entry and counts several", () => {
    expect(choiceTitle(title, [])).toBe("Labels");
    expect(choiceTitle(title, ["ui"])).toBe("Label: ui");
    expect(choiceTitle(title, ["ui", "data"])).toBe("Labels: 2");
  });
});

describe("needsYouCount", () => {
  it("counts the cards with any attention item", () => {
    expect(needsYouCount(cards)).toBe(4);
  });
});
