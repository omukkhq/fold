import { describe, expect, it } from "vitest";
import type { ThreadView } from "@/api/messages";
import { message, thread } from "@/test/messages";
import {
  filterLabel,
  filterOptions,
  filterThreads,
  findThread,
  isHuman,
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
    expect(isHuman("human:tanim")).toBe(true);
    expect(isHuman("frontend/ui")).toBe(false);
  });
  it("replies to the agents of a thread", () => {
    expect(replyTargets(threads[0] as ThreadView)).toEqual(["fe/ui"]);
  });
});

describe("filterLabel", () => {
  it("names the filter and its value once set", () => {
    expect(filterLabel("Agent", undefined)).toBe("Agent");
    expect(filterLabel("Agent", "frontend")).toBe("Agent: frontend");
  });
});
