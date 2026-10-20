import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import { countUnreadNotes, listNotes, markNotesRead } from "./notes";

afterEach(() => {
  vi.unstubAllGlobals();
});

const note = (id: string) => ({
  id,
  author: "web-lead",
  agent: "web-lead",
  area: "web",
  title: "Cards sort by due date",
  decision: "d",
  reason: "r",
  links: [],
  written_at: 0,
});

describe("listNotes", () => {
  it("follows the cursor of each page to the last", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/notes": (request) =>
        request.search.get("before") === "n-40"
          ? { body: { items: [note("n-30")], next: null, unread: 0 } }
          : { body: { items: [note("n-50")], next: "n-40", unread: 0 } },
    });
    const { items, more } = await listNotes("demo");
    expect(items.map((n) => n.id)).toEqual(["n-50", "n-30"]);
    expect(more).toBe(false);
    expect(calls[0]?.search.get("limit")).toBe("200");
  });
});

describe("countUnreadNotes", () => {
  it("reads the human's count from a page of one note", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/notes": {
        body: { items: [note("n-9")], next: "n-9", unread: 2 },
      },
    });
    await expect(countUnreadNotes("demo")).resolves.toBe(2);
    expect(calls[0]?.search.get("limit")).toBe("1");
  });

  it("answers null when fold does not know the human", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/notes": {
        body: { items: [], next: null, unread: null },
      },
    });
    await expect(countUnreadNotes("demo")).resolves.toBeNull();
  });
});

describe("markNotesRead", () => {
  it("posts the newest note seen and answers with the unread count", async () => {
    const calls = stubFetch({
      "POST /api/v1/projects/demo/notes/read": { body: { unread: 0 } },
    });
    await expect(markNotesRead("demo", "n-12")).resolves.toBe(0);
    expect(calls[0]?.body).toEqual({ through: "n-12" });
  });

  it("rejects with the daemon's code when the request names no human", async () => {
    stubFetch({
      "POST /api/v1/projects/demo/notes/read": errorReply(
        401,
        "human_name_required",
      ),
    });
    await expect(markNotesRead("demo", "n-1")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "human_name_required"),
    );
  });
});
