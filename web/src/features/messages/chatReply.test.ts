import { describe, expect, it } from "vitest";
import { message, recipient, thread } from "@/test/messages";
import {
  aboutOptions,
  aboutValue,
  chatReplyTarget,
  REPLY_HINT,
  replyHint,
  threadLabel,
} from "./chatReply";
import { groupChats, type AgentChat } from "./chats";
import { chatSets } from "./sets";

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
  const [set] = chatSets(chat, new Set(["m-1"]));
  if (!set) throw new Error("no set");

  it("offers every thread newest first", () => {
    expect(aboutOptions(chat)).toEqual([
      { value: "m-2", label: "P-3 · Ready." },
      { value: "m-1", label: "T-12 · Which chart?" },
      { value: "m-4", label: "Hello" },
    ]);
  });

  it("starts at the thread of the set on the card, else at the newest thread, and keeps a valid choice", () => {
    expect(aboutValue(chat, set, null)).toBe("m-1");
    expect(aboutValue(chat, null, null)).toBe("m-2");
    expect(aboutValue(chat, set, "m-4")).toBe("m-4");
    expect(aboutValue(chat, set, "m-99")).toBe("m-1");
    expect(aboutValue({ ...chat, threads: [] }, null, null)).toBeNull();
  });
});

describe("chatReplyTarget", () => {
  const chat = chatOf(asked, report, loose);
  const open = new Set(["m-1"]);
  const [set] = chatSets(chat, open);
  if (!set) throw new Error("no set");

  it("writes in the thread of the set on the card, as that thread's reply box would", () => {
    const target = chatReplyTarget(chat, "tanim", set, null, open);
    expect(target?.replyTo).toBe("m-1");
    expect(target?.thread.id).toBe("m-1");
  });

  it("replies to the newest message of the newest thread without a set", () => {
    const target = chatReplyTarget(chat, "tanim", null, null, open);
    expect(target).toMatchObject({ replyTo: "m-3", answers: null });
    expect(target?.thread.id).toBe("m-2");
  });

  it("replies in the thread the person chose", () => {
    const target = chatReplyTarget(chat, "tanim", set, "m-4", open);
    expect(target).toMatchObject({ replyTo: "m-4", answers: null });
  });

  it("never claims that a reply answers an open set, and says so under the box", () => {
    const free = thread("m-30", {
      awaiting_human: true,
      messages: [
        message("m-30", {
          sender: "frontend",
          recipients: [recipient("human:tanim")],
          body: "Is Pencil open?",
          asks_human: true,
        }),
      ],
    });
    const freeChat = chatOf(free);
    const waiting = new Set(["m-30"]);
    const [card] = chatSets(freeChat, waiting);
    expect(
      chatReplyTarget(freeChat, "tanim", card ?? null, null, waiting),
    ).toMatchObject({ replyTo: "m-30", answers: null });
    expect(replyHint(free, waiting)).toBe(REPLY_HINT);
    expect(
      chatReplyTarget(freeChat, "tanim", null, null, new Set())?.answers?.id,
    ).toBe("m-30");
    expect(replyHint(free, new Set())).toBeNull();
    expect(replyHint(null, waiting)).toBeNull();
  });

  it("has no target for a chat without threads or messages", () => {
    expect(
      chatReplyTarget({ ...chat, threads: [] }, null, null, null, open),
    ).toBeNull();
    expect(
      chatReplyTarget(chatOf(thread("m-20")), null, null, null, open),
    ).toBeNull();
  });
});
