import { describe, expect, it } from "vitest";
import type { ProjectAgent } from "../api/agentBar";
import { agent as baseAgent, claim, worker } from "../test/agents";
import { INITIAL_STATUS, type LiveStatus } from "../live/status";
import {
  MAX_CHIPS,
  agentBoardPath,
  agentStateText,
  chipAgents,
  chipStateText,
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
    expect(chipAgents(input).map((a) => a.name)).toEqual([
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
    expect(chipAgents(after).map((a) => a.name)).toEqual(
      chipAgents(before).map((a) => a.name),
    );
  });

  it("draws at most the chips the top bar has room for, connected ones first", () => {
    const input = [
      agent({ name: "a", presence: "offline" }),
      agent({ name: "b" }),
      agent({ name: "c", presence: "idle" }),
      agent({ name: "d" }),
      agent({ name: "e" }),
    ];
    const shown = chipAgents(input);
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
