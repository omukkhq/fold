import { describe, expect, it } from "vitest";
import {
  activityText,
  activityVerb,
  agentOf,
  isStall,
  toldText,
  workerActivity,
  type LiveActivity,
} from "./workerActivity";

const NOW = 1_800_000_000_000;
const MINUTE = 60_000;

function seen(over: Partial<LiveActivity> = {}): LiveActivity {
  return {
    state: "working",
    slot: null,
    call: "command",
    since: NOW - 8 * MINUTE,
    supervised: true,
    told: [],
    parked_by: null,
    wait_for: null,
    ...over,
  };
}

describe("activityText", () => {
  it("says what the worker does while a call is open", () => {
    expect(activityText(seen(), "web-worker", NOW)).toBe(
      "Running a command for 8 min",
    );
    expect(activityText(seen({ call: "fold" }), "web-worker", NOW)).toBe(
      "Waiting on fold for 8 min",
    );
    expect(activityText(seen({ call: "subagent" }), "web-worker", NOW)).toBe(
      "Running a subagent for 8 min",
    );
    expect(activityText(seen({ call: null }), "web-worker", NOW)).toBe(
      "Running a tool for 8 min",
    );
  });

  it("says what fold saw of a stall", () => {
    const quiet = seen({
      state: "silent",
      slot: null,
      call: null,
      since: NOW - 12 * MINUTE,
    });
    expect(activityText(quiet, "web-worker", NOW)).toBe(
      "No activity from @web-worker for 12 min",
    );
    const long = seen({ state: "long_call", since: NOW - 25 * MINUTE });
    expect(activityText(long, "web-worker", NOW)).toBe(
      "A command of @web-worker has run for 25 min",
    );
    expect(activityText({ ...long, call: "tool" }, "web-worker", NOW)).toBe(
      "A tool call of @web-worker has run for 25 min",
    );
    expect(
      activityText(seen({ state: "failed", call: null }), "web-worker", NOW),
    ).toBe("@web-worker failed");
  });

  it("says it after the worker's name in the task strip", () => {
    expect(activityVerb(seen(), NOW)).toBe("running a command for 8 min");
    expect(activityVerb(seen({ state: "silent", call: null }), NOW)).toBe(
      "no activity for 8 min",
    );
    expect(activityVerb(seen({ state: "long_call" }), NOW)).toBe(
      "a command has run for 8 min",
    );
    expect(activityVerb(seen({ state: "stopped", call: null }), NOW)).toBe(
      "stopped",
    );
    expect(activityVerb(seen({ state: "failed", call: null }), NOW)).toBe(
      "failed",
    );
  });
});

describe("a process that ended", () => {
  it("is neutral when someone stopped it on purpose: no stall, no person, no restart", () => {
    const stopped = seen({ state: "stopped", call: null });
    expect(activityText(stopped, "web-worker", NOW)).toBe(
      "@web-worker stopped",
    );
    expect(isStall("stopped")).toBe(false);
    const view = workerActivity(
      { activity: stopped, stalled: true, worker: "web-worker" },
      NOW,
    );
    expect(view).toMatchObject({
      state: "stopped",
      stall: false,
      needsYou: false,
      restart: null,
    });
    expect(view === null ? "" : toldText(view)).toBeNull();
  });

  it("is a stall when it failed, which a person restarts once the stall waits for one", () => {
    const failed = seen({ state: "failed", call: null, told: ["web-lead"] });
    expect(isStall("failed")).toBe(true);
    expect(
      workerActivity(
        { activity: failed, stalled: false, worker: "web-worker" },
        NOW,
      ),
    ).toMatchObject({
      text: "@web-worker failed",
      stall: true,
      needsYou: false,
      restart: null,
    });
    expect(
      workerActivity(
        { activity: failed, stalled: true, worker: "web-worker" },
        NOW,
      ),
    ).toMatchObject({ stall: true, needsYou: true, restart: "web-worker" });
  });
});

describe("a resume that waits for a free slot", () => {
  const waits = seen({
    state: "waits_for_slot",
    call: null,
    slot: "task_worker",
    since: NOW - 30 * MINUTE,
  });

  it("says one text for every role, with no time", () => {
    expect(activityText(waits, "web-worker", NOW)).toBe(
      "Waiting for a free slot",
    );
    expect(activityText({ ...waits, slot: "reviewer" }, "review", NOW)).toBe(
      "Waiting for a free slot",
    );
    expect(activityVerb(waits, NOW)).toBe("waiting for a free slot");
  });

  it("is neutral: never a stall, never a person's, never a restart", () => {
    expect(isStall("waits_for_slot")).toBe(false);
    expect(isStall("working")).toBe(false);
    expect(isStall("silent")).toBe(true);
    const view = workerActivity(
      { activity: waits, stalled: true, worker: "web-worker" },
      NOW,
    );
    expect(view).toMatchObject({
      text: "Waiting for a free slot",
      stall: false,
      needsYou: false,
      restart: null,
    });
    expect(view === null ? "" : toldText(view)).toBeNull();
  });
});

