import { afterEach, describe, expect, it, vi } from "vitest";
import { workspaceEntry } from "@/test/payloads";
import { errorReply, stubFetch } from "@/test/stubFetch";
import { isApiError } from "./errors";
import {
  getWorkspace,
  getWorkspaceChanges,
  getWorkspaceTree,
  listWorkspaces,
} from "./workspaces";

afterEach(() => {
  vi.unstubAllGlobals();
});

const BASE = "/api/v1/projects/demo";

describe("the workspace seam", () => {
  it("lists the workspaces with their attention items", async () => {
    const entry = workspaceEntry("ws-1", {
      attention: [{ kind: "stalled", closable: false }],
    });
    stubFetch({
      [`GET ${BASE}/workspaces`]: { body: { workspaces: [entry] } },
    });
    expect(await listWorkspaces("demo")).toEqual([entry]);
  });

  it("reads one workspace, live: null when it is not attached", async () => {
    const entry = workspaceEntry("ws-2", { state: "archived", live: null });
    stubFetch({ [`GET ${BASE}/workspaces/ws-2`]: { body: entry } });
    expect((await getWorkspace("demo", "ws-2")).live).toBeNull();
  });

  it("lists one directory of the working files", async () => {
    const entries = [{ name: "a.ts", path: "src/a.ts", kind: "file" }];
    stubFetch({
      [`GET ${BASE}/workspaces/ws-1/tree`]: {
        body: { path: "src", entries },
      },
    });
    expect(await getWorkspaceTree("demo", "ws-1", "src")).toEqual(entries);
  });

  it("reads the working changes and rejects a workspace that is not active", async () => {
    const changes = {
      ...workspaceEntry("ws-1").live,
      source: "working",
      watch: "degraded",
      warnings: [{ path: "a.log", reason: "unstable" }],
      unfinished_write: null,
    };
    stubFetch({
      [`GET ${BASE}/workspaces/ws-1/changes`]: { body: changes },
      [`GET ${BASE}/workspaces/ws-3/changes`]: errorReply(
        409,
        "workspace_not_active",
      ),
    });
    expect((await getWorkspaceChanges("demo", "ws-1")).watch).toBe("degraded");
    await expect(getWorkspaceChanges("demo", "ws-3")).rejects.toSatisfy(
      (e: unknown) => isApiError(e, "workspace_not_active"),
    );
  });
});
