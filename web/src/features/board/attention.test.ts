import { Hand, MessageCircle, UserCheck } from "lucide-react";
import { describe, expect, it } from "vitest";
import type { AttentionItem, AttentionKind } from "@/api/board";
import { attentionMark, attentionSpec, hasKind } from "./attention";

const items = (...kinds: AttentionKind[]): AttentionItem[] =>
  kinds.map((kind) => ({ kind, closable: kind === "question" }));

describe("attentionSpec", () => {
  it("maps each kind to its badge and text", () => {
    expect(attentionSpec("question")).toMatchObject({
      icon: MessageCircle,
      tone: "needs-you",
      mark: "Question for you",
    });
    expect(attentionSpec("human_review")).toMatchObject({
      icon: UserCheck,
      tone: "human-review",
      mark: "Your review",
    });
    expect(attentionSpec("stalled")).toMatchObject({
      icon: Hand,
      tone: "stalled",
      mark: "Stalled · needs you",
    });
    expect(attentionSpec("stale_claim")).toMatchObject({
      icon: Hand,
      tone: "stale",
      mark: "Stale claim · needs you",
    });
  });
});

describe("hasKind", () => {
  it("finds a kind in the list", () => {
    expect(hasKind(items("stalled"), "stalled")).toBe(true);
    expect(hasKind(items("question"), "stalled")).toBe(false);
  });
});

describe("attentionMark", () => {
  it("is null for a card with no item", () => {
    expect(attentionMark([])).toBeNull();
  });

  it("shows the one item's text and names it in the tooltip", () => {
    expect(attentionMark(items("stalled"))).toEqual({
      icon: Hand,
      text: "Stalled · needs you",
      tooltip: "Stalled: its worker made no edits for a while",
    });
  });

  it("leads with a question, then a review, and names every item", () => {
    const mark = attentionMark(
      items("stale_claim", "human_review", "question"),
    );
    expect(mark?.text).toBe("Question for you");
    expect(mark?.icon).toBe(MessageCircle);
    expect(mark?.tooltip).toBe(
      [
        "Question: an agent waits for your answer",
        "Human review: a proposal waits for your approval",
        "Stale claim: the worker's lease ran out",
      ].join(" · "),
    );
  });

  it("names a kind once when the list repeats it", () => {
    expect(attentionMark(items("question", "question"))?.tooltip).toBe(
      "Question: an agent waits for your answer",
    );
  });

  it("ignores a kind this build does not know", () => {
    expect(attentionMark([{ kind: "later" }])).toBeNull();
  });
});