describe("a turn that waits for the network or a usage limit", () => {
  const network = seen({
    state: "waits_for_network",
    call: null,
    since: NOW - 20 * MINUTE,
  });
  const limit = seen({ ...network, state: "waits_for_usage_limit" });

  it("says the wait with no time and no name", () => {
    expect(activityText(network, "web-worker", NOW)).toBe(
      "Waiting for the network",
    );
    expect(activityText(limit, "web-worker", NOW)).toBe(
      "Waits for the usage limit reset",
    );
    expect(activityVerb(network, NOW)).toBe("waiting for the network");
    expect(activityVerb(limit, NOW)).toBe("waits for the usage limit reset");
  });

  it("is neutral: never a stall, never a person's, never a restart, also once the daemon flags it", () => {
    for (const waits of [network, limit]) {
      expect(isStall(waits.state)).toBe(false);
      const view = workerActivity(
        { activity: waits, stalled: true, worker: "web-worker" },
        NOW,
      );
      expect(view).toMatchObject({
        stall: false,
        needsYou: false,
        restart: null,
      });
      expect(view === null ? "" : toldText(view)).toBeNull();
    }
  });
});

describe("a worker that waits for its lead's go", () => {
  const parked = seen({
    state: "parked",
    call: null,
    since: NOW - 40 * MINUTE,
    parked_by: "backend-lead",
    wait_for: "my go after the T-288 migration",
  });

  it("names the agent whose go it waits for, with no time", () => {
    expect(activityText(parked, "backend-worker", NOW)).toBe(
      "Waits for @backend-lead's go",
    );
    expect(activityVerb(parked, NOW)).toBe("waits for @backend-lead's go");
    expect(activityText({ ...parked, parked_by: null }, "w", NOW)).toBe(
      "Waits for its lead's go",
    );
  });

  it("is neutral: never a stall, never a person's, never a restart, also when the daemon flags it", () => {
    expect(isStall("parked")).toBe(false);
    const view = workerActivity(
      { activity: parked, stalled: true, worker: "backend-worker" },
      NOW,
    );
    expect(view).toMatchObject({
      state: "parked",
      stall: false,
      needsYou: false,
      restart: null,
      waitFor: "my go after the T-288 migration",
    });
    expect(view === null ? null : toldText(view)).toBe(
      "Stopped to wait: my go after the T-288 migration. The go resumes it in its session; nobody else needs to act",
    );
  });

  it("keeps only a park's own words: another state has none", () => {
    expect(
      workerActivity(
        {
          activity: seen({ wait_for: "stale words" }),
          stalled: false,
          worker: "w",
        },
        NOW,
      )?.waitFor,
    ).toBeNull();
  });

  it("counts a state this build does not know as no stall", () => {
    expect(isStall("later")).toBe(false);
  });
});

describe("workerActivity", () => {
  it("is null without a frame or without anything to say", () => {
    expect(workerActivity(undefined, NOW)).toBeNull();
    expect(workerActivity(null, NOW)).toBeNull();
    expect(
      workerActivity({ activity: null, stalled: false, worker: "w" }, NOW),
    ).toBeNull();
  });

  it("is never a stall or a restart while the worker works", () => {
    const working = workerActivity(
      { activity: seen(), stalled: true, worker: "web-worker" },
      NOW,
    );
    expect(working).toMatchObject({
      stall: false,
      needsYou: false,
      restart: null,
    });
  });

  it("waits for a person and offers the restart only once the daemon flags the stall, and restarts only a worker fold runs", () => {
    const silent = seen({ state: "silent", call: null, told: ["web-lead"] });
    const told = workerActivity(
      { activity: silent, stalled: false, worker: "web-worker" },
      NOW,
    );
    expect(told).toMatchObject({
      stall: true,
      needsYou: false,
      restart: null,
      told: ["web-lead"],
      since: silent.since,
      waitFor: null,
    });
    const flagged = workerActivity(
      { activity: silent, stalled: true, worker: "frontend/ui" },
      NOW,
    );
    expect(flagged).toMatchObject({ needsYou: true, restart: "frontend" });
    expect(
      workerActivity(
        {
          activity: { ...silent, supervised: false },
          stalled: true,
          worker: "frontend",
        },
        NOW,
      )?.restart,
    ).toBeNull();
  });
});

describe("toldText", () => {
  const base = {
    state: "silent" as const,
    text: "",
    stall: true,
    needsYou: false,
    restart: null,
    since: 0,
    waitFor: null,
  };

  it("names whom fold told, and says when a person must act", () => {
    expect(toldText({ ...base, told: ["web-lead"] })).toBe(
      "fold told @web-lead, who can start it again",
    );
    expect(toldText({ ...base, needsYou: true, told: ["web-lead"] })).toBe(
      "fold told @web-lead; it did not start again, so it waits for you",
    );
    expect(toldText({ ...base, needsYou: true, told: [] })).toBe(
      "No agent can act on it, so it waits for you",
    );
    expect(toldText({ ...base, stall: false, told: [] })).toBeNull();
  });
});

describe("agentOf", () => {
  it("is the agent of a worker label", () => {
    expect(agentOf("frontend/ui")).toBe("frontend");
    expect(agentOf("web-worker")).toBe("web-worker");
  });
});
