import { QueryClient } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { daemon, ok, refuse } from "@/test/daemon";
import { boardCard, boardView } from "@/test/payloads";
import { boardQuery, getBoard } from "./board";
import { apiErrorCode } from "./errors";
import { keys } from "./keys";

vi.mock("@/api/client", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/api/client")>();
  const { daemon: fake } = await import("@/test/daemon");
  return { ...real, api: fake.api };
});

beforeEach(() => {
  daemon.reset();
});

describe("getBoard", () => {
  it("reads the board of the project", async () => {
    const board = boardView([boardCard("T-1")], { done: 3 });
    daemon.on("GET /api/v1/projects/demo/board", ok(board));
    expect(await getBoard("demo")).toEqual(board);
    expect(daemon.calls).toHaveLength(1);
  });

  it("throws the daemon's code for a project that is not there", async () => {
    daemon.on(
      "GET /api/v1/projects/ghost/board",
      refuse(404, "project_not_found"),
    );
    await expect(getBoard("ghost")).rejects.toSatisfy(
      (e: unknown) => apiErrorCode(e) === "project_not_found",
    );
  });

  it("throws a coded error for an answer that is not the error envelope", async () => {
    daemon.on("GET /api/v1/projects/demo/board", {
      status: 502,
      body: "bad gateway",
    });
    await expect(getBoard("demo")).rejects.toSatisfy(
      (e: unknown) => apiErrorCode(e) === "daemon_unavailable",
    );
  });
});

describe("boardQuery", () => {
  it("caches the board under the board key and never goes stale by time", async () => {
    const board = boardView([boardCard("T-1")]);
    daemon.on("GET /api/v1/projects/demo/board", ok(board));
    const client = new QueryClient();
    expect(await client.query(boardQuery("demo"))).toEqual(board);
    expect(client.getQueryData(keys.board("demo"))).toEqual(board);
    expect(boardQuery("demo").staleTime).toBe(Infinity);
  });
});
