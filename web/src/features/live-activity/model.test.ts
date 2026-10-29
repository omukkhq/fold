import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { LiveWorkspace } from "@/api/tasks";
import { liveWorkspace, workspaceEntry } from "@/test/payloads";
import { keys } from "../../api/keys";
import { attentionView, behindTag, mainHeadText } from "./attention";
import { changeKind, fileLabel, visibleFiles } from "./files";
import { checkpointHref, taskHref, workingFileHref } from "./links";
import { createLiveStatesStore, shownAsOf } from "./liveStates";
import {
  buildPanels,
  lastActivity,
  panelHeading,
  seedLive,
  workerOf,
} from "./panels";
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
    expect(changeKind("added")).toEqual({
      letter: "A",
      label: "Added",
      tone: "bg-ok-soft text-ok",
    });
    expect(changeKind("deleted").tone).toBe("bg-bad-soft text-bad");
    expect(changeKind("modified").tone).toBe("bg-fill text-text-2");
    expect(changeKind("renamed").letter).toBe("R");
    expect(fileLabel(file("b.ts", "a.ts"))).toBe("a.ts → b.ts");
    expect(fileLabel(file("b.ts"))).toBe("b.ts");
  });
});

describe("attention", () => {
  it("shows the daemon's stalled and stale items and nothing else", () => {
    expect(attentionView([], 0, 1000)).toEqual({
      checks: null,
      stalled: null,
      staleClaim: false,
    });
    expect(
      attentionView([{ kind: "stalled", closable: false }], 0, 7 * 60_000),
    ).toEqual({
      checks: null,
      stalled: "no edits for 7 min",
      staleClaim: false,
    });
    expect(
      attentionView([{ kind: "stale_claim", closable: false }], 0, 1000),
    ).toEqual({
      checks: null,
      stalled: null,
      staleClaim: true,
    });
    expect(
      attentionView(
        [
          { kind: "question", closable: true },
          { kind: "human_review", closable: false },
        ],
        0,
        1000,
      ),
    ).toEqual({ checks: null, stalled: null, staleClaim: false });
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
    expect(mainHeadText(repository)).toBe("Main is at r9");
    expect(behindTag("r9", repository)).toBeNull();
    expect(behindTag("r8", repository)).toBe("1 behind");
    expect(behindTag("r7", repository)).toBe("2 behind");
    expect(behindTag("r2", repository)).toBe("3+ behind");
  });

  it("says nothing of main until it is read", () => {
    expect(mainHeadText(undefined)).toBeNull();
    expect(mainHeadText({ head: null, recent: [] })).toBeNull();
    expect(behindTag("r7", undefined)).toBeNull();
    expect(behindTag("r7", { head: null, recent: [] })).toBeNull();
  });

  it("shows running checks instead of a stall while the live state names a run", () => {
    const run = {
      run: "cr-4",
      purpose: "check" as const,
      proposal: null,
      started_at: 0,
    };
    const items = [
      { kind: "stalled" as const, closable: false },
      { kind: "stale_claim" as const, closable: false },
    ];
    expect(attentionView(items, 0, 7 * 60_000, { check_run: run })).toEqual({
      checks: "running checks",
      stalled: null,
      staleClaim: true,
    });
    expect(
      attentionView(items, 0, 7 * 60_000, { check_run: null }).stalled,
    ).toBe("no edits for 7 min");
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

  it("heads a task workspace with its task and the main workspace with its own name", () => {
    expect(panelHeading(workspaceEntry("ws-1"))).toEqual({
      task: "T-1",
      title: "Title T-1",
    });
    expect(
      panelHeading(workspaceEntry("ws-1", { task: "T-4", task_title: null })),
    ).toEqual({ task: "T-4", title: "T-4" });
    expect(
      panelHeading(
        workspaceEntry("ws-2", { task: null, task_title: null, kind: "main" }),
      ),
    ).toEqual({ task: null, title: "Main workspace" });
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
