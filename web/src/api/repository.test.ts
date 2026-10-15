import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import { getFile, getRepository, getTree, listRevisions } from "./repository";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("listRevisions", () => {
  it("follows the page cursor to r1", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/revisions": (request) =>
        request.search.get("before") === "r2"
          ? { body: { revisions: [{ id: "r1" }], before: null } }
          : { body: { revisions: [{ id: "r3" }, { id: "r2" }], before: "r2" } },
    });
    const { items, more } = await listRevisions("demo");
    expect(items.map((r) => r.id)).toEqual(["r3", "r2", "r1"]);
    expect(more).toBe(false);
    expect(calls[0]?.search.get("limit")).toBe("200");
  });
});

describe("getRepository", () => {
  it("answers main's head and the path of its copy", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/repository": {
        body: { head: "r17", main_path: "/srv/demo/main", recent: [] },
      },
    });
    const state = await getRepository("demo");
    expect(state.head).toBe("r17");
    expect(state.main_path).toBe("/srv/demo/main");
  });
});

describe("getTree", () => {
  it("lists the entries of a directory by its path", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/revisions/r5/tree": {
        body: { path: "docs", entries: [{ name: "a.md", path: "docs/a.md" }] },
      },
    });
    const entries = await getTree("demo", "r5", "docs");
    expect(entries.map((e) => e.path)).toEqual(["docs/a.md"]);
    expect(calls[0]?.search.get("path")).toBe("docs");
  });

  it("rejects with the daemon's code for an unknown revision", async () => {
    stubFetch({
      "GET /api/v1/projects/demo/revisions/r99/tree": errorReply(
        404,
        "revision_not_found",
      ),
    });
    await expect(getTree("demo", "r99", "")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "revision_not_found"),
    );
  });
});

describe("getFile", () => {
  it("answers the flat file view, including the too-large flag", async () => {
    const calls = stubFetch({
      "GET /api/v1/projects/demo/revisions/r5/file": {
        body: {
          path: "big.bin",
          kind: "file",
          size: 2_000_000,
          hash: "abc",
          binary: false,
          too_large: true,
        },
      },
    });
    const file = await getFile("demo", "r5", "big.bin");
    expect(file.too_large).toBe(true);
    expect(file.text).toBeUndefined();
    expect(calls[0]?.search.get("path")).toBe("big.bin");
  });
});
