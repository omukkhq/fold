import { isHumanName } from "@/people/names";
import { describe, expect, it } from "vitest";
import type { ThreadView } from "@/api/messages";
import { message, recipient, thread } from "@/test/messages";
import {
  choicesWith,
  filterOptions,
  filterSummary,
  filterThreads,
  findThread,
  hasAsk,
  searchThreads,
  showFilter,
  showOptions,
  startedBy,
  chatCount,
  withShow,
  replyPlaceholder,
  replyTargets,
} from "./filters";

const threads = [
  thread("m-1", {
    task: "T-12",
    last_at: 1,
    participants: ["human:a", "fe/ui"],
  }),
  thread("m-2", {
    task: "T-21",
    proposal: "P-4",
    last_at: 3,
    awaiting_human: true,
  }),
  thread("m-3", { task: "T-12", proposal: "P-4", last_at: 2 }),
];

describe("filterThreads", () => {
  it("returns every thread, newest first, without a filter", () => {
    expect(filterThreads(threads, {}).map((t) => t.id)).toEqual([
      "m-2",
      "m-3",
      "m-1",
    ]);
  });
  it("filters by agent, task and proposal", () => {
    expect(filterThreads(threads, { agent: "fe/ui" }).map((t) => t.id)).toEqual(
      ["m-1"],
    );
    expect(filterThreads(threads, { task: "T-12" }).map((t) => t.id)).toEqual([
      "m-3",
      "m-1",
    ]);
    expect(
      filterThreads(threads, { proposal: "P-4" }).map((t) => t.id),
    ).toEqual(["m-2", "m-3"]);
  });
  it("requires every set field to match", () => {
    expect(
      filterThreads(threads, { task: "T-12", proposal: "P-4" }).map(
        (t) => t.id,
      ),
    ).toEqual(["m-3"]);
    expect(filterThreads(threads, { agent: "nobody" })).toEqual([]);
  });
  it("keeps for an agent only the threads of its own chat, not the threads it joined", () => {
    const joined = thread("m-9", {
      last_at: 4,
      participants: ["main", "backend", "human:tanim"],
      messages: [
        message("m-9", { sender: "main", recipients: [recipient("backend")] }),
        message("m-10", { sender: "backend" }),
      ],
    });
    const own = thread("m-11", {
      last_at: 5,
      participants: ["backend", "human:tanim"],
      messages: [message("m-11", { sender: "backend" })],
    });
    const list = [joined, own];
    expect(filterThreads(list, { agent: "backend" }).map((t) => t.id)).toEqual([
      "m-11",
    ]);
    expect(filterThreads(list, { agent: "main" }).map((t) => t.id)).toEqual([
      "m-9",
    ]);
    expect(filterOptions(list).agents).toEqual(["backend", "main"]);
  });
  it("does not change the input order", () => {
    const copy = [...threads];
    filterThreads(copy, {});
    expect(copy).toEqual(threads);
  });
});

describe("filterOptions", () => {
  it("lists agents without humans, and sorted tasks and proposals", () => {
    expect(filterOptions(threads)).toEqual({
      agents: ["fe/ui", "frontend"],
      tasks: ["T-12", "T-21"],
      proposals: ["P-4"],
    });
  });
  it("sorts numbers as numbers", () => {
    const many = [thread("a", { task: "T-10" }), thread("b", { task: "T-9" })];
    expect(filterOptions(many).tasks).toEqual(["T-9", "T-10"]);
  });
});

describe("findThread", () => {
  const withMessages = [
    thread("m-1", {
      messages: [
        message("m-1"),
        message("m-9", { thread: "m-1", reply_to: "m-1" }),
      ],
    }),
  ];
  it("finds a thread by its id or by one of its messages", () => {
    expect(findThread(withMessages, "m-1")?.id).toBe("m-1");
    expect(findThread(withMessages, "m-9")?.id).toBe("m-1");
  });
  it("returns null for no id or an unknown id", () => {
    expect(findThread(withMessages, undefined)).toBeNull();
    expect(findThread(withMessages, "m-77")).toBeNull();
  });
});

describe("names", () => {
  it("tells humans from agents", () => {
    expect(isHumanName("human:tanim")).toBe(true);
    expect(isHumanName("frontend/ui")).toBe(false);
  });
  it("replies to the agents of a thread", () => {
    expect(replyTargets(threads[0] as ThreadView)).toEqual(["fe/ui"]);
  });
  it("names the agents of the reply box as @names", () => {
    expect(replyPlaceholder(threads[0] as ThreadView)).toBe("Reply to @fe/ui…");
    expect(replyPlaceholder(thread("m-9", { participants: ["human:a"] }))).toBe(
      "Reply…",
    );
    expect(
      replyPlaceholder(
        thread("m-8", {
          participants: ["human:a", "api-audit", "api-lead", "fold"],
        }),
      ),
    ).toBe("Reply to @api-audit, @api-lead…");
    expect(replyPlaceholder(threads[0] as ThreadView, true)).toBe(
      "Reply to @fe/ui…  Enter sends, Shift+Enter adds a line",
    );
  });
});

describe("the waiting filter", () => {
  it("keeps the threads the daemon marks", () => {
    expect(filterThreads(threads, { waiting: true }).map((t) => t.id)).toEqual([
      "m-2",
    ]);
    expect(filterThreads(threads, { waiting: false })).toHaveLength(3);
  });
});

