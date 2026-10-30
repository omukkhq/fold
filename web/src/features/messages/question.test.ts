import { describe, expect, it } from "vitest";
import type { MessageView, ThreadView } from "@/api/messages";
import {
  chosenAnswers,
  chosenOption,
  isQuestion,
  questionState,
} from "./question";

function message(
  id: string,
  sender: string,
  body: string,
  labels: string[] = [],
): MessageView {
  return {
    id,
    sender,
    body,
    choices: labels.map((label) => ({ label, description: null })),
    header: null,
    asks_human: false,
    lines: null,
    questions: [],
    answers: [],
    answers_ask: null,
    worker_question: false,
    proposal: null,
    recipients: [],
    reply_to: null,
    request: null,
    sent_at: 0,
    task: null,
    thread: "m-1",
  };
}

function thread(messages: MessageView[]): ThreadView {
  return {
    id: "m-1",
    closable: false,
    messages,
    participants: [],
    awaiting_human: false,
    last_at: 0,
    proposal: null,
    request: null,
    task: null,
  };
}

const question = message("m-2", "web-lead", "Which library?", [
  "Recharts",
  "Plain SVG",
]);

describe("question", () => {
  it("tells a question by its choices", () => {
    expect(isQuestion(question)).toBe(true);
    expect(isQuestion(message("m-3", "web-lead", "hi"))).toBe(false);
  });

  it("matches an option without case and surrounding space", () => {
    expect(chosenOption(question.choices, "  plain svg ")).toBe("Plain SVG");
    expect(chosenOption(question.choices, "neither")).toBeNull();
  });

  it("takes the first later human message as the answer", () => {
    const waiting = thread([
      message("m-1", "human:tanim", "Add charts"),
      question,
      message("m-3", "web-lead", "Also, any colour?"),
    ]);
    expect(questionState(waiting, question)).toEqual({
      answer: null,
      chosen: null,
    });
    expect(chosenAnswers(waiting).size).toBe(0);

    const answer = message("m-4", "human:tanim", "recharts");
    const answered = thread([question, answer]);
    expect(questionState(answered, question)).toEqual({
      answer,
      chosen: "Recharts",
    });
    expect([...chosenAnswers(answered)]).toEqual(["m-4"]);

    const free = thread([question, message("m-4", "human:tanim", "Neither")]);
    expect(questionState(free, question).chosen).toBeNull();
    expect(chosenAnswers(free).size).toBe(0);
  });
});
