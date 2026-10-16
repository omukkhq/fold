import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { LiveWorkspace } from "@/api/tasks";
import { liveWorkspace, workspaceEntry } from "@/test/payloads";
import { keys } from "../../api/keys";
import { attentionView, mainLine } from "./attention";
import { changeKind, fileLabel, visibleFiles } from "./files";
import { checkpointHref, taskHref, workingFileHref } from "./links";
import { createLiveStatesStore, shownAsOf } from "./liveStates";
import { buildPanels, lastActivity, seedLive, workerOf } from "./panels";
import { recordedItems } from "./recorded";

const live = (
  workspace: string,
  at = 5,
  over: Partial<LiveWorkspace> = {},
): LiveWorkspace =>
  liveWorkspace(workspace, "T-1", { last_activity_at: at, ...over });
const entry = (
  id: string,
  over: Partial<Parameters<typeof workspaceEntry>[1]> = {},
) => workspaceEntry(id, { live: live(id), ...over });

describe("links", () => {
  it("links a working file from the base to the workspace", () => {
    expect(workingFileHref("demo", "r7", "ws-1", "src/a b.ts")).toBe(
      "/p/demo/diff?from=r7&to=ws-1&path=src%2Fa+b.ts",
    );
  });

  it("links a checkpoint to its comparison", () => {
    expect(checkpointHref("demo", "r7", "ck-3")).toBe(
      "/p/demo/diff?from=r7&to=ck-3",
    );
    expect(taskHref("demo", "T-12")).toBe("/p/demo/t/T-12");
  });
});

describe("recordedItems", () => {
  it("lists the checkpoints newest first, each with its comparison", () => {
    const items = recordedItems("demo", "r7", [
      { id: "ck-1", label: null, automatic: false, created_at: 10 },
      { id: "ck-2", label: "quiet 2 min", automatic: true, created_at: 30 },
    ]);
    expect(items.map((i) => [i.id, i.text, i.href])).toEqual([
      [
        "ck-2",
        "checkpoint · automatic · quiet 2 min",
        "/p/demo/diff?from=r7&to=ck-2",
      ],
      ["ck-1", "checkpoint · explicit", "/p/demo/diff?from=r7&to=ck-1"],
    ]);
  });

  it("breaks a time tie by id", () => {
    const items = recordedItems("demo", "r7", [
      { id: "ck-2", label: null, automatic: true, created_at: 1 },
      { id: "ck-1", label: null, automatic: true, created_at: 1 },
    ]);
    expect(items.map((i) => i.id)).toEqual(["ck-1", "ck-2"]);
  });
});

describe("files", () => {
  const file = (path: string, previous: string | null = null) => ({
    path,
    previous_path: previous,
    change: "modified" as const,
    added_lines: 0,
    removed_lines: 0,
    binary: false,
  });

  it("shows three files and counts the hidden rest", () => {
    const files = ["a", "b", "c", "d", "e"].map((p) => file(p));
    expect(visibleFiles(files, false)).toEqual({
      shown: files.slice(0, 3),
      hidden: 2,
    });
    expect(visibleFiles(files, true)).toEqual({ shown: files, hidden: 0 });
    expect(visibleFiles(files.slice(0, 3), false).hidden).toBe(0);
  });

  it("names the change kinds and both paths of a rename", () => {
    expect(changeKind("added")).toEqual({ letter: "A", label: "Added" });
    expect(changeKind("renamed").letter).toBe("R");
    expect(fileLabel(file("b.ts", "a.ts"))).toBe("a.ts → b.ts");
    expect(fileLabel(file("b.ts"))).toBe("b.ts");
  });
});

describe("attention", () => {
  it("shows the daemon's stalled and stale items and nothing else", () => {
    expect(attentionView([], 0, 1000)).toEqual({
      stalled: null,
      staleClaim: false,
    });
    expect(attentionView([{ kind: "stalled" }], 0, 7 * 60_000)).toEqual({
      stalled: "no edits for 7 min",
      staleClaim: false,
    });
    expect(attentionView([{ kind: "stale_claim" }], 0, 1000)).toEqual({
      stalled: null,
      staleClaim: true,
    });
    expect(
      attentionView([{ kind: "question" }, { kind: "human_review" }], 0, 1000),
    ).toEqual({ stalled: null, staleClaim: false });
  });

  it("says how far main is ahead of the base", () => {
    const repository = {
      head: "r9",
      recent: [
        { id: "r9", parent: "r8" },
        { id: "r8", parent: "r7" },
        { id: "r7", parent: null },
      ],
    };
    expect(mainLine("r9", repository)).toBe("Base r9 is current main.");
    expect(mainLine("r8", repository)).toBe(
      "Main is at r9; base r8 is 1 revision behind.",
    );
    expect(mainLine("r7", repository)).toBe(
      "Main is at r9; base r7 is 2 revisions behind.",
    );
    expect(mainLine("r2", repository)).toBe(
      "Main is at r9; base r2 is at least 3 revisions behind.",
    );
  });

  it("names the base alone until main is read", () => {
    expect(mainLine("r7", undefined)).toBe("Base r7.");
    expect(mainLine("r7", { head: null, recent: [] })).toBe("Base r7.");
  });
});

