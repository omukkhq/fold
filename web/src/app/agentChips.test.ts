import { describe, expect, it } from "vitest";
import type { ProjectAgent } from "../api/agentBar";
import { INITIAL_STATUS, type LiveStatus } from "../live/status";
import {
  MAX_CHIPS,
  agentBoardPath,
  agentStateText,
  chipAgents,
  chipStateText,
} from "./agentChips";

const agent = (extra: Partial<ProjectAgent>): ProjectAgent => ({
  name: "frontend",
  presence: "active",
  working: 0,
  stalled: 0,
  ...extra,
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

  it("counts working and stalled tasks", () => {
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
      chipStateText(agent({ working: 2 }), status({ state: "live" }), now),
    ).toBe("2 working");
  });

  it("shows a dash before the first connection", () => {
    expect(chipStateText(agent({ working: 2 }), INITIAL_STATUS, now)).toBe("—");
  });

  it("shows the time of the last sync to the minute while reconnecting", () => {
    expect(
      chipStateText(
        agent({ working: 2 }),
        status({ state: "reconnecting", lastEventAt: synced }),
        now,
      ),
    ).toBe("as of 14:12");
  });

  it("shows a dash while reconnecting when nothing was ever synced", () => {
    expect(
      chipStateText(agent({}), status({ state: "reconnecting" }), now),
    ).toBe("—");
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
