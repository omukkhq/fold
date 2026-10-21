import { describe, expect, it } from "vitest";
import type {
  CheckpointSummary,
  LiveWorkspace,
  TaskDetail,
  TaskWorkspaceInfo,
} from "@/api/tasks";
import {
  baseText,
  changeLetter,
  changesNote,
  changeTone,
  recordedCheckpoints,
  latestCheckpoint,
  lineCounts,
  liveChipText,
  liveStrip,
} from "./liveStrip";

const NOW = 1_800_000_000_000;
const SECOND = 1_000;
const MINUTE = 60 * SECOND;

function live(extra: Partial<LiveWorkspace> = {}): LiveWorkspace {
  return {
    workspace: "ws-2",
    task: "T-2",
    worker: "frontend/ui",
    base: "r7",
    files: [
      {
        path: "a.ts",
        previous_path: null,
        change: "modified",
        added_lines: 1,
        removed_lines: 0,
        binary: false,
      },
    ],
    summary: "",
    last_activity_at: NOW - 12 * SECOND,
    checkpoints: [],
    proposal: null,
    stalled: false,
    edits_after_version: null,
    ...extra,
  };
}

function workspace(extra: Partial<TaskWorkspaceInfo> = {}): TaskWorkspaceInfo {
  return {
    directory: "ws/T-2",
    base: "r7",
    main: "r8",
    behind: 1,
    ...extra,
  };
}

const ck = (id: string, ago: number): CheckpointSummary => ({
  id,
  label: null,
  automatic: true,
  created_at: NOW - ago,
});

type StripTask = Pick<
  TaskDetail,
  | "activity"
  | "attention"
  | "claims"
  | "checkpoints"
  | "edits_after_version"
  | "proposal"
  | "workspace"
  | "workspace_info"
>;

function task(extra: Partial<StripTask> = {}): StripTask {
  return {
    activity: { path: "src/routes/login.tsx", at: NOW - 12 * SECOND },
    attention: [],
    claims: [
      {
        kind: "implementation",
        holder: "frontend/ui",
        claimed_at: NOW - 40 * MINUTE,
        renewed_at: NOW - 2 * MINUTE,
        stale: false,
      },
    ],
    checkpoints: [ck("ck-31", 3 * MINUTE)],
    edits_after_version: null,
    proposal: null,
    workspace: "ws-2",
    workspace_info: workspace(),
    ...extra,
  };
}

describe("liveStrip", () => {
  it("says who edits which file, the files, the latest checkpoint and the base against main", () => {
    expect(liveStrip(task(), live(), NOW)).toEqual({
      worker: "frontend/ui",
      verb: "is editing",
      file: "src/routes/login.tsx",
      rest: "· 1 file · ck-31 3 min ago · base r7 (main r8)",
      stalled: false,
    });
  });

  it("counts files in the plural and marks a base on the main head", () => {
    const two = live({ files: [...live().files, ...live().files] });
    const strip = liveStrip(
      task({ workspace_info: workspace({ base: "r8" }), checkpoints: [] }),
      two,
      NOW,
    );
    expect(strip?.rest).toBe("· 2 files · base r8 (main)");
  });

  it("falls back to the claim holder before the stream reports the workspace", () => {
    const strip = liveStrip(task({ activity: null }), null, NOW);
    expect(strip).toMatchObject({ worker: "frontend/ui", file: null });
    expect(strip?.rest).toBe("· ck-31 3 min ago · base r7 (main r8)");
  });

  it("says how long the worker has been quiet while the daemon reports it stalled", () => {
    const strip = liveStrip(
      task({
        attention: [{ kind: "stalled" }],
        activity: { path: "a.ts", at: NOW - 7 * MINUTE },
      }),
      live({ last_activity_at: NOW - 8 * MINUTE }),
      NOW,
    );
    expect(strip).toMatchObject({
      verb: "no edits for 7 min",
      file: null,
      stalled: true,
    });
  });

  it("keeps the verb when a stalled worker has no recorded edit", () => {
    const strip = liveStrip(
      task({ attention: [{ kind: "stalled" }], activity: null }),
      null,
      NOW,
    );
    expect(strip?.verb).toBe("is editing");
  });

  it("says the holder sent the version and what it waits for, never the stall, while the files are that version", () => {
    const proposal = {
      id: "P-1",
      title: "Add the form",
      review_claim: null,
      files: 3,
      state: "open" as const,
      version: 1,
      checks_failed: false,
      reviews: [],
    };
    const sent = task({
      attention: [{ kind: "stalled" }],
      proposal,
      edits_after_version: false,
    });
    expect(liveStrip(sent, null, NOW)).toEqual({
      worker: "frontend/ui",
      verb: "sent P-1 v1",
      file: null,
      rest: "· waits for review · ck-31 3 min ago · base r7 (main r8)",
      stalled: false,
    });
    const frame = live({ proposal: "P-1", edits_after_version: true });
    expect(liveStrip(sent, frame, NOW)).toMatchObject({
      verb: "no edits for 12 s",
      stalled: true,
    });
    expect(
      liveStrip(
        task({ proposal, edits_after_version: true }),
        live({ proposal: "P-1", edits_after_version: false }),
        NOW,
      )?.rest,
    ).toBe("· waits for review · 1 file · ck-31 3 min ago · base r7 (main r8)");
  });

  it("says the stall and the wait while a sent version is back with its author", () => {
    const back = task({
      attention: [{ kind: "stalled" }],
      proposal: {
        id: "P-1",
        title: "Add the form",
        review_claim: null,
        files: 3,
        state: "conflict",
        version: 1,
        checks_failed: false,
        reviews: [],
      },
      edits_after_version: false,
    });
    expect(liveStrip(back, null, NOW)).toMatchObject({
      verb: "no edits for 12 s",
      file: null,
      rest: "· conflicts with main · ck-31 3 min ago · base r7 (main r8)",
      stalled: true,
    });
    expect(
      liveStrip({ ...back, activity: null }, null, NOW)?.verb,
      "without a recorded edit the sent text stays",
    ).toBe("sent P-1 v1");
  });

  it("is null without a workspace or without anyone working in it", () => {
    expect(liveStrip(task({ workspace: null }), null, NOW)).toBeNull();
    expect(liveStrip(task({ claims: [] }), null, NOW)).toBeNull();
  });
});

