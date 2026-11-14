import { Hand, MessageCircle, UserCheck } from "lucide-react";
import { describe, expect, it } from "vitest";
import type { AttentionItem, AttentionKind } from "@/api/board";
import { actionOf, attentionMark, hasKind } from "./attention";

const items = (...kinds: AttentionKind[]): AttentionItem[] =>
  kinds.map((kind) => ({ kind, closable: kind === "question", action: null }));

describe("attentionMark per kind", () => {
  it("maps each kind to its icon and text", () => {
    expect(attentionMark(items("question"))).toMatchObject({
      icon: MessageCircle,
      text: "Question for you",
    });
    expect(attentionMark(items("human_review"))).toMatchObject({
      icon: UserCheck,
      text: "Your review",
    });
    expect(attentionMark(items("stalled"))).toMatchObject({
      icon: Hand,
      text: "Stalled · needs you",
    });
    expect(attentionMark(items("stale_claim"))).toMatchObject({
      icon: Hand,
      text: "Stale claim · needs you",
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
      tooltip:
        "Stalled: fold told the worker's lead and it did not start again, or no agent can act",
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
        "Stale claim: the worker made no fold call for its lease, and its lead did not act or no agent can",
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

describe("a mark that needs a person", () => {
  it("names the person's action for a stall or a stale claim", () => {
    expect(
      attentionMark([
        { kind: "stale_claim", closable: false, action: "message_worker" },
      ]),
    ).toEqual({
      icon: Hand,
      text: "Stale claim · message worker",
      tooltip:
        "Stale claim: the worker made no fold call for its lease, and its lead did not act or no agent can; message the worker, which resumes it in its session",
    });
    expect(
      attentionMark([
        { kind: "stalled", closable: false, action: "restart_worker" },
      ])?.text,
    ).toBe("Stalled · restart worker");
    expect(
      attentionMark([
        { kind: "stalled", closable: false, action: "restart_worker" },
      ])?.tooltip,
    ).toContain("; restart the worker in a new session");
  });

  it("says needs you only when the daemon names no action this build knows", () => {
    expect(
      attentionMark([{ kind: "stale_claim", closable: false, action: null }])
        ?.text,
    ).toBe("Stale claim · needs you");
    expect(attentionMark([{ kind: "stalled", action: "later" }])?.text).toBe(
      "Stalled · needs you",
    );
    expect(actionOf([{ kind: "stalled", action: "later" }], "stalled")).toBe(
      null,
    );
    expect(
      actionOf(
        [{ kind: "stale_claim", closable: false, action: "message_worker" }],
        "stale_claim",
      ),
    ).toBe("message_worker");
    expect(actionOf([], "stalled")).toBeNull();
  });
});
