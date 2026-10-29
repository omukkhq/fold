import { describe, expect, it } from "vitest";
import { message, recipient, thread } from "@/test/messages";
import { groupChats, type AgentChat } from "./chats";
import {
  cursorAt,
  markableThread,
  openQuestions,
  queueIndex,
  startCursor,
  stepText,
  unmarkedAsk,
} from "./questionQueue";

const asks = (id: string, sentAt: number, fields = {}) =>
  message(id, {
    sender: "frontend",
    recipients: [recipient("human:tanim")],
    choices: [{ label: "Yes", description: null }],
    body: `Question ${id}`,
    sent_at: sentAt,
    ...fields,
  });

const late = thread("m-5", {
  awaiting_human: true,
  last_at: 50,
  messages: [asks("m-5", 50)],
});
const early = thread("m-1", {
  awaiting_human: true,
  last_at: 30,
  messages: [
    asks("m-1", 10),
    message("m-2", { sender: "frontend", body: "Also:", sent_at: 20 }),
    asks("m-3", 30, { choices: [], asks_human: true }),
  ],
});
const done = thread("m-6", {
  last_at: 40,
  messages: [
    asks("m-6", 35),
    message("m-7", { sender: "human:tanim", body: "Yes", sent_at: 40 }),
  ],
});
const other = thread("m-8", {
  awaiting_human: true,
  last_at: 45,
  messages: [asks("m-8", 45, { recipients: [recipient("human:rafeed")] })],
});

function chat(): AgentChat {
  const [only] = groupChats([late, early, done, other]);
  if (!only) throw new Error("no chat");
  return only;
}

describe("openQuestions", () => {
  it("queues the asks that wait for you across the chat's threads, oldest first", () => {
    const queue = openQuestions(chat(), "tanim");
    expect(queue.map((q) => q.message.id)).toEqual(["m-1", "m-3", "m-5"]);
    expect(queue.map((q) => q.thread.id)).toEqual(["m-1", "m-1", "m-5"]);
  });

  it("leaves out answered asks and asks to another human", () => {
    const ids = openQuestions(chat(), "tanim").map((q) => q.message.id);
    expect(ids).not.toContain("m-6");
    expect(ids).not.toContain("m-8");
    expect(openQuestions(chat(), "rafeed").map((q) => q.message.id)).toEqual([
      "m-8",
    ]);
  });
});

describe("queueIndex", () => {
  const queue = openQuestions(chat(), "tanim");

  it("follows the question the person stepped to", () => {
    expect(queueIndex(queue, { id: "m-3", index: 0 })).toBe(1);
    expect(queueIndex(queue, cursorAt(queue, 2))).toBe(2);
  });

  it("moves to the next question once the current one leaves the queue", () => {
    const answered = queue.filter((q) => q.message.id !== "m-3");
    expect(queueIndex(answered, { id: "m-3", index: 1 })).toBe(1);
    expect(answered[1]?.message.id).toBe("m-5");
    const last = queue.slice(0, 2);
    expect(queueIndex(last, { id: "m-5", index: 2 })).toBe(1);
    expect(queueIndex(queue, { id: null, index: -4 })).toBe(0);
  });

  it("has no question for an empty queue", () => {
    expect(queueIndex([], { id: "m-1", index: 0 })).toBe(-1);
    expect(cursorAt([], 0)).toEqual({ id: null, index: 0 });
  });
});

describe("startCursor", () => {
  const queue = openQuestions(chat(), "tanim");

  it("starts at the linked question, else at the oldest question of the linked thread", () => {
    expect(startCursor(queue, "m-5")).toEqual({ id: "m-5", index: 2 });
    expect(startCursor(queue, "m-3")).toEqual({ id: "m-3", index: 1 });
    expect(startCursor(queue, "m-2")).toEqual({ id: "m-1", index: 0 });
  });

  it("starts at the oldest question for any other link", () => {
    expect(startCursor(queue, "m-7")).toEqual({ id: "m-1", index: 0 });
    expect(startCursor(queue, null)).toEqual({ id: "m-1", index: 0 });
    expect(startCursor([], null)).toEqual({ id: null, index: 0 });
  });
});

describe("stepText", () => {
  it("counts the steps only while more than one question waits", () => {
    expect(stepText(0, 1)).toBe("Question");
    expect(stepText(1, 3)).toBe("Question 2 of 3");
  });
});

describe("markableThread", () => {
  it("closes the current question's thread, else the newest thread with a closable ask", () => {
    const closable = { ...late, closable: true };
    const [mixed] = groupChats([closable, { ...early, closable: true }]);
    if (!mixed) throw new Error("no chat");
    const queue = openQuestions(mixed, "tanim");
    expect(markableThread(mixed, queue[0] ?? null)?.id).toBe("m-1");
    expect(markableThread(mixed, null)?.id).toBe("m-5");
    expect(markableThread(chat(), null)).toBeNull();
  });
});

describe("unmarkedAsk", () => {
  const plain = (id: string, sender: string, at: number) =>
    message(id, {
      sender,
      recipients: sender === "human:tanim" ? [] : [recipient("human:tanim")],
      sent_at: at,
    });

  it("takes the newest agent message to you after the last human message of a waiting thread", () => {
    const waiting = thread("m-30", {
      awaiting_human: true,
      messages: [
        plain("m-30", "frontend", 1),
        plain("m-31", "human:tanim", 2),
        plain("m-32", "frontend", 3),
        plain("m-33", "frontend", 4),
      ],
    });
    expect(unmarkedAsk(waiting, "tanim")?.id).toBe("m-33");
    const [lone] = groupChats([waiting]);
    if (!lone) throw new Error("no chat");
    expect(openQuestions(lone, "tanim").map((q) => q.message.id)).toEqual([
      "m-33",
    ]);
  });

  it("is none for a thread that does not wait or whose ask the client sees", () => {
    expect(unmarkedAsk(early, "tanim")).toBeNull();
    expect(
      unmarkedAsk(
        thread("m-34", { messages: [plain("m-34", "frontend", 1)] }),
        "tanim",
      ),
    ).toBeNull();
    expect(
      unmarkedAsk(
        thread("m-35", {
          awaiting_human: true,
          messages: [plain("m-35", "human:tanim", 1)],
        }),
        "tanim",
      ),
    ).toBeNull();
  });
});
