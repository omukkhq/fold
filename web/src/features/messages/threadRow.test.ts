import { describe, expect, it } from "vitest";
import { segmentsText } from "@/people/segments";
import { message, recipient, thread } from "@/test/messages";
import { groupChats } from "./chats";
import { chatRow, questionsLabel, rowPreview, unreadLabel } from "./threadRow";

const ask = message("m-2", {
  sender: "web-worker-39",
  recipients: [recipient("human:tanim")],
  asks_human: true,
  body: "Use null?",
});

describe("rowPreview", () => {
  it("leaves out the sender the row already names", () => {
    expect(segmentsText(rowPreview(ask, "rafeed", "web-worker-39"))).toBe(
      "Use null?",
    );
  });
  it("calls your own message yours and names any other sender", () => {
    const own = message("m-6", { sender: "human:tanim", body: "Yes." });
    expect(segmentsText(rowPreview(own, "tanim", "main"))).toBe("You: Yes.");
    expect(segmentsText(rowPreview(ask, "tanim", "main"))).toBe(
      "@web-worker-39: Use null?",
    );
  });
});

describe("chatRow", () => {
  const waiting = thread("m-2", {
    awaiting_human: true,
    last_at: 20,
    messages: [{ ...ask, sent_at: 20 }],
  });
  const note = thread("m-1", {
    last_at: 10,
    messages: [
      message("m-1", {
        sender: "web-worker-39",
        recipients: [recipient("human:tanim")],
        body: "Started T-4.",
        sent_at: 10,
      }),
    ],
  });

  it("names the agent, shows its newest message and counts what waits for you", () => {
    const [chat] = groupChats([note, waiting]);
    if (!chat) throw new Error("no chat");
    const row = chatRow(chat, "tanim", new Set(["m-2"]));
    expect(row.title).toEqual({ agent: "web-worker-39" });
    expect(segmentsText(row.preview)).toBe("Use null?");
    expect(row.questions).toBe(1);
    // The unread ask counts once, as a question set.
    expect(row.unread).toBe(1);
    // An ask that Needs you no longer lists is no set, only an unread message.
    const closed = chatRow(chat, "tanim", new Set());
    expect(closed.questions).toBe(0);
    expect(closed.unread).toBe(2);
  });

  it("names a chat without an agent by its thread and counts nothing for no one signed in", () => {
    const own = thread("m-5", {
      participants: ["human:tanim"],
      messages: [message("m-5", { sender: "human:tanim", body: "Todo" })],
    });
    const [chat] = groupChats([own]);
    if (!chat) throw new Error("no chat");
    const row = chatRow(chat, null, new Set());
    expect(row.title).toEqual({ text: "Todo" });
    expect(row.unread).toBe(0);
    expect(chatRow({ ...chat, threads: [] }, null, new Set())).toEqual({
      title: { text: "m-5" },
      preview: [],
      questions: 0,
      unread: 0,
    });
  });

  it("words its counts", () => {
    expect(questionsLabel(1)).toBe("1 question set waits for you");
    expect(questionsLabel(3)).toBe("3 question sets wait for you");
    expect(unreadLabel(1)).toBe("1 unread message");
    expect(unreadLabel(2)).toBe("2 unread messages");
  });
});
