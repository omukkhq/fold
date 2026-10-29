import { describe, expect, it } from "vitest";
import { message, recipient, thread } from "@/test/messages";
import { needsItem } from "@/test/workers";
import { groupChats, type AgentChat } from "./chats";
import {
  answerStep,
  answeredAway,
  answeredPairs,
  chatSets,
  choiceKey,
  draftAnswers,
  draftComplete,
  headingQuestion,
  HEADING_QUESTION,
  listedQuestions,
  markScope,
  moreSetsText,
  openAskIds,
  ownStart,
  setQuestions,
  startDraft,
  stepBack,
  stepText,
  typesText,
  waitingSets,
} from "./sets";

const choices = [
  { label: "Yes, it is reconnected", description: null },
  { label: "Not yet", description: null },
];
const questions = [
  { text: "Is the MCP server connected again?", header: null, choices },
  { text: "Which keys must the workers use?", header: "Keys", choices: [] },
];
const first = message("m-1", {
  sender: "backend-lead",
  recipients: [recipient("human:tanim")],
  body: "Both workers wait.",
  questions,
  sent_at: 10,
});
const second = message("m-3", {
  sender: "backend-lead",
  recipients: [recipient("human:tanim")],
  body: "Pencil is open now?",
  questions: [{ text: "Pencil is open now?", header: null, choices: [] }],
  asks_human: true,
  sent_at: 30,
});
const note = message("m-2", {
  sender: "backend-lead",
  body: "fyi",
  sent_at: 20,
});

function chatOf(...threads: Parameters<typeof groupChats>[0]): AgentChat {
  const [chat] = groupChats(threads);
  if (!chat) throw new Error("no chat");
  return chat;
}

const chat = chatOf(
  thread("m-1", { messages: [first, note], last_at: 20 }),
  thread("m-3", { messages: [second], last_at: 30 }),
);

describe("open sets", () => {
  const items = [
    needsItem({ kind: "question", message: "m-1", since: 10 }),
    needsItem({ kind: "question", message: "m-3", humans: ["rafeed"] }),
    needsItem({ kind: "question", message: null }),
    needsItem({ kind: "human_review", message: "m-9" }),
  ];

  it("takes the asks of Needs you that wait for the signed-in human", () => {
    expect([...openAskIds(items, "tanim")]).toEqual(["m-1"]);
    expect([...openAskIds(items, null)]).toEqual(["m-1", "m-3"]);
    expect(waitingSets(items, "tanim")).toBe(1);
    expect(waitingSets([], "tanim")).toBe(0);
  });

  it("lists the chat's open sets oldest first", () => {
    const sets = chatSets(chat, new Set(["m-3", "m-1"]));
    expect(sets.map((s) => s.message.id)).toEqual(["m-1", "m-3"]);
    expect(sets[0]?.thread.id).toBe("m-1");
    expect(chatSets(chat, new Set(["m-2-gone"]))).toEqual([]);
  });

  it("knows when the set on the card left the open sets", () => {
    const sets = chatSets(chat, new Set(["m-3"]));
    expect(answeredAway("m-1", sets)).toBe(true);
    expect(answeredAway("m-3", sets)).toBe(false);
    expect(answeredAway(null, sets)).toBe(false);
  });
});

describe("set questions and their text", () => {
  it("reads the questions of a set, or the body of an ask without any", () => {
    expect(setQuestions(first)).toBe(questions);
    expect(setQuestions(message("m-5", { body: "  Why?  " }))).toEqual([
      { text: "Why?", header: null, choices: [] },
    ]);
  });

  it("shows a short one-line question as a heading", () => {
    expect(headingQuestion("Connected?")).toBe(true);
    expect(headingQuestion("x".repeat(HEADING_QUESTION + 1))).toBe(false);
    expect(headingQuestion("First line\nsecond")).toBe(false);
  });

  it("words the step and the sets that wait behind the card", () => {
    expect(stepText(0, 3)).toBe("1 of 3");
    expect(moreSetsText(0)).toBeNull();
    expect(moreSetsText(1)).toBe("1 more set waits");
    expect(moreSetsText(2)).toBe("2 more sets wait");
  });

  it("lists the questions of a set of several under its body only", () => {
    expect(listedQuestions(first)).toBe(questions);
    expect(listedQuestions(second)).toEqual([]);
  });
});

