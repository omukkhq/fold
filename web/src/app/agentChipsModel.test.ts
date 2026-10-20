import { describe, expect, it } from "vitest";
import type { ProjectAgent } from "../api/agentBar";
import { agent as baseAgent, claim, worker } from "../test/agents";
import { INITIAL_STATUS, type LiveStatus } from "../live/status";
import {
  MAX_CHIPS,
  agentBoardPath,
  agentStateText,
  chipAgents,
  chipPresence,
  chipStateText,
  failedText,
  chipErrorTitle,
  MODEL_UNREACHABLE_TEXT,
  isPaused,
} from "./agentChipsModel";

interface ChipOptions {
  name?: string;
  presence?: ProjectAgent["presence"];
  /** Implementation claims held. */
  working?: number;
  /** Of those, the tasks with a stalled item. */
  stalled?: number;
}

/** An agent whose single worker holds `working` implementation claims, `stalled` of their tasks stalled. */
const agent = ({
  name = "frontend",
  presence = "active",
  working = 0,
  stalled = 0,
}: ChipOptions): ProjectAgent => ({
  ...baseAgent(name, { presence }),
  workers: [
    worker(name, {
      claims: Array.from({ length: working }, (_, i) =>
        claim({ task: `T-${String(i + 1)}` }),
      ),
    }),
  ],
  attention: Array.from({ length: stalled }, (_, i) => ({
    kind: "stalled" as const,
    task: `T-${String(i + 1)}`,
  })),
});

const status = (patch: Partial<LiveStatus>): LiveStatus => ({
  ...INITIAL_STATUS,
  ...patch,
});

describe("agentStateText", () => {
  it("says not connected for an offline agent", () => {
    expect(agentStateText(agent({ presence: "offline", working: 2 }))).toBe(
      "not connected",
    );
  });

  it("says waiting for a connected agent with no claim", () => {
    expect(agentStateText(agent({ presence: "idle" }))).toBe("waiting");
  });

  it("counts working tasks and adds the stalled ones when there are any", () => {
    expect(agentStateText(agent({ working: 2 }))).toBe("2 working");
    expect(agentStateText(agent({ working: 2, stalled: 1 }))).toBe(
      "2 working · 1 stalled",
    );
  });
});

/** `agent` as fold supervises it, with its process in `state`. */
const supervised = (
  state: NonNullable<ProjectAgent["process"]>["state"],
  options: ChipOptions = {},
  failedReason: string | null = null,
  error: { last_error: string | null; model_unreachable: boolean } = {
    last_error: null,
    model_unreachable: false,
  },
): ProjectAgent => ({
  ...agent({ presence: "offline", ...options }),
  process: { state, failed_reason: failedReason, ...error },
});

describe("agentStateText of a supervised agent", () => {
  it("shows the process state instead of the presence it has between turns", () => {
    expect(agentStateText(supervised("idle"))).toBe("idle");
    expect(agentStateText(supervised("starting"))).toBe("starting");
    expect(agentStateText(supervised("stopping"))).toBe("stopping");
    expect(agentStateText(supervised("stopped"))).toBe("stopped");
  });

  it("counts the claims of a working one and the stalled tasks of a working or idle one", () => {
    expect(agentStateText(supervised("working"))).toBe("working");
    expect(agentStateText(supervised("working", { working: 2 }))).toBe(
      "2 working",
    );
    expect(
      agentStateText(supervised("working", { working: 2, stalled: 1 })),
    ).toBe("2 working · 1 stalled");
    expect(agentStateText(supervised("idle", { stalled: 1 }))).toBe(
      "idle · 1 stalled",
    );
  });

  it("says that a process whose turns fail cannot reach the model unless it failed for a reason the chip names, with Claude Code's error as the tooltip", () => {
    const failing = { last_error: "Invalid key", model_unreachable: true };
    for (const state of ["idle", "working", "failed"] as const) {
      expect(agentStateText(supervised(state, {}, null, failing))).toBe(
        MODEL_UNREACHABLE_TEXT,
      );
    }
    expect(
      agentStateText(supervised("failed", {}, "fld_missing", failing)),
    ).toBe("failed: fld not found");
    expect(agentStateText(supervised("failed", {}, "exited", failing))).toBe(
      MODEL_UNREACHABLE_TEXT,
    );
    expect(chipErrorTitle(supervised("idle", {}, null, failing))).toBe(
      "Invalid key",
    );
    expect(chipErrorTitle(supervised("idle"))).toBeUndefined();
    expect(chipErrorTitle(agent({ presence: "active" }))).toBeUndefined();
  });

  it("names the failures a person can act on", () => {
    expect(agentStateText(supervised("failed", {}, "mcp_unavailable"))).toBe(
      "failed: no fold connection",
    );
    expect(agentStateText(supervised("failed", {}, "fld_missing"))).toBe(
      "failed: fld not found",
    );
    expect(agentStateText(supervised("failed", {}, "exited"))).toBe("failed");
    expect(failedText(null)).toBe("failed");
  });

  it("reads a live session by the presence rules once the process has ended, and the process without one", () => {
    const session = (
      state: "stopped" | "failed",
      presence: ProjectAgent["presence"],
    ): ProjectAgent => ({
      ...supervised(state, { working: 1 }, "fld_missing"),
      presence,
    });
    expect(agentStateText(session("failed", "active"))).toBe("1 working");
    expect(chipPresence(session("failed", "active"), false)).toBe("active");
    expect(agentStateText(session("stopped", "idle"))).toBe("1 working");
    expect(chipPresence(session("stopped", "idle"), false)).toBe("idle");
    expect(agentStateText(session("failed", "offline"))).toBe(
      "failed: fld not found",
    );
    expect(chipPresence(session("failed", "offline"), false)).toBe("offline");
    expect(agentStateText(session("stopped", "offline"))).toBe("stopped");
    const running = { ...supervised("idle"), presence: "active" as const };
    expect(agentStateText(running)).toBe("idle");
    expect(chipPresence(running, false)).toBe("idle");
  });

  it("still reads paused while the project is paused", () => {
    expect(
      chipStateText(supervised("idle"), status({ state: "live" }), 0, true),
    ).toBe("paused");
  });
});

