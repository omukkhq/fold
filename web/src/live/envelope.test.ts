import { describe, expect, it } from "vitest";
import {
  isEventEnvelope,
  isLiveWorkspace,
  isSnapshot,
  liveWorkspaceOf,
  type EventEnvelope,
  type LiveWorkspace,
} from "./envelope";

function workspace(): LiveWorkspace {
  return {
    workspace: "ws-9",
    task: "T-12",
    worker: "frontend/ui",
    base: "r17",
    files: [
      {
        path: "src/board.ts",
        previous_path: "src/cards.ts",
        change: "renamed",
        added_lines: 4,
        removed_lines: 1,
        binary: false,
      },
    ],
    summary: "1 file, +4 -1",
    last_activity_at: 1_760_000_000_500,
    checkpoints: [
      {
        id: "ck-88",
        label: "before sync",
        automatic: false,
        created_at: 1_760_000_000_100,
      },
    ],
    proposal: "P-3",
    stalled: false,
  };
}

function stored(): Record<string, unknown> {
  return {
    kind: "event",
    seq: 41,
    type: "task.created",
    actor: "frontend/ui",
    task: "T-12",
    proposal: null,
    workspace: "ws-9",
    revision: "r17",
    data: { title_len: 14 },
    created_at: 1_760_000_000_000,
  };
}

function changed(): Record<string, unknown> {
  return { kind: "workspace_changed", ...workspace() };
}

function snapshot(): Record<string, unknown> {
  return { kind: "snapshot", seq: 41, workspaces: [workspace()] };
}

function without(value: object, key: string): Record<string, unknown> {
  return Object.fromEntries(Object.entries(value).filter(([k]) => k !== key));
}

function asFrame(value: unknown): EventEnvelope {
  if (!isEventEnvelope(value)) {
    throw new Error("the sample is a valid frame");
  }
  return value;
}

