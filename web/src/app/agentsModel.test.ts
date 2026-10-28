import { describe, expect, it } from "vitest";
import type { ProjectAgent } from "../api/agentBar";
import { agent, claim, worker } from "../test/agents";
import {
  agentRowLook,
  agentsButton,
  agentsSummary,
  workingAgents,
} from "./agentsModel";

type Process = NonNullable<ProjectAgent["process"]>;

function supervised(
  name: string,
  state: Process["state"],
  extra: Partial<Process> = {},
): ProjectAgent {
  return {
    ...agent(name, { presence: "offline" }),
    process: {
      state,
      failed_reason: null,
      last_error: null,
      model_unreachable: false,
      ...extra,
    },
  };
}

describe("workingAgents", () => {
  it("counts agents whose process works or whose session is active", () => {
    const agents = [
      supervised("web-lead", "working"),
      supervised("api-lead", "idle"),
      agent("main", { presence: "active" }),
      agent("docs", { presence: "offline" }),
    ];
    expect(workingAgents(agents, false)).toBe(2);
  });

  it("counts only the main agent while the project is paused", () => {
    const agents = [
      supervised("web-lead", "working"),
      { ...agent("main", { presence: "active" }), tags: ["main"] },
    ];
    expect(workingAgents(agents, true)).toBe(1);
  });
});

describe("agentsButton", () => {
  it("says how many agents work while the stream is live", () => {
    expect(agentsButton("live", false, 0)).toEqual({
      label: "No agents working",
      tone: "live",
    });
    expect(agentsButton("live", false, 1).label).toBe("1 agent working");
    expect(agentsButton("live", false, 3).label).toBe("3 agents working");
  });

  it("marks a stream that is not live yet", () => {
    expect(agentsButton("connecting", false, 2)).toEqual({
      label: "2 agents working",
      tone: "connecting",
    });
  });

  it("says Reconnecting while the stream is down", () => {
    expect(agentsButton("reconnecting", false, 2)).toEqual({
      label: "Reconnecting…",
      tone: "reconnecting",
    });
  });

  it("says the agents are paused before anything else", () => {
    expect(agentsButton("reconnecting", true, 0)).toEqual({
      label: "Agents paused",
      tone: "paused",
    });
  });
});

describe("agentRowLook", () => {
  it("fills the dot of a working agent and rings an idle or offline one", () => {
    expect(agentRowLook(supervised("a", "working"), false)).toEqual({
      dot: "work",
      tone: null,
    });
    expect(agentRowLook(supervised("a", "idle"), false).dot).toBe("idle");
    expect(agentRowLook(agent("a", { presence: "offline" }), false).dot).toBe(
      "off",
    );
  });

  it("shows a failed process or a model it cannot reach in red", () => {
    expect(agentRowLook(supervised("a", "failed"), false)).toEqual({
      dot: "fail",
      tone: "bad",
    });
    expect(
      agentRowLook(
        supervised("a", "working", { model_unreachable: true }),
        false,
      ),
    ).toEqual({ dot: "fail", tone: "bad" });
  });

  it("marks stalled work in gold, but not while the agents are paused", () => {
    const stalled: ProjectAgent = {
      ...agent("a", {
        workers: [worker("a", { claims: [claim({ task: "T-1" })] })],
      }),
      attention: [{ kind: "stalled", task: "T-1" }],
    };
    expect(agentRowLook(stalled, false).tone).toBe("warn");
    expect(agentRowLook(stalled, true)).toEqual({ dot: "off", tone: null });
  });
});

describe("agentsSummary", () => {
  it("says how many work of how many", () => {
    expect(agentsSummary(3, 8)).toBe("3 working · 8 in all");
  });
});