describe("recordedCheckpoints", () => {
  it("lists the checkpoints of the task and the frame newest first, each once", () => {
    const frame = live({
      checkpoints: [ck("ck-40", SECOND), ck("ck-31", MINUTE)],
    });
    expect(
      recordedCheckpoints(
        [ck("ck-31", MINUTE), ck("ck-30", 2 * MINUTE)],
        frame,
      ).map((c) => c.id),
    ).toEqual(["ck-40", "ck-31", "ck-30"]);
    expect(recordedCheckpoints([], null)).toEqual([]);
  });
});

describe("liveStrip without the task details", () => {
  it("shows what the frame reports when the payload has no workspace details, activity or checkpoints", () => {
    const bare = {
      activity: null,
      attention: [],
      checkpoints: [],
      claims: task().claims,
      workspace: "ws-2",
      workspace_info: null,
      proposal: null,
      edits_after_version: null,
    };
    expect(liveStrip(bare, live(), NOW)).toEqual({
      worker: "frontend/ui",
      verb: "is editing",
      file: "a.ts",
      rest: "· 1 file · base r7",
      stalled: false,
    });
    expect(liveStrip(bare, null, NOW)?.rest).toBe("");
  });
});

describe("latestCheckpoint", () => {
  it("takes the newest of the task and the live frame", () => {
    const frame = live({ checkpoints: [ck("ck-40", SECOND)] });
    expect(latestCheckpoint([ck("ck-31", MINUTE)], frame)?.id).toBe("ck-40");
    expect(
      latestCheckpoint([ck("ck-31", MINUTE), ck("ck-30", 2 * MINUTE)], null)
        ?.id,
    ).toBe("ck-31");
    expect(latestCheckpoint([], null)).toBeNull();
  });
});

describe("the working-changes texts", () => {
  it("say live, how many files and that they are not in main", () => {
    expect(liveChipText(live(), NOW)).toBe("Live · 12 s ago");
    expect(changesNote(live(), workspace())).toBe(
      "1 file against base r7 · not in main",
    );
    expect(changesNote({ ...live(), base: "r6" }, null)).toBe(
      "1 file against base r6 · not in main",
    );
    expect(baseText({ base: "r7", main: "r9" })).toBe("base r7 (main r9)");
    expect(baseText({ base: "r7" })).toBe("base r7");
  });

  it("letter and colour each change kind", () => {
    expect(
      (["added", "modified", "deleted", "renamed"] as const).map(changeLetter),
    ).toEqual(["A", "M", "D", "R"]);
    expect(changeTone("added")).toBe("added");
    expect(changeTone("deleted")).toBe("deleted");
    expect(changeTone("modified")).toBe("other");
    expect(changeTone("renamed")).toBe("other");
  });

  it("count lines with a true minus sign and leave out zeros and binaries", () => {
    expect(
      lineCounts({ added_lines: 42, removed_lines: 8, binary: false }),
    ).toEqual({ added: "+42", removed: "−8" });
    expect(
      lineCounts({ added_lines: 0, removed_lines: 3, binary: false }),
    ).toEqual({ added: null, removed: "−3" });
    expect(
      lineCounts({ added_lines: 5, removed_lines: 5, binary: true }),
    ).toEqual({ added: null, removed: null });
  });
});
