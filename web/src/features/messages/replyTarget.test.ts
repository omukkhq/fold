import { describe, expect, it } from "vitest";
import { message, recipient, thread } from "@/test/messages";
import {
  askWaits,
  asksYou,
  freeAskWaitsForYou,
  isAsk,
  replyTarget,
} from "./replyTarget";

const toYou = [recipient("human:tanim")];
const choices = [
  { label: "Recharts", description: null },
  { label: "Plain SVG", description: null },
];
const question = message("m-1", {
  sender: "frontend",
  recipients: toYou,
  choices,
});
const free = message("m-2", {
  sender: "backend",
  recipients: toYou,
  asks_human: true,
});
const note = message("m-3", { sender: "frontend", recipients: toYou });
const answer = message("m-4", { sender: "human:tanim" });

describe("asks", () => {
  it("counts a question with choices and a marked ask as asks", () => {
    expect([question, free, note].map(isAsk)).toEqual([true, true, false]);
  });

  it("asks the signed-in human by name, or any human with no one signed in", () => {
    const toRafeed = message("m-5", {
      recipients: [recipient("human:rafeed")],
    });
    expect(asksYou(toRafeed, "tanim")).toBe(false);
    expect(asksYou(toRafeed, null)).toBe(true);
    expect(
      asksYou(message("m-6", { recipients: [recipient("main")] }), null),
    ).toBe(false);
    expect(asksYou(question, "tanim")).toBe(true);
  });

  it("waits until a human writes after it; a free ask also until the thread is marked answered", () => {
    const open = thread("m-1", {
      awaiting_human: true,
      messages: [question, free],
    });
    expect(askWaits(open, question)).toBe(true);
    expect(askWaits(open, free)).toBe(true);
    expect(askWaits(open, note)).toBe(false);
    const marked = { ...open, awaiting_human: false };
    expect(askWaits(marked, question)).toBe(true);
    expect(askWaits(marked, free)).toBe(false);
    const answered = { ...open, messages: [question, free, answer] };
    expect(askWaits(answered, question)).toBe(false);
    expect(askWaits(answered, free)).toBe(false);
  });

  it("marks only a waiting free ask to you", () => {
    const open = thread("m-1", {
      awaiting_human: true,
      messages: [question, free],
    });
    expect(freeAskWaitsForYou(open, free, "tanim")).toBe(true);
    expect(freeAskWaitsForYou(open, free, "rafeed")).toBe(false);
    expect(freeAskWaitsForYou(open, question, "tanim")).toBe(false);
  });
});

describe("replyTarget", () => {
  it("has nothing to reply to in an empty thread", () => {
    expect(replyTarget(thread("m-1"), "tanim", null)).toBeNull();
  });

  it("replies to the newest message when no free ask waits for you", () => {
    const shown = thread("m-1", {
      awaiting_human: true,
      messages: [question, note],
    });
    expect(replyTarget(shown, "tanim", null)).toEqual({
      replyTo: "m-3",
      answers: null,
    });
  });

  it("answers the picked question while it waits, and forgets it once answered", () => {
    const shown = thread("m-1", {
      awaiting_human: true,
      messages: [question, note],
    });
    expect(replyTarget(shown, "tanim", "m-1")).toEqual({
      replyTo: "m-1",
      answers: question,
    });
    const answered = { ...shown, messages: [question, note, answer] };
    expect(replyTarget(answered, "tanim", "m-1")).toEqual({
      replyTo: "m-4",
      answers: null,
    });
    expect(replyTarget(shown, "tanim", "m-3")?.answers).toBeNull();
  });

  it("answers the newest free ask to you while it is the newest waiting ask", () => {
    const shown = thread("m-1", {
      awaiting_human: true,
      messages: [question, free, note],
    });
    expect(replyTarget(shown, "tanim", null)).toEqual({
      replyTo: "m-2",
      answers: free,
    });
    expect(replyTarget(shown, "rafeed", null)?.answers).toBeNull();
    const later = { ...shown, messages: [free, question] };
    expect(replyTarget(later, "tanim", null)).toEqual({
      replyTo: "m-1",
      answers: null,
    });
  });
});
