import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { WorkspaceEntry } from "../../api/workspaces";
import { keys } from "../../api/keys";
import type { LiveWorkspace } from "../../live/envelope";
import { attentionView, mainLine } from "./attention";
import { changeKind, fileLabel, visibleFiles } from "./files";
import {
  changesetHref,
  checkpointHref,
  taskHref,
  workingFileHref,
} from "./links";
import { createLiveStatesStore, shownAsOf } from "./liveStates";
import { buildPanels, lastActivity, seedLive } from "./panels";
import { recordedItems } from "./recorded";

function live(workspace: string, at = 5): LiveWorkspace {
  return {
    workspace,
    task: "T-1",
    worker: "frontend",
    base: "r7",
    files: [],
    summary: "0 files, +0 -0",
    last_activity_at: at,
    checkpoints: [],
    proposal: null,
  };
}

function entry(
  workspace: string,
  patch: Partial<WorkspaceEntry> = {},
): WorkspaceEntry {
  return {
    workspace,
    task: "T-1",
    task_title: "t",
    worker: "frontend",
    state: "active",
    base: "r7",
    main: "r9",
    behind: 2,
    last_activity_at: 1,
    attention: [],
    watch_degraded: false,
    changesets: [],
    live: live(workspace),
    ...patch,
  };
}

describe("links", () => {
  it("links a working file from the base to the workspace", () => {
    expect(workingFileHref("demo", "r7", "ws-1", "src/a b.ts")).toBe(
      "/p/demo/diff?from=r7&to=ws-1&path=src%2Fa+b.ts",
    );
  });

  it("links a checkpoint and a changeset to their comparisons", () => {
    expect(checkpointHref("demo", "r7", "ck-3")).toBe(
      "/p/demo/diff?from=r7&to=ck-3",
    );
    expect(changesetHref("demo", "r6", "C-4")).toBe(
      "/p/demo/diff?from=r6&to=C-4",
    );
    expect(taskHref("demo", "T-12")).toBe("/p/demo/t/T-12");
  });
});

describe("recordedItems", () => {
  it("merges checkpoints and changesets newest first", () => {
    const items = recordedItems(
      "demo",
      "r7",
      [
        { id: "ck-1", label: null, automatic: false, created_at: 10 },
        { id: "ck-2", label: "quiet 2 min", automatic: true, created_at: 30 },
      ],
      [{ id: "C-1", message: "Fields", recorded_at: 20, base: "r6" }],
    );
    expect(items.map((i) => [i.id, i.text, i.href])).toEqual([
      [
        "ck-2",
        "checkpoint · automatic · quiet 2 min",
        "/p/demo/diff?from=r7&to=ck-2",
      ],
      ["C-1", "Fields", "/p/demo/diff?from=r6&to=C-1"],
      ["ck-1", "checkpoint · explicit", "/p/demo/diff?from=r7&to=ck-1"],
    ]);
    expect(items.map((i) => i.kind)).toEqual([
      "checkpoint",
      "changeset",
      "checkpoint",
    ]);
  });

  it("breaks a time tie by id", () => {
    const items = recordedItems(
      "demo",
      "r7",
      [{ id: "ck-2", label: null, automatic: true, created_at: 1 }],
      [{ id: "C-1", message: "m", recorded_at: 1, base: "r7" }],
    );
    expect(items.map((i) => i.id)).toEqual(["C-1", "ck-2"]);
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
    expect(mainLine({ base: "r9", main: "r9", behind: 0 })).toBe(
      "Base r9 is current main.",
    );
    expect(mainLine({ base: "r8", main: "r9", behind: 1 })).toBe(
      "Main is at r9; base r8 is 1 revision behind.",
    );
    expect(mainLine({ base: "r7", main: "r9", behind: 2 })).toBe(
      "Main is at r9; base r7 is 2 revisions behind.",
    );
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
    expect(panels.map((p) => [p.entry.workspace, p.live === null])).toEqual([
      ["ws-1", false],
      ["ws-3", true],
    ]);
    expect(
      buildPanels(entries, [{ nope: 1 }, live("ws-2"), null]).map(
        (p) => p.entry.workspace,
      ),
    ).toEqual(["ws-2", "ws-3"]);
  });

  it("reads the activity time from live state first", () => {
    expect(lastActivity({ entry: entry("ws-1"), live: live("ws-1", 50) })).toBe(
      50,
    );
    expect(lastActivity({ entry: entry("ws-1"), live: null })).toBe(1);
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