describe("the questions and mine filters", () => {
  const asked = thread("m-7", {
    last_at: 9,
    messages: [
      message("m-7", { sender: "human:tanim", body: "The build fails." }),
      message("m-8", {
        sender: "frontend",
        recipients: [recipient("human:tanim")],
        asks_human: true,
      }),
    ],
  });
  const choice = thread("m-9", {
    last_at: 8,
    messages: [
      message("m-9", {
        sender: "frontend",
        choices: [{ label: "A", description: null }],
      }),
    ],
  });
  const plain = thread("m-10", {
    last_at: 7,
    messages: [message("m-10", { sender: "backend" })],
  });
  const all = [plain, choice, asked];

  it("keeps the threads with an ask for the questions filter", () => {
    expect([asked, choice, plain, thread("m-11")].map(hasAsk)).toEqual([
      true,
      true,
      false,
      false,
    ]);
    expect(filterThreads(all, { questions: true }).map((t) => t.id)).toEqual([
      "m-7",
      "m-9",
    ]);
  });

  it("keeps the threads the signed-in human started for the mine filter", () => {
    expect(startedBy(asked, "tanim")).toBe(true);
    expect(startedBy(asked, null)).toBe(false);
    expect(startedBy(thread("m-12"), "tanim")).toBe(false);
    expect(
      filterThreads(all, { mine: true }, "tanim").map((t) => t.id),
    ).toEqual(["m-7"]);
    expect(filterThreads(all, { mine: true })).toEqual([]);
  });
});

describe("searchThreads", () => {
  const found = thread("m-20", {
    task: "T-125",
    proposal: "P-61",
    participants: ["web-worker-39", "human:rafeed"],
    messages: [
      message("m-20", { body: "Role analytics\nTwo charts lack a spec." }),
      message("m-21", { body: "Add decline reasons." }),
    ],
  });
  const other = thread("m-30", {
    participants: ["backend"],
    messages: [message("m-30", { body: "The migration needs a default." })],
  });

  it("keeps every thread for an empty query", () => {
    expect(searchThreads([found, other], "  ")).toHaveLength(2);
  });

  it("matches every word without case in the title, bodies, people, task, proposal and id", () => {
    const ids = (query: string) =>
      searchThreads([found, other], query).map((t) => t.id);
    expect(ids("role ANALYTICS")).toEqual(["m-20"]);
    expect(ids("decline")).toEqual(["m-20"]);
    expect(ids("@rafeed t-125")).toEqual(["m-20"]);
    expect(ids("p-61")).toEqual(["m-20"]);
    expect(ids("m-30")).toEqual(["m-30"]);
    expect(ids("migration charts")).toEqual([]);
  });
});

describe("the Show choice", () => {
  it("reads which choice is on, all when none is", () => {
    expect(showFilter({})).toBe("all");
    expect(showFilter({ waiting: true, task: "T-1" })).toBe("waiting");
    expect(showFilter({ questions: true })).toBe("questions");
    expect(showFilter({ mine: true })).toBe("mine");
  });
  it("turns one choice on and the others off, keeping the other filters", () => {
    expect(withShow({ waiting: true, task: "T-1" }, "mine")).toEqual({
      task: "T-1",
      waiting: undefined,
      questions: undefined,
      mine: true,
    });
    expect(showFilter(withShow({ mine: true }, "all"))).toBe("all");
    expect(showFilter(withShow({}, "questions"))).toBe("questions");
    expect(showFilter(withShow({}, "waiting"))).toBe("waiting");
  });
  it("shows the count of waiting sets and offers Mine only to a signed-in human", () => {
    expect(showOptions("tanim", 2).map((c) => [c.label, c.count])).toEqual([
      ["All chats", null],
      ["Waits for you", 2],
      ["Questions", null],
      ["Mine", null],
    ]);
    expect(showOptions(null, 0).map((c) => [c.label, c.count])).toEqual([
      ["All chats", null],
      ["Waits for you", 0],
      ["Questions", null],
    ]);
  });
  it("counts a question set as an ask", () => {
    const set = thread("m-30", {
      messages: [
        message("m-30", {
          sender: "frontend",
          recipients: [recipient("human:tanim")],
          questions: [{ text: "Connected?", header: null, choices: [] }],
        }),
      ],
    });
    expect(hasAsk(set)).toBe(true);
  });
  it("names the Show choice on the button and counts the other set filters", () => {
    expect(filterSummary({})).toBe("All chats");
    expect(filterSummary({ waiting: true })).toBe("Waits for you");
    expect(filterSummary({ mine: true, agent: "fe", task: "T-1" })).toBe(
      "Mine · 2 more",
    );
    expect(filterSummary({ proposal: "P-1" })).toBe("All chats · 1 more");
  });
  it("counts the chats it shows", () => {
    expect(chatCount(1)).toBe("1 chat");
    expect(chatCount(5)).toBe("5 chats");
  });
});

describe("choicesWith", () => {
  it("adds the value the address sets when no thread holds it", () => {
    expect(choicesWith(["T-1"], "T-9")).toEqual(["T-1", "T-9"]);
    expect(choicesWith(["T-1"], "T-1")).toEqual(["T-1"]);
    expect(choicesWith(["T-1"], undefined)).toEqual(["T-1"]);
  });
});
