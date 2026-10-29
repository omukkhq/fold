import { describe, expect, it } from "vitest";
import { message, recipient, thread } from "@/test/messages";
import {
  chatByKey,
  chatContexts,
  chatHolding,
  chatMessages,
  chatName,
  chatParticipants,
  groupChats,
  headerContexts,
  messageContext,
  newestMessage,
  threadAgent,
  threadContext,
  threadIn,
  unreadMessages,
} from "./chats";

const chart = thread("m-1", {
  task: "T-12",
  participants: ["frontend", "human:tanim"],
  last_at: 30,
  messages: [
    message("m-1", { sender: "frontend", body: "Which chart?", sent_at: 10 }),
    message("m-4", { sender: "human:tanim", body: "SVG.", sent_at: 30 }),
  ],
});
const port = thread("m-2", {
  proposal: "P-3",
  participants: ["frontend", "human:tanim"],
  last_at: 20,
  messages: [
    message("m-2", { sender: "frontend", body: "Which port?", sent_at: 20 }),
  ],
});
const feedback = thread("m-3", {
  task: "T-7",
  participants: ["human:tanim", "backend"],
  last_at: 25,
  messages: [
    message("m-3", {
      sender: "human:tanim",
      recipients: [recipient("backend")],
      body: "Look at T-7.",
      sent_at: 25,
    }),
  ],
});

describe("threadAgent", () => {
  it("is the first agent that wrote, or the agent the person wrote to", () => {
    expect(threadAgent(chart)).toBe("frontend");
    expect(threadAgent(feedback)).toBe("backend");
  });

  it("keeps a thread with several agents with the agent that started it", () => {
    const shared = thread("m-5", {
      participants: ["main", "backend", "human:tanim"],
      messages: [
        message("m-5", { sender: "main", recipients: [recipient("backend")] }),
        message("m-6", { sender: "backend" }),
      ],
    });
    expect(threadAgent(shared)).toBe("main");
  });

  it("falls back to an agent among the participants, else to none", () => {
    const toHuman = thread("m-7", {
      participants: ["human:tanim", "human:rafeed", "frontend"],
      messages: [
        message("m-7", {
          sender: "human:tanim",
          recipients: [recipient("human:rafeed")],
        }),
      ],
    });
    expect(threadAgent(toHuman)).toBe("frontend");
    expect(threadAgent(thread("m-8", { participants: ["human:tanim"] }))).toBe(
      null,
    );
  });

  it("never gives a thread to fold, which only writes its own notes", () => {
    const note = thread("m-9", {
      participants: ["fold", "web-lead", "human:tanim"],
      messages: [
        message("m-9", {
          sender: "fold",
          recipients: [recipient("web-lead")],
          body: "@web-worker cannot start now.",
        }),
        message("m-10", {
          sender: "web-lead",
          recipients: [recipient("human:tanim", { addressed_as: "humans" })],
          body: "Start it again?",
          asks_human: true,
        }),
      ],
    });
    expect(threadAgent(note)).toBe("web-lead");
    const forwarded = thread("m-11", {
      participants: ["human:tanim", "fold", "frontend"],
      messages: [
        message("m-11", {
          sender: "human:tanim",
          recipients: [recipient("fold"), recipient("frontend")],
        }),
      ],
    });
    expect(threadAgent(forwarded)).toBe("frontend");
    expect(
      threadAgent(thread("m-12", { participants: ["human:tanim", "fold"] })),
    ).toBe(null);
  });
});

describe("groupChats", () => {
  it("joins the threads of one agent into one chat, newest chat and thread first", () => {
    const chats = groupChats([port, chart, feedback, chart]);
    expect(chats.map((c) => c.key)).toEqual(["frontend", "backend"]);
    expect(chats[0]?.threads.map((t) => t.id)).toEqual(["m-1", "m-2"]);
    expect(chats[0]?.last_at).toBe(30);
  });

  it("finds a chat by its key and by a thread or message it holds", () => {
    const chats = groupChats([chart, port, feedback]);
    expect(chatByKey(chats, "backend")?.key).toBe("backend");
    expect(chatByKey(chats, "nobody")).toBeNull();
    expect(chatByKey(chats, undefined)).toBeNull();
    expect(chatHolding(chats, "m-4")?.key).toBe("frontend");
    expect(chatHolding(chats, "m-99")).toBeNull();
    const [frontend] = chats;
    if (!frontend) throw new Error("no chat");
    expect(threadIn(frontend, "m-2")?.id).toBe("m-2");
    expect(threadIn(frontend, "m-4")?.id).toBe("m-1");
    expect(threadIn(frontend, undefined)).toBeNull();
  });
});

