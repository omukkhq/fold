import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import {
  countAwaitingHuman,
  listThreads,
  markAnswered,
  sendMessage,
} from "./messages";

afterEach(() => {
  vi.unstubAllGlobals();
});

const thread = (id: string) => ({ id, messages: [], awaiting_human: false });

describe("listThreads", () => {
  it("follows the cursor of each page to the last", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/messages": (request) =>
        request.search.get("before") === "m-40"
          ? { body: { items: [thread("m-30")], next: null } }
          : { body: { items: [thread("m-50")], next: "m-40" } },
    });
    const { items, more } = await listThreads("demo");
    expect(items.map((t) => t.id)).toEqual(["m-50", "m-30"]);
    expect(more).toBe(false);
    expect(calls[0]?.search.get("limit")).toBe("200");
  });
});

describe("sendMessage", () => {
  it("posts a reply and answers with the stored message", async () => {
    const calls = stubFetch({
      "POST /api/v1/projects/demo/feedback": {
        status: 201,
        body: { id: "m-32", thread: "m-31" },
      },
    });
    const sent = await sendMessage("demo", { reply_to: "m-31", body: "ok" });
    expect(sent.thread).toBe("m-31");
    expect(calls[0]?.body).toEqual({ reply_to: "m-31", body: "ok" });
  });

  it("posts feedback linked to a task", async () => {
    const calls = stubFetch({
      "POST /api/v1/projects/demo/feedback": {
        status: 201,
        body: { id: "m-33", thread: "m-33" },
      },
    });
    await sendMessage("demo", { task: "T-12", body: "check" });
    expect(calls[0]?.body).toEqual({ task: "T-12", body: "check" });
  });

  it("rejects with the daemon's code for an unknown message", async () => {
    stubFetch({
      "POST /api/v1/projects/demo/feedback": errorReply(
        404,
        "message_not_found",
      ),
    });
    await expect(
      sendMessage("demo", { reply_to: "m-9", body: "x" }),
    ).rejects.toSatisfy((e: unknown) => isApiError(e, "message_not_found"));
  });
});

describe("countAwaitingHuman", () => {
  it("reads the project-wide count from a page of one thread", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/messages": {
        body: { items: [thread("m-9")], next: "m-9", total_awaiting_human: 3 },
      },
    });
    await expect(countAwaitingHuman("demo")).resolves.toBe(3);
    expect(calls[0]?.search.get("limit")).toBe("1");
  });
});

describe("markAnswered", () => {
  it("posts the thread or the task and answers with the closed asks", async () => {
    const calls = stubFetch({
      "POST /api/v1/projects/demo/asks/answered": {
        body: { answered: ["m-3", "m-4"] },
      },
    });
    await expect(markAnswered("demo", { thread: "m-4" })).resolves.toEqual([
      "m-3",
      "m-4",
    ]);
    await markAnswered("demo", { task: "T-2" });
    expect(calls.map((call) => call.body)).toEqual([
      { thread: "m-4" },
      { task: "T-2" },
    ]);
  });

  it("rejects with the daemon's code for an unknown task", async () => {
    stubFetch({
      "POST /api/v1/projects/demo/asks/answered": errorReply(
        404,
        "task_not_found",
      ),
    });
    await expect(markAnswered("demo", { task: "T-9" })).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "task_not_found"),
    );
  });
});