describe("chipPresence", () => {
  it("draws a supervised agent's dot from its process and any other agent's from its presence", () => {
    expect(chipPresence(agent({ presence: "idle" }), false)).toBe("idle");
    expect(chipPresence(supervised("starting"), false)).toBe("active");
    expect(chipPresence(supervised("working"), false)).toBe("active");
    expect(chipPresence(supervised("idle"), false)).toBe("idle");
    expect(chipPresence(supervised("stopping"), false)).toBe("idle");
    expect(chipPresence(supervised("stopped"), false)).toBe("offline");
    expect(chipPresence(supervised("failed"), false)).toBe("offline");
  });

  it("draws every agent but the main agent offline while the project is paused, whatever state its row kept", () => {
    expect(chipPresence(supervised("working"), true)).toBe("offline");
    expect(chipPresence(supervised("idle"), true)).toBe("offline");
    expect(chipPresence(agent({ presence: "active" }), true)).toBe("offline");
    const main = { ...agent({ presence: "active" }), tags: ["main"] };
    expect(chipPresence(main, true)).toBe("active");
  });
});

describe("chipStateText", () => {
  const now = new Date(2026, 9, 15, 14, 20, 0).getTime();
  const synced = new Date(2026, 9, 15, 14, 12, 31).getTime();

  it("shows the counts while the stream is live", () => {
    expect(
      chipStateText(
        agent({ working: 2 }),
        status({ state: "live" }),
        now,
        false,
      ),
    ).toBe("2 working");
  });

  it("shows a dash before the first connection", () => {
    expect(
      chipStateText(agent({ working: 2 }), INITIAL_STATUS, now, false),
    ).toBe("—");
  });

  it("shows the time of the last sync to the minute while reconnecting", () => {
    expect(
      chipStateText(
        agent({ working: 2 }),
        status({ state: "reconnecting", lastEventAt: synced }),
        now,
        true,
      ),
    ).toBe("as of 14:12");
  });

  it("shows a dash while reconnecting when nothing was ever synced", () => {
    expect(
      chipStateText(agent({}), status({ state: "reconnecting" }), now, false),
    ).toBe("—");
  });

  it("says paused for every agent but the main agent while the project is paused", () => {
    const live = status({ state: "live" });
    expect(chipStateText(agent({ working: 2 }), live, now, true)).toBe(
      "paused",
    );
    const main = { ...agent({ name: "main", working: 1 }), tags: ["main"] };
    expect(chipStateText(main, live, now, true)).toBe("1 working");
  });
});

describe("isPaused", () => {
  it("reads the overview's paused_since", () => {
    expect(isPaused(undefined)).toBe(false);
    expect(isPaused(null)).toBe(false);
    expect(isPaused(1_792_000_000_000)).toBe(true);
  });
});

describe("chipAgents", () => {
  it("puts connected agents first and keeps the API order inside each group, without changing the input", () => {
    const input = [
      agent({ name: "review", presence: "offline" }),
      agent({ name: "zed", presence: "idle" }),
      agent({ name: "backend", presence: "active" }),
    ];
    expect(chipAgents(input, false).map((a) => a.name)).toEqual([
      "zed",
      "backend",
      "review",
    ]);
    expect(input[0]?.name).toBe("review");
  });

  it("keeps a chip in place when its agent turns from active to idle", () => {
    const before = [
      agent({ name: "frontend", presence: "active" }),
      agent({ name: "backend", presence: "active" }),
    ];
    const after = [
      agent({ name: "frontend", presence: "idle" }),
      agent({ name: "backend", presence: "active" }),
    ];
    expect(chipAgents(after, false).map((a) => a.name)).toEqual(
      chipAgents(before, false).map((a) => a.name),
    );
  });

  it("puts a supervised agent with a running process among the connected ones", () => {
    const input = [
      { ...supervised("failed"), name: "web-lead" },
      agent({ name: "main", presence: "offline" }),
      { ...supervised("idle"), name: "api-lead" },
    ];
    expect(chipAgents(input, false).map((a) => a.name)).toEqual([
      "api-lead",
      "web-lead",
      "main",
    ]);
  });

  it("puts the paused agents after the main agent while the project is paused", () => {
    const input = [
      { ...supervised("working"), name: "web-lead" },
      { ...agent({ name: "main", presence: "active" }), tags: ["main"] },
      { ...supervised("idle"), name: "api-lead" },
    ];
    expect(chipAgents(input, true).map((a) => a.name)).toEqual([
      "main",
      "web-lead",
      "api-lead",
    ]);
  });

  it("draws at most the chips the top bar has room for, connected ones first", () => {
    const input = [
      agent({ name: "a", presence: "offline" }),
      agent({ name: "b" }),
      agent({ name: "c", presence: "idle" }),
      agent({ name: "d" }),
      agent({ name: "e" }),
    ];
    const shown = chipAgents(input, false);
    expect(shown).toHaveLength(MAX_CHIPS);
    expect(shown.map((a) => a.name)).toEqual(["b", "c", "d"]);
  });
});

describe("agentBoardPath", () => {
  it("opens the board with the agent filter", () => {
    expect(agentBoardPath("demo", "frontend")).toBe(
      "/p/demo/board?agent=frontend",
    );
  });
});