describe("the draft of a set's answers", () => {
  it("moves to the next question with each answer and completes after the last", () => {
    let draft = startDraft("m-1", 2);
    expect(draft).toEqual({ ask: "m-1", step: 0, answers: [null, null] });
    expect(answerStep(draft, "   ")).toBe(draft);
    draft = answerStep(draft, " Not yet ");
    expect(draft.step).toBe(1);
    expect(draftComplete(draft)).toBe(false);
    expect(draftAnswers(draft)).toEqual([]);
    draft = answerStep(draft, "Mock the calls");
    expect(draftComplete(draft)).toBe(true);
    expect(draftAnswers(draft)).toEqual(["Not yet", "Mock the calls"]);
    expect(answerStep(draft, "more")).toBe(draft);
  });

  it("goes back to change an earlier answer and keeps a written one", () => {
    let draft = answerStep(startDraft("m-1", 2), "My own words");
    draft = answerStep(draft, "Yes, it is reconnected");
    draft = stepBack(draft);
    expect(draft.step).toBe(1);
    const [connected, keys] = questions;
    if (!connected || !keys) throw new Error("no questions");
    expect(ownStart(draft, connected)).toBe("");
    draft = stepBack(draft);
    expect(draft.step).toBe(0);
    expect(stepBack(draft)).toBe(draft);
    expect(ownStart(draft, keys)).toBe("My own words");
    expect(ownStart(startDraft("m-1", 2), keys)).toBe("");
    draft = answerStep(draft, "Not yet");
    expect(draft.answers).toEqual(["Not yet", "Yes, it is reconnected"]);
  });
});

describe("keys", () => {
  it("picks a choice with its number key only", () => {
    expect(choiceKey("1", 2)).toBe(0);
    expect(choiceKey("2", 2)).toBe(1);
    expect(choiceKey("3", 2)).toBeNull();
    expect(choiceKey("0", 2)).toBeNull();
    expect(choiceKey("a", 2)).toBeNull();
    expect(choiceKey("12", 9)).toBeNull();
  });

  it("leaves a key typed into a field to the field", () => {
    expect(typesText({ tagName: "INPUT" })).toBe(true);
    expect(typesText({ tagName: "TEXTAREA" })).toBe(true);
    expect(typesText({ tagName: "select" })).toBe(true);
    expect(typesText({ tagName: "DIV", isContentEditable: true })).toBe(true);
    expect(typesText({ tagName: "BUTTON" })).toBe(false);
    expect(typesText({})).toBe(false);
    expect(typesText(null)).toBe(false);
  });
});

describe("answered sets", () => {
  const reply = message("m-4", {
    sender: "human:tanim",
    reply_to: "m-1",
    answers: ["Yes, it is reconnected", "Mock the calls"],
    answers_ask: "m-1",
  });
  const answered = thread("m-1", { messages: [first, reply] });

  it("pairs each question of the set with its answer", () => {
    expect(answeredPairs(reply, answered)).toEqual([
      {
        question: "Is the MCP server connected again?",
        answer: "Yes, it is reconnected",
      },
      {
        question: "Which keys must the workers use?",
        answer: "Mock the calls",
      },
    ]);
  });

  it("pairs nothing for a set of one, another message or a set outside the thread", () => {
    const one = { ...reply, answers: ["Yes"] };
    expect(answeredPairs(one, answered)).toBeNull();
    expect(answeredPairs(note, answered)).toBeNull();
    expect(
      answeredPairs(reply, thread("m-1", { messages: [reply] })),
    ).toBeNull();
    const short = thread("m-1", {
      messages: [{ ...first, questions: [] }, reply],
    });
    expect(answeredPairs(reply, short)?.[1]?.question).toBe("");
  });
});

describe("markScope", () => {
  const sets = chatSets(chat, new Set(["m-1"]));
  const [set] = sets;
  if (!set) throw new Error("no set");

  it("closes the set on the card alone, and none of a worker's AskUserQuestion", () => {
    expect(markScope(set, undefined)).toEqual({ ask: "m-1" });
    const worker = {
      ...set,
      message: { ...set.message, worker_question: true },
    };
    expect(markScope(worker, undefined)).toBeNull();
  });

  it("closes a closable thread without a card, else nothing", () => {
    const closable = thread("m-7", { awaiting_human: true, closable: true });
    expect(markScope(null, closable)).toEqual({ thread: "m-7" });
    expect(markScope(null, undefined)).toBeNull();
  });
});
