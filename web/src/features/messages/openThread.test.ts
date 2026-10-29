import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { message, recipient, thread } from "@/test/messages";
import { groupChats } from "./chats";
import {
  chatAnchor,
  isMissingThread,
  missingChatTitle,
  openChat,
  visibleChats,
} from "./openThread";

const a = thread("m-1", {
  participants: ["frontend"],
  last_at: 30,
  messages: [message("m-1", { sender: "frontend", sent_at: 30 })],
});
const b = thread("m-2", {
  participants: ["frontend"],
  last_at: 20,
  messages: [
    message("m-2", { sender: "frontend", sent_at: 10 }),
    message("m-5", { sender: "frontend", sent_at: 20 }),
  ],
});
const c = thread("m-3", {
  participants: ["backend"],
  last_at: 10,
  messages: [message("m-3", { sender: "backend", sent_at: 5 })],
});
const all = [a, b, c];
const chats = groupChats(all);

describe("visibleChats", () => {
  it("keeps every chat with a kept thread, with all of its threads", () => {
    const kept = visibleChats(chats, [b]);
    expect(kept.map((chat) => chat.key)).toEqual(["frontend"]);
    expect(kept[0]?.threads).toHaveLength(2);
  });
});

describe("openChat", () => {
  it("opens the newest visible chat when the address names none", () => {
    expect(openChat(all, chats, chats, {})).toEqual({
      kind: "listed",
      chat: chats[0],
    });
    expect(openChat([], [], [], {})).toEqual({ kind: "listed", chat: null });
  });

  it("opens a chat by its key, or the chat that holds a linked thread or message", () => {
    const open = (requested: { chat?: string; thread?: string }) => {
      const target = openChat(all, chats, chats, requested);
      return target.kind === "listed" ? target.chat?.key : target.kind;
    };
    expect(open({ chat: "backend" })).toBe("backend");
    expect(open({ thread: "m-3" })).toBe("backend");
    expect(open({ thread: "m-5" })).toBe("frontend");
    expect(open({ chat: "backend", thread: "m-5" })).toBe("frontend");
  });

  it("shows a chat key that no listed chat has as missing, never another chat", () => {
    expect(openChat(all, chats, chats, { chat: "nobody" })).toEqual({
      kind: "missing",
      key: "nobody",
    });
    expect(openChat([], [], [], { chat: "nobody" }).kind).toBe("missing");
    expect(missingChatTitle("nobody")).toBe("No chat with @nobody");
  });

  it("falls back to the first visible chat when the filters hide the requested one", () => {
    const shown = visibleChats(chats, [a]);
    const target = openChat(all, chats, shown, { thread: "m-3" });
    expect(target.kind === "listed" ? target.chat?.key : null).toBe("frontend");
  });

  it("reads a requested thread the listing does not hold on its own", () => {
    expect(openChat(all, chats, chats, { thread: "m-900" })).toEqual({
      kind: "unlisted",
      id: "m-900",
    });
  });
});

describe("chatAnchor", () => {
  const ask = thread("m-10", {
    awaiting_human: true,
    participants: ["frontend", "human:tanim"],
    last_at: 40,
    messages: [
      message("m-10", { sender: "frontend", sent_at: 35 }),
      message("m-11", {
        sender: "frontend",
        recipients: [recipient("human:tanim")],
        choices: [{ label: "Yes", description: null }],
        sent_at: 38,
      }),
      message("m-12", { sender: "frontend", sent_at: 40 }),
    ],
  });
  const [chat] = groupChats([a, b, ask]);
  if (!chat) throw new Error("no chat");

  it("opens at the newest message without a link or with a link outside the chat", () => {
    expect(chatAnchor(chat, undefined, "tanim")).toBe("m-12");
    expect(chatAnchor(chat, "m-3", "tanim")).toBe("m-12");
    expect(chatAnchor({ ...chat, threads: [] }, undefined, null)).toBeNull();
  });

  it("opens at a linked message", () => {
    expect(chatAnchor(chat, "m-5", "tanim")).toBe("m-5");
  });

  it("opens a linked thread at its question to you, else at its newest message", () => {
    expect(chatAnchor(chat, "m-10", "tanim")).toBe("m-11");
    expect(chatAnchor(chat, "m-2", "tanim")).toBe("m-5");
    const empty = thread("m-20", { participants: ["frontend"] });
    const [lone] = groupChats([empty]);
    if (!lone) throw new Error("no chat");
    expect(chatAnchor(lone, "m-20", null)).toBeNull();
  });
});

describe("isMissingThread", () => {
  it("reads an unknown and a malformed message id as missing, and nothing else", () => {
    expect(isMissingThread(new ApiError("message_not_found", ""))).toBe(true);
    expect(isMissingThread(new ApiError("invalid_id", ""))).toBe(true);
    expect(isMissingThread(new ApiError("project_unavailable", ""))).toBe(
      false,
    );
    expect(isMissingThread(null)).toBe(false);
  });
});
