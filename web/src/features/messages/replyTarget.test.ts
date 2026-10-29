import { describe, expect, it } from "vitest";
import { message, recipient, thread } from "@/test/messages";
import { askWaits, asksYou, isAsk, replyTarget } from "./replyTarget";

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
  it("counts a question set, a question with choices and a marked ask as asks", () => {
    expect(
      isAsk(
        message("m-9", {
          questions: [{ text: "Connected?", header: null, choices: [] }],
        }),
      ),
    ).toBe(true);
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
});

describe("replyTarget", () => {
  it("has nothing to reply to in an empty thread", () => {
    expect(replyTarget(thread("m-1"), "tanim")).toBeNull();
  });

  it("replies to the newest message when no free ask waits for you", () => {
    const shown = thread("m-1", {
      awaiting_human: true,
      messages: [question, note],
    });
    expect(replyTarget(shown, "tanim")).toEqual({
      replyTo: "m-3",
      answers: null,
    });
  });

  it("answers the newest free ask to you while it is the newest waiting ask", () => {
    const shown = thread("m-1", {
      awaiting_human: true,
      messages: [question, free, note],
    });
    expect(replyTarget(shown, "tanim")).toEqual({
      replyTo: "m-2",
      answers: free,
    });
    expect(replyTarget(shown, "rafeed")?.answers).toBeNull();
    const later = { ...shown, messages: [free, question] };
    expect(replyTarget(later, "tanim")).toEqual({
      replyTo: "m-1",
      answers: null,
    });
  });
});
