import { describe, expect, it } from "vitest";
import { message, recipient, thread } from "@/test/messages";
import {
  ABOUT_QUESTION,
  aboutOptions,
  aboutValue,
  chatReplyTarget,
  threadLabel,
} from "./chatReply";
import { groupChats, type AgentChat } from "./chats";
import { openQuestions } from "./questionQueue";

const asked = thread("m-1", {
  task: "T-12",
  awaiting_human: true,
  last_at: 10,
  messages: [
    message("m-1", {
      sender: "frontend",
      recipients: [recipient("human:tanim")],
      body: "Which chart?",
      choices: [{ label: "SVG", description: null }],
      sent_at: 10,
    }),
  ],
});
const report = thread("m-2", {
  proposal: "P-3",
  last_at: 20,
  messages: [
    message("m-2", { sender: "frontend", body: "Ready.", sent_at: 20 }),
    message("m-3", { sender: "human:tanim", body: "Thanks.", sent_at: 20 }),
  ],
});
const loose = thread("m-4", {
  last_at: 5,
  messages: [message("m-4", { sender: "frontend", body: "Hello", sent_at: 5 })],
});

function chatOf(...threads: Parameters<typeof groupChats>[0]): AgentChat {
  const [chat] = groupChats(threads);
  if (!chat) throw new Error("no chat");
  return chat;
}

describe("threadLabel", () => {
  it("names a thread by its id and title, or by its title alone", () => {
    expect(threadLabel(asked)).toBe("T-12 · Which chart?");
    expect(threadLabel(loose)).toBe("Hello");
  });
});

describe("aboutOptions and aboutValue", () => {
  const chat = chatOf(asked, report, loose);
  const [question] = openQuestions(chat, "tanim");
  if (!question) throw new Error("no question");

  it("offers the open question first, then every thread newest first", () => {
    expect(aboutOptions(chat, question, "Question")).toEqual([
      { value: ABOUT_QUESTION, label: "Answer to question" },
      { value: "m-2", label: "P-3 · Ready." },
      { value: "m-1", label: "T-12 · Which chart?" },
      { value: "m-4", label: "Hello" },
    ]);
    expect(aboutOptions(chat, null, "Question").map((o) => o.value)).toEqual([
      "m-2",
      "m-1",
      "m-4",
    ]);
  });

  it("starts at the open question, else at the newest thread, and keeps a valid choice", () => {
    expect(aboutValue(chat, question, null)).toBe(ABOUT_QUESTION);
    expect(aboutValue(chat, null, null)).toBe("m-2");
    expect(aboutValue(chat, question, "m-4")).toBe("m-4");
    expect(aboutValue(chat, null, ABOUT_QUESTION)).toBe("m-2");
    expect(aboutValue(chat, question, "m-99")).toBe(ABOUT_QUESTION);
    expect(aboutValue({ ...chat, threads: [] }, null, null)).toBeNull();
  });
});

describe("chatReplyTarget", () => {
  const chat = chatOf(asked, report, loose);
  const [question] = openQuestions(chat, "tanim");
  if (!question) throw new Error("no question");

  it("answers the open question in its own thread", () => {
    const target = chatReplyTarget(chat, "tanim", question, null, null);
    expect(target?.replyTo).toBe("m-1");
    expect(target?.answers?.id).toBe("m-1");
    expect(target?.thread.id).toBe("m-1");
  });

  it("replies to the newest message of the newest thread without a question", () => {
    const target = chatReplyTarget(chat, "tanim", null, null, null);
    expect(target).toMatchObject({ replyTo: "m-3", answers: null });
    expect(target?.thread.id).toBe("m-2");
  });

  it("replies in the thread the person chose", () => {
    const target = chatReplyTarget(chat, "tanim", question, "m-4", null);
    expect(target).toMatchObject({ replyTo: "m-4", answers: null });
  });

  it("answers the question the person picked to answer in their own words while it waits", () => {
    const target = chatReplyTarget(chat, "tanim", null, "m-4", "m-1");
    expect(target?.replyTo).toBe("m-1");
    expect(target?.answers?.id).toBe("m-1");
    const answered = chatOf(
      {
        ...asked,
        awaiting_human: false,
        messages: [
          ...asked.messages,
          message("m-9", { sender: "human:tanim", body: "SVG", sent_at: 11 }),
        ],
      },
      loose,
    );
    expect(chatReplyTarget(answered, "tanim", null, null, "m-1")?.replyTo).toBe(
      "m-9",
    );
    expect(chatReplyTarget(chat, "tanim", null, null, "m-99")?.replyTo).toBe(
      "m-3",
    );
  });

  it("has no target for a chat without threads or messages", () => {
    expect(
      chatReplyTarget({ ...chat, threads: [] }, null, null, null, null),
    ).toBeNull();
    expect(
      chatReplyTarget(chatOf(thread("m-20")), null, null, null, null),
    ).toBeNull();
  });
});