describe("chatMessages", () => {
  it("puts the messages of every thread in time order, then in number order", () => {
    const [chat] = groupChats([
      chart,
      port,
      thread("m-9", {
        participants: ["frontend"],
        last_at: 20,
        messages: [message("m-9", { sender: "frontend", sent_at: 20 })],
      }),
    ]);
    if (!chat) throw new Error("no chat");
    expect(chatMessages(chat).map((m) => m.id)).toEqual([
      "m-1",
      "m-2",
      "m-9",
      "m-4",
    ]);
    expect(newestMessage(chat)?.id).toBe("m-4");
    expect(newestMessage({ ...chat, threads: [] })).toBeNull();
  });
});

describe("contexts", () => {
  it("names a message by its request, then its proposal, then its task, filled in from the thread", () => {
    const m = message("m-1");
    expect(
      messageContext({ ...m, request: "RQ-2", task: "T-1" }, thread("m-1")),
    ).toEqual({ kind: "request", id: "RQ-2" });
    expect(messageContext(m, thread("m-1", { proposal: "P-3" }))).toEqual({
      kind: "proposal",
      id: "P-3",
    });
    expect(messageContext(m, thread("m-1", { task: "T-1" }))).toEqual({
      kind: "task",
      id: "T-1",
    });
    expect(messageContext(m, thread("m-1"))).toBeNull();
  });

  it("names a thread the same way", () => {
    expect(threadContext(thread("m-1", { request: "RQ-1" }))?.id).toBe("RQ-1");
    expect(threadContext(port)?.id).toBe("P-3");
    expect(threadContext(chart)?.id).toBe("T-12");
    expect(threadContext(thread("m-1"))).toBeNull();
  });

  it("lists each id of the chat's threads once", () => {
    const [chat] = groupChats([
      chart,
      port,
      thread("m-10", {
        request: "RQ-4",
        task: "T-12",
        participants: ["frontend"],
        messages: [message("m-10", { sender: "frontend" })],
      }),
    ]);
    if (!chat) throw new Error("no chat");
    expect(chatContexts(chat).map((c) => c.id)).toEqual([
      "T-12",
      "P-3",
      "RQ-4",
    ]);
  });

  it("names the newest few in the header and counts the rest", () => {
    const tasks = [1, 2, 3, 4, 5].map((n) =>
      thread(`m-${String(20 + n)}`, {
        task: `T-${String(n)}`,
        last_at: n,
        participants: ["frontend"],
      }),
    );
    const [chat] = groupChats(tasks);
    if (!chat) throw new Error("no chat");
    const header = headerContexts(chat);
    expect(header.shown.map((c) => c.id)).toEqual(["T-5", "T-4", "T-3"]);
    expect(header.more).toBe(2);
    expect(headerContexts(chat, 5)).toEqual({
      shown: chatContexts(chat),
      more: 0,
    });
  });
});

describe("chatName and chatParticipants", () => {
  it("names a chat by its agent, else by its one thread", () => {
    const chats = groupChats([
      chart,
      thread("m-8", {
        participants: ["human:tanim"],
        messages: [message("m-8", { sender: "human:tanim", body: "Notes" })],
      }),
    ]);
    expect(chats.map(chatName)).toEqual(["@frontend", "Notes"]);
    const lone = chats[1];
    if (!lone) throw new Error("no chat");
    expect(chatName({ ...lone, threads: [] })).toBe("m-8");
  });

  it("lists every person once, oldest thread first", () => {
    const shared = thread("m-11", {
      participants: ["frontend", "main"],
      last_at: 40,
      messages: [message("m-11", { sender: "frontend", sent_at: 40 })],
    });
    const [chat] = groupChats([chart, shared]);
    if (!chat) throw new Error("no chat");
    expect(chatParticipants(chat)).toEqual(["frontend", "human:tanim", "main"]);
  });
});

describe("unreadMessages", () => {
  it("lists the messages the signed-in human has not read, none for no one", () => {
    const sent = thread("m-12", {
      participants: ["frontend", "human:tanim"],
      messages: [
        message("m-12", {
          sender: "frontend",
          recipients: [recipient("human:tanim")],
        }),
        message("m-13", {
          sender: "frontend",
          recipients: [recipient("human:tanim", { acknowledged_at: 5 })],
        }),
      ],
    });
    const [chat] = groupChats([sent]);
    if (!chat) throw new Error("no chat");
    expect(unreadMessages(chat, "tanim").map((m) => m.id)).toEqual(["m-12"]);
    expect(unreadMessages(chat, "rafeed")).toEqual([]);
    expect(unreadMessages(chat, null)).toEqual([]);
  });
});