describe("panels", () => {
  it("seeds the live cache only where the stream has not written", () => {
    const client = new QueryClient();
    const streamed = live("ws-1", 99);
    client.setQueryData(keys.workspaceLive("demo", "ws-1"), streamed);
    seedLive(client, "demo", [
      entry("ws-1"),
      entry("ws-2"),
      entry("ws-3", { state: "missing", live: null }),
    ]);
    expect(client.getQueryData(keys.workspaceLive("demo", "ws-1"))).toBe(
      streamed,
    );
    expect(client.getQueryData(keys.workspaceLive("demo", "ws-2"))).toEqual(
      live("ws-2"),
    );
    expect(
      client.getQueryData(keys.workspaceLive("demo", "ws-3")),
    ).toBeUndefined();
  });

  it("builds a panel per workspace with live state and keeps a missing one without it", () => {
    const entries = [
      entry("ws-1"),
      entry("ws-2"),
      entry("ws-3", { state: "missing", live: null }),
    ];
    const panels = buildPanels(entries, [live("ws-1"), undefined, undefined]);
    expect(panels.map((p) => [p.entry.id, p.live === null])).toEqual([
      ["ws-1", false],
      ["ws-3", true],
    ]);
    expect(
      buildPanels(entries, [{ nope: 1 }, live("ws-2"), null]).map(
        (p) => p.entry.id,
      ),
    ).toEqual(["ws-2", "ws-3"]);
  });

  it("reads the activity time from live state, then the row, then the creation", () => {
    expect(lastActivity({ entry: entry("ws-1"), live: live("ws-1", 50) })).toBe(
      50,
    );
    expect(lastActivity({ entry: entry("ws-1"), live: null })).toBe(1);
    expect(
      lastActivity({
        entry: entry("ws-1", { last_activity_at: null, created_at: 3 }),
        live: null,
      }),
    ).toBe(3);
  });

  it("names the live worker, else the owner, else nobody", () => {
    expect(
      workerOf({
        entry: entry("ws-1", { owner: "old" }),
        live: live("ws-1", 5, { worker: "frontend/ui" }),
      }),
    ).toBe("frontend/ui");
    expect(workerOf({ entry: entry("ws-1"), live: null })).toBe("frontend/ui");
    expect(
      workerOf({ entry: entry("ws-1", { owner: null }), live: null }),
    ).toBeNull();
  });
});

describe("live states store", () => {
  it("bumps its version when a live entry is set or removed and ignores other keys", () => {
    const client = new QueryClient();
    const store = createLiveStatesStore(client, "demo");
    let calls = 0;
    const stop = store.subscribe(() => {
      calls += 1;
    });
    client.setQueryData(keys.workspaceLive("demo", "ws-1"), live("ws-1"));
    expect(store.read("ws-1")).toEqual(live("ws-1"));
    const after = store.getVersion();
    expect(after).toBeGreaterThan(0);
    client.setQueryData(keys.workspaceLive("other", "ws-1"), live("ws-1"));
    client.setQueryData(keys.workspaces("demo"), []);
    expect(store.getVersion()).toBe(after);
    client.removeQueries({
      queryKey: keys.workspaceLive("demo", "ws-1"),
      exact: true,
    });
    expect(store.getVersion()).toBeGreaterThan(after);
    expect(store.read("ws-1")).toBeUndefined();
    const seen = calls;
    stop();
    client.setQueryData(keys.workspaceLive("demo", "ws-2"), live("ws-2"));
    expect(calls).toBe(seen);
  });
});

describe("shownAsOf", () => {
  it("prefers the last snapshot, then the last event, then now", () => {
    expect(shownAsOf({ stateAsOf: 5, lastEventAt: 9 }, 20)).toBe(5);
    expect(shownAsOf({ stateAsOf: null, lastEventAt: 9 }, 20)).toBe(9);
    expect(shownAsOf({ stateAsOf: null, lastEventAt: null }, 20)).toBe(20);
  });
});
