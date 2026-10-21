import { describe, expect, it } from "vitest";
import type { AttentionKind, BoardCard } from "@/api/board";
import { boardCard } from "@/test/payloads";
import { agentOf, applyFilters, labelOptions, needsYouCount } from "./filters";

const make = boardCard;

const ui = { name: "ui", color: "sage" };
const data = { name: "data", color: "gold" };
const attention = (kind: AttentionKind) => [
  { kind, closable: kind === "question" },
];

const cards = [
  make("T-1", { worker: "frontend/ui", labels: [ui] }),
  make("T-2", { worker: "backend", labels: [data], blocked: true }),
  make("T-3", { worker: "backend/db", attention: attention("question") }),
  make("T-4", { attention: attention("human_review"), labels: [ui, data] }),
  make("T-5", { worker: "frontend", attention: attention("stalled") }),
  make("T-6", {
    worker: "frontend/styles",
    attention: attention("stale_claim"),
    blocked: true,
    labels: [ui],
  }),
  make("T-7", { title: "Rate-limit the login endpoint" }),
];

const tasks = (filtered: BoardCard[]) => filtered.map((c) => c.task);

describe("agentOf", () => {
  it("takes the agent of a worker label", () => {
    expect(agentOf("frontend/ui")).toBe("frontend");
  });

  it("treats a bare agent as its own main worker", () => {
    expect(agentOf("frontend")).toBe("frontend");
  });
});

describe("applyFilters", () => {
  it("keeps every card with no filter", () => {
    expect(applyFilters(cards, {})).toHaveLength(cards.length);
  });

  it("matches the agent of the responsible worker", () => {
    expect(tasks(applyFilters(cards, { agent: "frontend" }))).toEqual([
      "T-1",
      "T-5",
      "T-6",
    ]);
    expect(tasks(applyFilters(cards, { agent: "backend" }))).toEqual([
      "T-2",
      "T-3",
    ]);
  });

  it("drops cards nobody is responsible for under an agent filter", () => {
    expect(tasks(applyFilters(cards, { agent: "review" }))).toEqual([]);
  });

  it("matches a label by name", () => {
    expect(tasks(applyFilters(cards, { label: "data" }))).toEqual([
      "T-2",
      "T-4",
    ]);
  });

  it("keeps only blocked cards", () => {
    expect(tasks(applyFilters(cards, { blocked: true }))).toEqual([
      "T-2",
      "T-6",
    ]);
  });

  it("matches Attention on any attention item", () => {
    expect(tasks(applyFilters(cards, { needs: true }))).toEqual([
      "T-3",
      "T-4",
      "T-5",
      "T-6",
    ]);
  });

  it("searches the title and the exact id", () => {
    expect(tasks(applyFilters(cards, { q: "rate-LIMIT" }))).toEqual(["T-7"]);
    expect(tasks(applyFilters(cards, { q: "t-2" }))).toEqual(["T-2"]);
    expect(tasks(applyFilters(cards, { q: "   " }))).toHaveLength(cards.length);
  });

  it("combines filters so every one must match", () => {
    expect(
      tasks(applyFilters(cards, { agent: "frontend", label: "ui" })),
    ).toEqual(["T-1", "T-6"]);
    expect(
      tasks(
        applyFilters(cards, {
          agent: "frontend",
          label: "ui",
          blocked: true,
          needs: true,
        }),
      ),
    ).toEqual(["T-6"]);
    expect(
      tasks(applyFilters(cards, { agent: "backend", needs: true, q: "T-3" })),
    ).toEqual(["T-3"]);
  });
});

describe("labelOptions", () => {
  it("lists each label once, by name", () => {
    expect(labelOptions(cards).map((l) => l.name)).toEqual(["data", "ui"]);
  });
});

describe("needsYouCount", () => {
  it("counts the cards with any attention item", () => {
    expect(needsYouCount(cards)).toBe(4);
  });
});