describe("isEventEnvelope", () => {
  it("accepts a frame of each kind in the generated shape", () => {
    expect(isEventEnvelope(stored())).toBe(true);
    expect(isEventEnvelope(changed())).toBe(true);
    expect(isEventEnvelope(snapshot())).toBe(true);
    expect(isEventEnvelope({ ...snapshot(), workspaces: [] })).toBe(true);
  });

  it("accepts an agent_presence_changed frame of each presence and refuses a malformed one", () => {
    for (const presence of ["active", "idle", "offline"]) {
      expect(
        isEventEnvelope({
          kind: "agent_presence_changed",
          agent: "frontend",
          presence,
        }),
        presence,
      ).toBe(true);
    }
    for (const bad of [
      { kind: "agent_presence_changed", agent: "frontend", presence: "away" },
      { kind: "agent_presence_changed", agent: 3, presence: "idle" },
      { kind: "agent_presence_changed", presence: "idle" },
      { kind: "agent_presence_changed", agent: "frontend" },
    ]) {
      expect(isEventEnvelope(bad), JSON.stringify(bad)).toBe(false);
    }
  });

  it("accepts a worker_output frame and refuses a malformed one", () => {
    expect(
      isEventEnvelope({
        kind: "worker_output",
        agent: "web-worker",
        step: "tool_use",
        tool: "Bash",
        text_length: null,
      }),
    ).toBe(true);
    expect(
      isEventEnvelope({
        kind: "worker_output",
        agent: "web-worker",
        step: "text",
        tool: null,
        text_length: 8,
      }),
    ).toBe(true);
    for (const bad of [
      { kind: "worker_output", step: "text", tool: null, text_length: 8 },
      { kind: "worker_output", agent: "a", tool: null, text_length: 8 },
      {
        kind: "worker_output",
        agent: "a",
        step: "text",
        tool: 3,
        text_length: 8,
      },
      {
        kind: "worker_output",
        agent: "a",
        step: "text",
        tool: null,
        text_length: -1,
      },
    ]) {
      expect(isEventEnvelope(bad), JSON.stringify(bad)).toBe(false);
    }
  });

  it("accepts a workspace_changed frame without seq", () => {
    const frame = changed();
    expect("seq" in frame).toBe(false);
    expect(isEventEnvelope(frame)).toBe(true);
  });

  it("accepts a stored event of a type this build does not know", () => {
    expect(isEventEnvelope({ ...stored(), type: "future.thing" })).toBe(true);
  });

  it("refuses an unknown or missing kind", () => {
    for (const bad of [
      null,
      "x",
      [],
      { ...stored(), kind: "future" },
      without(stored(), "kind"),
    ]) {
      expect(isEventEnvelope(bad)).toBe(false);
    }
  });

  it("refuses a stored event with any field missing", () => {
    for (const key of Object.keys(stored())) {
      expect(isEventEnvelope(without(stored(), key)), key).toBe(false);
    }
  });

  it("refuses a stored event with a field of the wrong type", () => {
    for (const [key, value] of [
      ["seq", "1"],
      ["seq", 1.5],
      ["seq", -1],
      ["type", ""],
      ["type", 3],
      ["actor", null],
      ["task", 12],
      ["proposal", 3],
      ["workspace", 9],
      ["revision", 17],
      ["data", []],
      ["data", null],
      ["created_at", "now"],
    ] as const) {
      expect(isEventEnvelope({ ...stored(), [key]: value }), key).toBe(false);
    }
  });

  it("accepts the live state of the main workspace, which has no task", () => {
    expect(isEventEnvelope({ ...changed(), task: null })).toBe(true);
    expect(isLiveWorkspace({ ...workspace(), task: null })).toBe(true);
  });

  it("refuses a live workspace with any field missing or wrong", () => {
    for (const key of Object.keys(workspace())) {
      expect(isEventEnvelope(without(changed(), key)), key).toBe(false);
    }
    for (const [key, value] of [
      ["workspace", 9],
      ["task", 12],
      ["worker", null],
      ["base", 17],
      ["files", {}],
      ["summary", 1],
      ["last_activity_at", "now"],
      ["checkpoints", {}],
      ["proposal", 3],
      ["stalled", "yes"],
    ] as const) {
      expect(isEventEnvelope({ ...changed(), [key]: value }), key).toBe(false);
    }
  });

  it("refuses a changed file or checkpoint with any field missing or wrong", () => {
    const file = workspace().files[0];
    const checkpoint = workspace().checkpoints[0];
    const withFile = (value: unknown) => ({ ...changed(), files: [value] });
    const withCheckpoint = (value: unknown) => ({
      ...changed(),
      checkpoints: [value],
    });
    expect(isEventEnvelope(withFile("src/a.ts"))).toBe(false);
    expect(isEventEnvelope(withCheckpoint("ck-1"))).toBe(false);
    for (const key of Object.keys(file ?? {})) {
      expect(isEventEnvelope(withFile(without(file ?? {}, key))), key).toBe(
        false,
      );
    }
    for (const [key, value] of [
      ["path", 1],
      ["previous_path", 1],
      ["change", "moved"],
      ["added_lines", -1],
      ["added_lines", 0.5],
      ["removed_lines", "1"],
      ["binary", "no"],
    ] as const) {
      expect(isEventEnvelope(withFile({ ...file, [key]: value })), key).toBe(
        false,
      );
    }
    expect(isEventEnvelope(withFile({ ...file, previous_path: null }))).toBe(
      true,
    );
    for (const key of Object.keys(checkpoint ?? {})) {
      expect(
        isEventEnvelope(withCheckpoint(without(checkpoint ?? {}, key))),
        key,
      ).toBe(false);
    }
    for (const [key, value] of [
      ["id", 88],
      ["label", 1],
      ["automatic", "yes"],
      ["created_at", "now"],
    ] as const) {
      expect(
        isEventEnvelope(withCheckpoint({ ...checkpoint, [key]: value })),
        key,
      ).toBe(false);
    }
  });

  it("refuses a snapshot without a valid seq or workspace list", () => {
    for (const bad of [
      without(snapshot(), "seq"),
      { ...snapshot(), seq: -1 },
      without(snapshot(), "workspaces"),
      { ...snapshot(), workspaces: {} },
      { ...snapshot(), workspaces: [{ workspace: "ws-1" }] },
    ]) {
      expect(isEventEnvelope(bad)).toBe(false);
    }
  });
});

describe("frame kinds", () => {
  const event = asFrame(stored());
  const live = asFrame(changed());
  const snap = asFrame(snapshot());

  it("tells a snapshot apart", () => {
    expect(isSnapshot(snap)).toBe(true);
    expect(isSnapshot(event)).toBe(false);
    expect(isSnapshot(live)).toBe(false);
  });

  it("takes the live state out of a workspace_changed frame without its tag", () => {
    if (live.kind !== "workspace_changed") {
      throw new Error("the sample is a workspace_changed frame");
    }
    const state = liveWorkspaceOf(live);
    expect(state).toEqual(workspace());
    expect(isLiveWorkspace(state)).toBe(true);
  });
});
